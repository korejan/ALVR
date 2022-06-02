mod connection;
mod connection_utils;

use alvr_common::{prelude::*, ALVR_VERSION, HEAD_ID, LEFT_HAND_ID, RIGHT_HAND_ID};
use alvr_session::Fov;
use alvr_sockets::{
    BatteryPacket, HeadsetInfoPacket, Input, LegacyController, LegacyInput, MotionData,
    TimeSyncPacket, ViewsConfig,
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
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(name = "openxr_client", about = "An OpenXR based ALVR client.")]
pub struct Options {
    // short and long flags (-d, --debug) will be deduced from the field's name
    /// Enable this if server and client are running on the same host.
    #[structopt(/*short,*/ long)]
    pub localhost: bool,

    #[structopt(short = "g", long = "graphics", parse(from_str))]
    pub graphics_api: Option<ALXRGraphicsApi>,

    #[structopt(short = "d", long = "decoder", parse(from_str))]
    pub decoder_type: Option<ALXRDecoderType>,

    /// Number of threads to use for CPU based decoding.
    #[structopt(long, default_value = "1")]
    pub decoder_thread_count: u32,

    /// Output verbose log information.
    #[structopt(short, long)]
    pub verbose: bool,
    // /// Set speed
    // // we don't want to name it "speed", need to look smart
    // #[structopt(short = "v", long = "velocity", default_value = "42")]
    // speed: f64,

    // /// Input file
    // #[structopt(parse(from_os_str))]
    // input: PathBuf,

    // /// Output file, stdout if not present
    // #[structopt(parse(from_os_str))]
    // output: Option<PathBuf>,

    // /// Where to write the output: to `stdout` or `file`
    // #[structopt(short)]
    // out_type: String,

    // /// File name: only required when `out-type` is set to `file`
    // #[structopt(name = "FILE", required_if("out-type", "file"))]
    // file_name: Option<String>,
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
        };
        // unsafe {
        //     let mut value = [0 as libc::c_char; libc::PROP_VALUE_MAX as usize];
        //     let property_name = b"debug.alxr.graphicsPlugin\0";
        //     if libc::__system_property_get(property_name.as_ptr(), value.as_mut_ptr()) != 0 {
        //         let val_str = CStr::from_bytes_with_nul(&value).unwrap();
        //         new_options.graphics_api = Some(From::from(val_str.to_str().unwrap_or("auto")));
        //     }
        //     let property_name = b"debug.alxr.verbose\0";
        //     if libc::__system_property_get(property_name.as_ptr(), value.as_mut_ptr()) != 0 {
        //         let val_str = CStr::from_bytes_with_nul(&value).unwrap();
        //         new_options.verbose =
        //             std::str::FromStr::from_str(val_str.to_str().unwrap_or("false"))
        //                 .unwrap_or(false);
        //     }
        // }
        new_options
    }
}

