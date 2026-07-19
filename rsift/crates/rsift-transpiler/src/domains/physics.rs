//! Entity physics — only bodies ingested from WorldMirror.

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PhysicsBody {
    pub entity_id: i32,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub on_ground: bool,
}

pub struct PhysicsDomain {
    bodies: Vec<PhysicsBody>,
}

impl PhysicsDomain {
    pub fn empty() -> Self {
        Self { bodies: Vec::new() }
    }

    pub fn new(_count: usize) -> Self {
        Self::empty()
    }

    pub fn clear(&mut self) {
        self.bodies.clear();
    }

    pub fn ingest_from_mirror(&mut self, entities: &[crate::world_mirror::JvmEntityState]) {
        self.bodies.clear();
        self.bodies.reserve(entities.len());
        for e in entities {
            if e.removed != 0 {
                continue;
            }
            self.bodies.push(PhysicsBody {
                entity_id: e.entity_id,
                pos: [e.pos_x, e.pos_y, e.pos_z],
                vel: [e.vel_x, e.vel_y, e.vel_z],
                on_ground: e.on_ground != 0,
            });
        }
    }

    pub fn write_back(&self, entities: &mut [crate::world_mirror::JvmEntityState]) {
        for src in &self.bodies {
            if let Some(dst) = entities.iter_mut().find(|e| e.entity_id == src.entity_id) {
                dst.pos_x = src.pos[0];
                dst.pos_y = src.pos[1];
                dst.pos_z = src.pos[2];
                dst.vel_x = src.vel[0];
                dst.vel_y = src.vel[1];
                dst.vel_z = src.vel[2];
                dst.on_ground = if src.on_ground { 1 } else { 0 };
            }
        }
    }

    #[inline]
    fn integrate(b: &mut PhysicsBody, dt: f64) {
        const GRAVITY: f64 = -0.08;
        if !b.on_ground {
            b.vel[1] += GRAVITY * dt;
        }
        for axis in 0..3 {
            b.pos[axis] += b.vel[axis] * dt;
        }
        if b.pos[1] <= -64.0 {
            b.pos[1] = -64.0;
            b.vel[1] = 0.0;
            b.on_ground = true;
        }
        b.vel[0] *= 0.98;
        b.vel[2] *= 0.98;
    }

    pub fn tick(&mut self, parallel: bool, dt: f64, stats: &AtomicU64) -> u64 {
        let n = self.bodies.len();
        if n == 0 {
            stats.store(0, Ordering::Relaxed);
            return 0;
        }
        if parallel && n >= 64 {
            self.bodies
                .par_iter_mut()
                .for_each(|b| Self::integrate(b, dt));
        } else {
            for b in &mut self.bodies {
                Self::integrate(b, dt);
            }
        }
        stats.store(n as u64, Ordering::Relaxed);
        n as u64
    }
}
