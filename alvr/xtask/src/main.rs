mod command;
mod dependencies;
mod packaging;
mod version;

use alvr_filesystem::{self as afs, Layout};
use fs_extra::{self as fsx, dir as dirx};
use pico_args::Arguments;
use std::{env, fs, time::Instant};
use std::collections::HashSet;
use std::error::Error;
use std::process::{Stdio};
use std::io::BufReader;
use cargo_metadata::{Message};
use camino::{Utf8PathBuf};

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
    build-alxr-android  Build OpenXR based client (android platforms only), then copy binaries to build folder
    build-ffmpeg-linux  Build FFmpeg with VAAPI, NvEnc and Vulkan support. Only for CI
    publish-server      Build server in release mode, make portable version and installer
    publish-client      Build client for all headsets
    clean               Removes build folder
    kill-oculus         Kill all Oculus processes
    bump-versions       Bump server and client package versions
    clippy              Show warnings for selected clippy lints
    prettier            Format JS and CSS files with prettier; Requires Node.js and NPM.

FLAGS:
    --reproducible      Force cargo to build reproducibly. Used only for build subcommands
    --fetch             Update crates with "cargo update". Used only for build subcommands
    --release           Optimized build without debug info. Used only for build subcommands
    --experiments       Build unfinished features
    --nightly           Bump versions to nightly and build. Used only for publish subcommand
    --oculus-quest      Oculus Quest build. Used only for build-client subcommand
    --oculus-go         Oculus Go build. Used only for build-client subcommand
    --bundle-ffmpeg     Bundle ffmpeg libraries. Only used for build-server subcommand on Linux
    --no-nvidia         Additional flag to use with `build-server`. Disables nVidia support.
    --gpl               Enables usage of GPL libs like ffmpeg on Windows, allowing software encoding.
    --help              Print this text

ARGS:
    --version <VERSION> Specify version to set with the bump-versions subcommand
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
        fs::create_dir_all(lib_dir.clone()).unwrap();
        for lib in walkdir::WalkDir::new(ffmpeg_path)
            .into_iter()
            .filter_map(|maybe_entry| maybe_entry.ok())
            .map(|entry| entry.into_path())
            .filter(|path| path.file_name().unwrap().to_string_lossy().contains(".so."))
        {
            fs::copy(lib.clone(), lib_dir.join(lib.file_name().unwrap())).unwrap();
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
fn find_linked_native_paths(crate_path: &Path, build_flags:&str) -> Result<PathSet, Box<dyn Error> > {
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
                        Some(p) => { linked_path_set.insert(p.into()); },
                        None => ()
                    }
                }
            },
            _ => ()
        }
    }
    Ok(linked_path_set)
}

fn make_build_flags(is_release: bool, reproducible: bool) -> String {
    format!("{} {}",
        if is_release { "--release" } else { "" },
        if reproducible {
            "--offline --locked"
        } else {
            ""
        }
    )
}

pub fn build_alxr_client(
    is_release: bool,
    fetch_crates: bool,
    root: Option<String>,
    reproducible: bool,
) {   
    let build_type = if is_release { "release" } else { "debug" };
    let build_flags = make_build_flags(is_release, reproducible);

    if let Some(root) = root {
        env::set_var("ALVR_ROOT_DIR", root);
    }

    let target_dir = afs::target_dir();
    let artifacts_dir = target_dir.join(build_type);

    if fetch_crates {
        command::run("cargo update").unwrap();
    }

    let alxr_client_build_dir = afs::alxr_client_build_dir();
    fs::remove_dir_all(&alxr_client_build_dir).ok();
    fs::create_dir_all(&alxr_client_build_dir).unwrap();

    let alxr_client_dir = afs::workspace_dir().join("alvr/openxr-client/alxr-client");
    let (alxr_cargo_cmd, alxr_build_lid_dir) = if cfg!(target_os = "windows") {
        (format!("cargo build {}", build_flags),
        alxr_client_build_dir.to_owned())
    }
    else {
        (format!("cargo rustc {} -- -C link-args=\'-Wl,-rpath,$ORIGIN/lib\'", build_flags),
        alxr_client_build_dir.join("lib"))
    };
    command::run_in(&alxr_client_dir, &alxr_cargo_cmd).unwrap();
        
    fn is_linked_depends_file(path:&Path) -> bool {
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
    let linked_paths = find_linked_native_paths(&alxr_client_dir, &build_flags)
        .unwrap();
    for linked_path in linked_paths.iter() {
        for linked_depend_file in walkdir::WalkDir::new(linked_path)
            .into_iter()
            .filter_map(|maybe_entry| maybe_entry.ok())
            .map(|entry| entry.into_path())
            .filter(|entry| is_linked_depends_file(&entry)) {
            
            let relative_lpf = linked_depend_file.strip_prefix(linked_path).unwrap();
            let dst_file = alxr_build_lid_dir.join(relative_lpf);            
            std::fs::create_dir_all(dst_file.parent().unwrap()).unwrap();
            fs::copy(&linked_depend_file, &dst_file).unwrap();
        }
    }
    
    if cfg!(target_os = "windows") {
        let pdb_fname = "alxr_client.pdb";
        fs::copy(
            artifacts_dir.join(&pdb_fname),
            alxr_client_build_dir.join(&pdb_fname)
        )
        .unwrap();
    }

    let alxr_client_fname = afs::exec_fname("alxr-client");
    fs::copy(
        artifacts_dir.join(&alxr_client_fname),
        alxr_client_build_dir.join(&alxr_client_fname)
    )
    .unwrap();
}

pub fn build_alxr_android(
    is_release: bool,
    fetch_crates: bool,
    root: Option<String>,
    reproducible: bool,
) {   
    let build_type = if is_release { "release" } else { "debug" };
    let build_flags = make_build_flags(is_release, reproducible);

    if let Some(root) = root {
        env::set_var("ALVR_ROOT_DIR", root);
    }

    if fetch_crates {
        command::run("cargo update").unwrap();
    }
    command::run("cargo install cargo-apk").unwrap();
    
    let alxr_client_build_dir = afs::alxr_android_build_dir();
    fs::remove_dir_all(&alxr_client_build_dir).ok();
    fs::create_dir_all(&alxr_client_build_dir).unwrap();

    let alxr_client_dir = afs::workspace_dir().join("alvr/openxr-client/alxr-android-client");
    command::run_in
    (
        &alxr_client_dir,
        &format!("cargo apk build {}", build_flags)
    ).unwrap();

    fn is_package_file(p: &Path) -> bool {
        p.extension().map_or(false, |ext| {
            let ext_str = ext.to_str().unwrap();
            return ["apk", "aar", "idsig"].contains(&ext_str);
        })
    }
    let apk_dir = afs::target_dir().join(build_type).join("apk");
    for file in walkdir::WalkDir::new(&apk_dir)
        .into_iter()
        .filter_map(|maybe_entry| maybe_entry.ok())
        .map(|entry| entry.into_path())
        .filter(|entry| is_package_file(&entry)) {

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
        let for_oculus_quest = args.contains("--oculus-quest");
        let for_oculus_go = args.contains("--oculus-go");
        let bundle_ffmpeg = args.contains("--bundle-ffmpeg");
        let no_nvidia = args.contains("--no-nvidia");
        let gpl = args.contains("--gpl");
        let reproducible = args.contains("--reproducible");
        let root: Option<String> = args.opt_value_from_str("--root").unwrap();

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
                    is_release,
                    fetch,
                    root,
                    reproducible,
                ),
                "build-alxr-android" => build_alxr_android(
                    is_release,
                    fetch,
                    root,
                    reproducible,
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
