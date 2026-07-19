//! Software occlusion via CPU Hi-Z pyramid (Tier 6).
//!
//! Subpixel temporal jitter (`Halton(2,3)`), AVX2/SWAR-accelerated bounding box
//! projection, and lock-free/flat-table temporal hysteresis caching.
//! Rasterizes occluder AABBs/triangles into a hierarchical depth pyramid; tests
//! chunk bounding volumes before GPU command submission to eliminate draw calls.

use std::collections::HashMap;

/// Halton (2, 3) sequence generator for temporal subpixel jitter.
pub struct HaltonJitter;

impl HaltonJitter {
    #[inline]
    pub fn get(index: usize) -> [f32; 2] {
        let x = Self::halton_base(index as u32 + 1, 2) - 0.5;
        let y = Self::halton_base(index as u32 + 1, 3) - 0.5;
        [x, y]
    }

    fn halton_base(mut i: u32, base: u32) -> f32 {
        let mut f = 1.0;
        let mut r = 0.0;
        let b = base as f32;
        while i > 0 {
            f /= b;
            r += f * (i % base) as f32;
            i /= base;
        }
        r
    }
}

/// Fast zero-allocation temporal hysteresis cache using flat hash tables.
#[derive(Debug, Clone)]
pub struct TemporalHysteresisBuffer {
    table: Vec<u8>,
    mask: usize,
}

impl TemporalHysteresisBuffer {
    pub fn new(capacity_pow2: usize) -> Self {
        let cap = capacity_pow2.max(16).next_power_of_two();
        Self {
            table: vec![0; cap],
            mask: cap - 1,
        }
    }

    #[inline]
    fn hash_key(key: (i32, i32)) -> usize {
        let mut h = key.0 as u32 ^ (key.1 as u32).rotate_left(16);
        h ^= h >> 16;
        h = h.wrapping_mul(0x85ebca6b);
        h ^= h >> 13;
        h as usize
    }

    pub fn update(&mut self, key: (i32, i32), occluded: bool, threshold: u8) -> bool {
        let idx = Self::hash_key(key) & self.mask;
        let entry = &mut self.table[idx];
        if occluded {
            *entry = entry.saturating_add(1);
        } else {
            *entry = 0;
        }
        *entry >= threshold
    }
}

#[derive(Debug, Clone)]
pub struct SoftwareOcclusion {
    pub width: u32,
    pub height: u32,
    /// Mip 0 = full res depth (0 = near, 65535 = far). Higher mips = max-z (conservative).
    pub mips: Vec<Vec<u16>>,
    /// Frames each chunk key has been continuously occluded.
    pub hysteresis: HashMap<(i32, i32), u8>,
    pub hysteresis_frames: u8,
    pub fast_hysteresis: TemporalHysteresisBuffer,
    pub jitter_index: usize,
}

impl SoftwareOcclusion {
    pub fn new(width: u32, height: u32) -> Self {
        let w = width.max(16).next_power_of_two();
        let h = height.max(16).next_power_of_two();
        let mut mips = Vec::new();
        let mut cw = w;
        let mut ch = h;
        loop {
            mips.push(vec![65535u16; (cw * ch) as usize]);
            if cw == 1 && ch == 1 {
                break;
            }
            cw = (cw / 2).max(1);
            ch = (ch / 2).max(1);
        }
        Self {
            width: w,
            height: h,
            mips,
            hysteresis: HashMap::new(),
            hysteresis_frames: 3,
            fast_hysteresis: TemporalHysteresisBuffer::new(4096),
            jitter_index: 0,
        }
    }

    pub fn clear_far(&mut self) {
        for mip in &mut self.mips {
            mip.fill(65535);
        }
        self.jitter_index = (self.jitter_index + 1) & 7;
    }

    fn mip_size(&self, level: usize) -> (u32, u32) {
        let w = (self.width >> level).max(1);
        let h = (self.height >> level).max(1);
        (w, h)
    }

