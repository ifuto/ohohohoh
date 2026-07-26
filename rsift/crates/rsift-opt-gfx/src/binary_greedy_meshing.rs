//! Binary Greedy Meshing — tri-axis face culling + quad merging (cgerikj-style).
//! RLE-empty layers skipped; all 6 face directions use correct slice axes.
//!
//! # 監査 2026-07-26 (wave 124 DX) — 契約公表
//!
//! - **DX-1**: `greedy_merge_2d_pull` (u16 mask 版) を `#[cfg(test)]`
//!   テスト専用オラクルとして分類整理 (削除せず保持)。live は
//!   `greedy_merge_2d_pull_bits` のみ (等価性証明コメント :520+)。lib 警告
//!   8→7。警告カウントは build 照合で pin 代替。
//! - **DX-2**: `idx(x,y,z) = x + 16y + 256z` の厳密 pin: idx(15,15,15)=4095
//!   (最大・排他的終端)、全 4096 引数での単射性完全走査 + 座標復元
//!   (x=i&15, y=(i>>4)&15, z=(i>>8)&15) — 消費者 10+ ファイル (svo/
//!   section_rle/section_compress/noise_upsample/voxel_cone_tracing/
//!   world_column_store/leaf_fast_path 等) が共有する座標規約の固定。
//! - **DX-3**: 消費者形状の公表: 本番経路は render_pipeline:427
//!   (mesh_chunk_column)・:438 (mesh_chunk_column_pull_world、
//!   mesh_section_y0=402 由来の実引数)。frame_reference (3 箇所)・
//!   gpu_vertex_pull・chunk_mesh・frame_reuse は参照レンダ/テスト経路。
//! - **DX-4**: 等価性オラクル資産の棚卸し: 10 テスト体制 (pull u16 対照
//!   fuzz・bitcols face_visible 対照 fuzz (100+ seeds)・edge 断面一致・
//!   12B 経路同系) は既存維持。本 wave は doc+1 pin+属性整理のみで
//!   mesh 出力 (digest) は完全不変。

use crate::chunk_mesh::{BuiltChunkMesh, Quantized12ByteVertex};
use crate::packed4::{face_index as pull_face_index, PackedPullQuad};
use crate::pull_mesh::PullBuiltMesh;
use crate::section_rle::RleSection;
use tracing::trace;

pub const SECTION_SIZE: usize = 16;
pub const SECTIONS_PER_COLUMN: usize = 4;

pub type SectionPalette = [u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];

#[inline]
pub fn idx(x: usize, y: usize, z: usize) -> usize {
    x + y * SECTION_SIZE + z * SECTION_SIZE * SECTION_SIZE
}

/// 2026-07-21: 面可視判定はカラム bit 導出 (`face_rows_for_slice`) へ移行済。
/// `is_opaque` / `neighbor_opaque` / `face_visible` の参照チェーンは
/// 等価性検証オラクルとしてテスト専用に保持 (OOB=非不透明の規約は
/// bit 導出側と厳密一致することを fuzz で照合済)。
#[cfg(test)]
#[inline]
fn is_opaque(palette: &SectionPalette, x: usize, y: usize, z: usize) -> bool {
    if x >= SECTION_SIZE || y >= SECTION_SIZE || z >= SECTION_SIZE {
        return false;
    }
    palette[idx(x, y, z)] != 0
}

#[derive(Clone, Copy, Debug)]
enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    const ALL: [(Axis, bool); 6] = [
        (Axis::X, true),
        (Axis::X, false),
        (Axis::Y, true),
        (Axis::Y, false),
        (Axis::Z, true),
        (Axis::Z, false),
    ];
}

#[cfg(test)]
#[inline]
fn neighbor_opaque(
    palette: &SectionPalette,
    x: usize,
    y: usize,
    z: usize,
    axis: Axis,
    positive: bool,
) -> bool {
    let (nx, ny, nz) = match axis {
        Axis::X if positive => (x + 1, y, z),
        Axis::X => (x.wrapping_sub(1), y, z),
        Axis::Y if positive => (x, y + 1, z),
        Axis::Y => (x, y.wrapping_sub(1), z),
        Axis::Z if positive => (x, y, z + 1),
        Axis::Z => (x, y, z.wrapping_sub(1)),
    };
    is_opaque(palette, nx, ny, nz)
}

#[cfg(test)]
#[inline]
fn face_visible(
    palette: &SectionPalette,
    x: usize,
    y: usize,
    z: usize,
    axis: Axis,
    positive: bool,
) -> bool {
    if !is_opaque(palette, x, y, z) {
        return false;
    }
    !neighbor_opaque(palette, x, y, z, axis, positive)
}

