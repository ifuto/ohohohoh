//! GLFW `glfwSwapBuffers` hook — skip OpenGL present when DX12 owns the swap chain.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::agent_log::agent_log;

type GlfwSwapFn = unsafe extern "C" fn(*mut c_void);

static INSTALLED: AtomicBool = AtomicBool::new(false);
static SKIP_GLFW_SWAP: AtomicBool = AtomicBool::new(true);

#[cfg(windows)]
mod imp {
    use super::*;
    use std::sync::OnceLock;
    use windows::core::PCSTR;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    use windows::Win32::System::Memory::{VirtualProtect, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS};

    static ORIGINAL: OnceLock<GlfwSwapFn> = OnceLock::new();

    pub fn install() {
        if super::INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        unsafe {
            let Some(proc) = resolve_glfw_swap() else {
                agent_log("[GlfwHook] WARN glfwSwapBuffers not found (will retry)");
                super::INSTALLED.store(false, Ordering::SeqCst);
                return;
            };
            let target = proc as usize;
            if patch_rel_jmp(target, hooked_swap as usize).is_err() {
                agent_log("[GlfwHook] WARN relative jmp patch failed");
                super::INSTALLED.store(false, Ordering::SeqCst);
                return;
            }
            let _ = ORIGINAL.set(proc);
            agent_log("[GlfwHook] glfwSwapBuffers hooked — DXGI exclusive present");
        }
    }

    unsafe fn resolve_glfw_swap() -> Option<GlfwSwapFn> {
        for name in [b"glfw3.dll\0".as_slice(), b"glfw.dll\0".as_slice()] {
            if let Ok(module) = GetModuleHandleA(PCSTR(name.as_ptr())) {
                if let Some(proc) = GetProcAddress(module, PCSTR(b"glfwSwapBuffers\0".as_ptr())) {
                    return Some(std::mem::transmute(proc));
                }
            }
        }
        None
    }

    unsafe fn patch_rel_jmp(target: usize, detour: usize) -> Result<(), ()> {
        let offset = (detour as isize) - (target as isize) - 5;
        if !(i32::MIN as isize..=i32::MAX as isize).contains(&offset) {
            return Err(());
        }
        let mut old = PAGE_PROTECTION_FLAGS(0);
        VirtualProtect(
            target as *const _,
            5,
            PAGE_EXECUTE_READWRITE,
            &mut old,
        )
        .map_err(|_| ())?;
        let rel = offset as i32;
        let patch = [
            0xE9u8,
            (rel & 0xFF) as u8,
            ((rel >> 8) & 0xFF) as u8,
            ((rel >> 16) & 0xFF) as u8,
            ((rel >> 24) & 0xFF) as u8,
        ];
        std::ptr::copy_nonoverlapping(patch.as_ptr(), target as *mut u8, 5);
        let _ = VirtualProtect(target as *const _, 5, old, &mut old);
        Ok(())
    }

    unsafe extern "C" fn hooked_swap(window: *mut c_void) {
        if super::SKIP_GLFW_SWAP.load(Ordering::Relaxed) && rsift_render::proxy::dx12_active() {
            rsift_render::proxy::rsift_gl_on_swap_buffers();
            return;
        }
        if let Some(orig) = ORIGINAL.get() {
            orig(window);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn install() {}
}

pub fn install_glfw_swap_hook() {
    imp::install();
}

pub fn set_skip_glfw_swap(skip: bool) {
    SKIP_GLFW_SWAP.store(skip, Ordering::Relaxed);
}
