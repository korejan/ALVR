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
use std::{env, fmt, fs, io::Write, path::Path, time::Instant, vec};

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
    build-alxr-uwp      Build OpenXR based client for Windows UWP/Store (.msixbundle - self-signed app-bundle with x64+arm64)
    build-alxr-uwp-x64  Build OpenXR based client for Windows UWP/Store (x64 .msix app-package), then copy binaries to build folder
    build-alxr-uwp-arm64 Build OpenXR based client for Windows UWP/Store (arm64 .msix app-package), then copy binaries to build folder
    build-alxr-app-bundle Build UWP app-bundle (.msixbundle) from `build-alxr-uwp` builds.
    build-alxr-appimage Build OpenXR based client AppImage for linux only.
    build-alxr-android  Build OpenXR based client (android platforms only), then copy binaries to build folder
    build-alxr-quest    Build OpenXR based client for Oculus Quest (same as `build-alxr-android --oculus-quest`), then copy binaries to build folder
    build-alxr-pico     Build OpenXR based client for Pico 4/Neo 3 PUI >= 5.2.x (same as `build-alxr-android --pico`), then copy binaries to build folder
    build-alxr-pico-v4  Build OpenXR based client for Pico 4/Neo 3 PRE PUI 5.2.x (same as `build-alxr-android --pico-v4`), then copy binaries to build folder
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
    --no-decoder        Disables building decoder support and dependencies. Only used for build-alxr-*
    --oculus-ext        Enables using Oculus OpenXR extensions (headers), Used only for build-alxr-client subcommand
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

    let dir_content =
        dirx::get_dir_content2("alvr/server/resources", &dirx::DirOptions { depth: 1 }).unwrap();
    let items: Vec<&String> = dir_content.directories[1..]
        .iter()
        .chain(dir_content.files.iter())
        .collect();
    fs::create_dir_all(&layout.resources_dir()).unwrap();
    fsx::copy_items(&items, layout.resources_dir(), &dirx::CopyOptions::new()).unwrap();

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
    nightly: bool,
    env_var: Option<(&str, &str)>,
) -> Result<PathSet, Box<dyn Error>> {
    // let manifest_file = crate_path.join("Cargo.toml");
    // let metadata = MetadataCommand::new()
    //     .manifest_path(manifest_file)
    //     .exec()?;
    // let package = match metadata.root_package() {
    //     Some(p) => p,
    //     None => return Err("cargo out-dir must be run from within a crate".into()),
    // };
    let mut cmd = "cargo";
    let mut args = vec!["check", "--message-format=json", "--quiet"];
    if nightly {
        cmd = "rustup";
        let mut args1 = vec!["run", "nightly", "cargo"];
        args1.append(&mut args);
        args = args1;
    }
    args.extend(build_flags.split_ascii_whitespace());

    let mut command = std::process::Command::new(&cmd);
    if let Some((key, val)) = env_var {
        command.env(key, val);
    }
    let mut command = command
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
    no_decoder: bool,
    oculus_ext: bool,
}

impl Default for AlxBuildFlags {
    fn default() -> Self {
        AlxBuildFlags {
            is_release: true,
            reproducible: true,
            no_nvidia: true,
            bundle_ffmpeg: true,
            fetch_crates: false,
            no_decoder: false,
            oculus_ext: false,
        }
    }
}

