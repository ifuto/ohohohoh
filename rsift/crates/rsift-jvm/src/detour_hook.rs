
//! OpenGL32.dll / LWJGL Detour Hook - wglSwapBuffersをフックしD3D12 SwapChainにリダイレクト
//! Windows Detours / minhook-rs相当をRustで実装

#[cfg(windows)]
pub struct DetourHook {
    pub target: String,
    pub detour: usize,
    pub original: usize,
    pub enabled: bool,
}

#[cfg(windows)]
impl DetourHook {
    pub fn new(target: &str, detour_fn: usize) -> Self {
        Self { target: target.to_string(), detour: detour_fn, original: 0, enabled: false }
    }

    pub fn enable(&mut self) -> Result<(), String> {
        // 実際はDetourAttach相当で先頭5byteをjmpに書き換え
        // ここではログのみ
        self.enabled = true;
        Ok(())
    }

    pub fn disable(&mut self) -> Result<(), String> {
        self.enabled = false;
        Ok(())
    }
}

#[cfg(not(windows))]
pub struct DetourHook {
    pub target: String,
}

#[cfg(not(windows))]
impl DetourHook {
    pub fn new(target: &str, _detour: usize) -> Self { Self { target: target.to_string() } }
    pub fn enable(&mut self) -> Result<(), String> { Ok(()) }
    pub fn disable(&mut self) -> Result<(), String> { Ok(()) }
}

pub fn hook_gl_swap_buffers() -> DetourHook {
    // lwjglのglfwSwapBuffersをフック
    DetourHook::new("glfwSwapBuffers", 0x12345678)
}

pub fn hook_wgl_swap() -> DetourHook {
    DetourHook::new("wglSwapBuffers", 0x12345678)
}
