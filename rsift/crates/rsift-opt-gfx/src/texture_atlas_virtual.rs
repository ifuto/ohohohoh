
//! Sparse Virtual Texture Atlas - VRAM 100MB以下でも高解像度リソパ対応
//! タイル64x64、不要タイルは未常駐、必要時にストリーミング。

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileCoord { pub x: u32, pub y: u32, pub mip: u8 }

pub struct VirtualAtlas {
    pub width: u32,
    pub height: u32,
    pub tile_size: u32,
    pub resident: HashSet<TileCoord>,
    pub page_table: HashMap<TileCoord, u32>, // tile -> physical page index
    pub free_pages: Vec<u32>,
    pub capacity_pages: u32,
}

impl VirtualAtlas {
    /// `width`/`height`/`tile_size`/`capacity_pages` は全て 1 以上必須
    /// (0 の寸法や 0 容量のアトラスは無意味で、`residency_ratio` の
    /// 0 除算 NaN 静寂化を防ぐため fail-loud。2026-07-22 wave 37 契約化)。
    pub fn new(width: u32, height: u32, tile_size: u32, capacity_pages: u32) -> Self {
        assert!(
            width >= 1 && height >= 1 && tile_size >= 1 && capacity_pages >= 1,
            "VirtualAtlas::new 契約違反: 全引数は 1 以上必須 \
             (width={width}, height={height}, tile_size={tile_size}, capacity_pages={capacity_pages})"
        );
        Self {
            width, height, tile_size,
            resident: HashSet::new(),
            page_table: HashMap::new(),
            free_pages: (0..capacity_pages).collect(),
            capacity_pages,
        }
    }

    /// **契約**: 各タイルは `x*tile_size < width` かつ `y*tile_size < height`
    /// の範囲内必須 (存在しないタイルの常駐/ページ消費を fail-loud で防ぐ。
    /// u64 で評価するため u32 乗算 overflow を起こさない)。
    /// mip の有効段数は本モジュールの契約外 (呼び出し側管理)。
    ///
    /// 返却は「新たに常駐化されてストリーミングが要るタイル」のみ
    /// (入力順・重複は最初の 1 件)。ページ払い出しは free_pages の LIFO。
    pub fn request_tiles(&mut self, needed: &[TileCoord]) -> Vec<TileCoord> {
        let mut to_stream = Vec::new();
        for &tile in needed {
            assert!(
                (tile.x as u64) * (self.tile_size as u64) < self.width as u64
                    && (tile.y as u64) * (self.tile_size as u64) < self.height as u64,
                "request_tiles 契約違反: タイル範囲外 ({}, {}, mip {}) — atlas {}x{} tile {}",
                tile.x,
                tile.y,
                tile.mip,
                self.width,
                self.height,
                self.tile_size
            );
            if !self.resident.contains(&tile) {
                if let Some(page) = self.free_pages.pop() {
                    self.page_table.insert(tile, page);
                    self.resident.insert(tile);
                    to_stream.push(tile);
                }
            }
        }
        to_stream
    }

    /// 指定数のタイルを退避して物理ページを返却する。
    ///
    /// **正直な注記 (2026-07-22 wave 37 訂正)**: 名前は lru だが LRU 追跡は
    /// **実装されていない**。旧実装は `HashSet::iter().take()` での「任意
    /// 選択」だったが、これはハッシュシード由来で**プロセスごとに非決定**
    /// (再現性なし)。本実装は (mip 降順, x, y 昇順) の**決定的ヒューリス
    /// ティック** — 低詳細 mip から優先退避させる。真の LRU (アクセス時刻
    /// 追跡) は residency 需要が出た段階で導入する。to_evict が常駐数を
    /// 超える場合は全退避で頭打ち。
    pub fn evict_lru(&mut self, to_evict: usize) {
        let mut tiles: Vec<TileCoord> = self.resident.iter().copied().collect();
        tiles.sort_by_key(|t| (std::cmp::Reverse(t.mip), t.x, t.y));
        for tile in tiles.into_iter().take(to_evict) {
            if let Some(page) = self.page_table.remove(&tile) {
                self.free_pages.push(page);
            }
            self.resident.remove(&tile);
        }
    }

    pub fn residency_ratio(&self) -> f32 {
        self.resident.len() as f32 / self.capacity_pages as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: u32, y: u32, mip: u8) -> TileCoord {
        TileCoord { x, y, mip }
    }

    #[test]
    fn new_initializes_full_free_pool() {
        let a = VirtualAtlas::new(1024, 1024, 64, 4);
        assert_eq!(a.free_pages, (0..4).collect::<Vec<u32>>());
        assert!(a.resident.is_empty());
        assert!(a.page_table.is_empty());
        assert_eq!(a.residency_ratio(), 0.0);
    }

