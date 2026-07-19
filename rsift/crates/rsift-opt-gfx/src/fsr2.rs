//! FSR 2.0 — FidelityFX Super Resolution (temporal). Jitter + motion-vector
//! reprojection + neighborhood-clamped history accumulation.
//!
//! Renders the scene at a lower internal resolution and reconstructs the display
//! resolution across frames. Pure shader math — no special hardware — so it
//! works on Intel Iris/Arc and Apple Silicon iGPUs. After FSR1, this is the
//! biggest quality/perf lever for integrated GPUs.

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}
impl Vec3 {
    pub fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }
    pub fn clamp(self, lo: f32, hi: f32) -> Vec3 {
        Vec3::new(self.r.clamp(lo, hi), self.g.clamp(lo, hi), self.b.clamp(lo, hi))
    }
}
impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.r + o.r, self.g + o.g, self.b + o.b)
    }
}
impl std::ops::Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.r - o.r, self.g - o.g, self.b - o.b)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.r * s, self.g * s, self.b * s)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Fsr2 {
    pub input_w: usize,
    pub input_h: usize,
    pub output_w: usize,
    pub output_h: usize,
    /// Base EMA blend of history (0..1). Higher = more temporal stability.
    pub history_blend: f32,
    /// Jitter amplitude as a fraction of a pixel (0.5 = full pixel jitter).
    pub jitter_scale: f32,
}
impl Fsr2 {
    pub fn new(input_w: usize, input_h: usize, output_w: usize, output_h: usize) -> Self {
        Self {
            input_w,
            input_h,
            output_w,
            output_h,
            history_blend: 0.9,
            jitter_scale: 0.5,
        }
    }

    /// Halton sequence value for `i` (1-based) with the given `base`, in [0,1).
    pub fn halton(i: u32, base: u32) -> f32 {
        let mut f = 1.0f32;
        let mut r = 0.0f32;
        let mut n = i.max(1);
        while n > 0 {
            f /= base as f32;
            r += f * (n % base) as f32;
            n /= base;
        }
        r
    }

    /// Sub-pixel jitter offset (in pixels) for `frame` (0-based), in
    /// [-jitter_scale, +jitter_scale] for each axis.
    pub fn jitter(&self, frame: u32) -> (f32, f32) {
        let x = Self::halton(frame + 1, 2) - 0.5;
        let y = Self::halton(frame + 1, 3) - 0.5;
        (x * 2.0 * self.jitter_scale, y * 2.0 * self.jitter_scale)
    }

    /// Reproject a current-frame UV to previous-frame UV via a motion vector
    /// expressed in UV space (prevUV - curUV).
    pub fn reproject(&self, uv: (f32, f32), mv: (f32, f32)) -> (f32, f32) {
        let px = (uv.0 + mv.0).clamp(0.0, 1.0);
        let py = (uv.1 + mv.1).clamp(0.0, 1.0);
        (px, py)
    }

    /// Clamp `c` into the AABB of a 3x3 neighborhood (kills ghosting from
    /// disocclusions). `nb` is row-major [nw, n, ne, w, c, e, sw, s, se].
    pub fn neighborhood_clamp(&self, center: Vec3, nb: &[Vec3; 9]) -> Vec3 {
        let mut mn = nb[0];
        let mut mx = nb[0];
        for &c in nb.iter() {
            mn = vec_min(mn, c);
            mx = vec_max(mx, c);
        }
        Vec3::new(
            center.r.clamp(mn.r, mx.r),
            center.g.clamp(mn.g, mx.g),
            center.b.clamp(mn.b, mx.b),
        )
    }

    /// Resolve the current frame with history. `disocclusion` in [0,1]
    /// (1 = no usable history, e.g. camera cut / newly exposed surface).
    pub fn resolve(&self, current: Vec3, history: Vec3, disocclusion: f32) -> Vec3 {
        let a = self.history_blend.max(disocclusion.clamp(0.0, 1.0));
        current * a + history * (1.0 - a)
    }

    pub fn wgsl_source(&self) -> &'static str {
        FSR2_WGSL
    }
}

fn vec_min(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.r.min(b.r), a.g.min(b.g), a.b.min(b.b))
}
fn vec_max(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.r.max(b.r), a.g.max(b.g), a.b.max(b.b))
}

pub const FSR2_WGSL: &str = include_str!("../shaders/fsr2.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    fn solid(v: f32) -> Vec3 {
        Vec3::new(v, v, v)
    }
    #[test]
    fn halton_in_unit() {
        for i in 1..16u32 {
            let h = Fsr2::halton(i, 2);
            assert!((0.0..1.0).contains(&h), "halton out of range");
        }
    }
    #[test]
    fn halton_deterministic() {
        assert!((Fsr2::halton(1, 2) - 0.5).abs() < 1e-6);
        assert!((Fsr2::halton(2, 2) - 0.25).abs() < 1e-6);
    }
    #[test]
    fn jitter_in_range() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        for fr in 0..8u32 {
            let (x, y) = f.jitter(fr);
            assert!(x.abs() <= f.jitter_scale + 1e-6);
            assert!(y.abs() <= f.jitter_scale + 1e-6);
        }
    }
    #[test]
    fn reproject_moves() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let p = f.reproject((0.5, 0.5), (-0.1, 0.02));
        assert!((p.0 - 0.4).abs() < 1e-6);
        assert!((p.1 - 0.52).abs() < 1e-6);
    }
    #[test]
    fn clamp_keeps_inside() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let nb = [
            solid(0.2),
            solid(0.3),
            solid(0.25),
            solid(0.4),
            solid(0.35),
            solid(0.1),
            solid(0.5),
            solid(0.45),
            solid(0.15),
        ];
        let c = solid(0.9);
        let r = f.neighborhood_clamp(c, &nb);
        assert!(r.r <= 0.5 + 1e-6);
        assert!(r.r >= 0.1 - 1e-6);
    }
    #[test]
    fn resolve_disocclusion_uses_current() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let r = f.resolve(solid(0.7), solid(0.1), 1.0);
        assert!((r.r - 0.7).abs() < 1e-6);
    }
    #[test]
    fn resolve_stable_blends() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let r = f.resolve(solid(0.6), solid(0.4), 0.0);
        // a = 0.9 => current*0.9 + history*0.1 = 0.54 + 0.04 = 0.58
        assert!((r.r - 0.58).abs() < 1e-6);
    }
}
