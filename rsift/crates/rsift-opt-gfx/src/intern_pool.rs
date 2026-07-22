//! # InternPool — FerriteCore 思想の汎用等価データ重複排除プール
//!
//! 出典: malte0811/FerriteCore `summary.md` — Minecraft は「同じ中身の
//! BlockState predicate / multipart model / 形状 AABB リスト」をブロック状態ごとに
//! 個別インスタンス化しており、等価なものを 1 実体に集約するだけで 300-400MB 級の
//! ヒープが浮く。ここでは Rsift 側で使う汎用版を実装:
//!
//! * [`InternPool<T>`] — 任意の `Hash + Eq` な値を 1 実体に正規化し、
//!   軽量な [`InternId`] (u32) を返す。参照カウントで自動 GC。
//! * [`ShapeCache`] — 衝突/描画形状（AABB 列）専用キャッシュ。
//!   「同じ AABB 配列」を持つ数百のブロック状態が 1 エントリを共有する。
//!
//! どちらもアクセスは O(1)（ハッシュ 1 回）。メモリはユニーク数に比例する。

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash, Hasher};

/// Firefox/rustc-hash 系の乗算回転ハッシャ (SipHash 代替)。
/// インターンプールのキーは同一プロセス内で自分が生成した頂点バイト列等の
/// 信頼できるデータであり、HashDoS 耐性よりホットループ速度を取る設計。
/// 等値判定は `Eq` が担保するため、ハッシュ弱化は正しさに影響しない
/// (影響するのは衝突率 = perf のみ)。
/// 12B 頂点キーで実測されるように固定小数/整数ドメインで偏りが小さい
/// 乗算回転 (rotate 5 ^ word) * SEED を採用する。
#[derive(Clone, Default)]
pub struct FoldHasher {
    hash: u64,
}

impl FoldHasher {
    const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(Self::SEED);
    }
}

impl Hasher for FoldHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // 8B チャンクで畳み、末尾はリトルエンディアン詰めで 1 語にする
        let mut b = bytes;
        while b.len() >= 8 {
            self.add(u64::from_le_bytes(b[..8].try_into().unwrap()));
            b = &b[8..];
        }
        if !b.is_empty() {
            let mut word = 0u64;
            for (i, &x) in b.iter().enumerate() {
                word |= (x as u64) << (i * 8);
            }
            self.add(word);
        }
    }

    #[inline]
    fn write_u8(&mut self, v: u8) {
        self.add(v as u64);
    }
    #[inline]
    fn write_u16(&mut self, v: u16) {
        self.add(v as u64);
    }
    #[inline]
    fn write_u32(&mut self, v: u32) {
        self.add(v as u64);
    }
    #[inline]
    fn write_u64(&mut self, v: u64) {
        self.add(v);
    }
    #[inline]
    fn write_usize(&mut self, v: usize) {
        self.add(v as u64);
    }
    #[inline]
    fn write_u128(&mut self, v: u128) {
        self.add(v as u64);
        self.add((v >> 64) as u64);
    }
}

/// [`FoldHasher`] 用の BuildHasher (HashMap の第 3 型引数に使う)。
#[derive(Clone, Default)]
pub struct FoldBuildHasher;

impl BuildHasher for FoldBuildHasher {
    type Hasher = FoldHasher;

    #[inline]
    fn build_hasher(&self) -> FoldHasher {
        FoldHasher::default()
    }
}

/// プール内実体を指す軽量 ID（コピー・比較が u32 1 個）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InternId(pub u32);

#[derive(Debug)]
struct Entry {
    refs: u32,
}

/// 汎用インターンプール（値 → 単一実体）。
pub struct InternPool<T: Hash + Eq> {
    map: HashMap<T, InternId, FoldBuildHasher>,
    entries: Vec<Entry>,
    values: Vec<Option<T>>,
    free_slots: Vec<u32>,
    pub hits: u64,
    pub misses: u64,
}

