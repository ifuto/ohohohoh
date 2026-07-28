//! Projected (deferred) decals for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): transform a world position into a decal's local box
//! and compute a soft edge fade. Decals add detail (bullets, scorch marks)
//! without extra geometry — purely a quality feature, free on integrated GPUs.
//!
//! 【wave 165 FK (2026-07-28)】消費者: `FullGraphWiring` のデカール投影
//! 計測 (`decal_local` → report.decals_projected、EB-3 公知の junction
//! 設計)、`wgsl_source` = gpu_runtime 登録。捕捉 97 [小]: half.x/half.y
//! == 0.0 の除算チャネルが |0|<=0 ゲート通過後 0/0=NaN を Some で返し得た
//! 潜入口を、非物理 extent の事前拒否へ根治 (負 extent はゲート不成立で
//! 自然に None、half.z は z 生値返却で除算を持たず無害 — 後者は仕様
//! として明示)。捕捉 98 [小]: §7 消化 27 で `let _ = decal_local(...)`
//! 評価破棄を計測配線へ閉塞 + 消費者完全ゼロの `Vec4` (型+演算 3) と
//! `Vec3::{Add, Mul<f32>}` を機械 grep 証明で不可能証明削除 (EJ-2/FF/
//! FJ 判例)。`soft_edge` は decal_local と対をなす正当アルゴリズムで、
//! 自家 strict (端点 fade=0/中心 fade=1) と wgsl 語彙で消費証跡を保持
//! (junction 登録時の実消費に接続する保持判定、directive⑦)。

use std::ops::Sub;

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
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

/// A decal box oriented by an orthonormal-ish basis.
pub struct Decal {
    pub center: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub forward: Vec3, // points out of the projection face
    pub half: Vec3,    // half extents along right/up/forward
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a).max(1e-4)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Returns normalized local coords [-1,1]² in xy (and z in [-half.z,0]) if the
/// world point projects inside the decal, else `None`.
pub fn decal_local(world_pos: Vec3, d: &Decal) -> Option<Vec3> {
    let rel = world_pos - d.center;
    let x = rel.dot(d.right);
    let y = rel.dot(d.up);
    let z = rel.dot(d.forward);
    // 【wave 165 FK 捕捉 97】half.x/half.y == 0.0 は |x|<=0 ゲートを
    // x==0 ちょうどで通し `0.0/0.0` = NaN を Some として返し得た潜入口
    // (rq fk_decals (2))。非物理の除算チャネルは計算前に拒否する
    // (half.z は z を生値返却で除算しないため 0.0 でも正当、負 extent は
    // ゲート不成立で自然に None)。
    if d.half.x == 0.0 || d.half.y == 0.0 {
        return None;
    }
    if x.abs() <= d.half.x && y.abs() <= d.half.y && z <= 0.0 && z >= -d.half.z {
        Some(Vec3::new(x / d.half.x, y / d.half.y, z))
    } else {
        None
    }
}

/// Soft edge fade in [0,1] from world-space `off` relative to the decal box.
pub fn soft_edge(off: Vec3, half: Vec3, edge: Vec3) -> f32 {
    let fx = 1.0 - smoothstep(half.x - edge.x, half.x, off.x.abs());
    let fy = 1.0 - smoothstep(half.y - edge.y, half.y, off.y.abs());
    let fz = 1.0 - smoothstep(0.0, edge.z.max(1e-4), (-off.z).max(0.0));
    (fx * fy * fz).clamp(0.0, 1.0)
}

pub fn wgsl_source() -> &'static str {
    DECALS_WGSL
}

pub const DECALS_WGSL: &str = include_str!("../shaders/decals.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    fn box_decal() -> Decal {
        Decal {
            center: Vec3::new(0.0, 0.0, 0.0),
            right: Vec3::new(1.0, 0.0, 0.0),
            up: Vec3::new(0.0, 1.0, 0.0),
            forward: Vec3::new(0.0, 0.0, 1.0),
            half: Vec3::new(2.0, 2.0, 2.0),
        }
    }
    #[test]
    fn center_is_inside() {
        let l = decal_local(Vec3::new(0.0, 0.0, 0.0), &box_decal());
        assert!(l.is_some());
        let l = l.unwrap();
        assert!((l.x).abs() < 1e-6 && (l.y).abs() < 1e-6);
    }
    #[test]
    fn outside_is_none() {
        let d = box_decal();
        let l = decal_local(Vec3::new(3.0, 0.0, 0.0), &d);
        assert!(l.is_none());
    }
    // ==================== wave 165 (FK) strict ====================

    /// 捕捉 97 [小]: zero half 除算チャネルの拒否。旧実装は |0|<=0 ゲート通過
    /// 後に `0.0/0.0` = NaN を `Some` として返し得た (rq fk_decals (2))。
    /// 非物理 extent は内部点不在 (None) として拒否する契約 (負 extent は
    /// ゲート不成立で自然に None、half.z は除算を持たず無害 — いずれも rq・
    /// 実装双方で機械確認)。
    #[test]
    fn fk_zero_half_extent_is_rejected_not_nan() {
        let mut d = box_decal();
        d.half = Vec3::new(0.0, 2.0, 2.0);
        assert!(
            decal_local(Vec3::new(0.0, 0.0, 0.0), &d).is_none(),
            "half.x==0.0 は NaN ではなく None 拒否"
        );
        let mut d2 = box_decal();
        d2.half = Vec3::new(2.0, 0.0, 2.0);
        assert!(decal_local(Vec3::new(0.0, 0.0, 0.0), &d2).is_none());
        // half.z は除算チャネルを持たないため 0.0 でも正当 (内部判定に立つ)
        let mut d3 = box_decal();
        d3.half = Vec3::new(2.0, 2.0, 0.0);
        assert!(
            decal_local(Vec3::new(0.0, 0.0, 0.0), &d3).is_some(),
            "half.z=0 は z==0 の点のみ内部 (除算なし)"
        );
    }

    /// 境界等号の厳密 pin: |x|==half.x ・ z==-half.z は内部 (rq (1))、
    /// x/half.x は 2.0/2.0=1.0 bit 厳密。直外は None。
    #[test]
    fn fk_edge_inclusive_bit_golden() {
        let d = box_decal();
        let l = decal_local(Vec3::new(2.0, 0.0, 0.0), &d).expect("境界等号は内部");
        assert_eq!(l.x.to_bits(), 0x3f800000, "x/half.x == 1.0 exact");
        assert!(
            decal_local(Vec3::new(2.0, 0.0, -2.0), &d).is_some(),
            "z==-half.z 内部"
        );
        assert!(
            decal_local(Vec3::new(0.0, 0.0, -2.0000005), &d).is_none(),
            "z 直外は None"
        );
        assert!(
            decal_local(Vec3::new(2.0000005, 0.0, 0.0), &d).is_none(),
            "x 直外は None"
        );
    }

    #[test]
    fn edge_fades_to_zero() {
        let d = box_decal();
        let fade = soft_edge(Vec3::new(2.0, 0.0, 0.0), d.half, Vec3::new(0.5, 0.5, 0.5));
        assert!((fade - 0.0).abs() < 1e-6, "fade = {}", fade);
    }
    #[test]
    fn center_is_full() {
        let d = box_decal();
        let fade = soft_edge(Vec3::new(0.0, 0.0, 0.0), d.half, Vec3::new(0.5, 0.5, 0.5));
        assert!((fade - 1.0).abs() < 1e-6, "fade = {}", fade);
    }
}
