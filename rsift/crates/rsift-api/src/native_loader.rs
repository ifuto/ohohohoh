//! Native DLL mod discovery and loading (shared by launcher + java agent).

use crate::adaptive_perf::AdaptivePerfEngine;
use crate::mod_api::{
    ModContext, ModManifest, RsiftModInitFn, RsiftModOnFovScaleFn, RsiftModOnPacketFn,
    RsiftModOnRenderFn,
};
use crate::registry::ModRegistry;
use crate::runtime::RsiftRuntime;
use crate::TARGET_MINECRAFT_VERSION;
use libloading::Library;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use tracing::{debug, error, info, warn};

const LOAD_PRIORITY: &[&str] = &["rscalc", "rsgraphics", "rsreplay", "rszoom"];
const SPEED_FIRST_IMMEDIATE: &[&str] = &["rsgraphics", "rsreplay", "rszoom"];
const SPEED_FIRST_DEFERRED: &[&str] = &["rscalc"];

#[inline]
pub fn platform_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

pub fn normalize_mod_id(stem: &str) -> String {
    let s = stem.trim();
    if let Some(rest) = s.strip_prefix("lib") {
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    s.to_string()
}

#[derive(Clone)]
pub struct LoadedModLibrary {
    pub id: String,
    #[allow(dead_code)]
    pub library: Arc<Library>,
}

#[derive(Clone)]
pub struct NativeModLoadResult {
    pub loaded: Vec<String>,
    pub libraries: Vec<LoadedModLibrary>,
}

pub fn discover_mod_paths(mod_dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !mod_dir.exists() {
        std::fs::create_dir_all(mod_dir).map_err(|e| e.to_string())?;
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(mod_dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|e| e.to_string_lossy().to_lowercase() == platform_extension())
                .unwrap_or(false)
        })
        .collect();
    paths.sort_by(|a, b| {
        let stem_a = normalize_mod_id(a.file_stem().and_then(|s| s.to_str()).unwrap_or(""));
        let stem_b = normalize_mod_id(b.file_stem().and_then(|s| s.to_str()).unwrap_or(""));
        let pri_a = LOAD_PRIORITY
            .iter()
            .position(|&p| p == stem_a)
            .unwrap_or(999);
        let pri_b = LOAD_PRIORITY
            .iter()
            .position(|&p| p == stem_b)
            .unwrap_or(999);
        pri_a.cmp(&pri_b).then_with(|| stem_a.cmp(&stem_b))
    });
    Ok(paths)
}

pub fn load_mods_into_runtime(
    mod_dir: &Path,
    runtime: &RsiftRuntime,
    is_client: bool,
) -> Result<NativeModLoadResult, String> {
    let hw = AdaptivePerfEngine::hardware();
    let speed_first = AdaptivePerfEngine::render_profile(hw).speed_first;
    if speed_first && is_client {
        // Client: load all UI/packet/render mods immediately; only defer heavy compute (rscalc).
        let immediate = load_mods_filtered(mod_dir, runtime, is_client, SPEED_FIRST_IMMEDIATE)?;
        pin_loaded_libraries(immediate.clone());
        schedule_deferred_mod_load(mod_dir.to_path_buf());
        runtime.mark_mods_loaded();
        return Ok(immediate);
    }
    let result = load_all_mod_paths(mod_dir, runtime, is_client)?;
    pin_loaded_libraries(result.clone());
    runtime.mark_mods_loaded();
    Ok(result)
}

fn load_all_mod_paths(
    mod_dir: &Path,
    runtime: &RsiftRuntime,
    is_client: bool,
) -> Result<NativeModLoadResult, String> {
    let paths = discover_mod_paths(mod_dir)?;
    load_paths(&paths, runtime, is_client)
}

pub fn load_mods_filtered(
    mod_dir: &Path,
    runtime: &RsiftRuntime,
    is_client: bool,
    allow: &[&str],
) -> Result<NativeModLoadResult, String> {
    let paths = discover_mod_paths(mod_dir)?;
    let filtered: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| {
            let stem = normalize_mod_id(p.file_stem().and_then(|s| s.to_str()).unwrap_or(""));
            allow.contains(&stem.as_str())
        })
        .collect();
    load_paths(&filtered, runtime, is_client)
}

fn load_paths(
    paths: &[PathBuf],
    runtime: &RsiftRuntime,
    is_client: bool,
) -> Result<NativeModLoadResult, String> {
    if paths.is_empty() {
        warn!("[NativeLoader] No mods matched filter");
        return Ok(NativeModLoadResult {
            loaded: Vec::new(),
            libraries: Vec::new(),
        });
    }

    info!("[NativeLoader] Loading {} mod(s)", paths.len());
    let mut registry = runtime.registry.lock().map_err(|e| e.to_string())?;
    let mut loaded = Vec::new();
    let mut libraries = Vec::new();

    for path in paths {
        match load_single_mod(path, runtime, &mut registry, is_client) {
            Ok(lib) => {
                loaded.push(lib.id.clone());
                libraries.push(lib);
            }
            Err(e) => error!("[NativeLoader] {:?}: {}", path, e),
        }
    }

    info!("[NativeLoader] Loaded: {:?}", loaded);
    Ok(NativeModLoadResult { loaded, libraries })
}

