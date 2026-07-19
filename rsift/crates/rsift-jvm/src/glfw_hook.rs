//! GLFW `glfwSwapBuffers` hook — skip OpenGL present when DX12 owns the swap chain.
//!
//! 監査指摘の解消: 旧実装は `ORIGINAL = パッチ済みアドレス` を保持しており、
//! オリジナル呼び出しが **hooked_swap への無限再帰** (スタックオーバーフロー) に
//! なる実バグを抱えていた。本実装は汎用 `detour_hook::DetourHook` (実パッチ +
//! unpatch→call→repatch) でオリジナル呼出を安全に実行する。。

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::agent_log::agent_log;

static INSTALLED: AtomicBool = AtomicBool::new(false);
static SKIP_GLFW_SWAP: AtomicBool = AtomicBool::new(true);

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::detour_hook::DetourHook;
    use std::sync::Mutex;

    type GlfwSwapFn = unsafe extern "C" fn(*mut c_void);

    /// 実装パッチ本体 (機械語先頭5バイトを保持)。レンダースレッド専用。
    static HOOK: Mutex<Option<DetourHook>> = Mutex::new(None);

    pub fn install() {
        if super::INSTALLED.load(Ordering::SeqCst) {
            return;
        }
        let mut guard = HOOK.lock().unwrap();
        if guard.is_some() {
            super::INSTALLED.store(true, Ordering::SeqCst);
            return;
        }
        for module in ["glfw3.dll", "glfw.dll"] {
            match DetourHook::for_export(module, "glfwSwapBuffers", hooked_swap as usize) {
                Ok(mut h) => match h.enable() {
                    Ok(()) => {
                        agent_log(&format!(
                            "[GlfwHook] {} hooked — DXGI exclusive present",
                            h.target_name
                        ));
                        *guard = Some(h);
                        super::INSTALLED.store(true, Ordering::SeqCst);
                        return;
                    }
                    Err(e) => {
                        agent_log(&format!("[GlfwHook] WARN patch failed: {}", e));
                    }
                },
                Err(_) => continue, // 次のモジュール名で再試行
            }
        }
        agent_log("[GlfwHook] WARN glfwSwapBuffers not found (will retry via engine ensure)");
    }

    /// hooked_swap からのオリジナル呼出 (unpatch→call→repatch)。
    pub fn call_original_swap(window: *mut c_void) {
        let mut guard = HOOK.lock().unwrap();
        let Some(h) = guard.as_mut() else { return };
        unsafe {
            let _ = h.call_original(|addr| {
                let orig: GlfwSwapFn = std::mem::transmute(addr);
                orig(window);
            });
        }
    }

    unsafe extern "C" fn hooked_swap(window: *mut c_void) {
        if super::SKIP_GLFW_SWAP.load(Ordering::Relaxed) && rsift_render::proxy::dx12_active() {
            rsift_render::proxy::rsift_gl_on_swap_buffers();
            return;
        }
        call_original_swap(window);
    }
}

#[cfg(not(windows))]
mod imp {}

pub fn install_glfw_swap_hook() {
    #[cfg(windows)]
    imp::install();
}

pub fn set_skip_glfw_swap(skip: bool) {
    SKIP_GLFW_SWAP.store(skip, Ordering::Relaxed);
}
