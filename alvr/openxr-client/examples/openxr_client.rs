#![allow(non_upper_case_globals, non_snake_case, clippy::missing_safety_doc)]

mod connection;
mod connection_utils;

use std::{ffi::CStr, ffi::CString, str::FromStr};

// mod logging_backend;

// #[cfg(target_os = "android")]
// mod audio;

include!(concat!(env!("OUT_DIR"), "/oxr_bindings.rs"));

use alvr_common::{prelude::*, ALVR_NAME, ALVR_VERSION};
use alvr_sockets::{
    HeadsetInfoPacket,
    PrivateIdentity,
    //sockets::{LOCAL_IP}
};
// // use jni::{
// //     objects::{JClass, JObject, JString},
// //     JNIEnv,
// // };
use lazy_static::lazy_static;
use parking_lot::Mutex;
use std::{
    ptr, slice,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::{runtime::Runtime, sync::mpsc, sync::Notify};

use local_ipaddress;

//#[cfg(not(target_os = "android"))]
use structopt::{clap::arg_enum, StructOpt};

lazy_static! {
    static ref MAYBE_RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);
    static ref IDR_REQUEST_NOTIFIER: Notify = Notify::new();
    static ref IDR_PARSED: AtomicBool = AtomicBool::new(false);
    static ref MAYBE_LEGACY_SENDER: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>> =
        Mutex::new(None);
    static ref ON_PAUSE_NOTIFIER: Notify = Notify::new();
}

#[cfg(not(target_os = "android"))]
lazy_static! {
    static ref APP_CONFIG: Options = Options::from_args();
}
#[cfg(target_os = "android")]
const APP_CONFIG: Options = Options {
    localhost: false,
    graphics_api: Some(crate::GraphicsCtxApi::Auto),
};

pub extern "C" fn init_connections(sysProp: *const crate::SystemProperties) {
    //println!("Hello world\n");
    alvr_common::show_err(|| -> StrResult {
        println!("Hello world\n");

        // // struct OnResumeResult {
        // //     DeviceType deviceType;
        // //     int recommendedEyeWidth;
        // //     int recommendedEyeHeight;
        // //     float *refreshRates;
        // //     int refreshRatesCount;
        // // };

        // let java_vm = trace_err!(env.get_java_vm())?;
        // let activity_ref = trace_err!(env.new_global_ref(jactivity))?;
        // let nal_class_ref = trace_err!(env.new_global_ref(nal_class))?;

        //let result = onResumeNative(*jscreen_surface as _, dark_mode == 1);

        // let device_name = if result.deviceType == DeviceType_OCULUS_GO {
        //     "Oculus Go"
        // } else if result.deviceType == DeviceType_OCULUS_QUEST {
        //     "Oculus Quest"
        // } else if result.deviceType == DeviceType_OCULUS_QUEST_2 {
        //     "Oculus Quest 2"
        // } else {
        //     "Unknown device"
        // };

        let systemProperties = unsafe { *sysProp };
        let system_name = unsafe { CStr::from_ptr(systemProperties.systemName.as_ptr()) };
        let device_name: &str = system_name.to_str().unwrap_or("UnknownHMD");
        //let device_name = unsafe { CStr::from_ptr((*sysProp).systemName.as_ptr()).to_string_lossy().into_owned() };

        //let current_refresh_rate = systemProperties.currentRefreshRate;
        let available_refresh_rates = unsafe {
            slice::from_raw_parts(
                systemProperties.refreshRates,
                systemProperties.refreshRatesCount as _,
            )
            .to_vec()
        }; //vec![90.0];
        let preferred_refresh_rate = available_refresh_rates.last().cloned().unwrap_or(60_f32); //90.0;

        let headset_info = HeadsetInfoPacket {
            recommended_eye_width: systemProperties.recommendedEyeWidth as _,
            recommended_eye_height: systemProperties.recommendedEyeHeight as _,
            available_refresh_rates,
            preferred_refresh_rate,
            reserved: format!("{}", *ALVR_VERSION),
        };

        println!(
            "recommended eye width: {0}, height: {1}",
            headset_info.recommended_eye_width, headset_info.recommended_eye_height
        );

        let ipAddr = if APP_CONFIG.localhost {
            std::net::Ipv4Addr::LOCALHOST.to_string()
        } else {
            local_ipaddress::get().unwrap_or(alvr_sockets::LOCAL_IP.to_string())
        };
        let private_identity = alvr_sockets::create_identity(Some(ipAddr)).unwrap(); /*PrivateIdentity {
                                                                                         hostname: //trace_err!(env.get_string(jhostname))?.into(),
                                                                                         certificate_pem: //trace_err!(env.get_string(jcertificate_pem))?.into(),
                                                                                         key_pem: //trace_err!(env.get_string(jprivate_key))?.into(),
                                                                                     };*/

        let runtime = trace_err!(Runtime::new())?;

        runtime.spawn(async move {
            let connection_loop = connection::connection_lifecycle_loop(
                headset_info,
                device_name,
                private_identity,
                // Arc::new(java_vm),
                // Arc::new(activity_ref),
                // Arc::new(nal_class_ref),
            );

            tokio::select! {
                _ = connection_loop => (),
                _ = ON_PAUSE_NOTIFIER.notified() => ()
            };
        });

        *MAYBE_RUNTIME.lock() = Some(runtime);

        Ok(())
    }());
}

