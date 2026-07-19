//! Parity gate — native compute only when vanilla-equivalent result is guaranteed.
//! Returns `JvmFallback` to defer to original JVM bytecode (zero spec change).

use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, warn};

/// Native handled the call with vanilla-equivalent semantics
pub const PARITY_OK: i32 = 0;
/// Defer to JVM — native cannot guarantee identical behavior
pub const PARITY_JVM_FALLBACK: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParityVerdict {
    NativeOk,
    JvmFallback,
}

impl ParityVerdict {
    pub fn as_code(self) -> i32 {
        match self {
            Self::NativeOk => PARITY_OK,
            Self::JvmFallback => PARITY_JVM_FALLBACK,
        }
    }

    pub fn from_code(code: i32) -> Self {
        if code == PARITY_OK { Self::NativeOk } else { Self::JvmFallback }
    }
}

/// Tracks parity decisions for auditing
#[derive(Debug, Default)]
pub struct ParityAudit {
    pub native_ok: AtomicU64,
    pub jvm_fallback: AtomicU64,
    pub validation_failures: AtomicU64,
}

impl ParityAudit {
    pub fn record(&self, verdict: ParityVerdict) {
        match verdict {
            ParityVerdict::NativeOk => {
                self.native_ok.fetch_add(1, Ordering::Relaxed);
            }
            ParityVerdict::JvmFallback => {
                self.jvm_fallback.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn record_validation_failure(&self) {
        self.validation_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn native_ratio(&self) -> f64 {
        let ok = self.native_ok.load(Ordering::Relaxed) as f64;
        let fb = self.jvm_fallback.load(Ordering::Relaxed) as f64;
        let total = ok + fb;
        if total == 0.0 { 1.0 } else { ok / total }
    }

    pub fn log_summary(&self) {
        debug!(
            "[ParityGate] native_ok={} jvm_fallback={} validation_failures={} ratio={:.1}%",
            self.native_ok.load(Ordering::Relaxed),
            self.jvm_fallback.load(Ordering::Relaxed),
            self.validation_failures.load(Ordering::Relaxed),
            self.native_ratio() * 100.0,
        );
    }
}

/// Gate: only allow native execution when preconditions match vanilla spec
pub struct ParityGate {
    pub strict_mode: bool,
    pub audit: ParityAudit,
}

impl Default for ParityGate {
    fn default() -> Self {
        Self { strict_mode: true, audit: ParityAudit::default() }
    }
}

impl ParityGate {
    pub fn new(strict: bool) -> Self {
        Self { strict_mode: strict, audit: ParityAudit::default() }
    }

    /// Check entity state is valid for native AI step (vanilla invariants)
    pub fn allow_entity_ai(&self, entity_id: i32, health: f32, removed: bool) -> ParityVerdict {
        if !self.strict_mode {
            return ParityVerdict::NativeOk;
        }
        if entity_id <= 0 || removed {
            self.audit.record(ParityVerdict::JvmFallback);
            return ParityVerdict::JvmFallback;
        }
        if health < 0.0 || health.is_nan() {
            self.audit.record_validation_failure();
            self.audit.record(ParityVerdict::JvmFallback);
            return ParityVerdict::JvmFallback;
        }
        self.audit.record(ParityVerdict::NativeOk);
        ParityVerdict::NativeOk
    }

    /// Redstone: only native when strength in vanilla range 0..=15
    pub fn allow_redstone(&self, strength: u8, block_removed: bool) -> ParityVerdict {
        if block_removed || strength > 15 {
            self.audit.record(ParityVerdict::JvmFallback);
            return ParityVerdict::JvmFallback;
        }
        self.audit.record(ParityVerdict::NativeOk);
        ParityVerdict::NativeOk
    }

    /// Physics: only when entity not in invalid state (riding, portal, etc.)
    pub fn allow_physics(&self, flags: u32) -> ParityVerdict {
        const FLAG_NO_GRAVITY: u32 = 1 << 0;
        const FLAG_IN_PORTAL: u32 = 1 << 1;
        const FLAG_PASSENGER: u32 = 1 << 2;
        if flags & (FLAG_IN_PORTAL | FLAG_PASSENGER) != 0 {
            self.audit.record(ParityVerdict::JvmFallback);
            return ParityVerdict::JvmFallback;
        }
        if flags & FLAG_NO_GRAVITY != 0 {
            // Vanilla still runs travel but skips gravity — native handles this branch
        }
        self.audit.record(ParityVerdict::NativeOk);
        ParityVerdict::NativeOk
    }

    /// Chunk tick: vanilla random tick only when chunk is loaded and tickable
    pub fn allow_chunk_tick(&self, loaded: bool, in_spawn: bool) -> ParityVerdict {
        if !loaded || in_spawn {
            self.audit.record(ParityVerdict::JvmFallback);
            return ParityVerdict::JvmFallback;
        }
        self.audit.record(ParityVerdict::NativeOk);
        ParityVerdict::NativeOk
    }

    pub fn force_fallback(&self, reason: &str) -> ParityVerdict {
        warn!("[ParityGate] JVM fallback: {}", reason);
        self.audit.record(ParityVerdict::JvmFallback);
        ParityVerdict::JvmFallback
    }
}
