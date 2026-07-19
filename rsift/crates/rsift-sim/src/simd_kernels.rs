//! Popcount / byte kernels with AVX2 / SSE2 / NEON / scalar paths.

use crate::cpu_features::{CpuFeatures, SimdLevel};

/// Popcount with best available path.
pub fn popcnt_bytes(data: &[u8]) -> u64 {
    match CpuFeatures::detect().simd {
        SimdLevel::Avx2 => popcnt_avx2_or_scalar(data),
        SimdLevel::Sse2 => popcnt_sse_or_scalar(data),
        SimdLevel::Neon => popcnt_neon_or_scalar(data),
        SimdLevel::Scalar => popcnt_scalar(data),
    }
}

#[inline]
fn popcnt_scalar(data: &[u8]) -> u64 {
    data.iter().map(|b| b.count_ones() as u64).sum()
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn popcnt_avx2_or_scalar(data: &[u8]) -> u64 {
    if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("popcnt") {
        // SAFETY: feature gated above.
        return unsafe { popcnt_avx2(data) };
    }
    popcnt_scalar(data)
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn popcnt_avx2_or_scalar(data: &[u8]) -> u64 {
    popcnt_scalar(data)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn popcnt_sse_or_scalar(data: &[u8]) -> u64 {
    if is_x86_feature_detected!("popcnt") {
        return unsafe { popcnt_with_popcnt_inst(data) };
    }
    popcnt_scalar(data)
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn popcnt_sse_or_scalar(data: &[u8]) -> u64 {
    popcnt_scalar(data)
}

#[cfg(target_arch = "aarch64")]
fn popcnt_neon_or_scalar(data: &[u8]) -> u64 {
    // NEON lacks a direct byte-popcnt; use scalar which llvm often vectorizes.
    popcnt_scalar(data)
}

#[cfg(not(target_arch = "aarch64"))]
fn popcnt_neon_or_scalar(data: &[u8]) -> u64 {
    popcnt_scalar(data)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "popcnt")]
unsafe fn popcnt_with_popcnt_inst(data: &[u8]) -> u64 {
    let mut sum = 0u64;
    let (chunks, rem) = data.as_chunks::<8>();
    for c in chunks {
        let v = u64::from_ne_bytes(*c);
        sum += v.count_ones() as u64;
    }
    sum + rem.iter().map(|b| b.count_ones() as u64).sum::<u64>()
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2", enable = "popcnt")]
unsafe fn popcnt_avx2(data: &[u8]) -> u64 {
    // Process 32-byte lanes via two u128 / four u64 POPCNT — avoids raw AVX2 shuffle tables.
    let mut sum = 0u64;
    let mut i = 0;
    while i + 32 <= data.len() {
        for k in 0..4 {
            let off = i + k * 8;
            let v = u64::from_ne_bytes(data[off..off + 8].try_into().unwrap());
            sum += v.count_ones() as u64;
        }
        i += 32;
    }
    sum + popcnt_scalar(&data[i..])
}
