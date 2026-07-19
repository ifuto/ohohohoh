//! # CPU Overhead Reduction & Cache Optimization Engine (`cpu_saver`)
//!
//! 1) 分岐予測ミスを完全撲滅するブランチレス・ビットマスク選択 (`cmov` / `branchless_select_*`)
//! 2) ブランチレス DDA ボクセル歩進 (`BranchlessVoxelStepper`): 条件分岐 `if` なしの空間走査
//! 3) 4x4x4 L1 キャッシュライン・ループタイリング (`LoopTiledVoxelScanner`): 64 バイト境界最適化
//! 4) ソフトウェア・プリフェッチ (`CacheLinePrefetcher`): メモリレイテンシ隠蔽

use bytemuck::{Pod, Zeroable};

/// Branchless conditional select (`cond ? true_val : false_val`).
/// Generates bitwise CMOV/CSEL without pipeline flushes from branch mispredictions.
#[inline(always)]
pub const fn branchless_select_u32(cond: bool, true_val: u32, false_val: u32) -> u32 {
    // cond == true => mask = !0u32 (0xFFFFFFFF), cond == false => mask = 0u32
    let mask = ((cond as i32).wrapping_neg()) as u32;
    (true_val & mask) | (false_val & !mask)
}

#[inline(always)]
pub const fn branchless_select_i32(cond: bool, true_val: i32, false_val: i32) -> i32 {
    let mask = (cond as i32).wrapping_neg();
    (true_val & mask) | (false_val & !mask)
}

#[inline(always)]
pub fn branchless_select_f32(cond: bool, true_val: f32, false_val: f32) -> f32 {
    let t_bits = true_val.to_bits();
    let f_bits = false_val.to_bits();
    let res_bits = branchless_select_u32(cond, t_bits, f_bits);
    f32::from_bits(res_bits)
}

/// Branchless DDA Voxel Stepper — computes ray progression steps without `if/else` branches.
pub struct BranchlessVoxelStepper;

impl BranchlessVoxelStepper {
    #[inline(always)]
    pub fn step_direction(dir: f32) -> i32 {
        let is_pos = dir > 0.0;
        let is_neg = dir < 0.0;
        branchless_select_i32(is_pos, 1, branchless_select_i32(is_neg, -1, 0))
    }

    #[inline(always)]
    pub fn advance_axis(tmx: f32, tmy: f32, tmz: f32) -> (bool, bool, bool) {
        let step_x = tmx <= tmy && tmx <= tmz;
        let step_y = !step_x && tmy <= tmz;
        let step_z = !step_x && !step_y;
        (step_x, step_y, step_z)
    }
}

/// Software Cache Line Prefetcher (`_mm_prefetch`).
pub struct CacheLinePrefetcher;

impl CacheLinePrefetcher {
    #[inline(always)]
    pub fn prefetch_read<T>(ptr: *const T) {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        unsafe {
            #[cfg(target_arch = "x86_64")]
            use std::arch::x86_64::_mm_prefetch;
            #[cfg(target_arch = "x86")]
            use std::arch::x86::_mm_prefetch;
            #[cfg(target_arch = "x86_64")]
            use std::arch::x86_64::_MM_HINT_T0;
            #[cfg(target_arch = "x86")]
            use std::arch::x86::_MM_HINT_T0;

            _mm_prefetch::<_MM_HINT_T0>(ptr as *const i8);
        }
        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
        {
            let _ = ptr;
        }
    }
}

/// L1 Cache-Tiled 16x16x16 Section Scanner (`4x4x4` blocks = exactly 64 voxels per tile).
pub struct LoopTiledVoxelScanner;

impl LoopTiledVoxelScanner {
    /// Execute callback over all 4,096 voxels in L1-resident 4x4x4 block tiles.
    #[inline(always)]
    pub fn scan_tiled<F>(mut callback: F)
    where
        F: FnMut(usize, usize, usize),
    {
        // Outer tile loop: 4x4x4 tiles (each tile is 4x4x4 voxels)
        for ty in (0..16).step_by(4) {
            for tz in (0..16).step_by(4) {
                for tx in (0..16).step_by(4) {
                    // Inner L1-resident tile loop: exactly 64 contiguous/near voxels
                    for dy in 0..4 {
                        let y = ty + dy;
                        for dz in 0..4 {
                            let z = tz + dz;
                            for dx in 0..4 {
                                let x = tx + dx;
                                callback(x, y, z);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_branchless_select() {
        assert_eq!(branchless_select_u32(true, 123, 456), 123);
        assert_eq!(branchless_select_u32(false, 123, 456), 456);
        assert_eq!(BranchlessVoxelStepper::step_direction(15.0), 1);
        assert_eq!(BranchlessVoxelStepper::step_direction(-3.0), -1);
        assert_eq!(BranchlessVoxelStepper::step_direction(0.0), 0);
    }

    #[test]
    fn test_loop_tiling_completeness() {
        let mut count = 0;
        LoopTiledVoxelScanner::scan_tiled(|_, _, _| count += 1);
        assert_eq!(count, 4096, "Tiled scan must visit all 4096 section voxels");
    }
}
