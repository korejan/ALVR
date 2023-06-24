mod build;
mod command;
mod dependencies;
mod packaging;
mod version;

use crate::build::Profile;
use afs::Layout;
use alvr_filesystem as afs;
use pico_args::Arguments;
use std::{fs, time::Instant};
use xshell::{cmd, Shell};

const HELP_STR: &str = r#"
cargo xtask
Developement actions for ALVR.

USAGE:
    cargo xtask <SUBCOMMAND> [FLAG] [ARGS]

SUBCOMMANDS:
    prepare-deps        Download and compile streamer and client external dependencies
    build-streamer      Build streamer, then copy binaries to build folder
    build-client        Build client, then copy binaries to build folder
    build-client-lib    Build a C-ABI ALVR client library and header.
    run-streamer        Build streamer and then open the dashboard
    package-streamer    Build streamer in release mode, make portable version and installer
    package-client-lib  Build client library then zip it
    clean               Removes all build artifacts and dependencies.
    bump                Bump streamer and client package versions
    clippy              Show warnings for selected clippy lints
    kill-oculus         Kill all Oculus processes

FLAGS:
    --help              Print this text
    --keep-config       Preserve the configuration file between rebuilds (session.json)
    --no-nvidia         Disables nVidia support on Linux. For prepare-deps subcommand
    --release           Optimized build with less debug checks. For build subcommands
    --gpl               Bundle GPL libraries (FFmpeg). Only for Windows
    --appimage          Package as AppImage. For package-streamer subcommand
    --zsync             For --appimage, create .zsync update file and build AppImage with embedded update information. For package-streamer subcommand
    --nightly           Append nightly tag to versions. For bump subcommand
    --no-rebuild        Do not rebuild the streamer with run-streamer
    --ci                Do some CI related tweaks. Depends on the other flags and subcommand
    --no-stdcpp         Disable linking to libc++_shared with build-client-lib

ARGS:
    --platform <NAME>   Name of the platform (operative system or hardware name). snake_case
    --version <VERSION> Specify version to set with the bump-versions subcommand
    --root <PATH>       Installation root. By default no root is set and paths are calculated using
                        relative paths, which requires conforming to FHS on Linux.
"#;

pub fn run_streamer() {
    let sh = Shell::new().unwrap();

    let dashboard_exe = Layout::new(&afs::streamer_build_dir()).dashboard_exe();

    cmd!(sh, "{dashboard_exe}").run().unwrap();
}

pub fn clean() {
    fs::remove_dir_all(afs::build_dir()).ok();
    fs::remove_dir_all(afs::deps_dir()).ok();
    if afs::target_dir() == afs::workspace_dir().join("target") {
        // Detete target folder only if in the local wokspace!
        fs::remove_dir_all(afs::target_dir()).ok();
    }
}

fn clippy() {
    // lints updated for Rust 1.59
    let restriction_lints = [
        "allow_attributes_without_reason",
        "clone_on_ref_ptr",
        "create_dir",
        "decimal_literal_representation",
        "else_if_without_else",
        "expect_used",
        "float_cmp_const",
        "fn_to_numeric_cast_any",
        "get_unwrap",
        "if_then_some_else_none",
        "let_underscore_must_use",
        "lossy_float_literal",
        "mem_forget",
        "multiple_inherent_impl",
        "rest_pat_in_fully_bound_structs",
        // "self_named_module_files",
        "str_to_string",
        // "string_slice",
        "string_to_string",
        "try_err",
        "unnecessary_self_imports",
        "unneeded_field_pattern",
        "unseparated_literal_suffix",
        "verbose_file_reads",
        "wildcard_enum_match_arm",
    ];
    let pedantic_lints = [
        "borrow_as_ptr",
        "enum_glob_use",
        "explicit_deref_methods",
        "explicit_into_iter_loop",
        "explicit_iter_loop",
        "filter_map_next",
        "flat_map_option",
        "float_cmp",
        // todo: add more lints
    ];

    let flags = restriction_lints
        .into_iter()
        .chain(pedantic_lints)
        .flat_map(|name| ["-W".to_owned(), format!("clippy::{name}")]);

    let sh = Shell::new().unwrap();
    cmd!(sh, "cargo clippy -- {flags...}").run().unwrap();
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
    command::run("cargo install cargo-apk --git https://github.com/korejan/android-ndk-rs.git --branch android-manifest-entries").unwrap();
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
    let sh = Shell::new().unwrap();
    cmd!(
        sh,
        "powershell Start-Process taskkill -ArgumentList \"/F /IM OVR* /T\" -Verb runas"
    )
    .run()
    .unwrap();
}

fn main() {
    let begin_time = Instant::now();

    let mut args = Arguments::from_env();

    if args.contains(["-h", "--help"]) {
        println!("{HELP_STR}");
    } else if let Ok(Some(subcommand)) = args.subcommand() {
        let no_nvidia = args.contains("--no-nvidia");
        let is_release = args.contains("--release");
        let profile = if is_release {
            Profile::Release
        } else {
            Profile::Debug
        };
        let gpl = args.contains("--gpl");
        let is_nightly = args.contains("--nightly");
        let no_rebuild = args.contains("--no-rebuild");
        let for_ci = args.contains("--ci");
        let keep_config = args.contains("--keep-config");
        let appimage = args.contains("--appimage");
        let zsync = args.contains("--zsync");
        let link_stdcpp = !args.contains("--no-stdcpp");

        let platform: Option<String> = args.opt_value_from_str("--platform").unwrap();
        let version: Option<String> = args.opt_value_from_str("--version").unwrap();
        let root: Option<String> = args.opt_value_from_str("--root").unwrap();

        let default_var = String::from("release/6.0");
        let mut ffmpeg_version: String =
            args.opt_value_from_str("--ffmpeg-version").unwrap().map_or(
                default_var.clone(),
                |s: String| if s.is_empty() { default_var } else { s },
            );
        assert!(!ffmpeg_version.is_empty());

        if args.finish().is_empty() {
            match subcommand.as_str() {
                "prepare-deps" => {
                    if let Some(platform) = platform {
                        match platform.as_str() {
                            "windows" => dependencies::prepare_windows_deps(for_ci),
                            "linux" => dependencies::build_ffmpeg_linux(!no_nvidia),
                            "android" => dependencies::build_android_deps(for_ci),
                            _ => panic!("Unrecognized platform."),
                        }
                    } else {
                        if cfg!(windows) {
                            dependencies::prepare_windows_deps(for_ci);
                        } else if cfg!(target_os = "linux") {
                            dependencies::build_ffmpeg_linux(!no_nvidia);
                        }

                        dependencies::build_android_deps(for_ci);
                    }
                }
                "build-streamer" => build::build_streamer(profile, gpl, None, false, keep_config),
                "build-client" => build::build_android_client(profile),
                "build-client-lib" => build::build_client_lib(profile, link_stdcpp),
                "run-streamer" => {
                    if !no_rebuild {
                        build::build_streamer(profile, gpl, None, false, keep_config);
                    }
                    run_streamer();
                }
                "package-streamer" => packaging::package_streamer(gpl, root, appimage, zsync),
                "package-client" => build::build_android_client(Profile::Distribution),
                "package-client-lib" => packaging::package_client_lib(link_stdcpp),
                "clean" => clean(),
                "bump" => version::bump_version(version, is_nightly),
                "clippy" => clippy(),
                "kill-oculus" => kill_oculus_processes(),
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
