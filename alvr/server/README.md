# alvr_server

SteamVR driver written in C++ and wrapped into a Rust cdylib. The wrapping is done to use the same cross-platform build system as the other ALVR components, and to allow easily extending the C++ code with Rust.

## Layout

- `src/` — the Rust crate; exposes the C++ driver through bindgen-generated FFI (`cpp/alvr_server/bindings.h`).
- `cpp/` — a self-contained C++20 CMake project (`alvr_server_cpp` static library):
  - `cpp/alvr_server/` — platform-independent driver core (OpenVR device implementations, client connection, settings).
  - `cpp/platform/{linux,win32,macos}/` — platform encoders (VAAPI/NVENC/software via FFmpeg on Linux; NVENC/AMF/software D3D11 on Windows).
  - `cpp/ALVR-common/`, `cpp/shared/` — shared utilities. The [nanors](https://github.com/korejan/nanors) Reed-Solomon library is fetched at configure time via CMake FetchContent.
  - `cpp/openvr/` — vendored prebuilt OpenVR SDK, wrapped as an imported CMake target.

`build.rs` drives the CMake build via the [cmake](https://docs.rs/cmake) crate (using the Ninja generator when available), links the resulting static libraries plus system dependencies, and generates the Rust bindings.

Environment variables honored by `build.rs`:

- `ALVR_CMAKE_GEN` — force a specific CMake generator (falls back to `CMAKE_GENERATOR`, then Ninja if available).
- `ALVR_FFMPEG_DIR` — root of the prebuilt shared FFmpeg to build against; exported automatically by `cargo xtask` for bundled/GPL builds. When unset, `deps/` is scanned for a previously extracted FFmpeg (the build fails rather than guessing if several match).

The CMake project can also be configured standalone for C++ development, which produces `compile_commands.json` for clangd/IDE use:

```sh
cmake -S cpp -B build -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build build
```

Notable CMake options (normally set by `build.rs` from cargo features):

- `ALVR_BUNDLED_FFMPEG` — dlopen a bundled FFmpeg at runtime instead of linking the system one (Linux; cargo feature `bundled_ffmpeg`).
- `ALVR_GPL` — enable GPL-licensed FFmpeg software encoding on Windows (cargo feature `gpl`).
- `ALVR_FFMPEG_DIR` — root of a prebuilt shared FFmpeg, used with either option above.