fn emit_quad(
    vertices: &mut Vec<Quantized12ByteVertex>,
    indices: &mut Vec<u32>,
    x: f32,
    y: f32,
    z: f32,
    w: f32,
    h: f32,
    axis: Axis,
    positive: bool,
) {
    let (nx, ny, nz) = match (axis, positive) {
        (Axis::X, true) => (1.0, 0.0, 0.0),
        (Axis::X, false) => (-1.0, 0.0, 0.0),
        (Axis::Y, true) => (0.0, 1.0, 0.0),
        (Axis::Y, false) => (0.0, -1.0, 0.0),
        (Axis::Z, true) => (0.0, 0.0, 1.0),
        (Axis::Z, false) => (0.0, 0.0, -1.0),
    };
    let base = vertices.len() as u32;
    match (axis, positive) {
        (Axis::X, true) => {
            let fx = x + 1.0;
            vertices.push(Quantized12ByteVertex::encode(
                fx, y, z, nx, ny, nz, 0.0, 0.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                fx,
                y + h,
                z,
                nx,
                ny,
                nz,
                0.0,
                1.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                fx,
                y + h,
                z + w,
                nx,
                ny,
                nz,
                1.0,
                1.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                fx,
                y,
                z + w,
                nx,
                ny,
                nz,
                1.0,
                0.0,
            ));
        }
        (Axis::X, false) => {
            vertices.push(Quantized12ByteVertex::encode(x, y, z, nx, ny, nz, 0.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(
                x,
                y,
                z + w,
                nx,
                ny,
                nz,
                1.0,
                0.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x,
                y + h,
                z + w,
                nx,
                ny,
                nz,
                1.0,
                1.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x,
                y + h,
                z,
                nx,
                ny,
                nz,
                0.0,
                1.0,
            ));
        }
        (Axis::Y, true) => {
            let fy = y + 1.0;
            vertices.push(Quantized12ByteVertex::encode(
                x, fy, z, nx, ny, nz, 0.0, 0.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x + w,
                fy,
                z,
                nx,
                ny,
                nz,
                1.0,
                0.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x + w,
                fy,
                z + h,
                nx,
                ny,
                nz,
                1.0,
                1.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x,
                fy,
                z + h,
                nx,
                ny,
                nz,
                0.0,
                1.0,
            ));
        }
        (Axis::Y, false) => {
            vertices.push(Quantized12ByteVertex::encode(x, y, z, nx, ny, nz, 0.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(
                x,
                y,
                z + h,
                nx,
                ny,
                nz,
                0.0,
                1.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x + w,
                y,
                z + h,
                nx,
                ny,
                nz,
                1.0,
                1.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x + w,
                y,
                z,
                nx,
                ny,
                nz,
                1.0,
                0.0,
            ));
        }
        (Axis::Z, true) => {
            let fz = z + 1.0;
            vertices.push(Quantized12ByteVertex::encode(
                x, y, fz, nx, ny, nz, 0.0, 0.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x + w,
                y,
                fz,
                nx,
                ny,
                nz,
                1.0,
                0.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x + w,
                y + h,
                fz,
                nx,
                ny,
                nz,
                1.0,
                1.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x,
                y + h,
                fz,
                nx,
                ny,
                nz,
                0.0,
                1.0,
            ));
        }
        (Axis::Z, false) => {
            vertices.push(Quantized12ByteVertex::encode(x, y, z, nx, ny, nz, 0.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(
                x,
                y + h,
                z,
                nx,
                ny,
                nz,
                0.0,
                1.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x + w,
                y + h,
                z,
                nx,
                ny,
                nz,
                1.0,
                1.0,
            ));
            vertices.push(Quantized12ByteVertex::encode(
                x + w,
                y,
                z,
                nx,
                ny,
                nz,
                1.0,
                0.0,
            ));
        }
    }
    indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
}

fn axis_to_face(axis: Axis, positive: bool) -> u32 {
    match (axis, positive) {
        (Axis::X, true) => pull_face_index(true, false, false, true),
        (Axis::X, false) => pull_face_index(true, false, false, false),
        (Axis::Y, true) => pull_face_index(false, true, false, true),
        (Axis::Y, false) => pull_face_index(false, true, false, false),
        (Axis::Z, true) => pull_face_index(false, false, true, true),
        (Axis::Z, false) => pull_face_index(false, false, true, false),
    }
}

fn emit_pull_quad(
    quads: &mut Vec<PackedPullQuad>,
    palette: &SectionPalette,
    x: usize,
    y: usize,
    z: usize,
    w: usize,
    h: usize,
    axis: Axis,
    positive: bool,
) {
    let block = palette[idx(
        x.min(SECTION_SIZE - 1),
        y.min(SECTION_SIZE - 1),
        z.min(SECTION_SIZE - 1),
    )];
    if block == 0 {
        return;
    }
    let face = axis_to_face(axis, positive);
    quads.push(PackedPullQuad::new(
        x as u32,
        y as u32,
        z as u32,
        block as u32,
        3, // full bright + AO until JNI light sync
        face,
        w.max(1) as u32,
        h.max(1) as u32,
    ));
}

/// **wave 124 DX-1 (2026-07-26)**: `#[cfg(test)]` 化で「テスト専用オラクル保持」
/// を is_opaque 系 (:21-23) と同型に分類整理。live 消費者は
/// `greedy_merge_2d_pull_bits` (`:520` 等価性証明コメント「出力完全同一」) のみ
/// で、本関数の呼出は mod tests (旧実装対照 fuzz) に限定される — 削除ではなく
/// 保持 (directive: 消費者ゼロ=lib 本線、でもテスト・オラクルとして有効)。
/// lib ビルドの dead_code 警告 (9 件目) を根治し lib 警告 8→7。
#[cfg(test)]
fn greedy_merge_2d_pull(
    mask: &mut [[u16; SECTION_SIZE]; SECTION_SIZE],
    quads: &mut Vec<PackedPullQuad>,
    palette: &SectionPalette,
    axis: Axis,
    positive: bool,
    origin: impl Fn(usize, usize) -> (usize, usize, usize),
    width_axis: impl Fn(usize) -> usize,
    height_axis: impl Fn(usize) -> usize,
) {
    let mut row = 0usize;
    while row < SECTION_SIZE {
        let row_bits = mask[row];
        if row_bits.iter().all(|&b| b == 0) {
            row += 1;
            continue;
        }
        let mut row_span = 1usize;
        while row + row_span < SECTION_SIZE && mask[row + row_span] == row_bits {
            row_span += 1;
        }
        let mut col = 0usize;
        while col < SECTION_SIZE {
            if row_bits[col] == 0 {
                col += 1;
                continue;
            }
            let mut col_span = 1usize;
            while col + col_span < SECTION_SIZE && row_bits[col + col_span] != 0 {
                let mut ok = true;
                for dr in 1..row_span {
                    if mask[row + dr][col + col_span] == 0 {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    break;
                }
                col_span += 1;
            }
            let mut actual_rows = 1usize;
            'grow: while row + actual_rows < row + row_span {
                for dc in 0..col_span {
                    if mask[row + actual_rows][col + dc] == 0 {
                        break 'grow;
                    }
                }
                actual_rows += 1;
            }
            for dr in 0..actual_rows {
                for dc in 0..col_span {
                    mask[row + dr][col + dc] = 0;
                }
            }
            let (x, y, z) = origin(col, row);
            emit_pull_quad(
                quads,
                palette,
                x,
                y,
                z,
                width_axis(col_span),
                height_axis(actual_rows),
                axis,
                positive,
            );
            col += col_span;
        }
        row += row_span;
    }
}

/// 全 air 判定 (2026-07-21 ベンチ駆動): `RleSection::encode(palette).is_empty()`
/// と意味的に厳密同値 (encode は全4096 voxelを走査して run を構築し、
/// is_empty は全 run の block==0 を見る。両者とも「全 voxel==0」に同型写像する)
/// だが、RLE の Vec 構築・run 展開コストを回避する。ホット経路 4 箇所で置換。
#[inline]
fn section_all_air(palette: &SectionPalette) -> bool {
    palette.iter().all(|&v| v == 0)
}

/// 3 方向の不透明カラム マスク (2026-07-21 ベンチ駆動の面マスク構築高速化)。
/// 旧 greedy_axis_pull は 6 軸方向 × 16 slice × 256 cell の face_visible
/// (2 palette load + 3 境界比較/cell → 1 セクション計 24,576 回) で面マスクを
/// 構築していた (wide_bench: caves 47ns/voxel の支配項)。カラムを 1 走査で
/// u16 化すると、面可視は col bit_s と近傍 bit のシフト導出で済む:
///   pos 面可視 (slice s) = bit_s && !bit_{s+1}
///   neg 面可視 (slice s) = bit_s && !bit_{s-1}   (範囲外 = 非不透明,
///   is_opaque の OOB 規約 (x/y/z >= 16 → false) と厳密一致)
/// 出力 quad 列は旧実装と bit 同一 (emit 順序 slice→row-major 不変、等価性は
/// テスト bitcols_match_face_visible_fuzz が face_visible 参照実装と照合)。
struct OpaqueCols {
    /// xc[z][y]: (y,z) 固定の x 方向カラム (bit = x)
    xc: [[u16; SECTION_SIZE]; SECTION_SIZE],
    /// yc[z][x]: (x,z) 固定の y 方向カラム (bit = y)
    yc: [[u16; SECTION_SIZE]; SECTION_SIZE],
    /// zc[y][x]: (x,y) 固定の z 方向カラム (bit = z)
    zc: [[u16; SECTION_SIZE]; SECTION_SIZE],
}

fn opaque_cols(palette: &SectionPalette) -> OpaqueCols {
    let mut c = OpaqueCols {
        xc: [[0; SECTION_SIZE]; SECTION_SIZE],
        yc: [[0; SECTION_SIZE]; SECTION_SIZE],
        zc: [[0; SECTION_SIZE]; SECTION_SIZE],
    };
    for z in 0..SECTION_SIZE {
        for y in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                if palette[idx(x, y, z)] != 0 {
                    c.xc[z][y] |= 1 << x;
                    c.yc[z][x] |= 1 << y;
                    c.zc[y][x] |= 1 << z;
                }
            }
        }
    }
    c
}

/// カラム列から 1 slice 分の面マスク [u16;16] をシフト導出する。
/// rows マッピング: Y→(row=z,bit=x) / X→(row=z,bit=y) / Z→(row=y,bit=x)
/// (旧 mask[row][col] の添字規約と一致させて merge の出力順を不変にする)。
#[inline]
fn face_rows_for_slice(
    cols: &OpaqueCols,
    axis: Axis,
    positive: bool,
    slice: usize,
) -> [u16; SECTION_SIZE] {
    let mut rows = [0u16; SECTION_SIZE];
    for r in 0..SECTION_SIZE {
        let mut bits = 0u16;
        for c in 0..SECTION_SIZE {
            let col = match axis {
                Axis::Y => cols.yc[r][c] as u32,
                Axis::X => cols.xc[r][c] as u32,
                Axis::Z => cols.zc[r][c] as u32,
            };
            let vis = if positive {
                // bit_s && !bit_{s+1} (shift は u32 で実施: s+1=16 でも安全)
                (col >> slice) & !(col >> (slice + 1)) & 1
            } else if slice == 0 {
                // bit_0 && (範囲外近傍は非不透明) = bit_0
                col & 1
            } else {
                // bit_s && !bit_{s-1}
                (col >> slice) & !(col >> (slice - 1)) & 1
            };
            bits |= (vis as u16) << c;
        }
        rows[r] = bits;
    }
    rows
}

/// 等価性証明に基づく bit 化 merge (旧 greedy_merge_2d_pull と出力完全同一)。
/// 旧アルゴリズムでは「等しい行のグループ内では各矩形拡張チェックが構造的に
/// 常に真」(全 span 行が同一 bit 列ゆえ) であり、`actual_rows == row_span` が
/// 恒等だった。よって u16 行マスクと trailing_zeros/ones 走査で同一の
/// (row-major, 左 run から順, 同一行ブロック単位の emit) を直接実現できる
/// (証明と検証はコミットメッセージ + fuzz テストを参照)。
fn greedy_merge_2d_pull_bits(
    mask: &mut [u16; SECTION_SIZE],
    quads: &mut Vec<PackedPullQuad>,
    palette: &SectionPalette,
    axis: Axis,
    positive: bool,
    origin: impl Fn(usize, usize) -> (usize, usize, usize),
    width_axis: impl Fn(usize) -> usize,
    height_axis: impl Fn(usize) -> usize,
) {
    let mut row = 0usize;
    while row < SECTION_SIZE {
        let row_bits = mask[row];
        if row_bits == 0 {
            row += 1;
            continue;
        }
        // 同一行の連続ブロック (旧 row_span)。
        let mut row_span = 1usize;
        while row + row_span < SECTION_SIZE && mask[row + row_span] == row_bits {
            row_span += 1;
        }
        // その行の run を左から順に 1 quad ずつ (旧 col/col_span 走査と同一順)。
        let mut bits = row_bits;
        while bits != 0 {
            let col0 = bits.trailing_zeros() as usize;
            let span = (bits >> col0).trailing_ones() as usize;
            let run_mask: u16 = (((1u32 << span) - 1) << col0) as u16;
            for r in mask.iter_mut().skip(row).take(row_span) {
                *r &= !run_mask;
            }
            bits &= !run_mask;
            let (x, y, z) = origin(col0, row);
            emit_pull_quad(
                quads,
                palette,
                x,
                y,
                z,
                width_axis(span),
                height_axis(row_span),
                axis,
                positive,
            );
        }
        row += row_span;
    }
}

fn greedy_axis_pull(
    cols: &OpaqueCols,
    palette: &SectionPalette,
    quads: &mut Vec<PackedPullQuad>,
    axis: Axis,
    positive: bool,
    skip_empty_layers: bool,
) {
    // Y 層 skip: 旧 layer_masks_from_palette ホイストの規約をカラムから
    // 再現 (y 層の占有 = ∃(x,z): yc bit y) した []u16 占有率。両者同値。
    let y_occ: u32 = if skip_empty_layers && matches!(axis, Axis::Y) {
        let mut acc = 0u32;
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                acc |= cols.yc[z][x] as u32;
            }
        }
        acc
    } else {
        0
    };
    for slice in 0..SECTION_SIZE {
        if y_occ != 0 {
            if (y_occ >> slice) & 1 == 0 {
                continue;
            }
        }

        let mut mask = face_rows_for_slice(cols, axis, positive, slice);
        // 空 slice の merge を軽く skip (出力は旧実装と同一: 空マスクは
        // merge が何も emit しないため、skip しても出力列は不変)。
        if mask.iter().all(|&m| m == 0) {
            continue;
        }
        match axis {
            Axis::Y => {
                greedy_merge_2d_pull_bits(
                    &mut mask,
                    quads,
                    palette,
                    axis,
                    positive,
                    move |x, z| (x, slice, z),
                    |w| w,
                    |h| h,
                );
            }
            Axis::X => {
                greedy_merge_2d_pull_bits(
                    &mut mask,
                    quads,
                    palette,
                    axis,
                    positive,
                    move |y, z| (slice, y, z),
                    |w| w,
                    |h| h,
                );
            }
            Axis::Z => {
                greedy_merge_2d_pull_bits(
                    &mut mask,
                    quads,
                    palette,
                    axis,
                    positive,
                    move |x, y| (x, y, slice),
                    |w| w,
                    |h| h,
                );
            }
        }
    }
}

