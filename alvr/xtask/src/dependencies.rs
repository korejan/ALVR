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

pub fn build_ffmpeg_linux_install(
    nvenc_flag: bool,
    version_tag: &str,
    enable_decoders: bool,
    install_path: &std::path::Path,
) -> std::path::PathBuf {
    /* dependencies: build-essential pkg-config nasm libva-dev libdrm-dev libvulkan-dev
                     libx264-dev libx265-dev libffmpeg-nvenc-dev nvidia-cuda-toolkit
    */

    let download_path = afs::deps_dir().join("linux");
    let ffmpeg_path = download_path.join(format!("FFmpeg-{}", version_tag.replace("/", "-")));
    if !ffmpeg_path.exists() {
        download_and_extract_zip(
            format!(
                "https://codeload.github.com/FFmpeg/FFmpeg/zip/{}",
                version_tag
            )
            .as_str(),
            &download_path,
        );
    }

    #[inline(always)]
    fn enable_if(flag: bool, val: &'static str) -> &'static str {
        if flag {
            val
        } else {
            ""
        }
    }

    let install_prefix = match install_path.to_str() {
        Some(ips) if ips.len() > 0 => {
            format!("--prefix={}", ips)
        }
        _ => String::new(),
    };

    bash_in(
        &ffmpeg_path,
        &format!(
            // The reason for 4x$ in LDSOFLAGS var refer to https://stackoverflow.com/a/71429999
            // all varients of --extra-ldsoflags='-Wl,-rpath,$ORIGIN' do not work! don't waste your time trying!
            //
            r#"LDSOFLAGS=-Wl,-rpath,\''$$$$ORIGIN'\' ./configure {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}"#,
            install_prefix,
            "--disable-static",
            "--disable-programs",
            "--disable-doc",
            "--disable-avdevice --disable-avformat --disable-swresample --disable-postproc",
            "--disable-network",
            "--disable-debug --disable-everything",
            " --enable-shared --enable-gpl --enable-version3",
            "--enable-lto",
            /*
               Describing Nvidia specific options --nvccflags:
               nvcc from CUDA toolkit version 11.0 or higher does not support compiling for 'compute_30' (default in ffmpeg)
               52 is the minimum required for the current CUDA 11 version (Quadro M6000 , GeForce 900, GTX-970, GTX-980, GTX Titan X)
               https://arnon.dk/matching-sm-architectures-arch-and-gencode-for-various-nvidia-cards/
               Anyway below 50 arch card don't support nvenc encoding hevc https://developer.nvidia.com/nvidia-video-codec-sdk (Supported devices)
               Nvidia docs:
               https://docs.nvidia.com/video-technologies/video-codec-sdk/ffmpeg-with-nvidia-gpu/#commonly-faced-issues-and-tips-to-resolve-them
            */
            (if nvenc_flag {
                let cuda = pkg_config::Config::new().probe("cuda").unwrap();
                let include_flags = cuda
                    .include_paths
                    .iter()
                    .map(|path| format!("-I{path:?}"))
                    .reduce(|a, b| format!("{a}{b}"))
                    .expect("pkg-config cuda entry to have include-paths");
                let link_flags = cuda
                    .link_paths
                    .iter()
                    .map(|path| format!("-L{path:?}"))
                    .reduce(|a, b| format!("{a}{b}"))
                    .expect("pkg-config cuda entry to have link-paths");

                format!(
                    "{} {} {} {} {} --extra-cflags=\"{}\" --extra-ldflags=\"{}\" {} {}",
                    enable_if(enable_decoders, "--enable-decoder=h264_nvdec --enable-decoder=hevc_nvdec --enable-decoder=h264_cuvid --enable-decoder=hevc_cuvid"),
                    "--enable-encoder=h264_nvenc --enable-encoder=hevc_nvenc --enable-nonfree",
                    "--enable-ffnvcodec --enable-cuda-nvcc --enable-libnpp",
                    enable_if(enable_decoders, "--enable-nvdec --enable-nvenc --enable-cuvid"),
                    "--nvccflags=\"-gencode arch=compute_52,code=sm_52 -O2\"",
                    include_flags,
                    link_flags,
                    enable_if(enable_decoders, "--enable-hwaccel=h264_nvdec --enable-hwaccel=hevc_nvdec --enable-hwaccel=h264_cuvid --enable-hwaccel=hevc_cuvid"),
                    "--enable-hwaccel=h264_nvenc --enable-hwaccel=hevc_nvenc"
                )
            } else {
                "".to_string()
            }),
            "--enable-encoder=h264_vaapi --enable-encoder=hevc_vaapi",
            "--enable-encoder=libx264 --enable-encoder=libx264rgb --enable-encoder=libx265",
            "--enable-hwaccel=h264_vaapi --enable-hwaccel=hevc_vaapi",
            enable_if(enable_decoders, "--enable-decoder=libx264 --enable-decoder=libx265 --enable-decoder=h264_vaapi --enable-decoder=hevc_vaapi --enable-vaapi"),
            "--enable-filter=scale --enable-filter=scale_vaapi",
            "--enable-libx264 --enable-libx265 --enable-vulkan",
            "--enable-libdrm --enable-pic --enable-rpath"
        ),
    )
    .unwrap();
    bash_in(&ffmpeg_path, "make -j$(nproc)").unwrap();
    if install_prefix.len() > 0 {
        bash_in(&ffmpeg_path, "make install").unwrap();
    }

    ffmpeg_path
}

pub fn build_ffmpeg_linux(nvenc_flag: bool) -> std::path::PathBuf {
    build_ffmpeg_linux_install(
        nvenc_flag,
        "release/5.1",
        /*enable_decoders=*/ true,
        std::path::Path::new(""),
    )
}

pub fn extract_ffmpeg_windows() -> std::path::PathBuf {
    let download_path = afs::deps_dir().join("windows");
    let ffmpeg_path = download_path.join("ffmpeg-n8.0-latest-win64-gpl-shared-8.0");
    if !ffmpeg_path.exists() {
        download_and_extract_zip(
            //"https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n6.0-latest-win64-gpl-shared-6.0.zip",
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n8.0-latest-win64-gpl-shared-8.0.zip",
            &download_path,
        );
    }

    ffmpeg_path
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
