@if not defined _echo echo off
setlocal enableDelayedExpansion

set VSInstallPath="C:\Program Files\Microsoft Visual Studio\2022\Community"
set toolpath="%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
for /f "usebackq delims=" %%i in (`%toolpath% -latest -property installationPath`) do (
    set VSInstallPath="%%i"
    echo VS Install Path Found: !VSInstallPath!

)

echo Starting to install Visual Studio Components

set VSInstaller="%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\setup.exe"
echo | !VSInstaller! modify --installPath !VSInstallPath! --add Microsoft.VisualStudio.ComponentGroup.UWP.VC --add Microsoft.VisualStudio.Component.UWP.VC.ARM64 --downloadThenInstall --quiet --norestart --force

echo Finished installing Visual Studio Components