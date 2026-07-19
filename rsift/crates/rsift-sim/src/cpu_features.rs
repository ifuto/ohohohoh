//! Phase 1/17 — CPU feature detect + scalar/SSE/AVX2/NEON path selection.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdLevel {
    Scalar,
    Sse2,
    Avx2,
    Neon,
}

#[derive(Debug, Clone, Copy)]
pub struct CpuFeatures {
    pub simd: SimdLevel,
    pub cores: usize,
}

impl CpuFeatures {
    pub fn detect() -> Self {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let simd = detect_simd();
        Self { simd, cores }
    }
}

fn detect_simd() -> SimdLevel {
    #[cfg(target_arch = "aarch64")]
    {
        return SimdLevel::Neon;
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("avx2") {
            return SimdLevel::Avx2;
        }
        if is_x86_feature_detected!("sse2") {
            return SimdLevel::Sse2;
        }
        return SimdLevel::Scalar;
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86",
        target_arch = "x86_64"
    )))]
    {
        SimdLevel::Scalar
    }
}

/// Popcount with best available path (see `simd_kernels`).
pub fn popcnt_bytes(data: &[u8]) -> u64 {
    crate::simd_kernels::popcnt_bytes(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_something() {
        let f = CpuFeatures::detect();
        assert!(f.cores >= 1);
        assert_eq!(popcnt_bytes(&[0xff, 0x0f]), 12);
    }
}
