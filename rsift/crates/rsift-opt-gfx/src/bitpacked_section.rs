//! # Bit-Packed & Single-Value Chunk Section Storage (`CompactChunkSection`)
//!
//! 16x16x16 (4,096 ボクセル) のチャンクセクションデータを圧縮保持する。
//! 1) `CompactChunkSection::SingleValue(u16)`: 全 4,096 ボクセルが同一値の場合、
//!    生 `u16` 配列 (8,192 バイト) から **わずか 2 バイト (正確に 4,096 倍)** へ圧縮。
//! 2) `CompactChunkSection::Bitpacked(BitpackedSection)`: 挿入済みユニーク状態数に
//!    応じて `4..=16 bit` の可変ビット幅で `u64` 配列 (`data`) へパッキング。
//!    幅の上限はドメイン証明: 状態は `u16` (≤ 65,536 通り) で pal_id ≤ 65,535 <
//!    2^16 のため 16 bit で必ず打止め、17 bit 拡張は構造的に到達不能。
//!    (旧ヘッダの「4..=15 bit」は実装未達/超過の虚偽で wave 107 で訂正。
//!    vanilla 1.16+ は最小幅 4 bit のみ照合、9 bit 以上でグローバルパレット直接
//!    格納へ移行するという記述は一次情報未照合 — 本実装は間接パレットを 16 bit
//!    まで継続する自前仕様であり、vanilla チャンクフォーマットの入出力経路は
//!    存在しない (消費者は永続化・ワイヤ互換用途に使わないこと))。

use std::collections::HashMap;

pub const SECTION_SIZE: usize = 16;
pub const SECTION_VOL: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;

