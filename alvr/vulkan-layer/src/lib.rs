#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_imports
)]
mod bindings {
    include!(concat!(env!("OUT_DIR"), "/layer_bindings.rs"));
}
use bindings::*;

use std::ffi::CString;
use std::os::raw::c_char;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vkGetInstanceProcAddr(
    instance: VkInstance,
    p_name: *const c_char,
) -> PFN_vkVoidFunction {
    unsafe {
        g_sessionPath = CString::new(
            alvr_filesystem::filesystem_layout_from_invalid()
                .session()
                .to_string_lossy()
                .to_string(),
        )
        .unwrap()
        .into_raw();

        bindings::wsi_layer_vkGetInstanceProcAddr(instance, p_name)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vkGetDeviceProcAddr(
    instance: VkDevice,
    p_name: *const c_char,
) -> PFN_vkVoidFunction {
    unsafe { bindings::wsi_layer_vkGetDeviceProcAddr(instance, p_name) }
}
