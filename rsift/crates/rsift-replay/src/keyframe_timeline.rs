//! # Keyframe Timeline & Camera Path Studio (`KeyframeTimeline`)
//!
//! ReplayMod 互換および機能強化版カメラパス編集システム。
//! 1) `Catmull-Rom` スプライン (テンション `alpha = 0.5` デフォルト)、3次エルミート、線形補間
//! 2) クォータニオン Slerp (`glam::Quat::slerp`) によるスムーズな視線・ロール角 (`Roll`) 回転
//! 3) 時間キーフレーム (`Time Keyframe TK`) によるゲーム内時間の独立制御 (スロー/早送り/一時停止パン)
//! 4) ブックマーク＆イベントマーカー (`ReplayBookmark`) によるタイムライン注釈

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum InterpolationMode {
    Linear,
    CubicHermite,
    CatmullRom(f32), // alpha tension (0.0 = uniform, 0.5 = centripetal, 1.0 = chordal)
}

impl Default for InterpolationMode {
    fn default() -> Self {
        Self::CatmullRom(0.5)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionKeyframe {
    pub timeline_pos_ms: u64,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub interpolator: InterpolationMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TimeKeyframe {
    pub timeline_pos_ms: u64,
    pub replay_timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayBookmark {
    pub timestamp_ms: u64,
    pub label: String,
    pub color_argb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CameraPathTimeline {
    pub position_keyframes: Vec<PositionKeyframe>,
    pub time_keyframes: Vec<TimeKeyframe>,
    pub bookmarks: Vec<ReplayBookmark>,
    pub default_interpolator: InterpolationMode,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct EvaluatedCameraState {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub replay_timestamp_ms: u64,
}

impl CameraPathTimeline {
    pub fn new() -> Self {
        Self {
            position_keyframes: Vec::new(),
            time_keyframes: Vec::new(),
            bookmarks: Vec::new(),
            default_interpolator: InterpolationMode::CatmullRom(0.5),
            duration_ms: 10000,
        }
    }

    pub fn add_position_keyframe(
        &mut self,
        timeline_ms: u64,
        pos: [f32; 3],
        yaw: f32,
        pitch: f32,
        roll: f32,
    ) -> usize {
        let kf = PositionKeyframe {
            timeline_pos_ms: timeline_ms,
            pos,
            yaw,
            pitch,
            roll,
            interpolator: self.default_interpolator,
        };
        self.position_keyframes.push(kf);
        self.position_keyframes.sort_by_key(|k| k.timeline_pos_ms);
        if timeline_ms > self.duration_ms {
            self.duration_ms = timeline_ms;
        }
        self.position_keyframes
            .iter()
            .position(|k| k.timeline_pos_ms == timeline_ms)
            .unwrap_or(0)
    }

    pub fn add_time_keyframe(&mut self, timeline_ms: u64, replay_timestamp_ms: u64) -> usize {
        let kf = TimeKeyframe {
            timeline_pos_ms: timeline_ms,
            replay_timestamp_ms,
        };
        self.time_keyframes.push(kf);
        self.time_keyframes.sort_by_key(|k| k.timeline_pos_ms);
        self.time_keyframes
            .iter()
            .position(|k| k.timeline_pos_ms == timeline_ms)
            .unwrap_or(0)
    }

    pub fn add_bookmark(&mut self, timestamp_ms: u64, label: impl Into<String>, color_argb: u32) {
        self.bookmarks.push(ReplayBookmark {
            timestamp_ms,
            label: label.into(),
            color_argb,
        });
        self.bookmarks.sort_by_key(|b| b.timestamp_ms);
    }

    /// Evaluate camera state and replay time at a specific timeline scrubber position (`t_ms`).
    pub fn evaluate(&self, t_ms: u64) -> Option<EvaluatedCameraState> {
        if self.position_keyframes.is_empty() {
            return None;
        }

        // Evaluate position & rotation
        let (pos, yaw, pitch, roll) = self.eval_position_and_rotation(t_ms);
        let replay_timestamp_ms = self.eval_replay_time(t_ms);

        Some(EvaluatedCameraState {
            pos,
            yaw,
            pitch,
            roll,
            replay_timestamp_ms,
        })
    }

    fn eval_replay_time(&self, t_ms: u64) -> u64 {
        if self.time_keyframes.is_empty() {
            return t_ms;
        }
        if t_ms <= self.time_keyframes.first().unwrap().timeline_pos_ms {
            return self.time_keyframes.first().unwrap().replay_timestamp_ms;
        }
        if t_ms >= self.time_keyframes.last().unwrap().timeline_pos_ms {
            return self.time_keyframes.last().unwrap().replay_timestamp_ms;
        }

        for window in self.time_keyframes.windows(2) {
            let k0 = &window[0];
            let k1 = &window[1];
            if t_ms >= k0.timeline_pos_ms && t_ms <= k1.timeline_pos_ms {
                let dt = (k1.timeline_pos_ms - k0.timeline_pos_ms) as f64;
                if dt <= 0.0 {
                    return k0.replay_timestamp_ms;
                }
                let factor = (t_ms - k0.timeline_pos_ms) as f64 / dt;
                return (k0.replay_timestamp_ms as f64
                    + (k1.replay_timestamp_ms as f64 - k0.replay_timestamp_ms as f64) * factor)
                    as u64;
            }
        }
        t_ms
    }

    fn eval_position_and_rotation(&self, t_ms: u64) -> (Vec3, f32, f32, f32) {
        let n = self.position_keyframes.len();
        if n == 1 || t_ms <= self.position_keyframes[0].timeline_pos_ms {
            let k = &self.position_keyframes[0];
            return (Vec3::from_array(k.pos), k.yaw, k.pitch, k.roll);
        }
        if t_ms >= self.position_keyframes[n - 1].timeline_pos_ms {
            let k = &self.position_keyframes[n - 1];
            return (Vec3::from_array(k.pos), k.yaw, k.pitch, k.roll);
        }

        // Find keyframe segment `[i, i+1]`
        let mut idx = 0;
        while idx + 1 < n && self.position_keyframes[idx + 1].timeline_pos_ms < t_ms {
            idx += 1;
        }

        let k1 = &self.position_keyframes[idx];
        let k2 = &self.position_keyframes[idx + 1];
        let span = (k2.timeline_pos_ms - k1.timeline_pos_ms) as f32;
        let t = if span > 0.0 {
            (t_ms - k1.timeline_pos_ms) as f32 / span
        } else {
            0.0
        };

        // Interpolate Rotation via Quaternion Slerp
        let (yaw, pitch, roll) = slerp_rotation_euler(k1.yaw, k1.pitch, k1.roll, k2.yaw, k2.pitch, k2.roll, t);

        // Interpolate Position based on selected segment interpolator
        let pos = match k1.interpolator {
            InterpolationMode::Linear => {
                let p1 = Vec3::from_array(k1.pos);
                let p2 = Vec3::from_array(k2.pos);
                p1.lerp(p2, t)
            }
            InterpolationMode::CatmullRom(alpha) => {
                let k0 = if idx > 0 { &self.position_keyframes[idx - 1] } else { k1 };
                let k3 = if idx + 2 < n { &self.position_keyframes[idx + 2] } else { k2 };
                catmull_rom_point(
                    Vec3::from_array(k0.pos),
                    Vec3::from_array(k1.pos),
                    Vec3::from_array(k2.pos),
                    Vec3::from_array(k3.pos),
                    t,
                    alpha,
                )
            }
            InterpolationMode::CubicHermite => {
                let p1 = Vec3::from_array(k1.pos);
                let p2 = Vec3::from_array(k2.pos);
                // Hermite smooth step
                let t2 = t * t;
                let t3 = t2 * t;
                let h = 3.0 * t2 - 2.0 * t3;
                p1.lerp(p2, h)
            }
        };

        (pos, yaw, pitch, roll)
    }

    /// Generate 3D world points for Path Preview rendering (`H` key overlay).
    pub fn generate_preview_points(&self, samples_per_segment: usize) -> Vec<[f32; 3]> {
        if self.position_keyframes.len() < 2 {
            return self.position_keyframes.iter().map(|k| k.pos).collect();
        }
        let mut points = Vec::new();
        for window in self.position_keyframes.windows(2) {
            let k1 = &window[0];
            let k2 = &window[1];
            let span = k2.timeline_pos_ms - k1.timeline_pos_ms;
            for s in 0..samples_per_segment {
                let factor = s as f32 / samples_per_segment as f32;
                let t_ms = k1.timeline_pos_ms + (span as f32 * factor) as u64;
                if let Some(state) = self.evaluate(t_ms) {
                    points.push(state.pos.to_array());
                }
            }
        }
        if let Some(last) = self.position_keyframes.last() {
            points.push(last.pos);
        }
        points
    }
}

#[inline]
pub fn slerp_rotation_euler(y0: f32, p0: f32, r0: f32, y1: f32, p1: f32, r1: f32, t: f32) -> (f32, f32, f32) {
    let q0 = Quat::from_euler(glam::EulerRot::YXZ, y0.to_radians(), p0.to_radians(), r0.to_radians());
    let q1 = Quat::from_euler(glam::EulerRot::YXZ, y1.to_radians(), p1.to_radians(), r1.to_radians());
    let q = q0.slerp(q1, t);
    let (y, p, r) = q.to_euler(glam::EulerRot::YXZ);
    (y.to_degrees(), p.to_degrees(), r.to_degrees())
}

#[inline]
pub fn catmull_rom_point(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32, alpha: f32) -> Vec3 {
    if alpha.abs() < 1e-4 {
        // Uniform Catmull-Rom
        let t2 = t * t;
        let t3 = t2 * t;
        return 0.5 * ((2.0 * p1)
            + (-p0 + p2) * t
            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
            + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);
    }

    // Centripetal / alpha-weighted Catmull-Rom
    let get_t = |t_val: f32, a: Vec3, b: Vec3| -> f32 {
        let d = a.distance_squared(b);
        if d < 1e-6 {
            t_val + 1e-3
        } else {
            t_val + d.powf(alpha * 0.5)
        }
    };

    let t0 = 0.0f32;
    let t1 = get_t(t0, p0, p1);
    let t2 = get_t(t1, p1, p2);
    let t3 = get_t(t2, p2, p3);

    let span = t2 - t1;
    let u = t1 + t * span;

    let a1 = (t1 - u) / (t1 - t0) * p0 + (u - t0) / (t1 - t0) * p1;
    let a2 = (t2 - u) / (t2 - t1) * p1 + (u - t1) / (t2 - t1) * p2;
    let a3 = (t3 - u) / (t3 - t2) * p2 + (u - t2) / (t3 - t2) * p3;

    let b1 = (t2 - u) / (t2 - t0) * a1 + (u - t0) / (t2 - t0) * a2;
    let b2 = (t3 - u) / (t3 - t1) * a2 + (u - t1) / (t3 - t1) * a3;

    (t2 - u) / (t2 - t1) * b1 + (u - t1) / (t2 - t1) * b2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_camera_timeline_evaluation() {
        let mut timeline = CameraPathTimeline::new();
        timeline.add_position_keyframe(0, [0.0, 64.0, 0.0], 0.0, 0.0, 0.0);
        timeline.add_position_keyframe(1000, [100.0, 70.0, 100.0], 90.0, 15.0, 5.0);
        timeline.add_time_keyframe(0, 0);
        timeline.add_time_keyframe(1000, 5000); // 5x timelapse

        let mid = timeline.evaluate(500).unwrap();
        assert!((mid.pos.x - 50.0).abs() < 10.0);
        assert_eq!(mid.replay_timestamp_ms, 2500);
    }
}
