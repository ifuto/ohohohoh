//! Live Minecraft column store — ClientLevel sections → compressed SectionPalette for meshing.
//!
//! Hot columns near the camera keep RLE/palette-friendly CompactSection;
//! columns pruned toward the edge of the keep-radius are re-encoded as LZ4 cold form.

use crate::binary_greedy_meshing::{SectionPalette, SECTION_SIZE, SECTIONS_PER_COLUMN};
use crate::hzb_2d::CameraState;
use crate::section_compress::CompactSection;
use std::collections::HashMap;
use tracing::debug;

/// One loaded chunk column from the game (full height sections, compressed).
#[derive(Debug, Clone)]
pub struct StoredColumn {
    pub base_section_y: i32,
    pub sections: Vec<CompactSection>,
    pub generation: u64,
    pub cold: bool,
}

/// Pull-mesh packing uses 6-bit coords (0..63). World window is 64³ blocks.
pub const PULL_WINDOW_BLOCKS: i32 = 64;

#[derive(Debug, Default)]
pub struct WorldColumnStore {
    columns: HashMap<(i32, i32), StoredColumn>,
    pub camera: CameraState,
    pub has_live_data: bool,
    generation: u64,
    /// World-block origin of the current pull window (min corner).
    pub mesh_origin: [i32; 3],
    /// Cumulative uncompressed bytes if we stored dense u16[4096] per section.
    pub dense_bytes_estimate: u64,
    /// Actual stored bytes after compression.
    pub compressed_bytes: u64,
}

impl WorldColumnStore {
    pub fn new() -> Self {
        Self {
            columns: HashMap::new(),
            camera: CameraState {
                fov_y: 70.0_f32.to_radians(),
                aspect: 16.0 / 9.0,
                ..Default::default()
            },
            has_live_data: false,
            generation: 0,
            mesh_origin: [0, 0, 0],
            dense_bytes_estimate: 0,
            compressed_bytes: 0,
        }
    }

    pub fn set_camera(&mut self, x: f32, y: f32, z: f32, yaw_deg: f32, pitch_deg: f32) {
        self.camera.x = x;
        self.camera.y = y;
        self.camera.z = z;
        self.camera.yaw = yaw_deg.to_radians();
        self.camera.pitch = pitch_deg.to_radians();
        self.recompute_mesh_origin();
    }

    pub fn recompute_mesh_origin(&mut self) {
        let cx = (self.camera.x.floor() as i32).div_euclid(SECTION_SIZE as i32);
        let cy = (self.camera.y.floor() as i32).div_euclid(SECTION_SIZE as i32);
        let cz = (self.camera.z.floor() as i32).div_euclid(SECTION_SIZE as i32);
        self.mesh_origin = [
            (cx - 1) * SECTION_SIZE as i32,
            (cy - 1) * SECTION_SIZE as i32,
            (cz - 1) * SECTION_SIZE as i32,
        ];
    }

    pub fn camera_chunk(&self) -> (i32, i32) {
        (
            (self.camera.x.floor() as i32).div_euclid(SECTION_SIZE as i32),
            (self.camera.z.floor() as i32).div_euclid(SECTION_SIZE as i32),
        )
    }

    pub fn camera_section_y(&self) -> i32 {
        (self.camera.y.floor() as i32).div_euclid(SECTION_SIZE as i32)
    }

    fn recount_bytes(&mut self) {
        let mut dense = 0u64;
        let mut stored = 0u64;
        for col in self.columns.values() {
            dense += (col.sections.len() * SECTION_SIZE * SECTION_SIZE * SECTION_SIZE * 2) as u64;
            stored += col.sections.iter().map(|s| s.stored_bytes() as u64).sum::<u64>();
        }
        self.dense_bytes_estimate = dense;
        self.compressed_bytes = stored;
    }

