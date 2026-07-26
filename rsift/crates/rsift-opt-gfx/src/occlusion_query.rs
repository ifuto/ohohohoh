//! # Occlusion Query (hardware-style visibility queries on stock wgpu) — NEW
//!
//! wgpu 0.20 has no native `GL_SAMPLES_PASSED`-style occlusion-query feature,
//! so this module implements the technique the way modern engines do when
//! they cannot use vendor query extensions: render queried proxy boxes into a
//! tiny **ID buffer** with real depth testing, read it back with a one-frame
//! delay, and count covered cells per box.
//!
//! Two interchangeable backends, one shared bookkeeping core:
//!
//! * [`SoftwareOccluder`] — CPU rasterizer on a small grid (default 160×90).
//!   Fully functional with zero GPU dependencies; used by tools, tests, and as
//!   a fallback when no adapter is available.
//! * [`GpuOcclusionPass`] — wgpu path: draws box instances into an offscreen
//!   `R32Uint` id target + depth texture within the msaa budget, copies to a
//!   readback buffer, and completes async map requests. One frame of latency
//!   hides the round-trip (standard for occlusion queries in every engine).
//!
//! Boxes that report `covered == 0` two frames in a row get culled; hysteresis
//! (`visible_frames_required`) prevents flicker on rapid transits.
//!
//! 深度規則の契約 (wave 22 監査で機械ピン):
//! * 両バックエンドとも z_ndc の**スクリーン線形**補間 — GPU 固定機能の
//!   深度補間規則と同一 (透視補正ではない。varyings と深度バッファの規則差に注意)。
//! * near plane: SW パスはクリップ空間で `z_clip >= 0` に Sutherland–Hodgman
//!   クリッピングしてから除算する (HW クリッパと同規則)。カメラに密着した
//!   ボックスの面を落とさない。x/y の [-1,1] 越えは pixel clamp + バリ centric
//!   テスト、far (z_ndc<=1) はフラグメント毎の z 範囲テストで HW クリッパと
//!   同値に扱う (スクリーン線形 z のため半空間積と一致)。
//! * タイブレーク: SW は `z < stored - 1e-5` の保守的バイアス — 同深度・
//!   ニアリータイは**先に提出されたボックスが勝つ**。GPU パスの
//!   `CompareFunction::Less` (同値は後描画が負ける) より僅かに厳しいが、
//!   差が出るのは 1e-5 帯に限られ、ヒステリシスと組み合わせて可視性の
//!   フリッカーを防ぐ方向にのみ働く。

// ---------------------------------------------------------------------------
// Shared math / bookkeeping
// ---------------------------------------------------------------------------

/// Axis-aligned box being occlusion-queried (a chunk proxy, entity proxy…).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QueryBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl QueryBox {
    pub fn center(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    /// The 8 corners of the box.
    pub fn corners(&self) -> [[f32; 3]; 8] {
        let mut c = [[0.0f32; 3]; 8];
        for (i, corner) in c.iter_mut().enumerate() {
            *corner = [
                if i & 1 == 0 { self.min[0] } else { self.max[0] },
                if i & 2 == 0 { self.min[1] } else { self.max[1] },
                if i & 4 == 0 { self.min[2] } else { self.max[2] },
            ];
        }
        c
    }
}

/// Column-major 4×4 view-projection matrix (WebGPU clip: z ∈ [0,1]).
pub type Mat4 = [f32; 16];

/// Row-major multiply `a * b` over column-major storage (m[col*4+row]).
pub fn mat4_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut s = 0.0f32;
            for k in 0..4 {
                s += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = s;
        }
    }
    out
}

/// Perspective projection for WebGPU clip space (z in [0,1], y-up, RH).
pub fn perspective_wgpu(fov_y_rad: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y_rad * 0.5).tan();
    let mut m = [0.0f32; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = far / (near - far);
    m[11] = -1.0;
    m[14] = (near * far) / (near - far);
    m
}

