//! Global Allocator置換 - mimalloc設定
//! rsift-opt-gfxは中サイズ頻繁確保でmimallocが2倍高速を意図、ここで設定を提供

#[derive(Debug, Clone)]
pub struct AllocConfig {
    /// アリーナ予約サイズ (MiB)。mimalloc オプション `arena_reserve` (KiB 単位)
    /// に写像可能な唯一の推奨値 (wave 170 FN 捕捉 107/108: 旧 use_mimalloc は
    /// Cargo.lock/Toml に mimalloc が存在しない truth に対し恒 true の虚構、
    /// 旧 large_object_threshold は mimalloc env 語彙に対応物が存在しない
    /// 未配線値のため両者を不可能証明削除)。
    pub arena_size_mb: usize,
}

impl Default for AllocConfig {
    fn default() -> Self {
        Self { arena_size_mb: 64 }
    }
}

impl AllocConfig {
    pub fn for_tier(tier: rsift_api::PerformanceTier) -> Self {
        match tier {
            rsift_api::PerformanceTier::Minimal => Self { arena_size_mb: 16 },
            rsift_api::PerformanceTier::Low => Self { arena_size_mb: 32 },
            _ => Self::default(),
        }
    }

    /// mimalloc 公式 env 語彙の推奨設定文字列 (truth: README のオプション
    /// `arena_reserve` = `MIMALLOC_ARENA_RESERVE`, 単位 KiB)。旧形式
    /// "MIMALLOC_ARENA=64MB_LARGE=1048576" は変数名非存在・単位接尾非
    /// truth・1 文字列 2 設定の無効構造の架空語彙だったため根治。
    /// mimalloc は現状未リンク (Cargo.lock/Toml 皆無) であり、導入時に
    /// この文字列をそのまま環境変数として使用可能にするための推奨表記。
    pub fn env_string(&self) -> String {
        format!("MIMALLOC_ARENA_RESERVE={}", self.arena_size_mb * 1024)
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
        assert_eq!(m.arena_size_mb, 16);
        let l = AllocConfig::for_tier(PerformanceTier::Low);
        assert_eq!(l.arena_size_mb, 32);
        for t in [PerformanceTier::Medium, PerformanceTier::High] {
            let c = AllocConfig::for_tier(t);
            assert_eq!(c.arena_size_mb, 64);
        }
    }

    /// 【wave 170 FN 捕捉 107】env_string は mimalloc 公式 env 語彙
    /// (README truth: オプション arena_reserve = MIMALLOC_ARENA_RESERVE, 単位 KiB)
    /// に一致すること。旧 "MIMALLOC_ARENA=64MB_LARGE=1048576" は mimalloc が
    /// 決して読まない架空語彙 (変数名非存在・単位接尾非 truth・1文字列2設定の
    /// 無効構造)。
    #[test]
    fn fn_env_string_truth_vocabulary() {
        assert_eq!(
            AllocConfig::default().env_string(),
            "MIMALLOC_ARENA_RESERVE=65536"
        );
    }

    /// 【wave 170 FN 捕捉 107/108】tier 選択は arena_size_mb (KiB truth に写像
    /// 可能な唯一の値) のみが実語彙を持つ。large_object_threshold は mimalloc
    /// env 語彙に対応物がなく (公式オプション一覧に large object threshold
    /// は存在しない)、use_mimalloc は truth (mimalloc 未リンク) と恒 true が
    /// 矛盾するため両者を削除。arena KiB 真値 (rq fn_mimalloc) を pin。
    #[test]
    fn fn_tier_arena_kib_truth() {
        let m = AllocConfig::for_tier(PerformanceTier::Minimal);
        let l = AllocConfig::for_tier(PerformanceTier::Low);
        let d = AllocConfig::default();
        assert_eq!(m.arena_size_mb * 1024, 16384);
        assert_eq!(l.arena_size_mb * 1024, 32768);
        assert_eq!(d.arena_size_mb * 1024, 65536);
        assert_eq!(m.env_string(), "MIMALLOC_ARENA_RESERVE=16384");
        assert_eq!(l.env_string(), "MIMALLOC_ARENA_RESERVE=32768");
    }

    #[test]
    fn env_string_format_and_recommendation() {
        let c = AllocConfig::default();
        assert_eq!(c.env_string(), "MIMALLOC_ARENA_RESERVE=65536");
        assert_eq!(recommended_allocator(), "mimalloc");
    }
}