    /// Flat layout: `section_count * 4096` block ids (0 = air).
    pub fn ingest(
        &mut self,
        cx: i32,
        cz: i32,
        base_section_y: i32,
        flat: &[u16],
        section_count: usize,
    ) {
        if section_count == 0 {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let mut sections = Vec::with_capacity(section_count);
        for s in 0..section_count {
            let mut palette = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
            let start = s * palette.len();
            let end = start + palette.len();
            if end <= flat.len() {
                palette.copy_from_slice(&flat[start..end]);
            }
            sections.push(CompactSection::encode_hot(&palette));
        }
        self.columns.insert(
            (cx, cz),
            StoredColumn {
                base_section_y,
                sections,
                generation: self.generation,
                cold: false,
            },
        );
        if !self.has_live_data {
            debug!(
                "[WorldColumnStore] first live column ({}, {}) sections={}",
                cx, cz, section_count
            );
        }
        self.has_live_data = true;
        self.recount_bytes();
    }

    /// Re-encode columns outside `hot_radius` as LZ4 cold form to cut RAM.
    pub fn compress_distant(&mut self, center_cx: i32, center_cz: i32, hot_radius: i32) {
        for ((cx, cz), col) in self.columns.iter_mut() {
            let far = (cx - center_cx).abs() > hot_radius || (cz - center_cz).abs() > hot_radius;
            if far && !col.cold {
                col.sections = col.sections.iter().map(|s| s.to_cold()).collect();
                col.cold = true;
            } else if !far && col.cold {
                // Warm back to RLE for meshing locality.
                col.sections = col
                    .sections
                    .iter()
                    .map(|s| CompactSection::encode_hot(&s.decode()))
                    .collect();
                col.cold = false;
            }
        }
        self.recount_bytes();
    }

    pub fn prune_outside(&mut self, center_cx: i32, center_cz: i32, radius: i32) {
        // Compress mid-ring before hard prune.
        let hot = (radius / 2).max(1);
        self.compress_distant(center_cx, center_cz, hot);
        self.columns.retain(|(cx, cz), _| {
            (cx - center_cx).abs() <= radius && (cz - center_cz).abs() <= radius
        });
        if self.columns.is_empty() {
            self.has_live_data = false;
        }
        self.recount_bytes();
    }

    /// Returns up to [`SECTIONS_PER_COLUMN`] sections for meshing, plus world section Y of `out[0]`.
    pub fn column_for_mesh(&self, cx: i32, cz: i32) -> Option<(Vec<SectionPalette>, i32)> {
        let col = self.columns.get(&(cx, cz))?;
        let want_base = self.camera_section_y() - 1;
        let mut out = vec![[0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE]; SECTIONS_PER_COLUMN];
        let mut any = false;
        for i in 0..SECTIONS_PER_COLUMN {
            let world_sy = want_base + i as i32;
            let idx = world_sy - col.base_section_y;
            if idx >= 0 && (idx as usize) < col.sections.len() {
                out[i] = col.sections[idx as usize].decode();
                any = true;
            }
        }
        if !any {
            return None;
        }
        Some((out, want_base))
    }

    pub fn column_generation(&self, cx: i32, cz: i32) -> Option<u64> {
        self.columns.get(&(cx, cz)).map(|c| c.generation)
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn compression_ratio(&self) -> f32 {
        if self.dense_bytes_estimate == 0 {
            return 1.0;
        }
        self.compressed_bytes as f32 / self.dense_bytes_estimate as f32
    }
}

/// Row-major view-projection + chunk origin for DX12 `FrameCB` (20 × f32).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TerrainFrameConstants {
    pub view_proj: [[f32; 4]; 4],
    pub chunk_origin: [f32; 4],
}

impl TerrainFrameConstants {
    pub fn from_camera(cam: &CameraState, origin: [i32; 3]) -> Self {
        let eye = [cam.x, cam.y, cam.z];
        let (sin_y, cos_y) = cam.yaw.sin_cos();
        let (sin_p, cos_p) = cam.pitch.sin_cos();
        let forward = [-sin_y * cos_p, -sin_p, cos_y * cos_p];
        let target = [eye[0] + forward[0], eye[1] + forward[1], eye[2] + forward[2]];
        let view = look_at_rh(eye, target, [0.0, 1.0, 0.0]);
        let proj = perspective_rh(cam.fov_y.max(0.1), cam.aspect.max(0.1), 0.05, 512.0);
        Self {
            view_proj: mul4(view, proj),
            chunk_origin: [origin[0] as f32, origin[1] as f32, origin[2] as f32, 0.0],
        }
    }

    pub fn as_f32_slice(&self) -> &[f32] {
        bytemuck::cast_slice(std::slice::from_ref(self))
    }
}

fn perspective_rh(fov_y: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fov_y * 0.5).tan();
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = f / aspect;
    m[1][1] = f;
    m[2][2] = far / (near - far);
    m[2][3] = -1.0;
    m[3][2] = (far * near) / (near - far);
    m
}

fn look_at_rh(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
    let f = normalize([
        target[0] - eye[0],
        target[1] - eye[1],
        target[2] - eye[2],
    ]);
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}

fn mul4(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j] + a[i][3] * b[3][j];
        }
    }
    out
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-8);
    [v[0] / len, v[1] / len, v[2] / len]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary_greedy_meshing::idx;

    #[test]
    fn ingest_and_window_around_camera() {
        let mut store = WorldColumnStore::new();
        store.set_camera(8.0, 72.0, 8.0, 0.0, 0.0);
        let mut flat = vec![0u16; 4096];
        flat[idx(1, 2, 3)] = 7;
        store.ingest(0, 0, 4, &flat, 1);
        assert!(store.has_live_data);
        assert!(store.compressed_bytes < store.dense_bytes_estimate || store.dense_bytes_estimate == 0);
        let (sections, base) = store.column_for_mesh(0, 0).expect("column");
        assert_eq!(base, store.camera_section_y() - 1);
        assert_eq!(sections[1][idx(1, 2, 3)], 7);
    }

    #[test]
    fn frame_constants_have_origin() {
        let mut store = WorldColumnStore::new();
        store.set_camera(24.0, 80.0, 40.0, 90.0, 0.0);
        let cb = TerrainFrameConstants::from_camera(&store.camera, store.mesh_origin);
        assert_eq!(cb.chunk_origin[0], store.mesh_origin[0] as f32);
        assert_eq!(cb.chunk_origin[1], store.mesh_origin[1] as f32);
        assert_eq!(cb.chunk_origin[2], store.mesh_origin[2] as f32);
    }
}
