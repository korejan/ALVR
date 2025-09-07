@if not defined _echo echo off
setlocal enableDelayedExpansion

set toolpath="%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
for /f "usebackq delims=" %%i in (`%toolpath% -latest -property installationPath`) do (
    set VCVarsAllBat="%%i\VC\Auxiliary\Build\vcvarsall.bat"
)

if exist !VCVarsAllBat! (
    call !VCVarsAllBat! x64 uwp
    set BundleApp=alxr-client-uwp_%2_%1.msixbundle
    echo Building App-bundle: !BundleApp!
    makeappx bundle /v /o /bv %2 /f %3 /p !BundleApp!
    @REM if NOT([%4]==[]) (
        echo Self-Signing App-bundle: !BundleApp! with key file: %4
        signtool sign /v /fd SHA256 /a /f "%4" !BundleApp!
    @REM )
)
