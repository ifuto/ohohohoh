//! # Migration Hub — All rendering → RsGraphics, All compute → RsCalc
//!
//! Mod がなくてもビルトインエンジンで動作。DLL があればそちらを優先。

use crate::adaptive_perf::{AdaptivePerfEngine, AdaptiveComputeProfile, AdaptiveRenderProfile};
use crate::hyper_opt::BumpArena;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{info, debug};

/// Graphics migration target (RsGraphics)
pub trait GraphicsMigrationTarget: Send + Sync {
    fn init(&mut self) -> Result<(), String>;
    fn render_frame(&mut self, width: u32, height: u32, delta: f32);
    fn is_active(&self) -> bool;
    fn source_label(&self) -> &'static str;
}

/// Compute migration target (RsCalc)
pub trait ComputeMigrationTarget: Send + Sync {
    fn init(&mut self) -> Result<(), String>;
    fn tick(&mut self, delta_ms: f32);
    fn on_packet(&self, packet_id: u32, ptr: i64, len: i32) -> bool;
    fn is_active(&self) -> bool;
    fn source_label(&self) -> &'static str;
}

/// Tracks CPU/memory budget usage
#[derive(Debug, Default)]
pub struct ResourceBudget {
    pub frames_rendered: AtomicU64,
    pub ticks_processed: AtomicU64,
    pub packets_handled: AtomicU64,
    pub arena_peak_bytes: AtomicU64,
}

impl ResourceBudget {
    pub fn record_frame(&self) {
        self.frames_rendered.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_tick(&self) {
        self.ticks_processed.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_packet(&self) {
        self.packets_handled.fetch_add(1, Ordering::Relaxed);
    }
}

/// Central router: 100% rendering via RsGraphics path, 100% compute via RsCalc path
pub struct MigrationRouter {
    pub graphics: Box<dyn GraphicsMigrationTarget>,
    pub compute: Box<dyn ComputeMigrationTarget>,
    pub render_profile: AdaptiveRenderProfile,
    pub compute_profile: AdaptiveComputeProfile,
    pub budget: ResourceBudget,
    pub mods_optional: bool,
}

impl MigrationRouter {
    pub fn new(
        graphics: Box<dyn GraphicsMigrationTarget>,
        compute: Box<dyn ComputeMigrationTarget>,
        mods_optional: bool,
    ) -> Self {
        let hw = AdaptivePerfEngine::hardware();
        Self {
            graphics,
            compute,
            render_profile: AdaptivePerfEngine::render_profile(hw),
            compute_profile: AdaptivePerfEngine::compute_profile(hw),
            budget: ResourceBudget::default(),
            mods_optional,
        }
    }

    pub fn initialize(&mut self) -> Result<(), String> {
        info!("========================================================================");
        info!(" [MigrationHub] Initializing unified engine migration");
        info!("   Graphics → {} (mods_optional={})", self.graphics.source_label(), self.mods_optional);
        info!("   Compute  → {} ", self.compute.source_label());
        info!("   Render: speed_first={} threads={} gpu_cull={} (vanilla dist untouched)",
            self.render_profile.speed_first,
            self.render_profile.chunk_builder_threads,
            self.render_profile.gpu_compute_culling);
        info!("   Compute: rayon={} aot={} tick_budget={}ms",
            self.compute_profile.rayon_threads,
            self.compute_profile.aot_transpile_enabled,
            self.compute_profile.tick_budget_ms);
        info!("========================================================================");

        self.graphics.init()?;
        self.compute.init()?;
        Ok(())
    }

    pub fn frame(&mut self, width: u32, height: u32, delta: f32, arena: &BumpArena) {
        self.graphics.render_frame(width, height, delta);
        self.compute.tick(delta * 1000.0);
        self.budget.record_frame();
        self.budget.record_tick();
        let remaining = arena.remaining();
        let used = arena.capacity() - remaining;
        let peak = self.budget.arena_peak_bytes.load(Ordering::Relaxed);
        if used as u64 > peak {
            self.budget.arena_peak_bytes.store(used as u64, Ordering::Relaxed);
        }
    }

    pub fn packet(&self, id: u32, ptr: i64, len: i32) -> bool {
        self.budget.record_packet();
        self.compute.on_packet(id, ptr, len)
    }

    pub fn log_status(&self) {
        debug!(
            "[MigrationHub] frames={} ticks={} packets={} arena_peak={}KB",
            self.budget.frames_rendered.load(Ordering::Relaxed),
            self.budget.ticks_processed.load(Ordering::Relaxed),
            self.budget.packets_handled.load(Ordering::Relaxed),
            self.budget.arena_peak_bytes.load(Ordering::Relaxed) / 1024,
        );
    }
}

/// No-op graphics fallback — explicitly inactive (never "migration complete").
pub struct NullGraphics;
impl GraphicsMigrationTarget for NullGraphics {
    fn init(&mut self) -> Result<(), String> {
        Err("NullGraphics: inactive fallback — not a migrated graphics engine".into())
    }
    fn render_frame(&mut self, _: u32, _: u32, _: f32) {}
    fn is_active(&self) -> bool { false }
    fn source_label(&self) -> &'static str { "null (inactive — vanilla passthrough)" }
}

/// No-op compute fallback — explicitly inactive.
pub struct NullCompute;
impl ComputeMigrationTarget for NullCompute {
    fn init(&mut self) -> Result<(), String> {
        Err("NullCompute: inactive fallback — not a migrated compute engine".into())
    }
    fn tick(&mut self, _: f32) {}
    fn on_packet(&self, _: u32, _: i64, _: i32) -> bool { true }
    fn is_active(&self) -> bool { false }
    fn source_label(&self) -> &'static str { "null (inactive — JVM passthrough)" }
}