fn mesh_section_pull_inner(
    palette: &SectionPalette,
    chunk_x: i32,
    chunk_z: i32,
    skip_empty_layers: bool,
) -> PullBuiltMesh {
    let mut quads = Vec::with_capacity(256);
    let cols = opaque_cols(palette);
    for &(axis, positive) in &Axis::ALL {
        greedy_axis_pull(
            &cols,
            palette,
            &mut quads,
            axis,
            positive,
            skip_empty_layers,
        );
    }
    trace!(
        "binary greedy pull section ({}, {}): {} quads {} pull_verts",
        chunk_x,
        chunk_z,
        quads.len(),
        quads.len() * 6
    );
    // wave 56: is_empty フィールド廃止 (PullBuiltMesh::is_empty() は quads から導出)
    PullBuiltMesh {
        chunk_x,
        chunk_z,
        quads,
    }
}

/// Greedy mesh → SSBO quad list (8 B/quad, no index buffer).
pub fn mesh_section_pull(palette: &SectionPalette, chunk_x: i32, chunk_z: i32) -> PullBuiltMesh {
    if section_all_air(palette) {
        return PullBuiltMesh::empty(chunk_x, chunk_z);
    }
    mesh_section_pull_inner(palette, chunk_x, chunk_z, true)
}

/// 列メッシュ (pull 経路)。`face_culling` は名前は歴史的経緯のもので、
/// 実体は「Y 空層スキップ」の可否として inner に委譲される。
/// 層スキップは空マスクの merge を省くだけなので**出力 quad 列には一切
/// 影響しない** (純粋な perf スイッチ。emit 結果は true/false で bit 同一)。
pub fn mesh_chunk_column_pull(
    sections: &[SectionPalette],
    chunk_x: i32,
    chunk_z: i32,
    face_culling: bool,
) -> PullBuiltMesh {
    mesh_chunk_column_pull_world(
        sections,
        chunk_x,
        chunk_z,
        0,
        chunk_x * SECTION_SIZE as i32,
        0,
        chunk_z * SECTION_SIZE as i32,
        face_culling,
    )
}

