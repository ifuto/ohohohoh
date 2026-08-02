//! Agent options file + DLL directory discovery (avoids broken -agentpath parsing with spaces).

use crate::agent_log::agent_log;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const OPTS_FILENAME: &str = "rsift-agent.opts";

pub fn dll_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::os::windows::ffi::OsStringExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetModuleHandleExW(
                dw_flags: u32,
                lp_module_name: *const u16,
                ph_module: *mut *mut c_void,
            ) -> i32;
            fn GetModuleFileNameW(h_module: *mut c_void, lp_filename: *mut u16, n_size: u32)
                -> u32;
        }
        const FROM_ADDRESS: u32 = 0x00000004;
        const UNCHANGED_REFCOUNT: u32 = 0x00000002;

        extern "C" {
            fn Agent_OnLoad(
                vm: *mut c_void,
                options: *mut std::os::raw::c_char,
                reserved: *mut c_void,
            ) -> std::os::raw::c_int;
        }

        let mut module: *mut c_void = std::ptr::null_mut();
        let anchor = Agent_OnLoad as *const () as *const u16;
        let ok =
            unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, anchor, &mut module) };
        if ok == 0 || module.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 2048];
        let len = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) };
        if len == 0 {
            return None;
        }
        buf.truncate(len as usize);
        let os = std::ffi::OsString::from_wide(&buf);
        return PathBuf::from(os).parent().map(|p| p.to_path_buf());
    }
    #[cfg(not(windows))]
    {
        // wave 207: Linux でも「自モジュール (=この .so) の絶対パス」を
        // dladdr で取得する。MS の GetModuleFileNameW (上) と同じ意味論 —
        // これにより bootstrap_jar_path() / ログ保存先 / bridge ロードが
        // Linux JVM 上でも動作し、実 Windows 配布環境と同型の検証が
        // sandbox で可能になる (dl_iterate_phdr より dladdr のほうが
        // anchor 指定が確実で、musl/glibc 双方に存在)。
        dll_directory_linux_dladdr()
    }
}

#[cfg(not(windows))]
fn dll_directory_linux_dladdr() -> Option<PathBuf> {
    use std::ffi::c_void;
    use std::os::unix::ffi::OsStrExt;
    #[repr(C)]
    struct DlInfo {
        dli_fname: *const std::os::raw::c_char,
        dli_fbase: *mut c_void,
        dli_sname: *const std::os::raw::c_char,
        dli_saddr: *mut c_void,
    }
    extern "C" {
        fn dladdr(addr: *const c_void, info: *mut DlInfo) -> std::os::raw::c_int;
    }
    extern "C" {
        fn Agent_OnLoad(
            vm: *mut c_void,
            options: *mut std::os::raw::c_char,
            reserved: *mut c_void,
        ) -> std::os::raw::c_int;
    }
    let mut info = DlInfo {
        dli_fname: std::ptr::null(),
        dli_fbase: std::ptr::null_mut(),
        dli_sname: std::ptr::null(),
        dli_saddr: std::ptr::null_mut(),
    };
    let anchor = Agent_OnLoad as *const () as *const c_void;
    let rc = unsafe { dladdr(anchor, &mut info) };
    if rc == 0 || info.dli_fname.is_null() {
        return None;
    }
    let c_str = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) };
    let path = std::ffi::OsStr::from_bytes(c_str.to_bytes());
    let p = PathBuf::from(path);
    // 「.so を含む絶対パス」の親ディレクトリ (= natives dir 相当) を返す
    p.parent().map(|d| d.to_path_buf())
}

