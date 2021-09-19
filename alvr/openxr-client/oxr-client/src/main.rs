use oxr_common::{
    init_connections, isOpenXRSessionRunning, legacy_send, openxrDestroy, openxrInit,
    openxrProcesFrame, shutdown, GraphicsCtxApi, RustCtx, APP_CONFIG,
};
use std::{thread, time};

const SLEEP_TIME: time::Duration = time::Duration::from_millis(250);

#[cfg(not(target_os = "android"))]
fn main() {
    println!("{:?}", *APP_CONFIG);
    let selected_api = APP_CONFIG.graphics_api.unwrap_or(GraphicsCtxApi::Auto);
    unsafe {
        loop {
            let ctx = RustCtx {
                initConnections: Some(init_connections),
                legacySend: Some(legacy_send),
                graphicsApi: selected_api,
                verbose: APP_CONFIG.verbose,
            };
            if !openxrInit(&ctx) {
                break;
            }

            let mut request_restart = false;
            loop {
                let mut exit_render_loop = false;
                openxrProcesFrame(&mut exit_render_loop, &mut request_restart);
                if exit_render_loop {
                    break;
                }
                if !isOpenXRSessionRunning() {
                    // Throttle loop since xrWaitFrame won't be called.
                    thread::sleep(SLEEP_TIME);
                }
            }

            shutdown();
            openxrDestroy();

            if !request_restart {
                break;
            }
        }
    }
    println!("successfully shutdown.");
}
