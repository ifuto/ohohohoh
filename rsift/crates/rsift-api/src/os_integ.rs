//! # OS Integration & Web API (`os_integ`)
//!
//! インゲームのボタンやショートカットから、デフォルトの Web ブラウザで URL を開く、
//! クリップボードを操作する OS 統合 API。

use std::process::Command;
use tracing::{info, warn};

pub struct OsIntegration;

impl OsIntegration {
    /// デフォルトの Web ブラウザで URL を開く API！ (`open_url_in_browser`)
    pub fn open_url_in_browser(url: &str) -> Result<(), String> {
        info!("🌐 [OsIntegration] Opening Web URL in default browser: {}", url);

        let res = if cfg!(target_os = "windows") {
            let mut cmd = Command::new("cmd");
            cmd.args(["/c", "start", "", url]);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            cmd.spawn()
        } else if cfg!(target_os = "macos") {
            Command::new("open").arg(url).spawn()
        } else {
            Command::new("xdg-open").arg(url).spawn()
        };

        match res {
            Ok(_) => {
                info!("✨ Web browser launched successfully!");
                Ok(())
            }
            Err(e) => {
                let err_msg = format!("Failed to launch web browser: {}", e);
                warn!("{}", err_msg);
                Err(err_msg)
            }
        }
    }

    /// クリップボードへテキストをコピーする API (`set_clipboard_text`)
    pub fn set_clipboard_text(text: &str) -> Result<(), String> {
        info!("📋 [OsIntegration] Copied text to OS clipboard");
        #[cfg(target_os = "windows")]
        {
            return windows_clipboard::set_text(text);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = text;
            Ok(())
        }
    }

    /// クリップボードの文字列を取得する API (`get_clipboard_text`)
    pub fn get_clipboard_text() -> String {
        info!("📋 [OsIntegration] Reading text from OS clipboard...");
        #[cfg(target_os = "windows")]
        {
            return windows_clipboard::get_text();
        }
        #[cfg(not(target_os = "windows"))]
        {
            String::new()
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_clipboard {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStringExt;

    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 0x0002;

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(hwnd: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
        fn GetClipboardData(format: u32) -> *mut c_void;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalLock(mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(mem: *mut c_void) -> *mut c_void;
        fn GlobalSize(mem: *mut c_void) -> usize;
    }

    fn utf16(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn set_text(text: &str) -> Result<(), String> {
        let wide = utf16(text);
        let bytes = wide.len() * 2;
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return Err("OpenClipboard failed".into());
            }
            let result = (|| {
                if EmptyClipboard() == 0 {
                    return Err("EmptyClipboard failed".into());
                }
                let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
                if mem.is_null() {
                    return Err("GlobalAlloc failed".into());
                }
                let lock = GlobalLock(mem);
                if lock.is_null() {
                    return Err("GlobalLock failed".into());
                }
                std::ptr::copy_nonoverlapping(wide.as_ptr(), lock as *mut u16, wide.len());
                GlobalUnlock(mem);
                if SetClipboardData(CF_UNICODETEXT, mem).is_null() {
                    return Err("SetClipboardData failed".into());
                }
                Ok(())
            })();
            CloseClipboard();
            result
        }
    }

    pub fn get_text() -> String {
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                return String::new();
            }
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return String::new();
            }
            let text = (|| {
                let mem = GetClipboardData(CF_UNICODETEXT);
                if mem.is_null() {
                    return String::new();
                }
                let lock = GlobalLock(mem);
                if lock.is_null() {
                    return String::new();
                }
                let size = GlobalSize(mem);
                if size < 2 {
                    GlobalUnlock(mem);
                    return String::new();
                }
                let len = size / 2;
                let slice = std::slice::from_raw_parts(lock as *const u16, len);
                let s = std::ffi::OsString::from_wide(slice)
                    .to_string_lossy()
                    .trim_end_matches('\0')
                    .to_string();
                GlobalUnlock(mem);
                s
            })();
            CloseClipboard();
            text
        }
    }
}
