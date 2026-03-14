#![cfg(target_os = "android")]
use jni;
use jni::objects::JObject;
use jni::refs::Global;
use jni::{jni_sig, jni_str};
use ndk_context;

use lazy_static::lazy_static;
use parking_lot::Mutex;

const WIFI_MODE_FULL_LOW_LATENCY: i32 = 4;
const WIFI_MODE_FULL_HIGH_PERF: i32 = 3;

lazy_static! {
    static ref WIFI_LOCK: Mutex<Option<Global<JObject<'static>>>> = Mutex::new(None);
}

fn get_wifi_manager<'a>(env: &mut jni::Env<'a>) -> jni::objects::JObject<'a> {
    let wifi_service_str = env.new_string("wifi").unwrap();

    let ctx = ndk_context::android_context().context();
    let ctx_obj = unsafe { jni::objects::JObject::from_raw(&*env, ctx as jni::sys::jobject) };
    env.call_method(
        &ctx_obj,
        jni_str!("getSystemService"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
        &[(&wifi_service_str).into()],
    )
    .unwrap()
    .l()
    .unwrap()
}

fn get_api_level() -> i32 {
    let vm_ptr = ndk_context::android_context().vm();
    let vm = unsafe { jni::JavaVM::from_raw(vm_ptr.cast()) };
    vm.attach_current_thread(|env| {
        let cls = env.find_class(jni_str!("android/os/Build$VERSION"))?;
        env.get_static_field(&cls, jni_str!("SDK_INT"), jni_sig!("I"))?
            .i()
    })
    .unwrap()
}

// This is needed to avoid wifi scans that disrupt streaming.
pub fn acquire_wifi_lock() {
    let mut maybe_wifi_lock = WIFI_LOCK.lock();

    if maybe_wifi_lock.is_none() {
        log::info!("ALXR: Aquring Wifi Lock");

        let wifi_mode = if get_api_level() >= 29 {
            // Recommended for virtual reality since it disables WIFI scans
            WIFI_MODE_FULL_LOW_LATENCY
        } else {
            WIFI_MODE_FULL_HIGH_PERF
        };

        let vm_ptr = ndk_context::android_context().vm();
        let vm = unsafe { jni::JavaVM::from_raw(vm_ptr.cast()) };
        let global = vm
            .attach_current_thread(|env| {
                let wifi_manager = get_wifi_manager(env);
                let wifi_lock_jstring = env.new_string("alxr_wifi_lock")?;
                let wifi_lock = env
                    .call_method(
                        &wifi_manager,
                        jni_str!("createWifiLock"),
                        jni_sig!("(ILjava/lang/String;)Landroid/net/wifi/WifiManager$WifiLock;"),
                        &[wifi_mode.into(), (&wifi_lock_jstring).into()],
                    )?
                    .l()?;
                env.call_method(&wifi_lock, jni_str!("acquire"), jni_sig!("()V"), &[])?;

                env.new_global_ref(&wifi_lock)
            })
            .unwrap();

        *maybe_wifi_lock = Some(global);

        log::info!("ALXR: Wifi Lock Aquired");
    }
}

pub fn release_wifi_lock() {
    if let Some(wifi_lock) = WIFI_LOCK.lock().take() {
        log::info!("ALXR: Releasing Wifi Lock");

        let vm_ptr = ndk_context::android_context().vm();
        let vm = unsafe { jni::JavaVM::from_raw(vm_ptr.cast()) };
        vm.attach_current_thread(move |env| -> jni::errors::Result<()> {
            env.call_method(&*wifi_lock, jni_str!("release"), jni_sig!("()V"), &[])?;
            drop(wifi_lock);
            Ok(())
        })
        .unwrap();

        // wifi_lock is dropped here
        log::info!("ALXR: Wifi Lock Released");
    }
}
