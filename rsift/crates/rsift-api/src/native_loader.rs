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

/// wave 210 HF: 公式 Mod 自動復旧。
///
/// 背景 (実機観測): rsift-setup は公式 mod (rsgraphics/rsreplay/rszoom) を
/// `<game>/mods/` へ配備するが、その配備ログは Prism 経路では残らず、実機では
/// ゲーム起動時に `mods loaded OK: []` (mods フォルダ空) という状態が再現され
/// た。復旧にはユーザーによる手作業が必要で、非エンジニアには自力切り分けが
/// 困難だった。エージェント自身のインストールディレクトリ (rsift home =
/// rsift-natives) には公式 mod が一式あるので、**ゲーム起動時に自壊復旧する**。
///
/// 権限境界 (least-privilege / 破壊不変条件):
/// - コピー元は固定 3 名のみ (`official_mod_file_names`)。任意ファイル・
///   path traversal の余地はゼロ。
/// - 既存ファイルは絶対に上書きしない (同名が既にあればその mod はスキップ)。
/// - 自動復旧の事実は marker ファイル (`OFFICIAL_MOD_MARKER`) に記録し、
///   marker が残る限り二度と実行しない — ユーザーが自動復旧後に公式 mod を
///   消去した場合、その意思を恒久的に尊重する。
/// - 復旧された mod も通常経路 (modsec vetting 含む) でロードされるため、
///   セキュリティゲートの迂回にはならない。
pub const OFFICIAL_MOD_IDS: [&str; 3] = ["rsgraphics", "rsreplay", "rszoom"];

/// 自動復旧の記録用 marker (mods フォルダ直下のドットファイル)。
/// platform ext ではないため mod discover には決して拾われない。
pub const OFFICIAL_MOD_MARKER: &str = ".rsift-official-mods-restored";

/// rsift-setup が同梱・配備する公式 mod のプラットフォーム別ファイル名。
/// setup 側の命名規則 (rsift-setup/src/lib.rs の LIB_* 定数) と厳密一致が必須。
pub fn official_mod_file_names() -> [&'static str; 3] {
    if cfg!(target_os = "windows") {
        ["rsgraphics.dll", "rsreplay.dll", "rszoom.dll"]
    } else if cfg!(target_os = "macos") {
        [
            "librsgraphics.dylib",
            "librsreplay.dylib",
            "librszoom.dylib",
        ]
    } else {
        ["librsgraphics.so", "librsreplay.so", "librszoom.so"]
    }
}

/// `restore_official_mods` の結果。`skip_reason` は監査ログ用の機械判別子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficialModRestore {
    /// 実際にコピーしたファイル名 (mods フォルダ内の名前)。
    pub restored: Vec<String>,
    /// 復旧が行われなかった理由 (復旧できた場合は None)。
    pub skip_reason: Option<&'static str>,
    /// 実際にコピー元として使ったディレクトリ (復旧が行われた場合のみ Some)。
    /// wave 211 HG: 複数源探索で「どの源から拾ったか」の監査情報。
    pub source_dir: Option<PathBuf>,
}