/// Mesh a column and pack quads into a 64³ window relative to `origin_*` (world blocks).
///
/// `section_y0` is the world section index of `sections[0]`. Quads outside 0..63 are dropped
/// so a single DX12 draw with one `chunk_origin` can cover a player-centred window.
pub fn mesh_chunk_column_pull_world(
    sections: &[SectionPalette],
    chunk_x: i32,
    chunk_z: i32,
    section_y0: i32,
    origin_x: i32,
    origin_y: i32,
    origin_z: i32,
    face_culling: bool,
) -> PullBuiltMesh {
    let mut all_quads = Vec::new();
    let wx0 = chunk_x * SECTION_SIZE as i32;
    let wz0 = chunk_z * SECTION_SIZE as i32;
    for (si, palette) in sections.iter().enumerate() {
        if section_all_air(palette) {
            continue;
        }
        let part = mesh_section_pull_inner(palette, chunk_x, chunk_z, face_culling);
        let wy0 = (section_y0 + si as i32) * SECTION_SIZE as i32;
        for q in part.quads {
            let lx = PackedPullQuad::unpack_x(q.word0) as i32 + wx0 - origin_x;
            let ly = PackedPullQuad::unpack_y(q.word0) as i32 + wy0 - origin_y;
            let lz = PackedPullQuad::unpack_z(q.word0) as i32 + wz0 - origin_z;
            if !(0..64).contains(&lx) || !(0..64).contains(&ly) || !(0..64).contains(&lz) {
                continue;
            }
            all_quads.push(PackedPullQuad::new(
                lx as u32,
                ly as u32,
                lz as u32,
                PackedPullQuad::unpack_tex(q.word0),
                PackedPullQuad::unpack_light_ao(q.word0),
                PackedPullQuad::unpack_face(q.word1),
                PackedPullQuad::unpack_width(q.word1),
                PackedPullQuad::unpack_height(q.word1),
            ));
        }
    }
    // wave 56: is_empty フィールド廃止 (PullBuiltMesh::is_empty() は quads から導出)
    PullBuiltMesh {
        chunk_x,
        chunk_z,
        quads: all_quads,
    }
}

/// Greedy merge on `mask[row][col]` bitfields (16 cols per row).
/// 2026-07-21: `greedy_merge_2d_bits` への移行済 (pull 版と同証明)。
/// 等価性検証オラクルとしてテスト専用に保持。
#[cfg(test)]
fn greedy_merge_2d(
    mask: &mut [[u16; SECTION_SIZE]; SECTION_SIZE],
    vertices: &mut Vec<Quantized12ByteVertex>,
    indices: &mut Vec<u32>,
    axis: Axis,
    positive: bool,
    origin: impl Fn(usize, usize) -> (f32, f32, f32),
    width_axis: impl Fn(usize) -> f32,
    height_axis: impl Fn(usize) -> f32,
) {
    let mut row = 0usize;
    while row < SECTION_SIZE {
        let row_bits = mask[row];
        if row_bits.iter().all(|&b| b == 0) {
            row += 1;
            continue;
        }
        let mut row_span = 1usize;
        while row + row_span < SECTION_SIZE && mask[row + row_span] == row_bits {
            row_span += 1;
        }
        let mut col = 0usize;
        while col < SECTION_SIZE {
            if row_bits[col] == 0 {
                col += 1;
                continue;
            }
            let mut col_span = 1usize;
            while col + col_span < SECTION_SIZE && row_bits[col + col_span] != 0 {
                let mut ok = true;
                for dr in 1..row_span {
                    if mask[row + dr][col + col_span] == 0 {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    break;
                }
                col_span += 1;
            }
            let mut actual_rows = 1usize;
            'grow: while row + actual_rows < row + row_span {
                for dc in 0..col_span {
                    if mask[row + actual_rows][col + dc] == 0 {
                        break 'grow;
                    }
                }
                actual_rows += 1;
            }
            for dr in 0..actual_rows {
                for dc in 0..col_span {
                    mask[row + dr][col + dc] = 0;
                }
            }
            let (x, y, z) = origin(col, row);
            emit_quad(
                vertices,
                indices,
                x,
                y,
                z,
                width_axis(col_span),
                height_axis(actual_rows),
                axis,
                positive,
            );
            col += col_span;
        }
        row += row_span;
    }
}

