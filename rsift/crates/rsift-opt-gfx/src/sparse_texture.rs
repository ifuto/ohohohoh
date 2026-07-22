//! Virtual (sparse) texturing — zero-allocation intrusive LRU page-table residency (`IntrusiveLruPageTable`).
//!
//! 16K×16K テクスチャ等の広大な仮想アドレス空間において、画面上に実際に可視なタイルのみを
//! `max_physical` スロットの物理 VRAM に割り当て、上限超過時は完全 $O(1)$ の侵入型
//! 双方向リストで LRU ページを即座に退避・入れ替える。`VecDeque` ヒープ確保ゼロ。

use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
struct LruNode {
    prev: u32,
    next: u32,
    key: (u32, u32), // (page_id, mip)
}

pub struct SparsePageTable {
    pub max_physical: u32,
    resident: HashMap<(u32, u32), u32>,
    nodes: Vec<LruNode>,
    head: u32,
    tail: u32,
}

impl SparsePageTable {
    pub fn new(max_physical: u32) -> Self {
        assert!(max_physical > 0, "max_physical must be at least 1");
        let cap = max_physical as usize;
        let mut nodes = Vec::with_capacity(cap);
        for _ in 0..cap {
            nodes.push(LruNode {
                prev: u32::MAX,
                next: u32::MAX,
                key: (0, 0),
            });
        }
        Self {
            max_physical,
            resident: HashMap::with_capacity(cap),
            nodes,
            head: u32::MAX,
            tail: u32::MAX,
        }
    }

    /// 常駐照会。**LRU 順は更新しない**純粋クエリ (&self)。
    /// 「見たら MRU に昇格」させたい場合は [`Self::request`] を使うこと。
    pub fn is_resident(&self, page: u32, mip: u32) -> bool {
        self.resident.contains_key(&(page, mip))
    }

    pub fn resident_count(&self) -> usize {
        self.resident.len()
    }

    /// Touch existing physical slot: detach and move to front of MRU list in $O(1)$.
    fn touch(&mut self, slot: u32) {
        if self.head == slot {
            return;
        }
        self.detach(slot);
        self.push_front(slot);
    }

    fn detach(&mut self, slot: u32) {
        let p = self.nodes[slot as usize].prev;
        let n = self.nodes[slot as usize].next;
        if p != u32::MAX {
            self.nodes[p as usize].next = n;
        } else {
            self.head = n;
        }
        if n != u32::MAX {
            self.nodes[n as usize].prev = p;
        } else {
            self.tail = p;
        }
    }

    fn push_front(&mut self, slot: u32) {
        self.nodes[slot as usize].prev = u32::MAX;
        self.nodes[slot as usize].next = self.head;
        if self.head != u32::MAX {
            self.nodes[self.head as usize].prev = slot;
        } else {
            self.tail = slot;
        }
        self.head = slot;
    }

    /// Request a page. Returns its physical slot, evicting the LRU resident page in $O(1)$ when full.
    ///
    /// GPU ページテーブル配線側は退避発生時に「どの key が消えたか」を
    /// 必ず必要とする (旧マッピングを unmap してから新規 map を貼るため)。
    /// その情報が要る場合は [`Self::request_with_eviction`] を使うこと。
    pub fn request(&mut self, page: u32, mip: u32) -> u32 {
        self.request_with_eviction(page, mip).0
    }

    /// [`Self::request`] の完全版: (確保スロット, 退避された (page, mip))
    /// を返す。退避なし (空きスロット充当・既常駐 touch) なら第 2 要素は
    /// `None`。
    ///
    /// **dense-slot 不変条件 (2026-07-23 wave 42 明文化)**: スロットは
    /// 常に `[0, resident.len())` に稠密に使用中。帰納: 空き時の新規確保は
    /// 丁度 `resident.len()` をスロットに選び +1、満杯時の退避再使用は
    /// len を不変に保つため初期状態から常に成立する。O(1) 新規確保の
    /// 根拠であり、これが破れると未使用スロットを叩く。
    pub fn request_with_eviction(&mut self, page: u32, mip: u32) -> (u32, Option<(u32, u32)>) {
        let key = (page, mip);
        if let Some(&slot) = self.resident.get(&key) {
            self.touch(slot);
            return (slot, None);
        }

        let (slot, evicted) = if self.resident.len() < self.max_physical as usize {
            let slot = self.resident.len() as u32;
            debug_assert!(
                (slot as usize) < self.nodes.len(),
                "dense-slot 不変条件違反"
            );
            (slot, None)
        } else {
            let lru_slot = self.tail;
            let old_key = self.nodes[lru_slot as usize].key;
            self.resident.remove(&old_key);
            self.detach(lru_slot);
            (lru_slot, Some(old_key))
        };

        self.nodes[slot as usize].key = key;
        self.resident.insert(key, slot);
        self.push_front(slot);
        (slot, evicted)
    }

