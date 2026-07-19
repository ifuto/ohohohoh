//! Phase 14 — allocator policy: subsystem arenas + cache caps + hot/cold split.
//! mimalloc/jemalloc are linked at binary level via `[profile]` / global allocator in apps;
//! this module provides the arena policy used by subsystems.

use crate::memory::BumpArena;
use parking_lot::Mutex;

pub struct AllocatorPolicy {
    pub tick_arena: Mutex<BumpArena>,
    pub net_arena: Mutex<BumpArena>,
    pub mesh_arena: Mutex<BumpArena>,
    pub cache_caps: CacheCaps,
}

#[derive(Debug, Clone, Copy)]
pub struct CacheCaps {
    pub noise_tiles: usize,
    pub structures: usize,
    pub paths: usize,
    pub packets: usize,
    pub mod_json: usize,
}

impl Default for CacheCaps {
    fn default() -> Self {
        Self {
            noise_tiles: 512,
            structures: 2048,
            paths: 4096,
            packets: 256,
            mod_json: 128,
        }
    }
}

impl AllocatorPolicy {
    pub fn for_memory_mb(system_mb: usize) -> Self {
        let scale = (system_mb / 2048).clamp(1, 8);
        Self {
            tick_arena: Mutex::new(BumpArena::with_capacity(2 * 1024 * 1024 * scale)),
            net_arena: Mutex::new(BumpArena::with_capacity(1024 * 1024 * scale)),
            mesh_arena: Mutex::new(BumpArena::with_capacity(8 * 1024 * 1024 * scale)),
            cache_caps: CacheCaps {
                noise_tiles: 512 * scale,
                structures: 2048 * scale,
                paths: 4096 * scale,
                packets: 256 * scale,
                mod_json: 128 * scale,
            },
        }
    }

    pub fn reset_frame_arenas(&self) {
        self.tick_arena.lock().reset();
        self.net_arena.lock().reset();
    }
}

/// Documented choice: prefer system allocator in library crates;
/// apps may set `mimalloc` / `tikv-jemallocator` as global based on OS benchmarks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalAllocHint {
    System,
    Mimalloc,
    Jemalloc,
}

pub fn recommend_allocator(os: &str) -> GlobalAllocHint {
    match os {
        "windows" | "macos" => GlobalAllocHint::Mimalloc,
        "linux" => GlobalAllocHint::Jemalloc,
        _ => GlobalAllocHint::System,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arenas_reset() {
        let p = AllocatorPolicy::for_memory_mb(8192);
        {
            let mut a = p.tick_arena.lock();
            assert!(a.alloc_bytes(64, 8).is_some());
            assert!(a.used() >= 64);
            a.reset();
            assert_eq!(a.used(), 0);
        }
        assert_eq!(recommend_allocator("linux"), GlobalAllocHint::Jemalloc);
    }
}
