//! Phase 2 — Tick Wheel, random-tick list, block-update dedupe, fluid local, redstone dirty graph.

use roaring::RoaringBitmap;
use rustc_hash::FxHashMap;
use std::collections::VecDeque;

const WHEEL_SIZE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub fn pack(self) -> u64 {
        let x = (self.x as u64) & 0x1f_ffff;
        let y = (self.y as u64) & 0xfff;
        let z = (self.z as u64) & 0x1f_ffff;
        x | (y << 21) | (z << 33)
    }
}

#[derive(Debug, Clone)]
pub struct ScheduledTick {
    pub pos: BlockPos,
    pub priority: i8,
}

/// Hierarchical timing wheel for scheduled block ticks.
pub struct TickWheel {
    slots: [VecDeque<ScheduledTick>; WHEEL_SIZE],
    cursor: usize,
    tick: u64,
}

impl Default for TickWheel {
    fn default() -> Self {
        Self::new()
    }
}

impl TickWheel {
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| VecDeque::new()),
            cursor: 0,
            tick: 0,
        }
    }

    pub fn schedule(&mut self, tick: ScheduledTick, delay_ticks: usize) {
        let slot = (self.cursor + delay_ticks.max(1)) % WHEEL_SIZE;
        self.slots[slot].push_back(tick);
    }

    pub fn advance(&mut self) -> Vec<ScheduledTick> {
        // Advance cursor first so `schedule(..., delay)` fires after exactly `delay` advances.
        self.cursor = (self.cursor + 1) % WHEEL_SIZE;
        self.tick += 1;
        self.slots[self.cursor].drain(..).collect()
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }
}

/// Only positions that can receive random ticks (grass, farmland, …).
#[derive(Debug, Default)]
pub struct RandomTickIndex {
    pub positions: RoaringBitmap, // packed lower 32 bits of BlockPos.pack for local windows
    pub full: Vec<BlockPos>,
}

impl RandomTickIndex {
    pub fn insert(&mut self, pos: BlockPos) {
        self.positions.insert((pos.pack() & 0xffff_ffff) as u32);
        self.full.push(pos);
    }

    pub fn sample(&self, count: usize, rng: &mut impl RngLite) -> Vec<BlockPos> {
        if self.full.is_empty() || count == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(count.min(self.full.len()));
        for _ in 0..count {
            let i = rng.next_u32() as usize % self.full.len();
            out.push(self.full[i]);
        }
        out
    }
}

pub trait RngLite {
    fn next_u32(&mut self) -> u32;
}

/// Deterministic Xoroshiro128++ lite.
#[derive(Debug, Clone)]
pub struct Xoroshiro64 {
    s0: u64,
    s1: u64,
}

impl Xoroshiro64 {
    pub fn new(seed: u64) -> Self {
        let mut s = Self {
            s0: seed ^ 0x9E37_79B9_7F4A_7C15,
            s1: seed.wrapping_mul(0xBF58_476D_1CE4_E5B9),
        };
        if s.s0 == 0 && s.s1 == 0 {
            s.s0 = 1;
        }
        s
    }
}

impl RngLite for Xoroshiro64 {
    fn next_u32(&mut self) -> u32 {
        let s0 = self.s0;
        let mut s1 = self.s1;
        let result = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        s1 ^= s0;
        self.s0 = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.s1 = s1.rotate_left(28);
        (result >> 32) as u32
    }
}

#[derive(Debug, Default)]
pub struct BlockUpdateDeduper {
    pending: RoaringBitmap,
    positions: FxHashMap<u32, BlockPos>,
}

impl BlockUpdateDeduper {
    pub fn push(&mut self, pos: BlockPos) {
        let k = (pos.pack() & 0xffff_ffff) as u32;
        if self.pending.insert(k) {
            self.positions.insert(k, pos);
        }
    }

    pub fn drain(&mut self) -> Vec<BlockPos> {
        let keys: Vec<u32> = self.pending.iter().collect();
        self.pending.clear();
        keys.into_iter()
            .filter_map(|k| self.positions.remove(&k))
            .collect()
    }
}

/// Localized fluid updates — only dirty fluid cells.
#[derive(Debug, Default)]
pub struct FluidLocalUpdater {
    dirty: RoaringBitmap,
    cells: FxHashMap<u32, BlockPos>,
}

impl FluidLocalUpdater {
    pub fn mark(&mut self, pos: BlockPos) {
        let k = (pos.pack() & 0xffff_ffff) as u32;
        if self.dirty.insert(k) {
            self.cells.insert(k, pos);
        }
    }

    pub fn step_local(&mut self, max: usize) -> Vec<BlockPos> {
        let mut out = Vec::new();
        let keys: Vec<u32> = self.dirty.iter().take(max).collect();
        for k in keys {
            self.dirty.remove(k);
            if let Some(p) = self.cells.remove(&k) {
                out.push(p);
            }
        }
        out
    }
}

/// Redstone dirty graph — nodes are block positions; edges = wire connections.
/// Compatible mode: BFS from dirty sources without changing vanilla power rules externally.
#[derive(Debug, Default)]
pub struct RedstoneDirtyGraph {
    adj: FxHashMap<u64, Vec<u64>>,
    dirty: RoaringBitmap,
    pos: FxHashMap<u64, BlockPos>,
}

impl RedstoneDirtyGraph {
    pub fn connect(&mut self, a: BlockPos, b: BlockPos) {
        let ka = a.pack();
        let kb = b.pack();
        self.pos.insert(ka, a);
        self.pos.insert(kb, b);
        self.adj.entry(ka).or_default().push(kb);
        self.adj.entry(kb).or_default().push(ka);
    }

    pub fn mark_dirty(&mut self, p: BlockPos) {
        let k = p.pack();
        self.pos.insert(k, p);
        self.dirty.insert((k & 0xffff_ffff) as u32);
    }

    /// Compatible BFS: returns nodes to recompute this tick (bounded).
    pub fn propagate_compatible(&mut self, max_nodes: usize) -> Vec<BlockPos> {
        let seeds: Vec<u64> = self
            .dirty
            .iter()
            .filter_map(|low| {
                self.pos
                    .keys()
                    .find(|k| (*k & 0xffff_ffff) as u32 == low)
                    .copied()
            })
            .collect();
        self.dirty.clear();
        let mut seen = RoaringBitmap::new();
        let mut q = VecDeque::new();
        let mut out = Vec::new();
        for s in seeds {
            q.push_back(s);
            seen.insert((s & 0xffff_ffff) as u32);
        }
        while let Some(n) = q.pop_front() {
            if out.len() >= max_nodes {
                // Re-dirty remainder for next tick — preserve compatibility over dropping.
                self.dirty.insert((n & 0xffff_ffff) as u32);
                continue;
            }
            if let Some(p) = self.pos.get(&n) {
                out.push(*p);
            }
            if let Some(nei) = self.adj.get(&n) {
                for &m in nei {
                    let low = (m & 0xffff_ffff) as u32;
                    if seen.insert(low) {
                        q.push_back(m);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_and_dedupe() {
        let mut w = TickWheel::new();
        w.schedule(
            ScheduledTick {
                pos: BlockPos { x: 1, y: 2, z: 3 },
                priority: 0,
            },
            2,
        );
        assert!(w.advance().is_empty());
        let t = w.advance();
        assert_eq!(t.len(), 1);
        let mut d = BlockUpdateDeduper::default();
        let p = BlockPos { x: 1, y: 1, z: 1 };
        d.push(p);
        d.push(p);
        assert_eq!(d.drain().len(), 1);
    }
}
