//! Phase 2/3 — Starlight-inspired stateless section lighting (4-bit nibbles, 16-level queue).

use roaring::RoaringBitmap;
use rustc_hash::FxHashMap;

pub const SEC: usize = 16;
pub const SEC_VOL: usize = SEC * SEC * SEC;

/// Packed nibble array: low/high 4 bits per byte (Minecraft style).
#[derive(Debug, Clone)]
pub struct NibbleArray {
    pub data: Vec<u8>, // SEC_VOL/2 bytes
}

impl NibbleArray {
    pub fn new_zero() -> Self {
        Self {
            data: vec![0u8; SEC_VOL / 2],
        }
    }

    pub fn filled(level: u8) -> Self {
        let n = level & 0x0f;
        let b = n | (n << 4);
        Self {
            data: vec![b; SEC_VOL / 2],
        }
    }

    #[inline]
    pub fn get(&self, idx: usize) -> u8 {
        let b = self.data[idx >> 1];
        if idx & 1 == 0 {
            b & 0x0f
        } else {
            b >> 4
        }
    }

    #[inline]
    pub fn set(&mut self, idx: usize, value: u8) {
        let v = value & 0x0f;
        let i = idx >> 1;
        if idx & 1 == 0 {
            self.data[i] = (self.data[i] & 0xf0) | v;
        } else {
            self.data[i] = (self.data[i] & 0x0f) | (v << 4);
        }
    }
}

#[inline]
pub fn section_index(x: usize, y: usize, z: usize) -> usize {
    (y * SEC + z) * SEC + x
}

#[derive(Debug, Clone)]
pub struct LightSection {
    pub block: NibbleArray,
    pub sky: NibbleArray,
}

impl LightSection {
    pub fn new() -> Self {
        Self {
            block: NibbleArray::new_zero(),
            sky: NibbleArray::filled(15),
        }
    }
}

impl Default for LightSection {
    fn default() -> Self {
        Self::new()
    }
}

/// 16 fixed queues by light level (Starlight / vanilla increase-queue pattern).
pub struct LightIncreaseQueue {
    levels: [Vec<u32>; 16],
}

impl LightIncreaseQueue {
    pub fn new() -> Self {
        Self {
            levels: std::array::from_fn(|_| Vec::new()),
        }
    }

    pub fn clear(&mut self) {
        for q in &mut self.levels {
            q.clear();
        }
    }

    pub fn push(&mut self, level: u8, encoded_pos: u32) {
        let l = (level as usize).min(15);
        self.levels[l].push(encoded_pos);
    }

    pub fn pop_highest(&mut self) -> Option<(u8, u32)> {
        for level in (1..16).rev() {
            if let Some(p) = self.levels[level].pop() {
                return Some((level as u8, p));
            }
        }
        None
    }
}

impl Default for LightIncreaseQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
pub struct StarlightEngine {
    sections: FxHashMap<(i32, i32, i32), LightSection>,
    dirty: RoaringBitmap,
    /// Delayed far updates (chunk key packed).
    delayed: Vec<(i32, i32, i32)>,
}

impl StarlightEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn pack_section(cx: i32, cy: i32, cz: i32) -> u32 {
        // 10+6+10 bits — enough for local worlds in tests / streaming windows
        let x = (cx + 512) as u32 & 0x3ff;
        let y = (cy + 32) as u32 & 0x3f;
        let z = (cz + 512) as u32 & 0x3ff;
        x | (y << 10) | (z << 16)
    }

    pub fn section_mut(&mut self, cx: i32, cy: i32, cz: i32) -> &mut LightSection {
        self.sections
            .entry((cx, cy, cz))
            .or_insert_with(LightSection::new)
    }

    pub fn mark_dirty_section(&mut self, cx: i32, cy: i32, cz: i32) {
        self.dirty.insert(Self::pack_section(cx, cy, cz));
    }

    pub fn set_block_light(&mut self, wx: i32, wy: i32, wz: i32, level: u8) {
        let cx = wx.div_euclid(16);
        let cy = wy.div_euclid(16);
        let cz = wz.div_euclid(16);
        let lx = wx.rem_euclid(16) as usize;
        let ly = wy.rem_euclid(16) as usize;
        let lz = wz.rem_euclid(16) as usize;
        let idx = section_index(lx, ly, lz);
        self.section_mut(cx, cy, cz).block.set(idx, level);
        self.mark_dirty_section(cx, cy, cz);
    }

    pub fn get_block_light(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        let cx = wx.div_euclid(16);
        let cy = wy.div_euclid(16);
        let cz = wz.div_euclid(16);
        let lx = wx.rem_euclid(16) as usize;
        let ly = wy.rem_euclid(16) as usize;
        let lz = wz.rem_euclid(16) as usize;
        self.sections
            .get(&(cx, cy, cz))
            .map(|s| s.block.get(section_index(lx, ly, lz)))
            .unwrap_or(0)
    }

    /// Propagate only dirty sections. `opaque[idx]=true` blocks light.
    pub fn propagate_dirty(
        &mut self,
        cx: i32,
        cy: i32,
        cz: i32,
        opaque: &[bool; SEC_VOL],
        max_steps: usize,
    ) -> u32 {
        let key = Self::pack_section(cx, cy, cz);
        if !self.dirty.contains(key) {
            return 0;
        }
        let Some(sec) = self.sections.get_mut(&(cx, cy, cz)) else {
            self.dirty.remove(key);
            return 0;
        };
        let mut q = LightIncreaseQueue::new();
        for i in 0..SEC_VOL {
            let l = sec.block.get(i);
            if l > 0 {
                q.push(l, i as u32);
            }
        }
        let mut steps = 0usize;
        while let Some((level, pos)) = q.pop_highest() {
            if steps >= max_steps {
                // Defer remainder
                self.delayed.push((cx, cy, cz));
                break;
            }
            steps += 1;
            if level <= 1 {
                continue;
            }
            let i = pos as usize;
            let x = i % SEC;
            let z = (i / SEC) % SEC;
            let y = i / (SEC * SEC);
            for (nx, ny, nz) in [
                (x.wrapping_sub(1), y, z),
                (x + 1, y, z),
                (x, y.wrapping_sub(1), z),
                (x, y + 1, z),
                (x, y, z.wrapping_sub(1)),
                (x, y, z + 1),
            ] {
                if nx >= SEC || ny >= SEC || nz >= SEC {
                    continue;
                }
                let ni = section_index(nx, ny, nz);
                if opaque[ni] {
                    continue;
                }
                let next = level - 1;
                if next > sec.block.get(ni) {
                    sec.block.set(ni, next);
                    q.push(next, ni as u32);
                }
            }
        }
        if steps < max_steps {
            self.dirty.remove(key);
        }
        steps as u32
    }

    pub fn flush_delayed(&mut self, max_sections: usize) -> usize {
        let n = self.delayed.len().min(max_sections);
        self.delayed.drain(0..n).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nibble_and_propagate() {
        let mut eng = StarlightEngine::new();
        eng.set_block_light(0, 0, 0, 15);
        let opaque = [false; SEC_VOL];
        let steps = eng.propagate_dirty(0, 0, 0, &opaque, 50_000);
        assert!(steps > 10);
        assert!(eng.get_block_light(3, 0, 0) > 0);
        assert!(eng.get_block_light(3, 0, 0) < 15);
    }
}