impl<T: Hash + Eq + Clone> InternPool<T> {
    pub fn new() -> Self {
        Self {
            map: HashMap::default(),
            entries: Vec::new(),
            values: Vec::new(),
            free_slots: Vec::new(),
            hits: 0,
            misses: 0,
        }
    }

    /// 値を正規化して ID を得る。既にあれば参照カウント +1。
    pub fn intern(&mut self, v: T) -> InternId {
        if let Some(&id) = self.map.get(&v) {
            self.entries[id.0 as usize].refs += 1;
            self.hits += 1;
            return id;
        }
        self.misses += 1;
        let id = if let Some(slot) = self.free_slots.pop() {
            self.entries[slot as usize] = Entry { refs: 1 };
            self.values[slot as usize] = Some(v.clone());
            InternId(slot)
        } else {
            let slot = self.entries.len() as u32;
            self.entries.push(Entry { refs: 1 });
            self.values.push(Some(v.clone()));
            InternId(slot)
        };
        self.map.insert(v, id);
        id
    }

    /// 参照を手放す。0 になったら実体を回収（スロットは再利用）。
    ///
    /// **契約 (2026-07-23 wave 48 厳格化)**: `id` は `intern` が返した
    /// **生存中**の ID 必須。二重 release は refs==0 で **全ビルド**で
    /// fail-loud (旧実装は debug_assert のみで、release build では
    /// `0 - 1 → u32::MAX` へアンダーフローして実体が不死化し、
    /// スロット再利用後の誤 release が他人の参照カウントを奪う
    /// 永久破壊に発展し得た)。
    /// **注意 (正直化)**: `InternId` は世代を持たないため「解放後に
    /// スロットが再利用され新しい実体が同じ ID 値を持つ」ABA は API 上
    /// 検出できない。intern/release の 1:1 対応は呼出側の規律とする
    /// (旧コメントの「gen による ABA 防止」は gen が一度も読まれない
    /// 偽主張だったため、wave 48 で gen ごと撤去)。
    pub fn release(&mut self, id: InternId) {
        let idx = id.0 as usize;
        assert!(
            idx < self.entries.len(),
            "release 契約違反: 範囲外 InternId {} (entries={})",
            id.0,
            self.entries.len()
        );
        let e = &mut self.entries[idx];
        assert!(
            e.refs > 0,
            "release 契約違反: 解放済みスロット {} への二重 release",
            id.0
        );
        e.refs -= 1;
        if e.refs == 0 {
            if let Some(v) = self.values[idx].take() {
                self.map.remove(&v);
            }
            self.free_slots.push(id.0);
        }
    }

    /// 現在のユニーク実体数。
    pub fn unique_count(&self) -> usize {
        self.map.len()
    }

    /// 実体の参照（GC 済みの場合 None）。
    pub fn get(&self, id: InternId) -> Option<&T> {
        self.values.get(id.0 as usize).and_then(|v| v.as_ref())
    }

    /// ヒット率（統計用）。
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

impl<T: Hash + Eq + Clone> Default for InternPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// 形状（AABB 列）専用キャッシュ。キーを Vec<[f32;6]> の正準形にして共有。
#[derive(Default)]
pub struct ShapeCache {
    pool: InternPool<Vec<ShapeBox>>,
    /// AABB 単体の半自動キー化（量子化して f32 の bit 揺れを吸収）
    quant: f32,
}

/// 正準化された AABB（量子化済み整数で Eq/Hash 可能）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShapeBox {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl ShapeCache {
    /// **契約 (2026-07-23 wave 48 厳格化)**: `quant` は正の有限値必須
    /// (旧実装は `.max(1e-4)` で負・ゼロ・NaN を静寂に 1e-4 矯正していた)。
    pub fn new(quant: f32) -> Self {
        assert!(
            quant.is_finite() && quant > 0.0,
            "ShapeCache::new 契約違反: quant は正の有限値必須 ({quant})"
        );
        Self {
            pool: InternPool::new(),
            quant,
        }
    }

