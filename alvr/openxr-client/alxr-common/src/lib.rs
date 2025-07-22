mod connection;
mod connection_utils;

#[cfg(target_os = "android")]
mod audio;

use alvr_common::{prelude::*, ALVR_VERSION, HEAD_ID, LEFT_HAND_ID, RIGHT_HAND_ID};
use alvr_session::Fov;
use alvr_sockets::{
    BatteryPacket, HeadsetInfoPacket, HiddenAreaMesh, Input, LegacyController, LegacyInput,
    MotionData, TimeSyncPacket, ViewsConfig,
};
pub use alxr_engine_sys::*;
use lazy_static::lazy_static;
use local_ipaddress;
use parking_lot::Mutex;
use std::ffi::CStr;
use std::{
    slice,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::{runtime::Runtime, sync::mpsc, sync::Notify};
//#[cfg(not(target_os = "android"))]
use glam::{Quat, Vec2, Vec3};
use semver::Version;
use structopt::StructOpt;

#[cfg(target_os = "android")]
use android_system_properties::AndroidSystemProperties;

#[cfg(any(target_os = "android", target_vendor = "uwp"))]
const ALXR_TRACKING_SERVER_PORT_NO: u16 = 49192;

#[derive(Debug, StructOpt)]
#[structopt(name = "alxr-client", about = "An OpenXR based ALVR client.")]
pub struct Options {
    // short and long flags (-d, --debug) will be deduced from the field's name
    /// Enable this if the server and client are running on the same host-os.
    #[structopt(/*short,*/ long)]
    pub localhost: bool,

    #[structopt(short = "g", long = "graphics", parse(from_str))]
    pub graphics_api: Option<ALXRGraphicsApi>,

    #[structopt(short = "d", long = "decoder", parse(from_str))]
    pub decoder_type: Option<ALXRDecoderType>,

    /// Number of threads to use for CPU based decoding.
    #[structopt(long, default_value = "1")]
    pub decoder_thread_count: u32,

    #[structopt(long, parse(from_str))]
    pub color_space: Option<ALXRColorSpace>,

    /// Disables sRGB linerization, use this if the output in your headset looks to "dark".
    #[structopt(long)]
    pub no_linearize_srgb: bool,

    /// Output verbose log information.
    #[structopt(short, long)]
    pub verbose: bool,

    // short and long flags (-d, --debug) will be deduced from the field's name
    /// Disables connections / client discovery to alvr server
    #[structopt(/*short,*/ long)]
    pub no_alvr_server: bool,

    /// Disables all OpenXR Suggested bindings for all interaction profiles. This means disabling all inputs.
    #[structopt(/*short,*/ long)]
    pub no_bindings: bool,

    /// Disables locking/typing the client's frame-rate to the server frame-rate
    #[structopt(/*short,*/ long)]
    pub no_server_framerate_lock: bool,

    /// Disables skipping frames, disabling may increase idle times.
    #[structopt(/*short,*/ long)]
    pub no_frameskip: bool,

    #[structopt(/*short,*/ long)]
    pub disable_localdimming: bool,

    /// Enables a headless OpenXR session when a runtime supports it.
    #[structopt(/*short,*/ long = "headless")]
    pub headless_session: bool,

    /// Disables TrackingServer, if disabled no third-party apps will be able to make connection for features like facial/eye tracking.
    #[structopt(/*short,*/ long)]
    pub no_tracking_server: bool,

    /// Disables passthrough extensions, (XR_FB_passthrough | XR_HTC_passthrough) no attempt will be made to enable the extension.
    #[structopt(/*short,*/ long)]
    pub no_passthrough: bool,

    /// Disables hand-tracking extensions, XR_EXT_hand_tracking no attempt will be made to enable the extension.
    #[structopt(/*short,*/ long)]
    pub no_hand_tracking: bool,

    /// Specifices which tracking sources to use for face-tracking, default is VisualSource only
    #[structopt(long, parse(from_str), default_value = "VisualSource")]
    pub face_tracking_data_sources: Option<Vec<ALXRFaceTrackingDataSource>>,

    /// Disable or Specify which type of facial tracking extension to use, default is auto detection in order of vendor specific to multi-vendor
    #[structopt(long, parse(from_str))]
    pub facial_tracking: Option<ALXRFacialExpressionType>,

    /// Disable or specify which type of facial tracking extension to use, default is auto detection in order of vendor specific to multi-vendor
    #[structopt(long, parse(from_str))]
    pub eye_tracking: Option<ALXREyeTrackingType>,

    /// Sets the port number for the tracking server to listen on.
    #[structopt(long, default_value = "49192")]
    pub tracking_server_port_no: u16,

    /// Enables a headless OpenXR session if supported by the runtime (same as `headless_session`).
    /// In the absence of native support, will attempt to simulate a headless session.
    /// Caution: May not be compatible with all runtimes and could lead to unexpected behavior.
    #[structopt(/*short,*/ long = "simulate-headless")]
    pub simulate_headless: bool,

    /// Sets the initial passthrough mode, default is None (no passthrough blending)
    #[structopt(long, parse(from_str))]
    pub passthrough_mode: Option<ALXRPassthroughMode>,

    /// Disables all usages of visibility masks
    #[structopt(/*short,*/ long = "disable-visibility-masks")]
    pub no_visibility_masks: bool,

    /// Force disables multi-view rendering support
    #[structopt(/*short,*/ long = "disable-multi-view")]
    pub no_multi_view_rendering: bool,

    /// Overrides the OpenXR Api Version used for XR instance creation, an advance option meant for runtime quirk workarounds.
    #[structopt(long = "xr-api-version")]
    pub xr_api_version: Option<Version>,
}

impl Options {
    pub fn get_face_tracking_data_source_flags(self: &Self) -> u32 {
        let mut source_flags: u32 = 0;
        if let Some(sources) = &self.face_tracking_data_sources {
            for source in sources {
                if *source == ALXRFaceTrackingDataSource::VisualSource {
                    source_flags |=
                        ALXRFaceTrackingDataSourceFlags_ALXR_FACE_TRACKING_DATA_SOURCE_VISUAL;
                }
                if *source == ALXRFaceTrackingDataSource::AudioSource {
                    source_flags |=
                        ALXRFaceTrackingDataSourceFlags_ALXR_FACE_TRACKING_DATA_SOURCE_AUDIO;
                }
            }
        }
        source_flags
    }
}

#[cfg(target_os = "android")]
impl Options {
    pub fn from_system_properties() -> Self {
        let mut new_options = Options {
            localhost: false,
            verbose: cfg!(debug_assertions),
            graphics_api: Some(ALXRGraphicsApi::Auto),
            decoder_type: None,
            decoder_thread_count: 0,
            color_space: Some(ALXRColorSpace::Default),
            no_linearize_srgb: false,
            no_alvr_server: false,
            no_bindings: false,
            no_server_framerate_lock: false,
            no_frameskip: false,
            disable_localdimming: false,
            headless_session: false,
            no_tracking_server: false,
            no_passthrough: false,
            no_hand_tracking: false,
            face_tracking_data_sources: Some(vec![ALXRFaceTrackingDataSource::VisualSource]),
            facial_tracking: Some(ALXRFacialExpressionType::Auto),
            eye_tracking: Some(ALXREyeTrackingType::Auto),
            tracking_server_port_no: ALXR_TRACKING_SERVER_PORT_NO,
            simulate_headless: false,
            passthrough_mode: Some(ALXRPassthroughMode::None),
            no_visibility_masks: false,
            no_multi_view_rendering: false,
            xr_api_version: None,
        };

        let sys_properties = AndroidSystemProperties::new();

        let property_name = "debug.alxr.graphicsPlugin";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.graphics_api = Some(From::from(value.as_str()));
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {:?}",
                new_options.graphics_api
            );
        }

        let property_name = "debug.alxr.verbose";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.verbose =
                std::str::FromStr::from_str(value.as_str()).unwrap_or(new_options.verbose);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.verbose
            );
        }

        let property_name = "debug.alxr.no_linearize_srgb";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.no_linearize_srgb = std::str::FromStr::from_str(value.as_str())
                .unwrap_or(new_options.no_linearize_srgb);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.no_linearize_srgb
            );
        }

        let property_name = "debug.alxr.no_server_framerate_lock";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.no_server_framerate_lock = std::str::FromStr::from_str(value.as_str())
                .unwrap_or(new_options.no_server_framerate_lock);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.no_server_framerate_lock
            );
        }

        let property_name = "debug.alxr.no_frameskip";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.no_frameskip =
                std::str::FromStr::from_str(value.as_str()).unwrap_or(new_options.no_frameskip);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.no_frameskip
            );
        }

        let property_name = "debug.alxr.disable_localdimming";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.disable_localdimming = std::str::FromStr::from_str(value.as_str())
                .unwrap_or(new_options.disable_localdimming);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.disable_localdimming
            );
        }

        let property_name = "debug.alxr.color_space";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.color_space = Some(From::from(value.as_str()));
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {:?}",
                new_options.color_space
            );
        }

        let property_name = "debug.alxr.headless_session";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.headless_session =
                std::str::FromStr::from_str(value.as_str()).unwrap_or(new_options.headless_session);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.headless_session
            );
        }

        let property_name = "debug.alxr.no_tracking_server";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.no_tracking_server = std::str::FromStr::from_str(value.as_str())
                .unwrap_or(new_options.no_tracking_server);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.no_tracking_server
            );
        }

        let property_name = "debug.alxr.no_passthrough";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.no_passthrough =
                std::str::FromStr::from_str(value.as_str()).unwrap_or(new_options.no_passthrough);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.no_passthrough
            );
        }

        let property_name = "debug.alxr.no_hand_tracking";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.no_hand_tracking =
                std::str::FromStr::from_str(value.as_str()).unwrap_or(new_options.no_hand_tracking);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.no_hand_tracking
            );
        }

        let property_name = "debug.alxr.facial_tracking";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.facial_tracking = Some(From::from(value.as_str()));
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {:?}",
                new_options.facial_tracking
            );
        }

        let property_name = "debug.alxr.eye_tracking";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.eye_tracking = Some(From::from(value.as_str()));
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {:?}",
                new_options.eye_tracking
            );
        }

        let property_name = "debug.alxr.tracking_server_port_no";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.tracking_server_port_no = std::str::FromStr::from_str(value.as_str())
                .unwrap_or(new_options.tracking_server_port_no);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.tracking_server_port_no
            );
        }

        let property_name = "debug.alxr.simulate_headless";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.simulate_headless = std::str::FromStr::from_str(value.as_str())
                .unwrap_or(new_options.simulate_headless);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.simulate_headless
            );
        }

        let property_name = "debug.alxr.passthrough_mode";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.passthrough_mode = Some(From::from(value.as_str()));
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {:?}",
                new_options.passthrough_mode
            );
        }

        let property_name = "debug.alxr.no_visibility_masks";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.no_visibility_masks = std::str::FromStr::from_str(value.as_str())
                .unwrap_or(new_options.no_visibility_masks);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.no_visibility_masks
            );
        }

        let property_name = "debug.alxr.no_multiview_rendering";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.no_multi_view_rendering = std::str::FromStr::from_str(value.as_str())
                .unwrap_or(new_options.no_multi_view_rendering);
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {}",
                new_options.no_multi_view_rendering
            );
        }

        let property_name = "debug.alxr.xr_api_version";
        if let Some(value) = sys_properties.get(&property_name) {
            new_options.xr_api_version = std::str::FromStr::from_str(value.as_str()).ok();
            println!(
                "ALXR System Property: {property_name}, input: {value}, parsed-result: {:?}",
                new_options.xr_api_version
            );
        }

        new_options
    }
}

