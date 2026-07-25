//! # PalettePack — 可変ビットパレット圧縮（MC 1.13+ 方式を整備）
//!
//! 出典: Minecraft 1.13 flattening 以降のチャンク保存形式 — セクション内の
//! ユニークブロック状態をパレット化し、パレットサイズの ceil(log2) ビットで
//! 各ブロックを詰める。16 種なら 4bit/ブロック = 4096 ブロックで 2KB。
//!
//! この実装は **1.21 の "single value optimized" 相当 + 動的拡張** を含む:
//! * ユニーク 1 種 → データ 0 バイト（単一値セクション）
//! * `set` 時に新種が出ると bits を自動拡張して全詰替え
//!
//! **bits の上限は 12 でフォールバックは存在しない** (wave 102 DB-1 訂正):
//! 1 セクションは 4096 ブロックなのでユニーク数は高々 4096、u16 状態を
//! 直接パレット格納する本形式では `needed_bits(4096) = 12` で全て表現可能。
//! 旧 doc の「322 種超 → 直接 16bit 格納にフォールバック」は vanilla の
//! 「9bit 超 → グローバルパレット直接 id」設計の化石であり未実装だった。
//!
//! メモリ (生 u16 配列 8192B/section 比, `memory_bytes` 定義値):
//! - 2 種: 524B ≒ 1/15.6 (stone/dirt 的典型セクション)
//! - 16 種: 2088B ≒ 1/3.9
//! - 322 種: 5340B ≒ 0.65
//! - **最悪 4096 全相異: 14760B ≒ 1.80 倍に膨張** (12bit 語パディング損 6560B
//!   + パレット 8192B。旧 doc の「最大 ~1/4」はこの敵対ケースで偽になるため
//!   訂正。`stats_for` の実測値が常に正しい一次情報)。

use std::collections::HashMap;

/// 1 セクション = 16^3 ブロック。
pub const SECTION_VOLUME: usize = 4096;

/// 可変ビット・パレット格納セクション。
///
/// パレットは **単調増加** (vanilla と同設計。`set` で一時的に 0 個になった
/// 状態もパレットから除去しない = refcount/compaction は持たない。
/// セクション全体の再 pack は `from_blocks` の呼び直しで得る)。
#[derive(Debug, Clone)]
pub struct PackedSection {
    /// パレット（id → block_state）
    palette: Vec<u16>,
    /// block_state → パレット id
    rev: HashMap<u16, u16>,
    /// 現在の bits/entry
    bits: u8,
    /// 詰められたエントリ（u64 語の配列）
    data: Vec<u64>,
}

impl PackedSection {
    pub fn new() -> Self {
        let mut rev = HashMap::new();
        rev.insert(0u16, 0u16); // air=0 は id 0 固定
        Self {
            palette: vec![0],
            rev,
            bits: 0, // bits=0: 単一値セクション（data なし）
            data: Vec::new(),
        }
    }

    pub fn unique_states(&self) -> usize {
        self.palette.len()
    }

    pub fn bits_per_entry(&self) -> u8 {
        self.bits
    }

    /// メモリ使用量（バイト）= **永続層 (wire/disk 相当) の定義値**:
    /// palette (`len*2`) + data 語 (`len*8`) + ヘッダ相当定数 8。
    /// `rev` (block_state → id の逆引き HashMap) は `set` 高速化のための
    /// working map で wire/disk には含まれないため非計上 (wave 102 DB-3 明記。
    /// 本体メモリではエントリあたり概ね数十 B が別途存在する点に注意)。
    pub fn memory_bytes(&self) -> usize {
        self.palette.len() * 2 + self.data.len() * 8 + 8
    }

    /// 生の u16 配列からの圧縮構築。
    pub fn from_blocks(blocks: &[u16; SECTION_VOLUME]) -> Self {
        let mut s = Self::new();
        // 最初にパレットを走査して安定サイズを決める（2 パスで再詰替えなし）
        let mut uniq: Vec<u16> = Vec::new();
        let mut seen = HashMap::new();
        for &b in blocks {
            if !seen.contains_key(&b) {
                seen.insert(b, uniq.len() as u16);
                uniq.push(b);
            }
        }
        let n = uniq.len();
        if n <= 1 {
            s.palette = uniq;
            s.rev = seen;
            s.bits = 0;
            return s;
        }
        let bits = needed_bits(n as u32);
        s.palette = uniq;
        s.rev = seen;
        s.bits = bits;
        let per_word = 64 / bits as usize;
        let words = (SECTION_VOLUME + per_word - 1) / per_word;
        s.data = vec![0u64; words];
        for (i, &b) in blocks.iter().enumerate() {
            let id = s.rev[&b];
            s.write(i, id);
        }
        s
    }

