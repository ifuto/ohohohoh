//! 32-bit (4-byte) voxel vertex packing — Voxel Game Mesh Optimizations layout.
//!
//! ```text
//! word0 (per-quad origin / block anchor):
//!   x[5:0] y[11:6] z[17:12] tex[29:18] light_ao[31:30]  = 32 bits
//!
//! word1 (greedy quad metadata — face + extent, no IBO):
//!   face[2:0] width_m1[8:3] height_m1[14:9]  (w,h in blocks, 1..64)
//! ```
//!
//! UV (0 bit) and normals (0 bit) are reconstructed in WGSL from
//! `vertex_index % 4` (corner) and `word1.face` (cube face).
//!
//! 語彙規約 (2026-07-23 wave 55 明文化):
//! - 実セクションは `binary_greedy_meshing::SECTION_SIZE = 16` (16³) で、
//!   座標の実供給域は [0,16)。語彙は参照設計どおり 6bit (64 まで表現可) で、
//!   リージョン再パック経路 (binary_greedy_meshing:743) は 0..64 フィルタ後に
//!   梱包する。旧 `PULL_CHUNK_VOXELS = 32` 定数は SECTION_SIZE=16 と矛盾
//!   する誤誘導だったため撤去 (消費者ゼロ実測)。
//! - `tex` は 12bit アトラス語彙 (< 4096)。パレットは [u16; 4096] (=65535
//!   まで格納可) なので、語彙超過のパレット値を梱包すると旧実装では
//!   **上位フィールドへ静寂ビット滲出** (release ビルドでは debug_assert が
//!   無効) していた。契約を assert に昇格済 (wave 55 BE-1)。現行供給域は
//!   アトラス index から外れた実ブロック ID を含まない (全生成経路を監査済)。
//! - WGSL (gpu_vertex_pull::SHADER_VERTEX_PULL) は同一ビット語彙の実カーネルで
//!   あり、マスク/シフト値はテストで表記一致ピンされる (BE-3)。

pub const COORD_BITS: u32 = 6;
pub const COORD_MASK: u32 = (1 << COORD_BITS) - 1;
pub const TEX_BITS: u32 = 12;
pub const TEX_MASK: u32 = (1 << TEX_BITS) - 1;
pub const LIGHT_AO_BITS: u32 = 2;
pub const LIGHT_AO_MASK: u32 = (1 << LIGHT_AO_BITS) - 1;

/// Vertices expanded per greedy quad (`draw(0..quads*6)` — no index buffer).
pub const VERTICES_PER_PULL_QUAD: u32 = 6;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PackedPullQuad {
    pub word0: u32,
    pub word1: u32,
}

impl PackedPullQuad {
    /// raw 語彙関数。値域外のフィールドは隣接フィールドへビット滲出する
    /// (throughput 構造ベンチが任意 bit 列を測定できるよう意図的に検査を
    /// 持たない。wide_static_bench E セクションがこの性質を利用)。
    /// **実データの梱包は検査付き `new()` を使うこと** (debug_assert は
    /// 誤用の開発時捕捉用として残置)。
    #[inline]
    pub fn pack_word0(x: u32, y: u32, z: u32, texture_id: u32, light_ao: u32) -> u32 {
        debug_assert!(x < 1 << COORD_BITS);
        debug_assert!(y < 1 << COORD_BITS);
        debug_assert!(z < 1 << COORD_BITS);
        debug_assert!(texture_id < 1 << TEX_BITS);
        debug_assert!(light_ao < 1 << LIGHT_AO_BITS);
        x | (y << 6) | (z << 12) | (texture_id << 18) | (light_ao << 30)
    }

    /// raw 語彙関数 (`pack_word0` 参照)。`-1` シフト前提のため w/h = 0 は
    /// (0-1)<<3 の wrap で巨大値になる — 検査付きは `new()`。
    #[inline]
    pub fn pack_word1(face: u32, width_blocks: u32, height_blocks: u32) -> u32 {
        debug_assert!(face < 6);
        debug_assert!((1..=64).contains(&width_blocks));
        debug_assert!((1..=64).contains(&height_blocks));
        face | ((width_blocks - 1) << 3) | ((height_blocks - 1) << 9)
    }

