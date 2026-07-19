// Mesh-shader path (Vulkan NV/EXT mesh_shader — not WebGPU core).
// Expands PullQuad SSBO records without an index buffer (mirrors terrain_vertex_pull.wgsl).
//
// Low-spec note: GpuVertexPullEngine always selects VertexPull on wgpu 0.20;
// this file is ready for when EXPERIMENTAL_MESH_SHADER lands.

enable mesh_shader;

struct PullQuad {
    word0: u32,
    word1: u32,
}

struct TaskPayload {
    quad_base: u32,
    quad_count: u32,
}

struct FrameUniforms {
    view_proj: mat4x4<f32>,
    chunk_origin: vec4<f32>,
}

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(0) @binding(1) var<storage, read> quads: array<PullQuad>;

var<workgroup> payload: TaskPayload;

const COORD_MASK: u32 = 63u;
const TEX_MASK: u32 = 4095u;
const TRI_CORNER: array<u32, 6> = array<u32, 6>(0u, 1u, 2u, 2u, 3u, 0u);
const FACE_UV: array<vec2<f32>, 4> = array<vec2<f32>, 4>(
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
        case 0u: { return vec3<f32>(ox + 1.0, oy + cv * h, oz + cu * w); }
        case 1u: { return vec3<f32>(ox, oy + cv * h, oz + cu * w); }
        case 2u: { return vec3<f32>(ox + cu * w, oy + 1.0, oz + cv * h); }
        case 3u: { return vec3<f32>(ox + cu * w, oy, oz + cv * h); }
        case 4u: { return vec3<f32>(ox + cu * w, oy + cv * h, oz + 1.0); }
        default: { return vec3<f32>(ox + cu * w, oy + cv * h, oz); }
    }
}

@task
fn ts_main(@builtin(workgroup_id) wid: vec3<u32>) {
    let quads_per_meshlet = 32u; // smaller meshlets = less register pressure on weak GPUs
    payload.quad_base = wid.x * quads_per_meshlet;
    let remain = arrayLength(&quads) - payload.quad_base;
    payload.quad_count = min(quads_per_meshlet, remain);
    DispatchMesh(payload.quad_count, 1u, 1u);
}

struct MsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) tex_id: u32,
    @location(2) @interpolate(flat) light_ao: u32,
    @location(3) normal: vec3<f32>,
}

@mesh
fn ms_main(
    @builtin(local_invocation_index) lid: u32,
) {
    // One meshlet invocation emits one quad (6 verts / 2 tris).
    SetMeshOutputs(6u, 2u);
    if lid > 0u || payload.quad_count == 0u {
        return;
    }
    let q = quads[payload.quad_base];
    let ox = f32(unpack_x(q.word0));
    let oy = f32(unpack_y(q.word0));
    let oz = f32(unpack_z(q.word0));
    let tex = unpack_tex(q.word0);
    let lao = unpack_light_ao(q.word0);
    let face = unpack_face(q.word1);
    let w = f32(unpack_width(q.word1));
    let h = f32(unpack_height(q.word1));
    let n = face_normal(face);

    for (var i = 0u; i < 6u; i++) {
        let corner = TRI_CORNER[i];
        let local = corner_pos(face, ox, oy, oz, w, h, corner);
        let world = local + frame.chunk_origin.xyz;
        var v: MsOut;
        v.clip = frame.view_proj * vec4<f32>(world, 1.0);
        v.uv = FACE_UV[corner];
        v.tex_id = tex;
        v.light_ao = lao;
        v.normal = n;
        SetMeshOutputVertex(i, v);
    }
    SetMeshOutputPrimitive(0u, 0u, 1u, 2u);
    SetMeshOutputPrimitive(1u, 3u, 4u, 5u);
}
