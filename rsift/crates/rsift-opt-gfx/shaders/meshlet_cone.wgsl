// Meshlet normal-cone backface culling (reference WGSL, runs in a task/amplification
// or compute shader). If the camera is outside the meshlet's normal cone, every
// triangle is back-facing and the whole cluster is skipped before vertex shading.

struct Meshlet {
    coneAxis   : vec3<f32>,
    cosAngle   : f32,        // cos(cone half-angle + 90°)
    center     : vec3<f32>,
};

@group(0) @binding(0) var<storage, read> meshlets : array<Meshlet>;
@group(0) @binding(1) var<storage, read_write> visibility : array<u32>; // 1 = visible
@group(0) @binding(2) var<uniform> camPos : vec3<f32>;

@compute @workgroup_size(64, 1, 1)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&meshlets)) { return; }
    let m = meshlets[i];
    let toCam = normalize(camPos - m.center);
    let visible = dot(toCam, m.coneAxis) >= m.cosAngle;
    visibility[i] = select(0u, 1u, visible);
}
