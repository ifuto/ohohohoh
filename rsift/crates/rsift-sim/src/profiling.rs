//! Phase 1 — tracing spans + optional Tracy frame marks + memory samples.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::{debug, info_span};

static TICK_NS: AtomicU64 = AtomicU64::new(0);
static CHUNK_GEN_NS: AtomicU64 = AtomicU64::new(0);
static LIGHT_NS: AtomicU64 = AtomicU64::new(0);
static IO_NS: AtomicU64 = AtomicU64::new(0);
static FRAME_NS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileDomain {
    Tick,
    ChunkGen,
    Light,
    Io,
    Frame,
    Mod(u32),
}

pub struct ScopeGuard {
    domain: ProfileDomain,
    start: Instant,
    #[cfg(feature = "tracy")]
    _tracy: Option<tracy_client::Span>,
}

impl ScopeGuard {
    pub fn enter(domain: ProfileDomain) -> Self {
        #[cfg(feature = "tracy")]
        let _tracy = {
            let name = match domain {
                ProfileDomain::Tick => "Tick",
                ProfileDomain::ChunkGen => "ChunkGen",
                ProfileDomain::Light => "Light",
                ProfileDomain::Io => "Io",
                ProfileDomain::Frame => "Frame",
                ProfileDomain::Mod(_) => "Mod",
            };
            Some(tracy_client::span!(name, 0))
        };
        Self {
            domain,
            start: Instant::now(),
            #[cfg(feature = "tracy")]
            _tracy,
        }
    }
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        let ns = self.start.elapsed().as_nanos() as u64;
        let slot = match self.domain {
            ProfileDomain::Tick => &TICK_NS,
            ProfileDomain::ChunkGen => &CHUNK_GEN_NS,
            ProfileDomain::Light => &LIGHT_NS,
            ProfileDomain::Io => &IO_NS,
            ProfileDomain::Frame | ProfileDomain::Mod(_) => &FRAME_NS,
        };
        slot.fetch_add(ns, Ordering::Relaxed);
    }
}

#[macro_export]
macro_rules! profile_scope {
    ($domain:expr) => {
        let _rsift_prof = $crate::profiling::ScopeGuard::enter($domain);
        let _rsift_span = tracing::info_span!(
            "rsift",
            domain = ?$domain
        )
        .entered();
    };
}

#[derive(Debug, Clone, Default)]
pub struct ProfileSnapshot {
    pub tick_ms: f64,
    pub chunk_gen_ms: f64,
    pub light_ms: f64,
    pub io_ms: f64,
    pub frame_ms: f64,
    pub rss_bytes: u64,
}

fn ns_to_ms(ns: u64) -> f64 {
    ns as f64 / 1_000_000.0
}

pub fn snapshot() -> ProfileSnapshot {
    ProfileSnapshot {
        tick_ms: ns_to_ms(TICK_NS.swap(0, Ordering::Relaxed)),
        chunk_gen_ms: ns_to_ms(CHUNK_GEN_NS.swap(0, Ordering::Relaxed)),
        light_ms: ns_to_ms(LIGHT_NS.swap(0, Ordering::Relaxed)),
        io_ms: ns_to_ms(IO_NS.swap(0, Ordering::Relaxed)),
        frame_ms: ns_to_ms(FRAME_NS.swap(0, Ordering::Relaxed)),
        rss_bytes: estimate_rss_bytes(),
    }
}

pub fn estimate_rss_bytes() -> u64 {
    #[cfg(windows)]
    {
        // Working set via GetProcessMemoryInfo would need winapi; approximate via peak.
        // Portable fallback: report 0 when unavailable — Tracy/OS tools remain authoritative.
        0
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/self/statm") {
            let pages: u64 = s
                .split_whitespace()
                .nth(1)
                .and_then(|x| x.parse().ok())
                .unwrap_or(0);
            return pages * 4096;
        }
        0
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        0
    }
}

pub fn frame_mark() {
    #[cfg(feature = "tracy")]
    {
        tracy_client::frame_mark();
    }
    debug!(target: "rsift::profile", "frame_mark");
}

pub fn log_snapshot(s: &ProfileSnapshot) {
    let _span = info_span!("profile_snapshot").entered();
    debug!(
        tick_ms = s.tick_ms,
        chunk_ms = s.chunk_gen_ms,
        light_ms = s.light_ms,
        io_ms = s.io_ms,
        frame_ms = s.frame_ms,
        rss = s.rss_bytes,
        "profile"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_accumulates() {
        {
            let _g = ScopeGuard::enter(ProfileDomain::Tick);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let s = snapshot();
        assert!(s.tick_ms >= 0.5);
    }
}
