#![cfg(target_os = "android")]
mod permissions;
mod wifi_manager;

use std::println;

use alxr_common::{
    alxr_destroy, alxr_init, alxr_is_session_running, alxr_on_pause, alxr_on_resume,
    alxr_process_frame, battery_send, init_connections, input_send, path_string_to_hash,
    request_idr, set_waiting_next_idr, shutdown, time_sync_send, video_error_report_send,
    views_config_send, ALXRColorSpace, ALXRDecoderType, ALXRGraphicsApi, ALXRRustCtx,
    ALXRSystemProperties, ALXRVersion, APP_CONFIG,
};
use permissions::check_android_permissions;
use wifi_manager::{acquire_wifi_lock, release_wifi_lock};

use ndk::looper::*;
use ndk_context;
use ndk_glue;
use version_compare::{Part, Version};

struct AppData {
    destroy_requested: bool,
    resumed: bool,
}

impl AppData {
    fn handle_lifecycle_event(&mut self, event: &ndk_glue::Event) {
        // Start,
        // Resume,
        // SaveInstanceState,
        // Pause,
        // Stop,
        // Destroy,
        // ConfigChanged,
        // LowMemory,
        // WindowLostFocus,
        // WindowHasFocus,
        // WindowCreated,
        // WindowResized,
        // WindowRedrawNeeded,
        // WindowDestroyed,
        // InputQueueCreated,
        // InputQueueDestroyed,
        // ContentRectChanged,
        match event {
            // ndk_glue::Event::WindowCreated => {
            //     let win = ndk_glue::native_window();
            //     if let Some(win2) = win.as_ref() {
            //         let width = win2.width();
            //         let height = win2.height();
            //         println!("NativeWindow: width: {0}, height: {1}", width, height);
            //     }
            // },
            ndk_glue::Event::Pause => {
                println!("alxr-client: received on_pause event.");
                self.resumed = false;
                unsafe { alxr_on_pause() };
                release_wifi_lock();
            }
            ndk_glue::Event::Resume => {
                println!("alxr-client: received on_resume event.");
                acquire_wifi_lock();
                unsafe { alxr_on_resume() };
                self.resumed = true;
            }
            ndk_glue::Event::Destroy => self.destroy_requested = true,
            _ => (),
        }
    }
}

fn get_build_property<'a>(jvm: &'a jni::JavaVM, property_name: &str) -> String {
    let env = jvm.attach_current_thread().unwrap();

    let jdevice_name = env
        .get_static_field("android/os/Build", &property_name, "Ljava/lang/String;")
        .unwrap()
        .l()
        .unwrap();
    let device_name_raw = env.get_string(jdevice_name.into()).unwrap();

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

#[cfg_attr(target_os = "android", ndk_glue::main(backtrace = "on"))]
pub fn main() {
    println!("{:?}", *APP_CONFIG);
    let mut app = AppData {
        destroy_requested: false,
        resumed: false,
    };
    run(&mut app).unwrap();
    println!("successfully shutdown.");
    // the ndk_glue api does not automatically call this and without
    // it main will hang on exit, currently there seems to be no plans to
    // make it automatic, refer to:
    // https://github.com/rust-windowing/android-ndk-rs/issues/154
    ndk_glue::native_activity().finish();
}

pub fn poll_all_ms(block: bool) -> Option<ndk_glue::Event> {
    let looper = ThreadLooper::for_thread().unwrap();
    let result = if block {
        looper.poll_all()
    } else {
        looper.poll_all_timeout(std::time::Duration::from_millis(0u64))
    };
    match result {
        Ok(Poll::Event { ident, .. }) => match ident {
            ndk_glue::NDK_GLUE_LOOPER_EVENT_PIPE_IDENT => ndk_glue::poll_events(),
            ndk_glue::NDK_GLUE_LOOPER_INPUT_QUEUE_IDENT => {
                let input_queue = ndk_glue::input_queue();
                let input_queue = input_queue.as_ref().expect("Input queue not attached!");
                assert!(input_queue.has_events());
                while let Some(event) = input_queue.get_event().unwrap() {
                    if let Ok(Some(event)) =
                        std::panic::catch_unwind(|| input_queue.pre_dispatch(event))
                    {
                        input_queue.finish_event(event, false);
                    }
                }
                None
            }
            _ => unreachable!("Unrecognized looper identifer"),
        },
        _ => None,
    }
}

fn run(app_data: &mut AppData) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let ctx = ndk_context::android_context();
        let native_activity = ctx.context();
        let vm_ptr = ctx.vm();

        let _lib = libloading::Library::new("libopenxr_loader.so")?;

        let vm = jni::JavaVM::from_raw(vm_ptr.cast())?;
        let _env = vm.attach_current_thread()?;

        check_android_permissions(native_activity as jni::sys::jobject, &vm)?;

        let ctx = ALXRRustCtx {
            graphicsApi: APP_CONFIG.graphics_api.unwrap_or(ALXRGraphicsApi::Auto),
            decoderType: ALXRDecoderType::NVDEC, // Not used on android.
            displayColorSpace: APP_CONFIG.color_space.unwrap_or(ALXRColorSpace::Rec2020),
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
            firmwareVersion: get_firmware_version(&vm),
        };
        let mut sys_properties = ALXRSystemProperties::new();
        if !alxr_init(&ctx, &mut sys_properties) {
            return Ok(());
        }

        if let Some((eye_w, eye_h)) = get_preferred_resolution(&vm, &sys_properties) {
            println!("ALXR: Overriding recommend eye resolution ({}x{}) with prefferred resolution ({eye_w}x{eye_h})",
                        sys_properties.recommendedEyeWidth, sys_properties.recommendedEyeHeight);
            sys_properties.recommendedEyeWidth = eye_w;
            sys_properties.recommendedEyeHeight = eye_h;
        }
        init_connections(&sys_properties);

        while !app_data.destroy_requested {
            // Main game loop
            loop {
                // event pump loop
                let block =
                    !app_data.destroy_requested && !app_data.resumed && !alxr_is_session_running();
                // If the timeout is zero, returns immediately without blocking.
                // If the timeout is negative, waits indefinitely until an event appears.
                if let Some(event) = poll_all_ms(block) {
                    app_data.handle_lifecycle_event(&event);
                } else {
                    break;
                }
            }
            // update and render
            let mut exit_render_loop = false;
            let mut request_restart = false;
            alxr_process_frame(&mut exit_render_loop, &mut request_restart);
            if exit_render_loop {
                break;
            }
        }

        shutdown();
        alxr_destroy();
    }
    Ok(())
}
