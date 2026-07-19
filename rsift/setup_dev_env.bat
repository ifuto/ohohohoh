@echo off
setlocal enabledelayedexpansion
title Rsift Dev Environment Setup and Automated Build Tool

echo ===============================================================================
echo            Rsift Project - Version 1.21.11 Edition                            
echo         One-Click Developer Environment Setup and Workspace Builder            
echo ===============================================================================

REM --- Step 1: Check and Setup Rust Toolchain ---
set "CARGO_EXE="
where cargo >nul 2>nul
if %ERRORLEVEL% equ 0 (
    set "CARGO_EXE=cargo"
) else if exist "%USERPROFILE%\.cargo\bin\cargo.exe" (
    set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
    set "CARGO_EXE=%USERPROFILE%\.cargo\bin\cargo.exe"
    echo [INFO] Found Rust in user profile cargo bin directory!
)

if not defined CARGO_EXE (
    echo [WARN] Rust toolchain was not found on your system!
    echo [INFO] Installing official Rust toolchain automatically...
    echo -------------------------------------------------------------------------------
    powershell -NoProfile -ExecutionPolicy Bypass -Command "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile '%TEMP%\rustup-init.exe'; & '%TEMP%\rustup-init.exe' -y --no-modify-path"
    if exist "%USERPROFILE%\.cargo\bin\cargo.exe" (
        set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
        set "CARGO_EXE=%USERPROFILE%\.cargo\bin\cargo.exe"
        echo [INFO] Rust toolchain installed successfully!
    ) else (
        echo [ERROR] Automatic Rust installation failed. Please check internet connection.
        pause
        exit /b 1
    )
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

REM --- Step 2: Smart Workspace Compilation ---
echo [INFO] Attempting release workspace build with MSVC toolchain (avoids dlltool)...
rustup default stable-x86_64-pc-windows-msvc >nul 2>nul
echo -------------------------------------------------------------------------------
cargo build --release --workspace

if %ERRORLEVEL% neq 0 (
    echo [WARN] MSVC build encountered an issue (or toolchain missing). Trying GNU fallback...
    rustup default stable-x86_64-pc-windows-gnu >nul 2>nul
    cargo build --release --workspace
    
    if !ERRORLEVEL! neq 0 (
        rustup default stable-x86_64-pc-windows-gnu >nul 2>nul
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
        ) do (
            if exist %%P set "PATH=%%~P;!PATH!"
        )
        for /r "%USERPROFILE%\.rustup" %%F in (dlltool.exe) do (
            if exist "%%F" set "PATH=%%~dpF;!PATH!"
        )
        cargo build --release --workspace
        
        if !ERRORLEVEL! neq 0 (
            echo [ERROR] All build attempts failed. Please check logs above.
            pause
            exit /b 1
        )
    )
)

echo -------------------------------------------------------------------------------
echo [INFO] Build completed successfully!

REM --- Step 3: Ensure Mods Directory Exists in ALL Official Minecraft Locations! ---
set "MC_DIR=%APPDATA%\.minecraft"
if not exist "%MC_DIR%" set "MC_DIR=C:\Users\%USERNAME%\AppData\Roaming\.minecraft"

if not exist "mods" mkdir mods
if not exist "%MC_DIR%\mods" mkdir "%MC_DIR%\mods" 2>nul

echo [INFO] Deploying compiled DLL Mods to local and official Minecraft mods folders...
for %%M in (rscalc.dll rsgraphics.dll rsreplay.dll smpsystem.dll) do (
    if exist "target\release\%%M" (
        copy /Y "target\release\%%M" "mods\" >nul
        copy /Y "target\release\%%M" "%MC_DIR%\mods\" >nul 2>nul
        echo [INFO] Copied DLL: %%M
    ) else if exist "target\x86_64-pc-windows-gnu\release\%%M" (
        copy /Y "target\x86_64-pc-windows-gnu\release\%%M" "mods\" >nul
        copy /Y "target\x86_64-pc-windows-gnu\release\%%M" "%MC_DIR%\mods\" >nul 2>nul
        echo [INFO] Copied GNU DLL: %%M
    )
)

if not exist "windows_binaries" mkdir "windows_binaries"
if exist "target\release\rsift-installer.exe" (
    copy /Y "target\release\rsift-installer.exe" "windows_binaries\Rsift-1.21.11-Setup.exe" >nul
) else if exist "target\x86_64-pc-windows-gnu\release\rsift-installer.exe" (
    copy /Y "target\x86_64-pc-windows-gnu\release\rsift-installer.exe" "windows_binaries\Rsift-1.21.11-Setup.exe" >nul
)

REM --- Step 4: Run Auto-Installer (creates rsift-loader-1.21.11_v1.0.0 folder) ---
echo [INFO] Running Rsift Auto-Installer...
if exist "target\release\rsift-installer.exe" (
    target\release\rsift-installer.exe
) else if exist "target\x86_64-pc-windows-gnu\release\rsift-installer.exe" (
    target\x86_64-pc-windows-gnu\release\rsift-installer.exe
)

echo ===============================================================================
echo  ALL DONE! Your developer environment, DLLs, and launcher integration are ready!
echo  You can now either run launch_minecraft_1.21.11.bat or open the Official Launcher!
echo ===============================================================================
pause
