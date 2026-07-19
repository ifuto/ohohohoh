// Virtual (sparse) texture address translation (reference WGSL). Translates a
// (page, mip) request into a physical page index via an indirection table;
// unmapped pages fall back to a low-res resident mip. Only streamed pages cost
// VRAM, which matters on unified-memory integrated GPUs.

struct U { maxMip : u32, };
@group(0) @binding(0) var<uniform> u : U;
// page table: page_id -> physical slot (or 0xFFFFFFFF if not resident)
@group(0) @binding(1) var<storage, read> pageTable : array<u32>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
    let page = gid.x;
    let mip = gid.y;
    let idx = page * (u.maxMip + 1u) + mip;
    var phys = pageTable[idx];
    if (phys == 0xFFFFFFFFu) {
        // not resident -> fall back to mip 0 if present, else stay invalid
        let base = page * (u.maxMip + 1u);
        if (pageTable[base] != 0xFFFFFFFFu) {
            phys = pageTable[base];
        }
    }
    // `phys` is what the sampler uses to index the physical texture atlas.
}
