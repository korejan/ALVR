use cmake::Config;
use core::str::FromStr;
use std::{env, path::PathBuf};
use std::{ffi::OsStr, process::Command};
use target_lexicon::{Architecture, ArmArchitecture, Environment, OperatingSystem, Triple};
use walkdir::DirEntry;

const BUNDLE_FFMPEG_INSTALL_DIR_VAR: &'static str = "ALXR_BUNDLE_FFMPEG_INSTALL_PATH";
const CMAKE_PREFIX_PATH_VAR: &'static str = "CMAKE_PREFIX_PATH";

fn make_ffmpeg_pkg_config_path() -> String {
    if cfg!(all(target_os = "linux", feature = "bundled-ffmpeg")) {
        let path = env::var(BUNDLE_FFMPEG_INSTALL_DIR_VAR).unwrap_or_default();
        if path.len() > 0 {
            return env::var(CMAKE_PREFIX_PATH_VAR)
                .map_or(path.clone(), |old| format!("{path};{old}"));
        }
    }
    env::var(CMAKE_PREFIX_PATH_VAR)
        .unwrap_or_default()
        .to_string()
}

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

fn gradle_task_from_profile(profile: &str, product_flavor: &str) -> String {
    let build_type = if profile.to_lowercase() == "debug" {
        "Debug"
    } else {
        "Release"
    };
    format!("assemble{0}{1}", product_flavor, build_type)
}

fn gradle_build_out_dir(
    target_triple: &target_lexicon::Triple,
    profile: &str,
    product_flavor: &str,
) -> PathBuf {
    PathBuf::from(format!(
        "intermediates/library_and_local_jars_jni/{0}{1}/jni/{2}",
        //"intermediates/stripped_native_libs/{0}{1}/out/lib/{2}",
        product_flavor,
        if profile == "debug" {
            "Debug"
        } else {
            "Release"
        }, // TODO: Fix case senstive folder names!
        android_abi_name(&target_triple).unwrap()
    ))
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

fn is_feature_enabled(feature_name: &str) -> bool {
    // yeah I know this is potentially very slow, sort this out later...
    env::vars().find(|(k, _)| feature_name == k).is_some()
}

// fn cmake_option_from_env_var(var_name : &str) -> &'static str {
//     let enabled = env::var(var_name)
//                     .unwrap_or_default()
//                     .parse::<u32>()
//                     .unwrap_or(0) > 0;
//     if enabled { "ON" } else { "OFF" }
// }

fn cmake_option_from_bool(flag: bool) -> &'static str {
    if flag {
        "ON"
    } else {
        "OFF"
    }
}

fn cmake_option_from_feature(feature_name: &str) -> &'static str {
    cmake_option_from_bool(is_feature_enabled(&feature_name))
}

