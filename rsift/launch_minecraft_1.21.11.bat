@echo off
setlocal enabledelayedexpansion
title Rsift — Official Minecraft Launcher Install
echo ===============================================================================
echo   Rsift Loader — register with the OFFICIAL Minecraft Launcher
echo   Minecraft 1.21.11  ^|  agentpath native injection
echo ===============================================================================
echo.
echo  Close the Minecraft Launcher before continuing (it overwrites profiles on exit).
echo.

set "MC_DIR=%APPDATA%\.minecraft"
if not exist "%MC_DIR%" (
    echo [ERROR] %MC_DIR% not found.
    echo         Install / run vanilla Minecraft 1.21.11 once from the official launcher.
    pause
    exit /b 1
)

if not exist "%MC_DIR%\versions\1.21.11\1.21.11.jar" (
    echo [ERROR] Vanilla 1.21.11 client jar missing.
    echo         In the official launcher: Installations -^> New -^> version 1.21.11 -^> Play once.
    pause
    exit /b 1
)

REM Prefer release binaries; fall back to gnu target
set "INSTALLER="
if exist "target\release\rsift-installer.exe" set "INSTALLER=target\release\rsift-installer.exe"
if exist "target\release\rsift_gui_installer.exe" set "INSTALLER=target\release\rsift_gui_installer.exe"
if exist "target\x86_64-pc-windows-gnu\release\rsift-installer.exe" set "INSTALLER=target\x86_64-pc-windows-gnu\release\rsift-installer.exe"

REM Stage mod DLLs into .\mods for the installer to pick up
if not exist "mods" mkdir mods
if exist "target\release\rscalc.dll" copy /Y "target\release\rscalc.dll" "mods\" >nul
if exist "target\release\rsgraphics.dll" copy /Y "target\release\rsgraphics.dll" "mods\" >nul
if exist "target\release\rsreplay.dll" copy /Y "target\release\rsreplay.dll" "mods\" >nul
if exist "target\release\rsift_jvm.dll" copy /Y "target\release\rsift_jvm.dll" "mods\" >nul
if exist "target\release\rsift_jvm.dll" copy /Y "target\release\rsift_jvm.dll" ".\" >nul

if not defined INSTALLER (
    echo [INFO] Installer exe not found — trying cargo run...
    cargo run -p rsift-installer --release -- "%CD%"
    if errorlevel 1 (
        echo [ERROR] Install failed. Build with: cargo build -p rsift-installer --release
        pause
        exit /b 1
    )
) else (
    echo [INFO] Running "%INSTALLER%" ...
    "%INSTALLER%" "%CD%"
    if errorlevel 1 (
        echo [ERROR] Installer reported failure.
        pause
        exit /b 1
    )
)

echo.
echo -------------------------------------------------------------------------------
echo  Install complete. Starting the official Minecraft Launcher...
echo  Select the "Rsift Loader" installation and press Play.
echo -------------------------------------------------------------------------------

set "LAUNCHER="
if exist "%ProgramFiles(x86)%\Minecraft Launcher\MinecraftLauncher.exe" set "LAUNCHER=%ProgramFiles(x86)%\Minecraft Launcher\MinecraftLauncher.exe"
if exist "%LOCALAPPDATA%\Programs\Minecraft Launcher\MinecraftLauncher.exe" set "LAUNCHER=%LOCALAPPDATA%\Programs\Minecraft Launcher\MinecraftLauncher.exe"
if exist "%ProgramFiles%\WindowsApps\Microsoft.Minecraft*\Minecraft.exe" (
    REM Microsoft Store edition — start via shell protocol
    start "" "minecraft:"
    goto :done
)

if defined LAUNCHER (
    start "" "%LAUNCHER%"
) else (
    echo [WARN] Could not locate MinecraftLauncher.exe — open it manually.
    echo        Profile is already in: %MC_DIR%\launcher_profiles.json
)

:done
echo.
pause
