//! LWJGL native endpoint proxy — blocks vanilla GL when DX12 is active.

use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
