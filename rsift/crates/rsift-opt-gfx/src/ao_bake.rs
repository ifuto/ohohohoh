//! AO precompute at mesh time (Tier 1) — Minecraft smooth-lighting corner AO.
//! Bakes 0..=3 into vertex so fragment shader stays cheap (Sodium LightPipeline idea).

/// 3×3×3 neighborhood centered on the face's outward cell.
/// `true` = opaque occluder.
pub type AoNeighborhood = [[[bool; 3]; 3]; 3];

/// Classic Minecraft corner AO: side1 + side2 + corner, clamped 0..=3.
#[inline]
pub fn corner_ao(side1: bool, side2: bool, corner: bool) -> u8 {
    if side1 && side2 {
        return 0;
    }
    let mut v = 0u8;
    if side1 {
        v += 1;
    }
    if side2 {
        v += 1;
    }
    if corner {
        v += 1;
    }
    3 - v.min(3)
}

/// Bake four corners for a face. `face`: 0=+X 1=-X 2=+Y 3=-Y 4=+Z 5=-Z.
/// Neighborhood index: [x+1][y+1][z+1] is center (the face origin block).
///
/// 【wave 167 FM 捕捉 103 保持判定 (directive⑦)】bake_face_ao/AoNeighborhood
/// は外部消費者ゼロだが vanilla 4-corner テーブルの真実装で、消費中の語彙
/// (corner_ao → full_graph_wiring:2681 実消費) の生成元。
/// fm_bake_face_ao_six_face_golden が 6 面全写像を pin (soft_edge 判例同型
/// の保持)。per-corner AO の GPU 消費は現行 pull 形式 (quad 単一 2bit AO)
/// に格納域がなく format 変更を要するため将来登録経路とする。旧 pack_ao4
/// (4×2bit byte packer) は同じ形式非存在で配線不能 (捏造禁止)、消費者・
/// 自家消費・WGSL 参照の皆無を機械確認の上、不可能証明で削除した。
pub fn bake_face_ao(n: &AoNeighborhood, face: u8) -> [u8; 4] {
    // Helper: opaque at offset from center
    let o = |dx: i32, dy: i32, dz: i32| -> bool {
        n[(1 + dx) as usize][(1 + dy) as usize][(1 + dz) as usize]
    };
    match face {
        0 => {
            // +X
            [
                corner_ao(o(1, 0, -1), o(1, -1, 0), o(1, -1, -1)),
                corner_ao(o(1, 0, 1), o(1, -1, 0), o(1, -1, 1)),
                corner_ao(o(1, 0, 1), o(1, 1, 0), o(1, 1, 1)),
                corner_ao(o(1, 0, -1), o(1, 1, 0), o(1, 1, -1)),
            ]
        }
        1 => {
            // -X
            [
                corner_ao(o(-1, 0, 1), o(-1, -1, 0), o(-1, -1, 1)),
                corner_ao(o(-1, 0, -1), o(-1, -1, 0), o(-1, -1, -1)),
                corner_ao(o(-1, 0, -1), o(-1, 1, 0), o(-1, 1, -1)),
                corner_ao(o(-1, 0, 1), o(-1, 1, 0), o(-1, 1, 1)),
            ]
        }
        2 => {
            // +Y
            [
                corner_ao(o(-1, 1, 0), o(0, 1, -1), o(-1, 1, -1)),
                corner_ao(o(1, 1, 0), o(0, 1, -1), o(1, 1, -1)),
                corner_ao(o(1, 1, 0), o(0, 1, 1), o(1, 1, 1)),
                corner_ao(o(-1, 1, 0), o(0, 1, 1), o(-1, 1, 1)),
            ]
        }
        3 => {
            // -Y
            [
                corner_ao(o(-1, -1, 0), o(0, -1, 1), o(-1, -1, 1)),
                corner_ao(o(1, -1, 0), o(0, -1, 1), o(1, -1, 1)),
                corner_ao(o(1, -1, 0), o(0, -1, -1), o(1, -1, -1)),
                corner_ao(o(-1, -1, 0), o(0, -1, -1), o(-1, -1, -1)),
            ]
        }
        4 => {
            // +Z
            [
                corner_ao(o(-1, 0, 1), o(0, -1, 1), o(-1, -1, 1)),
                corner_ao(o(1, 0, 1), o(0, -1, 1), o(1, -1, 1)),
                corner_ao(o(1, 0, 1), o(0, 1, 1), o(1, 1, 1)),
                corner_ao(o(-1, 0, 1), o(0, 1, 1), o(-1, 1, 1)),
            ]
        }
        _ => {
            // -Z
            [
                corner_ao(o(1, 0, -1), o(0, -1, -1), o(1, -1, -1)),
                corner_ao(o(-1, 0, -1), o(0, -1, -1), o(-1, -1, -1)),
                corner_ao(o(-1, 0, -1), o(0, 1, -1), o(-1, 1, -1)),
                corner_ao(o(1, 0, -1), o(0, 1, -1), o(1, 1, -1)),
            ]
        }
    }
}

