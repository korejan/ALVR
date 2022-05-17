mod command;
mod dependencies;
mod packaging;
mod version;

use alvr_filesystem::{self as afs, Layout};
use camino::Utf8PathBuf;
use cargo_metadata::Message;
use fs_extra::{self as fsx, dir as dirx};
use pico_args::Arguments;
use std::collections::HashSet;
use std::error::Error;
use std::io::BufReader;
use std::process::Stdio;
use std::{env, fs, path::Path, time::Instant};

const HELP_STR: &str = r#"
cargo xtask
Developement actions for ALVR.

USAGE:
    cargo xtask <SUBCOMMAND> [FLAG] [ARGS]

SUBCOMMANDS:
    build-windows-deps  Download and compile external dependencies for Windows
    build-android-deps  Download and compile external dependencies for Android
    build-server        Build server driver, then copy binaries to build folder
    build-client        Build client, then copy binaries to build folder
    build-alxr-client   Build OpenXR based client (non-android platforms only), then copy binaries to build folder
    build-alxr-appimage Build OpenXR based client AppImage for linux only.
    build-alxr-android  Build OpenXR based client (android platforms only), then copy binaries to build folder
    build-alxr-quest    Build OpenXR based client for Oculus Quest (same as `build-alxr-android --oculus-quest`), then copy binaries to build folder
    build-alxr-pico     Build OpenXR based client for Pico Neo 3 (same as `build-alxr-android --pico-neo`), then copy binaries to build folder
    build-ffmpeg-linux  Build FFmpeg with VAAPI, NvEnc and Vulkan support. Only for CI
    publish-server      Build server in release mode, make portable version and installer
    publish-client      Build client for all headsets
    clean               Removes build folder
    kill-oculus         Kill all Oculus processes
    bump-versions       Bump server and client package versions
    bump-alxr-versions  Bump alxr-client package versions
    clippy              Show warnings for selected clippy lints
    prettier            Format JS and CSS files with prettier; Requires Node.js and NPM.

FLAGS:
    --reproducible      Force cargo to build reproducibly. Used only for build subcommands
    --fetch             Update crates with "cargo update". Used only for build subcommands
    --release           Optimized build without debug info. Used only for build subcommands
    --experiments       Build unfinished features
    --nightly           Bump versions to nightly and build. Used only for publish subcommand
    --oculus-quest      Oculus Quest build. Used only for build-client/build-alxr-android subcommand
    --oculus-go         Oculus Go build. Used only for build-client subcommand
    --generic           Generic Android build (cross-vendor openxr loader). Used only for build-alxr-android subcommand
    --pico-neo          Pico Neo 3 build. Used only for build-alxr-android subcommand
    --all-flavors       Build all android variants (Generic,Quest,Pico, etc), Used only for build-alxr-android subcommand
    --bundle-ffmpeg     Bundle ffmpeg libraries. Only used for build-server subcommand on Linux
    --no-nvidia         Additional flag to use with `build-server` or `build-alxr-client`. Disables nVidia/CUDA support.
    --gpl               Enables usage of GPL libs like ffmpeg on Windows, allowing software encoding.
    --ffmpeg-version    Bundle ffmpeg libraries. Only used for build-alxr-client subcommand on Linux
    --help              Print this text

ARGS:
    --version <VERSION> Specify version to set with the bump-(alxr-)versions subcommand
    --root <PATH>       Installation root. By default no root is set and paths are calculated using
                        relative paths, which requires conforming to FHS on Linux.
"#;

pub fn remove_build_dir() {
    let build_dir = afs::build_dir();
    fs::remove_dir_all(&build_dir).ok();
}

fn build_copy_ffmpeg(lib_dir: &Path, nvenc_flag: bool) -> std::path::PathBuf {
    let ffmpeg_path = dependencies::build_ffmpeg_linux(nvenc_flag);
    fs::create_dir_all(lib_dir.clone()).unwrap();
    for lib in walkdir::WalkDir::new(&ffmpeg_path)
        .into_iter()
        .filter_map(|maybe_entry| maybe_entry.ok())
        .map(|entry| entry.into_path())
        .filter(|path| path.file_name().unwrap().to_string_lossy().contains(".so."))
    {
        fs::copy(lib.clone(), lib_dir.join(lib.file_name().unwrap())).unwrap();
    }
    ffmpeg_path
}

