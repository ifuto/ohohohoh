//! Chunk visibility culling — VisGraph + frustum + empty-column fast path.
//!
//! 【wave 157 FC 監査注記】
//! 1. 捕捉 78: 旧 `visgraph_enabled`/`max_section_draw` は from_profile が
//!    値を装填するのみで**読み手が crates 全域にゼロ** (census grep 機械
//!    確定、§7 未配線)。visgraph 側は render_pipeline に gate 対象の flood
//!    fill が存在せず (wave 129 EC 構造) + 全 tier `visgraph_occlusion=true`
//!    恒真 + BK 設計で verdict が flag を読まない → 捏造ゲートは偽装禁止
//!    抵触のため不可能証明の上削除。max_section_draw 側は部分 section cap
//!    が chunk 全体シェーディング構造と不整合 (捏造機能回避) で同様削除。
//! 2. 捕捉 79: `CullStats`+`apply` が消費者ゼロの孤立統計機構だった
//!    (render_pipeline は frame_stats arms 直加算のみ) → §7 消化 20 で
//!    第 2 帳簿として真蓄積 + frame 末端 cross 照合 (Σ 完全性+3 面一致)。
//! 3. 順序: EmptyColumn は距離判定**より先** (遠方の空列は EmptyColumn
//!    に帰属、range_skipped は遠方空列を数えない — 統計帰属の設計意図、
//!    順序 golden で pin)。
//! 4. 境界: `dist > radius` 厳密に大きい場合のみ OutOfRange。dist==radius
//!    は Visible (等号の帰属は 1-ulp 窓 golden で pin、実機 probe
//!    0x413504f3 = hypot(8,8)=sqrt(128) bit 一致確認済)。
//! 5. `Occluded` は wave 61 BK 設計で送出されない (隣接データ無しの全列
//!    遮蔽判定はカメラ移動時「透過ホール」を招く)。enum arm と pipeline
//!    両 call site の skip 統一 (wave 86 CJ-2) は将来 producer 出現時の
//!    乖離防止の防衛であり、現挙動では occluded_skipped は恒 0。

use crate::section_rle::RleSection;

/// Per-chunk cull decision before mesh build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CullVerdict {
    /// Build and draw.
    Visible,
    /// No solid blocks in column (RLE fast path).
    EmptyColumn,
    /// VisGraph: fully enclosed sections, no exposed faces toward neighbors.
    Occluded,
    /// Outside view distance.
    OutOfRange,
}

#[derive(Debug, Default, Clone)]
pub struct CullStats {
    pub tested: u32,
    pub empty_skipped: u32,
    pub occluded_skipped: u32,
    pub range_skipped: u32,
    pub visible: u32,
}

pub struct ChunkCullPass {
    pub view_radius_blocks: f32,
}

impl ChunkCullPass {
    pub fn from_profile(profile: &rsift_api::AdaptiveRenderProfile) -> Self {
        let rd = profile.render_distance.max(8) as f32;
        Self {
            view_radius_blocks: rd * 16.0,
        }
    }

    /// Cull using RLE section column (no full palette decode).
    pub fn verdict_column(
        &self,
        cx: i32,
        cz: i32,
        camera_x: f32,
        camera_z: f32,
        sections: &[RleSection],
    ) -> CullVerdict {
        if sections.iter().all(|s| s.is_empty()) {
            return CullVerdict::EmptyColumn;
        }
        let dx = cx as f32 * 16.0 + 8.0 - camera_x;
        let dz = cz as f32 * 16.0 + 8.0 - camera_z;
        let dist = dx.hypot(dz);
        if dist > self.view_radius_blocks {
            return CullVerdict::OutOfRange;
        }
        // wave 61 BK: 旧来は visgraph_enabled 時に occupied_section_indices を
        // 計算して**そのまま破棄**する死に計算があった (cull 熱経路での純粋無駄)。
        // 隣接データ無しにチャンク全体を occluded 判定しない設計自体は正しい
        // (カメラ移動時の「透過ホール」障害の再発防止) ので、実計算のみ撤去。
        CullVerdict::Visible
    }

