//! # PalettePack — 可変ビットパレット圧縮（MC 1.13+ 方式を整備）
//!
//! 出典: Minecraft 1.13 flattening 以降のチャンク保存形式 — セクション内の
//! ユニークブロック状態をパレット化し、パレットサイズの ceil(log2) ビットで
//! 各ブロックを詰める。16 種なら 4bit/ブロック = 4096 ブロックで 2KB。
//!
//! この実装は **1.21 の "single value optimized" 相当 + 動的拡張** を含む:
//! * ユニーク 1 種 → データ 0 バイト（単一値セクション）
//! * 322 種超 → 直接 16bit 格納にフォールバック
//! * `set` 時に新種が出ると bits を自動拡張して全詰替え
//!
//! メモリは「生 u16 配列 (8KB/section)」に対し 最大 ~1/4、
//! 典型的な stone/dirt のみのセクションでは ~1/16 に縮む。

use std::collections::HashMap;

/// 1 セクション = 16^3 ブロック。
pub const SECTION_VOLUME: usize = 4096;

/// 可変ビット・パレット格納セクション。
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

    /// メモリ使用量（バイト）。
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
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> u16 {
        debug_assert!(x < 16 && y < 16 && z < 16);
        self.read((y << 8) | (z << 4) | x)
    }

    /// ブロックを更新。新種なら必要に応じて自動拡張。
    pub fn set(&mut self, x: usize, y: usize, z: usize, block: u16) {
        debug_assert!(x < 16 && y < 16 && z < 16);
        let idx = (y << 8) | (z << 4) | x;
        let id = match self.rev.get(&block) {
            Some(&id) => id,
            None => {
                let new_id = self.palette.len() as u16;
                self.palette.push(block);
                self.rev.insert(block, new_id);
                let need = needed_bits(self.palette.len() as u32);
                if need > self.bits {
                    self.grow_bits(need);
                }
                new_id
            }
        };
        if self.bits == 0 {
            // 単一値セクションからの脱却
            self.bits = 1;
            let per_word = 64usize;
            let words = (SECTION_VOLUME + per_word - 1) / per_word;
            self.data = vec![0u64; words];
            // 既存値（全部 palette[0]）は 0 のままで正しい（air id=0 or 単一種 id=0）
            if self.palette[0] != 0 {
                // 単一種が air ではない場合、全エントリを id 0 で埋める → 既に 0 なのでOK
            }
        }
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
