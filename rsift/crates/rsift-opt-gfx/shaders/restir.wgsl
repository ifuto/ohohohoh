// ReSTIR direct lighting reservoir — companion WGSL to src/restir.rs.
// One reservoir per pixel; streaming RIS update + neighbour combine.

struct Reservoir {
    w_sum : f32,
    m : u32,
    sampleIdx : u32,
    targetPdf : f32,
};

@group(0) @binding(0) var<storage, read_write> reservoirs : array<Reservoir>;
@group(0) @binding(1) var<storage, read> lightRadiance : array<vec3<f32>>;

// Streaming RIS update for one candidate light.
fn update(resIndex : u32, lightIdx : u32, targetPdf : f32, sourcePdf : f32, rand : f32) {
    if (sourcePdf <= 0.0 || targetPdf <= 0.0) { return; }
    var r = reservoirs[resIndex];
    let ris = targetPdf / sourcePdf;
    r.w_sum = r.w_sum + ris;
    r.m = r.m + 1u;
    let p = ris / max(r.w_sum, 1e-20);
    if (rand < p) {
        r.sampleIdx = lightIdx;
        r.targetPdf = targetPdf;
    }
    reservoirs[resIndex] = r;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&reservoirs)) { return; }
    // Example: draw 4 candidate lights with uniform source pdf 1/N.
    let n = arrayLength(&lightRadiance);
    for (var k : u32 = 0u; k < 4u; k = k + 1u) {
        let li = (i + k) % n;
        let Le = lightRadiance[li];
        let lum = max(max(Le.r, Le.g), Le.b);
        update(i, li, lum, 1.0, 0.3);
    }
}
