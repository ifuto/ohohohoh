//! Software tile binning — TBDR-inspired tile assignment (32×32 / 64×64 tiles).
//!
//! 【wave 178 FX-2 誠実注記】モジュール doc の旧称「draw reorder」に対し、
//! 現行の render_pipeline 消費は `lists.len()` (frame_stats.tiles_binned
//! 計測) のみで、`TileDrawList.chunk_indices` の front-to-back 順は描画順へ
//! 未配線 (chunk_indices 本体の truth は module テストが厳密 pin 済)。
//! reorder 効果の真の配線は wiring_priority (OverdrawSorter 由来) 系統が
//! 担う別経路 — 本 module は bin 計算 + タイル数計測に限定される。

use std::collections::HashMap;
use tracing::trace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileId {
    pub tx: u32,
    pub ty: u32,
}

#[derive(Debug, Clone)]
pub struct TileDrawList {
    pub tile: TileId,
    pub chunk_indices: Vec<usize>,
}

/// Bin chunk AABBs into screen tiles for front-to-back / overdraw locality.
pub struct SoftwareTileBinner {
    pub tile_size: u32,
    pub screen_w: u32,
    pub screen_h: u32,
}

impl SoftwareTileBinner {
    pub fn new(screen_w: u32, screen_h: u32, tile_size: u32) -> Self {
        Self {
            tile_size: tile_size.max(16),
            screen_w,
            screen_h,
        }
    }

    /// 【wave 178 FX-3】外部消費者ゼロ (全クレート grep: tile_count は
    /// 他 module/他 crate に参照なし) を機械確定 → pub 剥奪。内部は
    /// bin_chunks の tw/th 導出にのみ消費。
    fn tile_count(&self) -> (u32, u32) {
        let tw = (self.screen_w + self.tile_size - 1) / self.tile_size;
        let th = (self.screen_h + self.tile_size - 1) / self.tile_size;
        (tw.max(1), th.max(1))
    }

