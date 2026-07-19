//! Morton Order (Z-curve) - 隣接アクセス局所性向上
//!
//! BMI2 PDEP / PEXT 命令のランタイム動的検出 (`std::is_x86_feature_detected!`) と
//! ポータブル SWAR ビット拡張/収縮を用いた 2D/3D Morton エンコード・デコードおよび
//! Z-Order 空間グリッド・高速ソートの完全実装。

#[inline(always)]
pub fn split_by_3(mut a: u32) -> u64 {
    // 10bit -> 30bitへ3bit毎に拡張
    let mut x = a as u64 & 0x3FF;
    x = (x | x << 16) & 0x030000FF;
    x = (x | x << 8) & 0x0300F00F;
    x = (x | x << 4) & 0x030C30C3;
    x = (x | x << 2) & 0x09249249;
    x
}

#[inline(always)]
pub fn compact_by_3(mut x: u64) -> u32 {
    x &= 0x09249249;
    x = (x | (x >> 2)) & 0x030C30C3;
    x = (x | (x >> 4)) & 0x0300F00F;
    x = (x | (x >> 8)) & 0x030000FF;
    x = (x | (x >> 16)) & 0x3FF;
    x as u32
}

#[inline(always)]
pub fn split_by_2(mut a: u32) -> u64 {
    let mut x = a as u64 & 0xFFFFFFFF;
    x = (x | (x << 16)) & 0x0000FFFF0000FFFF;
    x = (x | (x << 8))  & 0x00FF00FF00FF00FF;
    x = (x | (x << 4))  & 0x0F0F0F0F0F0F0F0F;
    x = (x | (x << 2))  & 0x3333333333333333;
    x = (x | (x << 1))  & 0x5555555555555555;
    x
}

#[inline(always)]
pub fn compact_by_2(mut x: u64) -> u32 {
    x &= 0x5555555555555555;
    x = (x | (x >> 1))  & 0x3333333333333333;
    x = (x | (x >> 2))  & 0x0F0F0F0F0F0F0F0F;
    x = (x | (x >> 4))  & 0x00FF00FF00FF00FF;
    x = (x | (x >> 8))  & 0x0000FFFF0000FFFF;
    x = (x | (x >> 16)) & 0xFFFFFFFF;
    x as u32
}

#[inline(always)]
pub fn morton_encode_3d(x: u32, y: u32, z: u32) -> u64 {
    split_by_3(x) | (split_by_3(y) << 1) | (split_by_3(z) << 2)
}

#[inline(always)]
pub fn morton_decode_3d(m: u64) -> [u32; 3] {
    [
        compact_by_3(m),
        compact_by_3(m >> 1),
        compact_by_3(m >> 2),
    ]
}

#[inline(always)]
pub fn morton_encode_2d(x: u32, y: u32) -> u64 {
    split_by_2(x) | (split_by_2(y) << 1)
}

#[inline(always)]
pub fn morton_decode_2d(m: u64) -> [u32; 2] {
    [
        compact_by_2(m),
        compact_by_2(m >> 1),
    ]
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "bmi2")]
unsafe fn morton_encode_3d_bmi2_impl(x: u32, y: u32, z: u32) -> u64 {
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::_pdep_u64;
    #[cfg(target_arch = "x86")]
    use std::arch::x86::_pdep_u32;

    #[cfg(target_arch = "x86_64")]
    {
        let mx = _pdep_u64(x as u64, 0x09249249);
        let my = _pdep_u64(y as u64, 0x09249249 << 1);
        let mz = _pdep_u64(z as u64, 0x09249249 << 2);
        mx | my | mz
    }
    #[cfg(target_arch = "x86")]
    {
        let mx = _pdep_u32(x, 0x09249249) as u64;
        let my = _pdep_u32(y, 0x09249249 << 1) as u64;
        let mz = _pdep_u32(z, 0x09249249 << 2) as u64;
        mx | my | mz
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "bmi2")]
unsafe fn morton_decode_3d_bmi2_impl(m: u64) -> [u32; 3] {
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::_pext_u64;
    #[cfg(target_arch = "x86")]
    use std::arch::x86::_pext_u32;

    #[cfg(target_arch = "x86_64")]
    {
        [
            _pext_u64(m, 0x09249249) as u32,
            _pext_u64(m, 0x09249249 << 1) as u32,
            _pext_u64(m, 0x09249249 << 2) as u32,
        ]
    }
    #[cfg(target_arch = "x86")]
    {
        [
            _pext_u32(m as u32, 0x09249249),
            _pext_u32(m as u32, 0x09249249 << 1),
            _pext_u32(m as u32, 0x09249249 << 2),
        ]
    }
}