    /// 距離から mip 段を選ぶ**ヒューリスティック** (厳密 texel:pixel 式では
    /// ないことを正直化する。2026-07-23 wave 42)。
    ///
    /// 式: `mip = clamp(ceil(log2(screen_h / max(apparent_px, 1))), 0, max_mip)`
    /// where `apparent_px = (world_size / max(distance, 1e-3)) * screen_h`。
    /// 暗黙仮定: (a) 視野角は 2·tan(fov/2) = 1 つまり **fov ≈ 53.13°** に
    /// 固定 (本来の見掛け高は `screen_h·size/(2·d·tan(fov/2))`)、(b) mip の
    /// 分子はテクスチャ texel 数ではなく `screen_h` を代理に使う。ゆえに
    /// 本値は「画面高の何分の1に見えるか」の対数ヒューリスティックであり、
    /// max_mip 側にのみ厳密な飽和保証がある。
    ///
    /// **契約 (wave 42 厳格化)**: `distance` は NaN 禁止 (±∞ は受理、
    /// +∞ → max_mip、−∞ → 0 に飽和)。`world_size`/`screen_h` は正の有限値
    /// 必須。旧実装は NaN を `.max(f)` が非 NaN 側を返す性質で**静寂に
    /// 退避経路へ着地させていた** (観測欠測の静寂混入、fail-loud 化)。
    pub fn mip_for_distance(distance: f32, world_size: f32, screen_h: f32, max_mip: u32) -> u32 {
        assert!(
            !distance.is_nan(),
            "mip_for_distance 契約違反: distance が NaN"
        );
        assert!(
            world_size.is_finite() && world_size > 0.0,
            "mip_for_distance 契約違反: world_size は正の有限値必須 ({world_size})"
        );
        assert!(
            screen_h.is_finite() && screen_h > 0.0,
            "mip_for_distance 契約違反: screen_h は正の有限値必須 ({screen_h})"
        );
        let screen_fraction = (world_size / distance.max(1e-3)) * screen_h;
        let mip = (screen_h / screen_fraction.max(1.0)).log2().ceil();
        mip.clamp(0.0, max_mip as f32) as u32
    }

    pub fn wgsl_source(&self) -> &'static str {
        SPARSE_TEXTURE_WGSL
    }
}

/// ページテーブルの「未常駐」センチネル (WGSL 側の `INVALID` と同一値)。
pub const INVALID_SLOT: u32 = u32::MAX;

/// `shaders/sparse_texture.wgsl` のアドレス変換カーネルと**同一規則**の
/// CPU ミラー (2026-07-23 wave 42 で実カーネル化と同時導入)。
///
/// 規則: `(page, mip)` を直接照会 → 未常駐なら **最細 resident 祖先 mip**
/// (mip-1, mip-2, …, 0 の順に最初の常駐) へ退化 → 全段未常駐または
/// 範囲外 (page ≥ page_count / mip > max_mip) なら `INVALID_SLOT`。
///
/// **契約**: `page_table.len() >= page_count * (max_mip + 1)` 必須
/// (GPU 側 binding もこの上傳契約で破綻しない)。`max_mip`/`page_count`
/// の 0 も合法 (その場合の範囲は自明に決まる)。
pub fn translate_with_fallback(
    page_table: &[u32],
    max_mip: u32,
    page_count: u32,
    page: u32,
    mip: u32,
) -> u32 {
    assert!(
        page_table.len() >= page_count as usize * (max_mip as usize + 1),
        "translate_with_fallback 契約違反: page_table 長 {} < {} * {} (不足)",
        page_table.len(),
        page_count,
        max_mip as usize + 1
    );
    if page >= page_count || mip > max_mip {
        return INVALID_SLOT;
    }
    let stride = max_mip as usize + 1;
    let base = page as usize * stride;
    let direct = page_table[base + mip as usize];
    if direct != INVALID_SLOT {
        return direct;
    }
    for k in 1..=mip {
        let cand = page_table[base + (mip - k) as usize];
        if cand != INVALID_SLOT {
            return cand;
        }
    }
    INVALID_SLOT
}

