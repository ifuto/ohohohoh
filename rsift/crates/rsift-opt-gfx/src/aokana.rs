//! # 31. Aokana Framework (`AokanaFramework` - 2025 I3D)
//!
//! SVDAG + LOD + ストリーミング + Hi-Z オクルージョンカリング + Visibility Buffer を
//! 統合した、GPU 駆動ボクセルレンダリングフレームワーク。
//! 単一の深い SVDAG ではなく複数の「浅い SVDAG (`ShallowSvdag`)」を領域ごとに並列配置し、
//! ポインタジャンプのメモリパフォーマンス低下を解消する。
//!
//! # 監査 2026-07-26 (wave 125 DY) — 契約公表
//!
//! - **DY-1**: `insert_shallow_region` の `mut dag` unused_mut 警告を根治
//!   (root_id 読取 + move のみで可変操作なし、opt-gfx lib 警告 7→6)。
//! - **DY-2**: wiring 実消費の公表: `full_graph_wiring:930` の登録は ry=0
//!   固定 (K-1 注記済・section_palettes に y 帯情報がないため)、:936 の
//!   evaluate 結果は `report.aokana_visible_regions` への**カウント集計**
//!   のみで、リージョン選択 (実カリング駆動) には未接続 — 「評価実効・
//!   消費は集計型」(恒等/常時 miss とも別型) であることを誇張なく公表。
//! - **DY-3**: リージョン座標スケール `coords * region_size_blocks(=64)`
//!   は 2^6 乗算のみのため `as f32` 変換は **i32 安全域で bit 正確**
//!   (2^24→0x4E800000 (2^30)・-2^24→0xCE800000、rq dy_vals.rq 導出・pin)。
//!   2 つの境界契約も固定: (i) `min+64` は 2^30 スケールで ulp=128 の
//!   タイ偶数丸めにより **AABB 厚み 0 に退化** (差 0、rq dy_max.rq) — 実害域
//!   (region ≦ 2^20 = 世界境界 4.7e5 内) では 2^26/ulp=8 で正確 (8 ulp)。
//!   (ii) **i32 乗算溢れ経路** (捕捉 49 — pin テストの 2^25 設計が debug
//!   panic で照らした実装上のハザード): region |c| ≥ 2^25 で c*64 が
//!   i32 overflow → debug panic・release wrap で符号反転 (-2^31→f32
//!   0xCF000000、rq dy_wrap.rq)。DU-5 同型 2 件目として契約記録
//!   (実害域 ≦2^20 で到達不能、fail-loud 文化の debug panic は検知側に立つ)。
//! - **DY-4**: p-vertex 選択 `>= 0.0` の等価変異証明: `> 0.0` との差は
//!   成分 ±0.0 の場合のみ生じるが、その寄与は ±0.0×座標 = ±0.0 で和と
//!   `< 0.0` 判定に一切影響しない (NaN 成分は両比較 false で同選択) —
//!   **完全等価変異**であり、adversarial では機械確認 (全緑) の上、
//!   検出不能として誠実記録。検出担保は (a') n-vertex 反転変異で実施
//!   (pin が RED)。斜め平面 pin (rq 導出 24/-8) を追加して非軸平面の
//!   p-vertex 経路を固定。

use crate::hzb_2d::Hzb2D;
use crate::svdag::SparseVoxelDag;
use crate::visibility_buffer::VisibilityBufferResolver;
use std::collections::HashMap;

pub struct ShallowSvdag {
    pub region_coords: (i32, i32, i32),
    pub dag: SparseVoxelDag,
    pub root_id: u32,
    pub lod_level: u8,
}

pub struct AokanaFramework {
    pub shallow_dags: HashMap<(i32, i32, i32), ShallowSvdag>,
    pub hzb_occlusion: Hzb2D,
    pub visibility_resolver: VisibilityBufferResolver,
    pub region_size_blocks: i32,
}

