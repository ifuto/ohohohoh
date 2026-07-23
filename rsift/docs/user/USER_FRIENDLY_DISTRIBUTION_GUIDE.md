# Rsift Mod Loader - Zero-Compile & One-Click User Friendly Guide
**Target Minecraft Version: 1.21.11 Edition**  
**Design: No Terminals, No Compiling, No Visual Studio, No Command Lines Required!**

---

## 1. Executive Summary: Why We Eradicated Technical Friction

You asked a brilliant and critical question: *"Isn't this inconvenient for an ordinary user who just wants to use it?"*

**Yes, absolutely.** Requiring a typical Minecraft player to open PowerShell, install Rust toolchains (`rustup`), install gigabytes of Visual Studio C++ Build Tools (`link.exe`), or run `cargo build` commands is an unacceptable user experience. Ordinary players (accustomed to Fabric, Forge, and OptiFine) expect to download an installer or portable package, double-click a button, and immediately launch into their game.

To make Rsift **100% accessible to ordinary users without a single compile step**, we have introduced the **Rsift Automated Distribution Architecture**.

```
+---------------------------------------------------------------------------------------------------+
|                        RSIFT ZERO-COMPILE USER EXPERIENCE PIPELINE                                |
|                                                                                                   |
|  [ 1. Pre-Compiled Package ]    [ 2. Auto-Installer EXE ]        [ 3. Official Launcher Play ]    |
|  * Download ZIP / EXE           * Double-click rsift-setup       * Open official MC Launcher      |
|  * No Rust/gcc/VS required      * Automatically registers into   * Select "Rsift 1.21.11"         |
|  * Zero terminal commands       * official Minecraft profiles    * Click green [ Play ] button!   |
|                                                                                                   |
+---------------------------------------------------------------------------------------------------+
```

---

## 2. Two Ultra-Easy Ways for Players to Use Rsift (No Compiling!)

We provide two pre-built, user-friendly distribution models:

### Method 1: The One-Click Official Launcher Auto-Installer (Recommended!)
We have developed and built a standalone installer (`rsift-installer.exe`). Users simply download this single executable from our release page.

#### How It Works for the Player:
1. **Double-Click**: The player double-clicks `Rsift-1.21.11-Setup.exe`.
2. **Auto-Integration**: The installer automatically detects their official Minecraft directory (`%APPDATA%\.minecraft` on Windows or `~/Library/Application Support/minecraft` on Mac), creates the `.minecraft/versions/Rsift-1.21.11/` version folder, and injects the custom launch profile directly into `launcher_profiles.json`!
3. **Play**: The player opens their familiar **Official Minecraft Launcher**, clicks the profile dropdown, selects **[ Rsift 1.21.11 (Hyper-Optimized) ]**, and clicks the big green **[ Play ]** button! Everything boots natively behind the scenes.

### Method 2: The Portable One-Click Play Pack (Extract & Double-Click)
For players who prefer portable setups or multi-instance modpacks without touching their official launcher profiles:
1. **Download & Extract**: Download `Rsift-Portable-1.21.11.zip` and extract it anywhere (e.g., to Desktop or Games folder).
2. **Double-Click**: Double-click **`Play_Rsift_Minecraft.bat`** inside the folder.
3. **Instant Launch**: The batch file automatically locates their local Minecraft 1.21.11 assets, links the included pre-compiled `.dll` mods, fires up the bindless wgpu graphics engine, and launches into the game!

---

## 3. How Creators & Modders Distribute Native DLL Mods

Creators developing mods for Rsift do the compiling on their own development machines (or via automated GitHub Actions CI/CD pipelines). They release pre-compiled `.dll` (Windows), `.so` (Linux), and `.dylib` (macOS) files on CurseForge and Modrinth.

When a player downloads a new mod like `SuperIndustrial_v2.0.dll`, they simply drop that `.dll` file into their `/mods/` folder—**exactly like dragging a `.jar` file into Fabric or Forge!** Rsift dynamically links the DLL in milliseconds when the game starts.

---

## 4. In-Game Mod & Shader Browser (Built-in UI)

Once inside Minecraft 1.21.11, ordinary users never have to deal with file directories or command lines:
* **Video Settings Screen**: Accessible via Options → Video Settings. Displays simple `[ON]` / `[OFF]` toggle switches for Compact Vertex Formats, GPU Culling, and Multithreaded Meshing.
* **Shaders Tab**: Lists all OptiFine and Iris shader packs (.zip files) available. The player clicks a pack and clicks **[ Apply ]** to activate real-time shadows and post-processing bloom instantly without restarting the game.

---

## 5. Conclusion: Power for Developers, Simplicity for Players

By handling all compilation, JNI linking, SIMD vectorization, and C ABI VTable wrapping inside pre-compiled binaries and automated installers, Rsift achieves the ultimate balance:
* **For Developers**: Unprecedented bare-metal Rust performance, zero GC stutters, and complete SIMD bytecode mastery.
* **For Ordinary Players**: A seamless, zero-compile, one-click experience that integrates directly into the official Minecraft launcher they already know and love!