pub const SPARSE_TEXTURE_WGSL: &str = include_str!("../shaders/sparse_texture.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_allocates_and_resident() {
        let mut t = SparsePageTable::new(4);
        let s = t.request(1, 0);
        assert!(t.is_resident(1, 0));
        assert_eq!(s, 0);
        assert_eq!(t.resident_count(), 1);
    }

    #[test]
    fn evicts_lru_when_full() {
        let mut t = SparsePageTable::new(2);
        t.request(1, 0); // slot 0
        t.request(2, 0); // slot 1
        let s = t.request(3, 0); // evicts LRU = page 1
        assert_eq!(s, 0); // reuses slot 0
        assert!(!t.is_resident(1, 0));
        assert!(t.is_resident(2, 0));
        assert!(t.is_resident(3, 0));
        assert_eq!(t.resident_count(), 2);
    }

    #[test]
    fn touch_promotes_mru() {
        let mut t = SparsePageTable::new(2);
        t.request(1, 0);
        t.request(2, 0);
        t.request(1, 0); // promote page 1
        t.request(3, 0); // evict page 2 instead of page 1
        assert!(t.is_resident(1, 0));
        assert!(!t.is_resident(2, 0));
        assert!(t.is_resident(3, 0));
    }

    /// wave 42-1: dense 充填・MRU 昇格・退避再使用のスロット厳密列ピン。
    #[test]
    fn lru_exact_slot_sequence_pinned() {
        let mut t = SparsePageTable::new(3);
        // dense 充填はスロット 0,1,2 の順 (dense-slot 不変条件の帰納基底)
        assert_eq!(t.request_with_eviction(10, 0), (0, None));
        assert_eq!(t.request_with_eviction(11, 0), (1, None));
        assert_eq!(t.request_with_eviction(12, 0), (2, None));
        // LRU リスト: head 12 → 11 → 10 tail。中間 (11) の touch で先頭へ
        assert_eq!(t.request_with_eviction(11, 0), (1, None));
        // この時点の tail は 10 → (10,0) が退避されスロット 0 が再使用される
        assert_eq!(t.request_with_eviction(13, 0), (0, Some((10, 0))));
        assert!(!t.is_resident(10, 0));
        assert!(t.is_resident(11, 0));
        assert!(t.is_resident(12, 0));
        assert!(t.is_resident(13, 0));
        // リスト: head 13 → 11 → 12 tail → 次の退避は (12,0) → スロット 2
        assert_eq!(t.request_with_eviction(14, 0), (2, Some((12, 0))));
        assert!(!t.is_resident(12, 0));
        assert_eq!(t.resident_count(), 3);
    }

    /// wave 42-2: 退避通知は key (page, mip) の完全一致で返る
    /// (mip が異なれば別エントリ)。
    #[test]
    fn request_with_eviction_reports_exact_key() {
        let mut t = SparsePageTable::new(2);
        assert_eq!(t.request_with_eviction(1, 0), (0, None));
        assert_eq!(t.request_with_eviction(2, 0), (1, None));
        assert_eq!(t.request_with_eviction(1, 0), (0, None)); // touch、退避なし
        assert_eq!(t.request_with_eviction(3, 1), (1, Some((2, 0))));
        assert!(!t.is_resident(2, 0));
        assert!(t.is_resident(3, 1));
        assert!(!t.is_resident(3, 0), "key は (page, mip) 完全一致");
    }

    /// wave 42-3: WGSL カーネルの CPU ミラー — 直接/祖先退化/全段未常駐/
    /// 範囲外の厳密表。
    #[test]
    fn translate_with_fallback_exact_table() {
        let tbl = [
            5,
            u32::MAX,
            7, // page 0: mip0=5, mip1 未常駐, mip2=7
            u32::MAX,
            u32::MAX,
            u32::MAX, // page 1: 全段未常駐
        ];
        assert_eq!(translate_with_fallback(&tbl, 2, 2, 0, 0), 5);
        assert_eq!(
            translate_with_fallback(&tbl, 2, 2, 0, 1),
            5,
            "最細祖先 mip0 へ退化"
        );
        assert_eq!(translate_with_fallback(&tbl, 2, 2, 0, 2), 7);
        assert_eq!(translate_with_fallback(&tbl, 2, 2, 1, 0), u32::MAX);
        assert_eq!(translate_with_fallback(&tbl, 2, 2, 1, 2), u32::MAX);
        assert_eq!(
            translate_with_fallback(&tbl, 2, 2, 2, 0),
            u32::MAX,
            "page 範囲外"
        );
        assert_eq!(
            translate_with_fallback(&tbl, 2, 2, 0, 3),
            u32::MAX,
            "mip 範囲外"
        );
    }

    /// wave 42-4: 性質オラクル — 結果は「要求 mip 以下で常駐する最大 mip の
    /// slot」に等しいことを、独立実装 (rev-find) で全 (page, mip) 突合。
    #[test]
    fn translate_fallback_is_finest_resident_ancestor_oracle() {
        let mut tbl = vec![u32::MAX; 8 * 5];
        let mut s: u64 = 0x9E3779B97F4A7C15;
        for v in tbl.iter_mut() {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            if s >> 61 == 0 {
                *v = (s >> 32) as u32 & 0x3FF; // 約 1/8 が常駐 (u32::MAX と衝突しない値域)
            }
        }
        for page in 0..8u32 {
            for mip in 0..=4u32 {
                let got = translate_with_fallback(&tbl, 4, 8, page, mip);
                let want = (0..=mip)
                    .rev()
                    .map(|m| tbl[page as usize * 5 + m as usize])
                    .find(|&v| v != u32::MAX)
                    .unwrap_or(u32::MAX);
                assert_eq!(got, want, "page={page} mip={mip}");
            }
        }
    }

    /// wave 42-5: page_table 長不足は fail-loud。
    #[test]
    #[should_panic(expected = "translate_with_fallback 契約違反")]
    fn translate_rejects_short_table() {
        let _ = translate_with_fallback(&[0u32; 3], 2, 2, 0, 0); // 要 2*(2+1)=6
    }

    /// wave 42-6: mip_for_distance の構造的厳密値列
    /// (log2 が冪・単調関係で正確に決まる点)。
    #[test]
    fn mip_for_distance_exact_structure() {
        // apparent=1080 → 1080/1080=1 → log2(1)=0 → mip 0
        assert_eq!(
            SparsePageTable::mip_for_distance(256.0, 256.0, 1080.0, 4),
            0
        );
        // apparent=270 → 1080/270=4.0 (厳密) → log2(4)=2
        assert_eq!(
            SparsePageTable::mip_for_distance(1024.0, 256.0, 1080.0, 4),
            2
        );
        // apparent=67.5 → 1080/67.5=16.0 (厳密) → log2(16)=4 → clamp max_mip
        assert_eq!(
            SparsePageTable::mip_for_distance(4096.0, 256.0, 1080.0, 4),
            4
        );
        // +∞ → apparent 0 → max(0,1)=1 → log2(1080)≈10.08 → ceil 11 → clamp 4
        assert_eq!(
            SparsePageTable::mip_for_distance(f32::INFINITY, 256.0, 1080.0, 4),
            4
        );
        // 至近 (距離 0 → 1e-3 飽和): apparent ≈ 2.76e8 → 商 < 1 → log2 < 0 → 0
        assert_eq!(SparsePageTable::mip_for_distance(0.0, 256.0, 1080.0, 4), 0);
        // −∞ も 1e-3 飽和側 → 0
        assert_eq!(
            SparsePageTable::mip_for_distance(f32::NEG_INFINITY, 256.0, 1080.0, 4),
            0
        );
    }

    /// wave 42-7: NaN / 非正の寸法は fail-loud (観測欠測の静寂着地を根治)。
    #[test]
    #[should_panic(expected = "distance が NaN")]
    fn mip_rejects_nan_distance() {
        let _ = SparsePageTable::mip_for_distance(f32::NAN, 256.0, 1080.0, 4);
    }

    #[test]
    #[should_panic(expected = "world_size は正の有限値必須")]
    fn mip_rejects_zero_world_size() {
        let _ = SparsePageTable::mip_for_distance(1024.0, 0.0, 1080.0, 4);
    }

    #[test]
    #[should_panic(expected = "screen_h は正の有限値必須")]
    fn mip_rejects_nan_screen_h() {
        let _ = SparsePageTable::mip_for_distance(1024.0, 256.0, f32::NAN, 4);
    }

    /// wave 42-8: WGSL 実カーネルの entry/構造体レイアウト/binding を
    /// naga 実機でピン (U: maxMip@0, pageCount@4, span=8)。
    #[test]
    fn wgsl_entry_layout_and_bindings_pinned() {
        let module = naga::front::wgsl::parse_str(SPARSE_TEXTURE_WGSL).expect("WGSL must parse");
        let ep = module
            .entry_points
            .iter()
            .find(|ep| ep.name == "main")
            .expect("main entry missing");
        assert_eq!(ep.stage, naga::ShaderStage::Compute);
        assert_eq!(ep.workgroup_size, [8, 8, 1]);
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("U"))
            .expect("U struct missing");
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("U is not a struct");
        };
        assert_eq!(*span, 8);
        let off = |name: &str| {
            members
                .iter()
                .find(|m| m.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("U.{name} missing"))
                .offset
        };
        assert_eq!(off("maxMip"), 0);
        assert_eq!(off("pageCount"), 4);
        let mut got: Vec<(u32, u32)> = module
            .global_variables
            .iter()
            .filter_map(|(_, g)| g.binding.as_ref().map(|b| (b.group, b.binding)))
            .collect();
        got.sort_unstable();
        assert_eq!(got, vec![(0, 0), (0, 1), (0, 2)]);
    }
}
