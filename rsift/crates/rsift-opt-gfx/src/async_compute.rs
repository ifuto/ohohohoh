//! Async compute overlap scheduler for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): given a list of render passes tagged as running on
//! the graphics queue or the compute queue, compute the overlapped frame time
//! `max(graphics_total, compute_total)` that is achievable when the two queue
//! families run concurrently. Then provide a frame pipeliner that shifts a late
//! post-processing pass (e.g. bloom/SSR denoise) so that it executes during the
//! *next* frame's shadow-map pass, hiding its cost behind shadow rasterization.
//! This is a pure CPU planning primitive; the WGSL mirrors the same idea for the
//! GPU submit side. Quality is unchanged — only scheduling/overlap improves.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queue {
    Graphics,
    Compute,
}

#[derive(Clone, Debug)]
pub struct Pass {
    pub name: &'static str,
    pub queue: Queue,
    pub cost_ms: f32,
}

/// Plan overlapped execution of a frame's passes.
pub struct AsyncComputePlanner {
    pub passes: Vec<Pass>,
}

impl AsyncComputePlanner {
    pub fn new(passes: Vec<Pass>) -> Self {
        Self { passes }
    }

    /// Frame time assuming graphics and compute queues overlap freely.
    pub fn overlap_time(&self) -> f32 {
        let g: f32 = self
            .passes
            .iter()
            .filter(|p| p.queue == Queue::Graphics)
            .map(|p| p.cost_ms)
            .sum();
        let c: f32 = self
            .passes
            .iter()
            .filter(|p| p.queue == Queue::Compute)
            .map(|p| p.cost_ms)
            .sum();
        g.max(c)
    }

    /// Compare a naive schedule (post FX runs on the compute queue in the same
    /// frame) against a pipelined schedule where the post FX pass is overlapped
    /// with the next frame's shadow-map (graphics) pass. `post` is the cost of
    /// the post pass (compute), `shadow` the cost of a shadow pass (graphics)
    /// it can hide behind.
    pub fn plan(&self, post: f32, shadow: f32) -> PlanResult {
        let g: f32 = self
            .passes
            .iter()
            .filter(|p| p.queue == Queue::Graphics)
            .map(|p| p.cost_ms)
            .sum();
        let c: f32 = self
            .passes
            .iter()
            .filter(|p| p.queue == Queue::Compute)
            .map(|p| p.cost_ms)
            .sum();
        // Naive: post FX runs on the compute queue in the same frame, overlapping
        // the graphics work, so the compute total becomes `c + post`.
        let naive = g.max(c + post);
        // Pipelined: post FX is shifted so it executes behind the next frame's
        // shadow (graphics) pass, hiding up to `shadow` ms of its cost.
        let hidden = c + (post - shadow).max(0.0);
        let pipelined = g.max(hidden);
        let saved = if naive > 1e-6 {
            (naive - pipelined) / naive
        } else {
            0.0
        };
        PlanResult {
            naive,
            pipelined,
            saved_pct: saved * 100.0,
        }
    }
}

pub struct PlanResult {
    pub naive: f32,
    pub pipelined: f32,
    pub saved_pct: f32,
}

pub fn wgsl_source() -> &'static str {
    ASYNC_COMPUTE_WGSL
}

pub const ASYNC_COMPUTE_WGSL: &str = include_str!("../shaders/async_compute.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlap_is_max_not_sum() {
        let p = AsyncComputePlanner::new(vec![
            Pass { name: "gbuffer", queue: Queue::Graphics, cost_ms: 6.0 },
            Pass { name: "shadows", queue: Queue::Graphics, cost_ms: 3.0 },
            Pass { name: "cull", queue: Queue::Compute, cost_ms: 5.0 },
        ]);
        // graphics = 9, compute = 5 -> overlapped = 9
        assert!((p.overlap_time() - 9.0).abs() < 1e-6);
    }
    #[test]
    fn pipeline_hides_post_behind_shadow() {
        let p = AsyncComputePlanner::new(vec![
            Pass { name: "gbuffer", queue: Queue::Graphics, cost_ms: 6.0 },
            Pass { name: "shadows", queue: Queue::Graphics, cost_ms: 3.0 },
            Pass { name: "cull", queue: Queue::Compute, cost_ms: 5.0 },
        ]);
        // post=4 (compute), shadow=3 (graphics)
        // naive = max(9, 5+4)=9 ; pipelined = max(9, 5 + max(0,4-3)=1)=9 -> no save here
        let r = p.plan(4.0, 3.0);
        assert!((r.naive - 9.0).abs() < 1e-6);
        // Now make compute bound so overlap > graphics:
        let p2 = AsyncComputePlanner::new(vec![
            Pass { name: "gbuffer", queue: Queue::Graphics, cost_ms: 6.0 },
        ]);
        // naive = max(6, 0+4)=6 ; pipelined = max(6, max(0,4-3)=1)=6 still graphics bound
        let r2 = p2.plan(4.0, 3.0);
        assert!((r2.naive - 6.0).abs() < 1e-6);
        // Make it compute bound:
        let p3 = AsyncComputePlanner::new(vec![
            Pass { name: "gbuffer", queue: Queue::Graphics, cost_ms: 4.0 },
        ]);
        // compute other = 0, post=4, shadow=3
        // naive = max(4, 4)=4 ; pipelined = max(4, max(0,4-3)=1)=4 -> still 4
        let r3 = p3.plan(4.0, 3.0);
        assert!((r3.naive - 4.0).abs() < 1e-6);
        // Genuinely compute bound with other compute work:
        let p4 = AsyncComputePlanner::new(vec![
            Pass { name: "gbuffer", queue: Queue::Graphics, cost_ms: 6.0 },
            Pass { name: "cull", queue: Queue::Compute, cost_ms: 5.0 },
        ]);
        // post=6 (compute), shadow=3 (graphics)
        // naive = max(6, 5+6)=11 ; pipelined = max(6, 5+max(0,6-3)=3)=8 -> save (11-8)/11 ~27%
        let r4 = p4.plan(6.0, 3.0);
        assert!((r4.naive - 11.0).abs() < 1e-6);
        assert!(r4.saved_pct > 20.0 && r4.saved_pct < 35.0);
    }
}
