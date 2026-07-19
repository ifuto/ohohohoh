//! Hermite spline + quaternion Slerp for 240FPS super-frame interpolation

use crate::packet::CameraSnapshot;
use glam::{Quat, Vec3};

/// Hermite spline interpolation between two camera keyframes
pub struct HermiteInterpolator;

impl HermiteInterpolator {
    /// Cubic Hermite: p(t) = h00*p0 + h10*m0 + h01*p1 + h11*m1
    pub fn interpolate_position(p0: Vec3, p1: Vec3, v0: Vec3, v1: Vec3, t: f32) -> Vec3 {
        let t2 = t * t;
        let t3 = t2 * t;
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        p0 * h00 + v0 * h10 + p1 * h01 + v1 * h11
    }

    pub fn interpolate_camera(a: &CameraSnapshot, b: &CameraSnapshot, t: f32) -> CameraSnapshot {
        let p0 = Vec3::new(a.x as f32, a.y as f32, a.z as f32);
        let p1 = Vec3::new(b.x as f32, b.y as f32, b.z as f32);
        let dt = ((b.timestamp_us - a.timestamp_us) as f32 / 1_000_000.0).max(0.05);
        let v0 = Vec3::new(0.0, 0.0, 0.0); // velocity from packets when available
        let v1 = Vec3::new(0.0, 0.0, 0.0);
        let pos = Self::interpolate_position(p0, p1, v0 * dt, v1 * dt, t);

        let (yaw, pitch) = Self::slerp_euler(a.yaw, a.pitch, b.yaw, b.pitch, t);

        CameraSnapshot {
            timestamp_us: (a.timestamp_us as f64 + (b.timestamp_us - a.timestamp_us) as f64 * t as f64) as u64,
            x: pos.x as f64,
            y: pos.y as f64,
            z: pos.z as f64,
            yaw,
            pitch,
            fov: a.fov + (b.fov - a.fov) * t,
            view_mode: if t < 0.5 { a.view_mode } else { b.view_mode },
            _pad: [0; 3],
        }
    }

    /// Slerp for yaw/pitch via quaternion
    pub fn slerp_euler(yaw0: f32, pitch0: f32, yaw1: f32, pitch1: f32, t: f32) -> (f32, f32) {
        let q0 = Quat::from_euler(glam::EulerRot::YXZ, yaw0.to_radians(), pitch0.to_radians(), 0.0);
        let q1 = Quat::from_euler(glam::EulerRot::YXZ, yaw1.to_radians(), pitch1.to_radians(), 0.0);
        let q = q0.slerp(q1, t);
        let (yaw, pitch, _) = q.to_euler(glam::EulerRot::YXZ);
        (yaw.to_degrees(), pitch.to_degrees())
    }
}

/// Generate super-frames between 20 TPS server ticks
pub struct SuperFrameGenerator {
    pub target_fps: u32,
}

impl Default for SuperFrameGenerator {
    fn default() -> Self {
        Self { target_fps: 240 }
    }
}

impl SuperFrameGenerator {
    pub fn frames_between_ticks(&self) -> u32 {
        self.target_fps / 20
    }

    pub fn generate(&self, keyframes: &[CameraSnapshot]) -> Vec<CameraSnapshot> {
        if keyframes.len() < 2 {
            return keyframes.to_vec();
        }
        let mut out = Vec::new();
        let sub = self.frames_between_ticks();
        for window in keyframes.windows(2) {
            let a = &window[0];
            let b = &window[1];
            for i in 0..sub {
                let t = i as f32 / sub as f32;
                out.push(HermiteInterpolator::interpolate_camera(a, b, t));
            }
        }
        out.push(*keyframes.last().unwrap());
        out
    }
}