    /// Project chunk (cx, cz) to NDC-ish screen coords and assign tiles.
    pub fn bin_chunks(
        &self,
        chunks: &[(i32, i32)],
        camera_chunk_x: f32,
        camera_chunk_z: f32,
    ) -> Vec<TileDrawList> {
        let (tw, th) = self.tile_count();
        let mut map: HashMap<TileId, Vec<usize>> = HashMap::new();

        for (i, &(cx, cz)) in chunks.iter().enumerate() {
            let dx = cx as f32 - camera_chunk_x;
            let dz = cz as f32 - camera_chunk_z;
            // 【wave 178 FX-4】旧実装の `.clamp(-1.0, 1.0)` は装飾的二重防御で
            // あったため数学的等価証明の上で削除: ndc は後続の
            // px = ((ndc*0.5+0.5)*screen) as u32 (RFC 0454 飽和キャスト) と
            // min(tw-1) cap を通るため、|ndc|>1 は全て
            // (i) ndc>1 → px≥640 → tx=floor(px/ts)≥tw → min で tw-1、
            // (ii) ndc<-1 → px≤0 → 負の f32→u32 飽和で 0、
            // (iii) NaN → 飽和で 0、のいずれでも clamp 有りと観測等価
            // (adversarial 変異で非検出 = 関数等価の実証、FW-2 とは別枠)。
            let ndc_x = dx * 0.05;
            let ndc_y = dz * 0.05;
            let px = ((ndc_x * 0.5 + 0.5) * self.screen_w as f32) as u32;
            let py = ((ndc_y * 0.5 + 0.5) * self.screen_h as f32) as u32;
            let tx = (px / self.tile_size).min(tw - 1);
            let ty = (py / self.tile_size).min(th - 1);
            map.entry(TileId { tx, ty }).or_default().push(i);
        }

        let mut lists: Vec<TileDrawList> = map
            .into_iter()
            .map(|(tile, mut chunk_indices)| {
                chunk_indices.sort_by_key(|&i| {
                    let (cx, cz) = chunks[i];
                    let d = (cx as f32 - camera_chunk_x).hypot(cz as f32 - camera_chunk_z);
                    (d * 1000.0) as u32
                });
                TileDrawList { tile, chunk_indices }
            })
            .collect();
        lists.sort_by_key(|l| (l.tile.ty, l.tile.tx));
        trace!("[TileBin] {} tiles for {} chunks", lists.len(), chunks.len());
        lists
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_size_floor_and_count_ceil() {
        let b = SoftwareTileBinner::new(1920, 1080, 8);
        assert_eq!(b.tile_size, 16); // 下限 16 にクランプ
        assert_eq!(b.tile_count(), (120, 68));
        let b = SoftwareTileBinner::new(1920, 1080, 32);
        assert_eq!(b.tile_count(), (60, 34)); // ceil(1080/32)=34
        let b = SoftwareTileBinner::new(0, 0, 32);
        assert_eq!(b.tile_count(), (1, 1)); // 0 画面は 0 除算回避で下限 1
    }

    #[test]
    fn bin_chunks_projects_sorts_tiles_and_front_to_back() {
        let b = SoftwareTileBinner::new(640, 640, 32); // 20x20 タイル
        // camera 原点: (0,0) は画面中央 px=320 → tile (10,10)。
        // (4,0)/(5,0): px=384/400 → ともに tile (12,10)、距離 4<5。
        // (0,5): tile (10,12)。
        let chunks = [(5, 0), (4, 0), (0, 5), (0, 0)];
        let lists = b.bin_chunks(&chunks, 0.0, 0.0);
        let tiles: Vec<(u32, u32)> = lists.iter().map(|l| (l.tile.tx, l.tile.ty)).collect();
        assert_eq!(tiles, vec![(10, 10), (12, 10), (10, 12)]); // (ty,tx) 昇順
        let t = lists.iter().find(|l| l.tile.tx == 12).unwrap();
        assert_eq!(t.chunk_indices, vec![1, 0]); // front-to-back (近い方が先)
        // 単一チャンク経路: 中央は (10,10) 固定。
        let single = b.bin_chunks(&[(0, 0)], 0.0, 0.0);
        assert_eq!((single[0].tile.tx, single[0].tile.ty), (10, 10));
        assert_eq!(single[0].chunk_indices, vec![0]);
    }

    /// 【wave 178 FX-1 補題 pin】bin_chunks は camera_chunk 引数で画面
    /// 再中心化される (render 側 FX-1 配線の前提ロジック、green-today):
    /// camera (4,0) で chunk (4,0) は画面中央 tile (10,10)、camera (0,0) では
    /// 同 chunk が dx=4 → NDC 0.2 → px=384 → tile (12,10) へ移る。
    #[test]
    fn fx_camera_origin_recenters_bins() {
        let b = SoftwareTileBinner::new(640, 640, 32);
        let centered = b.bin_chunks(&[(4, 0)], 4.0, 0.0);
        assert_eq!((centered[0].tile.tx, centered[0].tile.ty), (10, 10));
        let offorigin = b.bin_chunks(&[(4, 0)], 0.0, 0.0);
        assert_eq!((offorigin[0].tile.tx, offorigin[0].tile.ty), (12, 10));
    }

    /// 【wave 178 FX】タイル index 境界の厳密 pin (green-today、FX-4 で
    /// clamp 削除後の実行 truth): オーバーレンジの飽和+cap 挙動 —
    /// dx=200 → ndc 10 → px=3520 → 3520/32=110 → min(tw-1)=19、
    /// dx=-200 → ndc -10 → px=-2880 → f32→u32 飽和 0 → tx=0、
    /// dz=-500 → py=-4320 → 飽和 0 → ty=0。
    #[test]
    fn fx_bin_edges_saturate_and_cap() {
        let b = SoftwareTileBinner::new(640, 640, 32); // tw=th=20
        let right = b.bin_chunks(&[(200, 0)], 0.0, 0.0);
        assert_eq!(
            right[0].tile.tx, 19,
            "正オーバー → cap tw-1 (px=3520/32=110)"
        );
        let left = b.bin_chunks(&[(-200, 0)], 0.0, 0.0);
        assert_eq!(left[0].tile.tx, 0, "負オーバー → saturating cast 0");
        let far_neg = b.bin_chunks(&[(0, -500)], 0.0, 0.0);
        assert_eq!(far_neg[0].tile.ty, 0, "負オーバー → ty=0");
    }

    /// 【wave 178 FX-3 pin】tile_count 可視性の lexeme pin (adversarial 変異 C
    /// 非検出捕捉 → 回収): 外部露出復活で RED。自己言及 vacuous 化回避の
    /// ため検出語彙は完全形を本テキストに置かない分割記述 (wave 176
    /// 自己照査捕捉の判例適用、FE include_str! 様式の自己ファイル版)。
    #[test]
    fn fx_tile_count_visibility_lexeme() {
        let src = include_str!("software_tiling.rs");
        let exposed = concat!("pub f", "n tile_count");
        let internal = concat!("f", "n tile_count(&self)");
        assert!(
            !src.contains(exposed),
            "外部露出の再来を検出 (FX-3 private 化の実効 pin)"
        );
        assert!(src.contains(internal), "内部 fn 宣言の存在 pin");
    }

    /// 【wave 178 FX】距離 key (d*1000 as u32) 同値は sort 安定で入力順保持
    /// (front-to-back tie の厳密 pin、green-today)。
    #[test]
    fn fx_equal_distance_stable_order() {
        let b = SoftwareTileBinner::new(640, 640, 32);
        let chunks = [(4, 0), (4, 0), (4, 0)];
        let lists = b.bin_chunks(&chunks, 0.0, 0.0);
        assert_eq!(lists.len(), 1);
        assert_eq!(
            lists[0].chunk_indices,
            vec![0, 1, 2],
            "同距離同 tile は安定入力順"
        );
    }

    /// 【wave 178 FX】tile_size 下限境界の厳密 pin (green-today): 15→16
    /// (floor クランプ)・16→16・17→17。
    #[test]
    fn fx_tile_size_floor_boundaries() {
        assert_eq!(SoftwareTileBinner::new(640, 640, 15).tile_size, 16);
        assert_eq!(SoftwareTileBinner::new(640, 640, 16).tile_size, 16);
        assert_eq!(SoftwareTileBinner::new(640, 640, 17).tile_size, 17);
        // 下限に伴う tw/th: 17 px タイル → ceil(640/17)=38 (17*37=629<640)。
        let b = SoftwareTileBinner::new(640, 640, 17);
        assert_eq!(b.tile_count(), (38, 38));
    }
}
