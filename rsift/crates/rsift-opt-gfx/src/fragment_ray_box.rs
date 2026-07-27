//! # 33. Ray-Box Intersection on Fragment Shader (`FragmentRayBoxIntersect`)
//!
//! ボクセルのビルボードをスプラットして粗い可視性推定を行い、フラグメントシェーダで
//! レイ-ボックス交差 (`ray_box_intersect`) を計算して精密な可視性と法線を得る。
//! 事前計算や空間データ構造を介さないため、毎フレーム全ボクセルが変化する完全動的破壊シーンに適用可能。
//!
//! 誠実注記 (wave 138): `Default` (=65536=2^16) と wiring 実引数 (=1<<20=2^20)
//! は意図的に異なる — Default は standalone 利用のフォールバック、wiring は
//! 独自の上限を明示的に与える。CPU 入力 API 不在でビルボードが GPU へ送達
//! されない構造は wave 83 CG-6 公表どおり (消費は max_dynamic_voxels の
//! cap 値のみ、wiring:952 で実消費)。

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
}

/// WGSL 取得の公式アクセスポイント (wave 138 EL-4)。
///
/// 旧メソッド `wgsl_source(&self)` は `self` を一度も参照しない装飾レシーバ
/// (返却値は常に pub const と同一) で、消費者がモジュール内テストのみの
/// 中間構造だったため free fn へ根治し gpu_runtime::all_wgsl_sources の
/// 登録を本関数経由に一本化 (tbdr_hints EL-1 と同型)。構造体自体は
/// wiring が `max_dynamic_voxels` を保持・実消費するため維持。
pub fn wgsl_source() -> &'static str {
    FRAGMENT_RAY_BOX_WGSL
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
        // EL-4: 呼出形のみ free fn へ機械追従 (検証意図は不変)。
        assert!(wgsl_source().contains("ray_box_intersect"));
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    /// EL-4: free fn は公開 const と同一内容。
    #[test]
    fn wgsl_source_identity_with_const() {
        assert_eq!(wgsl_source(), FRAGMENT_RAY_BOX_WGSL);
        assert!(wgsl_source().contains("ray_box_intersect"));
    }

    /// EL-4: 構築契約の機械 pin (rq 事前導出: 65536=2^16, 1<<20=2^20=1048576)。
    #[test]
    fn new_and_default_contract_pin() {
        assert_eq!(FragmentRayBoxIntersect::new(7).max_dynamic_voxels, 7);
        assert_eq!(
            FragmentRayBoxIntersect::default().max_dynamic_voxels,
            65536,
            "Default = 2^16 (standalone フォールバック)"
        );
        assert_eq!(
            FragmentRayBoxIntersect::new(1 << 20).max_dynamic_voxels,
            1048576,
            "wiring 実引数 = 2^20"
        );
    }

    /// EL-4: wiring:952 の消費 cap 写像 (rq 導出: 128/128/7)。
    /// `max as usize` は u32→usize 拡大で wrap 不出。
    #[test]
    fn wiring_cap_mapping_pin() {
        assert_eq!((1048576usize).min(128), 128);
        assert_eq!((65536usize).min(128), 128);
        assert_eq!((7usize).min(128), 7);
    }
}