    #[inline]
    fn write(&mut self, index: usize, id: u16) {
        if self.bits == 0 {
            return;
        }
        let bits = self.bits as usize;
        let per_word = 64 / bits;
        let w = index / per_word;
        let off = (index % per_word) * bits;
        let mask = ((1u64 << bits) - 1) << off;
        self.data[w] = (self.data[w] & !mask) | (((id as u64) << off) & mask);
    }

    #[inline]
    fn read(&self, index: usize) -> u16 {
        if self.bits == 0 {
            return self.palette[0];
        }
        let bits = self.bits as usize;
        let per_word = 64 / bits;
        let w = index / per_word;
        let off = (index % per_word) * bits;
        let id = ((self.data[w] >> off) & ((1u64 << bits) - 1)) as u16;
        self.palette[id as usize]
    }

    /// (x,y,z 0..15) → block_state
    ///
    /// **契約 (wave 102 DB-5)**: x/y/z は 0..16 限定 (debug_assert)。
    /// release で範囲外を与えると index が別セルへエイリアスして静寂に
    /// 誤読される (ホットパスのため debug_assert 据置、呼出側で保証すること)。
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> u16 {
        debug_assert!(x < 16 && y < 16 && z < 16);
        self.read((y << 8) | (z << 4) | x)
    }

    /// ブロックを更新。新種なら必要に応じて自動拡張。
    /// 座標契約は `get` と同じ (release での範囲外は別セルの静寂誤更新)。
    pub fn set(&mut self, x: usize, y: usize, z: usize, block: u16) {
        debug_assert!(x < 16 && y < 16 && z < 16);
        let idx = (y << 8) | (z << 4) | x;
        let id = match self.rev.get(&block) {
            Some(&id) => {
                // wave 102 DB-4: 単一値セクション (bits == 0) にその唯一の値を
                // set しても状態は不変で no-op。旧実装はここで 64 語 (512B) を
                // 確保して静寂に「単一値脱却」していた (10B → 522B)。早期復帰。
                if self.bits == 0 {
                    return;
                }
                id
            }
            None => {
                let new_id = self.palette.len() as u16;
                self.palette.push(block);
                self.rev.insert(block, new_id);
                let need = needed_bits(self.palette.len() as u32);
                if need > self.bits {
                    // 新種追加時は必ず need >= 1 なので bits == 0 からの脱却
                    // (data 確保) はここで完了する (後段に同一処理の到達不能
                    // な死にコードがあったため除去。DB-4 同梱)。
                    self.grow_bits(need);
                }
                new_id
            }
        };
        self.write(idx, id);
    }

    fn grow_bits(&mut self, new_bits: u8) {
        if new_bits <= self.bits {
            return;
        }
        // 全読み出し → 新 bits で詰替え
        let old = std::mem::take(&mut self.data);
        let old_bits = self.bits;
        self.bits = new_bits;
        let bits = new_bits as usize;
        let per_word = 64 / bits;
        let words = (SECTION_VOLUME + per_word - 1) / per_word;
        self.data = vec![0u64; words];
        if old_bits > 0 {
            let old_per_word = 64 / old_bits as usize;
            for i in 0..SECTION_VOLUME {
                let w = i / old_per_word;
                let off = (i % old_per_word) * old_bits as usize;
                let id = ((old[w] >> off) & ((1u64 << old_bits) - 1)) as u16;
                self.write(i, id);
            }
        }
    }
}

impl Default for PackedSection {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn needed_bits(n: u32) -> u8 {
    if n <= 1 {
        0
    } else {
        let b = 32 - (n - 1).leading_zeros() as u8;
        b.clamp(1, 15)
    }
}

/// チャンク列 (16 セクション: 256 高) の圧縮統計。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnStats {
    pub sections: usize,
    pub raw_bytes: usize,
    pub packed_bytes: usize,
    pub single_value_sections: usize,
}

impl ColumnStats {
    /// packed/raw (> 1.0 の膨張ケースもあり得る: 全 4096 相異で ≈1.80。
    /// 空列 (raw_bytes == 0) では定義上 1.0 を返す)。
    pub fn ratio(&self) -> f64 {
        if self.raw_bytes == 0 {
            1.0
        } else {
            self.packed_bytes as f64 / self.raw_bytes as f64
        }
    }
}

