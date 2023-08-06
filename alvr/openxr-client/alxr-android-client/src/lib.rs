#![cfg(target_os = "android")]
mod permissions;
mod wifi_manager;

use permissions::check_android_permissions;
use std::time::Duration;
use version_compare::{Part, Version};
use wifi_manager::{acquire_wifi_lock, release_wifi_lock};

use android_activity::{AndroidApp, MainEvent, PollEvent};
use android_logger;

use alxr_common::{
    alxr_destroy, alxr_init, alxr_on_pause, alxr_on_resume, alxr_process_frame, battery_send,
    init_connections, input_send, path_string_to_hash, request_idr, set_waiting_next_idr, shutdown,
    time_sync_send, video_error_report_send, views_config_send, ALXRClientCtx, ALXRColorSpace,
    ALXRDecoderType, ALXREyeTrackingType, ALXRFacialExpressionType, ALXRGraphicsApi,
    ALXRSystemProperties, ALXRVersion, APP_CONFIG,
};

fn get_build_property<'a>(jvm: &'a jni::JavaVM, property_name: &str) -> String {
    let mut env = jvm.attach_current_thread().unwrap();

    let jdevice_name = env
        .get_static_field("android/os/Build", &property_name, "Ljava/lang/String;")
        .unwrap()
        .l()
        .unwrap();
    let device_name_raw = env.get_string((&jdevice_name).into()).unwrap();

    device_name_raw.to_string_lossy().as_ref().to_owned()
}

fn get_firmware_version<'a>(jvm: &'a jni::JavaVM) -> ALXRVersion {
    fn get_version_helper<'a, 'b>(jvm: &'a jni::JavaVM, prop_name: &str) -> Option<[u32; 3]> {
        let value_str = get_build_property(&jvm, &prop_name);
        match Version::from(&value_str) {
            Some(v) => {
                let mut ret: [u32; 3] = [0, 0, 0];
                for idx in 0..3 {
                    match v.part(idx) {
                        Ok(Part::Number(val)) => ret[idx] = val as u32,
                        _ => (),
                    }
                }
                Some(ret)
            }
            _ => None,
        }
    }

    let version = get_version_helper(&jvm, "ID")
        .unwrap_or_else(|| get_version_helper(&jvm, "DISPLAY").unwrap_or([0, 0, 0]));

    ALXRVersion {
        major: version[0],
        minor: version[1],
        patch: version[2],
    }
}

fn get_build_model<'a>(jvm: &'a jni::JavaVM) -> String {
    get_build_property(&jvm, "MODEL")
}

fn get_preferred_resolution<'a>(
    jvm: &'a jni::JavaVM,
    sys_prop: &ALXRSystemProperties,
) -> Option<(u32, u32)> {
    let sys_name = sys_prop.system_name();
    let model_name = get_build_model(&jvm);
    for name in [sys_name, model_name] {
        match name.as_str() {
            //"XR Elite" => return Some((1920, 1920)),
            //"Focus 3" => return Some((2448, 2448)),
            "Lynx" => return Some((1600, 1600)),
            "Meta Quest Pro" => return Some((1800, 1920)),
            "Pico Neo 3" | "Pico Neo 3 Link" | "Oculus Quest2" | "Oculus Quest 2" => {
                return Some((1832, 1920))
            }
            "Oculus Quest" => return Some((1440, 1600)),
            "Pico 4" | "A8150" => return Some((2160, 2160)),
            _ => (),
        }
    }
    None
}

#[no_mangle]
fn android_main(android_app: AndroidApp) {
    let log_level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    android_logger::init_once(android_logger::Config::default().with_max_level(log_level));
    log::info!("{:?}", *APP_CONFIG);
    unsafe { run(&android_app).unwrap() };
    log::info!("successfully shutdown.");
}

struct AppData {
    destroy_requested: bool,
    resumed: bool,
    gained_focus: bool,
    window_inited: bool,
}

impl AppData {
    fn pasue(&mut self) {
        self.resumed = false;
        log::debug!("alxr-client: received on_pause event.");
        unsafe { alxr_on_pause() };
        release_wifi_lock();
    }

    fn resume(&mut self) {
        acquire_wifi_lock();
        log::debug!("alxr-client: received on_resume event.");
        unsafe { alxr_on_resume() };
        self.resumed = true;
    }

    fn handle_lifecycle_event(&mut self, event: &PollEvent) {
        match event {
            PollEvent::Main(main_event) => match main_event {
                MainEvent::InitWindow { .. } => self.window_inited = true,
                MainEvent::LostFocus => self.gained_focus = false,
                MainEvent::GainedFocus => self.gained_focus = true,
                MainEvent::Pause => self.pasue(),
                MainEvent::Resume { .. } => self.resume(),
                MainEvent::Destroy => self.destroy_requested = true,
                _ => (),
            },
            // PollEvent::Wake  => (),
            // PollEvent::Timeout => (),
            _ => (),
        }
    }
}

