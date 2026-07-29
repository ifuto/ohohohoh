//! Slab + Object Pool + Generational Allocator for RenderSection.
//!
//! ポインタの付け替えと侵入型フリーリスト（Intrusive Free List）を用いた
//! Cache-Line アライン・世代管理付き（ABA バグ防止）スラブ＆オブジェクトプールを完全実装。
//! `VecDeque` による余分なヒープ確保を全廃。
//!
//! 【wave 156 FB 監査注記】
//! 1. **捕捉 74 (wiring 側)**: 旧 wiring は `slot == usize::MAX` を満杯と
//!    判定していたが、`Slab::alloc` は無制限成長 (Vec push) で MAX
//!    sentinel を返しえない (64-bit では u32 index 由来で到達不能)。
//!    満杯概念は存在しない無制限契約が module の真 — wiring は key
//!    ライフサイクル化 (初見 key のみ alloc・退去 key を free) で根治、
//!    vacuous 分岐は除去 (挙動同一)。module 側契約 pin は
//!    `alloc_unbounded_sequence_golden_strict`。
//! 2. §7 消化 19: 旧 wiring は alloc のみで free/get/len/is_empty/
//!    with_capacity/ObjectPool が全消費者ゼロ → wiring へ真消費者創出
//!    (evict free・material read-back get・report 4 フィールド・HUD
//!    scratch ObjectPool リサイクル・integrity debug_assert)。
//! 3. 捕捉 75: `insert` で free_head 非 sentinel なのに指先が Vacant
//!    でない場合の silent フォールスルー (内部不変式違反を静寂マスクし
//!    push 増長する経路) を debug_assert で不変式明示 (外部 API からは
//!    到達不能、挙動同一)。
//! 4. generation は u32 wrapping — 同一 slot が 2^32 回 reuse されると
//!    旧 handle と世代衝突しうる (ABA 境界)。60fps 全 tick 単一 slot
//!    reuse でも ~2.2 年で到達不能 (wave 153 jitter wrap 同型の誠実注記)。
//! 5. `Slab::get_mut` / `GenerationalSlab::get_mut` 削除: 変異消費経路が
//!    crates 全域に存在せず (census grep)、捏造した vacuous 書き込みは
//!    偽装禁止抵触のため完全削除 (不可能証明: mat は frame snapshot で
//!    不変、wiring の read-back は get のみ)。
//! 6. `Slab` は `GenerationalSlab` への薄い index 互換層 — idx_to_handle
//!    は free 時 take で二重 free 安全 (Some→None)、reuse 時に再 Some。
//!    LIFO 復帰順は `free_lifo_reuse_golden_strict` が pin (rq fb_slab)。

use std::marker::PhantomData;

/// 世代管理付きスラブハンドル（ABA 衝突と Use-After-Free を完全防止）。
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct SlabHandle<T> {
    pub index: u32,
    pub generation: u32,
    _marker: PhantomData<T>,
}

impl<T> Clone for SlabHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for SlabHandle<T> {}