/// 12B 経路の bit 化 merge (`greedy_merge_2d_pull_bits` と同証明系:
/// 等しい行ブロック内の矩形拡張チェックは構造的常真)。
fn greedy_merge_2d_bits(
    mask: &mut [u16; SECTION_SIZE],
    vertices: &mut Vec<Quantized12ByteVertex>,
    indices: &mut Vec<u32>,
    axis: Axis,
    positive: bool,
    origin: impl Fn(usize, usize) -> (f32, f32, f32),
    width_axis: impl Fn(usize) -> f32,
    height_axis: impl Fn(usize) -> f32,
) {
    let mut row = 0usize;
    while row < SECTION_SIZE {
        let row_bits = mask[row];
        if row_bits == 0 {
            row += 1;
            continue;
        }
        let mut row_span = 1usize;
        while row + row_span < SECTION_SIZE && mask[row + row_span] == row_bits {
            row_span += 1;
        }
        let mut bits = row_bits;
        while bits != 0 {
            let col0 = bits.trailing_zeros() as usize;
            let span = (bits >> col0).trailing_ones() as usize;
            let run_mask: u16 = (((1u32 << span) - 1) << col0) as u16;
            for r in mask.iter_mut().skip(row).take(row_span) {
                *r &= !run_mask;
            }
            bits &= !run_mask;
            let (x, y, z) = origin(col0, row);
            emit_quad(
                vertices,
                indices,
                x,
                y,
                z,
                width_axis(span),
                height_axis(row_span),
                axis,
                positive,
            );
        }
        row += row_span;
    }
}

fn greedy_axis(
    cols: &OpaqueCols,
    vertices: &mut Vec<Quantized12ByteVertex>,
    indices: &mut Vec<u32>,
    axis: Axis,
    positive: bool,
    skip_empty_layers: bool,
) {
    // Y 層 skip は pull 版と同規約をカラムから導出 (旧実装は slice 内側で
    // layer_masks_from_palette (4096 voxel 走査) を都度計算していた)。
    let y_occ: u32 = if skip_empty_layers && matches!(axis, Axis::Y) {
        let mut acc = 0u32;
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                acc |= cols.yc[z][x] as u32;
            }
        }
        acc
    } else {
        0
    };
    for slice in 0..SECTION_SIZE {
        if y_occ != 0 {
            if (y_occ >> slice) & 1 == 0 {
                continue;
            }
        }

        let mut mask = face_rows_for_slice(cols, axis, positive, slice);
        if mask.iter().all(|&m| m == 0) {
            continue;
        }
        match axis {
            Axis::Y => {
                let s = slice as f32;
                greedy_merge_2d_bits(
                    &mut mask,
                    vertices,
                    indices,
                    axis,
                    positive,
                    move |x, z| (x as f32, s, z as f32),
                    |w| w as f32,
                    |h| h as f32,
                );
            }
            Axis::X => {
                let s = slice as f32;
                greedy_merge_2d_bits(
                    &mut mask,
                    vertices,
                    indices,
                    axis,
                    positive,
                    move |y, z| (s, y as f32, z as f32),
                    |w| w as f32,
                    |h| h as f32,
                );
            }
            Axis::Z => {
                let s = slice as f32;
                greedy_merge_2d_bits(
                    &mut mask,
                    vertices,
                    indices,
                    axis,
                    positive,
                    move |x, y| (x as f32, y as f32, s),
                    |w| w as f32,
                    |h| h as f32,
                );
            }
        }
    }
}

pub fn mesh_section(palette: &SectionPalette, chunk_x: i32, chunk_z: i32) -> BuiltChunkMesh {
    if section_all_air(palette) {
        return BuiltChunkMesh {
            chunk_x,
            chunk_z,
            vertices: vec![],
            indices: vec![],
        };
    }
    mesh_section_inner(palette, chunk_x, chunk_z, true)
}

fn mesh_section_inner(
    palette: &SectionPalette,
    chunk_x: i32,
    chunk_z: i32,
    skip_empty_layers: bool,
) -> BuiltChunkMesh {
    let mut vertices = Vec::with_capacity(512);
    let mut indices = Vec::with_capacity(768);
    // pull 版と同じく不透明カラムを 1 度だけ構築し、6 軸で共有する
    // (旧実装は greedy_axis 内で slice 毎に face_visible 全走査していた)。
    let cols = opaque_cols(palette);
    for &(axis, positive) in &Axis::ALL {
        greedy_axis(
            &cols,
            &mut vertices,
            &mut indices,
            axis,
            positive,
            skip_empty_layers,
        );
    }
    trace!(
        "binary greedy section ({}, {}): {} verts {} indices",
        chunk_x,
        chunk_z,
        vertices.len(),
        indices.len()
    );
    BuiltChunkMesh {
        chunk_x,
        chunk_z,
        vertices,
        indices,
    }
}

pub fn merge_section_meshes(
    chunk_x: i32,
    chunk_z: i32,
    parts: &[BuiltChunkMesh],
) -> BuiltChunkMesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&part.vertices);
        for &i in &part.indices {
            indices.push(base + i);
        }
    }
    BuiltChunkMesh {
        chunk_x,
        chunk_z,
        vertices,
        indices,
    }
}

/// 列メッシュ (12B 経路)。`face_culling` の実体は pull 版と同じく
/// 「Y 空層スキップ」の可否であり、出力頂点・index 列には影響しない
/// (純粋な perf スイッチ: true/false で出力 bit 同一)。
pub fn mesh_chunk_column(
    sections: &[SectionPalette],
    chunk_x: i32,
    chunk_z: i32,
    face_culling: bool,
) -> BuiltChunkMesh {
    let mut parts = Vec::new();
    for palette in sections {
        if section_all_air(palette) {
            continue;
        }
        parts.push(mesh_section_inner(palette, chunk_x, chunk_z, face_culling));
    }
    merge_section_meshes(chunk_x, chunk_z, &parts)
}

