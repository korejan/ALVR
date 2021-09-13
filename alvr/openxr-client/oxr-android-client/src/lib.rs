use oxr_common:: {
    RustCtx,
    GraphicsCtxApi,
    init_connections,
    legacy_send,
    openxrInit,
    isOpenXRSessionRunning,
    openxrProcesFrame,
    APP_CONFIG
};

use ndk::looper::*;
use ndk_glue;
use ndk_sys;

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
            ndk_glue::Event::Resume => self.resumed = true,
            ndk_glue::Event::Stop | ndk_glue::Event::Destroy | ndk_glue::Event::WindowDestroyed => {
                self.destroy_requested = true
            }
            _ => self.destroy_requested = false,
        }
    }
}

#[cfg_attr(target_os = "android", ndk_glue::main(backtrace = "on"))]
pub fn main() {
    let mut app = AppData {
        destroy_requested: false,
        resumed: false,
    };
    test(&mut app).unwrap();
    // the ndk_glue api does not automatically call this and without
    // it main will hang on exit, currently there seems to be no plans to
    // make it automatic, refer to:
    // https://github.com/rust-windowing/android-ndk-rs/issues/154
    ndk_glue::native_activity().finish();
}

pub const LOOPER_ID_MAIN: u32 = 0;
pub const LOOPER_ID_INPUT: u32 = 1;

pub fn poll_all_ms(block: bool) -> Option<ndk_glue::Event> {
    let looper = ThreadLooper::for_thread().unwrap();
    let result = if block {
        let result = looper.poll_all();
        result
    } else {
        looper.poll_all_timeout(std::time::Duration::from_millis(0u64))
    };

    match result {
        Ok(Poll::Event { ident, .. }) => {
            let ident = ident as u32;
            if ident == LOOPER_ID_MAIN {
                ndk_glue::poll_events()
            } else if ident == LOOPER_ID_INPUT {
                if let Some(input_queue) = ndk_glue::input_queue().as_ref() {
                    while let Some(event) = input_queue.get_event() {
                        if let Some(event) = input_queue.pre_dispatch(event) {
                            input_queue.finish_event(event, false);
                        }
                    }
                }
                None
            } else {
                unreachable!("Unrecognized looper identifer");
            }
        }
        _ => None,
    }
}

fn test(app_data: &mut AppData) -> Result<(), Box<dyn std::error::Error>> {
    // Create a VM for executing Java calls
    let native_activity = ndk_glue::native_activity();
    let vm_ptr = native_activity.vm();
    let vm = unsafe { jni::JavaVM::from_raw(vm_ptr) }?;

    unsafe {
        match libloading::Library::new("libopenxr_loader.so") {
            Err(e) => {
                std::eprintln!("failed to load libopenxr_loader.so, reason: {0}", e)
            }
            _ => std::println!("libopenxr_loader.so loaded."),
        }
        // match libloading::Library::new("libc++_shared.so") {
        //     Err(e) => { std::eprintln!("failed to load libc++_shared.so, reason: {0}", e) }
        //     _ => std::println!("libc++_shared.so loaded.")
        // }
        // match libloading::Library::new("libopenxr_monado.so") {
        //     Err(e) => { std::eprintln!("failed to load libopenxr_monado.so, reason: {0}", e) }
        //     _ => std::println!("libopenxr_monado.so loaded.")
        // }
    }

    let env = vm.attach_current_thread()?;

    unsafe {
        let ctx = RustCtx {
            graphicsApi: GraphicsCtxApi::Auto,
            applicationVM: vm_ptr as *mut std::ffi::c_void,
            applicationActivity: (*native_activity.ptr().as_ptr()).clazz as *mut std::ffi::c_void,
            initConnections: Some(init_connections),
            legacySend: Some(legacy_send),
        };
        openxrInit(&ctx);
    }

    while !app_data.destroy_requested {
        // Main game loop
        loop {
            // event pump loop
            let block = !app_data.destroy_requested
                && !app_data.resumed
                && unsafe { !isOpenXRSessionRunning() }; // && app.ovr.is_none();
                                                                // If the timeout is zero, returns immediately without blocking.
                                                                // If the timeout is negative, waits indefinitely until an event appears.
                                                                // const int timeoutMilliseconds =
                                                                //     (!appState.Resumed && !program->IsSessionRunning() && app->destroyRequested == 0) ? -1 : 0;

            if let Some(event) = poll_all_ms(block) {
                //trace!("event: {:?}", event);
                app_data.handle_lifecycle_event(&event);
                //app.update_vr_mode();
            } else {
                break;
            }
        }
        // update and render
        unsafe {
            openxrProcesFrame();
        }
    }
    Ok(())
}