#[cfg(target_vendor = "uwp")]
impl Options {
    pub fn from_system_properties() -> Self {
        let new_options = Options {
            localhost: false,
            verbose: cfg!(debug_assertions),
            graphics_api: Some(ALXRGraphicsApi::D3D12),
            decoder_type: Some(ALXRDecoderType::D311VA),
            color_space: Some(ALXRColorSpace::Default),
            decoder_thread_count: 0,
            no_linearize_srgb: false,
            no_alvr_server: false,
            no_bindings: false,
            no_server_framerate_lock: false,
            no_frameskip: false,
            disable_localdimming: false,
            headless_session: false,
            no_tracking_server: false,
            no_passthrough: false,
            no_hand_tracking: false,
            face_tracking_data_sources: Some(vec![ALXRFaceTrackingDataSource::VisualSource]),
            facial_tracking: Some(ALXRFacialExpressionType::Auto),
            eye_tracking: Some(ALXREyeTrackingType::Auto),
            tracking_server_port_no: ALXR_TRACKING_SERVER_PORT_NO,
            simulate_headless: false,
            passthrough_mode: Some(ALXRPassthroughMode::None),
            no_visibility_masks: false,
            no_multi_view_rendering: false,
            xr_api_version: None,
        };
        new_options
    }
}