    pub fn apply(&self, verdict: CullVerdict, stats: &mut CullStats) {
        stats.tested += 1;
        match verdict {
            CullVerdict::Visible => stats.visible += 1,
            CullVerdict::EmptyColumn => stats.empty_skipped += 1,
            CullVerdict::Occluded => stats.occluded_skipped += 1,
            CullVerdict::OutOfRange => stats.range_skipped += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::SectionPalette;

    fn pass(view_radius_blocks: f32) -> ChunkCullPass {
        ChunkCullPass { view_radius_blocks }
    }

    fn air_sections(n: usize) -> Vec<RleSection> {
        let air: SectionPalette = [0u16; 4096];
        (0..n).map(|_| RleSection::encode(&air)).collect()
    }

    fn sections_with_one_block(n: usize) -> Vec<RleSection> {
        let mut v = air_sections(n);
        let mut p: SectionPalette = [0u16; 4096];
        p[0] = 7; // 1 voxel だけ非空
        v[0] = RleSection::encode(&p);
        v
    }

    #[test]
    fn all_air_column_is_empty_fast_path() {
        let c = pass(128.0);
        assert_eq!(
            c.verdict_column(0, 0, 0.0, 0.0, &air_sections(4)),
            CullVerdict::EmptyColumn
        );
    }

    #[test]
    fn empty_check_precedes_distance_check() {
        // 遠方でも空列は EmptyColumn が返る順序 (早期 return の固定)。
        let c = pass(128.0);
        assert_eq!(
            c.verdict_column(400, 0, 0.0, 0.0, &air_sections(4)),
            CullVerdict::EmptyColumn
        );
    }

    #[test]
    fn beyond_view_radius_is_out_of_range() {
        let c = pass(128.0);
        // チャンク(20,0) 中心 (328,8): camera (0,0) から ~328 > 128。
        assert_eq!(
            c.verdict_column(20, 0, 0.0, 0.0, &sections_with_one_block(4)),
            CullVerdict::OutOfRange
        );
    }

    #[test]
    fn in_range_occupied_column_is_visible() {
        let c = pass(128.0);
        assert_eq!(
            c.verdict_column(0, 0, 0.0, 0.0, &sections_with_one_block(4)),
            CullVerdict::Visible
        );
    }

    #[test]
    fn range_boundary_is_strictly_greater() {
        // チャンク(0,0) 中心 (8,8) → dist = hypot(8,8) ≈ 11.313708。
        let d = 8f32.hypot(8.0);
        let inside = pass(d + 1e-3);
        let outside = pass(d - 1e-3);
        let secs = sections_with_one_block(4);
        assert_eq!(
            inside.verdict_column(0, 0, 0.0, 0.0, &secs),
            CullVerdict::Visible
        );
        assert_eq!(
            outside.verdict_column(0, 0, 0.0, 0.0, &secs),
            CullVerdict::OutOfRange
        );
    }

    #[test]
    fn verdict_never_returns_occluded_by_design() {
        // 旧実装は近傍データ無しに Occluded を返してカメラ移動時に
        // 「透明な穴」が開く事故を起こした。verdict_column は統計のみで
        // チャンク丸ごとの遮蔽判定をしない、という設計決定を固定する
        // (旧 visgraph_enabled flag は読み手ゼロで wave 157 FC 捕捉 78 削除)。
        let c = pass(128.0);
        assert_eq!(
            c.verdict_column(0, 0, 0.0, 0.0, &sections_with_one_block(4)),
            CullVerdict::Visible
        );
    }

    #[test]
    fn apply_accumulates_each_verdict() {
        let c = pass(128.0);
        let mut st = CullStats::default();
        for v in [
            CullVerdict::Visible,
            CullVerdict::EmptyColumn,
            CullVerdict::Occluded,
            CullVerdict::OutOfRange,
            CullVerdict::Visible,
        ] {
            c.apply(v, &mut st);
        }
        assert_eq!(st.tested, 5);
        assert_eq!(st.visible, 2);
        assert_eq!(st.empty_skipped, 1);
        assert_eq!(st.occluded_skipped, 1);
        assert_eq!(st.range_skipped, 1);
    }

    #[test]
    fn from_profile_floors_zero_advisory_distance_at_8_chunks() {
        // 全 tier の render_distance は advisory 0 (vanilla 尊重)。
        // from_profile は max(8) 適用で 8*16=128 blocks の下限になる。
        use rsift_api::adaptive_perf::{AdaptivePerfEngine, HardwareProfile, PerformanceTier};
        let hw = HardwareProfile {
            tier: PerformanceTier::Minimal,
            cpu_cores: 2,
            cpu_threads: 4,
            cpu_model: "bench-cpu".to_string(),
            ram_gb: 4.0,
            gpu_score: 0,
            gpu_name: "bench-gpu".to_string(),
            is_mobile_gpu: false,
            is_software_renderer: true,
            flagship_boost: false,
        };
        let profile = AdaptivePerfEngine::render_profile(&hw);
        assert_eq!(profile.render_distance, 0); // 仕様の前提確認
        let c = ChunkCullPass::from_profile(&profile);
        assert_eq!(c.view_radius_blocks, 8.0 * 16.0);
        // 捕捉 78 (wave 157 FC): visgraph_enabled/max_section_draw は
        // 読み手ゼロで削除 — profile.visgraph_occlusion は全 tier 恒真で
        // gate 対象 (flood fill) が pipeline に存在しない (不可能証明)。
        assert!(
            profile.visgraph_occlusion,
            "全 tier 恒真の census 証拠 (rsift-api adaptive_perf 4 tier)"
        );
    }

    /// 【wave 157 FC-4】apply の Σ 完全性 golden (rq fc_cull (1)):
    /// [V,E,O,R,V,V,E] → tested=7, (3,2,1,1)、任意接頭辞で Σ==tested。
    #[test]
    fn apply_completeness_sigma_golden_strict() {
        let c = pass(128.0);
        let mut st = CullStats::default();
        let seq = [
            CullVerdict::Visible,
            CullVerdict::EmptyColumn,
            CullVerdict::Occluded,
            CullVerdict::OutOfRange,
            CullVerdict::Visible,
            CullVerdict::Visible,
            CullVerdict::EmptyColumn,
        ];
        for (i, v) in seq.iter().enumerate() {
            c.apply(*v, &mut st);
            assert_eq!(
                st.tested,
                st.visible + st.empty_skipped + st.occluded_skipped + st.range_skipped,
                "prefix {i}: Σ 完全性"
            );
        }
        assert_eq!(
            (
                st.tested,
                st.visible,
                st.empty_skipped,
                st.occluded_skipped,
                st.range_skipped
            ),
            (7, 3, 2, 1, 1),
            "終端 golden (rq fc_cull (1))"
        );
    }

    /// 【wave 157 FC-4】境界等号の帰属 + 1-ulp 窓 golden (rq fc_cull (4)、
    /// 実機 probe: hypot(8,8)=0x413504f3、sqrt(128) と bit 一致確認済):
    /// dist==radius は `>` 非成立で **Visible**、radius が 1 ulp 下回ると
    /// OutOfRange、1 ulp 上なら Visible の 3 点厳密窓。
    #[test]
    fn range_boundary_equality_ulp_window_strict() {
        let secs = sections_with_one_block(4);
        let d = 8f32.hypot(8.0);
        assert_eq!(d.to_bits(), 0x4135_04F3, "hypot(8,8) 実機 bits");
        let eq = pass(d);
        assert_eq!(
            eq.verdict_column(0, 0, 0.0, 0.0, &secs),
            CullVerdict::Visible,
            "dist==radius → Visible (> 厳密)"
        );
        let below = pass(f32::from_bits(d.to_bits() - 1));
        assert_eq!(
            below.verdict_column(0, 0, 0.0, 0.0, &secs),
            CullVerdict::OutOfRange,
            "radius 1 ulp 下 → dist>radius で OutOfRange"
        );
        let above = pass(f32::from_bits(d.to_bits() + 1));
        assert_eq!(
            above.verdict_column(0, 0, 0.0, 0.0, &secs),
            CullVerdict::Visible,
            "radius 1 ulp 上 → Visible"
        );
    }

    /// 【wave 157 FC-4】from_profile 半径のカスタム rd 導出 golden
    /// (rq fc_cull (3)): rd=12 → 12*16=192.0、floor は max(8) のみ。
    #[test]
    fn from_profile_custom_radius_golden_strict() {
        use rsift_api::adaptive_perf::{AdaptivePerfEngine, HardwareProfile, PerformanceTier};
        let hw = HardwareProfile {
            tier: PerformanceTier::Minimal,
            cpu_cores: 2,
            cpu_threads: 4,
            cpu_model: "bench-cpu".to_string(),
            ram_gb: 4.0,
            gpu_score: 0,
            gpu_name: "bench-gpu".to_string(),
            is_mobile_gpu: false,
            is_software_renderer: true,
            flagship_boost: false,
        };
        let mut profile = AdaptivePerfEngine::render_profile(&hw);
        profile.render_distance = 12;
        let c = ChunkCullPass::from_profile(&profile);
        assert_eq!(
            c.view_radius_blocks.to_bits(),
            192.0f32.to_bits(),
            "rd=12 → 192 exact"
        );
        profile.render_distance = 7; // floor 未満 → 8 へ丸め
        let c2 = ChunkCullPass::from_profile(&profile);
        assert_eq!(
            c2.view_radius_blocks.to_bits(),
            128.0f32.to_bits(),
            "rd=7 → floor 8 → 128"
        );
    }

    /// 【wave 157 FC-4】center 公式と dist 値 bits golden (rq fc_cull (4)、
    /// 実機 probe: centers は int-exact (24=0x41c00000/−8=0xc1000000/
    /// 8=0x41000000)、hypot(24,8)=0x41ca62c2、hypot(−8,8)=0x413504f3 で
    /// naive sqrt と bit 一致確認済)。±chunk は dist **非対称** (中心が
    /// camera 原点から ±16 でなく 8 オフセットのため) — 当初「対称」と
    /// 誤記した設計を probe 照合で自己捕捉・訂正した経緯を注記。
    #[test]
    fn center_formula_dist_bits_golden_strict() {
        // center 公式の実装逐次: cx as f32 * 16.0 + 8.0 − camera
        let cx1 = 1f32 * 16.0 + 8.0 - 0.0;
        let cxm = (-1f32) * 16.0 + 8.0 - 0.0;
        let czz = 0f32 * 16.0 + 8.0 - 0.0;
        assert_eq!(cx1.to_bits(), 24.0f32.to_bits(), "cx=1 → 24 exact");
        assert_eq!(cxm.to_bits(), (-8.0f32).to_bits(), "cx=-1 → -8 exact");
        assert_eq!(czz.to_bits(), 8.0f32.to_bits(), "cz=0 → 8 exact");
        let d_far = cx1.hypot(czz);
        let d_near = cxm.hypot(czz);
        assert_eq!(d_far.to_bits(), 0x41CA_62C2, "chunk(1,0) dist bits");
        assert_eq!(d_near.to_bits(), 0x4135_04F3, "chunk(-1,0) dist bits");
        assert!(d_far > d_near, "dist は ± 非対称 (8 offset 由来)");
        // 非対称の観測面 pin: 半径を d_far と d_near の中間に置くと
        // 近側のみ Visible (遠側は OutOfRange)。中点は平均で確実に内側。
        let mid = (d_far + d_near) * 0.5;
        assert!(d_near < mid && mid < d_far, "中点は両 dist の狭間");
        let c = pass(mid);
        let secs = sections_with_one_block(4);
        assert_eq!(
            c.verdict_column(1, 0, 0.0, 0.0, &secs),
            CullVerdict::OutOfRange,
            "radius=mid で chunk(1,0) は OutOfRange"
        );
        assert_eq!(
            c.verdict_column(-1, 0, 0.0, 0.0, &secs),
            CullVerdict::Visible,
            "radius=mid で chunk(-1,0) は Visible (非対称帰属)"
        );
    }

    /// 【wave 157 FC-4】2×2 帰属 matrix golden (rq fc_cull (2)(4)):
    /// (近/遠)×(空/占有) → Visible/EmptyColumn/OutOfRange/EmptyColumn の
    /// 全 4 セル + apply 帰属 (遠方空は range ではなく empty に帰属)。
    #[test]
    fn verdict_attribution_matrix_golden_strict() {
        let c = pass(128.0);
        let near_empty = c.verdict_column(0, 0, 0.0, 0.0, &air_sections(4));
        let near_occ = c.verdict_column(0, 0, 0.0, 0.0, &sections_with_one_block(4));
        let far_empty = c.verdict_column(400, 0, 0.0, 0.0, &air_sections(4));
        let far_occ = c.verdict_column(400, 0, 0.0, 0.0, &sections_with_one_block(4));
        assert_eq!(
            (near_empty, near_occ, far_empty, far_occ),
            (
                CullVerdict::EmptyColumn,
                CullVerdict::Visible,
                CullVerdict::EmptyColumn,
                CullVerdict::OutOfRange
            ),
            "2×2 帰属 matrix (empty は距離より先)"
        );
        let mut st = CullStats::default();
        c.apply(far_empty, &mut st);
        assert_eq!(st.empty_skipped, 1, "遠方空 → empty 帰属");
        assert_eq!(st.range_skipped, 0, "range_skipped は遠方空を数えない設計");
        c.apply(far_occ, &mut st);
        assert_eq!(st.range_skipped, 1, "遠方占有 → range 帰属");
    }
}
