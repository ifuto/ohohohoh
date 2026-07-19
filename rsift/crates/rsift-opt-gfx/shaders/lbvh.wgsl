// Linear BVH — Morton code + frustum cull in WGSL.
// Companion to src/lbvh.rs.

fn part1by2(mut n : u32) -> u32 {
    n = n & 0x3ffu;
    n = (n | (n << 16u)) & 0x30000ffu;
    n = (n | (n << 8u)) & 0x300f00fu;
    n = (n | (n << 4u)) & 0x30c30c3u;
    n = (n | (n << 2u)) & 0x9249249u;
    return n;
}

fn morton3(x : u32, y : u32, z : u32) -> u32 {
    return part1by2(x) | (part1by2(y) << 1u) | (part1by2(z) << 2u);
}

struct Plane { a : f32, b : f32, c : f32, d : f32, };
@group(0) @binding(0) var<storage, read> centers : array<vec3<f32>>;
@group(0) @binding(1) var<storage, read> radii : array<f32>;
@group(0) @binding(2) var<storage, read> planes : array<Plane>;
@group(0) @binding(3) var<storage, read_write> visible : array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&centers)) { return; }
    let c = centers[i];
    let r = radii[i];
    var outside = false;
    for (var p : u32 = 0u; p < arrayLength(&planes); p = p + 1u) {
        let pl = planes[p];
        if ((pl.a * c.x + pl.b * c.y + pl.c * c.z + pl.d) < -r) { outside = true; }
    }
    visible[i] = select(1u, 0u, outside);
}