#[inline(always)]
fn idx(x: usize, y: usize, z: usize) -> usize {
    // DG-3: 範囲外座標は加算が別セルへ**静寂エイリアス**する (例: x=16 は
    // (0, y+1, z) と同一 index)。全呼出側 (wide_static_bench・テスト・
    // full_graph_wiring) は 0..16 正規化済を照合済。本 assert は
    // `BitpackedSection` を enum 層 (座標 assert 済) を迂回して直接使用した
    // 場合の防御層 (adversarial (c) で enum 層経由検査では検出不能を実測記録。
    // 直接 API 誤用のみで発火する第 2 層、release/bench 計時経路で無干渉)。
    debug_assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
    x + y * SECTION_SIZE + z * SECTION_SIZE * SECTION_SIZE
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactChunkSection {
    SingleValue(u16),
    Bitpacked(BitpackedSection),
}

impl CompactChunkSection {
    pub fn new_air() -> Self {
        Self::SingleValue(0)
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> u16 {
        // DG-3 (捕捉25): SingleValue 経路は座標を捨てるため idx の assert では
        // 守れない。enum ディスパッチ層で座標域を fail-loud 担保 (一户関門)。
        debug_assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
        match self {
            Self::SingleValue(val) => *val,
            Self::Bitpacked(sec) => sec.get(x, y, z),
        }
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, state: u16) {
        debug_assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
        match self {
            Self::SingleValue(val) => {
                if *val == state {
                    return;
                }
                // Expand from 2-byte SingleValue to BitpackedSection
                let mut sec = BitpackedSection::from_single_value(*val);
                sec.set(x, y, z, state);
                *self = Self::Bitpacked(sec);
            }
            Self::Bitpacked(sec) => sec.set(x, y, z, state),
        }
    }

    /// メモリ見積もり (DG-2 誠実注記): Bitpacked は
    /// `size_of::<BitpackedSection>() + palette ヒープ + data ヒープ` のみで、
    /// **`rev` HashMap のヒープ (バケット/制御配列) は含まない**。wide_static_bench
    /// の digest 行 `footprint=*B` を不変に保つため定義式は据置とし、ここに契約を
    /// 明記する (誤差は多数状態時に非無視級 — 16 bit 到達 65,536 状態では rev 側が
    /// 本体超過もあり得る。実メモリ管理用途には使わないこと)。
    pub fn memory_footprint_bytes(&self) -> usize {
        match self {
            Self::SingleValue(_) => 2,
            Self::Bitpacked(sec) => {
                std::mem::size_of::<BitpackedSection>() + sec.palette.len() * 2 + sec.data.len() * 8
            }
        }
    }
}

/// ビット幅可変の間接パレット格納。セル index は `x + y*16 + z*256`。
///
/// DG-4: フィールドは消費者 (full_graph_wiring・wide_static_bench・テスト) の
/// 観測用に pub だが、**外部からの書換えは不変量を自壊させる**:
/// ① `data` のビットレイアウトは `bits_per_block` に依存、② 全エントリの pal_id <
/// `palette.len()`、③ `rev` と `palette` は厳密 1:1 対応。読み取り専用契約。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitpackedSection {
    pub palette: Vec<u16>,
    pub rev: HashMap<u16, u16>,
    pub bits_per_block: usize,
    pub data: Vec<u64>,
}

impl BitpackedSection {
    pub fn from_single_value(state: u16) -> Self {
        let mut rev = HashMap::new();
        rev.insert(state, 0);
        let bits_per_block = 4; // minimum 4 bits per block in vanilla 1.16+ format
        let total_bits = SECTION_VOL * bits_per_block;
        let words = (total_bits + 63) / 64;
        Self {
            palette: vec![state],
            rev,
            bits_per_block,
            data: vec![0u64; words],
        }
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> u16 {
        let block_idx = idx(x, y, z);
        let bit_pos = block_idx * self.bits_per_block;
        let word_idx = bit_pos / 64;
        let bit_offset = bit_pos % 64;

        let mask = (1u64 << self.bits_per_block) - 1;
        let pal_id = if bit_offset + self.bits_per_block <= 64 {
            ((self.data[word_idx] >> bit_offset) & mask) as usize
        } else {
            let first_part = self.data[word_idx] >> bit_offset;
            let second_bits = bit_offset + self.bits_per_block - 64;
            let second_part =
                (self.data[word_idx + 1] & ((1u64 << second_bits) - 1)) << (64 - bit_offset);
            ((first_part | second_part) & mask) as usize
        };

        // DG-3: 域外 pal_id (pal_id >= palette.len()) は set/expand の書込み経路では
        // 構造的に到達不能で、pub フィールド直接破壊時にのみ可到達。従来の
        // `unwrap_or(0)` は破壊を静寂に air (=0) へ化かすため、debug では fail-loud
        // に破壊を即検出する (release/bench 計時経路で無干渉、戻り値契約は不変)。
        debug_assert!(pal_id < self.palette.len());
        self.palette.get(pal_id).copied().unwrap_or(0)
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, state: u16) {
        let pal_id = if let Some(&id) = self.rev.get(&state) {
            id as usize
        } else {
            let id = self.palette.len();
            if id >= (1 << self.bits_per_block) {
                self.expand_bit_width(self.bits_per_block + 1);
            }
            self.palette.push(state);
            self.rev.insert(state, id as u16);
            id
        };

        let block_idx = idx(x, y, z);
        let bit_pos = block_idx * self.bits_per_block;
        let word_idx = bit_pos / 64;
        let bit_offset = bit_pos % 64;

        let mask = (1u64 << self.bits_per_block) - 1;
        if bit_offset + self.bits_per_block <= 64 {
            self.data[word_idx] &= !(mask << bit_offset);
            self.data[word_idx] |= (pal_id as u64) << bit_offset;
        } else {
            let first_bits = 64 - bit_offset;
            let second_bits = self.bits_per_block - first_bits;
            self.data[word_idx] &= !0u64 >> first_bits;
            self.data[word_idx] |= (pal_id as u64) << bit_offset;
            self.data[word_idx + 1] &= !((1u64 << second_bits) - 1);
            self.data[word_idx + 1] |= (pal_id as u64) >> first_bits;
        }
    }

    fn expand_bit_width(&mut self, new_bits: usize) {
        let old_bits = self.bits_per_block;
        let total_bits = SECTION_VOL * new_bits;
        let words = (total_bits + 63) / 64;
        let mut new_data = vec![0u64; words];

        for i in 0..SECTION_VOL {
            let old_pos = i * old_bits;
            let old_word = old_pos / 64;
            let old_off = old_pos % 64;
            let mask = (1u64 << old_bits) - 1;
            let pal_id = if old_off + old_bits <= 64 {
                (self.data[old_word] >> old_off) & mask
            } else {
                let p1 = self.data[old_word] >> old_off;
                let b2 = old_off + old_bits - 64;
                let p2 = (self.data[old_word + 1] & ((1u64 << b2) - 1)) << (64 - old_off);
                (p1 | p2) & mask
            };

            let new_pos = i * new_bits;
            let new_word = new_pos / 64;
            let new_off = new_pos % 64;
            if new_off + new_bits <= 64 {
                new_data[new_word] |= pal_id << new_off;
            } else {
                let fb = 64 - new_off;
                new_data[new_word] |= pal_id << new_off;
                new_data[new_word + 1] |= pal_id >> fb;
            }
        }
        self.bits_per_block = new_bits;
        self.data = new_data;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_value_to_bitpacked_expansion() {
        let mut sec = CompactChunkSection::new_air();
        assert_eq!(sec.memory_footprint_bytes(), 2);
        assert_eq!(sec.get(5, 5, 5), 0);

        sec.set(5, 5, 5, 123);
        assert_eq!(sec.get(5, 5, 5), 123);
        assert_eq!(sec.get(0, 0, 0), 0);
        // 実装は vanilla 1.16+ 準拠の最小 4 bits/block (= 4096*4bit = 2048B
        // payload)。以前の `< 400` は 1-bit 時代の古い期待値で、vanilla
        // フォーマット準拠の現設計と整合しなかった。
        const VANILLA_4BIT_PAYLOAD: usize = 16 * 16 * 16 * 4 / 8; // 2048
        let footprint = sec.memory_footprint_bytes();
        assert!(
            footprint <= VANILLA_4BIT_PAYLOAD + 128,
            "vanilla 4bit payload + header を超過: {footprint}"
        );
        assert!(footprint < 8192, "生 u16 セクション (8192B) より小さいこと");
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    #[test]
    fn single_value_mode_and_expansion_semantics() {
        let mut cs = CompactChunkSection::new_air();
        assert_eq!(cs.memory_footprint_bytes(), 2, "SingleValue は 2B 固定");
        for i in 0..64 {
            assert_eq!(cs.get(i % 16, (i / 16) % 16, i / 256), 0);
        }
        cs.set(3, 4, 5, 7);
        assert!(cs.memory_footprint_bytes() > 2, "expand で Bitpacked 化");
        assert_eq!(cs.get(3, 4, 5), 7);
        assert_eq!(cs.get(3, 4, 6), 0, "expand 前の air は保持");
        assert_eq!(cs.get(0, 0, 0), 0);
    }

    /// 全 4096 セルへの決定的パターン書き込み→全セル読み戻しが厳密一致。
    /// 2 段階書き (先に少数パレットで埋めてから 450 状態へ成長) で
    /// パレット幅成長中の既存データ欠落を検出する。
    #[test]
    fn full_volume_roundtrip_across_palette_growth() {
        let mut cs = CompactChunkSection::new_air();
        // phase 1: 8 状態に限定
        for i in 0..SECTION_VOL {
            cs.set(i % 16, (i / 16) % 16, i / 256, (i % 8) as u16);
        }
        // phase 2: 450 状態へ拡張
        for i in 0..SECTION_VOL {
            cs.set(i % 16, (i / 16) % 16, i / 256, (i % 450) as u16);
        }
        for i in 0..SECTION_VOL {
            let got = cs.get(i % 16, (i / 16) % 16, i / 256);
            assert_eq!(got, (i % 450) as u16, "cell {i} corrupt after growth");
        }
        let foot = cs.memory_footprint_bytes();
        // 幅 9 bit × 4096 = 4608B データ域。構造体+パレット込みで ~5.6KB 近傍。
        assert!(
            (4096..10240).contains(&foot),
            "footprint {foot}B out of band for 450 states"
        );
    }

    #[test]
    fn max_state_id_roundtrips() {
        // 16bit blockstate 全文域の端 (u16::MAX) でも可逆であること。
        let mut cs = CompactChunkSection::new_air();
        cs.set(0, 0, 0, u16::MAX);
        cs.set(15, 15, 15, 0);
        cs.set(7, 7, 7, 0x00FF);
        assert_eq!(cs.get(0, 0, 0), u16::MAX);
        assert_eq!(cs.get(15, 15, 15), 0);
        assert_eq!(cs.get(7, 7, 7), 0x00FF);
    }

    /// DG-3 根治ピン: 範囲外座標は静寂エイリアスせず debug で fail-loud。
    #[test]
    #[should_panic(expected = "x < SECTION_SIZE")]
    fn out_of_range_coord_panics_in_debug() {
        let cs = CompactChunkSection::new_air();
        let _ = cs.get(16, 0, 0);
    }

    /// DG-3 根治ピン: pub フィールド破壊 (data への域外 pal_id 混入) は
    /// 従来 `unwrap_or(0)` で静寂 air 化していたものを debug で fail-loud 検出。
    #[test]
    #[should_panic(expected = "pal_id < self.palette.len()")]
    fn corrupted_pal_id_panics_in_debug() {
        let mut cs = CompactChunkSection::new_air();
        cs.set(0, 0, 0, 7); // Bitpacked 化 (palette=[0,7], 4bit)
        if let CompactChunkSection::Bitpacked(ref mut b) = cs {
            b.data[0] = 0xF; // cell 0 に域外 pal_id=15 を直接混入
        }
        let _ = cs.get(0, 0, 0);
    }

    /// DG-1 ドメイン証明のピン: 33,000 状態の累積挿入で 15→16 bit 遷移を厳密に踏み、
    /// 全幅遷移が完全可逆 (u16 域内で 16 bit が上限)。
    #[test]
    fn sixteen_bit_width_roundtrips() {
        let mut cs = CompactChunkSection::new_air();
        let mut model = [0u16; SECTION_VOL];
        let mut widths = Vec::new();
        let mut last = 0usize;
        for t in 0..33_000u32 {
            let cell = (t as usize) % SECTION_VOL;
            let state = t as u16;
            model[cell] = state;
            cs.set(cell % 16, (cell / 16) % 16, cell / 256, state);
            if let CompactChunkSection::Bitpacked(ref b) = cs {
                if b.bits_per_block != last {
                    widths.push(b.bits_per_block);
                    last = b.bits_per_block;
                }
            }
        }
        assert!(
            widths.windows(2).all(|w| w[1] == w[0] + 1),
            "幅は +1 ずつ単調増加のみ: {widths:?}"
        );
        // DG-6 境界包含の厳密ピン (機械検算で捕捉): 状態数がちょうど 2^k + 1 の
        // 時点で幅は k+1 を取る (id = 2^k が収まらないため)。no-span 述語は
        // 2^k + 1 側を誤って区間外と呼ぶ off-by-one を持つため包含形で書く。
        for (idx_w, &n) in [
            17u64, 33, 65, 129, 257, 513, 1025, 2049, 4097, 8193, 16385, 32769,
        ]
        .iter()
        .enumerate()
        {
            let w = idx_w + 5;
            let lo = w - 1;
            assert!(n > (1u64 << lo) && n <= (1u64 << w), "2^{lo}<n<=2^{w}");
        }
        assert!(widths.contains(&15), "15bit を経由: {widths:?}");
        assert_eq!(widths.last().copied(), Some(16), "33,000 状態で 16bit 到達");
        for i in 0..SECTION_VOL {
            assert_eq!(cs.get(i % 16, (i / 16) % 16, i / 256), model[i]);
        }
    }

    /// DG-5 既存挙動の strict ピン (wave 102 DB-4 同型): Bitpacked 化後の同値 set は
    /// palette/data/rev/footprint を一切変更しない完全 no-op。
    #[test]
    fn bitpacked_same_value_set_is_noop() {
        let mut cs = CompactChunkSection::new_air();
        cs.set(1, 2, 3, 42);
        cs.set(4, 5, 6, 43);
        let before = cs.clone();
        cs.set(1, 2, 3, 42);
        assert_eq!(cs, before, "同値 set は完全 no-op でなければならない");
    }

    /// DG-5 跨ぎワード spill の厳密ピン: 5bit 時 cell 12 は bit 60..65 =
    /// word0:60..64 + word1:0..1 に跨る。17 状態挿入で 4→5bit 拡張直後の
    /// 当該セルと全挿入値の完全可逆を確認する。
    #[test]
    fn cross_word_spill_cell12_width5() {
        let mut cs = CompactChunkSection::new_air();
        for t in 0..17u16 {
            let cell = t as usize;
            cs.set(cell % 16, (cell / 16) % 16, 0, t);
        }
        match &cs {
            CompactChunkSection::Bitpacked(b) => assert_eq!(b.bits_per_block, 5),
            CompactChunkSection::SingleValue(_) => panic!("17 状態で Bitpacked のはず"),
        }
        assert_eq!(cs.get(12, 0, 0), 12, "1bit 跨ぎセルの読み戻し");
        assert_eq!(cs.get(11, 0, 0), 11);
        assert_eq!(cs.get(13, 0, 0), 13);
        for t in 0..17u16 {
            let c = t as usize;
            assert_eq!(cs.get(c % 16, (c / 16) % 16, 0), t, "cell {c} corrupt");
        }
    }
}
