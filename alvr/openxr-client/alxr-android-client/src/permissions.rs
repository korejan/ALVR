#![cfg(target_os = "android")]
use jni;

//
// \brief Gets the internal name for an android permission.
// \param[in] lJNIEnv a pointer to the JNI environment
// \param[in] perm_name the name of the permission, e.g.,
//   "READ_EXTERNAL_STORAGE", "WRITE_EXTERNAL_STORAGE".
// \return a jstring with the internal name of the permission,
//   to be used with android Java functions
//   Context.checkSelfPermission() or Activity.requestPermissions()
//
fn android_permission_name<'a>(
    jni_env: &'a jni::JNIEnv,
    perm_name: &'a str,
) -> jni::errors::Result<jni::objects::JValue<'a>> {
    // nested class permission in class android.Manifest,
    // hence android 'slash' Manifest 'dollar' permission
    let class_manifest_permission = jni_env.find_class("android/Manifest$permission")?;
    jni_env.get_static_field(class_manifest_permission, perm_name, "Ljava/lang/String;")
}

//
// \brief Tests whether a permission is granted.
// \param[in] app a pointer to the android app.
// \param[in] perm_name the name of the permission, e.g.,
//   "READ_EXTERNAL_STORAGE", "WRITE_EXTERNAL_STORAGE".
// \retval true if the permission is granted.
// \retval false otherwise.
// \note Requires Android API level 23 (Marshmallow, May 2015)
//
fn android_has_permission<'a>(
    activity: jni::sys::jobject,
    jni_env: &'a jni::JNIEnv,
    perm_name: &'a str,
) -> jni::errors::Result<bool> {
    let class_package_manager = jni_env.find_class("android/content/pm/PackageManager")?;
    let permission_granted = jni_env
        .get_static_field(class_package_manager, "PERMISSION_GRANTED", "I")?
        .i()?;

    let maybe_custom_perm_name = if perm_name.contains('.') {
        Some(jni_env.new_string(&perm_name)?)
    } else {
        Option::None
    };

    let ls_perm = if maybe_custom_perm_name.is_none() {
        android_permission_name(&jni_env, perm_name)?
    } else {
        maybe_custom_perm_name.unwrap().into()
    };
    let activity_obj = unsafe { jni::objects::JObject::from_raw(activity) };
    let int_result = jni_env
        .call_method(
            activity_obj,
            "checkSelfPermission",
            "(Ljava/lang/String;)I",
            &[ls_perm],
        )?
        .i()?;

    Ok(int_result == permission_granted)
}

//
// \brief Query file permissions.
// \details This opens the system dialog that lets the user
//  grant (or deny) the permission.
// \param[in] app a pointer to the android app.
// \note Requires Android API level 23 (Marshmallow, May 2015)
//
fn android_request_permissions<'a>(
    activity: jni::sys::jobject,
    jni_env: &'a jni::JNIEnv,
    permission_names: &'a [&str],
) -> jni::errors::Result<()> {
    if permission_names.is_empty() {
        return Ok(());
    }

    let perm_array = jni_env.new_object_array(
        permission_names.len() as i32,
        jni_env.find_class("java/lang/String")?,
        jni_env.new_string("")?,
    )?;
    for idx in 0..permission_names.len() {
        let perm_name = &permission_names[idx];

        let maybe_custom_perm_name = if perm_name.contains('.') {
            Some(jni_env.new_string(&perm_name)?)
        } else {
            Option::None
        };

        let jperm_name = if maybe_custom_perm_name.is_none() {
            android_permission_name(&jni_env, perm_name)?
        } else {
            maybe_custom_perm_name.unwrap().into()
        };

        jni_env.set_object_array_element(perm_array, idx as i32, jperm_name.l()?)?;
    }

    let activity_obj = unsafe { jni::objects::JObject::from_raw(activity) };
    let perm_array_obj = unsafe { jni::objects::JObject::from_raw(perm_array) };
    jni_env.call_method(
        activity_obj,
        "requestPermissions",
        "([Ljava/lang/String;I)V",
        &[perm_array_obj.into(), 0.into()],
    )?;
    return Ok(());
}

pub fn check_android_permissions<'a>(
    activity: jni::sys::jobject,
    jvm: &'a jni::JavaVM,
) -> jni::errors::Result<()> {
    let env = jvm.attach_current_thread()?;
    let mut permission_names = vec![];
    for perm_name in [
        "RECORD_AUDIO",
        "READ_EXTERNAL_STORAGE",
        "com.oculus.permission.EYE_TRACKING",
        "com.oculus.permission.FACE_TRACKING",
        "com.magicleap.permission.EYE_TRACKING",
        "com.picovr.permission.EYE_TRACKING"
    ] {
        if !android_has_permission(activity, &env, &perm_name)? {
            permission_names.push(perm_name);
        }
    }
    android_request_permissions(activity, &env, &permission_names)
}