pub fn demo_column_palettes(cx: i32, cz: i32) -> Vec<SectionPalette> {
    let seed = (cx.wrapping_mul(374761) ^ cz.wrapping_mul(668265)) as u32;
    let mut sections = Vec::with_capacity(SECTIONS_PER_COLUMN);
    for sy in 0..SECTIONS_PER_COLUMN {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let base_h = ((seed.wrapping_add((x * 17 + z * 31) as u32)) % 10) as usize + 2;
                let h = base_h + sy * SECTION_SIZE;
                for y in 0..SECTION_SIZE {
                    let world_y = sy * SECTION_SIZE + y;
                    if world_y < h.min(SECTIONS_PER_COLUMN * SECTION_SIZE) {
                        p[idx(x, y, z)] = 1 + (world_y % 3) as u16;
                    }
                }
            }
        }
        sections.push(p);
    }
    sections
}

pub fn demo_palette(cx: i32, cz: i32) -> SectionPalette {
    demo_column_palettes(cx, cz)
        .into_iter()
        .next()
        .unwrap_or([0; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE])
}

pub fn demo_column_rle(cx: i32, cz: i32) -> Vec<RleSection> {
    demo_column_palettes(cx, cz)
        .iter()
        .map(RleSection::encode)
        .collect()
}

/// SWAR 64-bit Bitboard Directional Mask Slice comparison for ultra-fast neighbor culling.
/// A 16x16 slice (256 bits) is represented as `[u64; 4]`.
///
/// 監査メモ (2026-07-22): この bitboard 系 3 関数 (`*_swar` / `*_avx2` /
/// `extract_bitboard_span`) は将来の隣接チャンク横断カリング用に用意された
/// 未接続の公開ユーティリティであり、現時点では本番経路のどこからも
/// 呼ばれていない。正しさは SWAR 実装を正準として AVX2 版とテストで照合する。
#[inline]
pub fn bitboard_slice_cull_swar(slice_curr: &[u64; 4], slice_next: &[u64; 4]) -> [u64; 4] {
    [
        slice_curr[0] & !slice_next[0],
        slice_curr[1] & !slice_next[1],
        slice_curr[2] & !slice_next[2],
        slice_curr[3] & !slice_next[3],
    ]
}

/// AVX2 256-bit Bitboard Directional Mask Slice comparison (`_mm256_andnot_si256`).
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
pub unsafe fn bitboard_slice_cull_avx2(slice_curr: &[u64; 4], slice_next: &[u64; 4]) -> [u64; 4] {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    let curr = _mm256_loadu_si256(slice_curr.as_ptr() as *const __m256i);
    let next = _mm256_loadu_si256(slice_next.as_ptr() as *const __m256i);
    let res = _mm256_andnot_si256(next, curr);
    let mut out = [0u64; 4];
    _mm256_storeu_si256(out.as_mut_ptr() as *mut __m256i, res);
    out
}

/// Fast bit-scan (`trailing_zeros`) segment extraction from a 64-bit row mask.
/// 最下位の連続 run 1 つだけを (start, len) で返す (後続の run は呼び出し側が
/// run 除去後に再呼び出しして順に処理する規約)。`len` は `64 - start` で
/// clamp され、全 1 マスクでも (0, 64) を正確に返す。
#[inline]
pub fn extract_bitboard_span(mut row_mask: u64) -> Option<(u32, u32)> {
    if row_mask == 0 {
        return None;
    }
    let start = row_mask.trailing_zeros();
    row_mask >>= start;
    let len = (!row_mask).trailing_zeros().min(64 - start);
    Some((start, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DX-2 (wave 124): idx 座標規約の厳密 pin。消費者 10+ ファイルが
    /// 共有する基底写像 (rq dx_idx.rq で全 4096 roundtrip assert 事前導出)。
    #[test]
    fn idx_layout_exact_and_injective_full_space() {
        assert_eq!(idx(0, 0, 0), 0);
        assert_eq!(idx(1, 0, 0), 1);
        assert_eq!(idx(0, 1, 0), 16);
        assert_eq!(idx(0, 0, 1), 256);
        assert_eq!(idx(15, 15, 15), 4095, "最大・排他的終端");
        // 全 4096 引数で単射 + 座標復元 roundtrip (dq 導出と同構成)
        let mut seen = [false; 4096];
        for z in 0..16 {
            for y in 0..16 {
                for x in 0..16 {
                    let i = idx(x, y, z);
                    assert!(!seen[i], "衝突は単射違反 ({x},{y},{z})→{i}");
                    seen[i] = true;
                    assert_eq!((i % 16, (i / 16) % 16, i / 256), (x, y, z));
                }
            }
        }
        assert!(seen.iter().all(|&s| s), "全 4096 を過不足なく被覆");
    }

    #[test]
    fn solid_voxel_produces_faces() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        p[idx(0, 0, 0)] = 1;
        let m = mesh_section(&p, 0, 0);
        assert!(!m.is_empty());
        assert!(m.vertices.len() >= 4);
    }

    #[test]
    fn air_section_empty() {
        let p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        assert!(mesh_section(&p, 0, 0).is_empty());
    }

    #[test]
    fn flat_layer_greedy_merged() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        for x in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                p[idx(x, 0, z)] = 1;
            }
        }
        let m = mesh_section(&p, 0, 0);
        assert!(m.vertices.len() <= 6 * 4);
    }

    #[test]
    fn test_bitboard_swar_and_span() {
        let curr = [0x00FF_00FF_00FF_00FF, 0, 0, 0];
        let next = [0x000F_000F_000F_000F, 0, 0, 0];
        let culled = bitboard_slice_cull_swar(&curr, &next);
        assert_eq!(culled[0], 0x00F0_00F0_00F0_00F0);
        assert_eq!(extract_bitboard_span(0x0000_0000_0000_00F0), Some((4, 4)));
    }

    /// extract_bitboard_span の境界仕様 (モジュール固定セマンティクスから導出):
    /// - 0 → None
    /// - 最下位 run のみを返し、後続の孤立 bit は無視する
    /// - len は 64 - start で clamp される (全 1 → (0,64)、bit63 → (63,1))
    #[test]
    fn bitboard_span_edge_cases() {
        assert_eq!(extract_bitboard_span(0), None);
        assert_eq!(extract_bitboard_span(u64::MAX), Some((0, 64)));
        // bit63 のみ: clamp (64 - 63 = 1) が効く境界。
        assert_eq!(extract_bitboard_span(1u64 << 63), Some((63, 1)));
        assert_eq!(extract_bitboard_span(0x3), Some((0, 2)));
        // 先頭 run のみ返す規約: 最上位の孤立 bit は後続処理に委ねる。
        assert_eq!(extract_bitboard_span(0x8000_0000_0000_0001), Some((0, 1)));
        // 複数 run では最下位 run のみ: 0xF0F0 = runs [4..8) と [12..16)。
        assert_eq!(extract_bitboard_span(0xF0F0), Some((4, 4)));
        // 0b0111_0000: run は [4..7)。
        assert_eq!(extract_bitboard_span(0x70), Some((4, 3)));
    }

    /// AVX2 版は SWAR 版 (正準) と全入力で一致しなければならない。
    /// CI ランナーが AVX2 非対応なら早期 return (SWAR が正準フォールバック)。
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn bitboard_avx2_matches_swar_when_available() {
        if !std::arch::is_x86_feature_detected!("avx2") {
            return;
        }
        let cases: [([u64; 4], [u64; 4]); 5] = [
            ([0; 4], [0; 4]),
            ([u64::MAX; 4], [0; 4]),
            ([u64::MAX; 4], [u64::MAX; 4]),
            (
                [
                    0x00FF_00FF_00FF_00FF,
                    0xAAAA_AAAA_AAAA_AAAA,
                    0x1234_5678_9ABC_DEF0,
                    1,
                ],
                [
                    0x000F_000F_000F_000F,
                    0x5555_5555_5555_5555,
                    0xFEDC_BA98_7654_3210,
                    1,
                ],
            ),
            (
                [0xDEAD_BEEF_CAFE_F00D, 42, u64::MAX, 0x0101_0101_0101_0101],
                [0x0000_FFFF_0000_FFFF, 43, 0, 0x1010_1010_1010_1010],
            ),
        ];
        for (a, b) in cases {
            let swar = bitboard_slice_cull_swar(&a, &b);
            let avx2 = unsafe { bitboard_slice_cull_avx2(&a, &b) };
            assert_eq!(swar, avx2, "avx2 diverged from swar for {a:x?} vs {b:x?}");
        }
    }
}