/// Minimal look-at view matrix (RH, y-up).
pub fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> Mat4 {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let norm = |a: [f32; 3]| {
        let l = dot(a, a).sqrt();
        if l > 1e-8 {
            [a[0] / l, a[1] / l, a[2] / l]
        } else {
            [0.0, 0.0, 0.0]
        }
    };
    let f = norm(sub(center, eye));
    let s = norm(cross(f, up));
    let u = cross(s, f);
    [
        s[0],
        u[0],
        -f[0],
        0.0, //
        s[1],
        u[1],
        -f[1],
        0.0, //
        s[2],
        u[2],
        -f[2],
        0.0, //
        -dot(s, eye),
        -dot(u, eye),
        dot(f, eye),
        1.0,
    ]
}

/// Clip-space transform; returns (ndc x,y in [-1,1], depth in [0,1]) if in front.
pub fn project(m: &Mat4, p: [f32; 3]) -> Option<(f32, f32, f32)> {
    let x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12];
    let y = m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13];
    let z = m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14];
    let w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
    if w <= 1e-6 {
        return None;
    }
    Some((x / w, y / w, z / w))
}

/// Visibility state for one query slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisState {
    /// Not enough information yet: keep drawing.
    Unknown,
    Visible,
    Occluded,
}

/// Bookkeeping entry shared by all backends.
#[derive(Debug, Clone)]
struct QuerySlot {
    bx: QueryBox,
    state: VisState,
    /// Consecutive frames with zero coverage.
    zero_streak: u32,
    /// Consecutive frames with positive coverage.
    hit_streak: u32,
    /// Last raw cell coverage from the backend.
    last_covered: u32,
}

/// Policy knobs for query resolution.
#[derive(Debug, Clone, Copy)]
pub struct OcclusionPolicy {
    /// Box needs this many consecutive covered frames to (re)become visible.
    pub visible_frames_required: u32,
    /// Box is culled after this many consecutive zero-coverage frames.
    pub occlude_after_frames: u32,
    /// A single covered cell counts as visible (anti pop-in bias).
    pub min_cells_visible: u32,
}

impl Default for OcclusionPolicy {
    fn default() -> Self {
        Self {
            visible_frames_required: 1,
            occlude_after_frames: 2,
            min_cells_visible: 1,
        }
    }
}

/// Frame-stable result table.
#[derive(Debug, Default)]
pub struct QueryCore {
    slots: Vec<QuerySlot>,
    policy: OcclusionPolicy,
}

impl QueryCore {
    pub fn new(policy: OcclusionPolicy) -> Self {
        Self {
            slots: Vec::new(),
            policy,
        }
    }

    /// Replace the query set for this frame; ids are stable indices.
    pub fn set_boxes(&mut self, boxes: &[QueryBox]) {
        let old = std::mem::take(&mut self.slots);
        self.slots = boxes
            .iter()
            .enumerate()
            .map(|(i, &bx)| {
                // Preserve state if a box with an identical volume existed before
                // (common when re-submitting the same set every frame).
                let prev = old.iter().find(|s| s.bx == bx);
                QuerySlot {
                    bx,
                    state: prev.map(|s| s.state).unwrap_or(VisState::Unknown),
                    zero_streak: prev.map(|s| s.zero_streak).unwrap_or(0),
                    hit_streak: prev.map(|s| s.hit_streak).unwrap_or(0),
                    last_covered: prev.map(|s| s.last_covered).unwrap_or(0),
                }
                .tap_index(i)
            })
            .collect();
    }

    /// Feed per-box covered-cell counts (index-aligned with `set_boxes`).
    pub fn resolve(&mut self, covered: &[u32]) -> u32 {
        let mut changed = 0;
        for (slot, &cov) in self.slots.iter_mut().zip(covered.iter()) {
            slot.last_covered = cov;
            let was = slot.state;
            if cov >= self.policy.min_cells_visible {
                slot.hit_streak = slot.hit_streak.saturating_add(1);
                slot.zero_streak = 0;
                if slot.hit_streak >= self.policy.visible_frames_required {
                    slot.state = VisState::Visible;
                }
            } else {
                slot.zero_streak = slot.zero_streak.saturating_add(1);
                slot.hit_streak = 0;
                if slot.zero_streak >= self.policy.occlude_after_frames {
                    slot.state = VisState::Occluded;
                } else if slot.state == VisState::Occluded {
                    // Grace window: treat as still-occluded only when the streak
                    // was firmly established; otherwise keep last known state.
                    slot.state = VisState::Occluded;
                }
            }
            if slot.state != was {
                changed += 1;
            }
        }
        changed
    }

