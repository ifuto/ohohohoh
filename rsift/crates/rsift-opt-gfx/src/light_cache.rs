//! Block/skylight propagation cache (Tier 6).
//! Dirty BFS flood — only recompute changed sections (Minecraft light engine idea).

use std::collections::VecDeque;

const SEC: usize = 16;
const SEC_VOL: usize = SEC * SEC * SEC;

#[derive(Debug, Clone)]
pub struct SectionLights {
    /// Packed: low4 = block, high4 = sky
    pub packed: Vec<u8>,
    pub dirty: bool,
}

impl SectionLights {
    pub fn new() -> Self {
        Self {
            packed: vec![0; SEC_VOL],
            dirty: true,
        }
    }

    #[inline]
    fn idx(x: usize, y: usize, z: usize) -> usize {
        (y * SEC + z) * SEC + x
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> (u8, u8) {
        let v = self.packed[Self::idx(x, y, z)];
        (v & 0x0f, v >> 4)
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, block: u8, sky: u8) {
        self.packed[Self::idx(x, y, z)] = (block & 0x0f) | ((sky & 0x0f) << 4);
        self.dirty = true;
    }
}

impl Default for SectionLights {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
pub struct LightPropagationCache {
    sections: std::collections::HashMap<(i32, i32, i32), SectionLights>,
}

impl LightPropagationCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn section_mut(&mut self, cx: i32, cy: i32, cz: i32) -> &mut SectionLights {
        self.sections
            .entry((cx, cy, cz))
            .or_insert_with(SectionLights::new)
    }

    pub fn get_light(&self, wx: i32, wy: i32, wz: i32) -> (u8, u8) {
        let cx = wx.div_euclid(16);
        let cy = wy.div_euclid(16);
        let cz = wz.div_euclid(16);
        let lx = wx.rem_euclid(16) as usize;
        let ly = wy.rem_euclid(16) as usize;
        let lz = wz.rem_euclid(16) as usize;
        self.sections
            .get(&(cx, cy, cz))
            .map(|s| s.get(lx, ly, lz))
            .unwrap_or((0, 0))
    }

    pub fn mark_dirty(&mut self, wx: i32, wy: i32, wz: i32) {
        let cx = wx.div_euclid(16);
        let cy = wy.div_euclid(16);
        let cz = wz.div_euclid(16);
        self.section_mut(cx, cy, cz).dirty = true;
    }

    pub fn set_emitter(&mut self, wx: i32, wy: i32, wz: i32, level: u8) {
        let cx = wx.div_euclid(16);
        let cy = wy.div_euclid(16);
        let cz = wz.div_euclid(16);
        let lx = wx.rem_euclid(16) as usize;
        let ly = wy.rem_euclid(16) as usize;
        let lz = wz.rem_euclid(16) as usize;
        let s = self.section_mut(cx, cy, cz);
        let (_, sky) = s.get(lx, ly, lz);
        s.set(lx, ly, lz, level.min(15), sky);
    }

    /// BFS propagate block light within a section (opaque mask: true = blocks light).
    pub fn propagate_dirty(&mut self, cx: i32, cy: i32, cz: i32, opaque: &[bool; SEC_VOL], max_steps: usize) -> u32 {
        let Some(sec) = self.sections.get_mut(&(cx, cy, cz)) else {
            return 0;
        };
        if !sec.dirty {
            return 0;
        }
        let mut queue = VecDeque::new();
        for i in 0..SEC_VOL {
            let (b, _) = {
                let v = sec.packed[i];
                (v & 0x0f, v >> 4)
            };
            if b > 0 {
                queue.push_back(i);
            }
        }
        let mut steps = 0usize;
        while let Some(i) = queue.pop_front() {
            if steps >= max_steps {
                break;
            }
            steps += 1;
            let level = sec.packed[i] & 0x0f;
            if level <= 1 {
                continue;
            }
            let x = i % SEC;
            let z = (i / SEC) % SEC;
            let y = i / (SEC * SEC);
            let neighbors = [
                (x.wrapping_sub(1), y, z),
                (x + 1, y, z),
                (x, y.wrapping_sub(1), z),
                (x, y + 1, z),
                (x, y, z.wrapping_sub(1)),
                (x, y, z + 1),
            ];
            for (nx, ny, nz) in neighbors {
                if nx >= SEC || ny >= SEC || nz >= SEC {
                    continue;
                }
                let ni = (ny * SEC + nz) * SEC + nx;
                if opaque[ni] {
                    continue;
                }
                let next = level - 1;
                let (cur_b, sky) = {
                    let v = sec.packed[ni];
                    (v & 0x0f, v >> 4)
                };
                if next > cur_b {
                    sec.packed[ni] = next | (sky << 4);
                    queue.push_back(ni);
                }
            }
        }
        sec.dirty = queue.is_empty() == false && steps >= max_steps;
        if queue.is_empty() {
            sec.dirty = false;
        }
        steps as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_spreads() {
        let mut c = LightPropagationCache::new();
        c.set_emitter(0, 0, 0, 15);
        let opaque = [false; SEC_VOL];
        let steps = c.propagate_dirty(0, 0, 0, &opaque, 10_000);
        assert!(steps > 0);
        let (b, _) = c.get_light(2, 0, 0);
        assert!(b > 0 && b < 15);
    }
}