fn schedule_deferred_mod_load(mod_dir: PathBuf) {
    std::thread::Builder::new()
        .name("rsift-deferred-mods".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(20));
            let Some(rt) = crate::runtime::runtime() else {
                return;
            };
            match load_mods_filtered(&mod_dir, rt, true, SPEED_FIRST_DEFERRED) {
                Ok(result) => {
                    pin_loaded_libraries(result);
                    info!(
                        "[NativeLoader] Deferred mods loaded: {:?}",
                        SPEED_FIRST_DEFERRED
                    );
                }
                Err(e) => error!("[NativeLoader] Deferred load failed: {}", e),
            }
        })
        .ok();
}

fn load_single_mod(
    path: &Path,
    runtime: &RsiftRuntime,
    registry: &mut ModRegistry,
    is_client: bool,
) -> Result<LoadedModLibrary, String> {
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mod_id = normalize_mod_id(&stem);
    debug!("[NativeLoader] Loading [{}] from {:?}", mod_id, path);

    // セキュリティゲート (wave 195): 動的リンク実行より先に静的検査 + 同意照合。
    // Deny/RequireConsent はここでロード中止 (サイレント乗っ取りの根絶)、
    // 検出ゼロの通常 Mod は従来どおり摩擦ゼロで通過する (幅不変)。
    let granted_caps: std::collections::BTreeSet<crate::mod_security::HostCapability> =
        match crate::mod_security::vet_mod_binary(&mod_id, path) {
            Ok(outcome) => match &outcome.verdict {
                crate::mod_security::LoadVerdict::Allow(granted) => {
                    if !granted.is_empty() {
                        info!(
                            "[modsec] {} vetted OK with granted host capabilities: {:?}",
                            mod_id,
                            granted.iter().map(|c| c.as_str()).collect::<Vec<_>>()
                        );
                    }
                    crate::mod_security::SecurityGate::register(&mod_id, granted.clone());
                    granted.clone()
                }
                crate::mod_security::LoadVerdict::RequireConsent(missing) => {
                    crate::mod_security::ensure_pending_entry(&outcome, &mod_id);
                    return Err(format!(
                    "[modsec] {} requires user consent for host capabilities [{}] — approve in {:?} ({})。承認後に再ロードされます",
                    mod_id,
                    missing.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", "),
                    outcome.consent_path,
                    missing.iter().map(|c| c.label_ja()).collect::<Vec<_>>().join(" / ")
                ));
                }
                crate::mod_security::LoadVerdict::Deny(reasons) => {
                    error!("[modsec] {} HARD-DENIED: {}", mod_id, reasons.join("; "));
                    return Err(format!(
                        "[modsec] {} load DENIED (hijack signature): {}",
                        mod_id,
                        reasons.join("; ")
                    ));
                }
            },
            Err(e) => return Err(e),
        };

    let library =
        unsafe { Library::new(path).map_err(|e| format!("Dynamic linker error: {}", e))? };
    let lib_arc = Arc::new(library);

    let manifest = ModManifest {
        id: mod_id.clone(),
        name: format!("NativeMod ({})", mod_id),
        version: "1.0.0".to_string(),
        author: "Rsift".to_string(),
        description: "Rsift Native DLL Mod".to_string(),
        target_rsift_version: TARGET_MINECRAFT_VERSION.to_string(),
        capabilities: granted_caps
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
    };

    if let Ok(init_fn) = unsafe { lib_arc.get::<RsiftModInitFn>(b"rsift_mod_init\0") } {
        let mut ctx = ModContext::new(manifest.clone(), registry, is_client, runtime);
        let status = init_fn(&mut ctx);
        if status != 0 {
            return Err(format!("rsift_mod_init returned {}", status));
        }
    } else {
        warn!("[{}] no rsift_mod_init export", mod_id);
    }

    if let Ok(f) = unsafe { lib_arc.get::<RsiftModOnPacketFn>(b"rsift_mod_on_packet\0") } {
        runtime.add_packet_handler(*f);
    }
    if let Ok(f) = unsafe { lib_arc.get::<RsiftModOnRenderFn>(b"rsift_mod_on_render\0") } {
        runtime.add_render_handler(*f);
    }
    // 任意 export `rsift_mod_get_fov_scale` (wave 201): 無い mod は恒等扱い。
    if let Ok(f) = unsafe { lib_arc.get::<RsiftModOnFovScaleFn>(b"rsift_mod_get_fov_scale\0") } {
        runtime.add_fov_scale_handler(*f);
    }

    Ok(LoadedModLibrary {
        id: mod_id,
        library: lib_arc,
    })
}

/// Keep libraries alive for process lifetime (agent / launcher).
static MOD_LIBRARY_KEEPALIVE: OnceLock<Mutex<Vec<LoadedModLibrary>>> = OnceLock::new();

pub fn pin_loaded_libraries(result: NativeModLoadResult) {
    let keep = MOD_LIBRARY_KEEPALIVE.get_or_init(|| Mutex::new(Vec::new()));
    keep.lock().unwrap().extend(result.libraries);
}
