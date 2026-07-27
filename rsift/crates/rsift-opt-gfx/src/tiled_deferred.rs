//! # 39. Deferred Rendering / Tiled Deferred (`TiledDeferredLighting`)
//!
//! ジオメトリパスで G バッファ（位置・法線・アルベド・粗さ等）のみ出力し、ライティングパスで
//! 画面を 16x16 ピクセルのタイルに分割して、各タイルに影響する光源リストを Compute/CPU で
//! 事前カリングする。光源数に比例する Forward 描画の負荷を排除し、100 個以上の動的・固定光源
//! でも 60 FPS を安定維持。

pub const TILE_SIZE_PIXELS: usize = 16;
pub const MAX_LIGHTS_PER_TILE: usize = 64;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PointLight {
    pub pos: [f32; 3],
    pub radius: f32,
    pub color_rgb: [f32; 3],
    pub intensity: f32,
}

#[derive(Debug, Clone)]
pub struct LightTile {
    pub light_indices: Vec<u32>,
}

pub struct TiledDeferredLighting {
    pub screen_width: usize,
    pub screen_height: usize,
    pub tiles_x: usize,
    pub tiles_y: usize,
    pub tiles: Vec<LightTile>,
    pub lights: Vec<PointLight>,
}

impl TiledDeferredLighting {
    pub fn new(screen_width: usize, screen_height: usize) -> Self {
        let tiles_x = (screen_width + TILE_SIZE_PIXELS - 1) / TILE_SIZE_PIXELS;
        let tiles_y = (screen_height + TILE_SIZE_PIXELS - 1) / TILE_SIZE_PIXELS;
        let total_tiles = tiles_x * tiles_y;
        Self {
            screen_width,
            screen_height,
            tiles_x,
            tiles_y,
            tiles: vec![LightTile { light_indices: Vec::with_capacity(16) }; total_tiles],
            lights: Vec::with_capacity(256),
        }
    }

    pub fn clear_lights(&mut self) {
        self.lights.clear();
    }

    pub fn add_light(&mut self, light: PointLight) -> u32 {
        let id = self.lights.len() as u32;
        self.lights.push(light);
        id
    }

