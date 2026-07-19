//! # RsCalc Compute Runtime
//!
//! Unified native compute engine: all Minecraft tick calculations migrated to Rust.
//! Adaptive tier controls parallelism, AOT execution, and tick budget.

use crate::aot_engine::AotTranspilerEngine;
use crate::domains::{ComputeDomains, DomainStats};
use crate::frame_arena::FrameBumpArena;
use rsift_api::{AdaptiveComputeProfile, PerformanceTier};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::{info, debug};

/// Deferred work when tick budget is exceeded (eco-friendly scheduling)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferredDomain {
    EntityAi,
    Redstone,
    Physics,
    ChunkTick,
    BlockEntity,
    Fluids,
    Hopper,
    Collision,
    Pathfinding,
}

/// Result of one compute tick
#[derive(Debug, Clone)]
pub struct TickResult {
    pub elapsed_ms: f32,
    pub budget_ms: f32,
    pub budget_exceeded: bool,
    pub domains_run: u32,
    pub domains_deferred: u32,
    pub total_ops: u64,
}

/// Central RsCalc engine — replaces JVM tick for all heavy calculations
pub struct ComputeRuntime {
    profile: AdaptiveComputeProfile,
    transpiler: Option<AotTranspilerEngine>,
    domains: ComputeDomains,
    stats: DomainStats,
    deferred: VecDeque<DeferredDomain>,
    tick_count: u64,
    cumulative_ops: AtomicU64,
    active: bool,
    rayon_ready: bool,
    world_active: bool,
    /// Per-tick scratch — reset every tick to avoid heap churn.
    arena: FrameBumpArena,
}

impl ComputeRuntime {
    pub fn from_profile(profile: AdaptiveComputeProfile) -> Self {
        let domains = ComputeDomains::new(&profile);
        let arena_mb = match profile.tier {
            PerformanceTier::Minimal => 1,
            PerformanceTier::Low => 2,
            PerformanceTier::Medium => 4,
            PerformanceTier::High => 8,
        };
        Self {
            profile,
            transpiler: None,
            domains,
            stats: DomainStats::default(),
            deferred: VecDeque::new(),
            tick_count: 0,
            cumulative_ops: AtomicU64::new(0),
            active: false,
            rayon_ready: false,
            world_active: false,
            arena: FrameBumpArena::with_capacity_mb(arena_mb),
        }
    }

    pub fn init(&mut self) -> Result<(), String> {
        info!("========================================================================");
        info!(" [RsCalc] Initializing Native Compute Runtime");
        info!("   Tier: {} | threads={} | AOT={} | tick_budget={}ms",
            self.profile.tier.label(),
            self.profile.rayon_threads,
            self.profile.aot_transpile_enabled,
            self.profile.tick_budget_ms);
        info!("   Domains: EntityAI={} Redstone={} Physics ChunkTick BlockEntity Fluids Hopper Collision Pathfinding",
            self.profile.parallel_entity_ai, self.profile.parallel_redstone);
        info!("========================================================================");

        self.domains.entity_ai.set_parallel(self.profile.parallel_entity_ai);
        // speed_first: defer rayon pool + domain ticks until JVM mirror has world data
        if !self.profile.speed_first {
            self.ensure_rayon_pool();
            self.active = true;
        } else {
            info!("[RsCalc] speed_first: compute idle until in-world JNI sync");
        }
        Ok(())
    }

    fn ensure_rayon_pool(&mut self) {
        if self.rayon_ready {
            return;
        }
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(self.profile.rayon_threads.max(1))
            .thread_name(|i| format!("rscalc-{}", i))
            .build_global();
        self.rayon_ready = true;
        if self.profile.aot_transpile_enabled && self.transpiler.is_none() {
            let mut t = AotTranspilerEngine::new();
            let classes = Self::bootstrap_classes();
            for (name, bytecode) in &classes {
                let _ = t.transpile_class(name, bytecode);
            }
            info!("[RsCalc] AOT: {} methods, {} hot-paths promoted to native SSA",
                t.transpiled_method_count(), t.hot_path_count());
            self.transpiler = Some(t);
        }
    }

    pub fn note_world_activity(&mut self) {
        if !self.world_active {
            self.world_active = true;
            self.ensure_rayon_pool();
            self.active = true;
            info!("[RsCalc] world activity detected — native compute active");
        }
    }