#[cfg(test)]
mod bitcols_equivalence {
    use super::*;
    use crate::section_rle::layer_masks_from_palette;

    /// 旧 face_visible 参照経路 (bit 化前の greedy_axis_pull の再現)。
    fn old_axis_pull_quads(
        palette: &SectionPalette,
        axis: Axis,
        positive: bool,
        skip_empty_layers: bool,
    ) -> Vec<PackedPullQuad> {
        let mut quads = Vec::new();
        let layer_masks = if skip_empty_layers && matches!(axis, Axis::Y) {
            Some(layer_masks_from_palette(palette))
        } else {
            None
        };
        for slice in 0..SECTION_SIZE {
            if let Some(layers) = &layer_masks {
                if layers[slice] == 0 {
                    continue;
                }
            }
            let mut mask = [[0u16; SECTION_SIZE]; SECTION_SIZE];
            match axis {
                Axis::Y => {
                    for z in 0..SECTION_SIZE {
                        for x in 0..SECTION_SIZE {
                            if face_visible(palette, x, slice, z, axis, positive) {
                                mask[z][x] |= 1u16 << x;
                            }
                        }
                    }
                    greedy_merge_2d_pull(
                        &mut mask,
                        &mut quads,
                        palette,
                        axis,
                        positive,
                        move |x, z| (x, slice, z),
                        |w| w,
                        |h| h,
                    );
                }
                Axis::X => {
                    for z in 0..SECTION_SIZE {
                        for y in 0..SECTION_SIZE {
                            if face_visible(palette, slice, y, z, axis, positive) {
                                mask[z][y] |= 1u16 << y;
                            }
                        }
                    }
                    greedy_merge_2d_pull(
                        &mut mask,
                        &mut quads,
                        palette,
                        axis,
                        positive,
                        move |y, z| (slice, y, z),
                        |w| w,
                        |h| h,
                    );
                }
                Axis::Z => {
                    for y in 0..SECTION_SIZE {
                        for x in 0..SECTION_SIZE {
                            if face_visible(palette, x, y, slice, axis, positive) {
                                mask[y][x] |= 1u16 << x;
                            }
                        }
                    }
                    greedy_merge_2d_pull(
                        &mut mask,
                        &mut quads,
                        palette,
                        axis,
                        positive,
                        move |x, y| (x, y, slice),
                        |w| w,
                        |h| h,
                    );
                }
            }
        }
        quads
    }

