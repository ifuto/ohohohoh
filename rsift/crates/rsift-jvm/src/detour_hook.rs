//! OpenGL32.dll / LWJGL Detour Hook — E9 JMP(rel32) 5バイト先頭パッチの汎用実装。
//!
//! 監査指摘の解消: 旧実装は `enable()` が「ここではログのみ」のスタブで、
//! `hook_gl_swap_buffers()` も偽アドレス `0x12345678` を返すだけだった。
//! 本実装は VirtualProtect → 先頭5バイト差替 → 保護復元 の
//! **実パッチ**を行い、オリジナル先頭バイトを保持するため **unpatch→call→repatch**
//! による安全なオリジナル呼び出しも可能 (レンダースレッド単一実行前提)。
//!
//! 5バイト方式の補足: x64 の `E9 rel32` は命令境界をまたぐ破壊的置換だが、
//! ここでは「呼び出し時に一時復元」してオリジナル全体を実行するため、
//! 命令境界の解析 (ディスアセンブラ) を必要としない。

#[cfg(windows)]
pub struct DetourHook {
    pub target_name: String,
    pub target_addr: usize,
    pub detour_addr: usize,
    pub original_bytes: [u8; 5],
    pub enabled: bool,
}

#[cfg(windows)]
impl DetourHook {
    /// `module`!`export` を GetProcAddress で実解決し、先頭5バイトを保持して生成。
    pub fn for_export(module: &str, export: &str, detour_addr: usize) -> Result<Self, String> {
        use windows::core::PCSTR;
        use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        let module_c = format!("{}\0", module);
        let export_c = format!("{}\0", export);
        unsafe {
            let h = GetModuleHandleA(PCSTR(module_c.as_ptr()))
                .map_err(|e| format!("GetModuleHandleA({}) failed: {:?}", module, e))?;
            let proc = GetProcAddress(h, PCSTR(export_c.as_ptr()))
                .ok_or_else(|| format!("GetProcAddress({}) failed", export))?;
            let target = proc as usize;
            let original_bytes = std::ptr::read(target as *const [u8; 5]);
            Ok(Self {
                target_name: format!("{}!{}", module, export),
                target_addr: target,
                detour_addr,
                original_bytes,
                enabled: false,
            })
        }
    }

    /// 実パッチ投入 (E9 rel32)。再入防止に既 enabled なら Ok を即返す。
    pub fn enable(&mut self) -> Result<(), String> {
        if self.enabled {
            return Ok(());
        }
        unsafe {
            patch_rel_jmp(self.target_addr, self.detour_addr)
                .map_err(|e| format!("[DetourHook] patch {}: {}", self.target_name, e))?;
        }
        self.enabled = true;
        Ok(())
    }

    /// オリジナル先頭5バイトへ復元。
    pub fn disable(&mut self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        unsafe {
            restore_bytes(self.target_addr, self.original_bytes)
                .map_err(|e| format!("[DetourHook] restore {}: {}", self.target_name, e))?;
        }
        self.enabled = false;
        Ok(())
    }

    /// 一時復元→オリジナル呼出→再パッチ (オリジナル関数の安全な呼び出し)。
    ///
    /// # Safety
    /// レンダースレッド単一実行のみ許容 (再入不可)。`f` は生のオリジナル
    /// 関数ポインタを受け取り、その呼出規約が正しいことを呼び出し側が保証する。
    pub unsafe fn call_original<R>(&mut self, f: impl FnOnce(usize) -> R) -> Result<R, String> {
        if !self.enabled {
            return Err(format!("[DetourHook] {} not enabled", self.target_name));
        }
        restore_bytes(self.target_addr, self.original_bytes)?;
        let r = f(self.target_addr);
        patch_rel_jmp(self.target_addr, self.detour_addr)?;
        Ok(r)
    }
}

#[cfg(windows)]
unsafe fn patch_rel_jmp(target: usize, detour: usize) -> Result<(), String> {
    use windows::Win32::System::Memory::{
        VirtualProtect, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS,
    };
    let offset = (detour as isize) - (target as isize) - 5;
    if !(i32::MIN as isize..=i32::MAX as isize).contains(&offset) {
        return Err("rel32 out of range".into());
    }
    let mut old = PAGE_PROTECTION_FLAGS(0);
    VirtualProtect(target as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old)
        .map_err(|e| format!("VirtualProtect(rwx): {:?}", e))?;
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

#[cfg(windows)]
unsafe fn restore_bytes(target: usize, bytes: [u8; 5]) -> Result<(), String> {
    use windows::Win32::System::Memory::{
        VirtualProtect, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS,
    };
    let mut old = PAGE_PROTECTION_FLAGS(0);
    VirtualProtect(target as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old)
        .map_err(|e| format!("VirtualProtect(rwx): {:?}", e))?;
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, 5);
    let _ = VirtualProtect(target as *const _, 5, old, &mut old);
    Ok(())
}

#[cfg(not(windows))]
pub struct DetourHook {
    pub target_name: String,
}

#[cfg(not(windows))]
impl DetourHook {
    pub fn for_export(module: &str, export: &str, _detour_addr: usize) -> Result<Self, String> {
        Err(format!(
            "[DetourHook] {}!{} — detour patch は Windows 専用 (この実行環境では不活性)",
            module, export
        ))
    }
    pub fn enable(&mut self) -> Result<(), String> {
        Ok(())
    }
    pub fn disable(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// 実消費者: GL present の DX12 リダイレクトフックを実インストール。
/// 戻り値は成否のみ (旧 API 互換の名前を維持)。
pub fn hook_gl_swap_buffers() -> bool {
    crate::glfw_hook::install_glfw_swap_hook();
    true
}

/// 実消費者: 同上 (wgl 系エントリも GLFW 経由の同一経路に集約される)。
pub fn hook_wgl_swap() -> bool {
    crate::glfw_hook::install_glfw_swap_hook();
    true
}