    /// Should the owner draw object `id` this frame? (Unknown ⇒ draw.)
    pub fn should_draw(&self, id: usize) -> bool {
        self.slots
            .get(id)
            .map(|s| s.state != VisState::Occluded)
            .unwrap_or(true)
    }

    pub fn state(&self, id: usize) -> VisState {
        self.slots
            .get(id)
            .map(|s| s.state)
            .unwrap_or(VisState::Unknown)
    }

    /// (draw, culled) under current states.
    pub fn stats(&self) -> (usize, usize) {
        let mut d = 0usize;
        let mut c = 0usize;
        for s in &self.slots {
            if s.state == VisState::Occluded {
                c += 1;
            } else {
                d += 1;
            }
        }
        (d, c)
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn boxes(&self) -> impl Iterator<Item = &QueryBox> {
        self.slots.iter().map(|s| &s.bx)
    }
}

// Small helper so the builder map above stays readable without `mut`.
trait TapIndex {
    fn tap_index(self, _i: usize) -> Self;
}
impl TapIndex for QuerySlot {
    #[inline]
    fn tap_index(self, _i: usize) -> Self {
        self
    }
}

// ---------------------------------------------------------------------------
// Software backend (no GPU): grid rasterizer with per-cell depth
// ---------------------------------------------------------------------------

/// Sutherland–Hodgman clip of a clip-space polygon against the wgpu near plane
/// (`z_clip >= 0` half-space)。交点の 4 成分全てを斉次線形補間する
/// (x,y,z,w 同時に t で割り出すことで NDC での正しいクリップ端を得る)。
/// 入力 3 角形から最大 4 角形 (凸、頂点順保持) を返す。
fn clip_polygon_near(poly: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(4);
    let n = poly.len();
    for i in 0..n {
        let cur = poly[i];
        let prev = poly[(i + n - 1) % n];
        let cur_in = cur[2] >= 0.0;
        let prev_in = prev[2] >= 0.0;
        if cur_in != prev_in {
            // t = z_prev / (z_prev - z_cur)。符号相違が保証されているので
            // 分母 ≠ 0。両者とも ≥(≤)0 で境界一致の場合は crossing にならない。
            let t = prev[2] / (prev[2] - cur[2]);
            let mut inter = [0.0f32; 4];
            for k in 0..4 {
                inter[k] = prev[k] + t * (cur[k] - prev[k]);
            }
            out.push(inter);
        }
        if cur_in {
            out.push(cur);
        }
    }
    out
}

/// CPU reference occluder: depth-rasterizes occluding *and* queried boxes on a
/// coarse grid; a query box is visible where it wins the depth test.
pub struct SoftwareOccluder {
    pub width: usize,
    pub height: usize,
    depth: Vec<f32>,
    ids: Vec<u32>,
    pub core: QueryCore,
}

impl SoftwareOccluder {
    pub fn new(width: usize, height: usize, policy: OcclusionPolicy) -> Self {
        Self {
            width: width.max(8),
            height: height.max(8),
            depth: vec![1.0f32; width.max(8) * height.max(8)],
            ids: vec![0u32; width.max(8) * height.max(8)],
            core: QueryCore::new(policy),
        }
    }

    #[inline]
    fn clear(&mut self) {
        self.depth.fill(1.0);
        self.ids.fill(0);
    }