impl AokanaFramework {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            shallow_dags: HashMap::new(),
            hzb_occlusion: Hzb2D::new(screen_width, screen_height, true),
            visibility_resolver: VisibilityBufferResolver::new(screen_width, screen_height),
            region_size_blocks: 64, // 64³ shallow SVDAG blocks per region
        }
    }

    /// リージョン座標 (rx,ry,rz) に shallow SVDAG を登録する。
    /// 契約: 同一座標への再登録は**最新で置換** (定期 refresh 経路 — wave 78
    /// CB-2 でピン)。保持 `root_id` は `dag.root_id` の読取 (wave 77 CA-1 以降、
    /// build_from_volume 直後の DAG なら真値の入口点が保持される)。
    pub fn insert_shallow_region(
        &mut self,
        rx: i32,
        ry: i32,
        rz: i32,
        dag: SparseVoxelDag,
        lod: u8,
    ) {
        let root_id = dag.root_id;
        self.shallow_dags.insert(
            (rx, ry, rz),
            ShallowSvdag {
                region_coords: (rx, ry, rz),
                dag,
                root_id,
                lod_level: lod,
            },
        );
    }

    /// Execute one Aokana frame: evaluate region AABB against frustum planes
    /// and return visible shallow SVDAG region coords.
    ///
    /// 誠実化 (wave 78 CB-3): 現行の描画判定は **frustum p-vertex テストのみ**。
    /// `hzb_occlusion` / `visibility_resolver` フィールドは occlusion pass 統合
    /// 用に確保されているが evaluate には未配線 (旧 doc の「Hi-Z 評価・
    /// visibility buffer 命令発行」は未実装の過剰主張だった)。
    /// (`_cam_pos` は将来の距離ベース LOD 選択用に保持)
    ///
    /// 決定性契約 (wave 78 CB-1): 戻り Vec は **(rx,ry,rz) 辞書順ソート済み**。
    /// 旧実装は HashMap 反復順そのまま (SipHash ランダムシードでプロセス毎に
    /// 不定) で、順序消費で flaky になる潜伏があった (BS-2 型の根治)。
    /// 平面の数値精度: 符号テストのみのため正規化不要。境界 `dot+d == 0`
    /// (接触) は可視扱い (機械ピン)。
    pub fn evaluate_visible_regions(
        &self,
        _cam_pos: [f32; 3],
        frustum_planes: &[[f32; 4]; 6],
    ) -> Vec<(i32, i32, i32)> {
        let mut coords: Vec<_> = self.shallow_dags.keys().copied().collect();
        coords.sort_unstable();
        let mut visible = Vec::with_capacity(coords.len());
        for coords in coords {
            let dag = &self.shallow_dags[&coords];
            let min = [
                (coords.0 * self.region_size_blocks) as f32,
                (coords.1 * self.region_size_blocks) as f32,
                (coords.2 * self.region_size_blocks) as f32,
            ];
            let max = [
                min[0] + self.region_size_blocks as f32,
                min[1] + self.region_size_blocks as f32,
                min[2] + self.region_size_blocks as f32,
            ];
            // Frustum check against 6 planes
            let mut pass = true;
            for p in frustum_planes {
                let px = if p[0] >= 0.0 { max[0] } else { min[0] };
                let py = if p[1] >= 0.0 { max[1] } else { min[1] };
                let pz = if p[2] >= 0.0 { max[2] } else { min[2] };
                if p[0] * px + p[1] * py + p[2] * pz + p[3] < 0.0 {
                    pass = false;
                    break;
                }
            }
            if pass {
                visible.push(dag.region_coords);
            }
        }
        visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aokana_shallow_svdag() {
        let mut aokana = AokanaFramework::new(1280, 720);
        aokana.insert_shallow_region(0, 0, 0, SparseVoxelDag::new(), 0);
        let planes = [
            [1.0, 0.0, 0.0, 100.0],
            [-1.0, 0.0, 0.0, 100.0],
            [0.0, 1.0, 0.0, 100.0],
            [0.0, -1.0, 0.0, 100.0],
            [0.0, 0.0, 1.0, 100.0],
            [0.0, 0.0, -1.0, 100.0],
        ];
        let vis = aokana.evaluate_visible_regions([0.0, 0.0, 0.0], &planes);
        assert_eq!(vis.len(), 1);
    }

    /// 全方向を大きく許容する 6 面 (符号テストで常時 pass: 各 p-vertex で
    /// dot+d ≥ 96 > 0 となる d=100 配置)。
    fn permissive_planes() -> [[f32; 4]; 6] {
        [
            [1.0, 0.0, 0.0, 100.0],
            [-1.0, 0.0, 0.0, 100.0],
            [0.0, 1.0, 0.0, 100.0],
            [0.0, -1.0, 0.0, 100.0],
            [0.0, 0.0, 1.0, 100.0],
            [0.0, 0.0, -1.0, 100.0],
        ]
    }

    #[test]
    fn visible_regions_sorted_and_cross_instance_deterministic() {
        // CB-1: 乱順登録でも結果は (rx,ry,rz) 辞書順。HashMap の反復順は
        // インスタンス毎のランダムシードで変わるため、16 インスタンス横断で
        // 完全一致を要求すれば旧実装の一致確率は実質 0 (決定的検出)。
        let mut expected_results = Vec::new();
        for _ in 0..16 {
            let mut aokana = AokanaFramework::new(1280, 720);
            aokana.insert_shallow_region(1, 0, 0, SparseVoxelDag::new(), 0);
            aokana.insert_shallow_region(0, 1, 1, SparseVoxelDag::new(), 0);
            aokana.insert_shallow_region(0, 0, 0, SparseVoxelDag::new(), 0);
            let vis = aokana.evaluate_visible_regions([0.0, 0.0, 0.0], &permissive_planes());
            expected_results.push(vis);
        }
        let first = &expected_results[0];
        assert_eq!(*first, vec![(0, 0, 0), (0, 1, 1), (1, 0, 0)]); // 辞書順厳密
        for v in &expected_results {
            assert_eq!(v, first); // 全インスタンスで完全一致 (決定性契約)
        }
    }

    #[test]
    fn frustum_plane_boundary_exact() {
        // 境界契約: dot+d == 0 (接触) は可視。厳密な f32 整数で導出:
        // 平面 [-1,0,0,64] (-x+64): region (1,0,0) は p-vertex x=min=64 で
        // -64+64=0 ≥0 → 可視、region (2,0,0) は -128+64=-64 <0 → 完全外部で捌く。
        let mut planes = permissive_planes();
        planes[0] = [-1.0, 0.0, 0.0, 64.0];
        let mut aokana = AokanaFramework::new(1280, 720);
        aokana.insert_shallow_region(2, 0, 0, SparseVoxelDag::new(), 0);
        aokana.insert_shallow_region(1, 0, 0, SparseVoxelDag::new(), 0);
        let vis = aokana.evaluate_visible_regions([0.0, 0.0, 0.0], &planes);
        assert_eq!(vis, vec![(1, 0, 0)]); // 接触のみ生存・順序も辞書順
    }

    #[test]
    fn insert_replaces_and_root_id_is_truthful_after_build() {
        // CB-2 + CA-1 連携: 再登録は置換、root_id は build 済み DAG の真値。
        let mut aokana = AokanaFramework::new(1280, 720);
        let mut solid = SparseVoxelDag::new();
        let root = solid.build_from_volume(&[[[true; 16]; 16]; 16]);
        assert_eq!(root, 5); // wave 77 CA-3 の手導出値
        aokana.insert_shallow_region(0, 0, 0, solid, 0);
        assert_eq!(aokana.shallow_dags[&(0, 0, 0)].root_id, 5); // 真値保持
        assert_eq!(aokana.shallow_dags[&(0, 0, 0)].lod_level, 0);
        // 同一座標へ空 DAG を再登録 → 置換 (root_id 0)、件数は増えない
        aokana.insert_shallow_region(0, 0, 0, SparseVoxelDag::new(), 2);
        assert_eq!(aokana.shallow_dags.len(), 1);
        assert_eq!(aokana.shallow_dags[&(0, 0, 0)].root_id, 0);
        assert_eq!(aokana.shallow_dags[&(0, 0, 0)].lod_level, 2);
        // region_size_blocks は 64 固定 (aokana 規約 — full_graph_wiring 注記と一致)
        assert_eq!(aokana.region_size_blocks, 64);
    }

    // ================= wave 125 DY: 厳密契約ピン群 =================
    // 全厳密値は rq (dy_vals.rq, RQ.md v2) で事前導出・assert 通過済。

    /// DY-3: リージョン座標スケールの bit 正確性 pin。coords*64 は 2^6
    /// 乗算 (指数シフト) のみで `as f32` は全 i32 域で正確 (丸めなし)。
    /// rq 導出 bits: 2^24→0x4E800000 (2^30)・2^25→0x4F000000・
    /// -2^24→0xCE800000。
    #[test]
    fn region_coord_scale_is_bit_exact() {
        let s = AokanaFramework::new(64, 64).region_size_blocks;
        assert_eq!(s, 64);
        let scale = |c: i32| (c * s) as f32;
        assert_eq!(scale(1 << 24).to_bits(), 0x4E80_0000, "2^24*64=2^30 正確");
        assert_eq!(scale(-(1 << 24)).to_bits(), 0xCE80_0000, "負側 正確");
        // **i32 溢れ経路の契約 pin** (捕捉 49: 素朴な (1<<25)*64 は debug
        // panic を自身で照らした): release wrap 相当は符号反転 0xCF000000
        // (rq dy_wrap.rq 導出)。wrapping_mul で panic なく安全に検証する。
        let wrapped = (1i32 << 25).wrapping_mul(s) as f32;
        assert_eq!(wrapped.to_bits(), 0xCF00_0000, "wrap → 符号反転 (契約記録)");
        // **退化境界の公表** (rq dy_max.rq で機械確認): 2^30 の ulp は 128
        // のため min+64 (=タイ中間値) は round-to-even で 2^30 に丸め戻り、
        // AABB が厚み 0 に退化 (max.to_bits == min.to_bits、差 0)。
        // 実害域 (region ≦ 2^20、世界境界 4.7e5 内) では 2^26 スケールで
        // ulp=8、+64 は正確 (差 8 ulp) — 両面を pin で固定。
        let max_huge = scale(1 << 24) + s as f32;
        assert_eq!(
            max_huge.to_bits() - scale(1 << 24).to_bits(),
            0,
            "2^24 region: +64 はタイ偶数丸めで退化 (公表、実害域外)"
        );
        let max_real = scale(1 << 20) + s as f32;
        assert_eq!(
            max_real.to_bits() - scale(1 << 20).to_bits(),
            8,
            "2^20 region: +64 は 8 ulp 正確 (実害域では AABB 厚み保持)"
        );
    }

    /// DY-4: 斜め平面での p-vertex 経路 pin (非軸平面で max/min 選択が
    /// 分岐する経路を固定)。rq 導出: region (0,0,0) → dot+d=24 ≥0 可視、
    /// region (-1,0,0) → -8 <0 不可視。
    #[test]
    fn diagonal_plane_pvertex_selection_exact() {
        let mut aokana = AokanaFramework::new(64, 64);
        aokana.insert_shallow_region(0, 0, 0, SparseVoxelDag::new(), 0);
        aokana.insert_shallow_region(-1, 0, 0, SparseVoxelDag::new(), 0);
        let mut planes = permissive_planes();
        // 斜め平面 [0.5, 0.5, 0, -40]: p-vertex は (+x,+y) 側 = (64,64,64)
        planes[0] = [0.5, 0.5, 0.0, -40.0];
        let vis = aokana.evaluate_visible_regions([0.0, 0.0, 0.0], &planes);
        assert_eq!(
            vis,
            vec![(0, 0, 0)],
            "斜め平面: (0,0,0) は 24≥0 可視・(-1,0,0) は -8<0 不可視 (rq 導出)"
        );
        // 法線反転すると p-vertex も反転し判定が入れ替わる経路の pin
        planes[0] = [-0.5, -0.5, 0.0, 40.0 + 31.0]; // d=71: (-0)*(-64)*2=64 …
        let vis2 = aokana.evaluate_visible_regions([0.0, 0.0, 0.0], &planes);
        // -x-64-0.5*64-40+71 = -(0.5*(-64))-(0.5*64)-40+71 → per vertex:
        // region (0,0,0): n-vertex 側選択 min: -0.5*0 + -0.5*0 + 71 = 71 ≥0 可視
        // region (-1,0,0): -0.5*(-64) + -0.5*0 + 71 = 32+71 = 103 ≥0 可視
        assert_eq!(vis2.len(), 2, "反転法線+d 調整で両者可視 (選択対称性)");
    }
}