pub fn build_server(
    is_release: bool,
    experiments: bool,
    fetch_crates: bool,
    bundle_ffmpeg: bool,
    no_nvidia: bool,
    gpl: bool,
    root: Option<String>,
    reproducible: bool,
) {
    // Always use CustomRoot for contructing the build directory. The actual runtime layout is respected
    let layout = Layout::new(&afs::server_build_dir());

    let build_type = if is_release { "release" } else { "debug" };

    let build_flags = format!(
        "{} {}",
        if is_release { "--release" } else { "" },
        if reproducible {
            "--offline --locked"
        } else {
            ""
        }
    );

    let mut server_features: Vec<&str> = vec![];
    let mut launcher_features: Vec<&str> = vec![];

    if bundle_ffmpeg {
        server_features.push("bundled_ffmpeg");
    }

    if gpl {
        server_features.push("gpl");
    }

    if server_features.is_empty() {
        server_features.push("default")
    }
    if launcher_features.is_empty() {
        launcher_features.push("default")
    }

    if let Some(root) = root {
        env::set_var("ALVR_ROOT_DIR", root);
    }

    let target_dir = afs::target_dir();
    let artifacts_dir = target_dir.join(build_type);

    if fetch_crates {
        command::run("cargo update").unwrap();
    }

    fs::remove_dir_all(&afs::server_build_dir()).ok();
    fs::create_dir_all(&afs::server_build_dir()).unwrap();
    fs::create_dir_all(&layout.openvr_driver_lib().parent().unwrap()).unwrap();
    fs::create_dir_all(&layout.launcher_exe().parent().unwrap()).unwrap();

    let mut copy_options = dirx::CopyOptions::new();
    copy_options.copy_inside = true;
    fsx::copy_items(
        &[afs::workspace_dir().join("alvr/xtask/resources/presets")],
        layout.presets_dir(),
        &copy_options,
    )
    .unwrap();

    if bundle_ffmpeg {
        let nvenc_flag = !no_nvidia;
        let ffmpeg_path = dependencies::build_ffmpeg_linux(nvenc_flag);
        let lib_dir = afs::server_build_dir().join("lib64").join("alvr");
        let mut libavcodec_so = std::path::PathBuf::new();
        fs::create_dir_all(lib_dir.clone()).unwrap();
        for lib in walkdir::WalkDir::new(ffmpeg_path)
            .into_iter()
            .filter_map(|maybe_entry| maybe_entry.ok())
            .map(|entry| entry.into_path())
            .filter(|path| path.file_name().unwrap().to_string_lossy().contains(".so."))
        {
            let lib_filename = lib.file_name().unwrap();
            if lib_filename.to_string_lossy().starts_with("libavcodec.so") {
                libavcodec_so = lib.canonicalize().unwrap();
            }
            fs::copy(lib.clone(), lib_dir.join(&lib_filename)).unwrap();
        }
        // copy ffmpeg shared lib dependencies.
        let lib_dir = lib_dir.canonicalize().unwrap();
        for solib in ["libx264.so", "libx265.so"] {
            let src_libs = dependencies::find_resolved_so_paths(&libavcodec_so, solib);
            if !src_libs.is_empty() {
                let src_lib = src_libs.first().unwrap();
                let dst_lib = lib_dir.join(src_lib.file_name().unwrap());
                fs::copy(src_lib, dst_lib).unwrap();
            }
        }
    }

    if gpl && cfg!(windows) {
        let ffmpeg_path = dependencies::extract_ffmpeg_windows();
        let bin_dir = afs::server_build_dir().join("bin").join("win64");
        fs::create_dir_all(bin_dir.clone()).unwrap();
        for dll in walkdir::WalkDir::new(ffmpeg_path.join("bin"))
            .into_iter()
            .filter_map(|maybe_entry| maybe_entry.ok())
            .map(|entry| entry.into_path())
            .filter(|path| path.file_name().unwrap().to_string_lossy().contains(".dll"))
        {
            fs::copy(dll.clone(), bin_dir.join(dll.file_name().unwrap())).unwrap();
        }
    }

    if cfg!(target_os = "linux") {
        command::run_in(
            &afs::workspace_dir().join("alvr/vrcompositor-wrapper"),
            &format!("cargo build {build_flags}"),
        )
        .unwrap();
        fs::create_dir_all(&layout.vrcompositor_wrapper_dir).unwrap();
        fs::copy(
            artifacts_dir.join("vrcompositor-wrapper"),
            layout.vrcompositor_wrapper(),
        )
        .unwrap();
    }

    command::run_in(
        &afs::workspace_dir().join("alvr/server"),
        &format!(
            "cargo build {build_flags} --no-default-features --features {}",
            server_features.join(",")
        ),
    )
    .unwrap();
    fs::copy(
        artifacts_dir.join(afs::dynlib_fname("alvr_server")),
        layout.openvr_driver_lib(),
    )
    .unwrap();

    command::run_in(
        &afs::workspace_dir().join("alvr/launcher"),
        &format!(
            "cargo build {build_flags} --no-default-features --features {}",
            launcher_features.join(",")
        ),
    )
    .unwrap();
    fs::copy(
        artifacts_dir.join(afs::exec_fname("alvr_launcher")),
        layout.launcher_exe(),
    )
    .unwrap();

    if experiments {
        let dir_content = dirx::get_dir_content2(
            "alvr/experiments/gui/resources/languages",
            &dirx::DirOptions { depth: 1 },
        )
        .unwrap();
        let items: Vec<&String> = dir_content.directories[1..]
            .iter()
            .chain(dir_content.files.iter())
            .collect();

        let destination = afs::server_build_dir().join("languages");
        fs::create_dir_all(&destination).unwrap();
        fsx::copy_items(&items, destination, &dirx::CopyOptions::new()).unwrap();
    }

    fs::copy(
        afs::workspace_dir().join("alvr/xtask/resources/driver.vrdrivermanifest"),
        layout.openvr_driver_manifest(),
    )
    .unwrap();

    if cfg!(windows) {
        let dir_content = dirx::get_dir_content("alvr/server/cpp/bin/windows").unwrap();
        fsx::copy_items(
            &dir_content.files,
            layout.openvr_driver_lib().parent().unwrap(),
            &dirx::CopyOptions::new(),
        )
        .unwrap();
    }

    // let dir_content =
    //     dirx::get_dir_content2("alvr/resources", &dirx::DirOptions { depth: 1 }).unwrap();
    // let items: Vec<&String> = dir_content.directories[1..]
    //     .iter()
    //     .chain(dir_content.files.iter())
    //     .collect();
    // fs::create_dir_all(&layout.resources_dir()).unwrap();
    // fsx::copy_items(&items, layout.resources_dir(), &dirx::CopyOptions::new()).unwrap();

    let dir_content = dirx::get_dir_content2(
        afs::workspace_dir().join("alvr/dashboard"),
        &dirx::DirOptions { depth: 1 },
    )
    .unwrap();
    let items: Vec<&String> = dir_content.directories[1..]
        .iter()
        .chain(dir_content.files.iter())
        .collect();

    fs::create_dir_all(&layout.dashboard_dir()).unwrap();
    fsx::copy_items(&items, layout.dashboard_dir(), &dirx::CopyOptions::new()).unwrap();

    if cfg!(target_os = "linux") {
        command::run_in(
            &afs::workspace_dir().join("alvr/vulkan-layer"),
            &format!("cargo build {build_flags}"),
        )
        .unwrap();

        let lib_dir = afs::server_build_dir().join("lib64");
        let manifest_dir = afs::server_build_dir().join("share/vulkan/explicit_layer.d");

        fs::create_dir_all(&manifest_dir).unwrap();
        fs::create_dir_all(&lib_dir).unwrap();
        fs::copy(
            afs::workspace_dir().join("alvr/vulkan-layer/layer/alvr_x86_64.json"),
            manifest_dir.join("alvr_x86_64.json"),
        )
        .unwrap();
        fs::copy(
            artifacts_dir.join(afs::dynlib_fname("alvr_vulkan_layer")),
            lib_dir.join(afs::dynlib_fname("alvr_vulkan_layer")),
        )
        .unwrap();
    }
}