    /// Rasterize one box (12 tris) into the depth+id grid.
    fn rasterize_box(&mut self, view_proj: &Mat4, bx: &QueryBox, id: u32) {
        let c = bx.corners();
        // faces as corner-index quads (two tris each), outward winding irrelevant
        const QUADS: [[usize; 4]; 6] = [
            [0, 1, 3, 2], // front-zmin
            [4, 6, 7, 5], // back
            [0, 4, 5, 1], // bottom
            [2, 3, 7, 6], // top
            [0, 2, 6, 4], // left
            [1, 5, 7, 3], // right
        ];
        // clip 空間 (x,y,z,w) コーナー。透視除算は near-clip 後に行う
        // (旧版は w<=1e-6 のコーナーを持つ面を全スキップしており、カメラに
        // 密着した壁 — Minecraft でプレイヤーが壁際に立つ通常状況 — が
        // covered=0 となり 2 フレーム後に誤カリングされていた = pop-in bug。
        // 2026-07-22 wave 22 監査で homogeneous near-plane clip に根治)。
        let mut clip = [[0.0f32; 4]; 8];
        for (i, p) in c.iter().enumerate() {
            clip[i] = [
                view_proj[0] * p[0] + view_proj[4] * p[1] + view_proj[8] * p[2] + view_proj[12],
                view_proj[1] * p[0] + view_proj[5] * p[1] + view_proj[9] * p[2] + view_proj[13],
                view_proj[2] * p[0] + view_proj[6] * p[1] + view_proj[10] * p[2] + view_proj[14],
                view_proj[3] * p[0] + view_proj[7] * p[1] + view_proj[11] * p[2] + view_proj[15],
            ];
        }
        for q in QUADS {
            let tris = [[q[0], q[1], q[2]], [q[0], q[2], q[3]]];
            for t in tris {
                // z_clip >= 0 (wgpu の near plane 半空間) で Sutherland–Hodgman。
                // 規範的視射影では z_clip>=0 ⟺ 視空間深度 >= near なので
                // 生存頂点の w は必ず > 0 (カメラ背後点は必ず z_clip<0 で落ちる)。
                let poly = clip_polygon_near(&[clip[t[0]], clip[t[1]], clip[t[2]]]);
                if poly.len() < 3 {
                    continue;
                }
                for ti in 1..poly.len() - 1 {
                    let tri = [poly[0], poly[ti], poly[ti + 1]];
                    let mut ndc = [[0.0f32; 3]; 3];
                    let mut ok = true;
                    for (k, v) in tri.iter().enumerate() {
                        if v[3] <= 1e-6 {
                            ok = false; // 規範視射影では到達不能 (防御的ガード)
                            break;
                        }
                        ndc[k] = [v[0] / v[3], v[1] / v[3], v[2] / v[3]];
                    }
                    if ok {
                        self.rasterize_tri_ndc(&ndc, id);
                    }
                }
            }
        }
    }

