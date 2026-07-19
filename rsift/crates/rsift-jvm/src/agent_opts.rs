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
            fn GetModuleFileNameW(h_module: *mut c_void, lp_filename: *mut u16, n_size: u32) -> u32;
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
        let ok = unsafe {
            GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, anchor, &mut module)
        };
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
        let _ = agent_log;
        None
    }
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
    for part in agent_args.split(',') {
        let part = part.trim();
        if let Some(dir) = part.strip_prefix("modDir=") {
            return PathBuf::from(dir.trim_matches('"'));
        }
        if let Some(dir) = part.strip_prefix("gameDir=") {
            return PathBuf::from(dir.trim_matches('"')).join("mods");
        }
    }
    if let Some(dir) = dll_directory() {
        let opts = read_opts_file(&dir);
        if let Some(m) = opts.get("modDir") {
            return PathBuf::from(m);
        }
        if let Some(g) = opts.get("gameDir") {
            return PathBuf::from(g).join("mods");
        }
        return dir.join("mods");
    }
    PathBuf::from("./mods")
}