impl AlxBuildFlags {
    pub fn make_build_string(&self) -> String {
        let enable_bundle_ffmpeg = cfg!(target_os = "linux") && self.bundle_ffmpeg;
        let enable_oculus_ext = cfg!(target_os = "windows") && self.oculus_ext;
        let feature_map = vec![
            (enable_bundle_ffmpeg, "bundled-ffmpeg"),
            (!self.no_nvidia, "cuda-interop"),
            (self.no_decoder, "no-decoder"),
            (enable_oculus_ext, "oculus-ext-headers"),
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
            /*nvenc_flag=*/ !flags.no_nvidia,
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
            if let Some(ext) = path.extension() {
                if ext.to_str().unwrap().eq("cso") {
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
    let linked_paths =
        find_linked_native_paths(&alxr_client_dir, &build_flags, false, None).unwrap();
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

#[derive(Clone, Copy)]
pub enum UWPArch {
    X86_64,
    Aarch64,
}
impl fmt::Display for UWPArch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let x = match self {
            UWPArch::X86_64 => "x86_64",
            UWPArch::Aarch64 => "aarch64",
        };
        write!(f, "{x}")
    }
}
fn batch_arch_str(arch: UWPArch) -> &'static str {
    match arch {
        UWPArch::X86_64 => "x64",
        UWPArch::Aarch64 => "arm64",
    }
}

pub fn build_alxr_uwp(root: Option<String>, arch: UWPArch, flags: AlxBuildFlags) {
    if let Some(root) = root {
        env::set_var("ALVR_ROOT_DIR", root);
    }

    let build_flags = flags.make_build_string();
    let target_dir = afs::target_dir();
    let build_type = if flags.is_release { "release" } else { "debug" };
    let target_type = format!("{arch}-uwp-windows-msvc");
    let artifacts_dir = target_dir.join(&target_type).join(build_type);

    let alxr_client_build_dir = afs::alxr_uwp_build_dir(build_type);
    //fs::remove_dir_all(&alxr_client_build_dir).ok();
    fs::create_dir_all(&alxr_client_build_dir).unwrap();

    if flags.fetch_crates {
        command::run("cargo update").unwrap();
    }

    let alxr_client_dir = afs::workspace_dir().join("alvr/openxr-client/alxr-client/uwp");
    let batch_arch = batch_arch_str(arch);
    command::run_in(
        &alxr_client_dir,
        &format!("cargo_build_uwp.bat {batch_arch} {build_flags}"),
    )
    .unwrap();

    let file_mapping = "FinalFileMapping.ini";
    {
        let file_mapping = artifacts_dir.join("FinalFileMapping.ini");
        std::fs::copy(artifacts_dir.join("FileMapping.ini"), &file_mapping).unwrap();
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(artifacts_dir.join(&file_mapping))
            .unwrap();

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
                if let Some(ext) = path.extension() {
                    if ext.to_str().unwrap().eq("cso") {
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

        // This is a workaround to bug since rustc 1.65.0-nightly,
        // UWP runtime DLLs need to be in the system path for find_linked_native_paths work correctly.
        // refer to https://github.com/rust-lang/rust/issues/100400#issuecomment-1212109010
        let uwp_runtime_dir = alxr_client_dir.join("uwp-runtime");
        let uwp_runtime_dir = uwp_runtime_dir.to_str().unwrap();
        let uwp_rt_var_path = match arch {
            UWPArch::X86_64 => Some(("PATH", uwp_runtime_dir)),
            _ => None,
        };
        let find_flags =
            format!("-Z build-std=std,panic_abort --target {target_type} {build_flags}");
        println!("Searching for linked native dependencies, please wait this may take some time.");
        let linked_paths =
            find_linked_native_paths(&alxr_client_dir, &find_flags, true, uwp_rt_var_path).unwrap();
        for linked_path in linked_paths.iter() {
            for linked_depend_file in walkdir::WalkDir::new(linked_path)
                .into_iter()
                .filter_map(|maybe_entry| maybe_entry.ok())
                .map(|entry| entry.into_path())
                .filter(|entry| is_linked_depends_file(&entry))
            {
                let relative_path = linked_depend_file.strip_prefix(&linked_path).unwrap();
                let fname = relative_path.to_str().unwrap();
                let fp = linked_depend_file.to_string_lossy();
                let line = format!("\n\"{fp}\" \"{fname}\"");
                file.write_all(line.as_bytes()).unwrap();
            }
        }
        file.sync_all().unwrap();
    }

    let alxr_version = command::crate_version(&alxr_client_dir) + ".0";
    assert_ne!(alxr_version, "0.0.0.0");

    assert!(artifacts_dir.join("FinalFileMapping.ini").exists());
    let pack_script_path = alxr_client_dir.join("build_app_package.bat");
    assert!(pack_script_path.exists());
    let pack_script = pack_script_path.to_string_lossy();
    command::run_in(
        &artifacts_dir,
        &format!("{pack_script} {batch_arch} {alxr_version} {file_mapping}"),
    )
    .unwrap();

    let packed_fname = format!("alxr-client-uwp_{alxr_version}_{batch_arch}.msix");
    let src_packed_file = artifacts_dir.join(&packed_fname);
    assert!(src_packed_file.exists());
    let dst_packed_file = alxr_client_build_dir.join(&packed_fname);
    fs::copy(&src_packed_file, &dst_packed_file).unwrap();
}

pub fn build_alxr_app_bundle(is_release: bool) {
    let build_type = if is_release { "release" } else { "debug" };
    let alxr_client_build_dir = afs::alxr_uwp_build_dir(build_type).canonicalize().unwrap();
    if !alxr_client_build_dir.exists() {
        eprintln!("uwp build directory does not exist, please run `cargo xtask build-alxr-uwp(-(x64|arm64)` first.");
        return;
    }

    let alxr_client_dir = afs::workspace_dir().join("alvr/openxr-client/alxr-client/uwp");
    let alxr_version = command::crate_version(&alxr_client_dir) + ".0";
    assert_ne!(alxr_version, "0.0.0.0");

    let alxr_client_build_dir = alxr_client_build_dir.to_str().unwrap();
    let alxr_client_build_dir = std::path::PathBuf::from(
        alxr_client_build_dir
            .strip_prefix(r#"\\?\"#)
            .unwrap_or(&alxr_client_build_dir),
    );

    let pack_map_fname = "PackMap.txt";
    let pack_map_path = alxr_client_build_dir.join(&pack_map_fname);
    let mut archs: Vec<&str> = Vec::new();
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for arch in [UWPArch::X86_64, UWPArch::Aarch64].map(|x| batch_arch_str(x)) {
        let pack_fname = format!("alxr-client-uwp_{alxr_version}_{arch}.msix");
        let pack_path = alxr_client_build_dir.join(&pack_fname);
        if pack_path.exists() {
            archs.push(arch);
            files.push(pack_path);
        }
    }

    if files.is_empty() {
        eprintln!(
            "No .msix files found, please run `cargo xtask build-alxr-uwp(-(x64|arm64)` first."
        );
        return;
    }

    {
        let mut pack_map_file = fs::File::create(&pack_map_path).unwrap();
        writeln!(pack_map_file, "[Files]").unwrap();
        for pack_path in files {
            let pack_fname = pack_path.file_name().unwrap();
            writeln!(pack_map_file, "{pack_path:?} {pack_fname:?}").unwrap();
        }
        pack_map_file.sync_all().unwrap();
    }
    assert!(pack_map_path.exists());

    let cert = "alxr_client_TemporaryKey.pfx";
    // copy exported code signing cert
    fs::copy(
        alxr_client_dir.join(&cert),
        alxr_client_build_dir.join(&cert),
    )
    .unwrap();

    let mut archs = archs.join("_");
    if !is_release {
        archs = archs + "_debug";
    }
    let bundle_script_path = alxr_client_dir.join("build_app_bundle.bat");
    let bundle_cmd = format!(
        "{} {archs} {alxr_version} {pack_map_fname} {cert}",
        bundle_script_path.to_string_lossy()
    );
    command::run_in(&alxr_client_build_dir, &bundle_cmd).unwrap();
}

fn _setup_cargo_appimage() {
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

pub fn build_alxr_app_image(_root: Option<String>, _ffmpeg_version: &str, _flags: AlxBuildFlags) {
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
    command::run("cargo install cargo-apk --git https://github.com/korejan/cargo-apk.git --branch android-manifest-entries").unwrap();
}

#[derive(Clone, Copy, Debug)]
pub enum AndroidFlavor {
    Generic,
    OculusQuest, // Q1 or Q2
    Pico,        // PUI >= 5.2.x
    PicoV4,      // PUI >= 4.7.x && < 5.2.x
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
        AndroidFlavor::Pico => "pico",
        AndroidFlavor::PicoV4 => "pico-v4",
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
        let for_pico = args.contains("--pico");
        let for_pico_neo_v4 = args.contains("--pico-v4");
        let for_all_flavors = args.contains("--all-flavors");
        let oculus_ext = args.contains("--oculus-ext");
        //
        let no_decoder = args.contains("--no-decoder");
        let bundle_ffmpeg = args.contains("--bundle-ffmpeg");
        let no_nvidia = args.contains("--no-nvidia");
        let gpl = args.contains("--gpl");
        let reproducible = args.contains("--reproducible");
        let root: Option<String> = args.opt_value_from_str("--root").unwrap();

        let default_var = String::from("release/5.1");
        let mut ffmpeg_version: String =
            args.opt_value_from_str("--ffmpeg-version").unwrap().map_or(
                default_var.clone(),
                |s: String| if s.is_empty() { default_var } else { s },
            );
        assert!(!ffmpeg_version.is_empty());

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
                        no_decoder: no_decoder,
                        oculus_ext: oculus_ext,
                        ..Default::default()
                    },
                ),
                "build-alxr-uwp-x64" => build_alxr_uwp(
                    root,
                    UWPArch::X86_64,
                    AlxBuildFlags {
                        is_release: is_release,
                        reproducible: reproducible,
                        no_nvidia: true,
                        bundle_ffmpeg: false,
                        fetch_crates: fetch,
                        no_decoder: no_decoder,
                        ..Default::default()
                    },
                ),
                "build-alxr-uwp-arm64" => build_alxr_uwp(
                    root,
                    UWPArch::Aarch64,
                    AlxBuildFlags {
                        is_release: is_release,
                        reproducible: reproducible,
                        no_nvidia: true,
                        bundle_ffmpeg: false,
                        fetch_crates: fetch,
                        no_decoder: no_decoder,
                        ..Default::default()
                    },
                ),
                "build-alxr-uwp" => {
                    let build_flags = AlxBuildFlags {
                        is_release: is_release,
                        reproducible: reproducible,
                        no_nvidia: true,
                        bundle_ffmpeg: false,
                        fetch_crates: fetch,
                        no_decoder: no_decoder,
                        ..Default::default()
                    };
                    build_alxr_uwp(root.to_owned(), UWPArch::Aarch64, build_flags);
                    build_alxr_uwp(root, UWPArch::X86_64, build_flags);
                    build_alxr_app_bundle(build_flags.is_release);
                }
                "build-alxr-app-bundle" => build_alxr_app_bundle(is_release),
                "build-alxr-appimage" => build_alxr_app_image(
                    root,
                    &ffmpeg_version,
                    AlxBuildFlags {
                        is_release: is_release,
                        reproducible: reproducible,
                        no_nvidia: no_nvidia,
                        bundle_ffmpeg: bundle_ffmpeg,
                        fetch_crates: fetch,
                        no_decoder: no_decoder,
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
                        no_decoder: no_decoder,
                        ..Default::default()
                    };
                    let flavours = vec![
                        (for_generic, AndroidFlavor::Generic),
                        (for_oculus_quest, AndroidFlavor::OculusQuest),
                        (for_pico, AndroidFlavor::Pico),
                        (for_pico_neo_v4, AndroidFlavor::PicoV4),
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
                        no_decoder: no_decoder,
                        ..Default::default()
                    },
                ),
                "build-alxr-pico" => build_alxr_android(
                    root,
                    AndroidFlavor::Pico,
                    AlxBuildFlags {
                        is_release: is_release,
                        reproducible: reproducible,
                        no_nvidia: true,
                        bundle_ffmpeg: false,
                        fetch_crates: fetch,
                        no_decoder: no_decoder,
                        ..Default::default()
                    },
                ),
                "build-alxr-pico-v4" => build_alxr_android(
                    root,
                    AndroidFlavor::PicoV4,
                    AlxBuildFlags {
                        is_release: is_release,
                        reproducible: reproducible,
                        no_nvidia: true,
                        bundle_ffmpeg: false,
                        fetch_crates: fetch,
                        no_decoder: no_decoder,
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
