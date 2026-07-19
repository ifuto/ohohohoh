// Clustered (forward+) light culling (reference WGSL compute pass). For each
// screen-space tile and depth slice, gathers the point lights whose sphere
// intersects the cluster AABB. The forward shader then loops only over the
// lights in its pixel's cluster. One invocation per cluster.

struct U {
    tilesX : u32,
    tilesY : u32,
    slices : u32,
};
@group(0) @binding(0) var<uniform> u : U;
struct Light { position : vec3<f32>, radius : f32, };
@group(0) @binding(1) var<storage, read> lights : array<Light>;
@group(0) @binding(2) var<storage, read_write> clusterLights : array<u32>; // packed counts/indices

@compute @workgroup_size(64, 1, 1)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
    let cx = gid.x % u.tilesX;
    let cy = (gid.x / u.tilesX) % u.tilesY;
    let cz = gid.x / (u.tilesX * u.tilesY);
    if (cz >= u.slices) { return; }

    let sx = 1.0 / f32(u.tilesX);
    let sy = 1.0 / f32(u.tilesY);
    let sz = 1.0 / f32(u.slices);
    let mn = vec3<f32>(f32(cx) * sx, f32(cy) * sy, f32(cz) * sz);
    let mx = vec3<f32>(f32(cx + 1) * sx, f32(cy + 1) * sy, f32(cz + 1) * sz);

    var count = 0u;
    for (var i : u32 = 0u; i < arrayLength(&lights); i = i + 1u) {
        let l = lights[i];
        let v = clamp(l.position, mn, mx);
        let diff = l.position - v;
        if (dot(diff, diff) <= l.radius * l.radius) {
            count = count + 1u;
        }
    }
    // In a full engine this would append indices; here we record the count.
    clusterLights[gid.x] = count;
}