    #[test]
    fn request_allocates_lifo_pages_and_is_idempotent() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 4);
        let s = a.request_tiles(&[tile(0, 0, 0), tile(1, 0, 0)]);
        assert_eq!(s, vec![tile(0, 0, 0), tile(1, 0, 0)]);
        // free_pages は Vec::pop (末尾=LIFO) → 3, 2 の順で払い出し。
        assert_eq!(a.page_table[&tile(0, 0, 0)], 3);
        assert_eq!(a.page_table[&tile(1, 0, 0)], 2);
        // 常駐済みの再要求は新規払い出し無し。
        assert!(a.request_tiles(&[tile(0, 0, 0)]).is_empty());
        assert_eq!(a.free_pages.len(), 2);
        // 同一バッチ内の重複タイルは常駐化により 1 件に dedupe。
        let dup = a.request_tiles(&[tile(9, 9, 0), tile(9, 9, 0)]);
        assert_eq!(dup, vec![tile(9, 9, 0)]);
        assert_eq!(a.free_pages.len(), 1);
        assert_eq!(a.residency_ratio(), 0.75);
    }

    #[test]
    fn capacity_exhaustion_rejects_overflow() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 2);
        let s = a.request_tiles(&[tile(0, 0, 0), tile(1, 0, 0), tile(2, 0, 0)]);
        assert_eq!(s.len(), 2);
        assert!(a.resident.contains(&tile(0, 0, 0)));
        assert!(!a.resident.contains(&tile(2, 0, 0)));
        assert!(a.page_table.get(&tile(2, 0, 0)).is_none());
    }

    #[test]
    fn evict_lru_returns_pages_and_allows_restream() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 2);
        let _ = a.request_tiles(&[tile(0, 0, 0), tile(1, 0, 0)]);
        a.evict_lru(2);
        assert!(a.resident.is_empty());
        assert!(a.page_table.is_empty());
        assert_eq!(a.free_pages.len(), 2);
        assert_eq!(a.residency_ratio(), 0.0);
        // 解放後は同じタイルが再ストリーミング可能。
        assert_eq!(a.request_tiles(&[tile(0, 0, 0)]), vec![tile(0, 0, 0)]);
    }

    /// wave 37-1: 退避は (mip 降順, x, y 昇順) の**決定的**ヒューリスティック
    /// (旧 HashSet 反復順 = プロセス非決定を根治)。ページは LIFO で循環。
    #[test]
    fn evict_is_deterministic_low_mip_first() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 4);
        let _ = a.request_tiles(&[tile(0, 0, 0), tile(1, 0, 0), tile(0, 0, 1), tile(5, 5, 1)]);
        // 4 タイル常駐、ページは LIFO で 3,2,1,0 の割当
        assert_eq!(a.page_table[&tile(5, 5, 1)], 0);
        a.evict_lru(2); // mip 1 の 2 タイル (低詳細) が優先退避
        assert!(a.resident.contains(&tile(0, 0, 0)));
        assert!(a.resident.contains(&tile(1, 0, 0)));
        assert!(!a.resident.contains(&tile(0, 0, 1)));
        assert!(!a.resident.contains(&tile(5, 5, 1)));
        // 返却ページは退避順 (mip desc,x,y) で free_pages に積まれる:
        // tile(0,0,1)→page 1, tile(5,5,1)→page 0 の順 push → 末尾が 0
        assert_eq!(a.residency_ratio(), 0.5);
        let s = a.request_tiles(&[tile(9, 9, 1)]);
        assert_eq!(s, vec![tile(9, 9, 1)]);
        assert_eq!(a.page_table[&tile(9, 9, 1)], 0, "LIFO 末尾が再払い出し");
        // 超過退避指定は全退避で頭打ち (panic しない)
        a.evict_lru(100);
        assert!(a.resident.is_empty());
        assert_eq!(a.free_pages.len(), 4);
    }

    /// wave 37-2: 退避順序は入力順・ハッシュシードに依らず同一
    /// (同じ最終状態を 2 経路で構成して比較)。
    #[test]
    fn evict_selection_is_input_order_independent() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 8);
        let mut b = VirtualAtlas::new(1024, 1024, 64, 8);
        let fwd = [tile(1, 1, 0), tile(2, 2, 1), tile(3, 3, 2), tile(4, 4, 0)];
        let mut rev = fwd;
        rev.reverse();
        let _ = a.request_tiles(&fwd);
        let _ = b.request_tiles(&rev);
        a.evict_lru(2);
        b.evict_lru(2);
        assert_eq!(a.resident, b.resident, "退避対象は挿入順に依らない");
    }

    /// wave 37-3: コンストラクタ引数 0 は fail-loud。
    #[test]
    #[should_panic(expected = "VirtualAtlas::new 契約違反")]
    fn new_rejects_zero_capacity() {
        let _ = VirtualAtlas::new(1024, 1024, 64, 0);
    }

    /// wave 37-4: タイル範囲外は fail-loud (x/y 各軸 + overflow 安全の確認)。
    #[test]
    #[should_panic(expected = "request_tiles 契約違反")]
    fn request_rejects_out_of_range_x() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 4);
        let _ = a.request_tiles(&[tile(16, 0, 0)]); // 16*64 == width → 範囲外
    }

    #[test]
    #[should_panic(expected = "request_tiles 契約違反")]
    fn request_rejects_out_of_range_y() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 4);
        let _ = a.request_tiles(&[tile(0, 16, 0)]);
    }

    #[test]
    #[should_panic(expected = "request_tiles 契約違反")]
    fn request_rejects_huge_coord_without_overflow_wrap() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 4);
        // u32 乗算なら x*tile_size が wrap して範囲内に化け得る → u64 評価で拒否
        let _ = a.request_tiles(&[tile(u32::MAX, 0, 0)]);
    }
}
