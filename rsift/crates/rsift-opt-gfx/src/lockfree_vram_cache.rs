//! # 22. Lock-Free VRAM Mesh Cache (`LockFreeVramMeshCache`)
//!
//! ロックフリー FIFO ページ置換方式による VRAM メッシュキャッシュ。
//! マルチスレッドでの並行チャンクメッシュ生成とロックフリーアップロードを実現する。
//!
//! 誠実注記 (wave 139 EM-3):
//! 1. Hit 探索は O(max_slots) 線形 — wiring 実引数 1<<16=65,536 スロットを
//!    hit 時に先頭から掃引する (per chunk O(N)・ボトルネック可能性は
//!    未計測の「構造的必然」推測として記録)。wiring の消費は hits/misses
//!    集計のみで handle (slot_idx/generation) は `_h` 破棄 = GPU
//!    アップロード経路未配線 (wave 83 CG-6 同型の構造)。
//! 2. 「lock-free」の精確化: CAS ループは存在せず、eviction は
//!    `head.fetch_add(1) % max_slots` の wait-free round-robin。複数
//!    writer は同一 slot を同時選択しうる (write-write race で片方の
//!    key が失われる = キャッシュ統計としては benign・一意性保証なし)。
//!    wiring は単スレッド駆動で安全側。
//! 3. generation は miss (evict/初挿入) のみ +1 で hit 不変 — 他 chunk
//!    evict 時の古 handle 検知 (ABA) が設計意図。u32 wrap は 2^32 evict
//!    後に一周 (実害域外、pin のみ)。
//! 4. hits/misses は Relaxed 統計 (順序保証なし・報告目的に適合)。
//!    occupied 独立フラグ化 (EM-1) により key 値域 u64 全 2^64 が利用可能
//!    (旧設計は空スロット sentinel u64::MAX と pack_key(-1,-1) が衝突し、
//!    (-1,-1) チャンクが空キャッシュで誤ヒットする真バグだった = 捕捉 56)。

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VramCacheHandle {
    pub slot_idx: u32,
    pub chunk_key: (i32, i32),
    pub generation: u32,
}

pub struct LockFreeVramMeshCache {
    pub max_slots: usize,
    chunk_keys: Vec<AtomicU64>, // packed (x, z)
    generations: Vec<AtomicU32>,
    /// 占有ビット (EM-1): 空スロット判定を key 値から分離。
    /// 旧設計は sentinel u64::MAX 値比較で pack_key(-1,-1) と衝突した
    /// (捕捉 56)。key 書込み (Release) 後に true 化し、読側は
    /// Acquire で観測後に key を読む (単一 writer 順序で linearizable)。
    occupied: Vec<AtomicBool>,
    head: AtomicUsize,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
}

