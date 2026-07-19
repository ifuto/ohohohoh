//! Subgroup / wavefront operation helpers for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): a CPU simulation of the wave ops modern GPUs expose
//! (reduce-add across a fixed wave, and a ballot mask). These back the
//! GPU-driven culling / prefix-sum passes — they let one invocation see its
//! neighbours' data, cutting bandwidth on integrated GPUs.

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}
impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}
impl Vec4 {
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}
impl Add for Vec4 {
    type Output = Vec4;
    fn add(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x + o.x, self.y + o.y, self.z + o.z, self.w + o.w)
    }
}
impl Sub for Vec4 {
    type Output = Vec4;
    fn sub(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x - o.x, self.y - o.y, self.z - o.z, self.w - o.w)
    }
}
impl Mul<f32> for Vec4 {
    type Output = Vec4;
    fn mul(self, s: f32) -> Vec4 {
        Vec4::new(self.x * s, self.y * s, self.z * s, self.w * s)
    }
}

/// Width of a hardware "wave"/"subgroup" (e.g. 32 on NVIDIA, 32/64 on AMD).
pub const WAVE_WIDTH: usize = 32;

/// Per-lane result of a wave-wide reduce-add: every lane in a wave gets the
/// sum of all lanes in that wave.
pub fn subgroup_reduce_add(values: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; values.len()];
    let mut i = 0usize;
    while i < values.len() {
        let wave_start = (i / WAVE_WIDTH) * WAVE_WIDTH;
        let wave_end = (((i / WAVE_WIDTH) + 1) * WAVE_WIDTH).min(values.len());
        let mut s = 0.0f32;
        for j in wave_start..wave_end {
            s += values[j];
        }
        for j in wave_start..wave_end {
            out[j] = s;
        }
        i = wave_end;
    }
    out
}

/// Ballot mask: bit `j` is set when lane `j` is true. Returns the full mask for
/// all lanes supplied.
pub fn subgroup_ballot(cond: &[bool]) -> u64 {
    let mut mask = 0u64;
    for (j, &c) in cond.iter().enumerate() {
        if c && j < 64 {
            mask |= 1u64 << (j as u32);
        }
    }
    mask
}

pub fn wgsl_source() -> &'static str {
    SUBGROUP_WGSL
}

pub const SUBGROUP_WGSL: &str = include_str!("../shaders/subgroup.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reduce_sums_each_wave() {
        let v: Vec<f32> = (0..33).map(|x| x as f32).collect();
        let out = subgroup_reduce_add(&v);
        // First 32 lanes share one wave -> sum 0..=31 = 496.
        assert!((out[0] - 496.0).abs() < 1e-6);
        assert!((out[31] - 496.0).abs() < 1e-6);
        // Lane 32 is its own wave -> sum = 32.
        assert!((out[32] - 32.0).abs() < 1e-6);
    }
    #[test]
    fn ballot_sets_true_lanes() {
        let cond = [true, false, true, false];
        let m = subgroup_ballot(&cond);
        assert_eq!(m, 0b0101);
    }
    #[test]
    fn ballot_empty_is_zero() {
        let cond = [false; 8];
        assert_eq!(subgroup_ballot(&cond), 0);
    }
}
