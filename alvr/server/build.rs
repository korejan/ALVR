use alvr_filesystem as afs;
use bindgen::callbacks::{DeriveInfo, ParseCallbacks, TypeKind};
use std::{env, path::PathBuf, process::Command};

/// Environment variable overriding the CMake generator, mirroring
/// ALXR_CMAKE_GEN in alxr-engine-sys. Falls back to CMAKE_GENERATOR,
/// then to Ninja when available.
const CMAKE_GEN_ENV_VAR: &str = "ALVR_CMAKE_GEN";

/// Environment variable pointing at the root of a prebuilt shared FFmpeg
/// (containing include/ and lib/). Set by `cargo xtask` for bundled/GPL
/// builds; when unset, deps/ is scanned for one extracted by a previous
/// xtask run.
const FFMPEG_DIR_ENV_VAR: &str = "ALVR_FFMPEG_DIR";

#[derive(Debug)]
struct PodDerive;

impl ParseCallbacks for PodDerive {
    fn add_derives(&self, info: &DeriveInfo<'_>) -> Vec<String> {
        let pod_types = ["TrackingVector2", "TrackingVector3", "TrackingQuat"];
        if info.kind == TypeKind::Struct && pod_types.contains(&info.name) {
            vec!["bytemuck::Pod".into(), "bytemuck::Zeroable".into()]
        } else {
            vec![]
        }
    }
}

fn feature_enabled(feature: &str) -> bool {
    // Cargo exposes features as CARGO_FEATURE_<name> with `-` mapped to `_`
    let var = format!("CARGO_FEATURE_{}", feature.replace('-', "_").to_uppercase());
    env::var_os(var).is_some()
}