    /// Cull lights against each screen tile bounding frustum/sphere (`O(Tiles * Lights)` or hierarchical).
    ///
    /// **契約 (wave 143 EQ)**: `view_proj` は本番規約の**行ベクトル p×M**
    /// (clip_j = Σ_i p_i·M[i][j]、平行移動は row 3 — full_graph_wiring の
    /// `extract_frustum_planes` 記述と同一規約) で全成分有限必須。
    /// `light.pos` 全成分と `radius` は有限必須・半径 ≥ 0。違反は panic
    /// (fail-loud、旧来は NaN pos が `as i32`=0 飽和でタイル (0,0) へ
    /// 静寂割当 = 照明の局地破壊だった — EQ-3 根治)。
    ///
    /// 誠実注記 (EQ-2): (a) `ww <= 0.1` でカメラ背後・超近接光源を**完全
    /// 除外** (巨大半径光源が近平面を跨ぐ場合の視野内影響を見逃す近似);
    /// (b) screen_radius は球投影の円錐近似 `(radius/ww)·width·0.5`
    /// (radius ≪ 距離で正確); (c) MAX_LIGHTS_PER_TILE=64 超過は静寂切捨て
    /// (ホットスポットの照度欠損上限、pin 済); (d) 本実装は ndc_z 非使用
    /// (深度範囲カリングなし = タイル列は深度全域に光源を含む保守形)。
    pub fn cull_lights_for_tiles(&mut self, view_proj: &[[f32; 4]; 4]) {
        // EQ-3: 契約 fail-loud (診断コストは lights 数に線形、cull 本体と同次数)。
        assert!(
            view_proj.iter().flatten().all(|c| c.is_finite()),
            "TiledDeferredLighting::cull_lights_for_tiles 契約違反: view_proj が非有限"
        );
        for (i, l) in self.lights.iter().enumerate() {
            assert!(
                l.pos.iter().all(|c| c.is_finite()) && l.radius.is_finite(),
                "TiledDeferredLighting::cull_lights_for_tiles 契約違反: lights[{i}] が非有限 ({l:?})"
            );
            assert!(
                l.radius >= 0.0,
                "TiledDeferredLighting::cull_lights_for_tiles 契約違反: lights[{i}].radius が負 ({})",
                l.radius
            );
        }
        for tile in &mut self.tiles {
            tile.light_indices.clear();
        }

        let m = view_proj;
        for (light_idx, light) in self.lights.iter().enumerate() {
            // 捕捉 58: **行ベクトル p×M** で読む (clip_j = Σ_i p_i·M[i][j])。
            // 旧実装は列ベクトル M·p の行内積で転置読み = 平行移動 (row 3)
            // を無視し w を意味破壊 (CG-1 同型、IDENTITY_VP 対称で潜伏、
            // T 平行移動 strict が修正前 RED で実証済)。
            let x =
                light.pos[0] * m[0][0] + light.pos[1] * m[1][0] + light.pos[2] * m[2][0] + m[3][0];
            let y =
                light.pos[0] * m[0][1] + light.pos[1] * m[1][1] + light.pos[2] * m[2][1] + m[3][1];
            let ww =
                light.pos[0] * m[0][3] + light.pos[1] * m[1][3] + light.pos[2] * m[2][3] + m[3][3];

            if ww <= 0.1 {
                continue;
            }
            let ndc_x = x / ww;
            let ndc_y = y / ww;
            let screen_x = ((ndc_x * 0.5 + 0.5) * self.screen_width as f32) as i32;
            let screen_y = ((1.0 - (ndc_y * 0.5 + 0.5)) * self.screen_height as f32) as i32;
            let screen_radius = ((light.radius / ww) * self.screen_width as f32 * 0.5) as i32;

            let min_tx = ((screen_x - screen_radius).max(0) as usize) / TILE_SIZE_PIXELS;
            let max_tx = (((screen_x + screen_radius).max(0) as usize) / TILE_SIZE_PIXELS).min(self.tiles_x.saturating_sub(1));
            let min_ty = ((screen_y - screen_radius).max(0) as usize) / TILE_SIZE_PIXELS;
            let max_ty = (((screen_y + screen_radius).max(0) as usize) / TILE_SIZE_PIXELS).min(self.tiles_y.saturating_sub(1));

            for ty in min_ty..=max_ty {
                for tx in min_tx..=max_tx {
                    let tile_idx = ty * self.tiles_x + tx;
                    if self.tiles[tile_idx].light_indices.len() < MAX_LIGHTS_PER_TILE {
                        self.tiles[tile_idx].light_indices.push(light_idx as u32);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tiled_deferred_light_culling() {
        let mut tdl = TiledDeferredLighting::new(1280, 720);
        tdl.add_light(PointLight {
            pos: [0.0, 64.0, 0.0],
            radius: 10.0,
            color_rgb: [1.0, 0.8, 0.5],
            intensity: 2.0,
        });
        let vp = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0, 10.0],
        ];
        tdl.cull_lights_for_tiles(&vp);
        assert!(!tdl.lights.is_empty());
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    fn light(pos: [f32; 3], radius: f32) -> PointLight {
        PointLight {
            pos,
            radius,
            color_rgb: [1.0, 1.0, 1.0],
            intensity: 1.0,
        }
    }
    const ID: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];

    /// 捕捉 58 pin: view_proj は本番規約の**行ベクトル p×M** (clip_j =
    /// Σ_i p_i·M[i][j]、平行移動は row 3)。旧実装は列ベクトル M·p で
    /// 転置読みしており、平行移動を無視 (CG-1 同型、IDVP 対称で潜伏)。
    /// T=(0.5,0,0) + light [0,0,1]: 正解は clip_x=0.5 → sx=1440 (f32
    /// exact) → min_tx=480/16=30、max_tx=min(150,119)=119
    /// (rq eq_tiled_b 導出)。旧転置読みは min_tx=0 → tiles[29] 差分。
    #[test]
    fn cull_px_m_convention_translation_pin() {
        let mut t = TiledDeferredLighting::new(1920, 1080);
        t.add_light(light([0.0, 0.0, 1.0], 1.0));
        let translate = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.5, 0.0, 0.0, 1.0],
        ];
        t.cull_lights_for_tiles(&translate);
        assert_eq!(
            t.tiles[30].light_indices.len(),
            1,
            "p×M: x+=0.5 → sx=1440 → タイル 30..119 (rq 導出)"
        );
        assert_eq!(
            t.tiles[29].light_indices.len(),
            0,
            "p×M: タイル 0..29 は空 (旧転置読みでは min_tx=0 でここにも割当 = 修正前 RED 差分)"
        );
    }

    /// 画面外光源の排除 pin: T=(10,0,0) で clip_x=10 → sx=10560 →
    /// min_tx=600 > max_tx=119 の**逆転空ループ** (min 側は tiles_x で
    /// clamp されない構造) → 影響球左端が画面右端の遥か外 = 物理的に
    /// 正しい完全除外。cull 境界の誠実固定 (rq eq_tiled 導出)。
    #[test]
    fn cull_offscreen_translation_excluded_pin() {
        let mut t = TiledDeferredLighting::new(1920, 1080);
        t.add_light(light([0.0, 0.0, 1.0], 1.0));
        let translate = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [10.0, 0.0, 0.0, 1.0],
        ];
        t.cull_lights_for_tiles(&translate);
        assert!(
            t.tiles.iter().all(|tl| tl.light_indices.is_empty()),
            "sx=10560 → min_tx=600 > 119 → 画面外完全除外 (rq 導出)"
        );
    }