pub fn read_opts_file(dir: &Path) -> HashMap<String, String> {
    let path = dir.join(OPTS_FILENAME);
    let mut map = HashMap::new();
    let Ok(text) = fs::read_to_string(&path) else {
        return map;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

pub fn resolve_mod_dir(agent_args: &str) -> PathBuf {
    let dir = dll_directory();
    resolve_mod_dir_with(agent_args, dir.as_deref(), |msg| agent_log(msg))
}

/// 解決順: 明示 arg → opts file → **ゲームプロセス cwd の mods (存在時)** →
/// dll_dir/mods → ./mods。
/// wave 205 欠陥A 根治: agentpath ロードでは dll_dir = rsift-natives であり、
/// ゲーム実体の mods/ (例 Prism `<instance>/.minecraft/mods`) と別物。
/// natives 側に mods/ が無い限り「mod_dir does not exist」で Mod 0 件に
/// なっていた。ゲームの cwd は実際のゲームディレクトリなので、そこに mods/
/// が存在するならそれが正本。
fn resolve_mod_dir_with(agent_args: &str, dll_dir: Option<&Path>, log: impl Fn(&str)) -> PathBuf {
    for part in agent_args.split(',') {
        let part = part.trim();
        if let Some(dir) = part.strip_prefix("modDir=") {
            let p = PathBuf::from(dir.trim_matches('"'));
            log(&format!("[Rsift] mod_dir from agent arg modDir=: {:?}", p));
            return p;
        }
        if let Some(dir) = part.strip_prefix("gameDir=") {
            let p = PathBuf::from(dir.trim_matches('"')).join("mods");
            log(&format!("[Rsift] mod_dir from agent arg gameDir=: {:?}", p));
            return p;
        }
    }
    if let Some(dir) = dll_dir {
        let opts = read_opts_file(dir);
        if let Some(m) = opts.get("modDir") {
            let p = PathBuf::from(m);
            if p.is_dir() {
                log(&format!("[Rsift] mod_dir from opts file modDir: {:?}", p));
                return p;
            }
            // opts の modDir が消えている環境 (リネーム/移動) — 盲目的に返すと
            // 0 件沈黙になるので存在しない場合は下の候補へ進む (ログは残す)。
            log(&format!(
                "[Rsift] opts modDir {:?} missing — falling through",
                p
            ));
        }
        if let Some(g) = opts.get("gameDir") {
            let p = PathBuf::from(g).join("mods");
            log(&format!("[Rsift] mod_dir from opts file gameDir: {:?}", p));
            return p;
        }
        // ゲームプロセスの cwd = 実ゲームディレクトリ (Mods の正本)。
        if let Ok(cwd) = std::env::current_dir() {
            let p = cwd.join("mods");
            if p.is_dir() {
                log(&format!("[Rsift] mod_dir from game cwd: {:?}", p));
                return p;
            }
        }
        let p = dir.join("mods");
        log(&format!("[Rsift] mod_dir fallback dll_dir/mods: {:?}", p));
        return p;
    }
    PathBuf::from("./mods")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // current_dir() はプロセスグローバル — 直列化して他テストと隔離する。
    static CWD_LOCK: Mutex<()> = Mutex::new(());

    fn tempdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rsift_moddir_test_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn arg_moddir_wins_over_everything() {
        let _g = CWD_LOCK.lock().unwrap();
        let dll = tempdir("argwins_dll");
        let got = resolve_mod_dir_with("modDir=/explicit/mods", Some(&dll), |_| {});
        assert_eq!(got, PathBuf::from("/explicit/mods"));
        let got = resolve_mod_dir_with("gameDir=/game", Some(&dll), |_| {});
        assert_eq!(got, PathBuf::from("/game/mods"));
        let _ = std::fs::remove_dir_all(&dll);
    }

    #[test]
    fn cwd_mods_is_preferred_over_dll_dir_mods() {
        let _g = CWD_LOCK.lock().unwrap();
        let saved = std::env::current_dir().unwrap();
        let cwd = tempdir("cwdpref_game");
        let dll = tempdir("cwdpref_dll");
        std::fs::create_dir_all(cwd.join("mods")).unwrap();
        std::env::set_current_dir(&cwd).unwrap();
        let got = resolve_mod_dir_with("", Some(&dll), |_| {});
        std::env::set_current_dir(&saved).unwrap();
        assert_eq!(got, cwd.join("mods"));
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&dll);
    }

    #[test]
    fn absent_cwd_mods_falls_back_to_dll_dir_mods() {
        let _g = CWD_LOCK.lock().unwrap();
        let saved = std::env::current_dir().unwrap();
        let cwd = tempdir("nocwdmods_game");
        let dll = tempdir("nocwdmods_dll");
        // cwd に mods/ を作らない → dll_dir/mods へ落ちる。
        std::env::set_current_dir(&cwd).unwrap();
        let got = resolve_mod_dir_with("", Some(&dll), |_| {});
        std::env::set_current_dir(&saved).unwrap();
        assert_eq!(got, dll.join("mods"));
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&dll);
    }

    #[test]
    fn opts_moddir_missing_falls_through_to_cwd() {
        let _g = CWD_LOCK.lock().unwrap();
        let saved = std::env::current_dir().unwrap();
        let cwd = tempdir("optsdead_game");
        let dll = tempdir("optsdead_dll");
        std::fs::create_dir_all(cwd.join("mods")).unwrap();
        // opts file に存在しない modDir を書く → 継続して cwd/mods が選ばれる。
        std::fs::write(dll.join(OPTS_FILENAME), "modDir=/gone/forever\n").unwrap();
        std::env::set_current_dir(&cwd).unwrap();
        let got = resolve_mod_dir_with("", Some(&dll), |_| {});
        std::env::set_current_dir(&saved).unwrap();
        assert_eq!(got, cwd.join("mods"));
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&dll);
    }

    #[test]
    fn no_dll_dir_yields_relative_mods() {
        let _g = CWD_LOCK.lock().unwrap();
        let got = resolve_mod_dir_with("", None, |_| {});
        assert_eq!(got, PathBuf::from("./mods"));
    }
}