pub fn build_client(is_release: bool, is_nightly: bool, for_oculus_go: bool) {
    let headset_name = if for_oculus_go {
        "oculus_go"
    } else {
        "oculus_quest"
    };

    let headset_type = if for_oculus_go {
        "OculusGo"
    } else {
        "OculusQuest"
    };
    let package_type = if is_nightly { "Nightly" } else { "Stable" };
    let build_type = if is_release { "release" } else { "debug" };

    let build_task = format!("assemble{headset_type}{package_type}{build_type}");

    let client_dir = afs::workspace_dir().join("alvr/client/android");
    let command_name = if cfg!(not(windows)) {
        "./gradlew"
    } else {
        "gradlew.bat"
    };

    let artifact_name = format!("alvr_client_{headset_name}");
    fs::create_dir_all(&afs::build_dir().join(&artifact_name)).unwrap();

    env::set_current_dir(&client_dir).unwrap();
    command::run(&format!("{command_name} {build_task}")).unwrap();
    env::set_current_dir(afs::workspace_dir()).unwrap();

    fs::copy(
        client_dir
            .join("app/build/outputs/apk")
            .join(format!("{headset_type}{package_type}"))
            .join(build_type)
            .join(format!(
                "app-{headset_type}-{package_type}-{build_type}.apk",
            )),
        afs::build_dir()
            .join(&artifact_name)
            .join(format!("{artifact_name}.apk")),
    )
    .unwrap();
}

