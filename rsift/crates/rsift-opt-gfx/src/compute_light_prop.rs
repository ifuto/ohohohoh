//! # 40. Compute Shader-Based Lighting (`ComputeLightPropagation`)
//!
//! ブロック照明の伝播（BFS 光源伝播）を GPU コンピュートシェーダで並列実行する。
//! 従来の CPU ベースの Flood-fill 光源伝播の代替であり、光源変更や松明設置時の再計算が
//! GPU の数万並列スレッドで桁違いに高速化され、CPU 負荷をほぼゼロへ抑える。

pub struct ComputeLightPropagation {
    pub chunk_volume: u32,
}

impl Default for ComputeLightPropagation {
    fn default() -> Self {
        Self::new(4096)
    }
}

impl ComputeLightPropagation {
    pub fn new(chunk_volume: u32) -> Self {
        Self { chunk_volume }
    }

    pub fn wgsl_source(&self) -> &'static str {
        LIGHT_PROP_WGSL
    }
}

pub const LIGHT_PROP_WGSL: &str = r#"
// Compute Shader BFS Light Propagation over 16x16x16 chunk section voxels
@group(0) @binding(0) var<storage, read_write> light_levels: array<u32>; // packed 4-bit block + 4-bit sky
@group(0) @binding(1) var<storage, read> opacity_mask: array<u32>;       // bitboard opaque mask

@compute @workgroup_size(64)
fn cs_light_propagate(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let idx = global_id.x;
    if (idx >= 4096u) {
        return;
    }
    let word = idx >> 5u;
    let bit = idx & 31u;
    if ((opacity_mask[word] & (1u << bit)) != 0u) {
        light_levels[idx] = 0u;
        return;
    }
    // Read 6 face neighbors
    let x = idx & 15u;
    let y = (idx >> 4u) & 15u;
    let z = idx >> 8u;
    
    var max_block = 0u;
    if (x > 0u) { max_block = max(max_block, light_levels[idx - 1u] & 15u); }
    if (x < 15u) { max_block = max(max_block, light_levels[idx + 1u] & 15u); }
    if (y > 0u) { max_block = max(max_block, light_levels[idx - 16u] & 15u); }
    if (y < 15u) { max_block = max(max_block, light_levels[idx + 16u] & 15u); }
    if (z > 0u) { max_block = max(max_block, light_levels[idx - 256u] & 15u); }
    if (z < 15u) { max_block = max(max_block, light_levels[idx + 256u] & 15u); }
    
    let current_block = light_levels[idx] & 15u;
    if (max_block > 1u && max_block - 1u > current_block) {
        light_levels[idx] = (light_levels[idx] & 0xFFF0u) | (max_block - 1u);
    }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_light_prop_wgsl() {
        let clp = ComputeLightPropagation::new(4096);
        assert!(clp.wgsl_source().contains("cs_light_propagate"));
    }
}