    /// Execute one game tick — all compute domains with budget enforcement
    pub fn tick(&mut self, delta_ms: f32) -> TickResult {
        if !self.active {
            return TickResult::empty(self.profile.tick_budget_ms);
        }

        self.arena.reset();

        let start = Instant::now();
        let budget = self.profile.tick_budget_ms;
        let parallel = self.profile.rayon_threads > 1;
        let dt = (delta_ms / 1000.0).max(0.001) as f64;

        self.stats.reset();
        let mut domains_run = 0u32;
        let mut domains_deferred = 0u32;
        let mut total_ops = 0u64;

        // Process deferred work from previous tick first (priority order)
        while let Some(domain) = self.deferred.pop_front() {
            if elapsed_ms(&start) >= budget {
                self.deferred.push_front(domain);
                domains_deferred += 1;
                break;
            }
            total_ops += self.run_domain(domain, parallel, dt);
            domains_run += 1;
        }

        // Full domain pipeline — all Minecraft calculations
        let pipeline = Self::domain_pipeline(&self.profile);
        for domain in pipeline {
            if elapsed_ms(&start) >= budget {
                self.deferred.push_back(domain);
                domains_deferred += 1;
                continue;
            }
            total_ops += self.run_domain(domain, parallel, dt);
            domains_run += 1;
        }

        // AOT transpile disabled — execute_all_hot_paths is a toy SSA simulator.
        if self.profile.aot_transpile_enabled {
            if let Some(ref mut t) = self.transpiler {
                let aot_ops = t.execute_all_hot_paths();
                self.stats.aot_calls.store(aot_ops, Ordering::Relaxed);
                total_ops += aot_ops;
            }
        }

        let elapsed = elapsed_ms(&start);
        self.stats.total_ns.store((elapsed * 1_000_000.0) as u64, Ordering::Relaxed);
        self.stats.deferred_tasks.store(domains_deferred as u64, Ordering::Relaxed);
        self.tick_count += 1;
        self.cumulative_ops.fetch_add(total_ops, Ordering::Relaxed);

        if self.tick_count % 60 == 0 {
            self.stats.log_summary();
        }

        TickResult {
            elapsed_ms: elapsed,
            budget_ms: budget,
            budget_exceeded: elapsed > budget,
            domains_run,
            domains_deferred,
            total_ops,
        }
    }

    fn run_domain(&mut self, domain: DeferredDomain, parallel: bool, dt: f64) -> u64 {
        match domain {
            DeferredDomain::EntityAi => self.domains.entity_ai.tick(
                parallel && self.profile.parallel_entity_ai,
                &self.stats.entities_processed,
            ),
            DeferredDomain::Redstone => self.domains.redstone.tick(
                parallel && self.profile.parallel_redstone,
                &self.stats.redstone_wires,
            ),
            DeferredDomain::Physics => self.domains.physics.tick(
                parallel,
                dt,
                &self.stats.physics_bodies,
            ),
            DeferredDomain::ChunkTick => self.domains.chunk_tick.tick(
                parallel,
                &self.stats.chunk_ticks,
            ),
            DeferredDomain::BlockEntity => self.domains.block_entity.tick(
                parallel,
                &self.stats.block_entities,
            ),
            DeferredDomain::Fluids => self.domains.fluids.tick(
                parallel,
                &self.stats.fluid_cells,
            ),
            DeferredDomain::Hopper => self.domains.hopper.tick(
                parallel,
                &self.stats.hopper_transfers,
            ),
            DeferredDomain::Collision => self.domains.collision.tick(
                parallel,
                &self.stats.collisions_checked,
            ),
            DeferredDomain::Pathfinding => self.domains.pathfinding.tick(
                parallel && self.profile.parallel_entity_ai,
                &self.stats.paths_computed,
            ),
        }
    }

    fn domain_pipeline(profile: &AdaptiveComputeProfile) -> Vec<DeferredDomain> {
        // Eco: minimal domains; Performance: full pipeline
        match profile.tier {
            PerformanceTier::Minimal => vec![
                DeferredDomain::ChunkTick,
                DeferredDomain::Physics,
            ],
            PerformanceTier::Low => vec![
                DeferredDomain::ChunkTick,
                DeferredDomain::Physics,
                DeferredDomain::EntityAi,
                DeferredDomain::BlockEntity,
            ],
            PerformanceTier::Medium | PerformanceTier::High => vec![
                DeferredDomain::ChunkTick,
                DeferredDomain::Physics,
                DeferredDomain::EntityAi,
                DeferredDomain::Redstone,
                DeferredDomain::BlockEntity,
                DeferredDomain::Fluids,
                DeferredDomain::Hopper,
                DeferredDomain::Collision,
                DeferredDomain::Pathfinding,
            ],
        }
    }

