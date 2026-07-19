//! Phase 1 — sound coalescing + particle caps + UI dirty rects.

use rustc_hash::FxHashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SoundKey {
    pub id: u32,
    pub cell_x: i32,
    pub cell_z: i32,
}

#[derive(Debug, Default)]
pub struct SoundBatcher {
    pending: FxHashMap<SoundKey, u32>, // count
}

impl SoundBatcher {
    pub fn play(&mut self, key: SoundKey) {
        *self.pending.entry(key).or_insert(0) += 1;
    }

    /// Merge identical sounds in same cell → one play with gain≈log2(count).
    pub fn flush(&mut self) -> Vec<(SoundKey, f32)> {
        let out: Vec<_> = self
            .pending
            .drain()
            .map(|(k, c)| {
                let gain = 1.0 + (c as f32).log2() * 0.15;
                (k, gain.min(2.0))
            })
            .collect();
        out
    }
}

#[derive(Debug)]
pub struct ParticleLimiter {
    pub max_per_frame: u32,
    spawned: u32,
}

impl ParticleLimiter {
    pub fn new(max_per_frame: u32) -> Self {
        Self {
            max_per_frame: max_per_frame.max(1),
            spawned: 0,
        }
    }

    pub fn begin_frame(&mut self) {
        self.spawned = 0;
    }

    pub fn try_spawn(&mut self, count: u32) -> u32 {
        let left = self.max_per_frame.saturating_sub(self.spawned);
        let n = count.min(left);
        self.spawned += n;
        n
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UiRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Default)]
pub struct UiDirtyTracker {
    dirty: Vec<UiRect>,
}

impl UiDirtyTracker {
    pub fn mark(&mut self, r: UiRect) {
        // Merge if intersects existing
        for d in &mut self.dirty {
            if intersects(*d, r) {
                *d = union(*d, r);
                return;
            }
        }
        self.dirty.push(r);
    }

    pub fn take(&mut self) -> Vec<UiRect> {
        std::mem::take(&mut self.dirty)
    }
}

fn intersects(a: UiRect, b: UiRect) -> bool {
    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
}

fn union(a: UiRect, b: UiRect) -> UiRect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.w).max(b.x + b.w);
    let y1 = (a.y + a.h).max(b.y + b.h);
    UiRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesce_and_cap() {
        let mut s = SoundBatcher::default();
        let k = SoundKey {
            id: 1,
            cell_x: 0,
            cell_z: 0,
        };
        s.play(k);
        s.play(k);
        assert_eq!(s.flush().len(), 1);
        let mut p = ParticleLimiter::new(10);
        p.begin_frame();
        assert_eq!(p.try_spawn(7), 7);
        assert_eq!(p.try_spawn(7), 3);
    }
}
