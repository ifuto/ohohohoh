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

    /// 非有限値 (NaN/±∞) は観測欠測として拒否する (旧カメラを維持)。
    /// FFI 入口 (C ABI / JNI) のため panic で落とすわけにはいかず drop とする:
    /// NaN as i32 は 0 で mesh_origin が静寂に原点附近化し、±∞ は
    /// `(cx-1) * SECTION_SIZE` の乗算で i32 overflow する。
    pub fn set_camera(&mut self, x: f32, y: f32, z: f32, yaw_deg: f32, pitch_deg: f32) {
        if !(x.is_finite()
            && y.is_finite()
            && z.is_finite()
            && yaw_deg.is_finite()
            && pitch_deg.is_finite())
        {
            debug!(
                "[WorldColumnStore] non-finite camera rejected: ({x}, {y}, {z}, {yaw_deg}, {pitch_deg})"
            );
            return;
        }
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

    fn stored_of(col: &StoredColumn) -> u64 {
        col.sections
            .iter()
            .map(|s| s.stored_bytes() as u64)
            .sum::<u64>()
    }

    fn dense_of(col: &StoredColumn) -> u64 {
        (col.sections.len() * SECTION_SIZE * SECTION_SIZE * SECTION_SIZE * 2) as u64
    }

    /// Recompute both byte counters from scratch (O(columns)).
    /// 通常は不要: ingest / compress_distant / prune_outside が差分で不変量
    /// (counters == 全再計算) を維持する。整合検証・将来の外部直接操作の
    /// 回復用の公開 API (reconciliation)。
    pub fn reconcile_byte_counters(&mut self) {
        let mut dense = 0u64;
        let mut stored = 0u64;
        for col in self.columns.values() {
            dense += Self::dense_of(col);
            stored += Self::stored_of(col);
        }
        self.dense_bytes_estimate = dense;
        self.compressed_bytes = stored;
    }

    /// Flat layout: `section_count * 4096` block ids (0 = air).
    ///
    /// 契約: 2 系統の実呼出 (JNI chunk_bridge / C ABI vtable) は共に
    /// `flat.len() >= section_count * 4096` を保証する (不足は上流で拒否)。
    /// 不足分は契約外入力への防御として欠測扱い — air (0) のまま残す
    /// (`if end <= flat.len()` 分岐)。`section_count == 0` は no-op
    /// (has_live_data / generation / counters ともに不変)。
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
        let new_col = StoredColumn {
            base_section_y,
            sections,
            generation: self.generation,
            cold: false,
        };
        let new_dense = Self::dense_of(&new_col);
        let new_stored = Self::stored_of(&new_col);
        // 差分会計: bulk ingest 時の全走査 recount (O(n²) 悪化) を回避する。
        // 上書き時は旧カラム分を引いてから新分を足す (u64 加減算は厳密で、
        // 不変量 counters == reconcile_byte_counters() の結果 を保持する)。
        if let Some(old) = self.columns.insert((cx, cz), new_col) {
            self.dense_bytes_estimate =
                self.dense_bytes_estimate - Self::dense_of(&old) + new_dense;
            self.compressed_bytes = self.compressed_bytes - Self::stored_of(&old) + new_stored;
        } else {
            self.dense_bytes_estimate += new_dense;
            self.compressed_bytes += new_stored;
        }
        if !self.has_live_data {
            debug!(
                "[WorldColumnStore] first live column ({}, {}) sections={}",
                cx, cz, section_count
            );
        }
        self.has_live_data = true;
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
        self.reconcile_byte_counters();
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
        self.reconcile_byte_counters();
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
                let sec = &col.sections[idx as usize];
                // 【wave 177 FW-1】air セクションは decode を skip (空気の
                // decode は全 0 で `out[i]` 初期値と逐語一致、数学的等価)。
                // 等価証明: CompactSection::is_air()=true ⟹ decode()≡[0;VOL]
                // — Single(0) は定義上全 0・Rle is_empty()=全 run block 0 で
                // fill 対象は全て 0 (空 runs は旧 decode 契約 panic、こちらは
                // 頑健に全 0)。any フラグ (範囲重複 truth) は不変。
                if !sec.is_air() {
                    out[i] = sec.decode();
                }
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

    #[test]
    fn non_finite_camera_is_rejected_and_previous_camera_kept() {
        // BW-1: NaN は 0 への飽和で mesh_origin が静寂に原点附近化し、±∞ は
        // (cx-1)*16 で i32 overflow する。FFI 入口なので panic せず drop し、
        // 直前の有限カメラを維持する契約を機械固定。
        let mut store = WorldColumnStore::new();
        store.set_camera(24.0, 80.0, 40.0, 90.0, 0.0);
        let keep = store.camera;
        let keep_origin = store.mesh_origin;
        store.set_camera(f32::NAN, 0.0, 0.0, 0.0, 0.0);
        assert_eq!(store.camera.x, keep.x, "NaN x は拒否され旧値が残る");
        assert_eq!(store.mesh_origin, keep_origin, "拒否時 origin 不変");
        store.set_camera(0.0, f32::INFINITY, 0.0, 0.0, 0.0);
        assert_eq!(store.camera.y, keep.y, "+∞ y も拒否");
        store.set_camera(0.0, 0.0, 0.0, f32::NEG_INFINITY, 0.0);
        assert_eq!(store.camera.yaw, keep.yaw, "-∞ yaw も拒否");
        // 拒否後も有限カメラは正常に適用される (固まらない)。
        store.set_camera(8.0, 72.0, 8.0, 0.0, 0.0);
        assert_eq!(store.mesh_origin, [-16, 48, -16]);
    }

    #[test]
    fn byte_counters_match_full_reconcile_as_invariant() {
        // BW-3: 差分会計 == 全再計算を複数経路 (新規/増設/縮退上書き/冷温往復)
        // で整数等値ピン。u64 加減算なので誤差ゼロでの一致が要求できる。
        let mut store = WorldColumnStore::new();
        let flat1 = vec![1u16; 4096];
        let flat2 = vec![2u16; 4096 * 2];
        store.ingest(0, 0, 0, &flat2, 2);
        store.ingest(1, 0, 0, &flat1, 1);
        store.ingest(0, 0, 0, &flat1, 1); // 2 sections → 1 section 縮退上書き
        let bytes_per_section = (SECTION_SIZE * SECTION_SIZE * SECTION_SIZE * 2) as u64;
        assert_eq!(
            store.dense_bytes_estimate,
            2 * bytes_per_section,
            "上書き後 dense は 2 カラム × 1 section の厳密値"
        );
        let (d0, c0) = (store.dense_bytes_estimate, store.compressed_bytes);
        store.reconcile_byte_counters();
        assert_eq!(store.dense_bytes_estimate, d0, "差分 == 全再計算 (dense)");
        assert_eq!(store.compressed_bytes, c0, "差分 == 全再計算 (stored)");
        // hot→cold→hot 往復後も不変量
        store.compress_distant(0, 0, 0); // (1,0) のみ far → cold 化
        store.compress_distant(0, 0, 8); // 全て近傍 → warm back
        let (d1, c1) = (store.dense_bytes_estimate, store.compressed_bytes);
        store.reconcile_byte_counters();
        assert_eq!(
            (store.dense_bytes_estimate, store.compressed_bytes),
            (d1, c1),
            "冷温往復後も差分 == 全再計算"
        );
    }

    #[test]
    fn column_for_mesh_no_overlap_returns_none() {
        // BW-4: 取得窓 (want_base .. want_base+SECTIONS_PER_COLUMN) と格納帯が
        // 交差しない場合は None — `any` フラグ経路の厳密固定 (air-only の
        // Some を返さないことが 4 頂点の meshing 入力契約)。
        let mut store = WorldColumnStore::new();
        store.set_camera(8.0, 72.0, 8.0, 0.0, 0.0); // cy=4 → want_base=3、窓 sy=3..7
        let flat = vec![1u16; 4096];
        store.ingest(0, 0, 10, &flat, 1); // 世界 sy=10 のみ → 窓外
        assert!(store.column_for_mesh(0, 0).is_none());
        // 下端ちょうど交差: base_section_y=6 (sy=6 は窓の最終 i=3 に入る)
        store.ingest(0, 0, 6, &flat, 1);
        assert!(store.column_for_mesh(0, 0).is_some(), "sy=6 は窓内 (3..7)");
        store.ingest(0, 0, 7, &flat, 1); // sy=7 → 窓外へ (境界は半開区間 [3,7))
        assert!(store.column_for_mesh(0, 0).is_none(), "sy=7 は窓外");
    }

    #[test]
    fn mesh_origin_tracks_camera_section_exact() {
        // BW-4: div_euclid の負側丸めを含むオリジン計算の厳密ピン。
        // (c-1)*16: カメラ所属 section の 1 つ手前を窓の min 隅とする定義。
        let mut store = WorldColumnStore::new();
        store.set_camera(8.0, 72.0, 8.0, 0.0, 0.0);
        assert_eq!(store.camera_chunk(), (0, 0));
        assert_eq!(store.camera_section_y(), 4, "72/16 = 4.5 → floor div で 4");
        assert_eq!(store.mesh_origin, [-16, 48, -16]);
        store.set_camera(-8.0, 72.0, -8.0, 0.0, 0.0);
        // div_euclid: -8 = 16*(-1) + 8 → section -1 (trunc 除算なら 0 になり誤る)
        assert_eq!(store.camera_chunk(), (-1, -1));
        assert_eq!(store.mesh_origin, [-32, 48, -32]);
        store.set_camera(-0.5, 15.9, 0.5, 0.0, 0.0);
        // -0.5 → section -1、15.9 → section 0
        assert_eq!(store.camera_section_y(), 0);
        assert_eq!(store.mesh_origin, [-32, -16, -16]);
    }

    #[test]
    fn look_at_rh_exact_unit_axes() {
        // BW-5: eye=(1,2,3) → target=(1,2,4) で f=(0,0,1)、
        // s=normalize(f×up)=(-1,0,0)、u=s×f=(0,1,0)。全成分 ±1/0/低整数で
        // f32 厳密 (除算は norm=1.0 のみ)。view 行列の全要素ピン。
        let m = look_at_rh([1.0, 2.0, 3.0], [1.0, 2.0, 4.0], [0.0, 1.0, 0.0]);
        let want = [
            [-1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, 0.0],
            [1.0, -2.0, 3.0, 1.0],
        ];
        assert_eq!(
            m, want,
            "look_at row-major [-s|-u ではなく s,u,-f 行] の厳密形"
        );
    }

    #[test]
    fn perspective_rh_exact_rational_terms() {
        // BW-5: near=1, far=2 → m22 = 2/(1-2) = -2、m32 = (2·1)/(1-2) = -2
        // (f32 除算だが両辺とも厳密値)。構造ゼロ項と m23=-1 も厳密ピン。
        let m = perspective_rh(1.0, 2.0, 1.0, 2.0);
        assert_eq!(m[2][2], -2.0, "far/(near-far) の厳密値");
        assert_eq!(m[3][2], -2.0, "(far*near)/(near-far) の厳密値");
        assert_eq!(m[2][3], -1.0, "RH 深度写像の -1 項");
        for (i, j) in [
            (0, 1),
            (0, 2),
            (0, 3), // m00, m11 は f 項のため対象外
            (1, 0),
            (1, 2),
            (1, 3), // m2x は m22/m32/m23 を個別ピン
            (2, 0),
            (2, 1),
            (3, 0),
            (3, 1),
            (3, 3),
        ] {
            assert_eq!(m[i][j], 0.0, "構造ゼロ項 m[{i}][{j}]");
        }
        // m00 = f/aspect, m11 = f 関係 (tan は libm 依存のため厳密ピンせず、
        // 相関比較で固定): m00 * aspect ≈ m11
        let rel = ((m[0][0] * 2.0 - m[1][1]) / m[1][1]).abs();
        assert!(rel < 1e-6, "m00*aspect ≈ m11: rel={rel}");
        // f = cot(fov/2) の単調減少: fov 増 → f 減 (構造的健全性)。
        let narrow = perspective_rh(0.5, 1.0, 1.0, 2.0);
        let wide = perspective_rh(1.5, 1.0, 1.0, 2.0);
        assert!(narrow[1][1] > wide[1][1], "fov 増で f 減");
    }

    #[test]
    fn mul4_identity_and_diagonal_exact() {
        // BW-5: A·I = A (×1.0 は厳密、0.0 項の加算は値不変)。
        // diag 右乗算は列スケール: out[i][j] = a[i][j] * d[j]。全項 f32 厳密。
        let a = [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
            [13.0, 14.0, 15.0, 16.0],
        ];
        let ident = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        assert_eq!(mul4(a, ident), a);
        let diag = [
            [2.0, 0.0, 0.0, 0.0],
            [0.0, 3.0, 0.0, 0.0],
            [0.0, 0.0, 4.0, 0.0],
            [0.0, 0.0, 0.0, 5.0],
        ];
        let want = [
            [2.0, 6.0, 12.0, 20.0],
            [10.0, 18.0, 28.0, 40.0],
            [18.0, 30.0, 44.0, 60.0],
            [26.0, 42.0, 60.0, 80.0],
        ];
        assert_eq!(mul4(a, diag), want, "列スケール [j*d_j] の厳密値");
    }
}
