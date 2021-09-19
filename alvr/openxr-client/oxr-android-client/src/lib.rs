use oxr_common::{
    init_connections, isOpenXRSessionRunning, legacy_send, openxrInit, openxrProcesFrame,
    openxrDestroy, shutdown, GraphicsCtxApi, RustCtx //, openxrRequestExitSession
};

use ndk::looper::*;
use ndk_glue;

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
            ndk_glue::Event::Pause => self.resumed = false,
            ndk_glue::Event::Resume => self.resumed = true,
            ndk_glue::Event::Destroy => {
                self.destroy_requested = true
            }
            _ => (),
        }
    }
}

#[cfg_attr(target_os = "android", ndk_glue::main(backtrace = "on"))]
pub fn main() {
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
                if let Some(input_queue) = ndk_glue::input_queue().as_ref() {
                    while let Some(event) = input_queue.get_event() {
                        if let Some(event) = input_queue.pre_dispatch(event) {
                            input_queue.finish_event(event, false);
                        }
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
        let native_activity = ndk_glue::native_activity();
        let vm_ptr = native_activity.vm();

        let _lib = libloading::Library::new("libopenxr_loader.so")?;

        let vm = jni::JavaVM::from_raw(vm_ptr)?;
        let _env = vm.attach_current_thread()?;

        let ctx = RustCtx {
            graphicsApi: GraphicsCtxApi::Auto,
            verbose: false,
            applicationVM: vm_ptr as *mut std::ffi::c_void,
            applicationActivity: (*native_activity.ptr().as_ptr()).clazz as *mut std::ffi::c_void,
            initConnections: Some(init_connections),
            legacySend: Some(legacy_send),
        };
        if !openxrInit(&ctx) {
            shutdown();
            return Ok(())
        }
        
        while !app_data.destroy_requested {
            // Main game loop
            loop {
                // event pump loop
                let block = !app_data.destroy_requested &&
                            !app_data.resumed &&
                            !isOpenXRSessionRunning();
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
            openxrProcesFrame(&mut exit_render_loop, &mut request_restart);
            if exit_render_loop {
                break;
            }
        }
        
        shutdown();
        openxrDestroy();
        
        vm.detach_current_thread();
    }
    Ok(())
}