    fn quantize(&self, aabb: [f32; 6]) -> ShapeBox {
        let q = self.quant;
        for (i, &v) in aabb.iter().enumerate() {
            assert!(
                v.is_finite(),
                "intern_shape 契約違反: aabb[{i}] が非有限 ({v}) — NaN の飽和 0 化による誤共有を防止"
            );
            // i32 飽和 cast で遠方形状が静寂に i32::MAX へ潰れ、
            // 別形状との誤共有 (衝突形状の誤マージ) が起きるのを防ぐ
            // (Minecraft ワールド端 ±3e7 blocks ÷ 細かい quant で容易に到達)。
            let sc = v / q;
            assert!(
                sc.abs() < 2147483648.0,
                "intern_shape 契約違反: aabb[{i}]/quant = {sc} が i32 量子化域外 — quant を大きくするか座標系を見直せ"
            );
        }
        ShapeBox {
            min: [
                (aabb[0] / q).round() as i32,
                (aabb[1] / q).round() as i32,
                (aabb[2] / q).round() as i32,
            ],
            max: [
                (aabb[3] / q).round() as i32,
                (aabb[4] / q).round() as i32,
                (aabb[5] / q).round() as i32,
            ],
        }
    }

    /// 形状（AABB 列）をインターン。
    pub fn intern_shape(&mut self, aabbs: &[[f32; 6]]) -> InternId {
        let mut v: Vec<ShapeBox> = aabbs.iter().map(|a| self.quantize(*a)).collect();
        v.sort();
        v.dedup();
        self.pool.intern(v)
    }

    /// ブロック形状全体を共有した場合の推定削減 bytes。
    /// 「N 個の状態が M 個のユニーク実体に集約された際の差分」。
    pub fn estimated_savings_bytes(&self, total_states: usize, bytes_per_instance: usize) -> usize {
        let uniq = self.pool.unique_count();
        if total_states <= uniq {
            0
        } else {
            (total_states - uniq) * bytes_per_instance
        }
    }

