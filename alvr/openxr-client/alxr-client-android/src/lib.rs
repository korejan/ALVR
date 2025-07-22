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
    time_sync_send, to_alxr_version, video_error_report_send, views_config_send, ALXRClientCtx,
    ALXRColorSpace, ALXRDecoderType, ALXREyeTrackingType, ALXRFacialExpressionType,
    ALXRGraphicsApi, ALXRPassthroughMode, ALXRSystemProperties, ALXRVersion, APP_CONFIG,
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

fn get_build_version_property<'a>(jvm: &'a jni::JavaVM, property_name: &str) -> String {
    let mut env = jvm.attach_current_thread().unwrap();

    let version_prop_name = env
        .get_static_field(
            "android/os/Build$VERSION",
            &property_name,
            "Ljava/lang/String;",
        )
        .unwrap()
        .l()
        .unwrap();
    let version_prop_raw = env.get_string((&version_prop_name).into()).unwrap();

    version_prop_raw.to_string_lossy().as_ref().to_owned()
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

#[allow(dead_code)]
fn get_build_model<'a>(jvm: &'a jni::JavaVM) -> String {
    get_build_property(&jvm, "MODEL")
}

#[allow(dead_code)]
fn get_build_device<'a>(jvm: &'a jni::JavaVM) -> String {
    get_build_property(&jvm, "DEVICE")
}

#[allow(dead_code)]
fn get_build_manufacturer<'a>(jvm: &'a jni::JavaVM) -> String {
    get_build_property(&jvm, "MANUFACTURER")
}

#[allow(dead_code)]
fn get_build_version_no<'a>(jvm: &'a jni::JavaVM) -> u64 {
    get_build_version_property(&jvm, "INCREMENTAL")
        .parse()
        .unwrap_or(0)
}

#[allow(dead_code)]
fn is_device<'a>(pname: &str, jvm: &'a jni::JavaVM) -> bool {
    let key = pname.to_lowercase();
    let model_name = get_build_model(&jvm).to_lowercase();
    let device_name = get_build_device(&jvm).to_lowercase();
    let man_name = get_build_manufacturer(&jvm).to_lowercase();
    for dname in [model_name, device_name, man_name] {
        if dname.contains(&key) {
            return true;
        }
    }
    false
}

#[allow(dead_code)]
fn is_android_emulator<'a>(jvm: &'a jni::JavaVM) -> bool {
    let device_name = get_build_device(&jvm).to_lowercase();
    device_name.starts_with("emulator64_")
}

#[allow(dead_code)]
fn print_device_info<'a>(jvm: &'a jni::JavaVM) {
    let model_name = get_build_model(&jvm);
    let device_name = get_build_device(&jvm);
    let man_name = get_build_manufacturer(&jvm);
    let build_id = get_build_property(&jvm, "ID");
    let version_incremental = get_build_version_no(&jvm);
    log::info!("           Device Details");
    log::info!("======================================");
    log::info!("model:                {0}", model_name);
    log::info!("device:               {0}", device_name);
    log::info!("manufacturer:         {0}", man_name);
    log::info!("build-id:             {0}", build_id);
    log::info!("version-incr:         {0}", version_incremental);
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
    sys_properties: Option<ALXRSystemProperties>,
}

impl AppData {
    fn pause(&mut self) {
        self.resumed = false;
        if self.sys_properties.is_some() {
            shutdown();
        }
        unsafe { alxr_on_pause() };
        release_wifi_lock();
    }

    fn resume(&mut self) {
        acquire_wifi_lock();
        unsafe { alxr_on_resume() };
        if let Some(sys_properties) = self.sys_properties {
            init_connections(&sys_properties);
        }
        self.resumed = true;
    }

    fn handle_lifecycle_event(&mut self, android_app: &AndroidApp, event: &PollEvent) {
        match event {
            PollEvent::Main(main_event) => match main_event {
                MainEvent::InitWindow { .. } => self.window_inited = true,
                MainEvent::WindowResized { .. } => {
                    let window = android_app.native_window().unwrap();
                    log::info!(
                        "alxr-client: received windows resize event, size: {0}x{1}",
                        window.width(),
                        window.height()
                    );
                }
                MainEvent::LostFocus => {
                    log::info!("alxr-client: received lost_focus event.");
                    self.gained_focus = false;
                }
                MainEvent::GainedFocus => {
                    log::info!("alxr-client: received gained_focus event.");
                    self.gained_focus = true;
                }
                MainEvent::Pause => {
                    log::info!("alxr-client: received pause event.");
                    self.pause();
                }
                MainEvent::Resume { .. } => {
                    log::info!("alxr-client: received resume event.");
                    self.resume();
                }
                MainEvent::Destroy => self.destroy_requested = true,
                _ => (),
            },
            //PollEvent::Wake  => { log::info!("alxr-client: received wake event."); },
            //PollEvent::Timeout => { log::info!("alxr-client: received timeout event."); },
            _ => (),
        }
    }
}

