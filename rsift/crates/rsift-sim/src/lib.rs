//! # rsift-sim
//!
//! Production world-simulation optimizations (Phases 1–3):
//! profiling, bounded jobs, region I/O, Starlight lighting, entity activation,
//! Folia-style regions, palette memory, NBT, network, pathfinding, and more.
//!
//! **Anti-patterns explicitly avoided:** unbounded queues/threads, Rayon-on-Tokio,
//! far-chunk fullgen storms, per-tiny-packet compression, redstone incompat hacks.

pub mod profiling;
pub mod job_system;
pub mod region_io;
pub mod autosave;
pub mod lighting;
pub mod entity;
pub mod collision;
pub mod block_tick;
pub mod memory;
pub mod hashmap_util;
pub mod network;
pub mod nbt;
pub mod worldgen;
pub mod advanced;
pub mod region_tick;
pub mod cpu_features;
pub mod sound_ui;
pub mod mod_cache;
pub mod alloc_policy;
pub mod platform_io;
pub mod zstd_dict;
pub mod simd_kernels;

pub use profiling::{ProfileDomain, ProfileSnapshot, ScopeGuard};
pub use job_system::{FrameBudget, Job, JobKind, JobSystem};
pub use region_io::{CompressionKind, RegionFile, RegionIndexCache};
pub use autosave::{AutosaveController, DirtyChunk};
pub use lighting::StarlightEngine;
pub use entity::{EntityActivationSystem, EntityActivity};
pub use collision::CollisionWorld;
pub use block_tick::{TickWheel, Xoroshiro64, RngLite};
pub use memory::{PalettedContainer, StringInterner, GenArena};
pub use network::{PacketBufferPool, QuicChannelPlanner, read_varint, write_varint};
pub use nbt::{RegistryU32, NbtCursor};
pub use worldgen::{NoiseTileCache, StructureCache, value_noise_2d};
pub use advanced::{FlowField, HpaStar, WorldSnapshotPublisher};
pub use region_tick::RegionTickScheduler;
pub use cpu_features::CpuFeatures;
pub use alloc_policy::{AllocatorPolicy, GlobalAllocHint};
pub use platform_io::{io_backend_name, open_random_access};
pub use zstd_dict::ZstdDictionary;

use std::sync::{Mutex, OnceLock};

static GLOBAL_SIM: OnceLock<Mutex<Option<SimRuntime>>> = OnceLock::new();

/// Aggregate façade wired for a game session.
pub struct SimRuntime {
    pub jobs: JobSystem,
    pub lights: StarlightEngine,
    pub entities: EntityActivationSystem,
    pub collision: CollisionWorld,
    pub ticks: TickWheel,
    pub autosave: AutosaveController,
    pub regions: RegionTickScheduler,
    pub alloc: AllocatorPolicy,
    pub cpu: CpuFeatures,
    pub quic: QuicChannelPlanner,
}

impl SimRuntime {
    pub fn new(world_dir: impl Into<std::path::PathBuf>) -> Self {
        let cpu = CpuFeatures::detect();
        Self {
            jobs: JobSystem::auto(),
            lights: StarlightEngine::new(),
            entities: EntityActivationSystem::new(),
            collision: CollisionWorld::new(),
            ticks: TickWheel::new(),
            autosave: AutosaveController::new(world_dir),
            regions: RegionTickScheduler::new(256),
            alloc: AllocatorPolicy::for_memory_mb(8192),
            cpu,
            quic: QuicChannelPlanner::new(32),
        }
    }

    pub fn begin_frame(&mut self) {
        crate::profiling::frame_mark();
        self.alloc.reset_frame_arenas();
        let _ = self.jobs.pump_cpu_frame();
        let _ = self.autosave.pulse();
    }
}

/// Install process-global sim (agent / dedicated server).
pub fn init_global_sim(world_dir: impl Into<std::path::PathBuf>) {
    let slot = GLOBAL_SIM.get_or_init(|| Mutex::new(None));
    let mut g = slot.lock().unwrap();
    if g.is_none() {
        *g = Some(SimRuntime::new(world_dir));
    }
}

pub fn with_global_sim<R>(f: impl FnOnce(&mut SimRuntime) -> R) -> Option<R> {
    let slot = GLOBAL_SIM.get()?;
    let mut g = slot.lock().ok()?;
    g.as_mut().map(f)
}

pub fn tick_global_sim() {
    let _ = with_global_sim(|rt| rt.begin_frame());
}

#[cfg(test)]
mod smoke {
    use super::*;

    #[test]
    fn runtime_boots() {
        let dir = std::env::temp_dir().join("rsift_sim_smoke");
        let mut rt = SimRuntime::new(&dir);
        rt.begin_frame();
        assert!(rt.cpu.cores >= 1);
        assert!(!io_backend_name().is_empty());
    }
}