    /// 捕捉 58 pin (消失経路): T=(0,0,-50) で旧転置読みは ww=row3·p =
    /// -50·1+1=-49 ≤ 0.1 → 光源**消失**。p×M 規約では w=1 不変 (z のみ
    /// -49、本実装は ndc_z 非使用) → 全タイル割当が維持される。
    #[test]
    fn cull_px_m_z_translation_no_vanish_pin() {
        let mut t = TiledDeferredLighting::new(1920, 1080);
        t.add_light(light([0.0, 0.0, 1.0], 1.0));
        let translate = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, -50.0, 1.0],
        ];
        t.cull_lights_for_tiles(&translate);
        assert_eq!(
            t.tiles[400].light_indices.len(),
            1,
            "p×M: w=1 不変で光源は消失しない (旧では ww=-49 → skip = 修正前 RED)"
        );
    }

    /// EQ-3 fail-loud: NaN pos は旧来 `as i32`=0 飽和でタイル (0,0) へ
    /// 静寂割当 (照明の局所破壊)。契約明文化で panic 化。
    #[test]
    #[should_panic(expected = "契約違反: lights[0] が非有限")]
    fn nan_pos_panics() {
        let mut t = TiledDeferredLighting::new(1920, 1080);
        t.add_light(light([f32::NAN, 0.0, 1.0], 1.0));
        t.cull_lights_for_tiles(&ID);
    }

    #[test]
    #[should_panic(expected = "契約違反: lights[0].radius が負")]
    fn negative_radius_panics() {
        let mut t = TiledDeferredLighting::new(1920, 1080);
        t.add_light(light([0.0, 0.0, 1.0], -1.0));
        t.cull_lights_for_tiles(&ID);
    }

    #[test]
    #[should_panic(expected = "契約違反: view_proj が非有限")]
    fn non_finite_view_proj_panics() {
        let mut t = TiledDeferredLighting::new(1920, 1080);
        t.add_light(light([0.0, 0.0, 1.0], 1.0));
        let mut vp = ID;
        vp[1][1] = f32::INFINITY;
        t.cull_lights_for_tiles(&vp);
    }

    /// identity VP golden (新旧不変の安全性 pin、rq eq_tiled 導出):
    /// light [0,0,1] radius=1.0 → sx=960, sr=960px, ty=0..67 全行
    /// → 全 8160 タイル割当。MAX cap (64) は 1 灯で非到達。
    #[test]
    fn cull_identity_golden_counts() {
        let mut t = TiledDeferredLighting::new(1920, 1080);
        t.add_light(light([0.0, 0.0, 1.0], 1.0));
        t.cull_lights_for_tiles(&ID);
        assert_eq!(t.tiles_x * t.tiles_y, 8160, "120x68 = 8160 タイル");
        assert_eq!(
            t.tiles
                .iter()
                .filter(|tl| !tl.light_indices.is_empty())
                .count(),
            8160,
            "全タイル 1 灯"
        );
        assert_eq!(
            t.tiles.iter().map(|tl| tl.light_indices.len()).max(),
            Some(1)
        );
    }

    /// MAX_LIGHTS_PER_TILE=64 cap 静寂切捨て pin: 同一点 65 灯 →
    /// 対象タイルは 64 で打ち止め (65 灯目は捨てる = 誠実注記 (c))。
    #[test]
    fn cap_64_silent_drop_pin() {
        let mut t = TiledDeferredLighting::new(1920, 1080);
        for _ in 0..65 {
            t.add_light(light([0.0, 0.0, 1.0], 1.0));
        }
        t.cull_lights_for_tiles(&ID);
        assert_eq!(
            t.tiles.iter().map(|tl| tl.light_indices.len()).max(),
            Some(64),
            "65 灯供給 → cap 64 (65 灯目は静寂切捨て、pin)"
        );
    }

    /// ww≤0.1 skip pin (誠実注記 (a)): カメラ背後・超近接光源は完全除外
    /// (巨大半径光源の視野内影響を見逃す近似 — 本 pin が境界を固定)。
    /// p×M で clip_w = -1: T 非依存に w=-1 を作る = col(3) = [0,0,-1,-1]
    /// …z 依存 w: M[0][3]=M[1][3]=0, M[2][3]=-1, M[3][3]=-1 → w=-z-1。
    #[test]
    fn ww_threshold_skip_pin() {
        let mut t = TiledDeferredLighting::new(1920, 1080);
        t.add_light(light([0.0, 0.0, 0.0], 1.0)); // w = -1 ≤ 0.1 → skip
        t.add_light(light([0.0, 0.0, -11.0], 1.0)); // w = 11-1 = 10 > 0.1 → 残存
        let mut vp = ID;
        vp[2][3] = -1.0;
        vp[3][3] = -1.0;
        t.cull_lights_for_tiles(&vp);
        assert_eq!(
            t.tiles.iter().map(|tl| tl.light_indices.len()).max(),
            Some(1),
            "w=-1 光源は skip、w=10 光源だけ残存"
        );
    }

    /// EQ-5: ww≤0.1 境界の閉区間 pin (adversarial `<` 変異の検出線
    /// 補完、rq eq_tiled_c 導出): w=0.1f32 (bits 0x3DCCCCCD) ちょうど
    /// は `<=` で skip、1 ulp 上 (0x3DCCCCCE) は残存。
    /// m33=col(3) 単一で w を直接制御 (pos 非依存)。
    #[test]
    fn ww_threshold_boundary_exact_pin() {
        let mut t = TiledDeferredLighting::new(1920, 1080);
        t.add_light(light([0.0, 0.0, 1.0], 1.0));
        let mut vp_skip = ID;
        vp_skip[3][3] = 0.1; // w ≡ 0.1f32 → <= で skip
        t.cull_lights_for_tiles(&vp_skip);
        assert!(
            t.tiles.iter().all(|tl| tl.light_indices.is_empty()),
            "w==0.1f32 (0x3DCCCCCD) は <= 規則で skip"
        );
        let mut vp_keep = ID;
        vp_keep[3][3] = f32::from_bits(0x3DCC_CCCE); // 1 ulp 上 → 残存
        t.cull_lights_for_tiles(&vp_keep);
        assert_eq!(
            t.tiles.iter().map(|tl| tl.light_indices.len()).max(),
            Some(1),
            "w=0x3DCCCCCE (>0.1) は残存 → 境界閉区間確定"
        );
    }

    /// new() 幾何 pin: 切上タイル数・容量・empty lights は全タイル空。
    #[test]
    fn new_geometry_and_empty_pin() {
        let t = TiledDeferredLighting::new(1920, 1080);
        assert_eq!((t.tiles_x, t.tiles_y, t.tiles.len()), (120, 68, 8160));
        let mut t2 = TiledDeferredLighting::new(1, 1);
        assert_eq!((t2.tiles_x, t2.tiles_y, t2.tiles.len()), (1, 1, 1));
        t2.cull_lights_for_tiles(&ID);
        assert!(
            t2.tiles[0].light_indices.is_empty(),
            "lights 空 → 全タイル空"
        );
    }
}
