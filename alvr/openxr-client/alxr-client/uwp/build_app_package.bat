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
    call !VCVarsAllBat! !arch! uwp 10.0.20348.0 -vcvars_ver=14.36.32532
    makeappx pack /o /p alxr-client-uwp_%2_%1.msix /v /f %3
)