/// 単一源コア。`restore_official_mods` と複数源探索の共通本体。
fn restore_official_mods_core(
    mod_dir: &Path,
    home_dir: &Path,
) -> Result<OfficialModRestore, String> {
    let skip = |reason: &'static str| {
        Ok(OfficialModRestore {
            restored: Vec::new(),
            skip_reason: Some(reason),
            source_dir: None,
        })
    };
    // 防御: 同一 dir 指定は「復旧」ではない。canonicalize は失敗しうるので
    // 正規化成功時のみ正規化済みパスで比較、fallback は生パス比較。
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canon(mod_dir) == canon(home_dir) {
        return skip("home == mods dir");
    }
    if !mod_dir.exists() {
        std::fs::create_dir_all(mod_dir)
            .map_err(|e| format!("create mods dir {}: {e}", mod_dir.display()))?;
    }
    // marker 優先: ユーザーが自動復旧分を消した場合は沈黙して尊重する。
    if mod_dir.join(OFFICIAL_MOD_MARKER).is_file() {
        return skip("restore marker present — user intent respected");
    }
    let names = official_mod_file_names();
    let candidates: Vec<&'static str> = names
        .iter()
        .copied()
        .filter(|name| home_dir.join(name).is_file())
        .collect();
    if candidates.is_empty() {
        return skip("no official mods in home dir");
    }
    let mut restored: Vec<String> = Vec::new();
    for name in candidates {
        let dst = mod_dir.join(name);
        if dst.exists() {
            // 既存 (ユーザー版/以前の手動配置) は絶対に上書きしない。
            continue;
        }
        std::fs::copy(home_dir.join(name), &dst)
            .map_err(|e| format!("restore {}: {e}", dst.display()))?;
        restored.push(name.to_string());
    }
    if restored.is_empty() {
        return skip("all official mods already present");
    }
    let unix_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let marker = format!(
        "rsift official mod auto-restore (wave 210)\n\
         source={}\n\
         restored={}\n\
         unix_time={unix_time}\n\
         \n\
         このフォルダの公式 mod を削除しても、この marker ファイルが残っている限り\n\
         自動復旧は再実行されません (ユーザーによる削除の意思を尊重します)。\n",
        home_dir.display(),
        restored.join(","),
    );
    std::fs::write(mod_dir.join(OFFICIAL_MOD_MARKER), marker).map_err(|e| {
        format!(
            "marker write {}: {e}",
            mod_dir.join(OFFICIAL_MOD_MARKER).display()
        )
    })?;
    Ok(OfficialModRestore {
        restored,
        skip_reason: None,
        source_dir: Some(home_dir.to_path_buf()),
    })
}

/// 公式 mod を `home_dir` (rsift home / natives) から `mod_dir` へ欠損分のみ
/// コピーする。非破壊 (上書き禁止) で冪等 (marker 管理)。
pub fn restore_official_mods(
    mod_dir: &Path,
    home_dir: &Path,
) -> Result<OfficialModRestore, String> {
    restore_official_mods_core(mod_dir, home_dir)
}

/// バニラの既定 `.minecraft` ディレクトリ (環境変数由来・存在検査は行わない)。
/// wave 211 HG: 自動復旧の副次源解決に使用。実機ユーザーの setup は
/// launcher flow (バニラ側) と prism flow (インスタンス側) の**両方**に
/// 公式 mod を配備するため、natives 側に公式 mod が無い実機でもバニラ側に
/// 残っていることがある (実機 #2 setup ログで両方への配備を機械確認)。
pub fn vanilla_minecraft_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(".minecraft"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join("Library")
                .join("Application Support")
                .join("minecraft")
        })
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".minecraft"))
    }
}

/// 自動復旧の探索源 (優先度順): rsift home (agent dll 所在) →
/// バニラ `.minecraft/mods` → バニラ version dir (`versions/rsift-<mc>`)。
/// 全て「rsift 自身が過去に配備した固定 3 名のみをコピー元にしうる場所」であり、
/// 任意パスを源にする経路は存在しない (least-privilege 維持)。
pub fn default_restore_source_dirs(home_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = home_dir {
        dirs.push(h.to_path_buf());
    }
    if let Some(vm) = vanilla_minecraft_dir() {
        dirs.push(vm.join("mods"));
        dirs.push(
            vm.join("versions")
                .join(format!("rsift-{TARGET_MINECRAFT_VERSION}")),
        );
    }
    dirs
}