fn ninja_available() -> bool {
    Command::new("ninja")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Locate a prebuilt FFmpeg for the current build. The ALVR_FFMPEG_DIR
/// environment variable (set by `cargo xtask`) takes precedence; otherwise
/// deps/<os_dir>/ is scanned for a directory extracted by a previous xtask
/// run. The scan refuses to guess between multiple matches.
fn find_prebuilt_ffmpeg(os_dir: &str, name_markers: &[&str]) -> PathBuf {
    if let Some(dir) = env::var_os(FFMPEG_DIR_ENV_VAR) {
        let dir = PathBuf::from(dir);
        assert!(
            dir.join("include").is_dir(),
            "{FFMPEG_DIR_ENV_VAR}={} does not contain include/",
            dir.display()
        );
        return dir;
    }

    let search_dir = afs::deps_dir().join(os_dir);
    let entries = std::fs::read_dir(&search_dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", search_dir.display()));

    let mut matches = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("ffmpeg-")
                && name_markers.iter().all(|marker| name.contains(marker))
                && path.join("include").is_dir()
        })
        .collect::<Vec<_>>();

    match matches.len() {
        1 => matches.pop().unwrap(),
        0 => panic!(
            "no prebuilt FFmpeg matching {name_markers:?} found in {}; \
             run the build through `cargo xtask` to download it, \
             or point {FFMPEG_DIR_ENV_VAR} at one",
            search_dir.display()
        ),
        _ => panic!(
            "multiple prebuilt FFmpeg directories match {name_markers:?} in {}: {:?}; \
             delete the stale ones or point {FFMPEG_DIR_ENV_VAR} at the right one",
            search_dir.display(),
            matches
        ),
    }
}

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let cpp_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("cpp");

    // Cargo tracks directories recursively. Track only the parts of the CMake
    // project that this target's build consumes, so that e.g. editing win32
    // sources does not rebuild the Linux driver.
    let platform_dir = match target_os.as_str() {
        "windows" => "win32",
        other => other,
    };
    println!("cargo:rerun-if-changed=cpp/CMakeLists.txt");
    println!("cargo:rerun-if-changed=cpp/openvr");
    println!("cargo:rerun-if-changed=cpp/ALVR-common");
    println!("cargo:rerun-if-changed=cpp/alvr_server");
    println!("cargo:rerun-if-changed=cpp/shared");
    println!("cargo:rerun-if-changed=cpp/platform/{platform_dir}");
    println!("cargo:rerun-if-env-changed={CMAKE_GEN_ENV_VAR}");
    println!("cargo:rerun-if-env-changed=CMAKE_GENERATOR");
    println!("cargo:rerun-if-env-changed={FFMPEG_DIR_ENV_VAR}");

    let mut cmake = cmake::Config::new(&cpp_dir);

    // Generator selection: explicit env overrides first, then prefer Ninja.
    if let Some(generator) = env::var_os(CMAKE_GEN_ENV_VAR) {
        cmake.generator(generator);
    } else if env::var_os("CMAKE_GENERATOR").is_none() && ninja_available() {
        cmake.generator("Ninja");
    }

    match target_os.as_str() {
        "linux" if feature_enabled("bundled_ffmpeg") => {
            let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
                "x86_64" => "linux64",
                "aarch64" => "linuxarm64",
                other => panic!("unsupported architecture for bundled FFmpeg: {other}"),
            };
            let license = if feature_enabled("gpl") {
                "-gpl-"
            } else {
                "-lgpl-"
            };
            let ffmpeg_dir = find_prebuilt_ffmpeg("linux", &[arch, license, "-shared-"]);
            cmake
                .define("ALVR_BUNDLED_FFMPEG", "ON")
                .define("ALVR_FFMPEG_DIR", &ffmpeg_dir);
        }
        "windows" if feature_enabled("gpl") => {
            let ffmpeg_dir = find_prebuilt_ffmpeg("windows", &["-gpl-", "-shared-"]);
            cmake
                .define("ALVR_GPL", "ON")
                .define("ALVR_FFMPEG_DIR", &ffmpeg_dir);

            // Link the prebuilt FFmpeg import libraries. Order relative to the
            // static libs below doesn't matter: MSVC's linker is insensitive
            // to library order.
            println!(
                "cargo:rustc-link-search=native={}",
                ffmpeg_dir.join("lib").display()
            );
            for lib in ["avcodec", "avutil", "avfilter", "swscale"] {
                println!("cargo:rustc-link-lib=dylib={lib}");
            }
        }
        _ => {}
    }

    let dst = cmake.build();

    // Static libraries built by CMake. These must precede the shared-library
    // dependencies below: the GNU linker resolves symbols left to right.
    println!(
        "cargo:rustc-link-search=native={}",
        dst.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=alvr_server_cpp");
    println!("cargo:rustc-link-lib=static=nanors");

    // C++ standard library (the cc crate used to emit this automatically)
    match target_os.as_str() {
        "linux" => println!("cargo:rustc-link-lib=dylib=stdc++"),
        "macos" => println!("cargo:rustc-link-lib=dylib=c++"),
        _ => {} // MSVC links its C++ runtime via .drectve defaultlib directives
    }

    // Vendored prebuilt OpenVR. No macOS library is vendored (see
    // cpp/openvr/CMakeLists.txt, which is headers-only on Apple).
    if target_os != "macos" {
        println!(
            "cargo:rustc-link-search=native={}",
            cpp_dir.join("openvr/lib").display()
        );
        println!("cargo:rustc-link-lib=dylib=openvr_api");
    }

    if target_os == "linux" {
        if feature_enabled("bundled_ffmpeg") {
            // The bundled FFmpeg is dlopen'ed at runtime.
            println!("cargo:rustc-link-lib=dylib=dl");
        } else {
            for lib in ["libavutil", "libavfilter", "libavcodec", "libswscale"] {
                pkg_config::probe_library(lib).unwrap();
            }
        }
        pkg_config::probe_library("vulkan").unwrap();

        // fail the build if there are undefined symbols in the final library
        println!("cargo:rustc-cdylib-link-arg=-Wl,--no-undefined");
    }

    bindgen::builder()
        .clang_arg("-xc++")
        .header(cpp_dir.join("alvr_server/bindings.h").to_string_lossy())
        .derive_default(true)
        .parse_callbacks(Box::new(PodDerive))
        .generate()
        .expect("bindings")
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("bindings.rs");
}