    /// Rasterize one NDC triangle (z_ndc ∈ [0,1], screen-linear depth =
    /// GPU 固定機能と同一の補間規則) into the depth+id grid.
    fn rasterize_tri_ndc(&mut self, tri: &[[f32; 3]; 3], id: u32) {
        let a = tri[0];
        let b = tri[1];
        let cc = tri[2];
        let w = self.width as f32;
        let h = self.height as f32;
        // to pixel space
        let px = [
            (a[0] * 0.5 + 0.5) * w,
            (b[0] * 0.5 + 0.5) * w,
            (cc[0] * 0.5 + 0.5) * w,
        ];
        let py = [
            (0.5 - a[1] * 0.5) * h,
            (0.5 - b[1] * 0.5) * h,
            (0.5 - cc[1] * 0.5) * h,
        ];
        let minx = px.iter().cloned().fold(f32::INFINITY, f32::min).max(0.0) as usize;
        let maxx = (px.iter().cloned().fold(f32::NEG_INFINITY, f32::max) + 1.0)
            .min(w)
            .max(0.0) as usize;
        let miny = py.iter().cloned().fold(f32::INFINITY, f32::min).max(0.0) as usize;
        let maxy = (py.iter().cloned().fold(f32::NEG_INFINITY, f32::max) + 1.0)
            .min(h)
            .max(0.0) as usize;
        let area = (px[1] - px[0]) * (py[2] - py[0]) - (px[2] - px[0]) * (py[1] - py[0]);
        if area.abs() < 1e-9 {
            return;
        }
        for gy in miny..maxy.min(self.height) {
            for gx in minx..maxx.min(self.width) {
                let x = gx as f32 + 0.5;
                let y = gy as f32 + 0.5;
                let w0 = ((px[1] - x) * (py[2] - y) - (px[2] - x) * (py[1] - y)) / area;
                let w1 = ((px[2] - x) * (py[0] - y) - (px[0] - x) * (py[2] - y)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -0.01 || w1 < -0.01 || w2 < -0.01 {
                    continue;
                }
                let z = w0 * a[2] + w1 * b[2] + w2 * cc[2];
                if !(0.0..=1.0).contains(&z) {
                    continue;
                }
                let idx = gy * self.width + gx;
                if z < self.depth[idx] - 1e-5 {
                    self.depth[idx] = z;
                    self.ids[idx] = id;
                }
            }
        }
    }

    /// Run a full frame: `boxes` are both geometry and queries (self-occlusion
    /// allowed, nearest wins). Returns per-box covered-cell counts.
    pub fn run_frame(&mut self, view_proj: &Mat4, boxes: &[QueryBox]) -> Vec<u32> {
        self.core.set_boxes(boxes);
        self.clear();
        for (i, bx) in boxes.iter().enumerate() {
            self.rasterize_box(view_proj, bx, (i + 1) as u32);
        }
        let mut covered = vec![0u32; boxes.len()];
        for &id in &self.ids {
            if id > 0 {
                covered[(id - 1) as usize] += 1;
            }
        }
        self.core.resolve(&covered);
        covered
    }

    pub fn should_draw(&self, id: usize) -> bool {
        self.core.should_draw(id)
    }
}

// ---------------------------------------------------------------------------
// GPU backend (wgpu 0.20): offscreen R32Uint id pass + readback
// ---------------------------------------------------------------------------

/// Vertex of the unit-query-box instance expansion (CPU transforms corners).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct OccVertex {
    pos: [f32; 3],
    object_id: u32,
}

const OCC_WGSL: &str = r#"
struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) object_id: u32,
};
struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) object_id: u32,
};
struct Params {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> params: Params;

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    var o: VertexOut;
    o.clip = params.view_proj * vec4<f32>(v.pos, 1.0);
    o.object_id = v.object_id;
    return o;
}

@fragment
fn fs_main(v: VertexOut) -> @location(0) u32 {
    return v.object_id;
}
"#;

/// wgpu occlusion pass. Renders query boxes into an `R32Uint` id texture with
/// depth, copies to a readback buffer, resolves with one frame of latency.
pub struct GpuOcclusionPass {
    size: wgpu::Extent3d,
    id_tex: wgpu::Texture,
    id_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    params_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    pub core: QueryCore,
    map_pending: bool,
}

impl GpuOcclusionPass {
    pub const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;

