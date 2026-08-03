//! wave 219 HP (P1 診断欠陥の根治): api 内部の重要エラーを、利用側クレート
//! (rsift-jvm) が差し込むログシンクへ橋渡しする。
//!
//! 背景 (実機 #5 = `logs/#5/rsift-bootstrap.log` 一次解析): rsift-api は
//! `tracing` の error!/info! で NativeLoader の個別失敗
//! (Dynamic linker error 等) を報告する設計だが、JVMTI エージェントとして
//! ゲームへ注入される実行環境では `tracing` の subscriber が一切初期化
//! されないため全行が静かに蒸発し、bootstrap ログには
//! 「mods loaded OK: []」だけが残った (3 候補があるのに空 = 真の失敗理由
//! 0 行)。sink 未登録環境では従来どおり `tracing` へ送る
//! (動作は変えず可視性のみ引上げる)。

use std::sync::OnceLock;

static SINK: OnceLock<fn(&str)> = OnceLock::new();

/// ホスト (rsift-jvm) がログシンクを登録する。最初の 1 回のみ有効で、
/// 後着の再登録は無視して false を返す (sink の一意性を機械保証)。
pub fn set_agent_log_sink(f: fn(&str)) -> bool {
    SINK.set(f).is_ok()
}

/// 重要行を sink へ。未登録なら従来挙動 (tracing::error!) にフォールバック。
pub fn log_important(line: &str) {
    if let Some(f) = SINK.get() {
        f(line);
    } else {
        tracing::error!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNT: AtomicUsize = AtomicUsize::new(0);

    fn sink(_line: &str) {
        COUNT.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn sink_receives_lines_once_registered_and_rejects_reregister() {
        let first = set_agent_log_sink(sink);
        assert!(first, "初回登録は受理されなければならない");
        let before = COUNT.load(Ordering::SeqCst);
        log_important("hello");
        assert_eq!(COUNT.load(Ordering::SeqCst), before + 1);
        assert!(
            !set_agent_log_sink(sink),
            "再登録は拒否されなければならない"
        );
    }
}
