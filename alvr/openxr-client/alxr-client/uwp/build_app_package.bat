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
)

if exist !VCVarsAllBat! (
    call !VCVarsAllBat! !arch! uwp
    makeappx pack /o /p alxr-client-uwp_%2_%1.msix /v /f %3
    if not [%4]==[] (
        echo Self-Signing App-package: alxr-client-uwp_%2_%1.msix with key file: %4
        signtool sign /v /fd SHA256 /a /f "%~4" alxr-client-uwp_%2_%1.msix
    )
)