type PathSet = HashSet<Utf8PathBuf>;
fn find_linked_native_paths(
    crate_path: &Path,
    build_flags: &str,
) -> Result<PathSet, Box<dyn Error>> {
    // let manifest_file = crate_path.join("Cargo.toml");
    // let metadata = MetadataCommand::new()
    //     .manifest_path(manifest_file)
    //     .exec()?;
    // let package = match metadata.root_package() {
    //     Some(p) => p,
    //     None => return Err("cargo out-dir must be run from within a crate".into()),
    // };
    let mut args = vec!["check", "--message-format=json", "--quiet"];
    args.extend(build_flags.split_ascii_whitespace());

    let mut command = std::process::Command::new("cargo")
        .current_dir(crate_path)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let reader = BufReader::new(command.stdout.take().unwrap());
    let mut linked_path_set = PathSet::new();
    for message in Message::parse_stream(reader) {
        match message? {
            Message::BuildScriptExecuted(script) => {
                for lp in script.linked_paths.iter() {
                    match lp.as_str().strip_prefix("native=") {
                        Some(p) => {
                            linked_path_set.insert(p.into());
                        }
                        None => (),
                    }
                }
            }
            _ => (),
        }
    }
    Ok(linked_path_set)
}

#[derive(Clone, Copy, Debug)]
pub struct AlxBuildFlags {
    is_release: bool,
    reproducible: bool,
    no_nvidia: bool,
    bundle_ffmpeg: bool,
    fetch_crates: bool,
}

impl Default for AlxBuildFlags {
    fn default() -> Self {
        AlxBuildFlags {
            is_release: true,
            reproducible: true,
            no_nvidia: true,
            bundle_ffmpeg: true,
            fetch_crates: false,
        }
    }
}