const FALVOR_FEATURE_NAMES: [&'static str; 3] =
    ["GENERIC_FLAVOR", "QUEST_FLAVOR", "PICO_NEO_FLAVOR"];
const GRADLE_FLAVOR_NAMES: [&'static str; 3] = ["Generic", "OculusMobileOXR", "PicoMobileOXR"];

fn get_product_flavour() -> &'static str {
    for i in 0..FALVOR_FEATURE_NAMES.len() {
        let feature_name = "CARGO_FEATURE_".to_string() + FALVOR_FEATURE_NAMES[i];
        if is_feature_enabled(&feature_name) {
            return GRADLE_FLAVOR_NAMES[i];
        }
    }
    GRADLE_FLAVOR_NAMES[0]
}

fn define_windows_store(config: &mut Config) -> &mut Config {
    config
        .env("CMAKE_SYSTEM_NAME", "WindowsStore")
        .define("CMAKE_SYSTEM_NAME", "WindowsStore")
        .env("CMAKE_SYSTEM_VERSION", "10.0")
        .define("CMAKE_SYSTEM_VERSION", "10.0")
}

const BUILD_CUDA_INTEROP_FEATURE: &'static str = "CARGO_FEATURE_CUDA_INTEROP";
const CMAKE_GEN_ENV_VAR: &'static str = "ALXR_CMAKE_GEN";

const ENV_VAR_MONITOR_LIST: [&'static str; 2] = [CMAKE_GEN_ENV_VAR, BUNDLE_FFMPEG_INSTALL_DIR_VAR]; //, CMAKE_PREFIX_PATH_VAR];

fn main() {
    let target_triple = Triple::from_str(&env::var("TARGET").unwrap()).unwrap();
    let profile = env::var("PROFILE").unwrap();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let project_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    assert!(project_dir.ends_with("alxr-engine-sys"));

    let alxr_engine_dir = project_dir.join("cpp/ALVR-OpenXR-Engine");
    let alxr_engine_src_dir = alxr_engine_dir.join("src");

    let android_dir = project_dir.join("android");
    let alvr_client_dir = project_dir.join("../../client");
    let alvr_common_cpp_dir = alvr_client_dir.join("android/ALVR-common");

    let file_filters = vec!["CMakeLists.txt", "AndroidManifest.xml"];
    let file_ext_filters = vec![
        "h",
        "hpp",
        "inl",
        "c",
        "cc",
        "cxx",
        "cpp",
        "glsl",
        "hlsl",
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
        .chain(walkdir::WalkDir::new(&alxr_engine_dir).into_iter())
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

    let alxr_engine_output_dir = if is_android_env(&target_triple) {
        let product_flavor = get_product_flavour();
        println!("selected product flavor: {0}", product_flavor);
        let gradle_output_dir = out_dir.join("gradle_build");
        let gradle_cmd_path = android_dir.join(gradle_cmd(target_lexicon::HOST.operating_system));
        let status = Command::new(gradle_cmd_path)
            .arg(format!(
                "-PbuildDir={}",
                gradle_output_dir.to_string_lossy()
            ))
            .arg(gradle_task_from_profile(&profile, &product_flavor))
            .current_dir(&android_dir)
            .status()
            .unwrap();
        if !status.success() {
            panic!("gradle failed to build libalxr_engine.so");
        }
        let bin_dir_rel = gradle_build_out_dir(&target_triple, &profile, &product_flavor);
        gradle_output_dir.join(&bin_dir_rel)
    } else {
        let pkg_config_path = make_ffmpeg_pkg_config_path();
        let build_cuda = cmake_option_from_feature(BUILD_CUDA_INTEROP_FEATURE);
        let default_generator = "Ninja";
        let cmake_generator = env::var(CMAKE_GEN_ENV_VAR)
            .map(|s| {
                if s.is_empty() {
                    String::from(default_generator)
                } else {
                    s
                }
            })
            .unwrap_or(String::from(default_generator));

        let mut config = Config::new("cpp/ALVR-OpenXR-Engine");
        if target_triple.vendor != target_lexicon::Vendor::Uwp {
            assert!(!cmake_generator.is_empty());
            config.generator(cmake_generator);
        } else {
            // Using Ninja to build UWP/WinStore apps fails to build.
            // This should not be neccessary if the enviroment is
            // setup correctly (i.e. using vcvarsall.bat) for building UWP/WinStore apps,
            // and VS's fork of cmake.
            define_windows_store(&mut config);
        }
        config
            .always_configure(true)
            .define(CMAKE_PREFIX_PATH_VAR, &pkg_config_path)
            .define("BUILD_CUDA_INTEROP", build_cuda)
            .define("BUILD_ALL_EXTENSIONS", "ON")
            .define("BUILD_API_LAYERS", "OFF")
            .define("BUILD_TESTS", "OFF")
            .define("BUILD_CONFORMANCE_TESTS", "OFF")
            .build()
    };

    let defines = if is_android_env(&target_triple) {
        "-DXR_USE_PLATFORM_ANDROID"
    } else {
        ""
    };

    let tracking_binding_path = alvr_client_dir.join("android/app/src/main/cpp");
    let binding_file = alxr_engine_src_dir.join("alxr_engine/alxr_engine.h");
    bindgen::builder()
        .clang_arg("-xc++")
        .clang_arg("-std=c++20")
        .clang_arg("-DALXR_CLIENT")
        .clang_arg(defines)
        .clang_arg(format!("-I{0}", tracking_binding_path.to_string_lossy()))
        .header(binding_file.to_string_lossy())
        .derive_default(true)
        .rustified_enum("ALXRGraphicsApi")
        .rustified_enum("ALXRDecoderType")
        .generate()
        .expect("bindings")
        .write_to_file(out_dir.join("alxr_engine.rs"))
        .expect("alxr_engine.rs");

    if is_android_env(&target_triple) {
        println!("\npath: {0}\n", alxr_engine_output_dir.to_string_lossy());
        println!(
            "cargo:rustc-link-search=native={0}",
            alxr_engine_output_dir.to_string_lossy()
        );
    } else {
        let alxr_engine_bin_dir = alxr_engine_output_dir.join("bin");
        let alxr_engine_lib_dir = alxr_engine_output_dir.join("lib");
        println!(
            "cargo:rustc-link-search=native={0}",
            alxr_engine_lib_dir.to_string_lossy()
        );
        println!(
            "cargo:rustc-link-search=native={0}",
            alxr_engine_bin_dir.to_string_lossy()
        );

        if cfg!(target_os = "windows") {
            let mut run_exe_dir = out_dir.clone();
            run_exe_dir.pop();
            run_exe_dir.pop();
            run_exe_dir.pop();

            fn is_cso_file(path: &std::path::Path) -> bool {
                if let Some(ext) = path.extension() {
                    if ext.to_str().unwrap().eq("cso") {
                        return true;
                    }
                }
                return false;
            }
            for cso_file in walkdir::WalkDir::new(&alxr_engine_bin_dir)
                .into_iter()
                .filter_map(|maybe_entry| maybe_entry.ok())
                .map(|entry| entry.into_path())
                .filter(|entry| is_cso_file(&entry))
            {
                let relative_csof = cso_file.strip_prefix(&alxr_engine_bin_dir).unwrap();
                let dst_file = run_exe_dir.join(relative_csof);
                std::fs::create_dir_all(dst_file.parent().unwrap()).unwrap();
                std::fs::copy(&cso_file, &dst_file).unwrap();
            }
        }
    };

    // println!("cargo:rustc-link-lib=dylib={0}", "VkLayer_khronos_validation");
    //println!("cargo:rustc-link-lib=dylib={0}", "XrApiLayer_core_validation");
    if target_triple.operating_system != OperatingSystem::Windows {
        // println!("cargo:rustc-link-lib=dylib=avutil");
        // println!("cargo:rustc-link-lib=dylib=swresample");
        // println!("cargo:rustc-link-lib=dylib=avdevice");
        // println!("cargo:rustc-link-lib=dylib=avcodec");
        // println!("cargo:rustc-link-lib=dylib=avformat");
        // println!("cargo:rustc-link-lib=dylib=avfilter");
        // println!("cargo:rustc-link-lib=dylib=swscale");
        println!("cargo:rustc-link-lib=dylib={0}", "openxr_loader");
    }

    println!("cargo:rustc-link-lib=dylib={0}", "alxr_engine");

    //println!("cargo:rustc-link-lib=static=stdc++");
    //println!("cargo:rustc-link-lib=static=stdc++");
    //println!("cargo:rustc-cdylib-link-arg=-Wl,--export-dynamic");

    for path in cpp_paths.iter() {
        println!("cargo:rerun-if-changed={}", path.to_string_lossy());
    }
    if !cpp_paths.contains(&binding_file) {
        println!("cargo:rerun-if-changed={0}", binding_file.to_string_lossy());
    }
    for env_var in ENV_VAR_MONITOR_LIST {
        println!("cargo:rerun-if-env-changed={0}", env_var);
    }
}
