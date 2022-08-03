@if not defined _echo echo off
setlocal enableDelayedExpansion

set arch=x64
set cargoArch=x86_64
if %1% == arm64 (
    set arch=amd64_arm64
    set cargoArch=aarch64
)
@REM echo Target-arch: !arch!

set toolpath="%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
for /f "usebackq delims=" %%i in (`%toolpath% -latest -property installationPath`) do (
    set VCVarsAllBat="%%i\VC\Auxiliary\Build\vcvarsall.bat"
    set CMakePath="%%i\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin"
)

if exist !VCVarsAllBat! (
    call !VCVarsAllBat! !arch! uwp 10.0.20348.0 -vcvars_ver=14.32.31326
    @REM Must use Visual Studio's fork of cmake for building UWP apps.
    if exist !CMakePath! (
        set PATH=!CMakePath!;!PATH!
    )
    cmake --version
    @REM cargo +nightly build -Z build-std=std,panic_abort --target !cargoArch!-uwp-windows-msvc %~2
    @REM ^ the above was the old way to build with nightly toolchain before rustup v1.25.
    rustup run nightly cargo build -Z build-std=std,panic_abort --target !cargoArch!-uwp-windows-msvc %~2
)