pub fn stats_for(sections: &[PackedSection]) -> ColumnStats {
    let raw = sections.len() * SECTION_VOLUME * 2;
    let packed: usize = sections.iter().map(|s| s.memory_bytes()).sum();
    let single = sections.iter().filter(|s| s.bits_per_entry() == 0).count();
    ColumnStats {
        sections: sections.len(),
        raw_bytes: raw,
        packed_bytes: packed,
        single_value_sections: single,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_value_section_uses_no_data() {
        let blocks = [3u16; SECTION_VOLUME];
        let s = PackedSection::from_blocks(&blocks);
        assert_eq!(s.bits_per_entry(), 0);
        assert_eq!(s.memory_bytes(), 2 + 8);
        assert_eq!(s.get(5, 5, 5), 3);
        let st = stats_for(&[s]);
        assert_eq!(st.single_value_sections, 1);
        assert!(st.ratio() < 0.01);
    }

    #[test]
    fn two_states_fit_one_bit() {
        let mut blocks = [0u16; SECTION_VOLUME];
        for i in 0..2048 {
            blocks[i] = 1;
        }
        let s = PackedSection::from_blocks(&blocks);
        assert_eq!(s.bits_per_entry(), 1);
        // 2048 ブロック目は id 1、残りは air
        assert_eq!(s.get(0, 7, 15), 1); // index 2047 = y=7,z=15,x=15
        assert_eq!(s.get(0, 8, 0), 0);
        // 1bit*4096 = 512B + パレット 4B + ヘッダ
        assert!(s.memory_bytes() < 600);
    }

    #[test]
    fn set_grows_bits_and_preserves_data() {
        let mut s = PackedSection::from_blocks(&[0u16; SECTION_VOLUME]);
        // 新種を次々足して bits 成長を強制
        s.set(0, 0, 0, 1);
        assert_eq!(s.bits_per_entry(), 1);
        s.set(1, 0, 0, 2);
        assert_eq!(s.bits_per_entry(), 2);
        s.set(2, 0, 0, 3);
        assert_eq!(s.bits_per_entry(), 2);
        s.set(3, 0, 0, 4);
        s.set(4, 0, 0, 5);
        assert!(s.bits_per_entry() >= 3, "5 種で 3bit");
        // 昔の値が壊れていないこと
        assert_eq!(s.get(0, 0, 0), 1);
        assert_eq!(s.get(1, 0, 0), 2);
        assert_eq!(s.get(2, 0, 0), 3);
        assert_eq!(s.get(3, 0, 0), 4);
        assert_eq!(s.get(4, 0, 0), 5);
        assert_eq!(s.get(5, 0, 0), 0);
    }

    #[test]
    fn roundtrip_random_mix() {
        let mut blocks = [0u16; SECTION_VOLUME];
        let mut seed = 99u64;
        let mut rng = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for (i, b) in blocks.iter_mut().enumerate() {
            if rng() % 4 == 0 {
                *b = (rng() % 300) as u16;
            }
            let _ = i;
        }
        let s = PackedSection::from_blocks(&blocks);
        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    let i = (y << 8) | (z << 4) | x;
                    assert_eq!(s.get(x, y, z), blocks[i], "mismatch at {x},{y},{z}");
                }
            }
        }
        let st = stats_for(&[s]);
        assert!(st.ratio() < 0.7, "300種入りでも生 8KB より明確に小さい: {}", st.ratio());
    }
}

/// wave 102 (DB) で追加した厳密ピンテスト群。
/// 全数値は Python 機械検算済 (bits 境界表・メモリモデル・ワイヤ語・f64 bit)。
#[cfg(test)]
mod strict_tests {
    use super::*;

    /// needed_bits の厳密境界表 (ids 0..n-1 を表す最小 bits)。
    #[test]
    fn needed_bits_boundary_exact() {
        for (n, expect) in [
            (0u32, 0u8),
            (1, 0),
            (2, 1),
            (3, 2),
            (4, 2),
            (5, 3),
            (16, 4),
            (17, 5),
            (256, 8),
            (257, 9),
            (322, 9),
            (4096, 12), // セクション最大ユニーク数 (フォールバック不要の数学的根拠)
        ] {
            assert_eq!(needed_bits(n), expect, "n={n}");
        }
    }