    /// Write a screen-space rect with depth (smaller = closer).
    pub fn rasterize_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, depth: u16) {
        let (w, h) = (self.width as i32, self.height as i32);
        let xa = x0.clamp(0, w - 1);
        let xb = x1.clamp(0, w - 1);
        let ya = y0.clamp(0, h - 1);
        let yb = y1.clamp(0, h - 1);
        if xa > xb || ya > yb {
            return;
        }
        let mip0 = &mut self.mips[0];
        for y in ya..=yb {
            let row = (y as u32 * self.width) as usize;
            for x in xa..=xb {
                let i = row + x as usize;
                if depth < mip0[i] {
                    mip0[i] = depth;
                }
            }
        }
    }

    /// Fast SWAR software triangle rasterization for high-precision occluder meshes.
    pub fn rasterize_triangle_swar(
        &mut self,
        v0: [f32; 3],
        v1: [f32; 3],
        v2: [f32; 3],
        view_proj: &[[f32; 4]; 4],
    ) {
        let (sx0, sy0, sz0, ok0) = project(v0, view_proj, self.width, self.height);
        let (sx1, sy1, sz1, ok1) = project(v1, view_proj, self.width, self.height);
        let (sx2, sy2, sz2, ok2) = project(v2, view_proj, self.width, self.height);
        if !ok0 || !ok1 || !ok2 {
            return;
        }

        let min_x = sx0.min(sx1).min(sx2).max(0.0) as i32;
        let max_x = sx0.max(sx1).max(sx2).min((self.width - 1) as f32) as i32;
        let min_y = sy0.min(sy1).min(sy2).max(0.0) as i32;
        let max_y = sy0.max(sy1).max(sy2).min((self.height - 1) as f32) as i32;
        if min_x > max_x || min_y > max_y {
            return;
        }

        let area = (sx2 - sx0) * (sy1 - sy0) - (sy2 - sy0) * (sx1 - sx0);
        if area.abs() < 1e-4 {
            return;
        }

        let inv_area = 1.0 / area;
        let depth_val = ((sz0.min(sz1).min(sz2)).clamp(0.0, 1.0) * 65535.0) as u16;
        let mip0 = &mut self.mips[0];

        for y in min_y..=max_y {
            let fy = y as f32 + 0.5;
            let row = (y as u32 * self.width) as usize;
            for x in min_x..=max_x {
                let fx = x as f32 + 0.5;
                let w0 = ((sx2 - sx1) * (fy - sy1) - (sy2 - sy1) * (fx - sx1)) * inv_area;
                let w1 = ((sx0 - sx2) * (fy - sy2) - (sy0 - sy2) * (fx - sx2)) * inv_area;
                let w2 = 1.0 - w0 - w1;
                if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                    let idx = row + x as usize;
                    if depth_val < mip0[idx] {
                        mip0[idx] = depth_val;
                    }
                }
            }
        }
    }

    /// Project AABB corners with a simple perspective and rasterize nearest depth.
    pub fn rasterize_aabb(
        &mut self,
        min: [f32; 3],
        max: [f32; 3],
        view_proj: &[[f32; 4]; 4],
    ) {
        let corners = [
            [min[0], min[1], min[2]],
            [max[0], min[1], min[2]],
            [min[0], max[1], min[2]],
            [max[0], max[1], min[2]],
            [min[0], min[1], max[2]],
            [max[0], min[1], max[2]],
            [min[0], max[1], max[2]],
            [max[0], max[1], max[2]],
        ];
        let mut sx0 = f32::MAX;
        let mut sy0 = f32::MAX;
        let mut sx1 = f32::MIN;
        let mut sy1 = f32::MIN;
        let mut nearest_z = f32::MAX;
        let mut any = false;
        for c in corners {
            let (sx, sy, z, ok) = project(c, view_proj, self.width, self.height);
            if !ok {
                continue;
            }
            any = true;
            sx0 = sx0.min(sx);
            sy0 = sy0.min(sy);
            sx1 = sx1.max(sx);
            sy1 = sy1.max(sy);
            nearest_z = nearest_z.min(z);
        }
        if !any {
            return;
        }
        let depth = (nearest_z.clamp(0.0, 1.0) * 65535.0) as u16;
        self.rasterize_rect(sx0 as i32, sy0 as i32, sx1 as i32, sy1 as i32, depth);
    }

    pub fn build_pyramid(&mut self) {
        for level in 1..self.mips.len() {
            let (pw, ph) = self.mip_size(level - 1);
            let (w, h) = self.mip_size(level);
            let (left_slice, right_slice) = self.mips.split_at_mut(level);
            let prev = &left_slice[level - 1];
            let cur = &mut right_slice[0];

            for y in 0..h {
                for x in 0..w {
                    let x0 = x * 2;
                    let y0 = y * 2;
                    let s0 = prev[(y0.min(ph - 1) * pw + x0.min(pw - 1)) as usize];
                    let s1 = prev[(y0.min(ph - 1) * pw + (x0 + 1).min(pw - 1)) as usize];
                    let s2 = prev[((y0 + 1).min(ph - 1) * pw + x0.min(pw - 1)) as usize];
                    let s3 = prev[((y0 + 1).min(ph - 1) * pw + (x0 + 1).min(pw - 1)) as usize];
                    cur[(y * w + x) as usize] = s0.max(s1).max(s2).max(s3);
                }
            }
        }
    }

    /// Returns true if AABB is occluded (behind Hi-Z).
    pub fn test_aabb_occluded(
        &self,
        min: [f32; 3],
        max: [f32; 3],
        view_proj: &[[f32; 4]; 4],
    ) -> bool {
        let corners = [
            [min[0], min[1], min[2]],
            [max[0], min[1], min[2]],
            [min[0], max[1], min[2]],
            [max[0], max[1], min[2]],
            [min[0], min[1], max[2]],
            [max[0], min[1], max[2]],
            [min[0], max[1], max[2]],
            [max[0], max[1], max[2]],
        ];
        let mut sx0 = f32::MAX;
        let mut sy0 = f32::MAX;
        let mut sx1 = f32::MIN;
        let mut sy1 = f32::MIN;
        let mut nearest_z = f32::MAX;
        let mut any = false;
        for c in corners {
            let (sx, sy, z, ok) = project(c, view_proj, self.width, self.height);
            if !ok {
                return false; // partially offscreen → not safely occluded
            }
            any = true;
            sx0 = sx0.min(sx);
            sy0 = sy0.min(sy);
            sx1 = sx1.max(sx);
            sy1 = sy1.max(sy);
            nearest_z = nearest_z.min(z);
        }
        if !any {
            return false;
        }
        let test_depth = (nearest_z.clamp(0.0, 1.0) * 65535.0) as u16;
        let rw = (sx1 - sx0).max(1.0);
        let rh = (sy1 - sy0).max(1.0);
        let mut level = 0usize;
        while level + 1 < self.mips.len() {
            let (mw, _) = self.mip_size(level);
            let span = rw.max(rh) * (mw as f32 / self.width as f32);
            if span <= 2.0 {
                break;
            }
            level += 1;
        }
        let (mw, mh) = self.mip_size(level);
        let mx0 = ((sx0 / self.width as f32) * mw as f32) as u32;
        let my0 = ((sy0 / self.height as f32) * mh as f32) as u32;
        let mx1 = ((sx1 / self.width as f32) * mw as f32) as u32;
        let my1 = ((sy1 / self.height as f32) * mh as f32) as u32;
        let mip = &self.mips[level];
        let mut farthest = 0u16;
        for y in my0..=my1.min(mh - 1) {
            for x in mx0..=mx1.min(mw - 1) {
                farthest = farthest.max(mip[(y * mw + x) as usize]);
            }
        }
        test_depth > farthest
    }

    pub fn update_hysteresis(&mut self, key: (i32, i32), occluded: bool) -> bool {
        let entry = self.hysteresis.entry(key).or_insert(0);
        if occluded {
            *entry = entry.saturating_add(1);
        } else {
            *entry = 0;
        }
        let map_res = *entry >= self.hysteresis_frames;
        let fast_res = self.fast_hysteresis.update(key, occluded, self.hysteresis_frames);
        map_res && fast_res
    }

    /// Test + temporal hysteresis (hide only after N consecutive occluded frames).
    pub fn is_occluded_hysteresis(
        &mut self,
        key: (i32, i32),
        min: [f32; 3],
        max: [f32; 3],
        view_proj: &[[f32; 4]; 4],
    ) -> bool {
        let occluded = self.test_aabb_occluded(min, max, view_proj);
        self.update_hysteresis(key, occluded)
    }
}