lazy_static! {
    pub static ref RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);
    static ref IDR_REQUEST_NOTIFIER: Notify = Notify::new();
    static ref IDR_PARSED: AtomicBool = AtomicBool::new(false);
    static ref INPUT_SENDER: Mutex<Option<mpsc::UnboundedSender<Input>>> = Mutex::new(None);
    static ref VIEWS_CONFIG_SENDER: Mutex<Option<mpsc::UnboundedSender<ViewsConfig>>> =
        Mutex::new(None);
    static ref BATTERY_SENDER: Mutex<Option<mpsc::UnboundedSender<BatteryPacket>>> =
        Mutex::new(None);
    static ref TIME_SYNC_SENDER: Mutex<Option<mpsc::UnboundedSender<TimeSyncPacket>>> =
        Mutex::new(None);
    static ref VIDEO_ERROR_REPORT_SENDER: Mutex<Option<mpsc::UnboundedSender<()>>> =
        Mutex::new(None);
    pub static ref ON_PAUSE_NOTIFIER: Notify = Notify::new();
}

#[cfg(all(not(target_os = "android"), not(target_vendor = "uwp")))]
lazy_static! {
    pub static ref APP_CONFIG: Options = Options::from_args();
}

#[cfg(any(target_os = "android", target_vendor = "uwp"))]
lazy_static! {
    pub static ref APP_CONFIG: Options = Options::from_system_properties();
}