    /// メモリモデル厳密値 + DB-2 の膨張誠実性ピン (worst ≈ 1.80 倍)。
    #[test]
    fn memory_model_and_expansion_exact() {
        // n=1 (単一値): パレット 2B + 定数 8 = 10B
        let s1 = PackedSection::from_blocks(&[7u16; SECTION_VOLUME]);
        assert_eq!(s1.memory_bytes(), 10);
        // n=2 (前半 1, 後半 0): bits=1, words=64 → 4+512+8 = 524B
        let mut b2 = [0u16; SECTION_VOLUME];
        for b in b2.iter_mut().take(2048) {
            *b = 1;
        }
        let s2 = PackedSection::from_blocks(&b2);
        assert_eq!(s2.memory_bytes(), 524);
        let st2 = stats_for(std::slice::from_ref(&s2));
        // ratio = 524/8192 = 0.06396484375 の f64 bit ピン
        assert_eq!(st2.ratio().to_bits(), 0x3fb0_6000_0000_0000);
        // n=16: bits=4, words=256 → 32+2048+8 = 2088B
        let mut b16 = [0u16; SECTION_VOLUME];
        for (i, b) in b16.iter_mut().enumerate() {
            *b = (i % 16) as u16;
        }
        assert_eq!(PackedSection::from_blocks(&b16).memory_bytes(), 2088);
        // n=322: bits=9, words=586 → 644+4688+8 = 5340B
        let mut b322 = [0u16; SECTION_VOLUME];
        for (i, b) in b322.iter_mut().enumerate() {
            *b = (i % 322) as u16;
        }
        assert_eq!(PackedSection::from_blocks(&b322).memory_bytes(), 5340);
        // n=4096 全相異: bits=12, words=820 → 8192+6560+8 = 14760B = 1.80 倍膨張
        // (旧 doc「最大 ~1/4」はこの敵対ケースで偽。実測ピンで誠実化の鎮座)
        let mut b4096 = [0u16; SECTION_VOLUME];
        for (i, b) in b4096.iter_mut().enumerate() {
            *b = i as u16;
        }
        let s4096 = PackedSection::from_blocks(&b4096);
        assert_eq!(s4096.bits_per_entry(), 12);
        assert_eq!(s4096.memory_bytes(), 14760);
        let st4096 = stats_for(std::slice::from_ref(&s4096));
        // ratio = 14760/8192 = 1.8017578125 の f64 bit ピン
        assert_eq!(st4096.ratio().to_bits(), 0x3ffc_d400_0000_0000);
        assert!(st4096.ratio() > 1.8, "膨張ケースを >1.8 に固定");
    }

    /// 12bit 語パッキングのワイヤ厳密ピン (per_word=5, 跨ぎなし = MC 1.16+ 同型)。
    #[test]
    fn wire_layout_12bit_exact() {
        let mut b = [0u16; SECTION_VOLUME];
        for (i, v) in b.iter_mut().enumerate() {
            *v = i as u16;
        }
        let s = PackedSection::from_blocks(&b);
        // 語 k は 5 エントリ (60bit 使用, 跨ぎなし): 語 0 = ids 0-4, 語 1 = ids 5-9。
        // 語数 = ceil(4096/5) = 820、index 4095 は語 819 の off 0 に id 4095。
        assert_eq!(s.data[0], 0x0004_0030_0200_1000u64);
        assert_eq!(s.data[1], 0x0009_0080_0700_6005u64);
        assert_eq!(s.data.len(), 820);
        assert_eq!(s.data[819], 0x0000_0000_0000_0fffu64);
        // 往復完全性 (全 4096 セル)
        for i in 0..SECTION_VOLUME {
            let (x, y, z) = (i & 15, i >> 8, (i >> 4) & 15);
            assert_eq!(s.get(x, y, z), i as u16, "index {i}");
        }
    }

    /// DB-4: 単一値セクションへの同値 set は no-op (旧: 10B → 522B 静寂膨張)。
    #[test]
    fn set_same_value_on_single_value_keeps_zero_bits() {
        // 非 air 単一値
        let mut s = PackedSection::from_blocks(&[7u16; SECTION_VOLUME]);
        s.set(3, 3, 3, 7);
        assert_eq!(s.bits_per_entry(), 0);
        assert_eq!(s.memory_bytes(), 10);
        assert_eq!(s.get(3, 3, 3), 7);
        assert_eq!(s.get(0, 0, 0), 7);
        // air 単一値 (new)
        let mut a = PackedSection::new();
        a.set(0, 0, 0, 0);
        assert_eq!(a.bits_per_entry(), 0);
        assert_eq!(a.memory_bytes(), 10);
        // 新種 set は従来通り単一値脱却 + 保存完全性
        s.set(0, 0, 0, 9);
        assert_eq!(s.bits_per_entry(), 1);
        assert_eq!(s.memory_bytes(), 2 * 2 + 64 * 8 + 8); // 524
        assert_eq!(s.get(0, 0, 0), 9);
        assert_eq!(s.get(15, 15, 15), 7);
    }

    /// DB-1 の根拠ピン: 現形式では bits ≤ 12 で完結 (フォールバック不在)。
    #[test]
    fn worst_case_needs_no_fallback() {
        let mut b = [0u16; SECTION_VOLUME];
        for (i, v) in b.iter_mut().enumerate() {
            *v = i as u16;
        }
        let s = PackedSection::from_blocks(&b);
        assert!(s.bits_per_entry() <= 12);
        // 全 65536 u16 状態のうち 4096 個までなら回収可能 (= u16 全域でも
        // セクション内容を完全に保持できる)
        for i in 0..SECTION_VOLUME {
            let (x, y, z) = (i & 15, i >> 8, (i >> 4) & 15);
            assert_eq!(s.get(x, y, z), i as u16);
        }
    }
}