    /// **検査付き構築入口 (wave 55 BE-1)**: 全実データ生成経路
    /// (binary_greedy_meshing emit_pull_quad / リージョン再梱包、
    /// full_graph_wiring ao_refine) はここを通る。語彙超過は release でも
    /// fail-loud で拒否する (旧来の静寂ビット滲出を遮断)。
    pub fn new(
        x: u32,
        y: u32,
        z: u32,
        texture_id: u32,
        light_ao: u32,
        face: u32,
        width_blocks: u32,
        height_blocks: u32,
    ) -> Self {
        assert!(x < (1 << COORD_BITS), "new 契約違反: x={x} >= 64");
        assert!(y < (1 << COORD_BITS), "new 契約違反: y={y} >= 64");
        assert!(z < (1 << COORD_BITS), "new 契約違反: z={z} >= 64");
        assert!(
            texture_id < (1 << TEX_BITS),
            "new 契約違反: tex={texture_id} >= 4096"
        );
        assert!(
            light_ao < (1 << LIGHT_AO_BITS),
            "new 契約違反: light_ao={light_ao} >= 4"
        );
        assert!(face < 6, "new 契約違反: face={face} >= 6");
        assert!(
            (1..=64).contains(&width_blocks),
            "new 契約違反: width={width_blocks} 範囲外"
        );
        assert!(
            (1..=64).contains(&height_blocks),
            "new 契約違反: height={height_blocks} 範囲外"
        );
        Self {
            word0: Self::pack_word0(x, y, z, texture_id, light_ao),
            word1: Self::pack_word1(face, width_blocks, height_blocks),
        }
    }

    #[inline]
    pub fn unpack_x(w: u32) -> u32 {
        w & COORD_MASK
    }

    #[inline]
    pub fn unpack_y(w: u32) -> u32 {
        (w >> 6) & COORD_MASK
    }

    #[inline]
    pub fn unpack_z(w: u32) -> u32 {
        (w >> 12) & COORD_MASK
    }

    #[inline]
    pub fn unpack_tex(w: u32) -> u32 {
        (w >> 18) & TEX_MASK
    }

    #[inline]
    pub fn unpack_light_ao(w: u32) -> u32 {
        (w >> 30) & LIGHT_AO_MASK
    }

    #[inline]
    pub fn unpack_face(w1: u32) -> u32 {
        w1 & 0x7
    }

    #[inline]
    pub fn unpack_width(w1: u32) -> u32 {
        ((w1 >> 3) & 0x3F) + 1
    }

    #[inline]
    pub fn unpack_height(w1: u32) -> u32 {
        ((w1 >> 9) & 0x3F) + 1
    }

    pub fn memory_bytes() -> usize {
        std::mem::size_of::<Self>()
    }
}

