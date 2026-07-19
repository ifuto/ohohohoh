//! Phase 2 — worldgen caches: noise tile, structure, negative cache, FastNoise-lite + SIMD-ish.

use crate::block_tick::{RngLite, Xoroshiro64};
use rustc_hash::FxHashMap;
use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub x: i32,
    pub z: i32,
    pub octave: u8,
}

#[derive(Debug)]
pub struct NoiseTileCache {
    map: FxHashMap<TileKey, Vec<f32>>,
    cap: usize,
}

impl NoiseTileCache {
    pub fn new(cap: usize) -> Self {
        Self {
            map: FxHashMap::default(),
            cap: cap.max(16),
        }
    }

    pub fn get_or_insert_with(&mut self, key: TileKey, f: impl FnOnce() -> Vec<f32>) -> &Vec<f32> {
        if self.map.len() >= self.cap && !self.map.contains_key(&key) {
            self.map.clear(); // bounded: drop all on overflow (simple, deterministic)
        }
        self.map.entry(key).or_insert_with(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StructureKey {
    pub kind: u32,
    pub cell_x: i32,
    pub cell_z: i32,
}

#[derive(Debug, Default)]
pub struct StructureCache {
    /// Some(true) present, Some(false) negative, None unknown
    map: FxHashMap<StructureKey, bool>,
    cap: usize,
}

impl StructureCache {
    pub fn with_cap(cap: usize) -> Self {
        Self {
            map: FxHashMap::default(),
            cap: cap.max(64),
        }
    }

    pub fn get(&self, k: StructureKey) -> Option<bool> {
        self.map.get(&k).copied()
    }

    pub fn insert_positive(&mut self, k: StructureKey) {
        self.insert(k, true);
    }

    pub fn insert_negative(&mut self, k: StructureKey) {
        self.insert(k, false);
    }

    fn insert(&mut self, k: StructureKey, v: bool) {
        if self.map.len() >= self.cap && !self.map.contains_key(&k) {
            self.map.clear();
        }
        self.map.insert(k, v);
    }
}

/// FastNoise-lite style value noise (deterministic, portable).
pub fn value_noise_2d(x: f32, z: f32, seed: u64) -> f32 {
    let xi = x.floor() as i32;
    let zi = z.floor() as i32;
    let xf = x - xi as f32;
    let zf = z - zi as f32;
    let v00 = hash_grad(xi, zi, seed);
    let v10 = hash_grad(xi + 1, zi, seed);
    let v01 = hash_grad(xi, zi + 1, seed);
    let v11 = hash_grad(xi + 1, zi + 1, seed);
    let u = smooth(xf);
    let v = smooth(zf);
    lerp(lerp(v00, v10, u), lerp(v01, v11, u), v)
}

#[inline]
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn hash_grad(x: i32, z: i32, seed: u64) -> f32 {
    let mut n = seed
        .wrapping_add(x as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(z as u64)
        .wrapping_mul(0xBF58_476D_1CE4_E5B9);
    n ^= n >> 33;
    n = n.wrapping_mul(0xff51_afd7_ed55_8ccd);
    n ^= n >> 33;
    ((n >> 40) as f32) / 255.0 * 2.0 - 1.0
}

/// Process 8 samples with independent hashes (portable "SIMD" batch).
pub fn value_noise_2d_batch8(xs: [f32; 8], zs: [f32; 8], seed: u64) -> [f32; 8] {
    let mut out = [0f32; 8];
    for i in 0..8 {
        out[i] = value_noise_2d(xs[i], zs[i], seed);
    }
    out
}

/// Worldgen result cache — same inputs → same outputs (does not change world).
#[derive(Debug, Default)]
pub struct WorldgenResultCache {
    heights: FxHashMap<(i32, i32), i32>,
    cap: usize,
}

impl WorldgenResultCache {
    pub fn new(cap: usize) -> Self {
        Self {
            heights: FxHashMap::default(),
            cap: cap.max(256),
        }
    }

    pub fn height(&mut self, cx: i32, cz: i32, seed: u64) -> i32 {
        if let Some(&h) = self.heights.get(&(cx, cz)) {
            return h;
        }
        if self.heights.len() >= self.cap {
            self.heights.clear();
        }
        let n = value_noise_2d(cx as f32 * 0.05, cz as f32 * 0.05, seed);
        let h = (64.0 + n * 24.0) as i32;
        self.heights.insert((cx, cz), h);
        h
    }
}

pub fn structure_seed(world_seed: u64, kind: u32, cell_x: i32, cell_z: i32) -> NonZeroU64 {
    let mut rng = Xoroshiro64::new(
        world_seed
            ^ (kind as u64).wrapping_mul(0x9E37_79B9)
            ^ (cell_x as u64).wrapping_shl(32)
            ^ (cell_z as u64),
    );
    NonZeroU64::new(rng.next_u32() as u64 | 1).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_noise_and_negative() {
        let a = value_noise_2d(1.5, 2.5, 42);
        let b = value_noise_2d(1.5, 2.5, 42);
        assert_eq!(a, b);
        let mut sc = StructureCache::with_cap(32);
        let k = StructureKey {
            kind: 1,
            cell_x: 0,
            cell_z: 0,
        };
        sc.insert_negative(k);
        assert_eq!(sc.get(k), Some(false));
    }
}
