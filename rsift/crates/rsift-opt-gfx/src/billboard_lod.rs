//! Billboard LOD for distant flora/trees (Tier 4).

#[derive(Debug, Clone, Copy)]
pub struct BillboardVert {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloraLod {
    FullMesh,
    CrossedPlanes,
    Billboard,
    Culled,
}

#[derive(Debug, Clone)]
pub struct BillboardLodSelector {
    pub full_mesh_dist: f32,
    pub crossed_dist: f32,
    pub billboard_dist: f32,
}

impl Default for BillboardLodSelector {
    fn default() -> Self {
        Self {
            full_mesh_dist: 48.0,
            crossed_dist: 96.0,
            billboard_dist: 192.0,
        }
    }
}

impl BillboardLodSelector {
    /// `scale` は有限必須 (NaN は clamp を素通りし全帯 NaN → select 側の
    /// 契約 assert に到る前の入口で拒否。±∞ は clamp が責任を持ち受理)。
    pub fn for_tier_scale(scale: f32) -> Self {
        assert!(
            !scale.is_nan(),
            "for_tier_scale 契約違反: scale は NaN 不可"
        );
        let s = scale.clamp(0.5, 1.5);
        Self {
            full_mesh_dist: 48.0 * s,
            crossed_dist: 96.0 * s,
            billboard_dist: 192.0 * s,
        }
    }

    /// **契約 (2026-07-22 wave 36 明文化)**:
    /// - `dist` は NaN 不可 (旧実装は全比較不成立で **Culled に静寂着地**、
    ///   植物が描画なく蒸発した — entity_tick_lod wave 34 と同型の根治)。
    ///   ±∞ dist は Culled として意味が通るため受理。
    /// - 帯境界値は単調かつ有限必須 (pub フィールドの非単調改変を検出)。
    pub fn select(&self, dist: f32) -> FloraLod {
        assert!(
            !dist.is_nan(),
            "select 契約違反: dist は NaN 不可 (dist={dist})"
        );
        assert!(
            self.full_mesh_dist.is_finite()
                && self.billboard_dist.is_finite()
                && self.full_mesh_dist <= self.crossed_dist
                && self.crossed_dist <= self.billboard_dist,
            "select 契約違反: 帯境界は単調かつ有限必須 (full={}, crossed={}, billboard={})",
            self.full_mesh_dist,
            self.crossed_dist,
            self.billboard_dist
        );
        if dist < self.full_mesh_dist {
            FloraLod::FullMesh
        } else if dist < self.crossed_dist {
            FloraLod::CrossedPlanes
        } else if dist < self.billboard_dist {
            FloraLod::Billboard
        } else {
            FloraLod::Culled
        }
    }