/// Cube face index: +X,-X,+Y,-Y,+Z,-Z matching greedy mesher axis.
/// **契約 (wave 55 BE-2)**: 軸フラグはちょうど 1 つのみ true。旧実装の
/// `_ => 5` は違法組合せ (複数軸/ゼロ軸) を** -Z として静寂誤分類** して
/// いた (WGSL 側 face_normal の default も同値で一貫したゴミポリシ)。
#[inline]
pub fn face_index(axis_x: bool, axis_y: bool, axis_z: bool, positive: bool) -> u32 {
    assert!(
        (axis_x as u32) + (axis_y as u32) + (axis_z as u32) == 1,
        "face_index 契約違反: 軸フラグはちょうど 1 つ ({axis_x}, {axis_y}, {axis_z})"
    );
    match (axis_x, axis_y, axis_z, positive) {
        (true, false, false, true) => 0,
        (true, false, false, false) => 1,
        (false, true, false, true) => 2,
        (false, true, false, false) => 3,
        (false, false, true, true) => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip_32bit() {
        let q = PackedPullQuad::new(31, 16, 8, 4095, 3, 4, 12, 7);
        assert_eq!(PackedPullQuad::unpack_x(q.word0), 31);
        assert_eq!(PackedPullQuad::unpack_y(q.word0), 16);
        assert_eq!(PackedPullQuad::unpack_z(q.word0), 8);
        assert_eq!(PackedPullQuad::unpack_tex(q.word0), 4095);
        assert_eq!(PackedPullQuad::unpack_light_ao(q.word0), 3);
        assert_eq!(PackedPullQuad::unpack_face(q.word1), 4);
        assert_eq!(PackedPullQuad::unpack_width(q.word1), 12);
        assert_eq!(PackedPullQuad::unpack_height(q.word1), 7);
        assert_eq!(std::mem::size_of::<PackedPullQuad>(), 8);
    }

    #[test]
    fn word0_fits_32_bits() {
        let w = PackedPullQuad::pack_word0(63, 63, 63, 4095, 3);
        // u32 への格納自体が「32bit に収まる」の実証 (恒真の `w <= u32::MAX` は削除)。
        assert_eq!(PackedPullQuad::unpack_x(w), 63);
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::*;

    /// 境界値スイープ: 各フィールドの語彙域端 (debug_assert の合法域) で
    /// pack → unpack が完全に往復すること (GPU pull 経路の語彙規約の固定)。
    #[test]
    fn boundary_value_roundtrip_sweep() {
        for &x in &[0u32, 1, 31, 63] {
            for &y in &[0u32, 32, 63] {
                for &z in &[0u32, 15, 63] {
                    let q = PackedPullQuad::new(x, y, z, 4095, 3, 5, 64, 1);
                    assert_eq!(PackedPullQuad::unpack_x(q.word0), x);
                    assert_eq!(PackedPullQuad::unpack_y(q.word0), y);
                    assert_eq!(PackedPullQuad::unpack_z(q.word0), z);
                    assert_eq!(PackedPullQuad::unpack_tex(q.word0), 4095);
                    assert_eq!(PackedPullQuad::unpack_light_ao(q.word0), 3);
                    assert_eq!(PackedPullQuad::unpack_face(q.word1), 5);
                    assert_eq!(PackedPullQuad::unpack_width(q.word1), 64);
                    assert_eq!(PackedPullQuad::unpack_height(q.word1), 1);
                }
            }
        }
    }

    #[test]
    fn word0_fields_do_not_bleed_into_each_other() {
        // tex を最大にしても x/y/z/ao を汚染しない (レイアウト 6+6+6+12+2=32bit 厳密性)。
        let w0 = PackedPullQuad::pack_word0(63, 63, 63, 4095, 3);
        assert_eq!(w0, u32::MAX, "全フィールド max で 32bit 全使用");
        // tex=0 では上位 14bit が ao 2bit 以外全て 0。
        let w1 = PackedPullQuad::pack_word0(0, 0, 0, 0, 0);
        assert_eq!(w1, 0);
    }

    #[test]
    fn word1_face_width_height_isolation() {
        // face 6 通り × (w,h) で相互汚染がないこと (w,h は 1..64 の格納は -1 シフト)。
        for face in 0..6u32 {
            for (w, h) in [(1u32, 1u32), (64, 64), (33, 7)] {
                let w1 = PackedPullQuad::pack_word1(face, w, h);
                assert_eq!(PackedPullQuad::unpack_face(w1), face);
                assert_eq!(PackedPullQuad::unpack_width(w1), w);
                assert_eq!(PackedPullQuad::unpack_height(w1), h);
            }
        }
    }

    #[test]
    fn memory_bytes_is_the_pod_ground_truth() {
        // SSBO アップロード語彙: 8B/quad の wire サイズが型レイアウトと一致。
        assert_eq!(PackedPullQuad::memory_bytes(), 8);
        let q = PackedPullQuad::new(1, 2, 3, 4, 3, 2, 8, 8);
        let bytes: &[u8] = bytemuck::bytes_of(&q);
        assert_eq!(bytes.len(), 8);
        // word0 が LE 先頭 (DX12 SSBO 側の vec2<u32> 読み規約に対応)。
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), q.word0);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), q.word1);
    }

    /// wave 55: word0/word1 の配置を厳密 bit 列でピン
    /// (1|2<<6|3<<12|4<<18|3<<30 と 2|7<<3|5<<9 を独立手導出)。
    #[test]
    fn pack_bit_layout_exact() {
        let w0 = PackedPullQuad::pack_word0(1, 2, 3, 4, 3);
        assert_eq!(w0, 0b11_000000000100_000011_000010_000001);
        let w1 = PackedPullQuad::pack_word1(2, 8, 6);
        assert_eq!(w1, 2618, "face2|(8-1)<<3|(6-1)<<9 = 2|56|2560 の独立導出");
        // width/height = 1 は -1 シフトで 0 格納
        assert_eq!(PackedPullQuad::pack_word1(5, 1, 1), 5);
    }

    /// wave 55 BE-1: 語彙超過の静寂ビット滲出は実データ入口 (new) で拒否
    /// (release ビルドでも assert は有効)。raw 語彙関数はベンチ特約で無検査。
    #[test]
    fn new_rejects_out_of_vocabulary() {
        let cases: Vec<Box<dyn Fn() + Send>> = vec![
            Box::new(|| {
                PackedPullQuad::new(64, 0, 0, 0, 0, 0, 1, 1);
            }),
            Box::new(|| {
                PackedPullQuad::new(0, 64, 0, 0, 0, 0, 1, 1);
            }),
            Box::new(|| {
                PackedPullQuad::new(0, 0, 64, 0, 0, 0, 1, 1);
            }),
            Box::new(|| {
                PackedPullQuad::new(0, 0, 0, 4096, 0, 0, 1, 1);
            }),
            Box::new(|| {
                PackedPullQuad::new(0, 0, 0, 0, 4, 0, 1, 1);
            }),
            Box::new(|| {
                PackedPullQuad::new(0, 0, 0, 0, 0, 6, 1, 1);
            }),
            Box::new(|| {
                PackedPullQuad::new(0, 0, 0, 0, 0, 0, 0, 1);
            }),
            Box::new(|| {
                PackedPullQuad::new(0, 0, 0, 0, 0, 0, 1, 65);
            }),
        ];
        for (i, f) in cases.iter().enumerate() {
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err(),
                "case {i}"
            );
        }
        // raw 語彙関数のデュアルモード: debug では debug_assert が開発時捕捉、
        // release (wide_static_bench E セクション) では無検査スループット
        // (bit 滲出は意図的特約)。
        if cfg!(debug_assertions) {
            let r = std::panic::catch_unwind(|| {
                PackedPullQuad::pack_word0(0, 383, 0, 511, 15);
            });
            assert!(r.is_err(), "debug ビルドでは語彙超過を捕獲");
        } else {
            let w0 = PackedPullQuad::pack_word0(0, 383, 0, 511, 15);
            assert_eq!(PackedPullQuad::unpack_x(w0), 0);
        }
    }

    /// wave 55 BE-2: face_index の 6 面写像厳密ピン + 違法組合せ拒否。
    #[test]
    fn face_index_exact_and_rejects_illegal_axes() {
        assert_eq!(face_index(true, false, false, true), 0);
        assert_eq!(face_index(true, false, false, false), 1);
        assert_eq!(face_index(false, true, false, true), 2);
        assert_eq!(face_index(false, true, false, false), 3);
        assert_eq!(face_index(false, false, true, true), 4);
        assert_eq!(face_index(false, false, true, false), 5);
        for axes in [
            (true, true, false),
            (false, false, false),
            (true, true, true),
        ] {
            let r = std::panic::catch_unwind(|| face_index(axes.0, axes.1, axes.2, true));
            assert!(r.is_err(), "{axes:?} は拒否されるべき");
        }
    }

    /// wave 55 BE-3: WGSL 実カーネルの bit 語彙が Rust 側宣言と表記一致
    /// (片側だけ変更された場合に機械検出する)。
    #[test]
    fn wgsl_unpack_mirrors_rust_vocabulary() {
        let wgsl = crate::gpu_vertex_pull::SHADER_VERTEX_PULL;
        for needle in [
            "const COORD_MASK: u32 = 63u;",
            "const TEX_MASK: u32 = 4095u;",
            "fn unpack_x(w: u32) -> u32 { return w & COORD_MASK; }",
            "fn unpack_y(w: u32) -> u32 { return (w >> 6u) & COORD_MASK; }",
            "fn unpack_z(w: u32) -> u32 { return (w >> 12u) & COORD_MASK; }",
            "fn unpack_tex(w: u32) -> u32 { return (w >> 18u) & TEX_MASK; }",
            "fn unpack_light_ao(w: u32) -> u32 { return (w >> 30u) & 3u; }",
            "fn unpack_face(w1: u32) -> u32 { return w1 & 7u; }",
            "fn unpack_width(w1: u32) -> u32 { return ((w1 >> 3u) & 63u) + 1u; }",
            "fn unpack_height(w1: u32) -> u32 { return ((w1 >> 9u) & 63u) + 1u; }",
        ] {
            assert!(
                wgsl.contains(needle),
                "WGSL 語彙が Rust 宣言と乖離: {needle}"
            );
        }
        // face 写像も WGSL face_normal と一致 (0:+X,1:-X,…,5:-Z)
        let pat = "case 0u: { return vec3<f32>(1.0, 0.0, 0.0); }";
        assert!(wgsl.contains(pat), "face 0 = +X の WGSL 写像が乖離");
        let pat = "default: { return vec3<f32>(0.0, 0.0, -1.0); }";
        assert!(
            wgsl.contains(pat),
            "face 5 (default) = -Z の WGSL 写像が乖離"
        );
    }
}
