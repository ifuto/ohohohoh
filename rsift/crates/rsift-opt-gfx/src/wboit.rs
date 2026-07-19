//! Weighted Blended Order-Independent Transparency (McGuire & Bavoil 2013).
//!
//! Renders transparent surfaces in a single geometry pass with no sorting.
//! Each fragment blends `(color.rgb * color.a * weight, color.a * weight)`; a
//! final resolve divides by the summed alpha. A depth-based weight biases toward
//! nearer surfaces, keeping smoke/glass plausible while avoiding sort stalls —
//! a good fit when transparent foliage/water is the bottleneck on low-spec GPUs.

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec4 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Vec4 {
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Wboit {
    /// `near` depth used to normalize the depth weight.
    pub near: f32,
    /// Higher `k` => nearer fragments dominate more strongly.
    pub k: f32,
}
impl Default for Wboit {
    fn default() -> Self {
        Self { near: 0.1, k: 8.0 }
    }
}
impl Wboit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Per-fragment weight: nearer (smaller depth) => larger weight.
    pub fn weight(&self, depth: f32) -> f32 {
        let d = (depth - self.near).max(0.0);
        1.0 / (1.0 + d * self.k)
    }

    /// Accumulate one fragment into the running sums. `sum` holds
    /// `(r*a*w, g*a*w, b*a*w, a*w)` and `depth` is the fragment's view depth.
    pub fn accumulate(&self, sum: &mut Vec4, color: Vec4, depth: f32) {
        let w = self.weight(depth) * color.a;
        sum.r += color.r * w;
        sum.g += color.g * w;
        sum.b += color.b * w;
        sum.a += w;
    }

    /// Resolve the accumulated buffer to a final color (alpha-premultiplied).
    pub fn resolve(&self, sum: Vec4) -> Vec4 {
        if sum.a <= 1e-5 {
            return Vec4::default();
        }
        Vec4 {
            r: sum.r / sum.a,
            g: sum.g / sum.a,
            b: sum.b / sum.a,
            a: sum.a,
        }
    }

    pub fn wgsl_source(&self) -> &'static str {
        WBOIT_WGSL
    }
}

pub const WBOIT_WGSL: &str = include_str!("../shaders/wboit.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearer_has_higher_weight() {
        let w = Wboit::new();
        assert!(w.weight(0.1) > w.weight(5.0));
    }
    #[test]
    fn front_biased_result() {
        let w = Wboit::new();
        let mut sum = Vec4::default();
        // far red, near blue
        w.accumulate(&mut sum, Vec4::new(1.0, 0.0, 0.0, 0.5), 5.0);
        w.accumulate(&mut sum, Vec4::new(0.0, 0.0, 1.0, 0.5), 0.1);
        let r = w.resolve(sum);
        // blue (near) should dominate red
        assert!(r.b > r.r, "near color should dominate: {:?}", r);
    }
    #[test]
    fn empty_resolves_transparent() {
        let w = Wboit::new();
        let r = w.resolve(Vec4::default());
        assert_eq!(r.a, 0.0);
    }
    #[test]
    fn single_fragment_normalizes_to_itself() {
        let w = Wboit::new();
        let mut sum = Vec4::default();
        w.accumulate(&mut sum, Vec4::new(0.2, 0.4, 0.6, 0.8), 1.0);
        let r = w.resolve(sum);
        // resolve normalizes rgb by sum.a; with one fragment a = w, rgb = color*w/w = color
        assert!((r.r - 0.2).abs() < 1e-5);
        assert!((r.g - 0.4).abs() < 1e-5);
        assert!((r.b - 0.6).abs() < 1e-5);
    }
}
