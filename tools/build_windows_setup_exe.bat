@echo off
setlocal enabledelayedexpansion

if exist "%~dp0..\Cargo.toml" cd /d "%~dp0.."
if exist "%~dp0..\rsift\Cargo.toml" cd /d "%~dp0..\rsift"
if exist "rsift\Cargo.toml" cd /d "rsift"

echo ==============================================================================
echo  📦 [Rsift Build Engine] Compiling Self-Extracting One-Click Windows Installer
echo  ⚡ Output: windows_binaries\Rsift-1.21.11-Setup.exe
echo ==============================================================================

where cargo >nul 2>nul
if %ERRORLEVEL% neq 0 (
    echo [ERROR] Rust toolchain (`cargo`) not detected in PATH.
    echo Please install Rust from https://rustup.rs and run this script again.
    exit /b 1
)

REM Automatically discover and add dlltool.exe and gcc to PATH across all common Windows toolchain directories!
for /d %%D in ("%USERPROFILE%\.rustup\toolchains\*gnu*") do (
    if exist "%%D\lib\rustlib\x86_64-pc-windows-gnu\bin" set "PATH=%%D\lib\rustlib\x86_64-pc-windows-gnu\bin;!PATH!"
    if exist "%%D\libexec\gcc\x86_64-w64-mingw32\bin" set "PATH=%%D\libexec\gcc\x86_64-w64-mingw32\bin;!PATH!"
)
for %%P in (
    "C:\mingw64\bin"
    "C:\msys64\mingw64\bin"
    "C:\msys64\usr\bin"
    "C:\tools\msys64\mingw64\bin"
    "%USERPROFILE%\scoop\apps\msys2\current\mingw64\bin"
    "%USERPROFILE%\AppData\Local\Programs\msys2\mingw64\bin"
    "C:\Program Files\mingw-w64\x86_64-8.1.0-posix-seh-rt_v6-rev0\mingw64\bin"
) do (
    if exist %%P set "PATH=%%~P;!PATH!"
)
for /r "%USERPROFILE%\.rustup" %%F in (dlltool.exe) do (
    if exist "%%F" set "PATH=%%~dpF;!PATH!"
)

echo [INFO] Selecting MSVC target toolchain on Windows if available (avoids dlltool)...
rustup default stable-x86_64-pc-windows-msvc >nul 2>nul

echo [1/4] Compiling official native DLL plugins (Release mode, LTO Fat, Opt-Level 3)...
cargo build --release -p rsgraphics -p rscalc -p rsreplay -p smpsystem -p sample-mod
if %ERRORLEVEL% neq 0 (
    echo [ERROR] Failed to compile official DLL plugins!
    exit /b 1
)

echo [2/4] Compiling master host executable (rsift.exe)...
cargo build --release -p rsift-launcher
if %ERRORLEVEL% neq 0 (
    echo [ERROR] Failed to compile master host launcher!
    exit /b 1
)

echo [3/4] Compiling self-extracting one-click setup installer (Rsift-1.21.11-Setup.exe)...
cargo build --release -p rsift-installer -p rsift-gui-installer
if %ERRORLEVEL% neq 0 (
    echo [ERROR] Failed to compile installer!
    exit /b 1
)

echo [4/4] Staging verified Windows binaries into `windows_binaries\`...
if not exist "windows_binaries" mkdir "windows_binaries"

copy /y "target\release\rsift.exe" "windows_binaries\rsift.exe" >nul
copy /y "target\release\rsift_installer.exe" "windows_binaries\Rsift-1.21.11-Setup.exe" >nul
copy /y "target\release\rsift_gui_installer.exe" "windows_binaries\Rsift-GUI-Installer.exe" >nul
copy /y "target\release\rsgraphics.dll" "windows_binaries\rsgraphics.dll" >nul
copy /y "target\release\rscalc.dll" "windows_binaries\rscalc.dll" >nul
copy /y "target\release\rsreplay.dll" "windows_binaries\rsreplay.dll" >nul
copy /y "target\release\smpsystem.dll" "windows_binaries\smpsystem.dll" >nul

echo ==============================================================================
echo  ✅ BUILD COMPLETE! All verified Windows binaries generated successfully:
echo     -> windows_binaries\Rsift-1.21.11-Setup.exe (Single-File Auto-Installer)
echo     -> windows_binaries\rsgraphics.dll (Official Graphics & Culling V2 Engine)
echo     -> windows_binaries\rscalc.dll (Official Physics & AI AOT Transpiler)
echo     -> windows_binaries\rsreplay.dll (Official Studio & MP4 Exporter)
echo     -> windows_binaries\smpsystem.dll (Official SMP Heavy Core Rare Loot Modifier)
echo ==============================================================================
endlocal