pub fn to_alxr_version(v: &semver::Version) -> ALXRVersion {
    ALXRVersion {
        major: v.major as u32,
        minor: v.minor as u32,
        patch: v.patch as u32,
    }
}

pub fn init_connections(sys_properties: &ALXRSystemProperties) {
    alvr_common::show_err(|| -> StrResult {
        println!("Init-connections started.");

        let device_name = sys_properties.system_name();
        let available_refresh_rates = unsafe {
            slice::from_raw_parts(
                sys_properties.refreshRates,
                sys_properties.refreshRatesCount as _,
            )
            .to_vec()
        };
        let preferred_refresh_rate = available_refresh_rates.last().cloned().unwrap_or(60_f32); //90.0;

        let headset_info = HeadsetInfoPacket {
            recommended_eye_width: sys_properties.recommendedEyeWidth as _,
            recommended_eye_height: sys_properties.recommendedEyeHeight as _,
            available_refresh_rates,
            preferred_refresh_rate,
            reserved: format!("{}", *ALVR_VERSION),
        };

        println!(
            "recommended eye width: {0}, height: {1}",
            headset_info.recommended_eye_width, headset_info.recommended_eye_height
        );

        let ip_addr = if APP_CONFIG.localhost {
            std::net::Ipv4Addr::LOCALHOST.to_string()
        } else {
            local_ipaddress::get().unwrap_or(alvr_sockets::LOCAL_IP.to_string())
        };
        let private_identity = alvr_sockets::create_identity(Some(ip_addr)).unwrap();

        let runtime = trace_err!(Runtime::new())?;

        runtime.spawn(async move {
            let connection_loop =
                connection::connection_lifecycle_loop(headset_info, &device_name, private_identity);
            tokio::select! {
                _ = connection_loop => (),
                _ = ON_PAUSE_NOTIFIER.notified() => ()
            };
        });

        *RUNTIME.lock() = Some(runtime);

        println!("Init-connections Finished");

        Ok(())
    }());
}

pub fn shutdown() {
    ON_PAUSE_NOTIFIER.notify_waiters();
    drop(RUNTIME.lock().take());
}

pub unsafe extern "C" fn path_string_to_hash(path: *const ::std::os::raw::c_char) -> u64 {
    alvr_common::hash_string(CStr::from_ptr(path).to_str().unwrap())
}