#[inline(always)]
fn wait_until_window_init(android_app: &AndroidApp, app_data: &mut AppData) {
    while !app_data.destroy_requested && !app_data.window_inited {
        log::info!("Waiting for native-window to initialize...");
        android_app.poll_events(Some(Duration::from_millis(100)), |event| {
            app_data.handle_lifecycle_event(&android_app, &event);
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
        sys_properties: None,
    };
    wait_until_window_init(&android_app, &mut app_data);
    if app_data.destroy_requested || android_app.native_window().is_none() {
        return Ok(());
    }
    assert!(app_data.window_inited && android_app.native_window().is_some());
    log::debug!("alxr-client: is activity paused? {0} ", !app_data.resumed);

    let no_linearize_srgb = APP_CONFIG.no_linearize_srgb || is_device("Lynx", &vm);
    log::info!("alxr-client: Disable shader gamma/sRGB linearization? {no_linearize_srgb}");

    print_device_info(&vm);

    let mut eye_tracking_type = APP_CONFIG.eye_tracking.unwrap_or(ALXREyeTrackingType::Auto);
    // quest firmware version 71.0.0.178.498 has a crash bug in `xrSyncActions` when
    // `XR_EXT_eye_gaze_interaction` extension is enabled.
    match eye_tracking_type {
        ALXREyeTrackingType::Auto | ALXREyeTrackingType::ExtEyeGazeInteraction => {
            let build_id = get_build_property(&vm, "ID");
            match build_id.as_str() {
                "UP1A.231005.007.A1" | "SQ3A.220605.009.A1" => {
                    log::warn!("alxr-client: override eye-tracking type workaround enabled.");
                    eye_tracking_type = ALXREyeTrackingType::FBEyeTrackingSocial;
                }
                _ => {}
            };
        }
        _ => {}
    };

    let no_multi_view_rendering = APP_CONFIG.no_multi_view_rendering || is_android_emulator(&vm);

    let mut no_visibility_masks = APP_CONFIG.no_visibility_masks;
    // quest firmware v77.0.0.x has a crash bug when using `XR_KHR_visibility_mask`
    let build_no = get_build_version_no(&vm);
    match build_no {
        50801630051100340 | 50801630046600340 => {
            // quest v77.0.0.x
            log::warn!("alxr-client: force disabling XR_KHR_visibility_mask, quest crash bug workaround for build-no.: {build_no}");
            no_visibility_masks = true
        }
        _ => {}
    };

    let xr_api_version = if is_device("Quest", &vm) {
        // Quest bug workaround, if OpenXR apiVersion is >= 1.[0|1].49, controller aim poses are broken.
        semver::Version::new(1, 0, 48)
    } else {
        APP_CONFIG
            .xr_api_version
            .clone()
            .unwrap_or(semver::Version::new(0, 0, 0))
    };

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
        disableLinearizeSrgb: no_linearize_srgb,
        noSuggestedBindings: APP_CONFIG.no_bindings,
        noServerFramerateLock: APP_CONFIG.no_server_framerate_lock,
        noFrameSkip: APP_CONFIG.no_frameskip,
        disableLocalDimming: APP_CONFIG.disable_localdimming,
        headlessSession: APP_CONFIG.headless_session,
        noPassthrough: APP_CONFIG.no_passthrough,
        noFTServer: APP_CONFIG.no_tracking_server,
        noHandTracking: APP_CONFIG.no_hand_tracking,
        faceTrackingDataSources: APP_CONFIG.get_face_tracking_data_source_flags(),
        facialTracking: APP_CONFIG
            .facial_tracking
            .unwrap_or(ALXRFacialExpressionType::Auto),
        eyeTracking: eye_tracking_type,
        firmwareVersion: get_firmware_version(&vm),
        trackingServerPortNo: APP_CONFIG.tracking_server_port_no,
        simulateHeadless: APP_CONFIG.simulate_headless,
        passthroughMode: APP_CONFIG
            .passthrough_mode
            .unwrap_or(ALXRPassthroughMode::None),
        internalDataPath: std::ptr::null(),
        noVisibilityMasks: no_visibility_masks,
        noMultiviewRendering: no_multi_view_rendering,
        xrApiVersion: to_alxr_version(&xr_api_version),
    };
    let mut sys_properties = ALXRSystemProperties::new();
    if !alxr_init(&ctx, &mut sys_properties) {
        return Ok(());
    }

    let window = android_app.native_window().unwrap();
    log::info!(
        "alxr-client: window-size={0}x{1}",
        window.width(),
        window.height()
    );

    let (eye_w, eye_h) = ((window.width() / 2) as u32, window.height() as u32);
    log::info!("alxr-client: Overriding OpeXR recommend eye resolution ({}x{}) with preferred resolution ({eye_w}x{eye_h})",
                sys_properties.recommendedEyeWidth, sys_properties.recommendedEyeHeight);
    sys_properties.recommendedEyeWidth = eye_w;
    sys_properties.recommendedEyeHeight = eye_h;

    init_connections(&sys_properties);
    app_data.sys_properties = Some(sys_properties);

    while !app_data.destroy_requested {
        android_app.poll_events(NO_WAIT_TIME, |event| {
            app_data.handle_lifecycle_event(&android_app, &event);
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