    /// Camera-facing quad centered at `center` with half-extents `half_w` / `half_h`.
    ///
    /// **契約**: `cam_right` / `cam_up` は**単位直交ベクトル必須**
    /// (view 基底想定)。非単位を渡すと quad は伸縮・せん断し、
    /// 非直交を渡すと平行四辺形化する (本関数は正規化しない — ホットパス
    /// 優先で呼び出し側の責務とする。2026-07-22 wave 36 明文化)。
    /// 頂点順・uv の写像はテスト `billboard_exact_corners_and_uvs` が機械ピン。
    pub fn make_billboard(
        center: [f32; 3],
        half_w: f32,
        half_h: f32,
        cam_right: [f32; 3],
        cam_up: [f32; 3],
    ) -> [BillboardVert; 4] {
        let r = cam_right;
        let u = cam_up;
        let corners = [
            [-half_w, -half_h],
            [half_w, -half_h],
            [half_w, half_h],
            [-half_w, half_h],
        ];
        let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let mut out = [BillboardVert {
            pos: [0.0; 3],
            uv: [0.0; 2],
        }; 4];
        for i in 0..4 {
            let px = center[0] + r[0] * corners[i][0] + u[0] * corners[i][1];
            let py = center[1] + r[1] * corners[i][0] + u[1] * corners[i][1];
            let pz = center[2] + r[2] * corners[i][0] + u[2] * corners[i][1];
            out[i] = BillboardVert {
                pos: [px, py, pz],
                uv: uvs[i],
            };
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_bands() {
        let s = BillboardLodSelector::default();
        assert_eq!(s.select(10.0), FloraLod::FullMesh);
        assert_eq!(s.select(200.0), FloraLod::Culled);
        let v = BillboardLodSelector::make_billboard(
            [0.0, 1.0, 0.0],
            0.5,
            1.0,
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        );
        assert!((v[0].pos[0] + 0.5).abs() < 1e-5);
    }

    /// wave 36-1: 帯境界の厳密列 (等号は次帯へ) + tier scale の clamp 両端。
    #[test]
    fn band_boundaries_exact_with_tier_clamps() {
        let s = BillboardLodSelector::default();
        let cases: &[(f32, FloraLod)] = &[
            (0.0, FloraLod::FullMesh),
            (47.0, FloraLod::FullMesh),
            (48.0, FloraLod::CrossedPlanes), // 等号 → 次帯
            (95.0, FloraLod::CrossedPlanes),
            (96.0, FloraLod::Billboard),
            (191.0, FloraLod::Billboard),
            (192.0, FloraLod::Culled),
            (f32::INFINITY, FloraLod::Culled), // ±∞ は意味通り受理
        ];
        for (dist, want) in cases {
            assert_eq!(s.select(*dist), *want, "dist={dist}");
        }
        // tier scale clamp: 0.25 → 0.5 (下限)、9.0 → 1.5 (上限) で 24/48/96 と 72/144/288
        let lo = BillboardLodSelector::for_tier_scale(0.25);
        assert_eq!(
            (lo.full_mesh_dist, lo.crossed_dist, lo.billboard_dist),
            (24.0, 48.0, 96.0)
        );
        let hi = BillboardLodSelector::for_tier_scale(9.0);
        assert_eq!(
            (hi.full_mesh_dist, hi.crossed_dist, hi.billboard_dist),
            (72.0, 144.0, 288.0)
        );
        assert_eq!(lo.select(24.0), FloraLod::CrossedPlanes);
        assert_eq!(hi.select(72.0), FloraLod::CrossedPlanes);
        assert_eq!(
            BillboardLodSelector::for_tier_scale(f32::INFINITY).full_mesh_dist,
            72.0
        );
    }

    /// wave 36-2: make_billboard の厳密 corner/uv 列 (単位基底、全成分 f32 正確)。
    #[test]
    fn billboard_exact_corners_and_uvs() {
        let v = BillboardLodSelector::make_billboard(
            [10.0, 20.0, 30.0],
            0.5,
            1.0,
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        );
        let want_pos = [
            [9.5, 19.0, 30.0],
            [10.5, 19.0, 30.0],
            [10.5, 21.0, 30.0],
            [9.5, 21.0, 30.0],
        ];
        let want_uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for i in 0..4 {
            assert_eq!(v[i].pos, want_pos[i], "corner {i}");
            assert_eq!(v[i].uv, want_uv[i], "uv {i}");
        }
        // 斜め基底: r=(0,0,1), u=(0,1,0) でも厳密に同式
        let w = BillboardLodSelector::make_billboard(
            [10.0, 20.0, 30.0],
            0.5,
            1.0,
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0],
        );
        assert_eq!(w[2].pos, [10.0, 21.0, 30.5]);
        assert_eq!(w[0].pos, [10.0, 19.0, 29.5]);
    }

    /// wave 36-3: NaN / 非単調帯は fail-loud。
    #[test]
    #[should_panic(expected = "select 契約違反: dist は NaN 不可")]
    fn select_rejects_nan_dist() {
        BillboardLodSelector::default().select(f32::NAN);
    }

    #[test]
    #[should_panic(expected = "select 契約違反: 帯境界は単調かつ有限必須")]
    fn select_rejects_non_monotonic_bands() {
        let mut s = BillboardLodSelector::default();
        s.crossed_dist = 10.0; // full(48) > crossed(10) — 非単調
        let _ = s.select(30.0);
    }

    #[test]
    #[should_panic(expected = "for_tier_scale 契約違反")]
    fn for_tier_scale_rejects_nan() {
        let _ = BillboardLodSelector::for_tier_scale(f32::NAN);
    }
}