pub extern "C" fn input_send(data_ptr: *const TrackingInfo) {
    #[inline(always)]
    fn from_tracking_quat(quat: &TrackingQuat) -> Quat {
        Quat::from_xyzw(quat.x, quat.y, quat.z, quat.w)
    }

    #[inline(always)]
    fn from_tracking_vector3(vec: &TrackingVector3) -> Vec3 {
        Vec3::new(vec.x, vec.y, vec.z)
    }

    #[inline(always)]
    fn from_tracking_vector2(vec: &TrackingVector2) -> Vec2 {
        Vec2::new(vec.x, vec.y)
    }

    let data: &TrackingInfo = unsafe { &*data_ptr };
    let input = Input {
        target_timestamp: std::time::Duration::from_nanos(data.targetTimestampNs),
        device_motions: vec![
            (
                *HEAD_ID,
                MotionData {
                    orientation: from_tracking_quat(&data.headPose.orientation),
                    position: from_tracking_vector3(&data.headPose.position),
                    linear_velocity: None,
                    angular_velocity: None,
                },
            ),
            (
                *LEFT_HAND_ID,
                MotionData {
                    orientation: from_tracking_quat(if data.controller[0].isHand {
                        &data.controller[0].boneRootPose.orientation
                    } else {
                        &data.controller[0].pose.orientation
                    }),
                    position: from_tracking_vector3(if data.controller[0].isHand {
                        &data.controller[0].boneRootPose.position
                    } else {
                        &data.controller[0].pose.position
                    }),
                    linear_velocity: Some(from_tracking_vector3(
                        &data.controller[0].linearVelocity,
                    )),
                    angular_velocity: Some(from_tracking_vector3(
                        &data.controller[0].angularVelocity,
                    )),
                },
            ),
            (
                *RIGHT_HAND_ID,
                MotionData {
                    orientation: from_tracking_quat(if data.controller[1].isHand {
                        &data.controller[1].boneRootPose.orientation
                    } else {
                        &data.controller[1].pose.orientation
                    }),
                    position: from_tracking_vector3(if data.controller[1].isHand {
                        &data.controller[1].boneRootPose.position
                    } else {
                        &data.controller[1].pose.position
                    }),
                    linear_velocity: Some(from_tracking_vector3(
                        &data.controller[1].linearVelocity,
                    )),
                    angular_velocity: Some(from_tracking_vector3(
                        &data.controller[1].angularVelocity,
                    )),
                },
            ),
        ],
        // left_hand_tracking: None,
        // right_hand_tracking: None,
        // button_values: std::collections::HashMap::new(), // unused for now
        legacy: LegacyInput {
            mounted: data.mounted,
            controllers: [
                LegacyController {
                    enabled: data.controller[0].enabled,
                    is_hand: data.controller[0].isHand,
                    buttons: data.controller[0].buttons,
                    joystick_position: from_tracking_vector2(&data.controller[0].joystickPosition),
                    trackpad_position: from_tracking_vector2(&data.controller[0].trackpadPosition),
                    trigger_value: data.controller[0].triggerValue,
                    grip_value: data.controller[0].gripValue,
                    bone_rotations: {
                        let bone_rotations = &data.controller[0].boneRotations;
                        let mut array = [Quat::IDENTITY; 19];
                        for i in 0..array.len() {
                            array[i] = from_tracking_quat(&bone_rotations[i]);
                        }
                        array
                    },
                    bone_positions_base: {
                        let bone_positions = &data.controller[0].bonePositionsBase;
                        let mut array = [Vec3::ZERO; 19];
                        for i in 0..array.len() {
                            array[i] = from_tracking_vector3(&bone_positions[i]);
                        }
                        array
                    },
                    hand_finger_confience: data.controller[0].handFingerConfidences,
                },
                LegacyController {
                    enabled: data.controller[1].enabled,
                    is_hand: data.controller[1].isHand,
                    buttons: data.controller[1].buttons,
                    joystick_position: from_tracking_vector2(&data.controller[1].joystickPosition),
                    trackpad_position: from_tracking_vector2(&data.controller[1].trackpadPosition),
                    trigger_value: data.controller[1].triggerValue,
                    grip_value: data.controller[1].gripValue,
                    bone_rotations: {
                        let bone_rotations = &data.controller[1].boneRotations;
                        let mut array = [Quat::IDENTITY; 19];
                        for i in 0..array.len() {
                            array[i] = from_tracking_quat(&bone_rotations[i]);
                        }
                        array
                    },
                    bone_positions_base: {
                        let bone_positions = &data.controller[1].bonePositionsBase;
                        let mut array = [Vec3::ZERO; 19];
                        for i in 0..array.len() {
                            array[i] = from_tracking_vector3(&bone_positions[i]);
                        }
                        array
                    },
                    hand_finger_confience: data.controller[1].handFingerConfidences,
                },
            ],
        },
    };
    if let Some(sender) = &*INPUT_SENDER.lock() {
        sender.send(input).ok();
    }
}