/// wave 211 HG: 複数源フォールバック復旧。`source_dirs` を優先度順に試し、
/// **公式 mod が見つかった最初の源からのみ**コピーする (探索打切)。
/// 単一源版の全保証 (上書き禁止/marker 尊重/modsec 非迂回) を継承する。
/// mod_dir 側の確定状態 (marker 残存・全本揃い) が検出された時点でも打切る
/// (後続の源を試しても結果が変わらないため)。
pub fn restore_official_mods_search(
    mod_dir: &Path,
    source_dirs: &[PathBuf],
) -> Result<OfficialModRestore, String> {
    if source_dirs.is_empty() {
        return Ok(OfficialModRestore {
            restored: Vec::new(),
            skip_reason: Some("no restore sources"),
            source_dir: None,
        });
    }
    for src in source_dirs {
        let out = restore_official_mods_core(mod_dir, src)?;
        if !out.restored.is_empty() {
            return Ok(out);
        }
        match out.skip_reason {
            // mod_dir 側の確定状態: 後続の源を試しても同じ結果になるため打切
            Some(r)
                if r == "restore marker present — user intent respected"
                    || r == "all official mods already present" =>
            {
                return Ok(OfficialModRestore {
                    restored: Vec::new(),
                    skip_reason: Some(r),
                    source_dir: None,
                });
            }
            // 源側の問題 (公式 mod 無し / home==mods 防御): 次の源へ進む
            _ => continue,
        }
    }
    Ok(OfficialModRestore {
        restored: Vec::new(),
        skip_reason: Some("no official mods in any source dir"),
        source_dir: None,
    })
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
            Err(e) => {
                // wave 219 HP: エージェント環境 (tracing subscriber 未初期化) でも
                // bootstrap ログへ届くよう log_bridge へも送る (P1 根治)。
                crate::log_bridge::log_important(&format!(
                    "[NativeLoader] {path:?}: {e}"
                ));
                error!("[NativeLoader] {:?}: {}", path, e);
            }
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
                Err(e) => {
                    crate::log_bridge::log_important(&format!(
                        "[NativeLoader] Deferred load failed: {e}"
                    ));
                    error!("[NativeLoader] Deferred load failed: {}", e);
                }
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
                    // wave HR: 初回起動同意 GUI — 危険権限を使うMod(公式/第三者問わず)は
                    // 起動直後にネイティブダイアログで「○○は...を触ろうとしています。許可しますか？」
                    // を出す。OK→承認を永続化→次回起動は素通り。No→スキップ。
                    let approved = crate::mod_security::prompt_consent(&mod_id, &missing);
                    if approved {
                        let caps = outcome.report.capabilities_detected.clone();
                        if let Err(e) = crate::mod_security::grant_caps(
                            &outcome.consent_path,
                            &mod_id,
                            &outcome.file_sha256_hex,
                            &caps,
                        ) {
                            warn!("[modsec] grant_caps persist failed for {}: {}", mod_id, e);
                        }
                        crate::mod_security::SecurityGate::register(&mod_id, caps.clone());
                        caps
                    } else {
                        crate::mod_security::ensure_pending_entry(&outcome, &mod_id);
                        return Err(format!(
                            "[modsec] {} not approved by user (host capabilities [{}] declined)",
                            mod_id,
                            missing.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ")
                        ));
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "rsift_native_loader_test_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    struct DirGuard(PathBuf);

    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn official_names() -> Vec<String> {
        official_mod_file_names()
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn plant_official_mods(home: &Path) {
        for name in official_names() {
            fs::write(home.join(&name), format!("OFFICIAL:{name}")).unwrap();
        }
    }

    // 名称 pin: setup が同梱する公式 mod ファイル名と厳密に一致すること。
    // 食い違うと実機で復旧元が見つからず沈黙スキップになるため固定する。
    #[test]
    fn hf_official_mod_file_names_pin() {
        let names = official_mod_file_names();
        assert_eq!(names.len(), 3);
        if cfg!(target_os = "windows") {
            assert_eq!(names, ["rsgraphics.dll", "rsreplay.dll", "rszoom.dll"]);
        } else if cfg!(target_os = "macos") {
            assert_eq!(
                names,
                [
                    "librsgraphics.dylib",
                    "librsreplay.dylib",
                    "librszoom.dylib"
                ]
            );
        } else {
            assert_eq!(
                names,
                ["librsgraphics.so", "librsreplay.so", "librszoom.so"]
            );
        }
    }

    #[test]
    fn hf_restore_copies_official_mods_when_mods_dir_empty() {
        let home = tmpdir("home");
        let mods = tmpdir("mods");
        let _g1 = DirGuard(home.clone());
        let _g2 = DirGuard(mods.clone());
        plant_official_mods(&home);
        // 非 mod ファイル (拡張子が platform ext 以外) は干渉対象ではない
        fs::write(mods.join("readme.txt"), "user notes").unwrap();

        let out = restore_official_mods(&mods, &home).unwrap();
        let mut restored = out.restored.clone();
        restored.sort();
        let mut expect = official_names();
        expect.sort();
        assert_eq!(restored, expect, "全公式 mod が復旧される");
        assert!(out.skip_reason.is_none());
        for name in official_names() {
            // コピー内容はバイト完全一致
            assert_eq!(
                fs::read(mods.join(&name)).unwrap(),
                format!("OFFICIAL:{name}").as_bytes(),
                "copied bytes must be identical: {name}"
            );
        }
        // marker に監査情報 (復旧名・コピー元) を残す
        let marker = fs::read_to_string(mods.join(OFFICIAL_MOD_MARKER)).unwrap();
        for name in official_names() {
            assert!(marker.contains(&name), "marker must list {name}");
        }
        assert!(marker.contains(&home.display().to_string()));
        // 非 mod ファイルを壊していない
        assert_eq!(
            fs::read_to_string(mods.join("readme.txt")).unwrap(),
            "user notes"
        );
    }

    #[test]
    fn hf_restore_fills_missing_only_and_never_overwrites() {
        let home = tmpdir("home");
        let mods = tmpdir("mods");
        let _g1 = DirGuard(home.clone());
        let _g2 = DirGuard(mods.clone());
        plant_official_mods(&home);
        let names = official_names();
        // 1 本目だけ mods 側に既存 (ユーザー版) — 絶対に上書きされないこと
        fs::write(mods.join(&names[0]), b"USER-VERSION").unwrap();
        // ユーザーの自作 mod も無改変
        let mine = format!("mymod.{}", platform_extension());
        fs::write(mods.join(&mine), b"USER-BYTES").unwrap();

        let out = restore_official_mods(&mods, &home).unwrap();
        assert_eq!(out.restored.len(), 2, "欠けている 2 本のみ復旧");
        assert!(!out.restored.contains(&names[0]));
        assert_eq!(
            fs::read(mods.join(&names[0])).unwrap(),
            b"USER-VERSION",
            "既存ファイルは絶対に上書きしない"
        );
        assert_eq!(fs::read(mods.join(&mine)).unwrap(), b"USER-BYTES");
    }

    #[test]
    fn hf_restore_respects_user_deletion_marker() {
        let home = tmpdir("home");
        let mods = tmpdir("mods");
        let _g1 = DirGuard(home.clone());
        let _g2 = DirGuard(mods.clone());
        plant_official_mods(&home);
        // 以前の自動復旧後にユーザーが公式 mod を消去した経緯 (marker 残存)
        fs::write(mods.join(OFFICIAL_MOD_MARKER), b"restored=x\n").unwrap();

        let out = restore_official_mods(&mods, &home).unwrap();
        assert!(out.restored.is_empty());
        assert_eq!(
            out.skip_reason,
            Some("restore marker present — user intent respected")
        );
        for name in official_names() {
            assert!(
                !mods.join(&name).exists(),
                "marker がある限り再コピーしない"
            );
        }
    }

    #[test]
    fn hf_restore_skips_when_home_lacks_official_mods() {
        let home = tmpdir("home");
        let mods = tmpdir("mods");
        let _g1 = DirGuard(home.clone());
        let _g2 = DirGuard(mods.clone());

        let out = restore_official_mods(&mods, &home).unwrap();
        assert!(out.restored.is_empty());
        assert_eq!(out.skip_reason, Some("no official mods in home dir"));
        assert!(!mods.join(OFFICIAL_MOD_MARKER).exists());
    }

    #[test]
    fn hf_restore_refuses_when_home_equals_mods_dir() {
        let home = tmpdir("same");
        let _g1 = DirGuard(home.clone());
        plant_official_mods(&home);

        let out = restore_official_mods(&home, &home).unwrap();
        assert!(out.restored.is_empty());
        assert_eq!(out.skip_reason, Some("home == mods dir"));
    }

    #[test]
    fn hf_restore_only_copies_fixed_official_names() {
        let home = tmpdir("home");
        let mods = tmpdir("mods");
        let _g1 = DirGuard(home.clone());
        let _g2 = DirGuard(mods.clone());
        plant_official_mods(&home);
        // 固定 3 名以外は rsift home にあっても復旧先へコピーしない
        let evil = format!("evil.{}", platform_extension());
        fs::write(home.join(&evil), b"NOT-OFFICIAL").unwrap();

        let out = restore_official_mods(&mods, &home).unwrap();
        assert!(!mods.join(&evil).exists(), "固定名以外は絶対にコピーしない");
        assert_eq!(out.restored.len(), 3);
        assert!(!out.restored.contains(&evil));
    }

    #[test]
    fn hf_restore_is_idempotent_second_call_is_noop() {
        let home = tmpdir("home");
        let mods = tmpdir("mods");
        let _g1 = DirGuard(home.clone());
        let _g2 = DirGuard(mods.clone());
        plant_official_mods(&home);

        let first = restore_official_mods(&mods, &home).unwrap();
        assert_eq!(first.restored.len(), 3);
        let second = restore_official_mods(&mods, &home).unwrap();
        assert!(second.restored.is_empty(), "2 回目は marker で no-op");
        assert_eq!(
            second.skip_reason,
            Some("restore marker present — user intent respected")
        );
    }

    // ---- wave 211 HG: 複数源フォールバック ----

    #[test]
    fn hg_search_falls_back_to_second_source() {
        let src1 = tmpdir("src1"); // 公式 mod 無し (実機の rsift-natives 相当)
        let src2 = tmpdir("src2"); // 公式 mod 有り (実機のバニラ .minecraft\mods 相当)
        let mods = tmpdir("mods");
        let _g1 = DirGuard(src1.clone());
        let _g2 = DirGuard(src2.clone());
        let _g3 = DirGuard(mods.clone());
        plant_official_mods(&src2);

        let out = restore_official_mods_search(&mods, &[src1.clone(), src2.clone()]).unwrap();
        assert_eq!(out.restored.len(), 3, "第2源から復旧される");
        assert_eq!(out.source_dir, Some(src2.clone()), "実際に使われた源を報告");
        for name in official_names() {
            assert_eq!(
                fs::read(mods.join(&name)).unwrap(),
                format!("OFFICIAL:{name}").as_bytes()
            );
        }
    }

    #[test]
    fn hg_search_prefers_first_available_source() {
        let src1 = tmpdir("src1");
        let src2 = tmpdir("src2");
        let mods = tmpdir("mods");
        let _g1 = DirGuard(src1.clone());
        let _g2 = DirGuard(src2.clone());
        let _g3 = DirGuard(mods.clone());
        plant_official_mods(&src1);
        plant_official_mods(&src2);

        let out = restore_official_mods_search(&mods, &[src1.clone(), src2]).unwrap();
        assert_eq!(out.restored.len(), 3);
        assert_eq!(
            out.source_dir,
            Some(src1),
            "最初に成立した源を使う (探索打切)"
        );
    }

    #[test]
    fn hg_search_empty_when_no_source_has_official() {
        let src1 = tmpdir("src1");
        let src2 = tmpdir("src2");
        let mods = tmpdir("mods");
        let _g1 = DirGuard(src1.clone());
        let _g2 = DirGuard(src2.clone());
        let _g3 = DirGuard(mods.clone());

        let out = restore_official_mods_search(&mods, &[src1, src2]).unwrap();
        assert!(out.restored.is_empty());
        assert_eq!(out.skip_reason, Some("no official mods in any source dir"));
        assert!(out.source_dir.is_none());
        assert!(!mods.join(OFFICIAL_MOD_MARKER).exists());
    }

    #[test]
    fn hg_search_marker_stops_scan_regardless_of_sources() {
        let src1 = tmpdir("src1");
        let src2 = tmpdir("src2");
        let mods = tmpdir("mods");
        let _g1 = DirGuard(src1.clone());
        let _g2 = DirGuard(src2.clone());
        let _g3 = DirGuard(mods.clone());
        plant_official_mods(&src2);
        fs::write(mods.join(OFFICIAL_MOD_MARKER), b"restored=x\n").unwrap();

        let out = restore_official_mods_search(&mods, &[src1, src2]).unwrap();
        assert!(out.restored.is_empty());
        assert_eq!(
            out.skip_reason,
            Some("restore marker present — user intent respected")
        );
        for name in official_names() {
            assert!(!mods.join(&name).exists());
        }
    }

    #[test]
    fn hg_search_source_equal_to_mod_dir_does_not_block_fallback() {
        let mods = tmpdir("mods");
        let src2 = tmpdir("src2");
        let _g1 = DirGuard(mods.clone());
        let _g2 = DirGuard(src2.clone());
        plant_official_mods(&src2);
        // 第1源が mods dir 自身 (= 防御 reject) でも、第2源へ進む
        let out = restore_official_mods_search(&mods, &[mods.clone(), src2.clone()]).unwrap();
        assert_eq!(out.restored.len(), 3);
        assert_eq!(out.source_dir, Some(src2));
    }

    #[test]
    fn hg_default_restore_source_dirs_order_pin() {
        let home = PathBuf::from("Z:/rsift-natives");
        let dirs = default_restore_source_dirs(Some(&home));
        assert_eq!(dirs[0], home, "第1源は rsift home (natives)");
        // バニラ既定パスが環境変数から解決できる環境なら 2 源追加
        if vanilla_minecraft_dir().is_some() {
            assert_eq!(dirs.len(), 3);
            assert!(
                dirs[1].ends_with("mods"),
                "第2源 = バニラ mods: {:?}",
                dirs[1]
            );
            assert!(
                dirs[2].ends_with(Path::new("versions").join("rsift-1.21.11")),
                "第3源 = バニラ version dir: {:?}",
                dirs[2]
            );
        } else {
            assert_eq!(dirs.len(), 1);
        }
        // home 未指定でもバニラ源は返る
        let bare = default_restore_source_dirs(None);
        assert!(!bare.iter().any(|d| d == &PathBuf::from("Z:/rsift-natives")));
    }

    #[test]
    fn hg_vanilla_minecraft_dir_platform_shape_pin() {
        // env が揃っている場合の形状 pin (実機の既定ランチャーパス構成)
        if let Some(d) = vanilla_minecraft_dir() {
            if cfg!(target_os = "windows") {
                assert!(d.ends_with(".minecraft"), "{d:?}");
            } else if cfg!(target_os = "macos") {
                assert!(
                    d.ends_with(
                        Path::new("Library")
                            .join("Application Support")
                            .join("minecraft")
                    ),
                    "{d:?}"
                );
            } else {
                assert!(d.ends_with(".minecraft"), "{d:?}");
            }
        }
    }
}
