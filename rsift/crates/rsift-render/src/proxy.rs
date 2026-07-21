//! LWJGL native endpoint proxy — blocks vanilla GL when DX12 is active.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use tracing::{debug, info};

pub static PROXIED_DRAW_CALLS: AtomicU64 = AtomicU64::new(0);
pub static PROXIED_SWAP_BUFFERS: AtomicU64 = AtomicU64::new(0);
pub static BLOCKED_GL_DRAWS: AtomicU64 = AtomicU64::new(0);

static PROXY: OnceLock<Mutex<LwjglProxy>> = OnceLock::new();
static DX12_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn global_proxy() -> std::sync::MutexGuard<'static, LwjglProxy> {
    PROXY
        .get_or_init(|| Mutex::new(LwjglProxy::new()))
        .lock()
        .expect("lwjgl proxy lock")
}

/// LWJGL プロキシコントローラー
pub struct LwjglProxy {
    pub is_enabled: bool,
    pub intercept_draw_calls: bool,
}

impl LwjglProxy {
    pub fn new() -> Self {
        Self {
            is_enabled: false,
            intercept_draw_calls: true,
        }
    }

    pub fn enable(&mut self) {
        info!("[LWJGL Proxy] DX12 Agility active — blocking vanilla OpenGL draws");
        self.is_enabled = true;
        DX12_ACTIVE.store(true, Ordering::Relaxed);
    }

    pub fn disable(&mut self) {
        self.is_enabled = false;
        DX12_ACTIVE.store(false, Ordering::Relaxed);
    }

    #[inline]
    pub fn on_gl_draw(&self) -> bool {
        if !self.is_enabled {
            return true;
        }
        PROXIED_DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
        if self.intercept_draw_calls {
            BLOCKED_GL_DRAWS.fetch_add(1, Ordering::Relaxed);
            false
        } else {
            true
        }
    }

    pub fn on_swap_buffers(&self) {
        if !self.is_enabled {
            return;
        }
        PROXIED_SWAP_BUFFERS.fetch_add(1, Ordering::Relaxed);
        debug!("[LWJGL Proxy] swap intercepted — DXGI owns present");
    }
}

#[inline]
pub fn dx12_active() -> bool {
    DX12_ACTIVE.load(Ordering::Relaxed)
}

/// Called from JNI when vanilla issues an OpenGL draw (hooked via RsiftRenderHooks).
#[no_mangle]
pub extern "C" fn rsift_gl_should_draw() -> bool {
    global_proxy().on_gl_draw() as u8 != 0
}

#[no_mangle]
pub extern "C" fn rsift_gl_on_swap_buffers() {
    global_proxy().on_swap_buffers();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 静的カウンタ共有のため、状態遷移は 1 テスト内で直列に検証する
    /// (並列テスト同士のインタリーブで delta が潰れるのを防ぐ設計)。
    #[test]
    fn proxy_state_machine_and_counter_deltas() {
        let mut p = LwjglProxy::new();
        assert!(!p.is_enabled && p.intercept_draw_calls);
        assert!(!dx12_active());

        // disabled: GL 通過・カウンタ非増分。
        let (d0, s0, b0) = (
            PROXIED_DRAW_CALLS.load(Ordering::Relaxed),
            PROXIED_SWAP_BUFFERS.load(Ordering::Relaxed),
            BLOCKED_GL_DRAWS.load(Ordering::Relaxed),
        );
        assert!(p.on_gl_draw(), "disabled では vanilla GL を通す");
        p.on_swap_buffers();
        assert_eq!(
            PROXIED_DRAW_CALLS.load(Ordering::Relaxed) - d0,
            0,
            "disabled では計測しない"
        );
        assert_eq!(PROXIED_SWAP_BUFFERS.load(Ordering::Relaxed) - s0, 0);

        // enable + intercept: GL 遮断・両カウンタ増分・dx12_active。
        p.enable();
        assert!(p.is_enabled && dx12_active());
        assert!(!p.on_gl_draw(), "DX12 稼働中は vanilla GL draw を遮断");
        p.on_swap_buffers();
        assert_eq!(PROXIED_DRAW_CALLS.load(Ordering::Relaxed) - d0, 1);
        assert_eq!(BLOCKED_GL_DRAWS.load(Ordering::Relaxed) - b0, 1);
        assert_eq!(PROXIED_SWAP_BUFFERS.load(Ordering::Relaxed) - s0, 1);

        // intercept 解除: 通過するが proxied 計測は増える (block は増えない)。
        p.intercept_draw_calls = false;
        let b1 = BLOCKED_GL_DRAWS.load(Ordering::Relaxed);
        assert!(p.on_gl_draw());
        assert_eq!(PROXIED_DRAW_CALLS.load(Ordering::Relaxed) - d0, 2);
        assert_eq!(BLOCKED_GL_DRAWS.load(Ordering::Relaxed) - b1, 0);

        // disable: 完全 vanilla 通過復帰。
        p.disable();
        assert!(!p.is_enabled && !dx12_active());
        let d2 = PROXIED_DRAW_CALLS.load(Ordering::Relaxed);
        assert!(p.on_gl_draw());
        assert_eq!(PROXIED_DRAW_CALLS.load(Ordering::Relaxed) - d2, 0);
    }

    #[test]
    fn extern_entrypoints_reflect_global_state() {
        // C ABI エントリポイントがグローバルプロキシに正しく委譲する
        // (States: グローバルはテストプロセスで共有→直列内で遷移させる)。
        {
            let mut g = global_proxy();
            g.disable();
        }
        assert!(rsift_gl_should_draw());
        {
            let mut g = global_proxy();
            g.enable();
        }
        assert!(!rsift_gl_should_draw());
        rsift_gl_on_swap_buffers(); // panic しないこと
        {
            let mut g = global_proxy();
            g.disable(); // 他テスト・後段への影響を残さない
        }
    }
}