#[cfg(target_vendor = "uwp")]
impl Options {
    pub fn from_system_properties() -> Self {
        let mut new_options = Options {
            localhost: false,
            verbose: cfg!(debug_assertions),
            graphics_api: Some(ALXRGraphicsApi::D3D12),
            decoder_type: Some(ALXRDecoderType::D311VA),
            decoder_thread_count: 0,
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

pub fn init_connections(sys_properties: &ALXRSystemProperties) {
    alvr_common::show_err(|| -> StrResult {
        println!("Init-connections started.");

        let system_name = unsafe { CStr::from_ptr(sys_properties.systemName.as_ptr()) };
        let device_name: &str = system_name.to_str().unwrap_or("UnknownHMD");
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
                connection::connection_lifecycle_loop(headset_info, device_name, private_identity);
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
    fn from_tracking_quat_val(quat: TrackingQuat) -> Quat {
        from_tracking_quat(&quat)
    }
    #[inline(always)]
    fn from_tracking_vector3(vec: &TrackingVector3) -> Vec3 {
        Vec3::new(vec.x, vec.y, vec.z)
    }
    #[inline(always)]
    fn from_tracking_vector3_val(vec: TrackingVector3) -> Vec3 {
        from_tracking_vector3(&vec)
    }

    let data: &TrackingInfo = unsafe { &*data_ptr };
    if let Some(sender) = &*INPUT_SENDER.lock() {
        let input = Input {
            target_timestamp: std::time::Duration::from_nanos(data.targetTimestampNs),
            device_motions: vec![
                (
                    *HEAD_ID,
                    MotionData {
                        orientation: from_tracking_quat(&data.HeadPose_Pose_Orientation),
                        position: from_tracking_vector3(&data.HeadPose_Pose_Position),
                        linear_velocity: None,
                        angular_velocity: None,
                    },
                ),
                (
                    *LEFT_HAND_ID,
                    MotionData {
                        orientation: from_tracking_quat(if data.controller[0].isHand {
                            &data.controller[0].boneRootOrientation
                        } else {
                            &data.controller[0].orientation
                        }),
                        position: from_tracking_vector3(if data.controller[0].isHand {
                            &data.controller[0].boneRootPosition
                        } else {
                            &data.controller[0].position
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
                            &data.controller[1].boneRootOrientation
                        } else {
                            &data.controller[1].orientation
                        }),
                        position: from_tracking_vector3(if data.controller[1].isHand {
                            &data.controller[1].boneRootPosition
                        } else {
                            &data.controller[1].position
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
            left_hand_tracking: None,
            right_hand_tracking: None,
            button_values: std::collections::HashMap::new(), // unused for now
            legacy: LegacyInput {
                mounted: data.mounted,
                controllers: [
                    LegacyController {
                        enabled: data.controller[0].enabled,
                        is_hand: data.controller[0].isHand,
                        buttons: data.controller[0].buttons,
                        trackpad_position: Vec2::new(
                            data.controller[0].trackpadPosition.x,
                            data.controller[0].trackpadPosition.y,
                        ),
                        trigger_value: data.controller[0].triggerValue,
                        grip_value: data.controller[0].gripValue,
                        bone_rotations: {
                            let vec = data.controller[0]
                                .boneRotations
                                .iter()
                                .cloned()
                                .map(from_tracking_quat_val)
                                .collect::<Vec<_>>();

                            let mut array = [Quat::IDENTITY; 19];
                            array.copy_from_slice(&vec);
                            array
                        },
                        bone_positions_base: {
                            let vec = data.controller[0]
                                .bonePositionsBase
                                .iter()
                                .cloned()
                                .map(from_tracking_vector3_val)
                                .collect::<Vec<_>>();

                            let mut array = [Vec3::ZERO; 19];
                            array.copy_from_slice(&vec);
                            array
                        },
                        hand_finger_confience: data.controller[0].handFingerConfidences,
                    },
                    LegacyController {
                        enabled: data.controller[1].enabled,
                        is_hand: data.controller[1].isHand,
                        buttons: data.controller[1].buttons,
                        trackpad_position: Vec2::new(
                            data.controller[1].trackpadPosition.x,
                            data.controller[1].trackpadPosition.y,
                        ),
                        trigger_value: data.controller[1].triggerValue,
                        grip_value: data.controller[1].gripValue,
                        bone_rotations: {
                            let vec = data.controller[1]
                                .boneRotations
                                .iter()
                                .cloned()
                                .map(from_tracking_quat_val)
                                .collect::<Vec<_>>();

                            let mut array = [Quat::IDENTITY; 19];
                            array.copy_from_slice(&vec);
                            array
                        },
                        bone_positions_base: {
                            let vec = data.controller[1]
                                .bonePositionsBase
                                .iter()
                                .cloned()
                                .map(from_tracking_vector3_val)
                                .collect::<Vec<_>>();
                            let mut array = [Vec3::ZERO; 19];
                            array.copy_from_slice(&vec);
                            array
                        },
                        hand_finger_confience: data.controller[1].handFingerConfidences,
                    },
                ],
            },
        };
        sender.send(input).ok();
    }
}

pub extern "C" fn views_config_send(eye_info_ptr: *const ALXREyeInfo) {
    let eye_info: &ALXREyeInfo = unsafe { &*eye_info_ptr };
    let fov = &eye_info.eyeFov;
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
