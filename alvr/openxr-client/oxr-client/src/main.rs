use oxr_common:: {
    RustCtx,
    GraphicsCtxApi,
    init_connections,
    legacy_send,
    openxrMain,
    shutdown,
    APP_CONFIG
};

// #[cfg(not(target_os = "android"))]
// lazy_static! {
//     pub static ref APP_CONFIG: Options = Options::from_args();
// }

#[cfg(not(target_os = "android"))]
fn main() {
    println!("{:?}", *APP_CONFIG);
    let selected_api = APP_CONFIG
        .graphics_api
        .unwrap_or(GraphicsCtxApi::Auto);
    unsafe {
        let ctx = RustCtx {
            initConnections: Some(init_connections),
            legacySend: Some(legacy_send),
            graphicsApi: selected_api,
        };
        openxrMain(&ctx);
    }
    shutdown();
    println!("successfully shutdown.");
}