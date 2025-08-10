use cmake::Config;
use core::str::FromStr;
use std::ffi::OsStr;
use std::{env, path::PathBuf};
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

fn cmake_option_from_bool(flag: bool) -> &'static str {
    if flag { "ON" } else { "OFF" }
}

fn cmake_option_from_feature(feature_name: &str) -> &'static str {
    cmake_option_from_bool(is_feature_enabled(&feature_name))
}

const FALVOR_FEATURE_NAMES: [&'static str; 2] = ["GENERIC_FLAVOR", "PICO_FLAVOR"];
const GRADLE_FLAVOR_NAMES: [&'static str; 2] = ["Generic", "PicoMobileOXR"];

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
const ENABLE_OCULUS_EXT_HEADERS_FEATURE: &'static str = "CARGO_FEATURE_OCULUS_EXT_HEADERS";
const DISABLE_DECODER_FEATURE: &'static str = "CARGO_FEATURE_NO_DECODER";
const CMAKE_GEN_ENV_VAR: &'static str = "ALXR_CMAKE_GEN";

const ENV_VAR_MONITOR_LIST: [&'static str; 2] = [CMAKE_GEN_ENV_VAR, BUNDLE_FFMPEG_INSTALL_DIR_VAR]; //, CMAKE_PREFIX_PATH_VAR];

fn main() {
    let target_triple = Triple::from_str(&env::var("TARGET").unwrap()).unwrap();
    let host_triple = Triple::from_str(&env::var("HOST").unwrap()).unwrap();
    let profile = env::var("PROFILE").unwrap();
    let out_dir_str = env::var("OUT_DIR").unwrap();
    let out_dir = PathBuf::from(&out_dir_str);
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
        config.generator(&cmake_generator);
    } else {
        // Using Ninja to build UWP/WinStore apps fails to build.
        // This should not be neccessary if the enviroment is
        // setup correctly (i.e. using vcvarsall.bat) for building UWP/WinStore apps,
        // and VS's fork of cmake.
        define_windows_store(&mut config);
    }

    config
        .always_configure(true)
        .define("BUILD_ALL_EXTENSIONS", "ON")
        .define("BUILD_API_LAYERS", "OFF")
        .define("BUILD_TESTS", "OFF")
        .define("BUILD_CONFORMANCE_TESTS", "OFF")
        .define(
            "USE_OCULUS_OXR_EXT_HEADERS",
            cmake_option_from_feature(&ENABLE_OCULUS_EXT_HEADERS_FEATURE),
        )
        .define(
            "DISABLE_DECODER_SUPPORT",
            cmake_option_from_feature(&DISABLE_DECODER_FEATURE),
        );

    let alxr_engine_output_dir = if is_android_env(&target_triple) {
        if profile == "release" && cmake_generator == "Ninja" {
            config.build_target("install/strip");
        }

        let product_flavor = get_product_flavour();
        println!("selected product flavor: {0}", product_flavor);
        match product_flavor {
            "PicoMobileOXR" => {
                config
                    .define("BUILD_LOADER", "OFF")
                    .define("USE_PICO_MOBILE_LOADER", "ON");
            }
            _ => {
                config.define("BUILD_LOADER", "ON");
            }
        };

        if host_triple.operating_system != OperatingSystem::Windows {
            // GNUInstallDirs vars workaround...
            config
                .define("CMAKE_INSTALL_INCLUDEDIR", out_dir_str.clone() + "/include")
                .define("CMAKE_INSTALL_LIBDIR", out_dir_str.clone() + "/lib");
        }

        let ndk_home = env::var("ANDROID_NDK_HOME").unwrap();
        let cmake_toolchain_file = ndk_home.clone() + "/build/cmake/android.toolchain.cmake";
        let android_abi = android_abi_name(&target_triple).unwrap();
        let min_sdk_version = "android-29";

        config
            .define("CMAKE_ANDROID_NDK", &ndk_home)
            .define("ANDROID_NDK", &ndk_home)
            .define("CMAKE_ANDROID_ARCH_ABI", &android_abi)
            .define("ANDROID_ABI", &android_abi)
            // ANDROID_PLATFORM == minSdkVersion / min_sdk_version
            .define("ANDROID_PLATFORM", &min_sdk_version)
            .define("CMAKE_TOOLCHAIN_FILE", &cmake_toolchain_file)
            .define("ANDROID_TOOLCHAIN", "clang")
            .define("ANDROID_STL", "c++_static")
            .define("ANDROID_ARM_NEON", "TRUE")
            .cflag("-flto=thin")
            .cxxflag("-std=c++20")
            .cxxflag("-fexceptions")
            .cxxflag("-frtti")
            .cxxflag("-flto=thin");
        if profile == "release" {
            config.cflag("-O3").cxxflag("-O3");
        }
        config.build()
    } else {
        let pkg_config_path = make_ffmpeg_pkg_config_path();
        let build_cuda = cmake_option_from_feature(BUILD_CUDA_INTEROP_FEATURE);
        config
            .define("BUILD_LOADER", "ON")
            .define(CMAKE_PREFIX_PATH_VAR, &pkg_config_path)
            .define("BUILD_CUDA_INTEROP", build_cuda)
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
        .rustified_enum("ALXRColorSpace")
        .rustified_enum("ALXRFacialExpressionType")
        .rustified_enum("ALXREyeTrackingType")
        .rustified_enum("ALXRPassthroughMode")
        .rustified_enum("ALXRFaceTrackingDataSource")
        .generate()
        .expect("bindings")
        .write_to_file(out_dir.join("alxr_engine.rs"))
        .expect("alxr_engine.rs");

    let alxr_engine_bin_dir = alxr_engine_output_dir.join("bin");
    let alxr_engine_lib_dir = alxr_engine_output_dir.join("lib");

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

    println!(
        "cargo:rustc-link-search=native={0}",
        alxr_engine_lib_dir.to_string_lossy()
    );
    println!(
        "cargo:rustc-link-search=native={0}",
        alxr_engine_bin_dir.to_string_lossy()
    );

    if target_triple.operating_system != OperatingSystem::Windows {
        println!("cargo:rustc-link-lib=dylib={0}", "openxr_loader");
    }

    println!("cargo:rustc-link-lib=dylib={0}", "alxr_engine");

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
