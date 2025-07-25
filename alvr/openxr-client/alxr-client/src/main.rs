#![cfg_attr(target_vendor = "uwp", windows_subsystem = "windows")]

use alxr_common::{
    alxr_destroy, alxr_init, alxr_is_session_running, alxr_process_frame, battery_send,
    init_connections, input_send, path_string_to_hash, request_idr, set_waiting_next_idr, shutdown,
    time_sync_send, to_alxr_version, video_error_report_send, views_config_send, ALXRClientCtx,
    ALXRColorSpace, ALXRDecoderType, ALXREyeTrackingType, ALXRFacialExpressionType,
    ALXRGraphicsApi, ALXRPassthroughMode, ALXRSystemProperties, ALXRVersion, APP_CONFIG,
};
use std::{thread, time};

// http://developer.download.nvidia.com/devzone/devcenter/gamegraphics/files/OptimusRenderingPolicies.pdf
#[cfg(target_os = "windows")]
#[allow(non_upper_case_globals)]
#[no_mangle]
pub static mut NvOptimusEnablement: i32 = 1;

// https://gpuopen.com/learn/amdpowerxpressrequesthighperformance/
#[cfg(target_os = "windows")]
#[allow(non_upper_case_globals)]
#[no_mangle]
pub static mut AmdPowerXpressRequestHighPerformance: i32 = 1;

const SLEEP_TIME: time::Duration = time::Duration::from_millis(250);

#[cfg(any(target_vendor = "uwp", target_os = "windows"))]
const DEFAULT_DECODER_TYPE: ALXRDecoderType = ALXRDecoderType::D311VA;

#[cfg(not(any(target_vendor = "uwp", target_os = "windows")))]
const DEFAULT_DECODER_TYPE: ALXRDecoderType = ALXRDecoderType::VAAPI;

#[cfg(target_vendor = "uwp")]
const DEFAULT_GRAPHICS_API: ALXRGraphicsApi = ALXRGraphicsApi::D3D12;

#[cfg(not(target_vendor = "uwp"))]
const DEFAULT_GRAPHICS_API: ALXRGraphicsApi = ALXRGraphicsApi::Auto;

#[cfg(not(target_os = "android"))]
fn main() {
    println!("{:?}", *APP_CONFIG);
    let selected_api = APP_CONFIG.graphics_api.unwrap_or(DEFAULT_GRAPHICS_API);
    let selected_decoder = APP_CONFIG.decoder_type.unwrap_or(DEFAULT_DECODER_TYPE);
    let xr_api_version = APP_CONFIG
        .xr_api_version
        .clone()
        .unwrap_or(semver::Version::new(0, 0, 0));
    unsafe {
        loop {
            let ctx = ALXRClientCtx {
                inputSend: Some(input_send),
                viewsConfigSend: Some(views_config_send),
                pathStringToHash: Some(path_string_to_hash),
                timeSyncSend: Some(time_sync_send),
                videoErrorReportSend: Some(video_error_report_send),
                batterySend: Some(battery_send),
                setWaitingNextIDR: Some(set_waiting_next_idr),
                requestIDR: Some(request_idr),
                graphicsApi: selected_api,
                decoderType: selected_decoder,
                displayColorSpace: APP_CONFIG.color_space.unwrap_or(ALXRColorSpace::Default),
                verbose: APP_CONFIG.verbose,
                disableLinearizeSrgb: APP_CONFIG.no_linearize_srgb,
                noSuggestedBindings: APP_CONFIG.no_bindings,
                noServerFramerateLock: false,
                noFrameSkip: false,
                disableLocalDimming: APP_CONFIG.disable_localdimming,
                headlessSession: APP_CONFIG.headless_session,
                noPassthrough: APP_CONFIG.no_passthrough,
                noFTServer: APP_CONFIG.no_tracking_server,
                noHandTracking: APP_CONFIG.no_hand_tracking,
                faceTrackingDataSources: APP_CONFIG.get_face_tracking_data_source_flags(),
                facialTracking: APP_CONFIG
                    .facial_tracking
                    .unwrap_or(ALXRFacialExpressionType::Auto),
                eyeTracking: APP_CONFIG.eye_tracking.unwrap_or(ALXREyeTrackingType::Auto),
                firmwareVersion: ALXRVersion {
                    major: 0,
                    minor: 0,
                    patch: 0,
                },
                trackingServerPortNo: APP_CONFIG.tracking_server_port_no,
                simulateHeadless: APP_CONFIG.simulate_headless,
                passthroughMode: APP_CONFIG
                    .passthrough_mode
                    .unwrap_or(ALXRPassthroughMode::None),
                internalDataPath: std::ptr::null(),
                noVisibilityMasks: APP_CONFIG.no_visibility_masks,
                noMultiviewRendering: APP_CONFIG.no_multi_view_rendering,
                xrApiVersion: to_alxr_version(&xr_api_version),
            };
            let mut sys_properties = ALXRSystemProperties::new();
            if !alxr_init(&ctx, &mut sys_properties) {
                break;
            }
            if !APP_CONFIG.no_alvr_server {
                init_connections(&sys_properties);
            }

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