#[inline(always)]
fn make_hidden_area_meshes(view_config: &ALXRViewConfig) -> [HiddenAreaMesh; 2] {
    let empty_ham = HiddenAreaMesh {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    let mut hams = [empty_ham.clone(), empty_ham];
    for view_idx in 0..hams.len() {
        let src_ham = &view_config.hidden_area_meshes[view_idx];
        if src_ham.vertices.is_null()
            || src_ham.indices.is_null()
            || src_ham.vertexCount == 0
            || src_ham.indexCount == 0
        {
            return hams; // both empty.
        }
    }
    for view_idx in 0..hams.len() {
        let src_ham = &view_config.hidden_area_meshes[view_idx];
        unsafe {
            let verts_slice =
                std::slice::from_raw_parts(src_ham.vertices, src_ham.vertexCount as _);
            let indxs_slice = std::slice::from_raw_parts(src_ham.indices, src_ham.indexCount as _);
            let mut verts = Vec::with_capacity(verts_slice.len());
            for vert in verts_slice {
                verts.push(Vec2::new(vert.x, vert.y));
            }
            hams[view_idx] = HiddenAreaMesh {
                vertices: verts,
                indices: indxs_slice.to_vec(),
            }
        }
    }
    return hams;
}

pub extern "C" fn views_config_send(view_config_ptr: *const ALXRViewConfig) {
    let view_config: &ALXRViewConfig = unsafe { &*view_config_ptr };
    let eye_info = &view_config.eyeInfo;
    let fov = &view_config.eyeInfo.eyeFov;
    if let Some(sender) = &*VIEWS_CONFIG_SENDER.lock() {
        sender
            .send(ViewsConfig {
                ipd_m: eye_info.ipd,
                fov: [
                    Fov {
                        left: fov[0].left,
                        right: fov[0].right,
                        top: fov[0].top,
                        bottom: fov[0].bottom,
                    },
                    Fov {
                        left: fov[1].left,
                        right: fov[1].right,
                        top: fov[1].top,
                        bottom: fov[1].bottom,
                    },
                ],
                hidden_area_meshes: make_hidden_area_meshes(&view_config),
            })
            .ok();
    }
}

pub extern "C" fn battery_send(device_id: u64, gauge_value: f32, is_plugged: bool) {
    if let Some(sender) = &*BATTERY_SENDER.lock() {
        sender
            .send(BatteryPacket {
                device_id,
                gauge_value,
                is_plugged,
            })
            .ok();
    }
}

pub extern "C" fn time_sync_send(data_ptr: *const TimeSync) {
    let data: &TimeSync = unsafe { &*data_ptr };
    if let Some(sender) = &*TIME_SYNC_SENDER.lock() {
        let time_sync = TimeSyncPacket {
            mode: data.mode,
            server_time: data.serverTime,
            client_time: data.clientTime,
            packets_lost_total: data.packetsLostTotal,
            packets_lost_in_second: data.packetsLostInSecond,
            average_send_latency: data.averageSendLatency,
            average_transport_latency: data.averageTransportLatency,
            average_decode_latency: data.averageDecodeLatency,
            idle_time: data.idleTime,
            fec_failure: data.fecFailure,
            fec_failure_in_second: data.fecFailureInSecond,
            fec_failure_total: data.fecFailureTotal,
            fps: data.fps,
            server_total_latency: data.serverTotalLatency,
            tracking_recv_frame_index: data.trackingRecvFrameIndex,
        };
        sender.send(time_sync).ok();
    }
}

pub extern "C" fn video_error_report_send() {
    if let Some(sender) = &*VIDEO_ERROR_REPORT_SENDER.lock() {
        sender.send(()).ok();
    }
}

pub extern "C" fn set_waiting_next_idr(waiting: bool) {
    IDR_PARSED.store(!waiting, Ordering::Relaxed);
}

pub extern "C" fn request_idr() {
    IDR_REQUEST_NOTIFIER.notify_waiters();
}
