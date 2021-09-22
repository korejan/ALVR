use oxr_common::{
    alxr_destroy, alxr_init, alxr_is_session_running, alxr_process_frame, init_connections,
    legacy_send, shutdown, ALXRGraphicsApi, ALXRRustCtx, ALXRSystemProperties, APP_CONFIG,
};
use std::{thread, time};

const SLEEP_TIME: time::Duration = time::Duration::from_millis(250);

#[cfg(not(target_os = "android"))]
fn main() {
    println!("{:?}", *APP_CONFIG);
    let selected_api = APP_CONFIG.graphics_api.unwrap_or(ALXRGraphicsApi::Auto);
    unsafe {
        loop {
            let ctx = ALXRRustCtx {
                legacySend: Some(legacy_send),
                graphicsApi: selected_api,
                verbose: APP_CONFIG.verbose,
            };
            let mut sys_properties = ALXRSystemProperties::new();
            if !alxr_init(&ctx, &mut sys_properties) {
                break;
            }
            init_connections(&sys_properties);

            let mut request_restart = false;
            loop {
                let mut exit_render_loop = false;
                alxr_process_frame(&mut exit_render_loop, &mut request_restart);
                if exit_render_loop {
                    break;
                }
                if !alxr_is_session_running() {
                    // Throttle loop since xrWaitFrame won't be called.
                    thread::sleep(SLEEP_TIME);
                }
            }

            shutdown();
            alxr_destroy();

            if !request_restart {
                break;
            }
        }
    }
    println!("successfully shutdown.");
}
