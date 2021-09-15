use cmake::Config;
use core::str::FromStr;
use std::{env, path::PathBuf};
use std::{ffi::OsStr, process::Command};
use target_lexicon::{Architecture, ArmArchitecture, Environment, OperatingSystem, Triple};
use walkdir::DirEntry;

fn android_abi_name(target_arch: &target_lexicon::Triple) -> Option<&'static str> {
    match target_arch.architecture {
        Architecture::Aarch64(_) => Some("arm64-v8a"),
        Architecture::Arm(ArmArchitecture::Armv7a) | Architecture::Arm(ArmArchitecture::Armv7) => {
            Some("armeabi-v7a")
        }
        Architecture::X86_64 => Some("x86_64"),
        Architecture::X86_32(_) => Some("x86"),
        _ => None,
    }
}

fn gradle_task_from_profile(profile: &str) -> &'static str {
    if profile.contains("debug") {
        "assembleDebug"
    } else {
        "assembleRelease"
    }
}

fn gradle_cmd(operating_system: target_lexicon::OperatingSystem) -> &'static str {
    match operating_system {
        OperatingSystem::Windows => "gradlew.bat",
        _ => "gradlew",
    }
}

fn is_android_env(target_triple: &target_lexicon::Triple) -> bool {
    match target_triple.environment {
        Environment::Android | Environment::Androideabi => return true,
        _ => return false,
    };
}

fn main() {
    let target_triple = Triple::from_str(&env::var("TARGET").unwrap()).unwrap();
    let profile = env::var("PROFILE").unwrap();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let project_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    assert!(project_dir.ends_with("common")); //"openxr-client"));

    let xr_engine_dir = project_dir.join("cpp/ALVR-OpenXR-Engine");
    let xr_engine_src_dir = xr_engine_dir.join("src");

    let android_dir = project_dir.join("android");
    let alvr_common_cpp_dir = project_dir.join("../../client/android/ALVR-common");

    let file_filters = vec!["CMakeLists.txt", "AndroidManifest.xml"];
    let file_ext_filters = vec![
        "h",
        "hpp",
        "inl",
        "c",
        "cc",
        "cxx",
        "cpp",
        "cmake",
        "in",
        "gradle",
        "pro",
        "properties",
    ]
    .into_iter()
    .map(OsStr::new)
    .collect::<Vec<_>>();

    let cpp_paths = walkdir::WalkDir::new(&alvr_common_cpp_dir)
        .into_iter()
        .chain(walkdir::WalkDir::new(&android_dir).into_iter())
        .chain(walkdir::WalkDir::new(&xr_engine_dir).into_iter())
        .filter_map(|maybe_entry| maybe_entry.ok())
        .filter(|dir_entry| {
            let path = dir_entry.path();
            for filter in file_filters.iter() {
                if path.ends_with(filter) {
                    return true;
                }
            }
            match path.extension() {
                Some(ext) => file_ext_filters.contains(&ext),
                _ => false,
            }
        })
        .map(DirEntry::into_path)
        .collect::<Vec<_>>();

    let xr_engine_output_dir = if is_android_env(&target_triple) {
        let gradle_output_dir = out_dir.join("gradle_build");
        let gradle_cmd_path = android_dir.join(gradle_cmd(target_lexicon::HOST.operating_system));
        let status = Command::new(gradle_cmd_path)
            .arg(format!(
                "-PbuildDir={}",
                gradle_output_dir.to_string_lossy()
            ))
            .arg(gradle_task_from_profile(&profile))
            .current_dir(&android_dir)
            .status()
            .unwrap();
        if !status.success() {
            panic!("gradle failed to build libxr_engine.so");
        }
        let bin_dir_rel = PathBuf::from(format!(
            "intermediates/library_and_local_jars_jni/{0}/jni/{1}",
            profile,
            android_abi_name(&target_triple).unwrap()
        ));
        gradle_output_dir.join(&bin_dir_rel)
    } else {
        let default_generator = "Ninja";
        let cmake_generator = env::var("ALVR_CMAKE_GEN")
            .map(|s| {
                if s.is_empty() {
                    String::from(default_generator)
                } else {
                    s
                }
            })
            .unwrap_or(String::from(default_generator));
        assert!(!cmake_generator.is_empty());
        Config::new("cpp/ALVR-OpenXR-Engine")
            .generator(cmake_generator)
            //.define("CMAKE_INSTALL_PREFIX", "cpp/ALVR-OpenXR-Engine/testout/install/test-x64-Debug")
            //.always_configure(true)
            //.profile("RELWITHDEBINFO")
            //.define("S", "cpp/ALVR-OpenXR-Engine")
            //.always_configure(true)
            .build()
    };

    let defines = if is_android_env(&target_triple) {
        "-DXR_USE_PLATFORM_ANDROID"
    } else {
        ""
    };

    let binding_file = xr_engine_src_dir.join("xr_engine/rust_bindings.h");
    bindgen::builder()
        .clang_arg("-xc++")
        .clang_arg("-std=c++17")
        .clang_arg(defines)
        .header(binding_file.to_string_lossy())
        .derive_default(true)
        .rustified_enum("GraphicsCtxApi")
        .generate()
        .expect("bindings")
        .write_to_file(out_dir.join("oxr_bindings.rs"))
        .expect("oxr_bindings.rs");

    if is_android_env(&target_triple) {
        println!(
            "cargo:rustc-link-search=native={0}",
            xr_engine_output_dir.to_string_lossy()
        );
        //println!("cargo:rustc-link-lib=dylib=openxr_loader");
        //println!("cargo:rustc-link-lib=dylib={0}", "c++_shared");
        //println!("cargo:rustc-link-lib=dylib={0}", "openxr_monado");
    } else {
        let xr_engine_bin_dir = xr_engine_output_dir.join("lib");
        let xr_engine_lib_dir = xr_engine_output_dir.join("bin");
        println!(
            "cargo:rustc-link-search=native={0}",
            xr_engine_bin_dir.to_string_lossy()
        );
        println!(
            "cargo:rustc-link-search=native={0}",
            xr_engine_lib_dir.to_string_lossy()
        );
    };

    //println!("cargo:rustc-link-lib=dylib={0}", "XrApiLayer_core_validation");
    //println!("cargo:rustc-link-lib=dylib={0}", "XrApiLayer_api_dump");
    if target_triple.operating_system != OperatingSystem::Windows {
        println!("cargo:rustc-link-lib=dylib={0}", "openxr_loader");
    }
    println!("cargo:rustc-link-lib=dylib={0}", "xr_engine");

    //println!("cargo:rustc-link-lib=static=stdc++");
    //println!("cargo:rustc-link-lib=static=stdc++");
    //println!("cargo:rustc-cdylib-link-arg=-Wl,--export-dynamic");

    for path in cpp_paths.iter() {
        println!("cargo:rerun-if-changed={}", path.to_string_lossy());
    }
    if !cpp_paths.contains(&binding_file) {
        println!("cargo:rerun-if-changed={0}", binding_file.to_string_lossy());
    }
}