impl AlxBuildFlags {
    pub fn make_build_string(&self) -> String {
        let enable_bundle_ffmpeg = cfg!(target_os = "linux") && self.bundle_ffmpeg;
        let feature_map = vec![
            (enable_bundle_ffmpeg, "bundled-ffmpeg"),
            (!self.no_nvidia, "cuda-interop"),
        ];

        let flag_map = vec![
            (self.is_release, "--release"),
            (self.reproducible, "--offline --locked"),
        ];

        fn to_str_vec(m: &Vec<(bool, &'static str)>) -> Vec<&'static str> {
            let mut strs: Vec<&str> = vec![];
            for (_, strv) in m.iter().filter(|(f, _)| *f) {
                strs.push(strv);
            }
            strs
        }
        let feature_strs = to_str_vec(&feature_map);
        let flag_strs = to_str_vec(&flag_map);

        let features = feature_strs.join(",");
        let mut build_str = flag_strs.join(" ").to_string();
        if features.len() > 0 {
            if build_str.len() > 0 {
                build_str.push(' ');
            }
            build_str.push_str("--features ");
            build_str.push_str(features.as_str());
        }
        build_str
    }
}

pub fn build_alxr_client(root: Option<String>, ffmpeg_version: &str, flags: AlxBuildFlags) {
    if let Some(root) = root {
        env::set_var("ALVR_ROOT_DIR", root);
    }

    let build_flags = flags.make_build_string();
    let target_dir = afs::target_dir();
    let build_type = if flags.is_release { "release" } else { "debug" };
    let artifacts_dir = target_dir.join(build_type);

    let alxr_client_build_dir = afs::alxr_client_build_dir(build_type, !flags.no_nvidia);
    fs::remove_dir_all(&alxr_client_build_dir).ok();
    fs::create_dir_all(&alxr_client_build_dir).unwrap();

    let bundle_ffmpeg_enabled = cfg!(target_os = "linux") && flags.bundle_ffmpeg;
    if bundle_ffmpeg_enabled {
        assert!(!ffmpeg_version.is_empty(), "ffmpeg-version is empty!");

        let ffmpeg_build_dir = &alxr_client_build_dir;
        dependencies::build_ffmpeg_linux_install(
            true,
            ffmpeg_version,
            /*enable_decoders=*/ true,
            &ffmpeg_build_dir,
        );

        assert!(ffmpeg_build_dir.exists());
        env::set_var(
            "ALXR_BUNDLE_FFMPEG_INSTALL_PATH",
            ffmpeg_build_dir.to_str().unwrap(),
        );

        fn find_shared_lib(dir: &Path, key: &str) -> Option<std::path::PathBuf> {
            for so_file in walkdir::WalkDir::new(dir)
                .into_iter()
                .filter_map(|maybe_entry| maybe_entry.ok())
                .map(|entry| entry.into_path())
                .filter(|path| afs::is_dynlib_file(&path))
            {
                let so_filename = so_file.file_name().unwrap();
                if so_filename.to_string_lossy().starts_with(&key) {
                    return Some(so_file.canonicalize().unwrap());
                }
            }
            None
        }

        let lib_dir = alxr_client_build_dir.join("lib").canonicalize().unwrap();
        if let Some(libavcodec_so) = find_shared_lib(&lib_dir, "libavcodec.so") {
            for solib in ["libx264.so", "libx265.so"] {
                let src_libs = dependencies::find_resolved_so_paths(&libavcodec_so, solib);
                if !src_libs.is_empty() {
                    let src_lib = src_libs.first().unwrap();
                    let dst_lib = lib_dir.join(src_lib.file_name().unwrap());
                    println!("Copying {src_lib:?} to {dst_lib:?}");
                    fs::copy(src_lib, dst_lib).unwrap();
                }
            }
        }
    }

    if flags.fetch_crates {
        command::run("cargo update").unwrap();
    }

    let alxr_client_dir = afs::workspace_dir().join("alvr/openxr-client/alxr-client");
    let (alxr_cargo_cmd, alxr_build_lib_dir) = if cfg!(target_os = "windows") {
        (
            format!("cargo build {}", build_flags),
            alxr_client_build_dir.to_owned(),
        )
    } else {
        (
            format!(
                "cargo rustc {} -- -C link-args=\'-Wl,-rpath,$ORIGIN/lib\'",
                build_flags
            ),
            alxr_client_build_dir.join("lib"),
        )
    };
    command::run_in(&alxr_client_dir, &alxr_cargo_cmd).unwrap();

    fn is_linked_depends_file(path: &Path) -> bool {
        if afs::is_dynlib_file(&path) {
            return true;
        }
        if cfg!(target_os = "windows") {
            if let Some(ext) = path.extension() {
                if ext.to_str().unwrap().eq("pdb") {
                    return true;
                }
            }
        }
        if let Some(ext) = path.extension() {
            if ext.to_str().unwrap().eq("json") {
                return true;
            }
        }
        return false;
    }
    println!("Searching for linked native dependencies, please wait this may take some time.");
    let linked_paths = find_linked_native_paths(&alxr_client_dir, &build_flags).unwrap();
    for linked_path in linked_paths.iter() {
        for linked_depend_file in walkdir::WalkDir::new(linked_path)
            .into_iter()
            .filter_map(|maybe_entry| maybe_entry.ok())
            .map(|entry| entry.into_path())
            .filter(|entry| is_linked_depends_file(&entry))
        {
            let relative_lpf = linked_depend_file.strip_prefix(linked_path).unwrap();
            let dst_file = alxr_build_lib_dir.join(relative_lpf);
            std::fs::create_dir_all(dst_file.parent().unwrap()).unwrap();
            fs::copy(&linked_depend_file, &dst_file).unwrap();
        }
    }

    if cfg!(target_os = "windows") {
        let pdb_fname = "alxr_client.pdb";
        fs::copy(
            artifacts_dir.join(&pdb_fname),
            alxr_client_build_dir.join(&pdb_fname),
        )
        .unwrap();
    }

    let alxr_client_fname = afs::exec_fname("alxr-client");
    fs::copy(
        artifacts_dir.join(&alxr_client_fname),
        alxr_client_build_dir.join(&alxr_client_fname),
    )
    .unwrap();
}

fn setup_cargo_appimage() {
    let ait_dir = afs::deps_dir().join("linux/appimagetool");

    fs::remove_dir_all(&ait_dir).ok();
    fs::create_dir_all(&ait_dir).unwrap();

    #[cfg(target_arch = "x86_64")]
    let target_arch_str = "x86_64";
    #[cfg(target_arch = "x86")]
    let target_arch_str = "i686";
    #[cfg(target_arch = "aarch64")]
    let target_arch_str = "aarch64";
    #[cfg(target_arch = "arm")]
    let target_arch_str = "armhf";

    let ait_exe = format!("appimagetool-{}.AppImage", &target_arch_str);

    let run_ait_cmd = |cmd: &str| command::run_in(&ait_dir, &cmd).unwrap();
    run_ait_cmd(&format!(
        "wget https://github.com/AppImage/AppImageKit/releases/download/13/{}",
        &ait_exe
    ));
    run_ait_cmd(&format!("mv {} appimagetool", &ait_exe));
    run_ait_cmd("chmod +x appimagetool");

    assert!(ait_dir.exists());

    env::set_var(
        "PATH",
        format!(
            "{}:{}",
            ait_dir.canonicalize().unwrap().to_str().unwrap(),
            env::var("PATH").unwrap_or_default()
        ),
    );

    command::run("cargo install cargo-appimage").unwrap();
}

pub fn build_alxr_app_image(root: Option<String>, _ffmpeg_version: &str, flags: AlxBuildFlags) {
    println!("Not Implemented!");
    // setup_cargo_appimage();

    // // let target_dir = afs::target_dir();

    // // let bundle_ffmpeg_enabled = cfg!(target_os = "linux") && flags.bundle_ffmpeg;
    // // if bundle_ffmpeg_enabled {
    // //     assert!(!ffmpeg_version.is_empty(), "ffmpeg-version is empty!");

    // //     let ffmpeg_lib_dir = &alxr_client_build_dir;
    // //     dependencies::build_ffmpeg_linux_install(true, ffmpeg_version, /*enable_decoders=*/true, &ffmpeg_lib_dir);

    // //     assert!(ffmpeg_lib_dir.exists());
    // //     env::set_var("ALXR_BUNDLE_FFMPEG_INSTALL_PATH", ffmpeg_lib_dir.to_str().unwrap());
    // // }

    // if let Some(root) = root {
    //     env::set_var("ALVR_ROOT_DIR", root);
    // }
    // if flags.fetch_crates {
    //     command::run("cargo update").unwrap();
    // }
    // let build_flags = flags.make_build_string();
    // let alxr_client_dir = afs::workspace_dir().join("alvr/openxr-client/alxr-client");

    // let rustflags = r#"RUSTFLAGS="-C link-args=-Wl,-rpath,$ORIGIN/lib""#;
    // //env::set_var("RUSTFLAGS", "-C link-args=\'-Wl,-rpath,$ORIGIN/lib\'");
    // command::run_in(&alxr_client_dir, &format!("{} cargo appimage {}", rustflags, build_flags)).unwrap();
}

fn install_alxr_depends() {
    command::run("rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android").unwrap();
    command::run("cargo install cargo-apk --git https://github.com/korejan/android-ndk-rs.git --branch android-manifest-entries").unwrap();
}

#[derive(Clone, Copy, Debug)]
pub enum AndroidFlavor {
    Generic,
    OculusQuest, // Q1 or Q2
    PicoNeo3,
}

pub fn build_alxr_android(
    root: Option<String>,
    client_flavor: AndroidFlavor,
    flags: AlxBuildFlags,
) {
    let build_type = if flags.is_release { "release" } else { "debug" };
    let build_flags = flags.make_build_string();

    if let Some(root) = root {
        env::set_var("ALVR_ROOT_DIR", root);
    }

    if flags.fetch_crates {
        command::run("cargo update").unwrap();
    }
    install_alxr_depends();

    let alxr_client_build_dir = afs::alxr_android_build_dir(build_type);
    //fs::remove_dir_all(&alxr_client_build_dir).ok();
    fs::create_dir_all(&alxr_client_build_dir).unwrap();

    let client_dir = match client_flavor {
        AndroidFlavor::OculusQuest => "quest",
        AndroidFlavor::PicoNeo3 => "pico-neo",
        _ => "",
    };
    // cargo-apk has an issue where it will search the entire "target" build directory for "output" files that contain
    // a build.rs print of out "cargo:rustc-link-search=...." and use those paths to determine which
    // shared libraries copy into the final apk, this can causes issues if there are multiple versions of shared libs
    // with the same name.
    //     E.g.: The wrong platform build of libopenxr_loader.so gets copied into the wrong apk when
    //           more than one variant of android client gets built.
    // The workaround is set different "target-dir" for each variant/flavour of android builds.
    let target_dir = afs::target_dir().join(client_dir);
    let alxr_client_dir = afs::workspace_dir()
        .join("alvr/openxr-client/alxr-android-client")
        .join(client_dir);

    command::run_in(
        &alxr_client_dir,
        &format!(
            "cargo apk build {0} --target-dir={1}",
            build_flags,
            target_dir.display()
        ),
    )
    .unwrap();

    fn is_package_file(p: &Path) -> bool {
        p.extension().map_or(false, |ext| {
            let ext_str = ext.to_str().unwrap();
            return ["apk", "aar", "idsig"].contains(&ext_str);
        })
    }
    let apk_dir = target_dir.join(build_type).join("apk");
    for file in walkdir::WalkDir::new(&apk_dir)
        .into_iter()
        .filter_map(|maybe_entry| maybe_entry.ok())
        .map(|entry| entry.into_path())
        .filter(|entry| is_package_file(&entry))
    {
        let relative_lpf = file.strip_prefix(&apk_dir).unwrap();
        let dst_file = alxr_client_build_dir.join(relative_lpf);
        std::fs::create_dir_all(dst_file.parent().unwrap()).unwrap();
        fs::copy(&file, &dst_file).unwrap();
    }
}

// Avoid Oculus link popups when debugging the client
pub fn kill_oculus_processes() {
    command::run_without_shell(
        "powershell",
        &[
            "Start-Process",
            "taskkill",
            "-ArgumentList",
            "\"/F /IM OVR* /T\"",
            "-Verb",
            "runAs",
        ],
    )
    .unwrap();
}

fn clippy() {
    command::run(&format!(
        "cargo clippy {} -- {} {} {} {} {} {} {} {} {} {} {}",
        "-p alvr_xtask -p alvr_common -p alvr_launcher -p alvr_dashboard", // todo: add more crates when they compile correctly
        "-W clippy::clone_on_ref_ptr -W clippy::create_dir -W clippy::dbg_macro",
        "-W clippy::decimal_literal_representation -W clippy::else_if_without_else",
        "-W clippy::exit -W clippy::expect_used -W clippy::filetype_is_file",
        "-W clippy::float_cmp_const -W clippy::get_unwrap -W clippy::let_underscore_must_use",
        "-W clippy::lossy_float_literal -W clippy::map_err_ignore -W clippy::mem_forget",
        "-W clippy::multiple_inherent_impl -W clippy::print_stderr -W clippy::print_stderr",
        "-W clippy::rc_buffer -W clippy::rest_pat_in_fully_bound_structs -W clippy::str_to_string",
        "-W clippy::string_to_string -W clippy::todo -W clippy::unimplemented",
        "-W clippy::unneeded_field_pattern -W clippy::unwrap_in_result",
        "-W clippy::verbose_file_reads -W clippy::wildcard_enum_match_arm",
        "-W clippy::wrong_pub_self_convention"
    ))
    .unwrap();
}

fn prettier() {
    command::run("npx -p prettier@2.2.1 prettier --config alvr/xtask/.prettierrc --write '**/*[!.min].{css,js}'").unwrap();
}

fn main() {
    let begin_time = Instant::now();

    env::set_var("RUST_BACKTRACE", "1");

    let mut args = Arguments::from_env();

    if args.contains(["-h", "--help"]) {
        println!("{HELP_STR}");
    } else if let Ok(Some(subcommand)) = args.subcommand() {
        let fetch = args.contains("--fetch");
        let is_release = args.contains("--release");
        let experiments = args.contains("--experiments");
        let version: Option<String> = args.opt_value_from_str("--version").unwrap();
        let is_nightly = args.contains("--nightly");
        // android flavours
        let for_oculus_quest = args.contains("--oculus-quest");
        let for_oculus_go = args.contains("--oculus-go");
        let for_generic = args.contains("--generic");
        let for_pico_neo = args.contains("--pico-neo");
        let for_all_flavors = args.contains("--all-flavors");
        //
        let bundle_ffmpeg = args.contains("--bundle-ffmpeg");
        let no_nvidia = args.contains("--no-nvidia");
        let gpl = args.contains("--gpl");
        let reproducible = args.contains("--reproducible");
        let root: Option<String> = args.opt_value_from_str("--root").unwrap();

        let default_var = String::from("n5.0");
        let mut ffmpeg_version: String =
            args.opt_value_from_str("--ffmpeg-version").unwrap().map_or(
                default_var.clone(),
                |s: String| if s.is_empty() { default_var } else { s },
            );
        assert!(!ffmpeg_version.is_empty());
        if !ffmpeg_version.starts_with('n') {
            ffmpeg_version.insert(0, 'n');
        }

        if args.finish().is_empty() {
            match subcommand.as_str() {
                "build-windows-deps" => dependencies::build_deps("windows"),
                "build-android-deps" => dependencies::build_deps("android"),
                "build-server" => build_server(
                    is_release,
                    experiments,
                    fetch,
                    bundle_ffmpeg,
                    no_nvidia,
                    gpl,
                    root,
                    reproducible,
                ),
                "build-client" => {
                    if (for_oculus_quest && for_oculus_go) || (!for_oculus_quest && !for_oculus_go)
                    {
                        build_client(is_release, false, false);
                        build_client(is_release, false, true);
                    } else {
                        build_client(is_release, false, for_oculus_go);
                    }
                }
                "build-alxr-client" => build_alxr_client(
                    root,
                    &ffmpeg_version,
                    AlxBuildFlags {
                        is_release: is_release,
                        reproducible: reproducible,
                        no_nvidia: no_nvidia,
                        bundle_ffmpeg: bundle_ffmpeg,
                        fetch_crates: fetch,
                        ..Default::default()
                    },
                ),
                "build-alxr-appimage" => build_alxr_app_image(
                    root,
                    &ffmpeg_version,
                    AlxBuildFlags {
                        is_release: is_release,
                        reproducible: reproducible,
                        no_nvidia: no_nvidia,
                        bundle_ffmpeg: bundle_ffmpeg,
                        fetch_crates: fetch,
                        ..Default::default()
                    },
                ),
                "build-alxr-android" => {
                    let build_flags = AlxBuildFlags {
                        is_release: is_release,
                        reproducible: reproducible,
                        no_nvidia: true,
                        bundle_ffmpeg: false,
                        fetch_crates: fetch,
                        ..Default::default()
                    };
                    let flavours = vec![
                        (for_generic, AndroidFlavor::Generic),
                        (for_oculus_quest, AndroidFlavor::OculusQuest),
                        (for_pico_neo, AndroidFlavor::PicoNeo3),
                    ];

                    for (_, flavour) in flavours.iter().filter(|(f, _)| for_all_flavors || *f) {
                        build_alxr_android(root.clone(), *flavour, build_flags);
                    }
                    if !for_all_flavors && flavours.iter().all(|(flag, _)| !flag) {
                        build_alxr_android(root, AndroidFlavor::Generic, build_flags);
                    }
                }
                "build-alxr-quest" => build_alxr_android(
                    root,
                    AndroidFlavor::OculusQuest,
                    AlxBuildFlags {
                        is_release: is_release,
                        reproducible: reproducible,
                        no_nvidia: true,
                        bundle_ffmpeg: false,
                        fetch_crates: fetch,
                        ..Default::default()
                    },
                ),
                "build-alxr-pico" => build_alxr_android(
                    root,
                    AndroidFlavor::PicoNeo3,
                    AlxBuildFlags {
                        is_release: is_release,
                        reproducible: reproducible,
                        no_nvidia: true,
                        bundle_ffmpeg: false,
                        fetch_crates: fetch,
                        ..Default::default()
                    },
                ),
                "build-ffmpeg-linux" => {
                    dependencies::build_ffmpeg_linux(true);
                }
                "build-ffmpeg-linux-no-nvidia" => {
                    dependencies::build_ffmpeg_linux(false);
                }
                "publish-server" => packaging::publish_server(is_nightly, root, reproducible, gpl),
                "publish-client" => packaging::publish_client(is_nightly),
                "clean" => remove_build_dir(),
                "kill-oculus" => kill_oculus_processes(),
                "bump-versions" => version::bump_version(version, is_nightly),
                "bump-alxr-versions" => version::bump_alxr_version(version, is_nightly),
                "clippy" => clippy(),
                "prettier" => prettier(),
                _ => {
                    println!("\nUnrecognized subcommand.");
                    println!("{HELP_STR}");
                    return;
                }
            }
        } else {
            println!("\nWrong arguments.");
            println!("{HELP_STR}");
            return;
        }
    } else {
        println!("\nMissing subcommand.");
        println!("{HELP_STR}");
        return;
    }

    let elapsed_time = Instant::now() - begin_time;

    println!(
        "\nDone [{}m {}s]\n",
        elapsed_time.as_secs() / 60,
        elapsed_time.as_secs() % 60
    );
}
