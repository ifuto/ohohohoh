
//! Global Allocator置換 - mimalloc設定
//! rsift-opt-gfxは中サイズ頻繁確保でmimallocが2倍高速を意図、ここで設定を提供

#[derive(Debug, Clone)]
pub struct AllocConfig {
    pub use_mimalloc: bool,
    pub arena_size_mb: usize,
    pub large_object_threshold: usize,
}

impl Default for AllocConfig {
    fn default() -> Self {
        Self { use_mimalloc: true, arena_size_mb: 64, large_object_threshold: 1024*1024 }
    }
}

impl AllocConfig {
    pub fn for_tier(tier: rsift_api::PerformanceTier) -> Self {
        match tier {
            rsift_api::PerformanceTier::Minimal => Self { use_mimalloc: true, arena_size_mb: 16, large_object_threshold: 256*1024 },
            rsift_api::PerformanceTier::Low => Self { use_mimalloc: true, arena_size_mb: 32, large_object_threshold: 512*1024 },
            _ => Self::default(),
        }
    }

    pub fn env_string(&self) -> String {
        format!("MIMALLOC_ARENA={}MB_LARGE={}", self.arena_size_mb, self.large_object_threshold)
    }
}

/// 実際にグローバルアロケータを切り替えるにはCargo.tomlでmimallocを有効化
/// ここでは設定値を提供し、ベンチで差を計測可能に
pub fn recommended_allocator() -> &'static str {
    "mimalloc"
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsift_api::PerformanceTier;

    #[test]
    fn tier_matrix_exact() {
        let m = AllocConfig::for_tier(PerformanceTier::Minimal);
        assert_eq!((m.arena_size_mb, m.large_object_threshold), (16, 256 * 1024));
        assert!(m.use_mimalloc);
        let l = AllocConfig::for_tier(PerformanceTier::Low);
        assert_eq!((l.arena_size_mb, l.large_object_threshold), (32, 512 * 1024));
        for t in [PerformanceTier::Medium, PerformanceTier::High] {
            let c = AllocConfig::for_tier(t);
            assert_eq!((c.arena_size_mb, c.large_object_threshold), (64, 1024 * 1024));
        }
    }

    #[test]
    fn env_string_format_and_recommendation() {
        let c = AllocConfig::default();
        assert_eq!(c.env_string(), "MIMALLOC_ARENA=64MB_LARGE=1048576");
        assert_eq!(recommended_allocator(), "mimalloc");
    }
}
