#![cfg(target_os = "android")]
use jni;
use jni::objects::GlobalRef;
use ndk_context;

use lazy_static::lazy_static;
use parking_lot::Mutex;

const WIFI_MODE_FULL_LOW_LATENCY: i32 = 4;
const WIFI_MODE_FULL_HIGH_PERF: i32 = 3;

lazy_static! {
    static ref WIFI_LOCK: Mutex<Option<GlobalRef>> = Mutex::new(None);
}

fn get_wifi_manager<'a>(env: &jni::JNIEnv<'a>) -> jni::objects::JObject<'a> {
    let wifi_service_str = env.new_string("wifi").unwrap();

    let ctx = ndk_context::android_context().context();
    env.call_method(
        unsafe { jni::objects::JObject::from_raw(ctx as jni::sys::jobject) },
        "getSystemService",
        "(Ljava/lang/String;)Ljava/lang/Object;",
        &[wifi_service_str.into()],
    )
    .unwrap()
    .l()
    .unwrap()
}

fn get_api_level() -> i32 {
    let vm_ptr = ndk_context::android_context().vm();
    let vm = unsafe { jni::JavaVM::from_raw(vm_ptr.cast()).unwrap() };
    let env = vm.attach_current_thread().unwrap();
    env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
        .unwrap()
        .i()
        .unwrap()
}

// This is needed to avoid wifi scans that disrupt streaming.
pub fn acquire_wifi_lock() {
    let mut maybe_wifi_lock = WIFI_LOCK.lock();

    if maybe_wifi_lock.is_none() {
        println!("ALXR: Aquring Wifi Lock");
        let vm_ptr = ndk_context::android_context().vm();
        let vm = unsafe { jni::JavaVM::from_raw(vm_ptr.cast()).unwrap() };
        let env = vm.attach_current_thread().unwrap();

        let wifi_mode = if get_api_level() >= 29 {
            // Recommended for virtual reality since it disables WIFI scans
            WIFI_MODE_FULL_LOW_LATENCY
        } else {
            WIFI_MODE_FULL_HIGH_PERF
        };

        let wifi_manager = get_wifi_manager(&env);
        let wifi_lock_jstring = env.new_string("alxr_wifi_lock").unwrap();
        let wifi_lock = env
            .call_method(
                wifi_manager,
                "createWifiLock",
                "(ILjava/lang/String;)Landroid/net/wifi/WifiManager$WifiLock;",
                &[wifi_mode.into(), wifi_lock_jstring.into()],
            )
            .unwrap()
            .l()
            .unwrap();
        env.call_method(wifi_lock, "acquire", "()V", &[]).unwrap();

        *maybe_wifi_lock = Some(env.new_global_ref(wifi_lock).unwrap());

        println!("ALXR: Wifi Lock Aquired");
    }
}

pub fn release_wifi_lock() {
    if let Some(wifi_lock) = WIFI_LOCK.lock().take() {
        println!("ALXR: Releasing Wifi Lock");

        let vm_ptr = ndk_context::android_context().vm();
        let vm = unsafe { jni::JavaVM::from_raw(vm_ptr.cast()).unwrap() };
        let env = vm.attach_current_thread().unwrap();

        env.call_method(wifi_lock.as_obj(), "release", "()V", &[])
            .unwrap();

        // wifi_lock is dropped here
        println!("ALXR: Wifi Lock Released");
    }
}
