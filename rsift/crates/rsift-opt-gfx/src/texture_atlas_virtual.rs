
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
    /// 各常駐タイルの最終アクセス論理時刻 (1-origin、`access_clock` の払い出し値)。
    /// **不変条件**: `keys(last_access) == resident` かつ値は全て一意
    /// (request_tiles / evict_lru が維持する。外部からの直接変更は不変条件を壊す)。
    pub last_access: HashMap<TileCoord, u64>,
    /// 次に払い出す論理時刻。アクセス 1 件毎に +1。
    /// u64 wrap は 2^64 回のアクセスを要し、10^9 アクセス/秒の持続でも
    /// 約 585 年かかるため到達不能 — リバース機構は不要 (証明された境界)。
    pub access_clock: u64,
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
            last_access: HashMap::new(),
            access_clock: 0,
        }
    }

    /// **契約**: 各タイルは `x*tile_size < width` かつ `y*tile_size < height`
    /// の範囲内必須 (存在しないタイルの常駐/ページ消費を fail-loud で防ぐ。
    /// u64 で評価するため u32 乗算 overflow を起こさない)。
    /// mip の有効段数は本モジュールの契約外 (呼び出し側管理)。
    ///
    /// 返却は「新たに常駐化されてストリーミングが要るタイル」のみ
    /// (入力順・重複は最初の 1 件)。ページ払い出しは free_pages の LIFO。
    ///
    /// **LRU アクセス記録 (2026-07-22 wave 40)**: 解決できた全タイル
    /// (新規常駐化・既常駐の双方) に `access_clock` を逐次採番する。
    /// 同一バッチ内でも入力順に 1 件ずつ tick するため、常駐タイル間で
    /// 時刻は必ず一意 (タイブレーク規則は存在しない)。容量枯渇でページを
    /// 得られなかったタイルは非常駐のままなので時刻も記録しない
    /// (`last_access` の不変条件 `keys == resident` を維持するため)。
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
                let Some(page) = self.free_pages.pop() else {
                    continue; // 容量枯渇: 非常駐タイルはアクセス記録しない
                };
                self.page_table.insert(tile, page);
                self.resident.insert(tile);
                to_stream.push(tile);
            }
            self.access_clock += 1;
            self.last_access.insert(tile, self.access_clock);
        }
        debug_assert_eq!(
            self.last_access.len(),
            self.resident.len(),
            "LRU 不変条件違反"
        );
        to_stream
    }

    /// 指定数のタイルを **真の LRU** (最終アクセス論理時刻の昇順) で退避して
    /// 物理ページを返却する。
    ///
    /// **2026-07-22 wave 40 根治**: wave 37 の (mip 降順, x, y 昇順) 決定的
    /// ヒューリスティックは「毎フレーム要求され続ける高 mip タイルですら
    /// 優先退避する」誤選択があり得たため、新指令「見送り・スタブ無し」に
    /// 基づき真の LRU に置換。`request_tiles` がタイル毎に単調一意の時刻を
    /// 採番するため、選択は時刻昇順のみで**完全に決定的** (タイブレークは
    /// 数学的に出現しない)。mip は退避判定に関与しない — 使用中タイルは
    /// 毎フレームの request が recency を更新するため、LRU が mip ヒューリ
    /// スティックの意図 (使用中のものを残す) を厳密に包含する。
    /// to_evict が常駐数を超える場合は全退避で頭打ち。
    /// 複雑性: O(n log n)、n = 常駐タイル数 ≤ capacity_pages。
    pub fn evict_lru(&mut self, to_evict: usize) {
        let mut tiles: Vec<(u64, TileCoord)> = self
            .resident
            .iter()
            .map(|t| (self.last_access[t], *t))
            .collect();
        tiles.sort_by_key(|&(clock, _)| clock);
        for (_, tile) in tiles.into_iter().take(to_evict) {
            if let Some(page) = self.page_table.remove(&tile) {
                self.free_pages.push(page);
            }
            self.resident.remove(&tile);
            self.last_access.remove(&tile);
        }
        debug_assert_eq!(
            self.last_access.len(),
            self.resident.len(),
            "LRU 不変条件違反"
        );
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

    /// wave 40-1 (wave 37-1 置換): 退避は**真の LRU** = 最終アクセス時刻昇順。
    /// 同一バッチ内でも入力順に逐次採番されるため (mip, 座標) は選択に関与
    /// しない。ページは退避順に free_pages へ積まれ LIFO で循環。
    #[test]
    fn evict_is_true_lru_recency_order() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 4);
        let _ = a.request_tiles(&[tile(0, 0, 0), tile(1, 0, 0), tile(0, 0, 1), tile(5, 5, 1)]);
        // ページは LIFO で 3,2,1,0 の割当、時刻は 1,2,3,4 (バッチ内入力順)
        assert_eq!(a.page_table[&tile(0, 0, 0)], 3);
        assert_eq!(a.page_table[&tile(5, 5, 1)], 0);
        assert_eq!(a.last_access[&tile(0, 0, 0)], 1);
        assert_eq!(a.last_access[&tile(5, 5, 1)], 4);
        assert_eq!(a.access_clock, 4);
        a.evict_lru(2); // 最古 2 件: (0,0,0)@1, (1,0,0)@2 — mip 0 でも古ければ退避
        assert!(!a.resident.contains(&tile(0, 0, 0)));
        assert!(!a.resident.contains(&tile(1, 0, 0)));
        assert!(a.resident.contains(&tile(0, 0, 1)));
        assert!(a.resident.contains(&tile(5, 5, 1)));
        assert!(!a.last_access.contains_key(&tile(0, 0, 0)));
        // 返却ページは退避順 (時刻昇順) に push: page 3 → page 2 → LIFO 末尾は 2
        assert_eq!(a.residency_ratio(), 0.5);
        let s = a.request_tiles(&[tile(9, 9, 1)]);
        assert_eq!(s, vec![tile(9, 9, 1)]);
        assert_eq!(a.page_table[&tile(9, 9, 1)], 2, "LIFO 末尾が再払い出し");
        assert_eq!(a.last_access[&tile(9, 9, 1)], 5);
        // 超過退避指定は全退避で頭打ち (panic しない)
        a.evict_lru(100);
        assert!(a.resident.is_empty());
        assert!(a.last_access.is_empty());
        assert_eq!(a.free_pages.len(), 4);
    }

    /// wave 40-2: **LRU の定義性質** — 最近アクセスしたタイルは退避から守られる。
    /// (wave 37 ヒューリスティックでは再要求は選択に一切影響しなかった。)
    #[test]
    fn recent_touch_protects_from_eviction() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 3);
        // 時刻 1,2,3 (バッチ内入力順)。再要求はストリーム不要でも recency 更新される
        let _ = a.request_tiles(&[tile(0, 0, 0), tile(1, 0, 0), tile(2, 0, 0)]);
        let s = a.request_tiles(&[tile(0, 0, 0)]);
        assert!(s.is_empty());
        assert_eq!(a.last_access[&tile(0, 0, 0)], 4);
        assert_eq!(a.access_clock, 4);
        a.evict_lru(2); // 最古 2 件: (1,0,0)@2, (2,0,0)@3
        assert!(a.resident.contains(&tile(0, 0, 0)), "touch 済みは生存");
        assert!(!a.resident.contains(&tile(1, 0, 0)));
        assert!(!a.resident.contains(&tile(2, 0, 0)));
        assert_eq!(a.resident.len(), 1);
        assert_eq!(a.last_access.len(), 1);
    }

    /// wave 40-3 (wave 37-2 置換): **同一履歴のリプレイは常に同一結果**
    /// (ハッシュシード非依存の完全決定性)。真の LRU では「入力順独立」は
    /// 定義上成り立たないので、決定性は履歴同一性に対して要求する。
    #[test]
    fn eviction_is_deterministic_for_identical_history() {
        let hist: [&[TileCoord]; 4] = [
            &[tile(1, 1, 0), tile(2, 2, 1)],
            &[tile(3, 3, 2)],
            &[tile(1, 1, 0)], // touch → 時刻 4
            &[tile(4, 4, 0)], // 時刻 5
        ];
        let mut a = VirtualAtlas::new(1024, 1024, 64, 8);
        let mut b = VirtualAtlas::new(1024, 1024, 64, 8);
        for h in hist {
            let _ = a.request_tiles(h);
        }
        for h in hist {
            let _ = b.request_tiles(h);
        }
        a.evict_lru(2);
        b.evict_lru(2);
        assert_eq!(a.resident, b.resident);
        assert_eq!(a.last_access, b.last_access);
        assert_eq!(a.free_pages, b.free_pages);
        // 厳密列ピン: touch で更新された (1,1,0)@4 と新着 (4,4,0)@5 が生存、
        // (2,2,1)@2, (3,3,2)@3 が時刻昇順で退避
        assert!(a.resident.contains(&tile(1, 1, 0)));
        assert!(a.resident.contains(&tile(4, 4, 0)));
        assert!(!a.resident.contains(&tile(2, 2, 1)));
        assert!(!a.resident.contains(&tile(3, 3, 2)));
        // ページは LIFO で 7,6,5,4 と割当 → 退避順 (時刻 2 → 3) に 6, 5 が返る
        assert_eq!(a.free_pages, vec![0, 1, 2, 3, 6, 5]);
    }

    /// wave 40-4: **退避選択は recency 履歴に依存する** (LRU の定義より)。
    /// wave 37-2 の「入力順独立」はヒューリスティックの欠陥を覆い隠す性質
    /// だったため、同一タイル集合・異なる recency 履歴で生存集合が変わる
    /// ことを厳密にピンする。
    #[test]
    fn evict_selection_is_recency_dependent() {
        let fwd = [tile(1, 1, 0), tile(2, 2, 1), tile(3, 3, 2), tile(4, 4, 0)];
        let mut rev = fwd;
        rev.reverse();
        let mut a = VirtualAtlas::new(1024, 1024, 64, 8);
        let mut b = VirtualAtlas::new(1024, 1024, 64, 8);
        let _ = a.request_tiles(&fwd); // 時刻: fwd[0]..[3] = 1..4
        let _ = b.request_tiles(&rev); // 時刻: rev[0]..[3] = 1..4
        a.evict_lru(2); // 先着 2 件 (1,1,0),(2,2,1) が退避
        b.evict_lru(2); // 先着 2 件 (4,4,0),(3,3,2) が退避
        assert_ne!(a.resident, b.resident, "recency が違えば生存集合も違う");
        assert!(a.resident.contains(&tile(3, 3, 2)));
        assert!(a.resident.contains(&tile(4, 4, 0)));
        assert!(b.resident.contains(&tile(1, 1, 0)));
        assert!(b.resident.contains(&tile(2, 2, 1)));
        assert_eq!(a.last_access[&tile(3, 3, 2)], 3);
        assert_eq!(b.last_access[&tile(2, 2, 1)], 3);
    }

    /// wave 40-5: 不変条件 `keys(last_access) == resident` は容量枯渇・
    /// 退避・再要求を通じて維持される (枯渇で拒否されたタイルは時刻非記録)。
    #[test]
    fn lru_invariant_keys_match_resident_across_ops() {
        let mut a = VirtualAtlas::new(1024, 1024, 64, 2);
        let s = a.request_tiles(&[tile(0, 0, 0), tile(1, 0, 0), tile(2, 0, 0)]);
        assert_eq!(s.len(), 2, "容量 2 で 3 件目は拒否");
        assert!(
            !a.last_access.contains_key(&tile(2, 0, 0)),
            "拒否タイルは時刻を持たない"
        );
        assert_eq!(a.access_clock, 2, "拒否タイルは tick しない");
        a.evict_lru(1); // (0,0,0)@1 (page 1) を退避 → free_pages=[1]
        assert!(!a.last_access.contains_key(&tile(0, 0, 0)));
        let s = a.request_tiles(&[tile(2, 0, 0)]); // page 1 を再取得、時刻 3
        assert_eq!(s, vec![tile(2, 0, 0)]);
        assert_eq!(a.page_table[&tile(2, 0, 0)], 1);
        assert_eq!(a.last_access[&tile(2, 0, 0)], 3);
        assert!(a.last_access.keys().all(|k| a.resident.contains(k)));
        assert!(a.resident.iter().all(|k| a.last_access.contains_key(k)));
        assert_eq!(a.last_access.len(), a.resident.len());
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
