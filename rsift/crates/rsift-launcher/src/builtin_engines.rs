//! Built-in RsGraphics and RsCalc engines — work without DLL mods

use crate::mod_loader::NativeModLoader;
use rsift_api::migration_hub::{ComputeMigrationTarget, GraphicsMigrationTarget};
use rsift_opt_gfx::{EcoRegionRenderer, MultithreadedChunkBuilder, IrisShaderEngine};
use rsift_transpiler::{ensure_runtime_initialized, rsift_native_on_packet, rsift_native_server_tick};
use std::sync::Arc;
use tracing::info;

/// Built-in RsGraphics (embedded in launcher when rsgraphics.dll absent)
pub struct BuiltinRsGraphics {
    mesher: Option<MultithreadedChunkBuilder>,
    eco: Option<EcoRegionRenderer>,
    iris: Option<IrisShaderEngine>,
    active: bool,
}

impl Default for BuiltinRsGraphics {
    fn default() -> Self { Self::new() }
}

impl BuiltinRsGraphics {
    pub fn new() -> Self {
        Self { mesher: None, eco: None, iris: None, active: false }
    }
}

impl GraphicsMigrationTarget for BuiltinRsGraphics {
    fn init(&mut self) -> Result<(), String> {
        info!("[BuiltinRsGraphics] Activating embedded graphics engine (no DLL required)");
        let hw = rsift_api::AdaptivePerfEngine::hardware();
        let rp = rsift_api::AdaptivePerfEngine::render_profile(hw);
        self.mesher = Some(MultithreadedChunkBuilder::adaptive());
        self.eco = Some(EcoRegionRenderer::new());
        let mut iris = IrisShaderEngine::with_tier("./shaderpacks", hw.tier);
        if rp.iris_shaders_enabled {
            let _ = iris.load_shaderpack("ComplementaryReimagined_v5.1.zip");
        } else {
            info!(
                "[BuiltinRsGraphics] Iris quality={:?} (shaders off for this tier)",
                iris.quality
            );
        }
        self.iris = Some(iris);
        self.active = true;
        Ok(())
    }

    fn render_frame(&mut self, width: u32, height: u32, delta: f32) {
        // Drive the live ChunkBridge-backed global pipeline (not a (0,0)-only stub).
        rsift_opt_gfx::on_render_frame(width.max(1), height.max(1), delta);
        if let Ok(pipe) = rsift_opt_gfx::global_pipeline().lock() {
            let coords: Vec<(i32, i32)> = {
                let (cx, cz) = pipe.world.camera_chunk();
                let r = if pipe.world.has_live_data { 1 } else { 2 };
                ((cz - r)..=(cz + r))
                    .flat_map(|z| ((cx - r)..=(cx + r)).map(move |x| (x, z)))
                    .collect()
            };
            drop(pipe);
            if let Ok(mut pipe) = rsift_opt_gfx::global_pipeline().lock() {
                let _ = pipe.frame(&coords, width.max(1), height.max(1), delta);
            }
        }
        if let Some(ref mut eco) = self.eco {
            if let Ok(pipe) = rsift_opt_gfx::global_pipeline().lock() {
                let (cx, cz) = pipe.world.camera_chunk();
                eco.build_regions(&[(cx, cz)], 400);
            }
        }
        let _ = self.iris.as_ref();
    }

    fn is_active(&self) -> bool { self.active }
    fn source_label(&self) -> &'static str { "RsGraphics (builtin)" }
}

/// Built-in RsCalc — shared JNI bridge runtime (single instance, parity strict)
pub struct BuiltinRsCalc {
    active: bool,
}

impl Default for BuiltinRsCalc {
    fn default() -> Self { Self::new() }
}

impl BuiltinRsCalc {
    pub fn new() -> Self {
        Self { active: false }
    }
}

impl ComputeMigrationTarget for BuiltinRsCalc {
    fn init(&mut self) -> Result<(), String> {
        info!("[BuiltinRsCalc] Activating shared native compute bridge (parity strict)");
        ensure_runtime_initialized()?;
        self.active = true;
        Ok(())
    }

    fn tick(&mut self, delta_ms: f32) {
        let code = rsift_native_server_tick(delta_ms);
        if code != 0 {
            tracing::debug!("[BuiltinRsCalc] partial JVM fallback this tick (spec preserved)");
        }
    }

    fn on_packet(&self, id: u32, ptr: i64, len: i32) -> bool {
        rsift_native_on_packet(id, ptr, len) == 0
    }

    fn is_active(&self) -> bool { self.active }
    fn source_label(&self) -> &'static str { "RsCalc (builtin)" }
}

/// DLL-backed graphics delegate (forwards to mod_loader render dispatch)
pub struct DllGraphicsDelegate {
    pub loader: Arc<NativeModLoader>,
    pub active: bool,
}

impl DllGraphicsDelegate {
    pub fn new(loader: Arc<NativeModLoader>) -> Self {
        Self { loader, active: false }
    }
}

impl GraphicsMigrationTarget for DllGraphicsDelegate {
    fn init(&mut self) -> Result<(), String> {
        info!("[DllRsGraphics] Using rsgraphics.dll — render dispatch wired");
        self.active = true;
        Ok(())
    }
    fn render_frame(&mut self, w: u32, h: u32, d: f32) {
        self.loader.dispatch_render(w, h, d);
    }
    fn is_active(&self) -> bool { self.active }
    fn source_label(&self) -> &'static str { "RsGraphics (DLL)" }
}

/// DLL-backed compute delegate — single runtime in rscalc.dll only (no duplicate init)
pub struct DllComputeDelegate {
    pub loader: Arc<NativeModLoader>,
    pub active: bool,
}

impl DllComputeDelegate {
    pub fn new(loader: Arc<NativeModLoader>) -> Self {
        Self { loader, active: false }
    }
}

impl ComputeMigrationTarget for DllComputeDelegate {
    fn init(&mut self) -> Result<(), String> {
        info!("[DllRsCalc] Using rscalc.dll — compute owned by DLL (no duplicate runtime)");
        self.active = true;
        Ok(())
    }
    fn tick(&mut self, delta_ms: f32) {
        // Drive server-tick lifecycle (registered by rscalc / compute mods).
        // Never route compute through the render dispatcher.
        rsift_api::mod_suite::mod_suite()
            .events
            .dispatch_server_tick(delta_ms as u64);
    }
    fn on_packet(&self, id: u32, ptr: i64, len: i32) -> bool {
        self.loader.dispatch_packet(id, ptr, len)
    }
    fn is_active(&self) -> bool { self.active }
    fn source_label(&self) -> &'static str { "RsCalc (DLL)" }
}