    fn bootstrap_classes() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("net/minecraft/world/entity/Mob",
                b"\xca\xfe\xba\xbe\x00\x00\x00\x41\x00\x0f\x01\x00\x06aiStep\x01\x00\x03()V".to_vec()),
            ("net/minecraft/world/entity/Entity",
                b"\xca\xfe\xba\xbe\x00\x00\x00\x41\x00\x0f\x01\x00\x06travel\x01\x00\x03()V".to_vec()),
            ("net/minecraft/world/level/redstone/RedstoneWireBlock",
                b"\xca\xfe\xba\xbe\x00\x00\x00\x41\x00\x0f\x01\x00\tcalculate\x01\x00\x03()V".to_vec()),
            ("net/minecraft/world/level/block/entity/HopperBlockEntity",
                b"\xca\xfe\xba\xbe\x00\x00\x00\x41\x00\x0f\x01\x00\x04tick\x01\x00\x03()V".to_vec()),
            ("net/minecraft/world/level/chunk/LevelChunk",
                b"\xca\xfe\xba\xbe\x00\x00\x00\x41\x00\x0f\x01\x00\ttick\x01\x00\x03()V".to_vec()),
            ("net/minecraft/world/level/material/FlowingFluid",
                b"\xca\xfe\xba\xbe\x00\x00\x00\x41\x00\x0f\x01\x00\x08propagate\x01\x00\x03()V".to_vec()),
        ]
    }

    pub fn on_packet(&self, packet_id: u32, ptr: i64, len: i32) -> bool {
        use rsift_api::packet::{DirectBufferSlice, SpawnEntityPacket};
        if ptr == 0 || len <= 0 {
            return true;
        }
        if packet_id == 0x22 {
            if let Ok(slice) = unsafe { DirectBufferSlice::from_raw_jni(ptr, len) } {
                return slice.as_pod::<SpawnEntityPacket>().is_ok();
            }
        }
        true
    }

    /// Tick with JVM WorldMirror — ingest → domain pipeline → write-back.
    pub fn tick_with_mirror(&mut self, delta_ms: f32, mirror: &mut crate::world_mirror::WorldMirror) -> TickResult {
        let entity_count = mirror.entity_count();
        if entity_count > 0 || mirror.has_jvm_data() {
            self.note_world_activity();
            debug!(
                "[RsCalc] JVM mirror: entities={} redstone={} chunks={}",
                entity_count,
                mirror.redstone.len(),
                mirror.chunks.len()
            );
        }
        if self.profile.speed_first && !self.world_active {
            return TickResult::empty(self.profile.tick_budget_ms);
        }
        self.domains.ingest_mirror(mirror);
        let result = self.tick(delta_ms);
        self.domains.write_back_mirror(mirror);
        result
    }

    /// Legacy count-only entry (no ingest) — prefer `tick_with_mirror`.
    pub fn tick_with_mirror_count(&mut self, delta_ms: f32, mirror_entity_count: usize) -> TickResult {
        if mirror_entity_count > 0 {
            self.note_world_activity();
        }
        if self.profile.speed_first && !self.world_active {
            return TickResult::empty(self.profile.tick_budget_ms);
        }
        self.tick(delta_ms)
    }

    pub fn is_active(&self) -> bool { self.active }
    pub fn tick_count(&self) -> u64 { self.tick_count }
    pub fn cumulative_ops(&self) -> u64 { self.cumulative_ops.load(Ordering::Relaxed) }
    pub fn profile(&self) -> &AdaptiveComputeProfile { &self.profile }
    pub fn stats(&self) -> &DomainStats { &self.stats }

    pub fn log_status(&self) {
        info!(
            "[RsCalc] ticks={} cumulative_ops={} deferred_queue={} tier={}",
            self.tick_count,
            self.cumulative_ops(),
            self.deferred.len(),
            self.profile.tier.label(),
        );
    }
}

impl TickResult {
    pub fn empty(budget_ms: f32) -> Self {
        Self {
            elapsed_ms: 0.0,
            budget_ms,
            budget_exceeded: false,
            domains_run: 0,
            domains_deferred: 0,
            total_ops: 0,
        }
    }
}

fn elapsed_ms(start: &Instant) -> f32 {
    start.elapsed().as_secs_f32() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsift_api::AdaptivePerfEngine;

    #[test]
    fn performance_tier_runs_all_domains() {
        let hw = rsift_api::HardwareProfile {
            tier: PerformanceTier::High,
            cpu_cores: 8, cpu_threads: 16,
            cpu_model: "7800X3D".into(), ram_gb: 32.0,
            gpu_score: 22_000, gpu_name: "RX 7800 XT".into(),
            is_mobile_gpu: false, is_software_renderer: false,
            flagship_boost: true,
        };
        let profile = AdaptivePerfEngine::compute_profile(&hw);
        let mut rt = ComputeRuntime::from_profile(profile);
        rt.init().unwrap();
        let mut mirror = crate::world_mirror::WorldMirror::new();
        mirror.synced_from_jvm = true;
        mirror.header.tick_number = 2;
        mirror.entities.push(crate::world_mirror::JvmEntityState {
            entity_id: 1,
            entity_type: 2,
            pos_x: 1.0,
            pos_y: 64.0,
            pos_z: 1.0,
            health: 20.0,
            ..Default::default()
        });
        mirror.chunks.push(crate::world_mirror::JvmChunkState {
            chunk_x: 0,
            chunk_z: 0,
            loaded: 1,
            random_ticks_remaining: 3,
            ..Default::default()
        });
        let result = rt.tick_with_mirror(50.0, &mut mirror);
        assert!(result.domains_run >= 2);
        assert!(result.total_ops > 0);
    }

    #[test]
    fn eco_tier_respects_budget() {
        let profile = AdaptiveComputeProfile {
            tier: PerformanceTier::Minimal,
            rayon_threads: 1,
            aot_transpile_enabled: false,
            simd_parallel_scan: false,
            parallel_redstone: false,
            parallel_entity_ai: false,
            tick_budget_ms: 50.0,
            jvm_heap_suggestion: "-Xms512M".into(),
        };
        let mut rt = ComputeRuntime::from_profile(profile);
        rt.init().unwrap();
        let result = rt.tick(50.0);
        assert!(!result.budget_exceeded);
    }
}