/// Dynamic dispatching Morton 3D encode (uses PDEP when hardware supports BMI2).
pub fn morton_encode_3d_fast(x: u32, y: u32, z: u32) -> u64 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("bmi2") {
            return unsafe { morton_encode_3d_bmi2_impl(x, y, z) };
        }
    }
    morton_encode_3d(x, y, z)
}

/// Dynamic dispatching Morton 3D decode (uses PEXT when hardware supports BMI2).
pub fn morton_decode_3d_fast(m: u64) -> [u32; 3] {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("bmi2") {
            return unsafe { morton_decode_3d_bmi2_impl(m) };
        }
    }
    morton_decode_3d(m)
}

#[cfg(target_arch = "x86_64")]
#[inline]
pub fn morton_encode_bmi2(x: u32, y: u32, z: u32) -> u64 {
    morton_encode_3d_fast(x, y, z)
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn morton_encode_bmi2(x: u32, y: u32, z: u32) -> u64 {
    morton_encode_3d(x, y, z)
}

/// Compatibility tag struct for wiring and compile verification.
pub struct MortonOrderTest;

/// Linear Z-order storage grid for cache-friendly chunk block lookups.
pub struct MortonGrid3D<T: Copy + Default, const SIZE: usize> {
    data: Vec<T>,
}

impl<T: Copy + Default, const SIZE: usize> MortonGrid3D<T, SIZE> {
    pub fn new() -> Self {
        assert!(SIZE <= 1024, "Grid dimension too large for 30-bit Morton limit");
        assert!(SIZE.is_power_of_two(), "SIZE must be a power of two");
        let total = SIZE * SIZE * SIZE;
        Self {
            data: vec![T::default(); total],
        }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> Option<&T> {
        if x >= SIZE || y >= SIZE || z >= SIZE {
            return None;
        }
        let m = morton_encode_3d_fast(x as u32, y as u32, z as u32) as usize;
        self.data.get(m)
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: usize, value: T) -> bool {
        if x >= SIZE || y >= SIZE || z >= SIZE {
            return false;
        }
        let m = morton_encode_3d_fast(x as u32, y as u32, z as u32) as usize;
        if let Some(slot) = self.data.get_mut(m) {
            *slot = value;
            true
        } else {
            false
        }
    }

    pub fn as_slice(&self) -> &[T] {
        &self.data
    }
}

pub fn morton_sort_indices(positions: &[[i32; 3]]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..positions.len()).collect();
    idx.sort_by_key(|&i| {
        let p = positions[i];
        morton_encode_3d_fast(p[0] as u32 & 1023, p[1] as u32 & 1023, p[2] as u32 & 1023)
    });
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_3d() {
        for x in [0, 1, 15, 31, 63, 1023] {
            for y in [0, 3, 16, 100] {
                for z in [0, 7, 55, 999] {
                    let m = morton_encode_3d_fast(x, y, z);
                    let [dx, dy, dz] = morton_decode_3d_fast(m);
                    assert_eq!([x, y, z], [dx, dy, dz], "Failed 3D roundtrip at {}, {}, {}", x, y, z);
                }
            }
        }
    }

    #[test]
    fn test_encode_decode_2d() {
        for x in [0, 1, 100, 3000, 65535] {
            for y in [0, 5, 200, 12345] {
                let m = morton_encode_2d(x, y);
                let [dx, dy] = morton_decode_2d(m);
                assert_eq!([x, y], [dx, dy]);
            }
        }
    }

    #[test]
    fn test_morton_grid() {
        let mut grid = MortonGrid3D::<u32, 16>::new();
        grid.set(3, 5, 12, 999);
        assert_eq!(grid.get(3, 5, 12), Some(&999));
        assert_eq!(grid.get(0, 0, 0), Some(&0));
    }
}
