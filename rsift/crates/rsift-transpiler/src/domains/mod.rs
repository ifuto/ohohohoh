//! Compute domains — each maps a Minecraft calculation category to native Rust.

pub mod entity_ai;
pub mod redstone;
pub mod physics;
pub mod chunk_tick;
pub mod block_entity;
pub mod fluids;
pub mod hopper;
pub mod collision;
pub mod pathfinding;
pub mod task_scheduler;
pub mod compact_entity;

pub use task_scheduler::*;
pub use compact_entity::*;

use rsift_api::AdaptiveComputeProfile;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::debug;

/// Per-domain tick statistics (lock-free counters)
#[derive(Debug, Default)]
pub struct DomainStats {
    pub entities_processed: AtomicU64,
    pub redstone_wires: AtomicU64,
    pub physics_bodies: AtomicU64,
    pub chunk_ticks: AtomicU64,
    pub block_entities: AtomicU64,
    pub fluid_cells: AtomicU64,
    pub hopper_transfers: AtomicU64,
    pub collisions_checked: AtomicU64,
    pub paths_computed: AtomicU64,
    pub aot_calls: AtomicU64,
    pub deferred_tasks: AtomicU64,
    pub total_ns: AtomicU64,
}

impl DomainStats {
    pub fn reset(&self) {
        self.entities_processed.store(0, Ordering::Relaxed);
        self.redstone_wires.store(0, Ordering::Relaxed);
        self.physics_bodies.store(0, Ordering::Relaxed);
        self.chunk_ticks.store(0, Ordering::Relaxed);
        self.block_entities.store(0, Ordering::Relaxed);
        self.fluid_cells.store(0, Ordering::Relaxed);
        self.hopper_transfers.store(0, Ordering::Relaxed);
        self.collisions_checked.store(0, Ordering::Relaxed);
        self.paths_computed.store(0, Ordering::Relaxed);
        self.aot_calls.store(0, Ordering::Relaxed);
        self.deferred_tasks.store(0, Ordering::Relaxed);
        self.total_ns.store(0, Ordering::Relaxed);
    }

    pub fn log_summary(&self) {
        debug!(
            "[RsCalc] tick stats: entities={} redstone={} physics={} chunks={} \
             block_ent={} fluids={} hoppers={} collisions={} paths={} aot={} deferred={} ({:.2}ms)",
            self.entities_processed.load(Ordering::Relaxed),
            self.redstone_wires.load(Ordering::Relaxed),
            self.physics_bodies.load(Ordering::Relaxed),
            self.chunk_ticks.load(Ordering::Relaxed),
            self.block_entities.load(Ordering::Relaxed),
            self.fluid_cells.load(Ordering::Relaxed),
            self.hopper_transfers.load(Ordering::Relaxed),
            self.collisions_checked.load(Ordering::Relaxed),
            self.paths_computed.load(Ordering::Relaxed),
            self.aot_calls.load(Ordering::Relaxed),
            self.deferred_tasks.load(Ordering::Relaxed),
            self.total_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0,
        );
    }
}

/// All compute domains bundled for one tick pass
pub struct ComputeDomains {
    pub entity_ai: entity_ai::EntityAiDomain,
    pub redstone: redstone::RedstoneDomain,
    pub physics: physics::PhysicsDomain,
    pub chunk_tick: chunk_tick::ChunkTickDomain,
    pub block_entity: block_entity::BlockEntityDomain,
    pub fluids: fluids::FluidDomain,
    pub hopper: hopper::HopperDomain,
    pub collision: collision::CollisionDomain,
    pub pathfinding: pathfinding::PathfindingDomain,
}

impl ComputeDomains {
    pub fn new(_profile: &AdaptiveComputeProfile) -> Self {
        // Never allocate synthetic workloads — wait for WorldMirror ingest.
        Self {
            entity_ai: entity_ai::EntityAiDomain::empty(),
            redstone: redstone::RedstoneDomain::empty(_profile.parallel_redstone),
            physics: physics::PhysicsDomain::empty(),
            chunk_tick: chunk_tick::ChunkTickDomain::empty(),
            block_entity: block_entity::BlockEntityDomain::empty(),
            fluids: fluids::FluidDomain::empty(_profile.parallel_redstone),
            hopper: hopper::HopperDomain::empty(),
            collision: collision::CollisionDomain::empty(),
            pathfinding: pathfinding::PathfindingDomain::empty(_profile.parallel_entity_ai),
        }
    }

    /// Pull live JVM mirror state into every domain (no synthetic work).
    pub fn ingest_mirror(&mut self, mirror: &crate::world_mirror::WorldMirror) {
        let player = mirror
            .entities
            .iter()
            .find(|e| e.entity_id == 0 || e.entity_type == 0)
            .map(|e| [e.pos_x as f32, e.pos_y as f32, e.pos_z as f32])
            .or_else(|| {
                mirror
                    .entities
                    .first()
                    .map(|e| [e.pos_x as f32, e.pos_y as f32, e.pos_z as f32])
            })
            .unwrap_or([0.0, 64.0, 0.0]);

        // Distance-banded AI (Lithium-style): near every tick, mid every 2, far every 4.
        let tick = mirror.header.tick_number;
        self.entity_ai.ingest_from_mirror(&mirror.entities, player, |dist| {
            if dist < 32.0 {
                true
            } else if dist < 64.0 {
                tick % 2 == 0
            } else if dist < 128.0 {
                tick % 4 == 0
            } else {
                false
            }
        });
        self.physics.ingest_from_mirror(&mirror.entities);
        self.chunk_tick.ingest_from_mirror(&mirror.chunks);
        self.redstone.ingest_from_mirror(&mirror.redstone);
        self.collision.ingest_from_entities(&mirror.entities);
        self.pathfinding.ingest_from_mirror(&mirror.entities, player);
        // Fluids / block entities / hoppers stay empty until dedicated mirror arrays land.
    }

    pub fn write_back_mirror(&self, mirror: &mut crate::world_mirror::WorldMirror) {
        self.entity_ai.write_back(&mut mirror.entities);
        self.physics.write_back(&mut mirror.entities);
        self.chunk_tick.write_back(&mut mirror.chunks);
        self.redstone.write_back(&mut mirror.redstone);
    }
}
