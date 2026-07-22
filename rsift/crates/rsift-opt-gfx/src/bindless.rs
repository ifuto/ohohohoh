//! Bindless / descriptor-indexing handle packing for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): pack a `(set, binding, array_index)` triple into a
//! single `u32` and unpack it back. Bindless rendering collapses thousands of
//! material/texture binds into one descriptor array, slashing CPU bind-setup
//! and helping integrated GPUs stream many materials cheaply.

/// Pack `(set, binding, index)` into one handle.
/// Layout (32 bits): `[ set:4 | binding:8 | index:20 ]`.
///
/// **契約 (2026-07-23 wave 46 厳格化)**: `set < 16` かつ `binding < 256`
/// かつ `index < 2^20` 必須。旧実装は `& 0xF` 等のマスクで範囲外入力を
/// **静寂に切り捨て**、例えば set=16 のハンドルが set=0 と完全衝突し得た
/// (非単射 — 誤テクスチャ参照に直結。旧テストは wrap を期待仕様として
/// 祝っていた)。パッキングの要請は**全単射**なので fail-loud に根治。
/// WGSL 側 (cs_unpack_handles) は契約内入力では本実装と bitwise 一致
/// (GPU は assert 不可能なため mask 実装のまま防御整合)。
pub fn pack_handle(set: u32, binding: u32, index: u32) -> u32 {
    assert!(
        set < 16,
        "pack_handle 契約違反: set {set} >= 16 (4-bit 領域超過)"
    );
    assert!(
        binding < 256,
        "pack_handle 契約違反: binding {binding} >= 256 (8-bit 領域超過)"
    );
    assert!(
        index < (1 << 20),
        "pack_handle 契約違反: index {index} >= 2^20 (20-bit 領域超過)"
    );
    ((set & 0xF) << 28) | ((binding & 0xFF) << 20) | (index & 0xFFFFF)
}

/// Inverse of `pack_handle`.
pub fn unpack_handle(h: u32) -> (u32, u32, u32) {
    ((h >> 28) & 0xF, (h >> 20) & 0xFF, h & 0xFFFFF)
}

pub fn wgsl_source() -> &'static str {
    BINDLESS_WGSL
}

pub const BINDLESS_WGSL: &str = include_str!("../shaders/bindless.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_preserves_values() {
        let h = pack_handle(3, 42, 123456);
        let (s, b, i) = unpack_handle(h);
        assert_eq!((s, b, i), (3, 42, 123456));
    }
    #[test]
    fn distinct_inputs_distinct_handles() {
        let a = pack_handle(0, 0, 1);
        let b = pack_handle(0, 0, 2);
        let c = pack_handle(1, 0, 1);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }
    /// wave 46-1: レイアウトのビット位置を端点で機械ピン
    /// ([ set:4 | binding:8 | index:20 ] の厳密配置)。
    #[test]
    fn layout_bit_positions_pinned() {
        assert_eq!(pack_handle(0, 0, 0), 0x0000_0000);
        assert_eq!(pack_handle(8, 0, 0), 0x8000_0000, "set は bit 28..32");
        assert_eq!(
            pack_handle(0, 0x80, 0),
            0x0800_0000,
            "binding は bit 20..28"
        );
        assert_eq!(
            pack_handle(0, 0, 0x8_0000),
            0x0008_0000,
            "index は bit 0..20"
        );
        // 全ビット使用の端点: 15/255/0xFFFFF → 0xFFFF_FFFF
        assert_eq!(pack_handle(15, 255, 0xF_FFFF), 0xFFFF_FFFF);
        assert_eq!(unpack_handle(0xFFFF_FFFF), (15, 255, 0xF_FFFF));
        assert_eq!(unpack_handle(0), (0, 0, 0));
    }

    /// wave 46-2: roundtrip は全単射領域の構造部分集合で厳密
    /// (set 全域 × binding/index の 2 進境界点)。
    #[test]
    fn roundtrip_exhaustive_on_structure() {
        for set in 0..16u32 {
            for &binding in &[0u32, 1, 127, 128, 254, 255] {
                for &index in &[0u32, 1, 0x7_FFFF, 0x8_0000, 0xF_FFFE, 0xF_FFFF] {
                    let h = pack_handle(set, binding, index);
                    assert_eq!(
                        unpack_handle(h),
                        (set, binding, index),
                        "roundtrip ({set}, {binding}, {index})"
                    );
                }
            }
        }
    }

    /// wave 46-3: 範囲外入力は fail-loud (旧実装の静寂切捨て wrap を根治。
    /// 旧テスト index_overflow_wraps_within_field は非単射を祝う誤りだった
    /// ため、このピンに置き換える)。
    #[test]
    #[should_panic(expected = "pack_handle 契約違反: index 1048576")]
    fn pack_rejects_out_of_range_index() {
        // 2^20 (= 1048576) は 20-bit 領域の最初の範囲外値
        // (0x1FFFFF = 2^21−1 と混同しないこと — 2^20−1 = 0xFFFFF)。
        let _ = pack_handle(0, 0, 1 << 20);
    }

    #[test]
    #[should_panic(expected = "set 16 >= 16")]
    fn pack_rejects_out_of_range_set() {
        let _ = pack_handle(16, 0, 0);
    }

    #[test]
    #[should_panic(expected = "binding 256 >= 256")]
    fn pack_rejects_out_of_range_binding() {
        let _ = pack_handle(0, 256, 0);
    }
}
