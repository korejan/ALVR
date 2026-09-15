use crate::command::{self, run_as_bash_in as bash_in};
use alvr_filesystem as afs;
use std::{fs, io::BufRead, path::Path};

fn download_and_extract_zip(url: &str, destination: &Path) {
    let zip_file = afs::deps_dir().join("temp_download.zip");

    fs::remove_file(&zip_file).ok();
    fs::create_dir_all(afs::deps_dir()).unwrap();
    command::download(url, &zip_file).unwrap();

    fs::remove_dir_all(&destination).ok();
    fs::create_dir_all(&destination).unwrap();
    command::unzip(&zip_file, destination).unwrap();

    fs::remove_file(zip_file).unwrap();
}

fn download_and_extract_tarxz(url: &str, destination: &Path) {
    let tar_file = afs::deps_dir().join("temp_download.tar.xz");

    fs::remove_file(&tar_file).ok();
    fs::create_dir_all(afs::deps_dir()).unwrap();
    command::download(url, &tar_file).unwrap();

    fs::remove_dir_all(&destination).ok();
    fs::create_dir_all(&destination).unwrap();

    std::process::Command::new("tar")
        .args([
            "-xJf",
            &tar_file.to_string_lossy(),
            "-C",
            &destination.to_string_lossy(),
        ])
        .status()
        .expect("Failed to extract tar.xz archive");

    fs::remove_file(tar_file).unwrap();
}

/// Patch rpath of all shared libraries in a directory to include $ORIGIN
/// This ensures libraries can find their dependencies in the same directory
fn patch_rpath(lib_dir: &Path) {
    for entry in walkdir::WalkDir::new(lib_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().contains(".so"))
                .unwrap_or(false)
        })
    {
        // Use patchelf to set rpath to $ORIGIN so libs find each other
        let status = std::process::Command::new("patchelf")
            .args(["--set-rpath", "$ORIGIN", &entry.to_string_lossy()])
            .status();

        match status {
            Ok(s) if s.success() => {
                println!("Patched rpath for: {}", entry.display());
            }
            Ok(s) => {
                eprintln!(
                    "Warning: patchelf returned non-zero for {}: {}",
                    entry.display(),
                    s
                );
            }
            Err(e) => {
                eprintln!(
                    "Warning: Failed to run patchelf on {}: {}",
                    entry.display(),
                    e
                );
            }
        }
    }
}

pub fn extract_ffmpeg_windows() -> std::path::PathBuf {
    let download_path = afs::deps_dir().join("windows");
    let ffmpeg_path = download_path.join("ffmpeg-n9.0-latest-win64-gpl-shared-9.0");
    if !ffmpeg_path.exists() {
        download_and_extract_zip(
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n9.0-latest-win64-gpl-shared-9.0.zip",
            &download_path,
        );
    }

    ffmpeg_path
}

pub fn extract_ffmpeg_linux(version: &str, gpl: bool) -> std::path::PathBuf {
    let arch = if cfg!(target_arch = "x86_64") {
        "64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        panic!("Unsupported architecture")
    };

    let base_filename = format!(
        "ffmpeg-n{}-latest-linux{}-{}-shared-{}",
        version,
        arch,
        if gpl { "gpl" } else { "lgpl" },
        version
    );

    let url = format!(
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/{}.tar.xz",
        base_filename
    );

    let download_path = afs::deps_dir().join("linux");
    let mut ffmpeg_path = download_path.join(&base_filename);
    if !ffmpeg_path.exists() {
        download_and_extract_tarxz(&url, &download_path);
        assert!(ffmpeg_path.exists(), "FFmpeg extraction failed");
        ffmpeg_path = dunce::canonicalize(ffmpeg_path).unwrap();
        // Patch rpath to $ORIGIN so libraries find each other at runtime
        patch_rpath(&ffmpeg_path.join("lib"));
    }
    assert!(ffmpeg_path.exists(), "FFmpeg deps path does not exist!");
    dunce::canonicalize(ffmpeg_path).unwrap()
}

fn get_oculus_openxr_mobile_loader() {
    let temp_sdk_dir = afs::build_dir().join("temp_download");

    // OpenXR SDK v1.0.18. todo: upgrade when new version is available
    download_and_extract_zip(
        "https://securecdn.oculus.com/binaries/download/?id=4421717764533443",
        &temp_sdk_dir,
    );

    let destination_dir = afs::deps_dir().join("android/oculus_openxr/arm64-v8a");
    fs::create_dir_all(&destination_dir).unwrap();

    fs::copy(
        temp_sdk_dir.join("OpenXR/Libs/Android/arm64-v8a/Release/libopenxr_loader.so"),
        destination_dir.join("libopenxr_loader.so"),
    )
    .unwrap();

    fs::remove_dir_all(temp_sdk_dir).ok();
}

pub fn build_deps(target_os: &str) {
    if target_os == "android" {
        command::run("rustup target add aarch64-linux-android").unwrap();
        command::run("cargo install cargo-apk").unwrap();

        get_oculus_openxr_mobile_loader();
    } else {
        println!("Nothing to do for {target_os}!")
    }
}

pub fn find_resolved_so_paths(
    bin_or_so: &std::path::Path,
    depends_so: &str,
) -> Vec<std::path::PathBuf> {
    let cmdline = format!(
        "ldd {} | cut -d '>' -f 2 | awk \'{{print $1}}\' | grep {}",
        bin_or_so.display(),
        depends_so
    );
    std::process::Command::new("sh")
        .args(&["-c", &cmdline])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_or(vec![], |mut child| {
            let mut result = std::io::BufReader::new(child.stdout.take().unwrap())
                .lines()
                .filter(|line| line.is_ok())
                .map(|line| std::path::PathBuf::from(line.unwrap()).canonicalize()) // canonicalize resolves symlinks
                .filter(|result| result.is_ok())
                .map(|pp| pp.unwrap())
                .collect::<Vec<_>>();
            result.dedup();
            result
        })
}
