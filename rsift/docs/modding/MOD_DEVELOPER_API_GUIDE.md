# Rsift Native Mod Developer & API Guide (Version 1.21.11 Edition)
**The Complete Guide to Developing Pure Rust Native Dynamic Library (`cdylib`) Mods**

---

## 1. Executive Summary: Why Native Rust Mods?

Welcome to the **Rsift Native Mod Ecosystem**! Unlike traditional Minecraft modding platforms (Fabric, Forge, NeoForge) where mods are compiled to Java bytecode (`.jar`) and run inside the JVM's garbage-collected heap, Rsift loads **Native Dynamic Libraries (`.dll` on Windows, `.so` on Linux, `.dylib` on macOS)** compiled from Rust (`cdylib`).

### Benefits of Developing for Rsift (`rsift-api`):
* ⚡ **Bare-Metal Execution**: Your mod logic runs as optimized machine code tailored to modern CPU registers and SIMD instructions—skipping JVM interpreter friction.
* 🧠 **Zero Garbage Collection Stutters**: Memory allocations, packet inspection, and AI loops run off-heap (`bytemuck::Pod` + `DirectBufferSlice`), preventing `Stop-the-World` GC pauses.
* 🛡️ **Cross-Ecosystem Parity**: Our unified `rsift-api` crate provides native Rust equivalents of familiar Fabric and NeoForge concepts (`Registry`, `EventBus`, `ScreenRegistry`, `Cloth Config`, `Mod Menu`).
* 🔌 **Zero-Copy Network & Rendering Hooks**: Intercept every incoming/outgoing packet or wgpu render pass directly from C-ABI function pointers without heap copying.

---

## 2. Project Setup (`Cargo.toml`)

Creating an Rsift mod is as simple as creating a standard Rust library with `crate-type = ["cdylib"]`.

### `Cargo.toml`
```toml
[package]
name = "my_custom_mod"
version = "0.1.0"
edition = "2021"

[lib]
name = "my_custom_mod"
crate-type = ["cdylib"] # Essential: compiles to my_custom_mod.dll / .so

[dependencies]
rsift-api = { version = "0.1.0", path = "../../crates/rsift-api" }
bytemuck = { version = "1.16", features = ["derive"] }
tracing = "0.1"
```

---

## 3. Core C-ABI Entry Points

When Rsift boots Minecraft 1.21.11 (`rsift.exe`), it scans the `./mods` directory using `libloading::Library` and resolves four standard C-ABI exported functions (`#[no_mangle] pub extern "C" fn ...`) implemented by your mod:

| C-ABI Exported Function | Return / Signature | Purpose & Execution Lifecycle |
| :--- | :--- | :--- |
| `rsift_mod_init` | `fn(ctx: &mut ModContext) -> i32` | **Mod Initialization Entry Point.** Called exactly once during startup. Use `ctx` to register items, blocks, GUI buttons, and configs. Return `RsiftStatus::Success as i32` (`0`). |
| `rsift_mod_on_packet` | `fn(id: u32, ptr: i64, len: i32) -> bool` | **Zero-Copy Network Hook.** Called on every incoming/outgoing packet before Minecraft processes it. Return `true` to pass to the game, `false` to drop/cancel. |
| `rsift_mod_on_render` | `fn(w: u32, h: u32, dt: f32)` | **High-Speed Render Hook.** Called at the end of every frame/pass (`wgpu` / `LWJGL`). Use for custom overlays, HUDs, or UI drawing. |
| `rsift_mod_on_key_event` | `fn(key: i32, x: f32, y: f32, z: f32, yaw: f32, pitch: f32)` | **First-Person Input Hook.** Called when keyboard events trigger in-game, providing the exact camera position and look vector. |

---

## 4. Complete Mod Example (`src/lib.rs`)

Here is a comprehensive example demonstrating block/item registration, custom Title Screen buttons, `Cloth Config` settings integration, Mod Menu catalog registration, and zero-copy packet interception.

