# Rsift Windows Binaries (`windows_binaries/`) & One-Click Setup Guide

---

## ❓ Why aren't `Rsift-1.21.11-Setup.exe` or `.dll` files directly in Git source tree right now?

If you browsed the repository source code and asked: *"Where is the `.exe`?" (`exeなくない？`)*

**Answer:** In professional software engineering, compiled executable artifacts (`.exe`, `.dll`) are excluded from Git source tracking (`.gitignore`) because they are binary outputs compiled directly from our pure Rust source code (`crates/rsift-installer`, `crates/rsift-gui-installer`, `mods-official/*`).

---

## 📦 How Ordinary Users Get `Rsift-1.21.11-Setup.exe` (No Compiling Required!)

For ordinary Minecraft players who just want to play immediately:
1. Go to the **GitHub Releases Page (`https://github.com/ifuto/rsift/releases/latest`)**.
2. Under **Assets**, click to download:
   - 📦 **`Rsift-1.21.11-Setup.exe`** (Single-File Self-Extracting Automated Windows Installer)
3. Double-click `Rsift-1.21.11-Setup.exe`. Within 1 second, it deploys all required plugins (`rsgraphics.dll`, `rscalc.dll`, `rsreplay.dll`, `smpsystem.dll`), creates the `.minecraft/versions/Rsift-1.21.11/` directory, and registers **[ Rsift 1.21.11 (Hyper-Optimized) ]** right into your official Minecraft Launcher!

---

## 🛠️ How Developers / Power Users Generate `Rsift-1.21.11-Setup.exe` Locally

If you have cloned this repository onto a Windows PC (`C:\rsift\`) and want to generate the exact release binaries locally right now:

### Option A: Run our Automated Build Script (1-Click Build)
Double-click or run from Command Prompt / PowerShell:
```cmd
tools\build_windows_setup_exe.bat
```
This automatically executes Cargo release builds with fat LTO + maximum optimizations and deposits `Rsift-1.21.11-Setup.exe`, `rsift.exe`, `rsgraphics.dll`, `rscalc.dll`, `rsreplay.dll`, and `smpsystem.dll` directly into this `windows_binaries\` directory!

### Option B: Run Cargo Command Directly
```cmd
cargo build --release --workspace
```
The compiled installer will be available at `target\release\rsift_installer.exe` (you can rename it to `Rsift-1.21.11-Setup.exe`).