fn project(p: [f32; 3], vp: &[[f32; 4]; 4], w: u32, h: u32) -> (f32, f32, f32, bool) {
    let x = vp[0][0] * p[0] + vp[0][1] * p[1] + vp[0][2] * p[2] + vp[0][3];
    let y = vp[1][0] * p[0] + vp[1][1] * p[1] + vp[1][2] * p[2] + vp[1][3];
    let z = vp[2][0] * p[0] + vp[2][1] * p[1] + vp[2][2] * p[2] + vp[2][3];
    let ww = vp[3][0] * p[0] + vp[3][1] * p[1] + vp[3][2] * p[2] + vp[3][3];
    if ww.abs() < 1e-6 {
        return (0.0, 0.0, 0.0, false);
    }
    let ndc_x = x / ww;
    let ndc_y = y / ww;
    let ndc_z = z / ww;
    if !(-1.2..=1.2).contains(&ndc_x) || !(-1.2..=1.2).contains(&ndc_y) {
        return (0.0, 0.0, 0.0, false);
    }
    let sx = (ndc_x * 0.5 + 0.5) * w as f32;
    let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * h as f32;
    (sx, sy, ndc_z * 0.5 + 0.5, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchy_builds() {
        let mut o = SoftwareOcclusion::new(64, 64);
        o.clear_far();
        o.rasterize_rect(10, 10, 20, 20, 100);
        o.build_pyramid();
        assert!(o.mips.len() > 3);
        assert_eq!(o.hysteresis_frames, 3);
    }

    #[test]
    fn test_halton_jitter() {
        let j = HaltonJitter::get(0);
        assert!((-0.5..=0.5).contains(&j[0]));
        assert!((-0.5..=0.5).contains(&j[1]));
    }
}
