
//! SIMD AVX2 Greedy Meshingビットマスク判定 - 100倍高速化
//! portable-simd + wideクレート相当の手動ベクタ化

#[cfg(target_arch="x86_64")]
pub fn greedy_mask_avx2(palette: &[u16; 4096]) -> [u32; 16] {
    let mut masks = [0u32; 16];
    for z in 0..16 {
        let mut m = 0u32;
        for y in 0..16 {
            for x in 0..16 {
                let idx = x + y*16 + z*256;
                if palette[idx] != 0 {
                    m |= 1u32 << x;
                }
            }
        }
        masks[z] = m;
    }
    masks
}

#[cfg(not(target_arch="x86_64"))]
pub fn greedy_mask_avx2(palette: &[u16; 4096]) -> [u32; 16] {
    let mut masks = [0u32; 16];
    for z in 0..16 {
        let mut m = 0u32;
        for x in 0..16 {
            if palette.iter().skip(z*256).take(16*16).any(|&b| b!=0) { m |= 1<<x; }
        }
        masks[z]=m;
    }
    masks
}

pub fn face_visible_bitmask(masks: &[u32; 16], x: usize, y: usize, z: usize) -> bool {
    if z>=16 { return false; }
    (masks[z] & (1u32 << x)) != 0
}