extern "C" fn legacy_send(buffer_ptr: *const ::std::os::raw::c_uchar, len: ::std::os::raw::c_uint) {
    if let Some(sender) = &*MAYBE_LEGACY_SENDER.lock() {
        let mut vec_buffer = vec![0; len as _];

        // use copy_nonoverlapping (aka memcpy) to avoid freeing memory allocated by C++
        unsafe {
            ptr::copy_nonoverlapping(buffer_ptr, vec_buffer.as_mut_ptr(), len as _);
        }

        sender.send(vec_buffer).ok();
    }
}

// impl FromStr for crate::GraphicsCtxApi {
//     type Err = ();
//     fn from_str(input: &str) -> Result<crate::GraphicsCtxApi, Self::Err> {
//         let trimmed = input.trim();
//         match trimmed {
//             "Vulkan2"  => Ok(crate::GraphicsCtxApi::Vulkan2),
//             "Vulkan"  => Ok(crate::GraphicsCtxApi::Vulkan),
//             "D3D12"  => Ok(crate::GraphicsCtxApi::D3D12),
//             "D3D11" => Ok(crate::GraphicsCtxApi::D3D11),
//             "OpenGL" => Ok(crate::GraphicsCtxApi::OpenGL),
//             "OpenGLES" => Ok(crate::GraphicsCtxApi::OpenGLES),
//             _      => Err(()),
//         }
//     }
// }

impl From<&str> for crate::GraphicsCtxApi {
    fn from(input: &str) -> Self {
        let trimmed = input.trim();
        match trimmed {
            "Vulkan2" => crate::GraphicsCtxApi::Vulkan2,
            "Vulkan" => crate::GraphicsCtxApi::Vulkan,
            "D3D12" => crate::GraphicsCtxApi::D3D12,
            "D3D11" => crate::GraphicsCtxApi::D3D11,
            "OpenGLES" => crate::GraphicsCtxApi::OpenGLES,
            "OpenGL" => crate::GraphicsCtxApi::OpenGL,
            _ => crate::GraphicsCtxApi::Auto,
        }
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = "openxr_client", about = "An OpenXR based ALVR client.")]
struct Options {
    /// Activate debug mode
    // short and long flags (-d, --debug) will be deduced from the field's name
    #[structopt(/*short,*/ long)]
    localhost: bool,

    #[structopt(short = "g", long = "graphics", parse(from_str))]
    graphics_api: Option<crate::GraphicsCtxApi>,
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

#[cfg(not(target_os = "android"))]
fn main() {
    println!("{:?}", *APP_CONFIG);
    let selectedApi = APP_CONFIG
        .graphics_api
        .unwrap_or(crate::GraphicsCtxApi::Auto);
    unsafe {
        let ctx = crate::RustCtx {
            initConnections: Some(init_connections),
            legacySend: Some(legacy_send),
            graphicsApi: selectedApi,
        };
        crate::openxrMain(&ctx);
    }
}

#[cfg(target_os = "android")]
use ndk::looper::*;
#[cfg(target_os = "android")]
use ndk_glue;
#[cfg(target_os = "android")]
use ndk_sys;

#[cfg(target_os = "android")]
struct AppData {
    destroy_requested: bool,
    resumed: bool,
}

#[cfg(target_os = "android")]
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

#[cfg(target_os = "android")]
#[cfg_attr(target_os = "android", ndk_glue::main(backtrace = "on"))]
pub fn main() {
    let mut app = AppData {
        destroy_requested: false,
        resumed: false,
    };
    test(&mut app).unwrap();
}

#[cfg(target_os = "android")]
pub const LOOPER_ID_MAIN: u32 = 0;
#[cfg(target_os = "android")]
pub const LOOPER_ID_INPUT: u32 = 1;

#[cfg(target_os = "android")]
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

#[cfg(target_os = "android")]
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
        let ctx = crate::RustCtx {
            graphicsApi: crate::GraphicsCtxApi::Auto,
            applicationVM: vm_ptr as *mut std::ffi::c_void,
            applicationActivity: (*native_activity.ptr().as_ptr()).clazz as *mut std::ffi::c_void,
            initConnections: Some(init_connections),
            legacySend: Some(legacy_send),
        };
        crate::openxrInit(&ctx);
    }

    while !app_data.destroy_requested {
        // Main game loop
        loop {
            // event pump loop
            let block = !app_data.destroy_requested
                && !app_data.resumed
                && unsafe { !crate::isOpenXRSessionRunning() }; // && app.ovr.is_none();
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
            crate::openxrProcesFrame();
        }
    }
    Ok(())
}
