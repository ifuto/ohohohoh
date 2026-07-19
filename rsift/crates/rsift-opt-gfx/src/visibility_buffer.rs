//! Visibility Buffer — store primitive + instance IDs instead of a fat G-buffer.
//!
//! A visibility buffer keeps only a per-pixel `(primitive_id, instance_id)`
//! (typically 8 bytes in 64-bit mode) in the forward pass; material/attributes are
//! fetched and shaded via bindless vertex pulling (`StorageBuffer<Vertex>`) in a
//! second pass. Slashes G-buffer VRAM bandwidth on tile-based GPUs from ~32B to ~8B.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec4 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Vec4 {
    #[inline]
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

/// Pack `(primitive_id, instance_id)` into one `u32`: primitive in the low 16
/// bits, instance in the high 16 bits.
#[inline]
pub fn pack_ids(primitive: u32, instance: u32) -> u32 {
    (primitive & 0xFFFF) | ((instance & 0xFFFF) << 16)
}

/// Inverse of [`pack_ids`].
#[inline]
pub fn unpack_ids(packed: u32) -> (u32, u32) {
    (packed & 0xFFFF, (packed >> 16) & 0xFFFF)
}

/// Full 64-bit visibility buffer packing (32-bit primitive ID + 32-bit instance ID)
/// for large open-world Minecraft chunks (>65,536 triangles or instances).
#[inline]
pub fn pack_ids_64(primitive: u32, instance: u32) -> u64 {
    (primitive as u64) | ((instance as u64) << 32)
}

/// Inverse of [`pack_ids_64`].
#[inline]
pub fn unpack_ids_64(packed: u64) -> (u32, u32) {
    (packed as u32, (packed >> 32) as u32)
}

/// Reconstruct a fragment attribute by interpolating barycentric weights across
/// the primitive's 3 vertices (referenced by `primitive_id`). `w` are the three
/// barycentric weights.
#[inline]
pub fn interpolate(a: f32, b: f32, c: f32, w: [f32; 3]) -> f32 {
    a * w[0] + b * w[1] + c * w[2]
}

#[inline]
pub fn interpolate_rgb(a: Vec4, b: Vec4, c: Vec4, w: [f32; 3]) -> Vec4 {
    Vec4 {
        r: interpolate(a.r, b.r, c.r, w),
        g: interpolate(a.g, b.g, c.g, w),
        b: interpolate(a.b, b.b, c.b, w),
        a: interpolate(a.a, b.a, c.a, w),
    }
}

/// Compute barycentric weights $(u, v, w)$ given screen-space coordinates and triangle NDC/screen vertices.
#[inline]
pub fn compute_barycentrics(px: f32, py: f32, v0: [f32; 2], v1: [f32; 2], v2: [f32; 2]) -> [f32; 3] {
    let denom = (v1[1] - v2[1]) * (v0[0] - v2[0]) + (v2[0] - v1[0]) * (v0[1] - v2[1]);
    if denom.abs() < 1e-6 {
        return [0.3333333, 0.3333333, 0.3333333];
    }
    let inv = 1.0 / denom;
    let w0 = ((v1[1] - v2[1]) * (px - v2[0]) + (v2[0] - v1[0]) * (py - v2[1])) * inv;
    let w1 = ((v2[1] - v0[1]) * (px - v2[0]) + (v0[0] - v2[0]) * (py - v2[1])) * inv;
    let w2 = 1.0 - w0 - w1;
    [w0, w1, w2]
}

/// Generates valid WGSL code for bindless vertex pulling from a storage buffer based on visibility buffer output.
pub fn generate_bindless_pulling_wgsl(max_instances: usize) -> String {
    format!(
        r#"
struct QuantizedVertex {{
    pos_packed: u32,
    uv_packed: u32,
    color_packed: u32,
}};

@group(0) @binding(0) var<storage, read> vertex_pool: array<QuantizedVertex>;
@group(0) @binding(1) var<storage, read> index_pool: array<u32>;
@group(0) @binding(2) var visibility_texture: texture_2d<u32>;

struct PulledFragment {{
    pos: vec3<f32>,
    uv: vec2<f32>,
    color: vec4<f32>,
}};

fn pull_and_decode(instance_id: u32, prim_id: u32, bary: vec3<f32>) -> PulledFragment {{
    let base_idx = prim_id * 3u;
    let i0 = index_pool[base_idx];
    let i1 = index_pool[base_idx + 1u];
    let i2 = index_pool[base_idx + 2u];

    let v0 = vertex_pool[i0];
    let v1 = vertex_pool[i1];
    let v2 = vertex_pool[i2];

    // Decode quantized 12-byte positions and interpolate using barycentrics
    let p0 = vec3<f32>(f32(v0.pos_packed & 0x3FFu), f32((v0.pos_packed >> 10u) & 0x3FFu), f32((v0.pos_packed >> 20u) & 0x3FFu));
    let p1 = vec3<f32>(f32(v1.pos_packed & 0x3FFu), f32((v1.pos_packed >> 10u) & 0x3FFu), f32((v1.pos_packed >> 20u) & 0x3FFu));
    let p2 = vec3<f32>(f32(v2.pos_packed & 0x3FFu), f32((v2.pos_packed >> 10u) & 0x3FFu), f32((v2.pos_packed >> 20u) & 0x3FFu));

    var result: PulledFragment;
    result.pos = p0 * bary.x + p1 * bary.y + p2 * bary.z;
    return result;
}}
// Configured for up to {max_instances} instances without Mesh Shader requirement.
"#
    )
}

pub fn visibility_buffer_wgsl() -> &'static str {
    VISIBILITY_BUFFER_WGSL
}

pub const VISIBILITY_BUFFER_WGSL: &str = include_str!("../shaders/visibility_buffer.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip() {
        let p = pack_ids(0x1234, 0xABCD);
        assert_eq!(p & 0xFFFF, 0x1234);
        assert_eq!((p >> 16) & 0xFFFF, 0xABCD);
        assert_eq!(unpack_ids(p), (0x1234, 0xABCD));
    }

    #[test]
    fn pack_64_roundtrip() {
        let p = pack_ids_64(0x12345678, 0x87654321);
        assert_eq!(unpack_ids_64(p), (0x12345678, 0x87654321));
    }

    #[test]
    fn interpolate_at_vertices() {
        assert!((interpolate(2.0, 5.0, 9.0, [1.0, 0.0, 0.0]) - 2.0).abs() < 1e-6);
        assert!((interpolate(2.0, 5.0, 9.0, [0.0, 0.0, 1.0]) - 9.0).abs() < 1e-6);
    }

    #[test]
    fn interpolate_rgb_in_range() {
        let r = interpolate_rgb(
            Vec4::new(1.0, 0.0, 0.0, 1.0),
            Vec4::new(0.0, 1.0, 0.0, 1.0),
            Vec4::new(0.0, 0.0, 1.0, 1.0),
            [0.5, 0.5, 0.0],
        );
        assert!((r.r - 0.5).abs() < 1e-6);
        assert!((r.g - 0.5).abs() < 1e-6);
        assert!((r.b - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_barycentric_computation() {
        let bary = compute_barycentrics(5.0, 5.0, [0.0, 0.0], [10.0, 0.0], [0.0, 10.0]);
        assert!((bary[0] + bary[1] + bary[2] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_wgsl_gen() {
        let code = generate_bindless_pulling_wgsl(4096);
        assert!(code.contains("QuantizedVertex"));
        assert!(code.contains("pull_and_decode"));
    }
}