/// AO (0..=3、vanilla 輝度スケール: 3=無遮蔽) → シェード係数。
/// **単一語彙点**: GPU truth `terrain_vertex_pull.wgsl` fs_pull の
/// `0.55 + f32(light_ao) * 0.15` と同一式であり、CPU 参照
/// `frame_reference::shade` は本関数へ委譲する (wave 167 FM 捕捉 102:
/// 旧 0.2/0.45/0.7/1.0 テーブルは truth と k=0..2 で対立する漂流語彙だった)。
/// 契約: ao ∈ 0..=3 (quad の 2bit AO フィールド)。範囲外は線形外挿。
#[inline]
pub fn ao_to_shade(ao: u8) -> f32 {
    0.55 + ao as f32 * 0.15
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 【wave 167 FM 捕捉 102】GPU truth (terrain_vertex_pull.wgsl fs_pull
    /// `0.55 + f32(light_ao) * 0.15`) との bit 一致 pin。旧 0.2/0.45/0.7/1.0
    /// テーブルは truth と k=0..2 で対立する漂流語彙だった (fm_probe 実機 bit)。
    #[test]
    fn fm_ao_to_shade_truth_bits() {
        let golden = [0x3f0ccccd, 0x3f333334, 0x3f59999a, 0x3f800000];
        for k in 0u8..4 {
            assert_eq!(ao_to_shade(k).to_bits(), golden[k as usize], "k={k}");
        }
    }

    /// 【wave 167 FM 捕捉 103】bake_face_ao 6 面全写像 pin (fm_probe2/3 機械
    /// 導出): 無遮蔽=3 / 全遮蔽=0 / エッジのみ=0 / 対角のみ=2 の万有真理 +
    /// 単一セル 48 golden でコーナ順を含むテーブル構造を固定。
    /// セル順: (du,dv) ∈ (-1,-1),(-1,0),(-1,1),(0,-1),(0,1),(1,-1),(1,0),(1,1)。
    #[test]
    fn fm_bake_face_ao_six_face_golden() {
        let golden: [[[u8; 4]; 8]; 6] = [
            [
                [2, 3, 3, 3],
                [2, 2, 3, 3],
                [3, 2, 3, 3],
                [2, 3, 3, 2],
                [3, 2, 2, 3],
                [3, 3, 3, 2],
                [3, 3, 2, 2],
                [3, 3, 2, 3],
            ],
            [
                [3, 2, 3, 3],
                [2, 2, 3, 3],
                [2, 3, 3, 3],
                [3, 2, 2, 3],
                [2, 3, 3, 2],
                [3, 3, 2, 3],
                [3, 3, 2, 2],
                [3, 3, 3, 2],
            ],
            [
                [2, 3, 3, 3],
                [2, 3, 3, 2],
                [3, 3, 3, 2],
                [2, 2, 3, 3],
                [3, 3, 2, 2],
                [3, 2, 3, 3],
                [3, 2, 2, 3],
                [3, 3, 2, 3],
            ],
            [
                [3, 3, 3, 2],
                [2, 3, 3, 2],
                [2, 3, 3, 3],
                [3, 3, 2, 2],
                [2, 2, 3, 3],
                [3, 3, 2, 3],
                [3, 2, 2, 3],
                [3, 2, 3, 3],
            ],
            [
                [2, 3, 3, 3],
                [2, 3, 3, 2],
                [3, 3, 3, 2],
                [2, 2, 3, 3],
                [3, 3, 2, 2],
                [3, 2, 3, 3],
                [3, 2, 2, 3],
                [3, 3, 2, 3],
            ],
            [
                [3, 2, 3, 3],
                [3, 2, 2, 3],
                [3, 3, 2, 3],
                [2, 2, 3, 3],
                [3, 3, 2, 2],
                [2, 3, 3, 3],
                [2, 3, 3, 2],
                [3, 3, 3, 2],
            ],
        ];
        let cells: [(i32, i32); 8] = [
            (-1, -1),
            (-1, 0),
            (-1, 1),
            (0, -1),
            (0, 1),
            (1, -1),
            (1, 0),
            (1, 1),
        ];
        for f in 0..6usize {
            let (axis, sign) = match f {
                0 => (0usize, 1i32),
                1 => (0, -1),
                2 => (1, 1),
                3 => (1, -1),
                4 => (2, 1),
                _ => (2, -1),
            };
            let tans: [usize; 2] = if axis == 0 {
                [1, 2]
            } else if axis == 1 {
                [0, 2]
            } else {
                [0, 1]
            };
            for (ci, &(du, dv)) in cells.iter().enumerate() {
                let mut n = [[[false; 3]; 3]; 3];
                let mut off = [0i32; 3];
                off[axis] = sign;
                off[tans[0]] = du;
                off[tans[1]] = dv;
                n[(1 + off[0]) as usize][(1 + off[1]) as usize][(1 + off[2]) as usize] = true;
                assert_eq!(
                    bake_face_ao(&n, f as u8),
                    golden[f][ci],
                    "face {f} cell {cells:?}[{ci}]"
                );
            }
            // 万有真理 (観測対称性): 無遮蔽=3・全遮蔽=0
            assert_eq!(bake_face_ao(&[[[false; 3]; 3]; 3], f as u8), [3, 3, 3, 3]);
            assert_eq!(bake_face_ao(&[[[true; 3]; 3]; 3], f as u8), [0, 0, 0, 0]);
        }
    }

    #[test]
    fn enclosed_corner_is_dark() {
        assert_eq!(corner_ao(true, true, true), 0);
        assert_eq!(corner_ao(false, false, false), 3);
    }

    #[test]
    fn face_ao_open_sky() {
        let n = [[[false; 3]; 3]; 3];
        let ao = bake_face_ao(&n, 2);
        assert_eq!(ao, [3, 3, 3, 3]);
    }
    /// 【wave 185 GE フェーズ2 回収】dead code 系 16 例目 (wave 167 FM adversarial (c)
    /// pack_ao4 死救出 非検出、FM 捕捉 103 で呼出ゼロ+格納域非存在+WGSL 消費者皆無の
    /// 3 点不可能証明削除済) の lexeme pin 化。同宣言形の将来復活を静寂に通さない。
    #[test]
    fn ge_removed_pack_ao4_lexeme() {
        let src = include_str!("ao_bake.rs");
        for lex in [concat!("fn pack_", "ao4")] {
            assert!(
                !src.contains(lex),
                "dead code 系削除語彙の宣言形復活を検出 (wave 185 GE lexeme pin)"
            );
        }
    }
}
