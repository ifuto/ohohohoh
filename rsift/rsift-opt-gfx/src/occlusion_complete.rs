//! Software occlusion via CPU Hi-Z pyramid (Tier 6).
//! Rasterize chunk AABBs into a depth pyramid; test before submitting draws.

#[derive(Debug, Clone)]
pub struct SoftwareOcclusion {
    pub width: u32,
    pub height: u32,
    /// Mip 0 = full res depth (0 = near, 65535 = far). Higher mips = max-z (conservative).
    pub mips: Vec<Vec<u16>>,
    /// Frames each chunk key has been continuously occluded.
    pub hysteresis: std::collections::HashMap<(i32, i32), u8>,
    pub hysteresis_frames: u8,
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
            hysteresis: std::collections::HashMap::new(),
            hysteresis_frames: 3,
        }
    }

    pub fn clear_far(&mut self) {
        for mip in &mut self.mips {
            mip.fill(65535);
        }
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
            for x in xa..=xb {
                let i = (y as u32 * self.width + x as u32) as usize;
                if depth < mip0[i] {
                    mip0[i] = depth;
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
            let prev = self.mips[level - 1].clone();
            let cur = &mut self.mips[level];
            for y in 0..h {
                for x in 0..w {
                    let x0 = x * 2;
                    let y0 = y * 2;
                    let samples = [
                        prev[(y0.min(ph - 1) * pw + x0.min(pw - 1)) as usize],
                        prev[(y0.min(ph - 1) * pw + (x0 + 1).min(pw - 1)) as usize],
                        prev[((y0 + 1).min(ph - 1) * pw + x0.min(pw - 1)) as usize],
                        prev[((y0 + 1).min(ph - 1) * pw + (x0 + 1).min(pw - 1)) as usize],
                    ];
                    // Max-z = farthest (conservative occlusion: if max behind test depth, occluded)
                    cur[(y * w + x) as usize] = samples.iter().copied().max().unwrap();
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
        // Pick mip where rect ~ 1 texel
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
        // Occluded if nearest point of AABB is behind the farthest occluder in the region
        test_depth > farthest
    }

    pub fn update_hysteresis(&mut self, key: (i32, i32), occluded: bool) -> bool {
        let entry = self.hysteresis.entry(key).or_insert(0);
        if occluded {
            *entry = entry.saturating_add(1);
        } else {
            *entry = 0;
        }
        *entry >= self.hysteresis_frames
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
}
