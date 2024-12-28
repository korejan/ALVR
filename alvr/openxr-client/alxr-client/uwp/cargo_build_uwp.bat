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
    call !VCVarsAllBat! !arch! uwp 10.0.20348.0
    @REM Must use Visual Studio's fork of cmake for building UWP apps.
    if exist !CMakePath! (
        set PATH=!CMakePath!;!PATH!
    )
    @REM Print which version of cmake, should be the one that comes with visual studio/c++
    cmake --version
    @REM Print which version of cl.exe is being used
    cl
    if !arch! == x64 (
        @REM This is a workaround to bug since rustc 1.65.0-nightly
        @REM refer to https://github.com/rust-lang/rust/issues/100400#issuecomment-1212109010
        @REM set UwpRuntimePath=C:\Program Files\WindowsApps\Microsoft.VCLibs.140.00_14.0.30704.0_x64__8wekyb3d8bbwe
        set UwpRuntimePath=!cd!\uwp-runtime
        set PATH=!UwpRuntimePath!;!PATH!
        set LINK=OneCore.lib WindowsApp.lib
    )
    @REM cargo +nightly build -Z build-std=std,panic_abort --target !cargoArch!-uwp-windows-msvc %~2
    @REM ^ the above was the old way to build with nightly toolchain before rustup v1.25.
    rustup run nightly cargo build -Z build-std=std,panic_abort --target !cargoArch!-uwp-windows-msvc %~2
)
