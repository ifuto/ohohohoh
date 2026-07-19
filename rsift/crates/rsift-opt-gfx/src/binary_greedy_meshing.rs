//! Binary Greedy Meshing — tri-axis face culling + quad merging (cgerikj-style).
//! RLE-empty layers skipped; all 6 face directions use correct slice axes.

use crate::chunk_mesh::{BuiltChunkMesh, Quantized12ByteVertex};
use crate::packed4::{face_index as pull_face_index, PackedPullQuad};
use crate::pull_mesh::PullBuiltMesh;
use crate::section_rle::{layer_masks_from_palette, RleSection};
use tracing::trace;

pub const SECTION_SIZE: usize = 16;
pub const SECTIONS_PER_COLUMN: usize = 4;

pub type SectionPalette = [u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];

#[inline]
pub fn idx(x: usize, y: usize, z: usize) -> usize {
    x + y * SECTION_SIZE + z * SECTION_SIZE * SECTION_SIZE
}

#[inline]
fn is_opaque(palette: &SectionPalette, x: usize, y: usize, z: usize) -> bool {
    if x >= SECTION_SIZE || y >= SECTION_SIZE || z >= SECTION_SIZE {
        return false;
    }
    palette[idx(x, y, z)] != 0
}

#[derive(Clone, Copy)]
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

