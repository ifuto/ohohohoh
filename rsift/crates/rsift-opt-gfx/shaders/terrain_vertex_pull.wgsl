// Vertex pulling terrain — no VBO attributes, no index buffer.
// SSBO quad records (8 B each) expanded from vertex_index alone.

struct PullQuad {
    word0: u32,
    word1: u32,
}

struct FrameUniforms {
    view_proj: mat4x4<f32>,
    chunk_origin: vec4<f32>, // xyz world blocks, w unused
}

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(0) @binding(1) var<storage, read> quads: array<PullQuad>;

const COORD_MASK: u32 = 63u;
const TEX_MASK: u32 = 4095u;

// Two triangles: corners [0,1,2] [2,3,0]
// 注: FACE_UV と同様、vid 由来の動的 index を通すため const ではなく var<private>
// (naga IndexMustBeConstant 回避、値は不変)。
var<private> TRI_CORNER: array<u32, 6> = array<u32, 6>(0u, 1u, 2u, 2u, 3u, 0u);

// Per-face unit UVs (0-bit stored — derived from corner % 4)
// 注: 元は const 配列だったが、corner (実行時値) による動的 index は WGSL 検証不可
// (naga IndexMustBeConstant — 2026-07-21 監査で検出)。var<private> に置き換えて
// モジュールスコープのプライベートメモリから読む形にする (内容は不変、同一値)。
var<private> FACE_UV: array<vec2<f32>, 4> = array<vec2<f32>, 4>(
    vec2<f32>(0.0, 0.0),
    vec2<f32>(1.0, 0.0),
    vec2<f32>(1.0, 1.0),
    vec2<f32>(0.0, 1.0),
);

fn unpack_x(w: u32) -> u32 { return w & COORD_MASK; }
fn unpack_y(w: u32) -> u32 { return (w >> 6u) & COORD_MASK; }
fn unpack_z(w: u32) -> u32 { return (w >> 12u) & COORD_MASK; }
fn unpack_tex(w: u32) -> u32 { return (w >> 18u) & TEX_MASK; }
fn unpack_light_ao(w: u32) -> u32 { return (w >> 30u) & 3u; }
fn unpack_face(w1: u32) -> u32 { return w1 & 7u; }
fn unpack_width(w1: u32) -> u32 { return ((w1 >> 3u) & 63u) + 1u; }
fn unpack_height(w1: u32) -> u32 { return ((w1 >> 9u) & 63u) + 1u; }

fn face_normal(face: u32) -> vec3<f32> {
    switch face {
        case 0u: { return vec3<f32>(1.0, 0.0, 0.0); }
        case 1u: { return vec3<f32>(-1.0, 0.0, 0.0); }
        case 2u: { return vec3<f32>(0.0, 1.0, 0.0); }
        case 3u: { return vec3<f32>(0.0, -1.0, 0.0); }
        case 4u: { return vec3<f32>(0.0, 0.0, 1.0); }
        default: { return vec3<f32>(0.0, 0.0, -1.0); }
    }
}

fn corner_pos(face: u32, ox: f32, oy: f32, oz: f32, w: f32, h: f32, corner: u32) -> vec3<f32> {
    let cu = FACE_UV[corner].x;
    let cv = FACE_UV[corner].y;
    switch face {
        // +X
        case 0u: {
            let y = oy + cv * h;
            let z = oz + cu * w;
            return vec3<f32>(ox + 1.0, y, z);
        }
        // -X
        case 1u: {
            let y = oy + cv * h;
            let z = oz + cu * w;
            return vec3<f32>(ox, y, z);
        }
        // +Y
        case 2u: {
            let x = ox + cu * w;
            let z = oz + cv * h;
            return vec3<f32>(x, oy + 1.0, z);
        }
        // -Y
        case 3u: {
            let x = ox + cu * w;
            let z = oz + cv * h;
            return vec3<f32>(x, oy, z);
        }
        // +Z
        case 4u: {
            let x = ox + cu * w;
            let y = oy + cv * h;
            return vec3<f32>(x, y, oz + 1.0);
        }
        // -Z
        default: {
            let x = ox + cu * w;
            let y = oy + cv * h;
            return vec3<f32>(x, y, oz);
        }
    }
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) tex_id: u32,
    @location(2) @interpolate(flat) light_ao: u32,
    @location(3) normal: vec3<f32>,
}

@vertex
fn vs_pull(@builtin(vertex_index) vid: u32) -> VsOut {
    let quad_id = vid / 6u;
    let corner = TRI_CORNER[vid % 6u];
    let q = quads[quad_id];

    let ox = f32(unpack_x(q.word0));
    let oy = f32(unpack_y(q.word0));
    let oz = f32(unpack_z(q.word0));
    let tex = unpack_tex(q.word0);
    let lao = unpack_light_ao(q.word0);
    let face = unpack_face(q.word1);
    let w = f32(unpack_width(q.word1));
    let h = f32(unpack_height(q.word1));

    let local = corner_pos(face, ox, oy, oz, w, h, corner);
    let world = local + frame.chunk_origin.xyz;

    var out: VsOut;
    out.clip = frame.view_proj * vec4<f32>(world, 1.0);
    out.uv = FACE_UV[corner];
    out.tex_id = tex;
    out.light_ao = lao;
    out.normal = face_normal(face);
    return out;
}

@fragment
fn fs_pull(in: VsOut) -> @location(0) vec4<f32> {
    let shade = 0.55 + f32(in.light_ao) * 0.15;
    let band = f32(in.tex_id % 7u) / 7.0;
    return vec4<f32>(band * shade, band * 0.6 * shade, band * 0.3 * shade, 1.0);
}
