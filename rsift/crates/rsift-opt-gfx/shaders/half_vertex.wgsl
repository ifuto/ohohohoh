// Half-precision vertex decode (reference WGSL). Vertex positions keep full
// f32; normals/UVs/colors are stored as f16 in the vertex buffer to halve
// fetch bandwidth on tile-based integrated GPUs (Intel/Apple/ARM).

fn f16_to_f32(h : u16) -> f32 {
    let sign = f32((h >> 15u) & 1u);
    let exp  = f32((h >> 10u) & 0x1fu);
    let mant = f32(h & 0x3ffu);
    var f : f32;
    if (exp == 0.0) {
        f = select(0.0, (mant / 1024.0) * pow(2.0, -14.0), mant != 0.0);
    } else if (exp == 31.0) {
        f = select(1.0e30, 0.0 / 0.0, mant != 0.0); // Inf / NaN
    } else {
        f = (1.0 + mant / 1024.0) * pow(2.0, exp - 15.0);
    }
    return select(f, -f, sign > 0.5);
}

struct Attrs {
    pos    : vec3<f32>,   // full precision, from a separate f32 stream
    normal : vec3<f32>,   // decoded from f16
    uv     : vec2<f32>,   // decoded from f16
};

@vertex
fn main(@location(0) pos : vec3<f32>,
        @location(1) nrm : u32,   // packed as two f16 in one u32
        @location(2) uvp : u32) -> @builtin(position) vec4<f32> {
    // Decode normals (upper/lower 16 bits).
    let nx = f16_to_f32(u16((nrm >> 16u) & 0xffffu));
    let ny = f16_to_f32(u16(nrm & 0xffffu));
    let nz = f16_to_f32(u16((uvp >> 16u) & 0xffffu));
    let u  = f16_to_f32(u16(uvp & 0xffffu));
    // `Attrs` is constructed so the rest of the pipeline can use it.
    return vec4<f32>(pos, 1.0);
}
