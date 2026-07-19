@echo off
setlocal enabledelayedexpansion

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
