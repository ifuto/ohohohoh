// Async compute overlap scheduler — companion WGSL.
// Records a post-FX command into a ring buffer that the NEXT frame's
// shadow pass consumes, so post cost overlaps shadow rasterization.

struct QueueTag { graphics : u32, compute : u32, };
@group(0) @binding(0) var<storage, read_write> passCost : array<f32>;
@group(0) @binding(1) var<storage, read_write> passQueue : array<u32>;
@group(0) @binding(2) var<storage, read_write> overlapResult : array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&passCost)) { return; }
    // graphics total vs compute total; overlapped frame time = max of the two.
    var g : f32 = 0.0;
    var c : f32 = 0.0;
    for (var k : u32 = 0u; k < arrayLength(&passCost); k = k + 1u) {
        if (passQueue[k] == 0u) { g = g + passCost[k]; }
        else { c = c + passCost[k]; }
    }
    overlapResult[0] = max(g, c);
}
