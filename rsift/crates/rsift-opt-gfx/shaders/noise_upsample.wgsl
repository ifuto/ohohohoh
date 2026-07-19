// Coarse 3D noise grid + trilinear upsample (GPU path).
// stride=4 → ~45× fewer noise evaluations vs dense 32³.

struct UpsampleParams {
    stride: u32,
    seed: u32,
    origin_x: i32,
    origin_y: i32,
    origin_z: i32,
    size_x: u32,
    size_y: u32,
    size_z: u32,
    cave_threshold: f32,
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: UpsampleParams;
@group(0) @binding(1) var<storage, read_write> coarse: array<f32>;
@group(0) @binding(2) var<storage, read_write> dense: array<f32>;

fn hash3(x: u32, y: u32, z: u32, seed: u32) -> u32 {
    var h = seed ^ x * 0x9E3779B9u ^ y * 0x85EBCA6Bu ^ z * 0xC2B2AE35u;
    h ^= h >> 16u;
    h = h * 0x7FEB352Du;
    h ^= h >> 15u;
    h = h * 0x846CA68Bu;
    h ^= h >> 16u;
    return h;
}

fn fade(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

fn perlin_at(wx: i32, wy: i32, wz: i32, seed: u32) -> f32 {
    let xf = f32(wx & 255);
    let yf = f32(wy & 255);
    let zf = f32(wz & 255);
    let fx = f32(wx) - floor(f32(wx));
    let fy = f32(wy) - floor(f32(wy));
    let fz = f32(wz) - floor(f32(wz));
    let u = fade(fx);
    let v = fade(fy);
    let w = fade(fz);
    // Simplified value lerp (matches CPU path closely enough for terrain).
    let h000 = f32(hash3(xf, yf, zf, seed) & 0xFFu) / 255.0;
    let h100 = f32(hash3(xf + 1u, yf, zf, seed) & 0xFFu) / 255.0;
    let h010 = f32(hash3(xf, yf + 1u, zf, seed) & 0xFFu) / 255.0;
    let h110 = f32(hash3(xf + 1u, yf + 1u, zf, seed) & 0xFFu) / 255.0;
    let h001 = f32(hash3(xf, yf, zf + 1u, seed) & 0xFFu) / 255.0;
    let h101 = f32(hash3(xf + 1u, yf, zf + 1u, seed) & 0xFFu) / 255.0;
    let h011 = f32(hash3(xf, yf + 1u, zf + 1u, seed) & 0xFFu) / 255.0;
    let h111 = f32(hash3(xf + 1u, yf + 1u, zf + 1u, seed) & 0xFFu) / 255.0;
    let x00 = mix(h000, h100, u);
    let x10 = mix(h010, h110, u);
    let x01 = mix(h001, h101, u);
    let x11 = mix(h011, h111, u);
    let y0 = mix(x00, x10, v);
    let y1 = mix(x01, x11, v);
    return mix(y0, y1, w);
}

@compute @workgroup_size(4, 4, 4)
fn cs_coarse_noise(@builtin(global_invocation_id) gid: vec3<u32>) {
    let sx = (params.size_x / params.stride) + 1u;
    let sy = (params.size_y / params.stride) + 1u;
    let sz = (params.size_z / params.stride) + 1u;
    if gid.x >= sx || gid.y >= sy || gid.z >= sz {
        return;
    }
    let wx = params.origin_x + i32(gid.x * params.stride);
    let wy = params.origin_y + i32(gid.y * params.stride);
    let wz = params.origin_z + i32(gid.z * params.stride);
    let density = perlin_at(wx, wy, wz, params.seed);
    let cave = perlin_at(wx + 97, wy + 53, wz + 31, params.seed ^ 0xCAFEu);
    let idx = gid.x + gid.y * sx + gid.z * sx * sy;
    coarse[idx] = density - cave * params.cave_threshold;
}

@compute @workgroup_size(4, 4, 4)
fn cs_trilinear_fill(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.size_x || gid.y >= params.size_y || gid.z >= params.size_z {
        return;
    }
    let sx = (params.size_x / params.stride) + 1u;
    let sy = (params.size_y / params.stride) + 1u;
    let sz = (params.size_z / params.stride) + 1u;
    let stride = f32(params.stride);
    let lx = f32(gid.x) / stride;
    let ly = f32(gid.y) / stride;
    let lz = f32(gid.z) / stride;
    let x0 = u32(floor(lx));
    let y0 = u32(floor(ly));
    let z0 = u32(floor(lz));
    let x1 = min(x0 + 1u, sx - 1u);
    let y1 = min(y0 + 1u, sy - 1u);
    let z1 = min(z0 + 1u, sz - 1u);
    let tx = lx - f32(x0);
    let ty = ly - f32(y0);
    let tz = lz - f32(z0);
    let at = |cx: u32, cy: u32, cz: u32| -> f32 {
        coarse[cx + cy * sx + cz * sx * sy]
    };
    let c000 = at(x0, y0, z0);
    let c100 = at(x1, y0, z0);
    let c010 = at(x0, y1, z0);
    let c110 = at(x1, y1, z0);
    let c001 = at(x0, y0, z1);
    let c101 = at(x1, y0, z1);
    let c011 = at(x0, y1, z1);
    let c111 = at(x1, y1, z1);
    let x00 = mix(c000, c100, tx);
    let x10 = mix(c010, c110, tx);
    let x01 = mix(c001, c101, tx);
    let x11 = mix(c011, c111, tx);
    let y0v = mix(x00, x10, ty);
    let y1v = mix(x01, x11, ty);
    let v = mix(y0v, y1v, tz);
    let did = gid.x + gid.y * params.size_x + gid.z * params.size_x * params.size_y;
    dense[did] = v;
}