impl LockFreeVramMeshCache {
    pub fn new(max_slots: usize) -> Self {
        assert!(
            max_slots >= 1,
            "lockfree_vram_cache: max_slots >= 1 (0 は miss 経路の % 0 でゼロ除算)"
        );
        assert!(
            max_slots <= u32::MAX as usize,
            "lockfree_vram_cache: max_slots <= u32::MAX (slot_idx: u32 の truncate 折り畳み防止)"
        );
        let mut keys = Vec::with_capacity(max_slots);
        let mut gens = Vec::with_capacity(max_slots);
        let mut occ = Vec::with_capacity(max_slots);
        for _ in 0..max_slots {
            keys.push(AtomicU64::new(u64::MAX));
            gens.push(AtomicU32::new(0));
            occ.push(AtomicBool::new(false));
        }
        Self {
            max_slots,
            chunk_keys: keys,
            generations: gens,
            occupied: occ,
            head: AtomicUsize::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    fn pack_key(cx: i32, cz: i32) -> u64 {
        ((cx as u32) as u64) | (((cz as u32) as u64) << 32)
    }

    pub fn lookup_or_insert(&self, cx: i32, cz: i32) -> (VramCacheHandle, bool) {
        let key = Self::pack_key(cx, cz);
        for i in 0..self.max_slots {
            // EM-1: 占有ビット独立判定 (sentinel 値比較を廃止)。
            // occupied=true を観測したなら key 書込みは完了済 (Release/Acquire)。
            if self.occupied[i].load(Ordering::Acquire)
                && self.chunk_keys[i].load(Ordering::Acquire) == key
            {
                self.hits.fetch_add(1, Ordering::Relaxed);
                let gen = self.generations[i].load(Ordering::Acquire);
                return (
                    VramCacheHandle {
                        slot_idx: i as u32,
                        chunk_key: (cx, cz),
                        generation: gen,
                    },
                    true,
                );
            }
        }

        // Miss: evict oldest slot lock-free via atomic head increment
        self.misses.fetch_add(1, Ordering::Relaxed);
        let slot = self.head.fetch_add(1, Ordering::Relaxed) % self.max_slots;
        self.chunk_keys[slot].store(key, Ordering::Release);
        let new_gen = self.generations[slot].fetch_add(1, Ordering::Relaxed) + 1;
        // key 書込み公開後に占有化 (読側は true 観測 ⇒ key 確定を保証)。
        self.occupied[slot].store(true, Ordering::Release);

        (
            VramCacheHandle {
                slot_idx: slot as u32,
                chunk_key: (cx, cz),
                generation: new_gen,
            },
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lockfree_vram_mesh_cache() {
        let cache = LockFreeVramMeshCache::new(4);
        let (h1, hit1) = cache.lookup_or_insert(10, 20);
        assert!(!hit1);
        let (h2, hit2) = cache.lookup_or_insert(10, 20);
        assert!(hit2);
        assert_eq!(h1.slot_idx, h2.slot_idx);
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    /// EM-1 / 捕捉 56: 空スロット初期値 u64::MAX は pack_key(-1,-1) と
    /// 同一ビット列 (rq 事前導出: (-1 as u32)=0xFFFFFFFF が lo/hi 両方で
    /// sentinel 全 1 ペアと一致)。occupied フラグ導入前は空キャッシュでも
    /// Hit=true を誤報しメッシュ欠落のまま hit 扱いになりうる真バグ
    /// (Minecraft チャンク座標は負も通常出現)。根治後: 初回 miss・2 回目 hit。
    #[test]
    fn minus1_minus1_not_aliased_with_empty_sentinel() {
        let cache = LockFreeVramMeshCache::new(4);
        let (_, existed_first) = cache.lookup_or_insert(-1, -1);
        assert!(
            !existed_first,
            "空キャッシュの (-1,-1) は miss でなければならない (sentinel 衝突)"
        );
        let (_, existed_second) = cache.lookup_or_insert(-1, -1);
        assert!(existed_second, "挿入済の (-1,-1) は hit");
    }

    /// EM-2: new() fail-loud 契約 (max_slots=0 は miss 経路の % 0 で
    /// ゼロ除算 panic、EH-2 同型の堕落形)。
    #[test]
    #[should_panic(expected = "max_slots >= 1")]
    fn new_zero_slots_panics() {
        let _ = LockFreeVramMeshCache::new(0);
    }

    /// EM-2: slot_idx は u32 公開のため max_slots > u32::MAX は truncate
    /// 折り畳み衝突。割当前に assert (48GB 級割当を試さない)。
    #[test]
    #[should_panic(expected = "max_slots <= u32::MAX")]
    fn new_over_u32_slots_panics() {
        let _ = LockFreeVramMeshCache::new((u32::MAX as usize) + 1);
    }

    /// EM-4: pack_key 単射・bit 配置 pin (rq 導出: (1,0)→1, (0,1)→1<<32)。
    #[test]
    fn pack_key_injective_and_layout() {
        let k_m1 = LockFreeVramMeshCache::pack_key(-1, -1);
        let k_00 = LockFreeVramMeshCache::pack_key(0, 0);
        let k_min = LockFreeVramMeshCache::pack_key(i32::MIN, i32::MIN);
        let k_max = LockFreeVramMeshCache::pack_key(i32::MAX, i32::MAX);
        for (i, a) in [k_m1, k_00, k_min, k_max].iter().enumerate() {
            for (j, b) in [k_m1, k_00, k_min, k_max].iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "pack_key 単射境界 ({i},{j})");
                }
            }
        }
        assert_eq!(k_m1, u64::MAX, "(-1,-1) = 全 1 = 旧 sentinel 値 (rq 導出)");
        assert_eq!(LockFreeVramMeshCache::pack_key(1, 0), 1);
        assert_eq!(LockFreeVramMeshCache::pack_key(0, 1), 1u64 << 32);
        assert_ne!(
            LockFreeVramMeshCache::pack_key(-1, 0),
            LockFreeVramMeshCache::pack_key(0, -1),
            "lo/hi 非対称"
        );
    }

    /// EM-4: FIFO eviction 順 (rq 導出: new(2) head = 0,1,0,1,0)。
    #[test]
    fn fifo_eviction_order() {
        let cache = LockFreeVramMeshCache::new(2);
        let (h0, e0) = cache.lookup_or_insert(0, 0);
        assert!(!e0 && h0.slot_idx == 0);
        let (h1, e1) = cache.lookup_or_insert(1, 1);
        assert!(!e1 && h1.slot_idx == 1);
        let (h2, e2) = cache.lookup_or_insert(2, 2);
        assert!(!e2, "容量超過は必ず miss");
        assert_eq!(h2.slot_idx, 0, "FIFO: 最古 slot 0 を退避");
        let (h3, e3) = cache.lookup_or_insert(3, 3);
        assert!(!e3 && h3.slot_idx == 1);
        // 退避済み (0,0) の再参照は miss → その場で slot 0 (= (2,2)) を退避して再挿入
        let (_, e4) = cache.lookup_or_insert(0, 0);
        assert!(!e4, "(0,0) は slot 0 から退避済み");
        let (_, e5) = cache.lookup_or_insert(2, 2);
        assert!(!e5, "(2,2) は (0,0) 再挿入で slot 0 から退避済み");
        assert_eq!(cache.hits.load(Ordering::Relaxed), 0);
        assert_eq!(cache.misses.load(Ordering::Relaxed), 6);
    }

    /// EM-4: generation は hit 不変・evict で +1 (rq 導出 gen 2)。
    #[test]
    fn generation_monotonic_on_evict() {
        let cache = LockFreeVramMeshCache::new(1);
        let (h0, _) = cache.lookup_or_insert(5, 5);
        assert_eq!(h0.generation, 1, "初回挿入 gen=1");
        let (h1, e1) = cache.lookup_or_insert(5, 5);
        assert!(e1);
        assert_eq!(h1.generation, 1, "hit では generation 不変");
        let (h2, e2) = cache.lookup_or_insert(6, 6);
        assert!(!e2);
        assert_eq!(h2.generation, 2, "evict で generation +1");
        assert_eq!(h2.slot_idx, 0, "new(1) は常に slot 0");
    }

    /// EM-4: hit/miss 統計の Relaxed 集計 pin。
    #[test]
    fn hits_misses_accounting() {
        let cache = LockFreeVramMeshCache::new(4);
        let _ = cache.lookup_or_insert(7, 7); // miss
        let _ = cache.lookup_or_insert(7, 7); // hit
        let _ = cache.lookup_or_insert(8, 8); // miss
        assert_eq!(cache.hits.load(Ordering::Relaxed), 1);
        assert_eq!(cache.misses.load(Ordering::Relaxed), 2);
    }
}