#[inline(always)]
fn wait_until_window_init(android_app: &AndroidApp, app_data: &mut AppData) {
    while !app_data.destroy_requested && !app_data.window_inited {
        log::info!("Waiting for native-window to initialize...");
        android_app.poll_events(Some(Duration::from_millis(100)), |event| {
            app_data.handle_lifecycle_event(&event);
        });
    }
    let msg = if app_data.window_inited {
        "successfully."
    } else {
        "never"
    };
    log::info!("native-window {msg} initialized.");
}

const NO_WAIT_TIME: Option<Duration> = Some(Duration::from_millis(0));

#[inline(always)]
unsafe fn run(android_app: &AndroidApp) -> Result<(), Box<dyn std::error::Error>> {
    let _lib = libloading::Library::new("libopenxr_loader.so")?;

    let native_activity = android_app.activity_as_ptr();
    let vm_ptr = android_app.vm_as_ptr();

    let vm = jni::JavaVM::from_raw(vm_ptr.cast())?;
    let _env = vm.attach_current_thread()?;

    check_android_permissions(native_activity as jni::sys::jobject, &vm)?;

    let mut app_data = AppData {
        destroy_requested: false,
        resumed: false,
        gained_focus: false,
        window_inited: false,
    };
    wait_until_window_init(&android_app, &mut app_data);
    if app_data.destroy_requested {
        return Ok(());
    }
    assert!(app_data.window_inited);
    log::debug!("alxr-client: is activity paused? {0} ", !app_data.resumed);

    let ctx = ALXRClientCtx {
        graphicsApi: APP_CONFIG.graphics_api.unwrap_or(ALXRGraphicsApi::Auto),
        decoderType: ALXRDecoderType::NVDEC, // Not used on android.
        displayColorSpace: APP_CONFIG.color_space.unwrap_or(ALXRColorSpace::Default),
        verbose: APP_CONFIG.verbose,
        applicationVM: vm_ptr as *mut std::ffi::c_void,
        applicationActivity: native_activity,
        inputSend: Some(input_send),
        viewsConfigSend: Some(views_config_send),
        pathStringToHash: Some(path_string_to_hash),
        timeSyncSend: Some(time_sync_send),
        videoErrorReportSend: Some(video_error_report_send),
        batterySend: Some(battery_send),
        setWaitingNextIDR: Some(set_waiting_next_idr),
        requestIDR: Some(request_idr),
        disableLinearizeSrgb: APP_CONFIG.no_linearize_srgb,
        noSuggestedBindings: APP_CONFIG.no_bindings,
        noServerFramerateLock: APP_CONFIG.no_server_framerate_lock,
        noFrameSkip: APP_CONFIG.no_frameskip,
        disableLocalDimming: APP_CONFIG.disable_localdimming,
        headlessSession: APP_CONFIG.headless_session,
        noPassthrough: APP_CONFIG.no_passthrough,
        noFTServer: APP_CONFIG.no_tracking_server,
        noHandTracking: APP_CONFIG.no_hand_tracking,
        facialTracking: APP_CONFIG
            .facial_tracking
            .unwrap_or(ALXRFacialExpressionType::Auto),
        eyeTracking: APP_CONFIG.eye_tracking.unwrap_or(ALXREyeTrackingType::Auto),
        firmwareVersion: get_firmware_version(&vm),
        trackingServerPortNo: APP_CONFIG.tracking_server_port_no,
    };
    let mut sys_properties = ALXRSystemProperties::new();
    if !alxr_init(&ctx, &mut sys_properties) {
        return Ok(());
    }

    if let Some((eye_w, eye_h)) = get_preferred_resolution(&vm, &sys_properties) {
        log::info!("ALXR: Overriding recommend eye resolution ({}x{}) with prefferred resolution ({eye_w}x{eye_h})",
                    sys_properties.recommendedEyeWidth, sys_properties.recommendedEyeHeight);
        sys_properties.recommendedEyeWidth = eye_w;
        sys_properties.recommendedEyeHeight = eye_h;
    }
    init_connections(&sys_properties);

    while !app_data.destroy_requested {
        android_app.poll_events(NO_WAIT_TIME, |event| {
            app_data.handle_lifecycle_event(&event);
        });

        let mut exit_render_loop = false;
        let mut request_restart = false;
        alxr_process_frame(&mut exit_render_loop, &mut request_restart);
        if exit_render_loop {
            break;
        }
    }

    shutdown();
    alxr_destroy();
    Ok(())
}