### `src/lib.rs`
```rust
use rsift_api::{
    content::BlockDefinition,
    packet::DirectBufferSlice,
    ClothConfigBuilder, ModContext, ModManifest, RsiftStatus,
    TARGET_MINECRAFT_VERSION,
};
use tracing::{info, warn};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PlayerPositionPacket {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub on_ground: u8,
    pub _pad: [u8; 7],
}

/// 1. Mod Initialization Entry Point (`rsift_mod_init`)
#[no_mangle]
pub extern "C" fn rsift_mod_init(ctx: &mut ModContext) -> i32 {
    info!("===========================================================");
    info!(" 🚀 [MyMod] Initializing for Minecraft {}", ctx.minecraft_version);
    info!("===========================================================");

    // Register a Custom Block via the Unified Content Suite
    if let Ok(mut content) = ctx.suite().content.write() {
        let block_id = content.register_block(
            BlockDefinition::builder("mymod", "quantum_reactor")
                .strength(3.5, 12.0)
                .luminance(15),
        );
        info!("[MyMod] Registered custom block: mymod:quantum_reactor (ID {})", block_id);
    }

    // Register a Custom Item via ModContext Registry
    let item_id = ctx.registry.register_item("mymod", "plasma_blade", 1);
    info!("[MyMod] Registered custom item: mymod:plasma_blade (ID {})", item_id);

    // Register a Custom Title Screen Button via ScreenRegistry
    ctx.screen_registry().add_button(
        "net.minecraft.client.gui.screens.TitleScreen",
        "MyMod Settings",
        8, 80, 120, 20,
        Some("Open MyMod configuration panel"),
        |_id| {
            info!("[MyMod] Title Screen settings button clicked!");
            rsift_api::platform::request_open_screen("cloth_config");
        },
    );

    // Build a Cloth Config GUI & Register into Mod Menu Catalog
    let mut cloth = ClothConfigBuilder::new();
    cloth.set_title("Quantum Reactor Configuration");
    let cat = cloth.add_category("General");
    cloth.add_bool_toggle(cat, "Enable Plasma Overdrive", true, Some("Increases blade damage"));
    cloth.add_int_slider(cat, "Reactor Output (%)", 0, 100, 85, None);

    ctx.mod_menu().register_mod(
        ModManifest {
            id: "my_custom_mod".into(),
            name: "Quantum Reactor Mod".into(),
            version: "0.1.0".into(),
            author: "Rsift Developer".into(),
            description: "Exercises native block, item, GUI, and config registration".into(),
            target_rsift_version: env!("CARGO_PKG_VERSION").into(),
        },
        None,
        Some("https://github.com/ifuto/rsift"),
        Some(cloth),
    );

    RsiftStatus::Success as i32 // Return 0 for success
}

/// 2. Zero-Copy Packet Interception (`rsift_mod_on_packet`)
#[no_mangle]
pub extern "C" fn rsift_mod_on_packet(packet_id: u32, buf_ptr: i64, buf_len: i32) -> bool {
    // Intercept Player Position Packet (ID 0x1A in our parity mapping)
    if packet_id == 0x1A {
        // Zero-copy cast directly from OS memory — NO copying across JVM boundary!
        if let Ok(slice) = unsafe { DirectBufferSlice::from_raw_jni(buf_ptr, buf_len) } {
            if let Ok(pos) = slice.as_pod::<PlayerPositionPacket>() {
                if pos.y < -64.0 {
                    warn!("[MyMod] Void fall protection triggered at Y={:.1} — cancelling packet!", pos.y);
                    return false; // Cancel packet to prevent void death
                }
            }
        }
    }
    true // Pass packet to game
}

/// 3. High-Speed Render Hook (`rsift_mod_on_render`)
#[no_mangle]
pub extern "C" fn rsift_mod_on_render(width: u32, height: u32, _delta_time: f32) {
    // Custom wgpu / 2D UI overlays run directly inside the render pass
    let _center_x = width / 2;
    let _center_y = height / 2;
}
```

---

## 5. API Reference Summary

### `ModContext<'a>`
The central bridge passed into `rsift_mod_init(ctx: &mut ModContext)`:
* `ctx.registry`: `ModRegistry` for registering items, blocks, and entity definitions.
* `ctx.screen_registry()`: `ScreenRegistry` for attaching interactive buttons (`add_button`) to specific Minecraft screen classes (e.g., `TitleScreen`, `PauseScreen`, `InventoryScreen`).
* `ctx.suite()`: Access to the unified `ModSuite` containing the `ContentRegistry` (`write().register_block(...)`).
* `ctx.mod_menu()`: `ModMenuCatalog` for registering your mod's `ModManifest` and pre-built `ClothConfigBuilder` UI.
* `ctx.minecraft_version`: Returns `"1.21.11"`.

### `DirectBufferSlice` (`rsift_api::packet::DirectBufferSlice`)
Provides safe, zero-copy pointer views over Netty direct memory buffers passed across the JNI bridge (`buf_ptr: i64, buf_len: i32`):
* `unsafe { DirectBufferSlice::from_raw_jni(ptr, len) }`: Creates a safe slice reference over the direct memory payload.
* `slice.as_pod::<T>()`: Safely casts the memory directly into a `#[derive(Pod)]` struct `T` in $O(1)$ CPU cycles (`0 bytes copied`).

### `ScreenRegistry` (`rsift_api::registry::ScreenRegistry`)
Allows non-invasive GUI injection:
* `add_button(target_screen_class, label, x, y, width, height, tooltip, callback)`: Automatically injects a styled button into the target Minecraft UI screen when opened.

---

## 6. How to Build & Test Your Mod

### Step 1: Compile in Release Mode
Run Cargo to build your dynamic library (`cdylib`):
```bash
cargo build --release
# On Windows, this produces: target/release/my_custom_mod.dll
# On Linux, this produces:   target/release/libmy_custom_mod.so
# On macOS, this produces:   target/release/libmy_custom_mod.dylib
```

### Step 2: Deploy into `/mods`
Copy the compiled dynamic library (`my_custom_mod.dll` or `.so`) directly into your Minecraft installation's `/mods` directory alongside official plugins like `rsgraphics.dll` and `rscalc.dll`.

### Step 3: Launch Rsift
Double-click `Rsift-1.21.11-Setup.exe` (or launch via your Minecraft Launcher profile). Rsift will automatically discover your dynamic library, invoke `rsift_mod_init`, and connect your zero-copy hooks instantly!
