
//! Morton Order (Z-curve) - 隣接アクセス局所性向上
//! BMI2 PDEPを使った高速エンコード

#[inline]
pub fn split_by_3(mut a: u32) -> u64 {
    // 10bit -> 30bitへ3bit毎に拡張
    let mut x = a as u64 & 0x3FF;
    x = (x | x << 16) & 0x030000FF;
    x = (x | x << 8) & 0x0300F00F;
    x = (x | x << 4) & 0x030C30C3;
    x = (x | x << 2) & 0x09249249;
    x
}

#[inline]
pub fn morton_encode_3d(x: u32, y: u32, z: u32) -> u64 {
    split_by_3(x) | (split_by_3(y) << 1) | (split_by_3(z) << 2)
}

#[cfg(target_arch = "x86_64")]
#[inline]
pub fn morton_encode_bmi2(x: u32, y: u32, z: u32) -> u64 {
    #[cfg(target_feature = "bmi2")]
    unsafe {
        use std::arch::x86_64::_pdep_u64;
        let mx = _pdep_u64(x as u64, 0x09249249);
        let my = _pdep_u64(y as u64, 0x09249249 << 1);
        let mz = _pdep_u64(z as u64, 0x09249249 << 2);
        mx | my | mz
    }
    #[cfg(not(target_feature = "bmi2"))]
    {
        morton_encode_3d(x, y, z)
    }
}

pub fn morton_sort_indices(positions: &[[i32;3]]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..positions.len()).collect();
    idx.sort_by_key(|&i| {
        let p = positions[i];
        morton_encode_3d(p[0] as u32 & 1023, p[1] as u32 & 1023, p[2] as u32 & 1023)
    });
    idx
}