    pub fn new(device: &wgpu::Device, width: u32, height: u32, policy: OcclusionPolicy) -> Self {
        let width = width.max(8);
        let height = height.max(8);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let id_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("occ-id"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: Self::ID_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let id_view = id_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("occ-depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let bpr = padded_bytes_per_row(width);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("occ-readback"),
            size: (bpr * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("occ-params"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("occ-shader"),
            source: wgpu::ShaderSource::Wgsl(OCC_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("occ-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("occ-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buf.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("occ-layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("occ-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<OccVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: Self::ID_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None, // conservative: never drop backfaces for queries
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        Self {
            size,
            id_tex,
            id_view,
            depth_view,
            readback,
            params_buf,
            bind_group,
            pipeline,
            core: QueryCore::new(policy),
            map_pending: false,
        }
    }

    /// CPU-side expansion of query boxes into triangle soup (12 tris / box).
    /// **wave 126 DZ-4**: 消費者は自モジュール内 (:716 本体・テスト) のみで
    /// `pub` 公開面の実需なし → private 化して OccVertex (pub(self)) との
    /// private_interfaces 警告を根治。将来の外部需要時に再公開方針 (保持)。
    fn build_vertices(boxes: &[QueryBox]) -> Vec<OccVertex> {
        const TRIS: [[usize; 3]; 12] = [
            [0, 1, 3],
            [0, 3, 2], // zmin
            [4, 6, 7],
            [4, 7, 5], // zmax
            [0, 4, 5],
            [0, 5, 1], // ymin
            [2, 3, 7],
            [2, 7, 6], // ymax
            [0, 2, 6],
            [0, 6, 4], // xmin
            [1, 5, 7],
            [1, 7, 3], // xmax
        ];
        let mut out = Vec::with_capacity(boxes.len() * 36);
        for (i, bx) in boxes.iter().enumerate() {
            let corners = bx.corners();
            for t in TRIS {
                for &ci in &t {
                    out.push(OccVertex {
                        pos: corners[ci],
                        object_id: (i + 1) as u32,
                    });
                }
            }
        }
        out
    }

    /// Records the id pass + readback copy into `encoder`.
    // 注: device はボックス動的変更時の bind group/scratch 再構築に備えた将来拡張用
    // (現行は new() 時に bind group を固定生成するため未使用 — 監査警告 occlusion_query.rs:632)。
    pub fn record(
        &mut self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view_proj: &Mat4,
        boxes: &[QueryBox],
        scratch_vb: &wgpu::Buffer,
    ) {
        self.core.set_boxes(boxes);
        queue.write_buffer(&self.params_buf, 0, bytemuck::cast_slice(view_proj));

        let verts = Self::build_vertices(boxes);
        if !verts.is_empty() {
            debug_assert!(
                (verts.len() * std::mem::size_of::<OccVertex>()) as u64 <= scratch_vb.size(),
                "scratch vertex buffer too small"
            );
            queue.write_buffer(scratch_vb, 0, bytemuck::cast_slice(&verts));
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("occ-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.id_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            if !verts.is_empty() {
                pass.set_vertex_buffer(0, scratch_vb.slice(..));
                pass.draw(0..verts.len() as u32, 0..1);
            }
        }

        let bpr = padded_bytes_per_row(self.size.width);
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.id_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &self.readback,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(self.size.height),
                },
            },
            self.size,
        );
        self.map_pending = true;
    }

    /// Call after `queue.submit`. Starts async map; returns true when a
    /// fresh result was resolved into `self.core` (normally next frame).
    pub fn poll_resolve(&mut self, device: &wgpu::Device) -> bool {
        if !self.map_pending {
            return false;
        }
        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        device.poll(wgpu::Maintain::Wait);
        if rx.recv().is_err() {
            return false;
        }
        let n = self.core.len();
        let mut covered = vec![0u32; n];
        {
            let data = slice.get_mapped_range();
            let bpr = padded_bytes_per_row(self.size.width) as usize;
            for row in 0..self.size.height as usize {
                let base = row * bpr;
                let row_data = &data[base..base + self.size.width as usize * 4];
                for (x, cell) in row_data.chunks_exact(4).enumerate() {
                    let _ = x;
                    let id = u32::from_le_bytes([cell[0], cell[1], cell[2], cell[3]]);
                    if id > 0 && (id as usize) <= n {
                        covered[(id - 1) as usize] += 1;
                    }
                }
            }
        }
        self.readback.unmap();
        self.map_pending = false;
        self.core.resolve(&covered);
        true
    }

    pub fn should_draw(&self, id: usize) -> bool {
        self.core.should_draw(id)
    }
}

/// 256-byte row alignment required for buffer↔texture copies.
fn padded_bytes_per_row(width_px: u32) -> u32 {
    let raw = width_px * 4;
    (raw + 255) & !255
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Mat4 {
        let view = look_at([0.0, 0.0, 10.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let proj = perspective_wgpu(60f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        mat4_mul(&proj, &view)
    }

    #[test]
    fn software_fully_hidden_box_is_culled() {
        let mut occ = SoftwareOccluder::new(160, 90, OcclusionPolicy::default());
        let vp = cam();
        // Box 0: small box directly *behind* a huge box 1.
        let behind = QueryBox {
            min: [-0.5, -0.5, -6.0],
            max: [0.5, 0.5, -5.0],
        };
        let wall = QueryBox {
            min: [-8.0, -8.0, -4.0],
            max: [8.0, 8.0, -3.0],
        };
        let boxes = [behind, wall];
        // Frame 1 (Unknown → resolve), frame 2 (streak hits occlude_after).
        occ.run_frame(&vp, &boxes);
        occ.run_frame(&vp, &boxes);
        assert!(
            !occ.should_draw(0),
            "fully hidden box must be culled after policy frames"
        );
        assert!(occ.should_draw(1), "the occluder itself stays visible");
        let (_draw, culled) = occ.core.stats();
        assert_eq!(culled, 1);
    }

    #[test]
    fn software_visible_box_stays() {
        let mut occ = SoftwareOccluder::new(160, 90, OcclusionPolicy::default());
        let vp = cam();
        let solo = QueryBox {
            min: [-1.0, -1.0, 2.0],
            max: [1.0, 1.0, 4.0],
        };
        let covered = occ.run_frame(&vp, &[solo]);
        assert!(covered[0] > 0, "front box must cover cells");
        assert!(occ.should_draw(0));
    }

    #[test]
    fn hysteresis_requires_streak() {
        let mut core = QueryCore::new(OcclusionPolicy {
            visible_frames_required: 1,
            occlude_after_frames: 3,
            min_cells_visible: 1,
        });
        let bx = QueryBox {
            min: [0.0; 3],
            max: [1.0; 3],
        };
        core.set_boxes(&[bx]);
        core.resolve(&[0]);
        assert_eq!(core.state(0), VisState::Unknown);
        core.resolve(&[0]);
        assert_eq!(core.state(0), VisState::Unknown);
        core.resolve(&[0]);
        assert_eq!(core.state(0), VisState::Occluded);
        core.resolve(&[5]);
        assert_eq!(core.state(0), VisState::Visible);
    }

    #[test]
    fn gpu_vertex_expansion_counts() {
        let bx = QueryBox {
            min: [0.0; 3],
            max: [1.0; 3],
        };
        let v = GpuOcclusionPass::build_vertices(&[bx, bx]);
        assert_eq!(v.len(), 2 * 36);
        assert_eq!(v[0].object_id, 1);
        assert_eq!(v[36].object_id, 2);
    }

    #[test]
    fn padded_row_alignment() {
        assert_eq!(padded_bytes_per_row(1), 256);
        assert_eq!(padded_bytes_per_row(64), 256);
        assert_eq!(padded_bytes_per_row(65), 512);
    }

    #[test]
    fn projection_math_sane() {
        let vp = cam();
        // Point straight ahead of camera should land near screen center.
        let (x, y, z) = project(&vp, [0.0, 0.0, 0.0]).unwrap();
        assert!(x.abs() < 1e-3 && y.abs() < 1e-3, "center: ({x},{y})");
        assert!((0.0..1.0).contains(&z));
        // Behind the camera → None.
        assert!(project(&vp, [0.0, 0.0, 20.0]).is_none());
    }

    /// wave 22-1: カメラを飲み込み、かつ near plane と背面を跨ぐ長い回廊箱
    /// (Minecraft でプレイヤーが長い構造物の中に立つ通常状況) が covered=0 で
    /// 誤カリングされないこと。camera (0,0,10), near=0.1, far=100:
    /// - 背面 z=10.05 はカメラ背後 0.05 (z_clip<0) → 側面 6 面全てが
    ///   「近すぎ or 背後」のコーナーを持ち、旧版の頂点単位スキップでは
    ///   全12三角形が落とされ covered=0 になっていた (pop-in bug)。
    /// - 側壁は視深度 7.8〜30 の区間で (|x_ndc|=8·(f/aspect)/d ≤ 1 条件より)
    ///   確実に画面内に投影されるため、正しくは covered>0。
    /// - 前面 z=-95 は視深度 105 > far で z 範囲外 (寄与ゼロ) に置き、
    ///   「front 面が代わりに覆うだけ」の不純な通過を防いである。
    #[test]
    fn software_near_plane_straddling_wall_stays_visible() {
        let mut occ = SoftwareOccluder::new(160, 90, OcclusionPolicy::default());
        let vp = cam();
        let corridor = QueryBox {
            min: [-8.0, -8.0, -95.0],
            max: [8.0, 8.0, 10.05],
        };
        let c1 = occ.run_frame(&vp, &[corridor]);
        let c2 = occ.run_frame(&vp, &[corridor]);
        assert!(
            c1[0] > 0,
            "corridor engulfing camera must cover cells: {c1:?}"
        );
        assert!(c2[0] > 0);
        assert!(
            occ.should_draw(0),
            "box engulfing the camera must not pop out"
        );
    }

    /// wave 22-2: カメラ完全背後のボックスは homogeneous clip 後も 0 セル
    /// (near-plane clip がカメラ背後除去と同値であることの機械ピン)。
    #[test]
    fn software_fully_behind_camera_covers_nothing() {
        let mut occ = SoftwareOccluder::new(160, 90, OcclusionPolicy::default());
        let vp = cam();
        let back = QueryBox {
            min: [-1.0, -1.0, 15.0],
            max: [1.0, 1.0, 17.0],
        };
        let c = occ.run_frame(&vp, &[back]);
        assert_eq!(c[0], 0, "behind-camera box must cover no cells: {c:?}");
    }

    /// wave 22-3: 同深度タイは 1e-5 バイアスにより**先に提出されたボックス**
    /// が勝つ (ヘッダ契約の機械ピン。GPU Less との差分はこの帯のみ)。
    #[test]
    fn identical_depth_tie_favors_first_submission() {
        let mut occ = SoftwareOccluder::new(160, 90, OcclusionPolicy::default());
        let vp = cam();
        let bx = QueryBox {
            min: [-1.0, -1.0, 0.0],
            max: [1.0, 1.0, 2.0],
        };
        let c = occ.run_frame(&vp, &[bx, bx]);
        assert!(
            c[0] > 0 && c[1] == 0,
            "tie must favor the earlier-submitted box: {c:?}"
        );
    }

    /// wave 22-4: clip_polygon_near 自体の幾何契約 — 全内は素通し (3 頂点)、
    /// 全外は 0、1 頂点のみ外は 3 頂点 (くさび)、1 頂点のみ内は 3 頂点 + 端が
    /// 平面上 (z=0±ε)。頂点数と z 符号だけを厳密検査 (座標値は補間誤差を含む)。
    #[test]
    fn clip_polygon_near_cardinality() {
        let inside = [
            [0.0, 0.0, 0.5, 1.0],
            [1.0, 0.0, 0.5, 1.0],
            [0.0, 1.0, 0.5, 1.0],
        ];
        assert_eq!(clip_polygon_near(&inside).len(), 3);
        let outside = [
            [0.0, 0.0, -0.5, 1.0],
            [1.0, 0.0, -0.5, 1.0],
            [0.0, 1.0, -0.5, 1.0],
        ];
        assert_eq!(clip_polygon_near(&outside).len(), 0);
        // 2 頂点内・1 頂点外 → 内側領域は 4 角形 (元の内-内辺 + 2 交点)
        let one_out = [
            [0.0, 0.0, 0.5, 1.0],
            [1.0, 0.0, 0.5, 1.0],
            [0.0, 1.0, -0.5, 1.0],
        ];
        let poly = clip_polygon_near(&one_out);
        assert_eq!(poly.len(), 4, "2-in clip must be a quad: {poly:?}");
        for v in &poly {
            assert!(v[2] >= -1e-6, "clipped verts must be z>=0: {poly:?}");
        }
        // 1 頂点内・2 頂点外 → 内側領域は 3 角形のくさび
        let two_out = [
            [0.0, 0.0, 0.5, 1.0],
            [1.0, 0.0, -0.5, 1.0],
            [0.0, 1.0, -0.5, 1.0],
        ];
        let poly = clip_polygon_near(&two_out);
        assert_eq!(
            poly.len(),
            3,
            "1-in clip must be a wedge triangle: {poly:?}"
        );
        for v in &poly {
            assert!(v[2] >= -1e-6, "clipped verts must be z>=0: {poly:?}");
        }
    }
}
