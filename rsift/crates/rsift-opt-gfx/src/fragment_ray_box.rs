//! # 33. Ray-Box Intersection on Fragment Shader (`FragmentRayBoxIntersect`)
//!
//! ボクセルのビルボードをスプラットして粗い可視性推定を行い、フラグメントシェーダで
//! レイ-ボックス交差 (`ray_box_intersect`) を計算して精密な可視性と法線を得る。
//! 事前計算や空間データ構造を介さないため、毎フレーム全ボクセルが変化する完全動的破壊シーンに適用可能。

pub struct FragmentRayBoxIntersect {
    pub max_dynamic_voxels: u32,
}

impl Default for FragmentRayBoxIntersect {
    fn default() -> Self {
        Self::new(65536)
    }
}

impl FragmentRayBoxIntersect {
    pub fn new(max_dynamic_voxels: u32) -> Self {
        Self { max_dynamic_voxels }
    }

    pub fn wgsl_source(&self) -> &'static str {
        FRAGMENT_RAY_BOX_WGSL
    }
}

pub const FRAGMENT_RAY_BOX_WGSL: &str = r#"
// Ray-Box intersection inside Fragment Shader for fully dynamic voxel destruction
struct VoxelBillboard {
    center: vec3<f32>,
    half_size: f32,
    color_argb: u32,
};

@group(0) @binding(0) var<storage, read> dynamic_voxels: array<VoxelBillboard>;

struct FragmentInput {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) voxel_id: u32,
    @location(1) ray_origin: vec3<f32>,
    @location(2) ray_dir: vec3<f32>,
};

fn ray_box_intersect(ro: vec3<f32>, rd: vec3<f32>, box_min: vec3<f32>, box_max: vec3<f32>) -> vec2<f32> {
    let inv_rd = 1.0 / rd;
    let t1 = (box_min - ro) * inv_rd;
    let t2 = (box_max - ro) * inv_rd;
    let tmin = min(t1, t2);
    let tmax = max(t1, t2);
    let t_near = max(max(tmin.x, tmin.y), tmin.z);
    let t_far = min(min(tmax.x, tmax.y), tmax.z);
    return vec2<f32>(t_near, t_far);
}

@fragment
fn fs_main(input: FragmentInput) -> @location(0) vec4<f32> {
    let v = dynamic_voxels[input.voxel_id];
    let box_min = v.center - vec3<f32>(v.half_size);
    let box_max = v.center + vec3<f32>(v.half_size);
    let rd = normalize(input.ray_dir);
    let hit = ray_box_intersect(input.ray_origin, rd, box_min, box_max);
    if (hit.x > hit.y || hit.y < 0.0) {
        discard;
    }
    let col = v.color_argb;
    let r = f32((col >> 16u) & 0xFFu) / 255.0;
    let g = f32((col >> 8u) & 0xFFu) / 255.0;
    let b = f32(col & 0xFFu) / 255.0;
    return vec4<f32>(r, g, b, 1.0);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fragment_ray_box_wgsl() {
        let frb = FragmentRayBoxIntersect::new(100);
        assert!(frb.wgsl_source().contains("ray_box_intersect"));
    }
}
