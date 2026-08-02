//! Bootstrap log — always works without tracing subscriber (safe inside Agent_OnLoad).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static LOG_SEQ: AtomicU64 = AtomicU64::new(0);
/// セッション相対時計の基準点 (初回ログ呼出時に確定。t+ は session start からの ms)。
static LOG_START: AtomicU64 = AtomicU64::new(0);

fn log_path() -> PathBuf {
    if let Some(dir) = crate::agent_opts::dll_directory() {
        return dir.join("rsift-bootstrap.log");
    }
    std::env::var("APPDATA")
        .ok()
        .map(|a| PathBuf::from(a).join(".minecraft").join("rsift-bootstrap.log"))
        .unwrap_or_else(|| PathBuf::from("rsift-bootstrap.log"))
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// wave 206 診断品質: t+ は従来 (epoch_ms % 1_000_000) の「16.6 分周期の
/// 下6桁」で時差が読めなかった。セッション相対 (初回ログ=0) に変更。
fn session_relative_ms() -> u128 {
    let now = timestamp_ms() as u64;
    loop {
        let cur = LOG_START.load(Ordering::Relaxed);
        if cur != 0 {
            return now.saturating_sub(cur) as u128;
        }
        if LOG_START
            .compare_exchange(0, now.max(1), Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return 0;
        }
    }
}

fn thread_label() -> String {
    format!("{:?}", std::thread::current().id())
}

pub fn agent_log(line: &str) {
    let seq = LOG_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let formatted = format!(
        "[{:04}|t+{}ms|{}] {}",
        seq,
        session_relative_ms(),
        thread_label(),
        line
    );
    eprintln!("{}", formatted);
    let path = log_path();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", formatted);
        let _ = f.flush();
    }
}

/// Structured step marker — grep for `[step]` in rsift-bootstrap.log.
pub fn agent_log_step(phase: &str, detail: &str) {
    agent_log(&format!("[step] {} — {}", phase, detail));
}

pub fn agent_log_warn(phase: &str, detail: &str) {
    agent_log(&format!("[warn] {} — {}", phase, detail));
}

pub fn agent_log_err(phase: &str, detail: &str) {
    agent_log(&format!("[err] {} — {}", phase, detail));
}

/// Call once per JVM start so users can tell fresh launches from accumulated history.
pub fn agent_log_session_start(tag: &str) {
    let path = log_path();
    let _ = std::fs::write(&path, "");
    agent_log("==========================================================================");
    agent_log(&format!("[Rsift] session start: {}", tag));
    agent_log(&format!("[Rsift] log file: {:?}", path));
    if let Some(dir) = crate::agent_opts::dll_directory() {
        agent_log(&format!("[Rsift] dll directory: {:?}", dir));
        let jar = dir.join("rsift-bootstrap.jar");
        let dll = dir.join("rsift_jvm.dll");
        agent_log(&format!(
            "[Rsift] bootstrap jar: exists={} size={:?}",
            jar.is_file(),
            jar.metadata().ok().map(|m| m.len())
        ));
        agent_log(&format!(
            "[Rsift] rsift_jvm.dll: exists={} size={:?}",
            dll.is_file(),
            dll.metadata().ok().map(|m| m.len())
        ));
    }
    agent_log("==========================================================================");
}