#[inline]
fn neighbor_opaque(palette: &SectionPalette, x: usize, y: usize, z: usize, axis: Axis, positive: bool) -> bool {
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

#[inline]
fn face_visible(palette: &SectionPalette, x: usize, y: usize, z: usize, axis: Axis, positive: bool) -> bool {
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
            vertices.push(Quantized12ByteVertex::encode(fx, y, z, nx, ny, nz, 0.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(fx, y + h, z, nx, ny, nz, 0.0, 1.0));
            vertices.push(Quantized12ByteVertex::encode(fx, y + h, z + w, nx, ny, nz, 1.0, 1.0));
            vertices.push(Quantized12ByteVertex::encode(fx, y, z + w, nx, ny, nz, 1.0, 0.0));
        }
        (Axis::X, false) => {
            vertices.push(Quantized12ByteVertex::encode(x, y, z, nx, ny, nz, 0.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(x, y, z + w, nx, ny, nz, 1.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(x, y + h, z + w, nx, ny, nz, 1.0, 1.0));
            vertices.push(Quantized12ByteVertex::encode(x, y + h, z, nx, ny, nz, 0.0, 1.0));
        }
        (Axis::Y, true) => {
            let fy = y + 1.0;
            vertices.push(Quantized12ByteVertex::encode(x, fy, z, nx, ny, nz, 0.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(x + w, fy, z, nx, ny, nz, 1.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(x + w, fy, z + h, nx, ny, nz, 1.0, 1.0));
            vertices.push(Quantized12ByteVertex::encode(x, fy, z + h, nx, ny, nz, 0.0, 1.0));
        }
        (Axis::Y, false) => {
            vertices.push(Quantized12ByteVertex::encode(x, y, z, nx, ny, nz, 0.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(x, y, z + h, nx, ny, nz, 0.0, 1.0));
            vertices.push(Quantized12ByteVertex::encode(x + w, y, z + h, nx, ny, nz, 1.0, 1.0));
            vertices.push(Quantized12ByteVertex::encode(x + w, y, z, nx, ny, nz, 1.0, 0.0));
        }
        (Axis::Z, true) => {
            let fz = z + 1.0;
            vertices.push(Quantized12ByteVertex::encode(x, y, fz, nx, ny, nz, 0.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(x + w, y, fz, nx, ny, nz, 1.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(x + w, y + h, fz, nx, ny, nz, 1.0, 1.0));
            vertices.push(Quantized12ByteVertex::encode(x, y + h, fz, nx, ny, nz, 0.0, 1.0));
        }
        (Axis::Z, false) => {
            vertices.push(Quantized12ByteVertex::encode(x, y, z, nx, ny, nz, 0.0, 0.0));
            vertices.push(Quantized12ByteVertex::encode(x, y + h, z, nx, ny, nz, 0.0, 1.0));
            vertices.push(Quantized12ByteVertex::encode(x + w, y + h, z, nx, ny, nz, 1.0, 1.0));
            vertices.push(Quantized12ByteVertex::encode(x + w, y, z, nx, ny, nz, 1.0, 0.0));
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
    let block = palette[idx(x.min(SECTION_SIZE - 1), y.min(SECTION_SIZE - 1), z.min(SECTION_SIZE - 1))];
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

fn greedy_axis_pull(
    palette: &SectionPalette,
    quads: &mut Vec<PackedPullQuad>,
    axis: Axis,
    positive: bool,
    skip_empty_layers: bool,
) {
    for slice in 0..SECTION_SIZE {
        if skip_empty_layers && matches!(axis, Axis::Y) {
            let layers = layer_masks_from_palette(palette);
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
                for z in 0..SECTION_SIZE {
                    for y in 0..SECTION_SIZE {
                        if face_visible(palette, slice, y, z, axis, positive) {
                            mask[z][y] |= 1u16 << y;
                        }
                    }
                }
                greedy_merge_2d_pull(
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
                for y in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        if face_visible(palette, x, y, slice, axis, positive) {
                            mask[y][x] |= 1u16 << x;
                        }
                    }
                }
                greedy_merge_2d_pull(
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
    for &(axis, positive) in &Axis::ALL {
        greedy_axis_pull(palette, &mut quads, axis, positive, skip_empty_layers);
    }
    trace!(
        "binary greedy pull section ({}, {}): {} quads {} pull_verts",
        chunk_x,
        chunk_z,
        quads.len(),
        quads.len() * 6
    );
    PullBuiltMesh {
        chunk_x,
        chunk_z,
        is_empty: quads.is_empty(),
        quads,
    }
}

/// Greedy mesh → SSBO quad list (8 B/quad, no index buffer).
pub fn mesh_section_pull(
    palette: &SectionPalette,
    chunk_x: i32,
    chunk_z: i32,
) -> PullBuiltMesh {
    if RleSection::encode(palette).is_empty() {
        return PullBuiltMesh::empty(chunk_x, chunk_z);
    }
    mesh_section_pull_inner(palette, chunk_x, chunk_z, true)
}

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
        if RleSection::encode(palette).is_empty() {
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
    PullBuiltMesh {
        chunk_x,
        chunk_z,
        is_empty: all_quads.is_empty(),
        quads: all_quads,
    }
}

/// Greedy merge on `mask[row][col]` bitfields (16 cols per row).
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

fn greedy_axis(
    palette: &SectionPalette,
    vertices: &mut Vec<Quantized12ByteVertex>,
    indices: &mut Vec<u32>,
    axis: Axis,
    positive: bool,
    skip_empty_layers: bool,
) {
    for slice in 0..SECTION_SIZE {
        if skip_empty_layers && matches!(axis, Axis::Y) {
            let layers = layer_masks_from_palette(palette);
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
    if RleSection::encode(palette).is_empty() {
        return BuiltChunkMesh {
            chunk_x,
            chunk_z,
            is_empty: true,
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
    for &(axis, positive) in &Axis::ALL {
        greedy_axis(
            palette,
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
        is_empty: vertices.is_empty(),
        vertices,
        indices,
    }
}

pub fn merge_section_meshes(chunk_x: i32, chunk_z: i32, parts: &[BuiltChunkMesh]) -> BuiltChunkMesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for part in parts {
        if part.is_empty {
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
        is_empty: vertices.is_empty(),
        vertices,
        indices,
    }
}

pub fn mesh_chunk_column(
    sections: &[SectionPalette],
    chunk_x: i32,
    chunk_z: i32,
    face_culling: bool,
) -> BuiltChunkMesh {
    let mut parts = Vec::new();
    for palette in sections {
        if RleSection::encode(palette).is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_voxel_produces_faces() {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        p[idx(0, 0, 0)] = 1;
        let m = mesh_section(&p, 0, 0);
        assert!(!m.is_empty);
        assert!(m.vertices.len() >= 4);
    }

    #[test]
    fn air_section_empty() {
        let p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        assert!(mesh_section(&p, 0, 0).is_empty);
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
}