    pub fn pool(&self) -> &InternPool<Vec<ShapeBox>> {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_deduplicates() {
        let mut p = InternPool::new();
        let a = p.intern(String::from("shape:north=true"));
        let b = p.intern(String::from("shape:north=true"));
        let c = p.intern(String::from("shape:north=false"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(p.unique_count(), 2);
        assert!(p.hit_rate() > 0.3);
    }

    #[test]
    fn release_reclaims_slot() {
        let mut p = InternPool::new();
        let a = p.intern(42u32);
        p.release(a);
        assert_eq!(p.unique_count(), 0);
        assert!(p.get(a).is_none());
        let b = p.intern(7u32);
        assert_eq!(b.0, a.0, "slot must be reused");
    }

    #[test]
    fn shape_cache_shares_identical_shapes() {
        let mut sc = ShapeCache::new(0.001);
        let full = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
        let mut ids = Vec::new();
        // 「128 個のブロック状態」が全部同じ立方体形状、という FerriteCore 的状況
        for _ in 0..128 {
            ids.push(sc.intern_shape(&full));
        }
        assert_eq!(sc.pool().unique_count(), 1);
        assert!(ids.iter().all(|&i| i == ids[0]));
        // 別形は別 ID
        let slab = [[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]];
        let slab_id = sc.intern_shape(&slab);
        assert_ne!(slab_id, ids[0]);
        assert_eq!(sc.pool().unique_count(), 2);
        // 削減推定: 129 状態 - 2 ユニーク
        let saved = sc.estimated_savings_bytes(129, 512);
        assert_eq!(saved, 127 * 512);
    }

    #[test]
    fn quantization_merges_tiny_float_drift() {
        let mut sc = ShapeCache::new(0.01);
        let a = sc.intern_shape(&[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]);
        let b = sc.intern_shape(&[[0.0, 0.0, 0.0, 1.0001, 0.9999, 1.0]]);
        assert_eq!(a, b, "1e-4 差は量子化で同一視");
    }

    /// wave 48-1: 量子化の丸め規則 (round half away from zero) を厳密値でピン。
    #[test]
    fn quantize_rounding_exact_values() {
        let mut sc = ShapeCache::new(0.5);
        // 1.0/0.5 = 2.0 → 2; 0.75/0.5 = 1.5 → 2 (half away from zero);
        // -0.25/0.5 = -0.5 → -1 (対称対称 round)。いずれも f32 厳密値。
        let id = sc.intern_shape(&[[-0.25, 0.0, 0.75, 1.0, 1.0, 1.0]]);
        let shape = sc.pool().get(id).expect("alive");
        assert_eq!(shape[0].min, [-1, 0, 2]);
        assert_eq!(shape[0].max, [2, 2, 2]);
        // 同一正準形は同一 ID (順序非依存の sort+dedup 経由)
        let id2 = sc.intern_shape(&[
            [1.0, 1.0, 1.0, 1.0, 1.0, 2.0],
            [-0.25, 0.0, 0.75, 1.0, 1.0, 1.0],
        ]);
        // 1.0/0.5=2,2.0/0.5=4 → 2 要素目は [2,2,2]→[2,2,4]
        let shape2 = sc.pool().get(id2).expect("alive");
        assert_eq!(shape2.len(), 2);
        assert_eq!(shape2[0].min, [-1, 0, 2], "sort 後先頭は小さい方");
        assert_eq!(shape2[1].max, [2, 2, 4]);
    }

    /// wave 48-2: 二重 release は全ビルドで fail-loud
    /// (旧 debug_assert 限定で release build は u32 underflow 不死化)。
    #[test]
    #[should_panic(expected = "解放済みスロット 0")]
    fn release_rejects_double_release() {
        let mut p: InternPool<u32> = InternPool::new();
        let a = p.intern(1);
        p.release(a);
        p.release(a);
    }

    /// wave 48-3: 範囲外 ID の release も明示 fail-loud。
    #[test]
    #[should_panic(expected = "範囲外 InternId 9")]
    fn release_rejects_out_of_range_id() {
        let mut p: InternPool<u32> = InternPool::new();
        p.intern(5);
        p.release(InternId(9));
    }

    /// wave 48-4: 非有限座標は飽和 0 化での誤共有を防ぐため拒否。
    #[test]
    #[should_panic(expected = "aabb[0] が非有限")]
    fn intern_shape_rejects_nan() {
        let mut sc = ShapeCache::new(0.001);
        let _ = sc.intern_shape(&[[f32::NAN, 0.0, 0.0, 1.0, 1.0, 1.0]]);
    }

    /// wave 48-5: i32 量子化域外 (遠方座標) は飽和潰れ誤共有を防ぐため拒否。
    #[test]
    #[should_panic(expected = "i32 量子化域外")]
    fn intern_shape_rejects_far_coordinates_below_quant_domain() {
        let mut sc = ShapeCache::new(0.001);
        // 3e6 blocks / 1e-3 = 3e9 quanta > 2^31 (2.147e9)
        let _ = sc.intern_shape(&[[3.0e6, 0.0, 0.0, 1.0, 1.0, 1.0]]);
    }

    /// wave 48-6: quant 契約 — ゼロ/NaN は静寂矯正せず拒否。
    #[test]
    #[should_panic(expected = "quant は正の有限値必須")]
    fn shape_cache_rejects_zero_quant() {
        let _ = ShapeCache::new(0.0);
    }

    #[test]
    #[should_panic(expected = "quant は正の有限値必須")]
    fn shape_cache_rejects_nan_quant() {
        let _ = ShapeCache::new(f32::NAN);
    }
}
