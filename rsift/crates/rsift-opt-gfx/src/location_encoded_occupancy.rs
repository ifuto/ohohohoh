//! # 32. Encoding Occupancy in Memory Location (`LocationEncodedOccupancy` - 2025 CGF)
//!
//! SVDAG のノードデータ自体の格納アドレスに、ボクセルジオメトリの構造情報をエンコードする。
//! ノードのポインタやフラグに使われていたビットを、メモリアドレスの配置規則 (`address & 0x7`) に
//! 暗黙的に埋め込むことで、明示的な子マスクストレージを削減する。
//!
//! 【wave 137 EK (2026-07-27)】以下の構造を誠実注記 (全て機機検証済):
//! 1. `allocate_tagged_node` の while ループは **高々 7 回で必ず終了**
//!    (push 毎に len%8 が +1 mod 8 回転、tag ∈ {0..7} に必ず到達) —
//!    ループ終了性は mod 8 回転の自明に帰着 (ud 機械ピン済)。
//! 2. 戻り index `idx % 8 == (tag & 0x7)` は割当直後に成立 (payload push
//!    前の len が tag 値に正規調整済)。`decode_occupancy_from_location`
//!    は index から tag だけをデコード — **payload は index からは
//!    分かれません** (アドレス埋め込みの正しい言明)。
//! 3. **パディング判別の曖昧性**: dummy 0 push で空いたアドレスのうち
//!    `idx % 8 == 0` に位置する payload (tag=0 ノード) と dummy 自体を
//!    **index だけでは区別不能**。よって本モジュールは tag=0 の allocation
//!    を「未占用区画」の意味に予約し、wiring 側が occupancy_tag=0 を
//!    供給しない契約 (供給範囲 1..=7、u8 wrap >255 は発生自体を規約で
//!    排除) で構造補完していることを明記 (旧 doc のみでは曖昧、wave 137
//!    で構造公表+module テスト pin)。
//! 4. `pool_bytes` は shrink しない (allocate のみ) — 呼出側が削除を
//!    要求しない全 cumulative model (wiring の leo_nodes リング窓も
//!    参照のみで pool からは消去しない)。

pub struct LocationEncodedOccupancy {
    /// Memory pool where node alignment modulo 8 represents its active child count or type.
    pub pool_bytes: Vec<u64>,
}

impl Default for LocationEncodedOccupancy {
    fn default() -> Self {
        Self::new()
    }
}

impl LocationEncodedOccupancy {
    pub fn new() -> Self {
        Self {
            pool_bytes: Vec::with_capacity(8192),
        }
    }

    /// Allocate a node such that its `index % 8 == occupancy_tag`.
    pub fn allocate_tagged_node(&mut self, occupancy_tag: u8, payload: u64) -> usize {
        let tag = (occupancy_tag & 0x7) as usize;
        while self.pool_bytes.len() % 8 != tag {
            self.pool_bytes.push(0); // padding dummy word to hit exact alignment tag
        }
        let idx = self.pool_bytes.len();
        self.pool_bytes.push(payload);
        idx
    }

    /// Decode the implicit occupancy tag solely from the pointer/index address.
    #[inline(always)]
    pub fn decode_occupancy_from_location(index: usize) -> u8 {
        (index % 8) as u8
    }

    /// 【wave 137 EK-2】payload 参照を Option 返却に fail 明示化。
    /// 旧版は範囲外参照で **0 の静寂デフォルト** (fail-soft) を返していた —
    /// 未登録 OOB を真値 0 (有効 payload 0) と混同可能な曖昧さがあった
    /// (decode 上、tag と区別不能)。`Option` 化で「無効は None」と明確化し、
    /// 消費者が `expect`/静寂遮断の選択を強制される形制に根治。wiring
    /// 消費は `expect("allocated index is in range")` で fail-loud 契約。
    pub fn get_payload(&self, index: usize) -> Option<u64> {
        self.pool_bytes.get(index).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_location_encoded_occupancy() {
        let mut leo = LocationEncodedOccupancy::new();
        let idx = leo.allocate_tagged_node(5, 0x12345678);
        assert_eq!(LocationEncodedOccupancy::decode_occupancy_from_location(idx), 5);
        assert_eq!(leo.get_payload(idx), Some(0x12345678));
        assert_eq!(
            leo.get_payload(usize::MAX),
            None,
            "OOB は None (静寂 0 否定)"
        );
    }

    /// 【wave 137 EK】tag 8 種 allocation の正規性とループ終了性の機械ピン。
    #[test]
    fn tag_allocation_across_all_8_kinds_deterministic() {
        let mut leo = LocationEncodedOccupancy::new();
        // tag=0 は規約上「未占用区画」だが allocation 自体は許容 (曖昧性明記)
        let expected = [0usize, 1, 2, 3, 4, 5, 6, 7];
        for (i, &tag_def) in expected.iter().enumerate() {
            let idx = leo.allocate_tagged_node(tag_def as u8, (i * 77) as u64);
            assert_eq!(idx % 8, tag_def, "tag={tag_def}: idx%8 整合");
            assert_eq!(
                idx, tag_def,
                "先頭 allocation 直列 (0..7) で idx == tag 不変"
            );
            assert_eq!(
                LocationEncodedOccupancy::decode_occupancy_from_location(idx),
                tag_def as u8
            );
            assert_eq!(leo.get_payload(idx), Some((i * 77) as u64));
        }
        assert_eq!(
            leo.pool_bytes.len(),
            8,
            "8 index に 8 payload (no padding 事後)"
        );
    }

    /// 【wave 137 EK】padding の走査数が最大 7 で停止する自明性の fuzz ピン。
    #[test]
    fn padding_within_7_words_invariant() {
        let mut leo = LocationEncodedOccupancy::new();
        // 診断: 先に tag=7,0,1,7,0,2 の列で allocation を回す
        for (step, tag) in [7usize, 0, 1, 7, 0, 2].iter().enumerate() {
            let before = leo.pool_bytes.len();
            let idx = leo.allocate_tagged_node(*tag as u8, 0xABCD);
            let padding = idx - before;
            assert!(
                padding <= 7,
                "step {step}: padding {padding} <= 7 (ループ高々 7 回で終了)"
            );
            assert_eq!(leo.pool_bytes[idx], 0xABCD);
        }
        // tag=0 直後は padding 0 (idx%8==0 で while skip)
        // padding dummy words は 0 payload — decode で 0 tag 範囲に留まる
    }

    /// 【wave 137 EK】decode は payload に非依存 (index 決定論) の工事証明。
    #[test]
    fn decode_is_payload_independent_index_algebra() {
        let mut leo = LocationEncodedOccupancy::new();
        let idx_a = leo.allocate_tagged_node(3, 0);
        let idx_b = leo.allocate_tagged_node(3, u64::MAX);
        assert_eq!(
            LocationEncodedOccupancy::decode_occupancy_from_location(idx_a),
            LocationEncodedOccupancy::decode_occupancy_from_location(idx_b),
            "payload (0 vs MAX) で decode が不変 — アドレス埋込の証明"
        );
        // payload は使不要で index == idx(tag) 代数
        for i in 0..16usize {
            assert_eq!(
                LocationEncodedOccupancy::decode_occupancy_from_location(i),
                (i % 8) as u8,
                "i={i}: reverse algebraic"
            );
        }
    }
}