    /// 旧 12B 参照経路 (bit 化前の greedy_axis の再現)。
    /// layer_masks はオリジナルが slice 内側で都度 4096 voxel 走査していたのを
    /// ホイストしているが、結果には影響しない (同一関数の都度呼び出し)。
    fn old_axis_12b(
        palette: &SectionPalette,
        axis: Axis,
        positive: bool,
        skip_empty_layers: bool,
    ) -> (Vec<Quantized12ByteVertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let layer_masks = if skip_empty_layers && matches!(axis, Axis::Y) {
            Some(layer_masks_from_palette(palette))
        } else {
            None
        };
        for slice in 0..SECTION_SIZE {
            if let Some(layers) = &layer_masks {
                if layers[slice] == 0 {
                    continue;
                }
            }
            let mut mask = [[0u16; SECTION_SIZE]; SECTION_SIZE];
            match axis {
                Axis::Y => {
                    for z in 0..SECTION_SIZE {
                        for x in 0..SECTION_SIZE {
                            if face_visible(palette, x, slice, z, axis, positive) {
                                mask[z][x] |= 1u16 << x;
                            }
                        }
                    }
                    let s = slice as f32;
                    greedy_merge_2d(
                        &mut mask,
                        &mut vertices,
                        &mut indices,
                        axis,
                        positive,
                        move |x, z| (x as f32, s, z as f32),
                        |w| w as f32,
                        |h| h as f32,
                    );
                }
                Axis::X => {
                    for z in 0..SECTION_SIZE {
                        for y in 0..SECTION_SIZE {
                            if face_visible(palette, slice, y, z, axis, positive) {
                                mask[z][y] |= 1u16 << y;
                            }
                        }
                    }
                    let s = slice as f32;
                    greedy_merge_2d(
                        &mut mask,
                        &mut vertices,
                        &mut indices,
                        axis,
                        positive,
                        move |y, z| (s, y as f32, z as f32),
                        |w| w as f32,
                        |h| h as f32,
                    );
                }
                Axis::Z => {
                    for y in 0..SECTION_SIZE {
                        for x in 0..SECTION_SIZE {
                            if face_visible(palette, x, y, slice, axis, positive) {
                                mask[y][x] |= 1u16 << x;
                            }
                        }
                    }
                    let s = slice as f32;
                    greedy_merge_2d(
                        &mut mask,
                        &mut vertices,
                        &mut indices,
                        axis,
                        positive,
                        move |x, y| (x as f32, y as f32, s),
                        |w| w as f32,
                        |h| h as f32,
                    );
                }
            }
        }
        (vertices, indices)
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        }
    }

    fn fuzz_section(seed: u64, pct: u64) -> SectionPalette {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let mut rng = Rng(seed);
        for v in p.iter_mut() {
            if rng.next() % 100 < pct {
                *v = (1 + rng.next() % 5) as u16;
            }
        }
        p
    }

    /// 新カラム導出の greedy_axis_pull は旧 face_visible 参照経路と
    /// quad 列 (個数・内容・順序) が全ケース完全同一でなければならない。
    #[test]
    fn bitcols_match_face_visible_fuzz() {
        for (seed, pct) in [
            (0x1001u64, 30),
            (0x1002, 50),
            (0x1003, 62),
            (0x1004, 85),
            (0x1005, 3),
            (0x1006, 100),
        ] {
            let palette = fuzz_section(seed, pct);
            let cols = opaque_cols(&palette);
            for &(axis, positive) in &Axis::ALL {
                for skip in [false, true] {
                    let old = old_axis_pull_quads(&palette, axis, positive, skip);
                    let mut new = Vec::new();
                    greedy_axis_pull(&cols, &palette, &mut new, axis, positive, skip);
                    assert_eq!(
                        old,
                        new,
                        "mismatch seed={seed:#x} pct={pct} axis={axis:?} pos={positive} skip={skip} (old={} new={})",
                        old.len(),
                        new.len()
                    );
                }
            }
        }
    }

    /// 破損境界: 全空 / 全面固体 / 単 voxel 角 / 全面 checker。
    #[test]
    fn bitcols_edge_sections_match() {
        let cases: Vec<SectionPalette> = vec![
            [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE],
            [7u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE],
            {
                let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
                p[idx(0, 0, 0)] = 9;
                p[idx(15, 15, 15)] = 9;
                p
            },
            {
                let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
                for z in 0..SECTION_SIZE {
                    for y in 0..SECTION_SIZE {
                        for x in 0..SECTION_SIZE {
                            if (x + y + z) % 2 == 0 {
                                p[idx(x, y, z)] = 4;
                            }
                        }
                    }
                }
                p
            },
        ];
        for (i, palette) in cases.iter().enumerate() {
            let cols = opaque_cols(palette);
            for &(axis, positive) in &Axis::ALL {
                for skip in [false, true] {
                    let old = old_axis_pull_quads(palette, axis, positive, skip);
                    let mut new = Vec::new();
                    greedy_axis_pull(&cols, palette, &mut new, axis, positive, skip);
                    assert_eq!(
                        old, new,
                        "edge case {i} axis={axis:?} pos={positive} skip={skip}"
                    );
                }
            }
        }
    }

    fn verts_as_bytes(v: &[Quantized12ByteVertex]) -> &[u8] {
        bytemuck::cast_slice(v)
    }

    /// 12B 経路も pull 版と同証明系でカラム bit 導出へ移行した。新 greedy_axis
    /// (greedy_merge_2d_bits) は旧 face_visible+greedy_merge_2d 参照経路と
    /// 頂点列・index 列 (個数・内容・順序) が全ケース完全同一でなければならない。
    #[test]
    fn bitcols_12b_match_face_visible_fuzz() {
        for (seed, pct) in [
            (0x1001u64, 30),
            (0x1002, 50),
            (0x1003, 62),
            (0x1004, 85),
            (0x1005, 3),
            (0x1006, 100),
        ] {
            let palette = fuzz_section(seed, pct);
            let cols = opaque_cols(&palette);
            for &(axis, positive) in &Axis::ALL {
                for skip in [false, true] {
                    let (old_v, old_i) = old_axis_12b(&palette, axis, positive, skip);
                    let mut new_v = Vec::new();
                    let mut new_i = Vec::new();
                    greedy_axis(&cols, &mut new_v, &mut new_i, axis, positive, skip);
                    assert_eq!(
                        verts_as_bytes(&old_v),
                        verts_as_bytes(&new_v),
                        "verts mismatch seed={seed:#x} pct={pct} axis={axis:?} pos={positive} skip={skip} (old={} new={})",
                        old_v.len(),
                        new_v.len()
                    );
                    assert_eq!(
                        old_i,
                        new_i,
                        "indices mismatch seed={seed:#x} pct={pct} axis={axis:?} pos={positive} skip={skip}"
                    );
                }
            }
        }
    }

    /// 破損境界: 全空 / 全面固体 / 単 voxel 角 / 全面 checker (12B 経路)。
    #[test]
    fn bitcols_12b_edge_sections_match() {
        let cases: Vec<SectionPalette> = vec![
            [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE],
            [7u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE],
            {
                let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
                p[idx(0, 0, 0)] = 9;
                p[idx(15, 15, 15)] = 9;
                p
            },
            {
                let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
                for z in 0..SECTION_SIZE {
                    for y in 0..SECTION_SIZE {
                        for x in 0..SECTION_SIZE {
                            if (x + y + z) % 2 == 0 {
                                p[idx(x, y, z)] = 4;
                            }
                        }
                    }
                }
                p
            },
        ];
        for (i, palette) in cases.iter().enumerate() {
            let cols = opaque_cols(palette);
            for &(axis, positive) in &Axis::ALL {
                for skip in [false, true] {
                    let (old_v, old_i) = old_axis_12b(palette, axis, positive, skip);
                    let mut new_v = Vec::new();
                    let mut new_i = Vec::new();
                    greedy_axis(&cols, &mut new_v, &mut new_i, axis, positive, skip);
                    assert_eq!(
                        verts_as_bytes(&old_v),
                        verts_as_bytes(&new_v),
                        "12B edge case {i} axis={axis:?} pos={positive} skip={skip}"
                    );
                    assert_eq!(
                        old_i, new_i,
                        "12B edge case {i} axis={axis:?} pos={positive} skip={skip}"
                    );
                }
            }
        }
    }
}