impl<T> SlabHandle<T> {
    #[inline]
    pub fn new(index: u32, generation: u32) -> Self {
        Self {
            index,
            generation,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone)]
enum Slot<T> {
    Vacant { next_free: u32, generation: u32 },
    Occupied { value: T, generation: u32 },
}

/// Cache-line aligned Generational Slab with zero-allocation intrusive free list.
#[repr(C, align(64))]
pub struct GenerationalSlab<T> {
    slots: Vec<Slot<T>>,
    free_head: u32,
    occupied_count: usize,
}

impl<T> GenerationalSlab<T> {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_head: u32::MAX,
            occupied_count: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            free_head: u32::MAX,
            occupied_count: 0,
        }
    }

    pub fn insert(&mut self, value: T) -> SlabHandle<T> {
        self.occupied_count += 1;
        if self.free_head != u32::MAX {
            let idx = self.free_head as usize;
            // 捕捉 75 (wave 156): free chain は remove で Vacant のみ繋ぐ
            // ため不変式として指先は必ず Vacant — 違反を静寂マスクして
            // push 増長する旧フォールスルーを debug_assert で明示する
            // (公開 API からは到達不能、挙動同一)。
            debug_assert!(
                matches!(self.slots[idx], Slot::Vacant { .. }),
                "free_head は必ず Vacant を指す (侵入型 free list 不変式)"
            );
            if let Slot::Vacant {
                next_free,
                generation,
            } = self.slots[idx]
            {
                self.free_head = next_free;
                self.slots[idx] = Slot::Occupied { value, generation };
                return SlabHandle::new(idx as u32, generation);
            }
        }

        let idx = self.slots.len() as u32;
        self.slots.push(Slot::Occupied { value, generation: 0 });
        SlabHandle::new(idx, 0)
    }

    pub fn remove(&mut self, handle: SlabHandle<T>) -> Option<T> {
        let idx = handle.index as usize;
        if idx >= self.slots.len() {
            return None;
        }
        match self.slots[idx] {
            Slot::Occupied { ref generation, .. } if *generation == handle.generation => {
                let next_gen = generation.wrapping_add(1);
                let old_slot = std::mem::replace(
                    &mut self.slots[idx],
                    Slot::Vacant {
                        next_free: self.free_head,
                        generation: next_gen,
                    },
                );
                self.free_head = idx as u32;
                self.occupied_count -= 1;
                if let Slot::Occupied { value, .. } = old_slot {
                    Some(value)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    #[inline]
    pub fn get(&self, handle: SlabHandle<T>) -> Option<&T> {
        let idx = handle.index as usize;
        match self.slots.get(idx) {
            Some(Slot::Occupied { value, generation }) if *generation == handle.generation => {
                Some(value)
            }
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.occupied_count
    }

    pub fn is_empty(&self) -> bool {
        self.occupied_count == 0
    }
}

/// Back-compat index-based `Slab<T>` without `VecDeque` overhead.
pub struct Slab<T> {
    inner: GenerationalSlab<T>,
    idx_to_handle: Vec<Option<SlabHandle<T>>>,
}

impl<T> Slab<T> {
    pub fn new() -> Self {
        Self {
            inner: GenerationalSlab::new(),
            idx_to_handle: Vec::new(),
        }
    }

    /// slot 配列を事前確保 (Vec::with_capacity 準拠、reserve のみで
    /// 挙動は new() と同一)。【wave 156 §7 消化 19】chunk universe 規模の
    /// 事前確保で push 時の再確保を避ける本来用途を wiring に接続。
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: GenerationalSlab::with_capacity(capacity),
            idx_to_handle: Vec::new(),
        }
    }

    /// 占有 slot 数 (inner の実カウンタ伝達)。
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn alloc(&mut self, val: T) -> usize {
        let handle = self.inner.insert(val);
        let idx = handle.index as usize;
        if idx >= self.idx_to_handle.len() {
            self.idx_to_handle.resize(idx + 1, None);
        }
        self.idx_to_handle[idx] = Some(handle);
        idx
    }

    pub fn free(&mut self, idx: usize) -> Option<T> {
        if let Some(Some(handle)) = self.idx_to_handle.get_mut(idx).map(|h| h.take()) {
            self.inner.remove(handle)
        } else {
            None
        }
    }

    #[inline]
    pub fn get(&self, idx: usize) -> Option<&T> {
        if let Some(Some(handle)) = self.idx_to_handle.get(idx) {
            self.inner.get(*handle)
        } else {
            None
        }
    }
}

/// Pre-allocated cache-friendly Object Pool with zero runtime allocation overhead.
#[repr(C, align(64))]
pub struct ObjectPool<T> {
    pool: Vec<T>,
}

impl<T: Default> ObjectPool<T> {
    pub fn new(cap: usize) -> Self {
        let mut pool = Vec::with_capacity(cap);
        for _ in 0..cap {
            pool.push(T::default());
        }
        Self { pool }
    }

    #[inline]
    pub fn acquire(&mut self) -> Option<T> {
        self.pool.pop()
    }

    #[inline]
    pub fn release(&mut self, obj: T) {
        self.pool.push(obj);
    }

    pub fn available(&self) -> usize {
        self.pool.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generational_slab() {
        let mut slab = GenerationalSlab::new();
        let h1 = slab.insert(100);
        let h2 = slab.insert(200);

        assert_eq!(slab.get(h1), Some(&100));
        assert_eq!(slab.get(h2), Some(&200));

        assert_eq!(slab.remove(h1), Some(100));
        assert_eq!(slab.get(h1), None);

        let h3 = slab.insert(300);
        assert_eq!(h3.index, h1.index);
        assert_ne!(h3.generation, h1.generation);
        assert_eq!(slab.get(h3), Some(&300));
        assert_eq!(slab.get(h1), None);
    }

    #[test]
    fn test_compat_slab() {
        let mut slab = Slab::new();
        let i1 = slab.alloc("hello");
        let i2 = slab.alloc("world");
        assert_eq!(slab.get(i1), Some(&"hello"));
        assert_eq!(slab.free(i1), Some("hello"));
        assert_eq!(slab.get(i1), None);
        assert_eq!(slab.get(i2), Some(&"world"));
    }

    /// 【wave 156 FB-1】alloc 系列 golden: 無制限・順序 = 挿入番号
    /// (usize::MAX sentinel は返りえない — 捕捉 74 vacuous 満杯分岐の
    /// module 真契約 pin、rq fb_slab (1))。
    #[test]
    fn alloc_unbounded_sequence_golden_strict() {
        let mut slab = Slab::<u32>::new();
        let got: Vec<usize> = (0..8u32).map(|v| slab.alloc(v)).collect();
        assert_eq!(got, vec![0, 1, 2, 3, 4, 5, 6, 7], "順序 = 挿入番号");
        assert!(
            got.iter().all(|&i| i != usize::MAX),
            "MAX sentinel 未到達 (無制限契約、捕捉 74)"
        );
    }

    /// 【wave 156 FB-1】free-list LIFO 復帰 golden (rq fb_slab (1)):
    /// alloc 0,1,2 → free 0 → free 2 → alloc は 2,0 の順に復帰 → 枯渇後は
    /// push で 3。head 逐次 sim との完全一致。
    #[test]
    fn free_lifo_reuse_golden_strict() {
        let mut slab = Slab::<u32>::new();
        assert_eq!(slab.alloc(10), 0);
        assert_eq!(slab.alloc(11), 1);
        assert_eq!(slab.alloc(12), 2);
        assert_eq!(slab.free(0), Some(10));
        assert_eq!(slab.free(2), Some(12));
        assert_eq!(slab.alloc(13), 2, "LIFO: 直近 free の 2 から復帰");
        assert_eq!(slab.alloc(14), 0, "LIFO: 次は 0");
        assert_eq!(slab.alloc(15), 3, "free 枯渇 → push 新規 idx 3");
        assert_eq!(slab.get(2), Some(&13), "復帰 slot は新値");
    }

    /// 【wave 156 FB-1】GenerationalSlab ABA pin (rq fb_slab (2)): 世代は
    /// remove 毎に +1、古 handle は get/remove 両方 None (UAF 完全防止)。
    #[test]
    fn generational_aba_golden_strict() {
        let mut slab = GenerationalSlab::<u32>::new();
        let h1 = slab.insert(100);
        assert_eq!((h1.index, h1.generation), (0, 0), "insert gen 0");
        assert_eq!(slab.remove(h1), Some(100));
        let h2 = slab.insert(200);
        assert_eq!((h2.index, h2.generation), (0, 1), "reuse gen +1 (ABA)");
        assert_eq!(slab.get(h1), None, "stale handle get → None");
        assert_eq!(slab.remove(h1), None, "stale handle remove → None");
        assert_eq!(slab.get(h2), Some(&200), "現役 handle のみ有効");
        assert_eq!(slab.remove(h2), Some(200));
        let h3 = slab.insert(300);
        assert_eq!((h3.index, h3.generation), (0, 2), "2 回目 reuse gen 2");
    }

    /// 【wave 156 FB-1】occupied_count 厳密系列 (rq fb_slab (3)): 二重
    /// free (stale) は None で occupied 不変、窓端は is_empty 厳密一致。
    #[test]
    fn occupied_double_free_golden_strict() {
        let mut slab = GenerationalSlab::<u32>::new();
        let h1 = slab.insert(1);
        slab.insert(2);
        assert_eq!(slab.len(), 2);
        assert!(!slab.is_empty());
        assert_eq!(slab.remove(h1), Some(1));
        assert_eq!(slab.len(), 1);
        assert_eq!(slab.remove(h1), None, "stale 二重 remove → None");
        assert_eq!(slab.len(), 1, "occupied は不変 (二重計上なし)");
        assert_eq!(slab.remove(SlabHandle::new(1, 0)), Some(2));
        assert_eq!(slab.len(), 0);
        assert!(slab.is_empty(), "len==0 と is_empty 厳密一致");
    }

    /// 【wave 156 FB-1】範囲外 / handle 直打ちの安全 None 契約:
    /// idx 範囲外 free/get + free(u32::MAX as usize) で idx_to_handle
    /// overflow しない (捕捉 74 sentinel 系の裏側 pin)。
    #[test]
    fn out_of_range_none_contract_strict() {
        let mut slab = Slab::<u32>::new();
        slab.alloc(7);
        assert_eq!(slab.get(5), None, "範囲外 get None");
        assert_eq!(slab.free(5), None, "範囲外 free None");
        assert_eq!(
            slab.free(u32::MAX as usize),
            None,
            "MAX idx overflow せず None"
        );
        let mut g = GenerationalSlab::<u32>::new();
        g.insert(9);
        assert_eq!(
            g.get(SlabHandle::new(99, 0)),
            None,
            "範囲外 gen handle None"
        );
        assert_eq!(
            g.remove(SlabHandle::new(0, 1)),
            None,
            "gen 不一致 remove None"
        );
    }

    /// 【wave 156 FB-2 §7 消化 19】ObjectPool 系列 golden (rq fb_slab
    /// (4)): cap=2 で acquire 2→1→0→None、release で cap 復帰。
    #[test]
    fn object_pool_sequence_golden_strict() {
        let mut pool = ObjectPool::<u32>::new(2);
        assert_eq!(pool.available(), 2, "init = cap");
        assert_eq!(pool.acquire(), Some(0));
        assert_eq!(pool.available(), 1, "1 outstanding");
        assert_eq!(pool.acquire(), Some(0));
        assert_eq!(pool.available(), 0, "drained");
        assert_eq!(pool.acquire(), None, "枯渇 acquire None");
        pool.release(9);
        pool.release(8);
        assert_eq!(pool.available(), 2, "release で cap 復帰");
        assert_eq!(pool.acquire(), Some(8), "LIFO: 直近 release から");
        let mut empty = ObjectPool::<u32>::new(0);
        assert_eq!(empty.acquire(), None, "cap 0 は即 None");
    }

    /// 【wave 156 FB-2 §7 消化 19】with_capacity は挙動同一 + reserve
    /// のみ (Vec::with_capacity 準拠): push 系列が new() と一致。
    #[test]
    fn with_capacity_semantics_pin_strict() {
        let mut a = Slab::<u32>::new();
        let mut b = Slab::<u32>::with_capacity(64);
        for v in 0..10u32 {
            assert_eq!(
                a.alloc(v),
                b.alloc(v),
                "系列一致 (capacity は reserve のみ)"
            );
        }
        assert_eq!(b.len(), 10);
        assert!(!b.is_empty());
    }

    /// 【wave 156 FB-2 §7 消化 19】keyed lifecycle pattern pin (wiring
    /// 実消費の module 側再現): key→slot map + evict free で占有が追従、
    /// 復帰 slot は LIFO (rq fb_slab (6))。
    #[test]
    fn keyed_lifecycle_pattern_golden_strict() {
        use std::collections::BTreeMap;
        let mut slab = Slab::<u32>::new();
        let mut map: BTreeMap<(i32, i32), usize> = BTreeMap::new();
        // t1: (0,0)=7, (1,0)=9 → slots 0,1
        for (k, m) in [((0, 0), 7u32), ((1, 0), 9u32)] {
            let slot = *map.entry(k).or_insert_with(|| slab.alloc(m));
            assert_eq!(*slab.get(slot).unwrap(), m);
        }
        assert_eq!(slab.len(), 2);
        // t2: keys (1,0),(2,0) — (0,0) evict free、(2,0) new alloc
        let cur: std::collections::BTreeSet<(i32, i32)> = [(1, 0), (2, 0)].into_iter().collect();
        let evict: Vec<(i32, i32)> = map.keys().copied().filter(|k| !cur.contains(k)).collect();
        for k in evict {
            let slot = map.remove(&k).unwrap();
            assert_eq!(slab.free(slot), Some(7), "evicted (0,0) の mat 7 回収");
        }
        for (k, m) in [((2, 0), 3u32)] {
            let slot = *map.entry(k).or_insert_with(|| slab.alloc(m));
            assert_eq!(*slab.get(slot).unwrap(), m);
        }
        assert_eq!(slab.len(), 2, "occupied 追従 (rq fb_slab (6))");
        assert_eq!(map[&(1, 0)], 1, "(1,0) slot 不変");
        assert_eq!(map[&(2, 0)], 0, "(2,0) は free 済 slot0 を LIFO 再利用");
        assert_eq!(*slab.get(map[&(1, 0)]).unwrap(), 9, "(1,0) mat 9 read-back");
    }

    /// 【wave 185 GE フェーズ2 回収】dead code 系 7 例目 (wave 156 FB adversarial (b)
    /// 削除済 get_mut 復活 非検出、census 消費者ゼロ確定で削除済) の lexeme pin 化。
    /// 同宣言形の将来復活を静寂に通さない。
    #[test]
    fn ge_removed_get_mut_lexeme() {
        let src = include_str!("pool_slab.rs");
        for lex in [concat!("fn get_", "mut")] {
            assert!(
                !src.contains(lex),
                "dead code 系削除語彙の宣言形復活を検出 (wave 185 GE lexeme pin)"
            );
        }
    }
}
