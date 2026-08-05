//! Java agent bridge — deferred mod load + safe JNI screen injection.

// 2026-07-21 監査: 以前は `#[path = "..."]` で兄弟ファイル 8 本
// (chunk_bridge / glfw_hook / jvmti_events / mod_bridge / platform_bridge /
// render_bridge / screen_buttons / screen_inject) を**私有 mod として二重
// ロード**していた。その結果、ファイル内の `#[no_mangle]` JNI エクスポートと
// static 状態がクレート内に 2 インスタンス化され、
//   * テストバイナリのリンクで `symbol Java_com_rsift_... is already defined`
//   * 実行時は static 状態の分裂 (agent 経由と JNI 直接経由で別 static)
// を引き起こしていた。各ファイルは `super::兄弟` 相互参照のみで木非依存に
// 書かれているため本体は lib.rs の `pub mod` 1 本で成立する。
// 本ファイルは以下の crate:: エイリアス経由で参照する (未使用の 3 本
// — chunk_bridge / glfw_hook / render_bridge — のインポートは不要)。
use crate::{jvmti_events, mod_bridge, platform_bridge, screen_buttons, screen_inject};

use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jarray, jbyteArray, jint, jstring};
use jni::JNIEnv;
use jni::NativeMethod;
use rsift_api::native_loader::load_mods_into_runtime;
use rsift_api::runtime::runtime_or_init;
use rsift_api::ui_ext::ScreenButtonDescriptor;
use rsift_parser::BytecodePatcher;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::agent_log::{agent_log, agent_log_err, agent_log_step, agent_log_warn};
use crate::agent_opts;

static INJECTED_SCREENS: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
static DEFERRED_STARTED: AtomicBool = AtomicBool::new(false);
/// wave 207 HC-1: AddToBootstrapClassLoaderSearch は 1 回だけ (冪等化 static)。
static BOOTSTRAP_CLASSPATH_ADDED: AtomicBool = AtomicBool::new(false);
static MODS_LOADED: AtomicBool = AtomicBool::new(false);
static AGENTPATH_LOADED: AtomicBool = AtomicBool::new(false);
static TRANSFORM_LOG_COUNT: AtomicU64 = AtomicU64::new(0);
/// 生存中 JavaVM 実体アドレス (0 = 未取得)。wave 205: 早期 CFLH install と
/// GetLoadedClasses 掃引が JVMTI を直接叩くための共用一次情報。
static JAVA_VM_ADDR: AtomicUsize = AtomicUsize::new(0);
static TITLE_MARKER_SET: AtomicBool = AtomicBool::new(false);

/// JavaVM 実体アドレス (未取得なら null)。JVMTI を直接使う側 (screen_inject) が参照。
pub fn java_vm_addr() -> *mut std::ffi::c_void {
    JAVA_VM_ADDR.load(Ordering::SeqCst) as *mut std::ffi::c_void
}

const TICK_ACTIVE_MS: u64 = 250;
const MAX_INJECT_TICKS: u32 = 600;

fn injected_set() -> std::sync::MutexGuard<'static, Option<HashSet<usize>>> {
    let mut g = INJECTED_SCREENS.lock().unwrap();
    if g.is_none() {
        *g = Some(HashSet::new());
    }
    g
}

pub(crate) fn is_screen_injected(key: usize) -> bool {
    injected_set()
        .as_ref()
        .map(|s| s.contains(&key))
        .unwrap_or(false)
}

pub(crate) fn mark_screen_injected(key: usize) {
    if let Some(set) = injected_set().as_mut() {
        set.insert(key);
    }
}

pub fn agent_premain(agent_args: &str) {
    if MODS_LOADED.swap(true, Ordering::SeqCst) {
        return;
    }
    // wave 219 HP (P1 根治): NativeLoader の個別失敗 (Dynamic linker error 等)
    // を本 bootstrap ログへ橋渡し — 実機 #5 は logger 未初期化で全行蒸発し
    // 「mods loaded OK: []」の原因が 0 行だった (登録は何度呼ばれても初回のみ
    // 有効 = 冪等)。
    rsift_api::log_bridge::set_agent_log_sink(agent_log);
    agent_log_step(
        "agent_premain",
        &format!("loading mods args={}", agent_args),
    );
    let mod_dir = agent_opts::resolve_mod_dir(agent_args);
    agent_log_step("agent_premain", &format!("mod_dir={:?}", mod_dir));
    // wave 210 HF: mods パイプラインの可観測性 + 公式 mod 自動復旧 (存在検査の
    // **前**に置く — mods dir 自体が削除された実機でも自壊復旧できるように)。
    // 実機では mods フォルダ空の `mods loaded OK: []` が再現され、setup 側
    // Prism 配備ログ欠落と併せて切り分け不能だった (launch ログ #4/#5)。
    // ここでは候補列挙を毎回記録し、mods が欠けている場合は rsift home
    // (natives = この agent dll の所在) から固定 3 名だけを**非破壊**コピー
    // する (既存上書き禁止・marker 冪等・modsec ゲートは通常どおり適用)。
    match rsift_api::native_loader::discover_mod_paths(&mod_dir) {
        Ok(paths) => {
            let names: Vec<String> = paths
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .collect();
            agent_log_step(
                "agent_premain",
                &format!("mod candidates ({}): {:?}", names.len(), names),
            );
        }
        Err(e) => agent_log_warn("agent_premain", &format!("mod candidate scan failed: {e}")),
    }
    // wave 211 HG: 復旧源を複数化 (rsift home → バニラ .minecraft/mods →
    // バニラ version dir)。実機ユーザーは dll を手動差替えする運用履歴があり
    // (ログ #2→#5 の dll size 遷移)、natives に公式 mod が無い実機では
    // 単一源のままでは復旧できない。バニラ側は setup の launcher flow が
    // 配備済み (ログ #2 で機械確認) のため、そこから拾えば実機が収束する。
    let restore_sources = rsift_api::native_loader::default_restore_source_dirs(
        agent_opts::dll_directory().as_deref(),
    );
    match rsift_api::native_loader::restore_official_mods_search(&mod_dir, &restore_sources) {
        Ok(out) => {
            if !out.restored.is_empty() {
                agent_log_step(
                    "agent_premain",
                    &format!(
                        "official mods auto-restored from {}: {:?}",
                        out.source_dir
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "(unknown)".to_string()),
                        out.restored
                    ),
                );
            } else if let Some(reason) = out.skip_reason {
                agent_log_step("agent_premain", &format!("official mod restore: {reason}"));
            }
        }
        Err(e) => agent_log_warn(
            "agent_premain",
            &format!("official mod restore failed (non-fatal): {e}"),
        ),
    }
    if !mod_dir.exists() {
        agent_log("[RsiftAgent] WARN mod_dir does not exist");
        return;
    }
    let rt = runtime_or_init(mod_dir.clone());
    rt.set_mod_dir(mod_dir.clone());
    // ABI 安定テーブルを実インストール (ネイティブ DLL Mod への受け渡し口を有効化)。
    let abi = crate::abi_stable::install_api();
    agent_log_step(
        "agent_premain",
        &format!(
            "abi table installed v{}.{}.{}",
            abi.version.major, abi.version.minor, abi.version.patch
        ),
    );
    let world_dir = mod_dir
        .parent()
        .map(|p| p.join("saves").join("rsift_sim"))
        .unwrap_or_else(|| mod_dir.join("rsift_sim"));
    rsift_sim::init_global_sim(&world_dir);
    agent_log_step(
        "agent_premain",
        &format!(
            "sim runtime ready backend={} cores={}",
            rsift_sim::io_backend_name(),
            rsift_sim::CpuFeatures::detect().cores
        ),
    );
    match load_mods_into_runtime(&mod_dir, rt, true) {
        Ok(result) => {
            agent_log_step(
                "agent_premain",
                &format!("mods loaded OK: {:?}", result.loaded),
            );
            // wave 210 HF: ロード 0 件かつフォルダ自体が空の場合は、非エンジニア
            // でも自己解決できる日本語ガイド行を残す (沈黙の「空 = 正常」誤認を根治)。
            // ファイルがあるのに 0 件ロードの場合は modsec/リンカ由来の個別
            // エラーが既に残っているのでガイドは出さない (誤誘導防止)。
            if result.loaded.is_empty() {
                let still_empty = rsift_api::native_loader::discover_mod_paths(&mod_dir)
                    .map(|p| p.is_empty())
                    .unwrap_or(false);
                if still_empty {
                    agent_log_step(
                        "agent_premain",
                        &format!(
                            "mods フォルダにロード可能な mod がありません: {} — 公式 mod (rsgraphics/rsreplay/rszoom) を使うには rsift-setup を再実行するか、mod ファイルをこのフォルダにコピーしてください",
                            mod_dir.display()
                        ),
                    );
                }
            }
        }
        Err(e) => agent_log_err("agent_premain", &format!("mod load FAILED: {}", e)),
    }
}

/// Load mod DLLs as early as possible (no JNI / Minecraft classes required).
pub fn agent_load_mods_early(agent_args: &str) {
    agent_premain(agent_args);
}

pub fn agentpath_active() -> bool {
    AGENTPATH_LOADED.load(Ordering::SeqCst)
}

pub fn mark_agentpath_loaded() {
    AGENTPATH_LOADED.store(true, Ordering::SeqCst);
    if let Some(dir) = crate::agent_opts::dll_directory() {
        let marker = dir.join(".rsift-agentpath-active");
        let _ = std::fs::write(&marker, b"1");
        crate::agent_log::agent_log_step("agentpath", &format!("marker written {:?}", marker));
    }
}

/// Defer JNI bridge until Minecraft ClassLoader exists. `from_agentpath` = primary boot path.
pub fn schedule_deferred_init(vm: *mut std::ffi::c_void, options: &str, from_agentpath: bool) {
    agent_log_step(
        "schedule_deferred_init",
        &format!(
            "from_agentpath={} opts_len={} agentpath_active={}",
            from_agentpath,
            options.len(),
            AGENTPATH_LOADED.load(Ordering::SeqCst)
        ),
    );
    if from_agentpath {
        mark_agentpath_loaded();
    } else if AGENTPATH_LOADED.load(Ordering::SeqCst) {
        agent_log_step(
            "schedule_deferred_init",
            "skipped — agentpath already owns boot",
        );
        return;
    }
    if DEFERRED_STARTED.swap(true, Ordering::SeqCst) {
        agent_log_warn(
            "schedule_deferred_init",
            "already started — ignoring duplicate call",
        );
        return;
    }
    let opts = options.to_string();
    let vm_addr = vm as usize;
    JAVA_VM_ADDR.store(vm_addr, Ordering::SeqCst);
    match std::thread::Builder::new()
        .name("rsift-deferred-init".into())
        // Agent JNI probes (getAllStackTraces) need more stack than the Windows default (~1 MiB).
        .stack_size(4 * 1024 * 1024)
        .spawn(move || {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                deferred_init_main(vm_addr, &opts);
            }));
            if let Err(payload) = run {
                agent_log_err(
                    "deferred_init_thread",
                    &format!("PANIC: {:?}", payload.downcast_ref::<&str>()),
                );
            }
        }) {
        Ok(_) => agent_log_step("schedule_deferred_init", "thread spawn OK"),
        Err(e) => agent_log_err(
            "schedule_deferred_init",
            &format!("thread spawn FAILED: {:?}", e),
        ),
    }
}

fn deferred_init_main(vm_addr: usize, opts: &str) {
    agent_log_step("deferred_init", "thread entered");
    agent_log_step(
        "deferred_init",
        "brief wait before JNI (agentpath-only, no javaagent)",
    );
    std::thread::sleep(Duration::from_secs(1));
    agent_log_step("deferred_init", "pre-JNI wait complete");
    if let Some(dir) = crate::agent_opts::dll_directory() {
        agent_log_step(
            "boot_markers",
            &format!(
                "agentpath={} javaagent={}",
                dir.join(".rsift-agentpath-active").is_file(),
                dir.join(".rsift-javaagent-ok").is_file()
            ),
        );
    }

    agent_log_step("deferred_init", "loading mods");
    if !MODS_LOADED.load(Ordering::SeqCst) {
        agent_load_mods_early(opts);
    } else {
        agent_log_step("deferred_init", "mods already loaded — skip");
    }

    agent_log_step(
        "deferred_init",
        &format!("JavaVM::from_raw addr=0x{:x}", vm_addr),
    );
    let vm = match unsafe { jni::JavaVM::from_raw(vm_addr as *mut jni::sys::JavaVM) } {
        Ok(v) => v,
        Err(e) => {
            agent_log_err(
                "deferred_init",
                &format!("JavaVM::from_raw failed: {:?}", e),
            );
            return;
        }
    };
    // wave HR (#5 根治基盤): CFLH install より**前に** obf_map モードを確定する。
    // ClassLoad コールバックが難読名で jcache 照合する際に resolve が必要なため、
    // obf_map は最初の CFLH/ClassLoad 発火前に install 済みでなければならない。
    // client.txt 不在/失敗は Unobfuscated (mojmap 恒等 = 従来挙動) へ安全落下。
    // (JNIEnv 不要・dll_dir のみで確定できるため attach ループの外で実行)
    if let Some(dir) = crate::agent_opts::dll_directory() {
        match crate::obf_map::install_from_dir(&dir) {
            Ok(mode) => agent_log_step(
                "deferred_init",
                &format!("obf_map mode = {:?}", mode),
            ),
            Err(e) => agent_log_err(
                "deferred_init",
                &format!("obf_map install_from_dir failed: {} (continuing Unobfuscated)", e),
            ),
        }
    } else {
        // dll_dir 不明でも安全側へ (Unobfuscated install を試みる)。
        let _ = crate::obf_map::install_from_dir(std::path::Path::new("."));
    }
    for attempt in 0..300 {
        match vm.attach_current_thread() {
            Ok(mut env) => {
                if attempt == 0 || attempt % 5 == 0 {
                    agent_log_step(
                        "classloader_probe",
                        &format!("attempt {} attach OK", attempt),
                    );
                }
                // wave 206 実機検証で判明 (HA-2): CFLH install (= GetEnv JVMTI)
                // を**未アタッチのネイティブスレッド**から呼ぶと JVM ごと
                // SIGSEGV する (Temurin 25 実機で確認: jni_GetEnv →
                // JvmtiExport::get_jvmti_interface が現在スレッド Thread*
                // (unattached = nullptr) を null+0x52c でデリファレンス)。
                // そのため install は「attach 成功直後・loader 探索より前」に
                // 置く。HOOK_INSTALLED 冪等で先着1回。attach 済みスレッドから
                // であれば GetEnv(JVMTI) は安全。loader 探索に先立って install
                // されるので捕捉の鮮度 (wave 205 欠陥B の意図) は保たれる。
                let hook_ok =
                    unsafe { jvmti_events::install_class_file_load_hook(vm_addr as *mut _) };
                // wave 209 HE: f3 静的 spec を「CFLH 稼働直後 = 対象クラスの
                // 自然ロードより必ず前」のタイミングで登録する。ハーネス機械確認
                // (wave 209 RED→GREEN): post_bridge_boot 時点の登録だと
                // DebugScreenEntryList が先に定義されてしまい CFLH の TAIL 注入が
                // 永久に間に合わず f3 不発 — 登録は CFLH install 成功直後が
                // **唯一正しい位置**。先着 1 回冪等。
                if hook_ok {
                    preregister_static_f3_specs();
                }
                if attempt == 0 || (!hook_ok && attempt % 25 == 0) {
                    agent_log_step(
                        "deferred_init",
                        &format!(
                            "JVMTI CFLH install (attached thread, attempt {}) = {}",
                            attempt, hook_ok
                        ),
                    );
                }
                // wave 207 HC-1: HEAD 注入先 com/rsift/RsiftHooks をゲームの
                // 全 ClassLoader から解決可能にするため、bootstrap CL 検索パスへ
                // rsift-bootstrap.jar を追加する (jvmti.xml num=149,
                // capability 不要)。呼出は初回 install 成功時の 1 回だけ。
                // 「以前に解決失敗した symbolic reference は同じエラーで失敗
                // し続ける」(vmspec 5.3.1) ため、hook クラス解決が起こりうる
                // メソッド実行より前 (install 直後) に入れる必要がある。
                // wave 207 HC-1: Agent_OnLoad での bootstrap classpath 追加
                // が失敗/未到達 (dll dir 未取得等) の場合のフォールバック。
                // 本体の規格位置は onload 相 (jvmti_hook.rs) — ここは live 相
                // なので WRONG_PHASE 環境では失敗するが、コスト 0 の再試行。
                if hook_ok && !jvmti_events::BOOTSTRAP_CLASSPATH_ADDED.load(Ordering::SeqCst) {
                    let _ = unsafe {
                        jvmti_events::ensure_bootstrap_classpath_once(
                            vm_addr as *mut _,
                            "CFLH install fallback",
                        )
                    };
                }
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    screen_inject::find_game_class_loader_verbose(&mut env, attempt)
                })) {
                    Ok(Some(_)) => {
                        agent_log_step(
                            "classloader_probe",
                            &format!("FOUND on attempt {}", attempt),
                        );
                        let bridge_ok = screen_inject::ensure_injector_loaded(&mut env);
                        let mod_ok = mod_bridge::ensure_mod_bridge(&mut env);
                        let _ = platform_bridge::ensure_platform_bridge(&mut env);
                        // CFLH は attach ループ先頭で install 済 (wave 205 前倒し
                        // → wave 206 修正: 未アタッチからの GetEnv は SIGSEGV
                        // するため attach 成功直後に移動、冪等)。
                        register_transformer_natives(&mut env);
                        register_hooks_natives(&mut env);
                        notify_transformer_ready(&mut env);
                        // Sync CoreMod → Mixin rules into the patcher
                        for rule in rsift_api::neoforge_coremod::all_coremod_rules() {
                            rsift_parser::register_dynamic_target(&rule.target_class);
                            rsift_parser::register_mixin_rule(rsift_parser::MixinRule {
                                target_class: rule.target_class.clone(),
                                target_method: rule.target_method.clone(),
                                target_descriptor: rule.target_descriptor.clone(),
                                action: match rule.action {
                                    rsift_api::neoforge_coremod::CoreModAction::Inject { point, dll_symbol } => {
                                        rsift_parser::MixinAction::Inject {
                                            point: match point {
                                                rsift_api::neoforge_coremod::CoreModInjectionPoint::Head => {
                                                    rsift_parser::InjectionPoint::Head
                                                }
                                                rsift_api::neoforge_coremod::CoreModInjectionPoint::Return => {
                                                    rsift_parser::InjectionPoint::Return
                                                }
                                                rsift_api::neoforge_coremod::CoreModInjectionPoint::Invoke(m) => {
                                                    rsift_parser::InjectionPoint::Invoke {
                                                        target_method: m,
                                                        shift: rsift_parser::InjectionShift::Before,
                                                    }
                                                }
                                            },
                                            dll_symbol,
                                            cancellable: false,
                                        }
                                    }
                                    rsift_api::neoforge_coremod::CoreModAction::Redirect { point, dll_symbol } => {
                                        rsift_parser::MixinAction::Redirect {
                                            point: match point {
                                                rsift_api::neoforge_coremod::CoreModInjectionPoint::Head => {
                                                    rsift_parser::InjectionPoint::Head
                                                }
                                                rsift_api::neoforge_coremod::CoreModInjectionPoint::Return => {
                                                    rsift_parser::InjectionPoint::Return
                                                }
                                                rsift_api::neoforge_coremod::CoreModInjectionPoint::Invoke(m) => {
                                                    rsift_parser::InjectionPoint::Invoke {
                                                        target_method: m,
                                                        shift: rsift_parser::InjectionShift::Before,
                                                    }
                                                }
                                            },
                                            dll_symbol,
                                        }
                                    }
                                    rsift_api::neoforge_coremod::CoreModAction::Overwrite { dll_symbol } => {
                                        rsift_parser::MixinAction::Overwrite { dll_symbol }
                                    }
                                },
                            });
                        }
                        agent_log_step(
                            "bridge_load",
                            &format!("UiBridge={} ModBridge={}", bridge_ok, mod_ok),
                        );
                        // wave 205: ブリッジ準備完了後の後追い処理 —
                        //   (1) 既ロード済みパッチ対象クラスへの Retransform 追撃
                        //   (2) F3 マーカー picker 登録 + 対象クラス Retransform
                        //   (3) ウィンドウタイトル・マーカー (「Vanilla判定」への直接応答)
                        post_bridge_boot(&mut env, vm_addr);
                        break;
                    }
                    Ok(None) => {
                        if attempt == 0 || attempt % 5 == 0 {
                            agent_log_step(
                                "classloader_probe",
                                &format!("attempt {} — not ready yet", attempt),
                            );
                        }
                    }
                    Err(_) => {
                        agent_log_err(
                            "classloader_probe",
                            &format!("attempt {} — Rust panic during probe", attempt),
                        );
                    }
                }
            }
            Err(e) => {
                agent_log_err(
                    "classloader_probe",
                    &format!("attempt {} attach FAILED: {:?}", attempt, e),
                );
            }
        }
        if attempt == 299 {
            agent_log_warn("classloader_probe", "game ClassLoader not ready after 300s");
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    agent_log_step("deferred_init", "entering render/tick loop");

    let mut tick: u32 = 0;
    loop {
        let sleep_ms = if tick % 4 == 0 {
            TICK_ACTIVE_MS
        } else {
            rsift_api::runtime::runtime()
                .map(|rt| {
                    if rt.has_render_handlers() {
                        16
                    } else {
                        TICK_ACTIVE_MS
                    }
                })
                .unwrap_or(TICK_ACTIVE_MS)
        };
        std::thread::sleep(Duration::from_millis(sleep_ms));
        if let Ok(mut env) = vm.attach_current_thread() {
            screen_inject::ensure_injector_loaded(&mut env);
            mod_bridge::ensure_mod_bridge(&mut env);
            client_tick(&mut env);
            if tick % 4 == 0 {
                mod_bridge::client_game_tick(&mut env);
            } else {
                mod_bridge::dispatch_render_only(&mut env);
            }
            if tick == 0 || tick % 120 == 0 {
                agent_log_step("tick_loop", &format!("alive tick={}", tick));
            }
        } else if tick % 120 == 0 {
            agent_log_warn("tick_loop", &format!("attach failed at tick={}", tick));
        }
        tick = tick.saturating_add(1);
    }
}

// ----------------------------------------------------------------------
// wave 205: ブリッジ準備完了後の後追い処理群
// ----------------------------------------------------------------------

/// (1) F3 静的 spec 先行登録 (wave 208 HD: RefTrans 非依存化のため最優先 —
/// sweep が dynamic target を loadClass する **前** に spec を登録しておくと
/// そのロード自体が CFLH でパッチされる) (2) Retransform 追撃
/// (3) タイトルマーカー。個別失敗はログのみでゲーム継続 (どれも致命傷でない設計)。
fn post_bridge_boot(env: &mut JNIEnv, vm_addr: usize) {
    install_f3_marker(env, vm_addr);
    retransform_loaded_targets(env, vm_addr);
    dump_loaded_classes(env, vm_addr);
    // タイトルは client_tick 側でもリトライするのでここは最善努力。
    if let Some(inst) = screen_inject::minecraft_instance(env) {
        maybe_set_window_title(env, &inst);
    } else {
        screen_inject::clear_pending_exception(env);
    }
}

/// install より前にロード済みのパッチ対象クラスへ後追いでフックを当てる。
/// RetransformClasses は CFLH を「元バイト列」で再発火させるため、
/// 登録済みターゲットは patch_if_needed 経由で正しくパッチされる。
fn retransform_loaded_targets(env: &mut JNIEnv, vm_addr: usize) {
    let Some(loader) = screen_inject::game_class_loader(env) else {
        agent_log_warn("retransform", "no game loader — sweep skipped");
        return;
    };
    let mut names: Vec<String> = rsift_parser::static_target_classes()
        .iter()
        .map(|s| s.to_string())
        .collect();
    names.extend(rsift_parser::dynamic_targets_snapshot());
    names.sort();
    names.dedup();
    let mut classes: Vec<*mut std::ffi::c_void> = Vec::new();
    let mut locals: Vec<JClass> = Vec::new(); // local ref 生存保持 (retransform 完了まで)
    for internal in &names {
        let dotted = internal.replace('/', ".");
        match screen_inject::load_class_with_loader(env, &loader, &dotted) {
            Some(cls) => {
                classes.push(cls.as_raw() as *mut _);
                locals.push(cls);
            }
            None => {
                // wave 207 診断強化: static target が loader 経路で拾えない
                // 理由 (pending exception の class+message) を 1 行で残す。
                // 「no statically targeted classes loaded yet」しか見えない
                // #4 の状況を二度と作らない。
                screen_inject::dump_pending_exception(
                    env,
                    &format!("retransform sweep: could not obtain jclass for {}", dotted),
                );
                screen_inject::clear_pending_exception(env);
            }
        }
    }
    if classes.is_empty() {
        agent_log_step(
            "retransform",
            "no statically targeted classes loaded yet — nothing to retransform",
        );
        return;
    }
    // SAFETY: vm_addr は生存中 JavaVM、classes は本スレッド attach 上の有効な jclass。
    let rc = unsafe { jvmti_events::retransform_classes(vm_addr as *mut _, &classes) };
    agent_log_step(
        "retransform",
        &format!(
            "RetransformClasses rc={} ({} of {} target classes already loaded)",
            rc,
            classes.len(),
            names.len()
        ),
    );
    drop(locals);
}


/// 全ロード済みクラスを JVMTI GetLoadedClasses で列挙し、短い名前(難読化クラス)
/// + com/mojang + net/minecraft をログにダンプする。1回のみ。
fn dump_loaded_classes(env: &mut JNIEnv, vm_addr: usize) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DUMPED: AtomicBool = AtomicBool::new(false);
    if DUMPED.swap(true, Ordering::SeqCst) {
        return;
    }
    let vm_raw = vm_addr as *mut std::ffi::c_void;
    let classes = match unsafe { jvmti_events::get_loaded_classes(vm_raw) } {
        Some(c) => c,
        None => {
            agent_log_warn("class_dump", "GetLoadedClasses failed");
            return;
        }
    };
    let mut interesting: Vec<String> = Vec::new();
    for jclass_raw in &classes {
        if jclass_raw.is_null() { continue; }
        let obj = unsafe { JObject::from_raw(*jclass_raw as jni::sys::jobject) };
        let name_result = env.call_method(&obj, "getName", "()Ljava/lang/String;", &[]).and_then(|v| v.l());
        screen_inject::clear_pending_exception(env);
        if let Ok(name_obj) = name_result {
            let name: String = env.get_string((&name_obj).into()).map(|s| s.into()).unwrap_or_default();
            let is_short_obf = name.len() <= 8 && !name.contains('$') && !name.contains('/');
            let is_mc = name.starts_with("net.minecraft.") || name.starts_with("com.mojang.");
            if is_short_obf || is_mc {
                let mojmap = crate::obf_map::resolve_class_reverse(&name)
                    .map(|m| format!(" → {}", m)).unwrap_or_default();
                interesting.push(format!("{}{}", name, mojmap));
            }
        }
    }
    interesting.sort();
    agent_log_step("class_dump", &format!("{} interesting classes loaded (of {} total)", interesting.len(), classes.len()));
    for name in &interesting {
        agent_log(&format!("[class_dump] {}", name));
    }
}

/// wave 209 HE: 1.21.11 確定マッピングの静的先行登録 (クラスをロードしない)。
///
/// 経緯の正直な記録: wave 208 でこの仕組みは「permission boundary が広がる」
/// と判断して見送られたが、それは rsift-parser 側テーブル編集の失敗を
/// 誤帰因したものだった — **本関数は register_f3_marker (公開 API、まさに
/// この用途) を agent 側から呼ぶだけで、patcher.rs の first-match 権限
/// テーブルには一切触れない**。「反射で spec を特定 → その瞬間にクラスが
/// 強制ロード (= 未パッチ) → RefransformClasses 必須」という構造的依存を
/// 断つために、CFLH が自然ロード時に TAIL 注入できるよう **ロード前に**
/// spec を登録する。spec 名が実クラスと不一致なら CFH は no-op で安全、
/// 反射経路が真名を後追い登録して従来通り Refransform を試みる
/// (両経路は冪等に共存)。RefTrans が決して効かない VM (prism ラッパー
/// 配下含む) でも F3 マーカーが成立する。
fn preregister_static_f3_specs() {
    use rsift_parser::ListElement;
    use std::sync::atomic::{AtomicBool, Ordering};
    static PREREGISTERED: AtomicBool = AtomicBool::new(false);
    if PREREGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    // wave HR (#5 Phase 2): クラス名 + getLines メソッド名を難読解決して CFLH spec へ登録。
    // CFLH は**実行時名 (難読名)** でロードクラスと照合するため、spec も実行時名でなければ
    // 一致しない (= 難読化版で F3 不発の真因)。Unobfuscated/未 install は原名へ安全落下。
    const MOJMAP_CLASS: &str = "net.minecraft.client.gui.components.debug.DebugScreenEntryList";
    let internal = crate::obf_map::resolve_class_internal(MOJMAP_CLASS)
        .unwrap_or_else(|| {
            "net/minecraft/client/gui/components/debug/DebugScreenEntryList".to_string()
        });
    let method = crate::obf_map::resolve_method_by_name(MOJMAP_CLASS, "getLines")
        .unwrap_or_else(|| "getLines".to_string());
    rsift_parser::register_f3_marker(&internal, &method, "()Ljava/util/List;", ListElement::String);
    agent_log_step(
        "f3_marker",
        &format!(
            "static spec pre-registered (load-free, reflection-independent): {}#{} ()Ljava/util/List; (mojmap DebugScreenEntryList#getLines)",
            internal, method
        ),
    );
}

/// F3 マーカー (wave 205 ユーザー提案採用): 実行時リフレクションで
/// 「0 引数・java.util.List 返り値・要素型 = String または Component」の
/// F3 行メソッドを特定 → TAIL 注入 spec を登録 → 対象クラスを Retransform。
/// 要件に合うメソッドが無い場合は理由ログつきで静黙スキップ (嘘の注入禁止)。
fn install_f3_marker(env: &mut JNIEnv, vm_addr: usize) {
    // wave 209 HE: 静的 spec を「ロード前に」登録 = RefTrans 非依存化
    // (詳細は preregister_static_f3_specs の doc)。引き続き反射経路は
    // 真名検証+後追い Refransform のフォールバックとして機能する。
    preregister_static_f3_specs();
    let Some(loader) = screen_inject::game_class_loader(env) else {
        agent_log_warn("f3_marker", "no game loader — F3 marker skipped");
        return;
    };
    // 1.21.11 実在確認済 (optifine srg patch 一覧 = wave 205 一次情報)。
    const CANDIDATE_CLASSES: &[&str] = &[
        "net.minecraft.client.gui.components.debug.DebugScreenEntryList",
        "net.minecraft.client.gui.components.DebugScreenOverlay",
        "net.minecraft.client.gui.components.debug.DebugScreenOverlay",
    ];
    for dotted in CANDIDATE_CLASSES {
        let Some(cls) = screen_inject::load_class_with_loader(env, &loader, dotted) else {
            screen_inject::clear_pending_exception(env);
            continue;
        };
        let Some((method, element)) = pick_f3_lines_method(env, &cls) else {
            screen_inject::clear_pending_exception(env);
            continue;
        };
        let internal = dotted.replace('.', "/");
        rsift_parser::register_f3_marker(&internal, &method, "()Ljava/util/List;", element);
        rsift_parser::register_dynamic_target(&internal);
        agent_log_step(
            "f3_marker",
            &format!("registered on {}#{} ({:?})", internal, method, element),
        );
        // picker が対象クラスをロード済み (= 未パッチ状態) なので後追い Retransform。
        let raw = cls.as_raw() as *mut std::ffi::c_void;
        // SAFETY: 生存中 JavaVM + 本スレッド有効な jclass。
        let rc = unsafe { jvmti_events::retransform_classes(vm_addr as *mut _, &[raw]) };
        agent_log_step(
            "f3_marker",
            &format!("RetransformClasses rc={} for {}", rc, internal),
        );
        return;
    }
    agent_log_step(
        "f3_marker",
        "no F3-lines method (0-arg, List<String|Component>) found via reflection — \
         F3 marker skipped honestly (window title marker remains)",
    );
}

/// 1 メソッドを検査して F3 行メソッド候補なら Some((name, element))。
/// 失敗・不適合は None。pending 例外は呼び出し側でクリアする。
fn inspect_f3_candidate_method<'local>(
    env: &mut JNIEnv<'local>,
    arr: &jni::objects::JObjectArray<'local>,
    index: jni::sys::jsize,
) -> Option<(String, rsift_parser::ListElement)> {
    let m = env.get_object_array_element(arr, index).ok()?;
    let pc = env
        .call_method(&m, "getParameterCount", "()I", &[])
        .ok()?
        .i()
        .ok()?;
    if pc != 0 {
        return None;
    }
    let mods = env
        .call_method(&m, "getModifiers", "()I", &[])
        .ok()?
        .i()
        .ok()?;
    if mods & (0x0400 | 0x0100) != 0 {
        // abstract | native は注入不可
        return None;
    }
    let rt = env
        .call_method(&m, "getReturnType", "()Ljava/lang/Class;", &[])
        .ok()?
        .l()
        .ok()?;
    let rtn = call_string_method(env, &rt, "getName", "()Ljava/lang/String;")?;
    if rtn != "java.util.List" {
        return None;
    }
    let gt = env
        .call_method(
            &m,
            "getGenericReturnType",
            "()Ljava/lang/reflect/Type;",
            &[],
        )
        .ok()?
        .l()
        .ok()?;
    let gts = call_string_method(env, &gt, "toString", "()Ljava/lang/String;")?;
    let element = classify_list_element(&gts)?;
    let name = call_string_method(env, &m, "getName", "()Ljava/lang/String;")?;
    Some((name, element))
}

/// クラスの宣言メソッドから F3 行メソッドを 1 本選ぶ。
/// 複数候補は名前ヒント (lines > f3 > info) でスコアリング、同点は名前順。
fn pick_f3_lines_method<'local>(
    env: &mut JNIEnv<'local>,
    cls: &JClass<'local>,
) -> Option<(String, rsift_parser::ListElement)> {
    let methods_obj = env
        .call_method(
            cls,
            "getDeclaredMethods",
            "()[Ljava/lang/reflect/Method;",
            &[],
        )
        .ok()?
        .l()
        .ok()?;
    // SAFETY: getDeclaredMethods は Method[] を返す (非 null)。
    let arr = unsafe {
        jni::objects::JObjectArray::from_raw(methods_obj.as_raw() as jni::sys::jobjectArray)
    };
    let count = env.get_array_length(&arr).ok()?;
    let mut candidates: Vec<(String, rsift_parser::ListElement)> = Vec::new();
    for i in 0..count {
        match inspect_f3_candidate_method(env, &arr, i) {
            Some(c) => candidates.push(c),
            None => screen_inject::clear_pending_exception(env),
        }
    }
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    if candidates.len() > 1 {
        let names: Vec<String> = candidates.iter().map(|c| c.0.clone()).collect();
        agent_log_step(
            "f3_marker",
            &format!(
                "{} candidates [{}] — scoring by name hints",
                candidates.len(),
                names.join(", ")
            ),
        );
    }
    candidates
        .into_iter()
        .max_by_key(|(n, _)| f3_method_name_score(n))
}

/// 名前ヒントのスコア (純粋関数 — テスト可能)。
fn f3_method_name_score(name: &str) -> u32 {
    let l = name.to_ascii_lowercase();
    l.contains("lines") as u32 * 4 + l.contains("f3") as u32 * 2 + l.contains("info") as u32
}

/// ジェネリクス戻り値の toString から List 要素型を判別 (純粋関数 — テスト可能)。
/// 判別不能 (raw List / 未知要素 / DebugScreenEntry 等) は None — 注入しない決定。
/// List<DebugScreenEntry> 等への String 混入は消費側キャストで CCE になるため。
fn classify_list_element(generic: &str) -> Option<rsift_parser::ListElement> {
    if generic.contains("java.lang.String") {
        Some(rsift_parser::ListElement::String)
    } else if generic.contains("net.minecraft.network.chat.Component")
        || generic.contains("MutableComponent")
    {
        Some(rsift_parser::ListElement::Component)
    } else {
        None
    }
}

fn call_string_method(env: &mut JNIEnv, obj: &JObject, name: &str, sig: &str) -> Option<String> {
    let v = env.call_method(obj, name, sig, &[]).ok()?;
    let o = v.l().ok()?;
    env.get_string((&o).into()).ok().map(|s| s.into())
}

/// 「Vanilla 判定」への直接応答 (wave 205): ウィンドウタイトルに Rsift 表記。
/// バニラ様式の `Minecraft* <version>` 接尾 (Mod UI はマイクラ味方針に整合)。
/// client_tick から繰り返し呼ばれる前提で成功時のみフラグを立てて冪等化。
fn maybe_set_window_title(env: &mut JNIEnv, inst: &JObject) {
    // wave HR: MC がロード中に setTitle で上書きするため定期再適用 (200ティック ≈ 50秒毎)
    use std::sync::atomic::AtomicU32;
    static TITLE_TICK: AtomicU32 = AtomicU32::new(0);
    let n = TITLE_TICK.fetch_add(1, Ordering::Relaxed);
    if TITLE_MARKER_SET.load(Ordering::SeqCst) && n % 200 != 0 {
        return;
    }
    let run = (|| -> Result<(), String> {
        if inst.as_raw().is_null() {
            return Err("Minecraft instance is null (client not ready)".into());
        }
        // wave HR (#5 Phase 2): getWindow (Minecraft) のメソッド名 + Window descriptor 解決。
        let gw = crate::obf_map::resolve_method_by_name("net.minecraft.client.Minecraft", "getWindow")
            .unwrap_or_else(|| "getWindow".to_string());
        let gw_desc = crate::obf_map::resolve_descriptor("()Lcom/mojang/blaze3d/platform/Window;");
        let window = env
            .call_method(inst, &gw, &gw_desc, &[])
            .and_then(|v| v.l())
            .map_err(|e| format!("getWindow: {:?}", e))?;
        if window.as_raw().is_null() {
            return Err("getWindow returned null".into());
        }
        let title = format!(
            "Minecraft* {} - Rsift (RsGraphics Render)",
            rsift_api::TARGET_MINECRAFT_VERSION
        );
        let title_j = env
            .new_string(&title)
            .map_err(|e| format!("new_string: {:?}", e))?;
        let st = crate::obf_map::resolve_method_by_name(
            "com.mojang.blaze3d.platform.Window",
            "setTitle",
        )
        .unwrap_or_else(|| "setTitle".to_string());
        env.call_method(&window, &st, "(Ljava/lang/String;)V", &[JValue::Object(&title_j)])
            .map_err(|e| format!("setTitle: {:?}", e))?;
        agent_log_step("title_marker", &format!("window title set: {}", title));
        Ok(())
    })();
    match run {
        Ok(()) => TITLE_MARKER_SET.store(true, Ordering::SeqCst),
        Err(e) => {
            screen_inject::clear_pending_exception(env);
            // wave HS (#9 spam抑制): MC ロード中は instance null で毎tick失敗する。
            // 失敗ログは 1 回目 + 200 回毎のみ (成功すれば throttle が効くまで数行)。
            if n == 0 || n % 200 == 0 {
                agent_log_step("title_marker", &format!("skipped this attempt (tick={}): {}", n, e));
            }
        }
    }
}

pub fn transform_class(class_name: &str, data: &[u8]) -> Option<Vec<u8>> {
    // wave HR (renderer): 読込クラス名(難読)を mojmap へ正規化してからパッチャへ渡す。
    let internal = crate::obf_map::normalize_to_mojmap_internal(&class_name.replace('.', "/"));
    // Sync CoreMod rules into the bytecode patcher target set.
    for rule in rsift_api::neoforge_coremod::all_coremod_rules() {
        rsift_parser::register_dynamic_target(&rule.target_class);
    }
    let patched = match BytecodePatcher::patch_if_needed(&internal, data) {
        Ok(r) if r.was_modified && !r.new_bytecode.is_empty() => Some(r.new_bytecode),
        _ => None,
    };

    // 第2パス (実配線): bytecode_transpiler の HEAD 挿入を ParserEngine の
    // メモ化経由で実適用 (JVMTI Retransform 反復時も注入は 1 回で済む)。
    let transpiled = {
        let base: &[u8] = patched.as_deref().unwrap_or(data);
        let tp = crate::bytecode_transpiler::BytecodeTranspiler::new();
        let mut engine = TRANSPILE_ENGINE
            .get_or_init(|| std::sync::Mutex::new(rsift_parser::ParserEngine::new()))
            .lock()
            .unwrap();
        let internal_cp = internal.clone();
        engine.transform(&internal_cp, base, |cf| {
            let mut applied = 0usize;
            for rule in tp.rules_for(&internal_cp) {
                applied +=
                    cf.inject_invokestatic_into(&rule.from_method, &rule.to_class, &rule.to_method);
            }
            applied > 0
        })
    };
    // patched への借用 (base) をここで打ち切る。トランスパイル後に内容が
    // 変わった (=注入が実際に行われた) かを先に評価して比較結果だけ保持する。
    let transpiled_changed = match &transpiled {
        Ok(out) => {
            let base: &[u8] = patched.as_deref().unwrap_or(data);
            out.as_slice() != base
        }
        Err(_) => false,
    };
    match (patched, transpiled) {
        (_, Ok(out)) if transpiled_changed => Some(out),
        (Some(p), _) => Some(p),
        _ => None,
    }
}

static TRANSPILE_ENGINE: std::sync::OnceLock<std::sync::Mutex<rsift_parser::ParserEngine>> =
    std::sync::OnceLock::new();

fn button_at(screen_class: &str, index: usize) -> Option<ScreenButtonDescriptor> {
    rsift_api::runtime::runtime().and_then(|rt| {
        let (_, buttons) = rt
            .screen_registry()
            .buttons_for_screen_matched(screen_class);
        buttons.get(index).cloned()
    })
}

pub fn try_inject_screen<'local>(
    env: &mut JNIEnv<'local>,
    screen: &JObject<'local>,
    screen_class: &str,
) {
    let key = screen.as_raw() as usize;
    if is_screen_injected(key) {
        return;
    }
    if button_at(screen_class, 0).is_none() {
        return;
    }
    let Some(loader) = screen_inject::game_class_loader(env) else {
        return;
    };
    match screen_buttons::inject_buttons_rust(env, screen, screen_class, &loader) {
        Ok(n) if n > 0 => {
            mark_screen_injected(key);
            agent_log(&format!(
                "[RsiftAgent] injected {} button(s) into {}",
                n, screen_class
            ));
        }
        Ok(_) => {}
        Err(e) => {
            agent_log(&format!(
                "[RsiftAgent] inject failed for {}: {}",
                screen_class, e
            ));
        }
    }
}

pub fn client_tick(env: &mut JNIEnv) {
    rsift_sim::tick_global_sim();
    if !MODS_LOADED.load(Ordering::SeqCst) {
        return;
    }
    let _ = screen_inject::find_game_class_loader(env);
    let _ = screen_inject::ensure_injector_loaded(env);
    let _ = screen_inject::ensure_screen_hooks(env);

    let minecraft = match screen_inject::find_minecraft_class(env) {
        Some(c) => c,
        None => return,
    };
    // wave HR (#5 Phase 2): getInstance (Minecraft 静的) のメソッド名 + descriptor 解決。
    let gi = crate::obf_map::resolve_method_by_name("net.minecraft.client.Minecraft", "getInstance")
        .unwrap_or_else(|| "getInstance".to_string());
    let gi_desc = crate::obf_map::resolve_descriptor("()Lnet/minecraft/client/Minecraft;");
    let inst = match env.call_static_method(minecraft, &gi, &gi_desc, &[]) {
        Ok(v) => v.l().ok(),
        Err(_) => None,
    };
    let inst = match inst {
        Some(o) => o,
        None => return,
    };
    // wave 205: インスタンス確定後に確実にタイトルマーカーを入れる (冪等)。
    maybe_set_window_title(env, &inst);

    // wave HR: screen (Minecraft) / getScreen (fallback) メソッド名 + descriptor 解決。
    let screen_desc =
        crate::obf_map::resolve_descriptor("()Lnet/minecraft/client/gui/screens/Screen;");
    let screen_m =
        crate::obf_map::resolve_method_by_name("net.minecraft.client.Minecraft", "screen")
            .unwrap_or_else(|| "screen".to_string());
    let getscreen_m =
        crate::obf_map::resolve_method_by_name("net.minecraft.client.Minecraft", "getScreen")
            .unwrap_or_else(|| "getScreen".to_string());
    let screen = match env.call_method(&inst, &screen_m, &screen_desc, &[]) {
        Ok(v) => v.l().ok(),
        Err(_) => env
            .call_method(&inst, &getscreen_m, &screen_desc, &[])
            .ok()
            .and_then(|v| v.l().ok()),
    };
    let screen = match screen {
        Some(s) => s,
        None => return,
    };

    let class_obj = match env.call_method(&screen, "getClass", "()Ljava/lang/Class;", &[]) {
        Ok(v) => v.l().ok(),
        Err(_) => None,
    };
    let class_obj = match class_obj {
        Some(c) => c,
        None => return,
    };
    let name = match env.call_method(&class_obj, "getName", "()Ljava/lang/String;", &[]) {
        Ok(v) => v.l().ok(),
        Err(_) => None,
    };
    let name = match name {
        Some(n) => n,
        None => return,
    };
    let name_str: String = match env.get_string((&name).into()) {
        Ok(s) => s.into(),
        Err(_) => return,
    };

    // wave HR: 画面クラス名(難読化)を mojmap へ逆解決してから判定
    let mojmap_screen = crate::obf_map::resolve_class_reverse(&name_str).unwrap_or_else(|| name_str.clone());
    if button_at(&mojmap_screen, 0).is_some() || mojmap_screen.contains("TitleScreen") || mojmap_screen.contains("PauseScreen") {
        agent_log_step("screen_check", &format!("obf={} mojmap={} → injecting", name_str, mojmap_screen));
        screen_buttons::inject_current_screen_on_render_thread(env, &inst);
    }

    update_idle_state();
}

fn update_idle_state() {
    let Some(rt) = rsift_api::runtime::runtime() else {
        return;
    };
    let reg = rt.screen_registry();
    let title = reg.buttons_for_screen("net.minecraft.client.gui.screens.TitleScreen");
    let pause = reg.buttons_for_screen("net.minecraft.client.gui.screens.PauseScreen");
    if title.is_empty() && pause.is_empty() {
        return;
    }
    let count = injected_set().as_ref().map(|s| s.len()).unwrap_or(0);
    if count > 0 && !rt.is_idle() {
        rt.set_idle_mode(true);
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftBootstrapAgent_nativeAgentPremain(
    mut env: JNIEnv,
    _class: JClass,
    args: JString,
) {
    let arg_str: String = env.get_string(&args).map(|s| s.into()).unwrap_or_default();
    agent_premain(&arg_str);
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftBootstrapAgent_nativeLogAgentpathCompanion(
    _env: JNIEnv,
    _class: JClass,
) {
    agent_log_step(
        "javaagent",
        "ClassFileTransformer registered (agentpath owns native boot)",
    );
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftBootstrapAgent_nativeAgentpathActive(
    _env: JNIEnv,
    _class: JClass,
) -> jni::sys::jboolean {
    if agentpath_active() {
        1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftBootstrapAgent_nativeScheduleDeferredInit(
    mut env: JNIEnv,
    _class: JClass,
    args: JString,
) {
    let arg_str: String = env.get_string(&args).map(|s| s.into()).unwrap_or_default();
    match env.get_java_vm() {
        Ok(vm) => {
            let raw = vm.get_java_vm_pointer() as *mut std::ffi::c_void;
            agent_log("[Rsift] javaagent premain — transformer registered");
            schedule_deferred_init(raw, &arg_str, false);
        }
        Err(e) => agent_log(&format!("[Rsift] get_java_vm failed: {:?}", e)),
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftScreenHooks_nativeLog(
    mut env: JNIEnv,
    _class: JClass,
    line: JString,
) {
    let msg: String = env.get_string(&line).map(|s| s.into()).unwrap_or_default();
    agent_log(&msg);
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftClassTransformer_nativeCaptureGameLoader<
    'local,
>(
    mut env: JNIEnv<'local>,
    _class: JClass,
    loader: JObject<'local>,
) {
    if !loader.as_raw().is_null() {
        screen_inject::cache_game_loader_from_java(&mut env, &loader);
        agent_log_step(
            "transformer",
            "game ClassLoader captured (Minecraft class load)",
        );
    }
}

fn maybe_log_transform(class_name: &str, byte_len: usize) {
    let n = TRANSFORM_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    let interesting = class_name.contains("minecraft")
        || class_name.starts_with("com/mojang")
        || class_name.starts_with("com/rsift");
    if interesting || n < 30 {
        agent_log_step(
            "transformer",
            &format!("#{} class={} bytes={}", n + 1, class_name, byte_len),
        );
    }
}

fn register_transformer_natives(env: &mut JNIEnv) {
    let methods = [
        NativeMethod {
            name: "nativeTransform".into(),
            sig: "(Ljava/lang/String;[B)[B".into(),
            fn_ptr: Java_com_rsift_RsiftClassTransformer_nativeTransform as *mut _,
        },
        NativeMethod {
            name: "nativeIsTargetClass".into(),
            sig: "(Ljava/lang/String;)Z".into(),
            fn_ptr: Java_com_rsift_RsiftClassTransformer_nativeIsTargetClass as *mut _,
        },
    ];
    if let Ok(cls) = env.find_class("com/rsift/RsiftClassTransformer") {
        let _ = env.register_native_methods(&cls, &methods);
        return;
    }
    let Some(loader) = screen_inject::game_class_loader(env) else {
        return;
    };
    let Some(jar) = screen_inject::bootstrap_jar() else {
        return;
    };
    let Ok(ucl) = screen_inject::url_classloader_for_jar(env, &loader, &jar) else {
        return;
    };
    let Ok(name) = env.new_string("com.rsift.RsiftClassTransformer") else {
        return;
    };
    let Ok(obj) = env
        .call_method(
            &ucl,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )
        .and_then(|v| v.l())
    else {
        return;
    };
    let jclass = JClass::from(obj);
    let _ = env.register_native_methods(&jclass, &methods);
}

fn register_hooks_natives(env: &mut JNIEnv) {
    let cls = if let Ok(c) = env.find_class("com/rsift/RsiftHooks") {
        c
    } else {
        let Some(loader) = screen_inject::game_class_loader(env) else {
            return;
        };
        let Some(jar) = screen_inject::bootstrap_jar() else {
            return;
        };
        let Ok(ucl) = screen_inject::url_classloader_for_jar(env, &loader, &jar) else {
            return;
        };
        let Ok(name) = env.new_string("com.rsift.RsiftHooks") else {
            return;
        };
        let Ok(obj) = env
            .call_method(
                &ucl,
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[JValue::Object(&name)],
            )
            .and_then(|v| v.l())
        else {
            return;
        };
        JClass::from(obj)
    };
    let _ = env.register_native_methods(
        &cls,
        &[
            NativeMethod {
                name: "nativeOnHook".into(),
                sig: "(Ljava/lang/String;)V".into(),
                fn_ptr: Java_com_rsift_RsiftHooks_nativeOnHook as *mut _,
            },
            NativeMethod {
                name: "nativeResolveClass0".into(),
                sig: "(Ljava/lang/String;)Ljava/lang/String;".into(),
                fn_ptr: Java_com_rsift_RsiftHooks_nativeResolveClass as *mut _,
            },
            NativeMethod {
                name: "nativeResolveMethod0".into(),
                sig: "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;".into(),
                fn_ptr: Java_com_rsift_RsiftHooks_nativeResolveMethod as *mut _,
            },
        ],
    );
}

fn notify_transformer_ready(env: &mut JNIEnv) {
    if let Ok(cls) = env.find_class("com/rsift/RsiftBootstrapAgent") {
        let _ = env.call_static_method(cls, "notifyNativeReady", "()V", &[]);
    } else if let Ok(cls) = env.find_class("com/rsift/RsiftClassTransformer") {
        let _ = env.call_static_method(cls, "markNativeReadyFromNative", "()V", &[]);
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftClassTransformer_nativeIsTargetClass(
    mut env: JNIEnv,
    _class: JClass,
    class_name: JString,
) -> jni::sys::jboolean {
    let name: String = env
        .get_string(&class_name)
        .map(|s| s.into())
        .unwrap_or_default();
    // wave HR (renderer): retransform sweep — 難読名を mojmap へ解決してターゲット判定。
    let mojmap_name = crate::obf_map::resolve_class_reverse(&name).unwrap_or_else(|| name.clone());
    if rsift_parser::BytecodePatcher::is_target_class(&mojmap_name) {
        jni::sys::JNI_TRUE
    } else {
        jni::sys::JNI_FALSE
    }
}

/// wave HS (ログ #8 観測強化): CFLH パッチ経由で RsiftHooks から呼ばれる各フックの
/// 発火を可視化する。flipFrame→onRenderFlip→nativeOnHook("render_flip") 等のチェーンが
/// 実行時に本当に駆動しているかの決定的証拠。スパム抑止: 各フック 1 回目 + 600 回毎。
fn log_hook_firing(hook: &str) {
    use std::collections::HashMap;
    static COUNTS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, u64>>> =
        std::sync::OnceLock::new();
    let counts = COUNTS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let n = {
        let mut g = match counts.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let e = g.entry(hook.to_string()).or_insert(0);
        *e += 1;
        *e
    };
    if n == 1 || n % 600 == 0 {
        agent_log(&format!("[hook] {} fired (count={})", hook, n));
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftHooks_nativeOnHook(
    mut env: JNIEnv,
    _class: JClass,
    name: JString,
) {
    let hook: String = env.get_string(&name).map(|s| s.into()).unwrap_or_default();
    log_hook_firing(&hook);
    match hook.as_str() {
        "client_tick" | "client_run" => {
            rsift_api::mod_dispatch::dispatch_op("client_tick", 0, 0, 0);
            // wave 203: Mod 宣言のバニラ KeyMapping を install+isDown 同期。
            super::keybind_bridge::poll_and_sync(&mut env);
        }
        "network_packet" => {
            // Packet tap is primary; this is a secondary HEAD marker.
        }
        "render_flip" => {
            if let Some(rt) = rsift_api::runtime::runtime() {
                if rt.has_render_handlers() {
                    rt.dispatch_render(0, 0, 0.016);
                }
            }
            // フレーム粒度でもキー状態を同期 (押し始め遅延を tick より短く)。
            super::keybind_bridge::poll_and_sync(&mut env);
            // wave HR renderer Wave 2→5: MC描画ループ(flipFrame HEAD)から DX12 present へ接続。
            // デフォルト有効(ゲート解除)。DX12 init 失敗時は ensure_engine が GL パススルーへ
            // フォールバックするため黒画面化はしない(バニラ描画継続)。
            if let Some(mc) = super::screen_inject::minecraft_instance(&mut env) {
                super::render_bridge::schedule_dx12_present(&mut env, &mc);
            }
        }
        "screen_init" => {
            // Button injection runs from client_tick path.
            // ただし最初のタイトル画面 init = ユーザーが Controls を開く前の
            // 最早点 → ここで KeyMapping を install しておく (wave 203)。
            super::keybind_bridge::poll_and_sync(&mut env);
        }
        "mob_ai_step" | "entity_travel" | "redstone" | "chunk_tick" | "hopper_tick"
        | "server_level_tick" | "fluid_tick" | "generic_compute" => {
            rsift_api::mod_suite::mod_suite()
                .events
                .dispatch_named("server_tick", "", "", 0, 0, 0);
            // Drive RsCalc native server tick (not client_tick — that is render/UI).
            let _ = rsift_transpiler::rsift_native_server_tick(50.0);
            match hook.as_str() {
                "mob_ai_step" => {
                    let _ = rsift_transpiler::rsift_native_mob_ai_step(0);
                }
                "entity_travel" => {
                    let _ = rsift_transpiler::rsift_native_entity_travel(0);
                }
                "redstone" => {
                    let _ = rsift_transpiler::rsift_native_redstone_calculate(0);
                }
                "chunk_tick" => {
                    let _ = rsift_transpiler::rsift_native_chunk_tick(0);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftHooks_nativeResolveClass(
    mut env: JNIEnv,
    _class: JClass,
    mojmap_dotted: JString,
) -> jstring {
    let input: String = env.get_string(&mojmap_dotted).map(|s| s.into()).unwrap_or_default();
    let resolved = crate::obf_map::resolve_class(&input).unwrap_or_else(|| input.clone());
    env.new_string(&resolved).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftHooks_nativeResolveMethod(
    mut env: JNIEnv,
    _class: JClass,
    class_dotted: JString,
    mojmap_method: JString,
) -> jstring {
    let cls: String = env.get_string(&class_dotted).map(|s| s.into()).unwrap_or_default();
    let mth: String = env.get_string(&mojmap_method).map(|s| s.into()).unwrap_or_default();
    let resolved = crate::obf_map::resolve_method_by_name(&cls, &mth).unwrap_or_else(|| mth.clone());
    env.new_string(&resolved).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftClassTransformer_nativeTransform(
    mut env: JNIEnv,
    _class: JClass,
    class_name: JString,
    buffer: jbyteArray,
) -> jarray {
    let name: String = match env.get_string(&class_name) {
        Ok(s) => s.into(),
        Err(_) => return std::ptr::null_mut(),
    };
    let arr = jni::objects::JByteArray::from_raw(buffer);
    let bytes = match env.convert_byte_array(&arr) {
        Ok(b) => b,
        Err(_) => return std::ptr::null_mut(),
    };
    maybe_log_transform(&name, bytes.len());
    match transform_class(&name, &bytes) {
        Some(out) => {
            agent_log_step(
                "transformer",
                &format!("PATCHED {} (+{} bytes)", name, out.len()),
            );
            match env.byte_array_from_slice(&out) {
                Ok(a) => a.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        }
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftUiBridge_nativePrepareScreen(
    mut env: JNIEnv,
    _class: JClass,
    screen_class: JString,
) {
    let name: String = env
        .get_string(&screen_class)
        .map(|s| s.into())
        .unwrap_or_default();
    // Mirror pause/title prep used by button injection paths.
    if name.contains("Pause") || name.contains("Title") || name.contains("Options") {
        if let Some(rt) = rsift_api::runtime::runtime() {
            let _ = rt.mod_menu();
        }
        tracing::debug!("[UiBridge] prepareScreen {}", name);
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftUiBridge_nativeButtonCount(
    mut env: JNIEnv,
    _class: JClass,
    screen_class: JString,
) -> jint {
    let name: String = env
        .get_string(&screen_class)
        .map(|s| s.into())
        .unwrap_or_default();
    rsift_api::runtime::runtime()
        .map(|rt| {
            rt.screen_registry()
                .buttons_for_screen_matched(&name)
                .1
                .len() as jint
        })
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftUiBridge_nativeButtonId(
    mut env: JNIEnv,
    _class: JClass,
    screen_class: JString,
    index: jint,
) -> jint {
    let name: String = env
        .get_string(&screen_class)
        .map(|s| s.into())
        .unwrap_or_default();
    button_at(&name, index as usize)
        .map(|b| b.id as jint)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftUiBridge_nativeButtonX(
    mut env: JNIEnv,
    _class: JClass,
    screen_class: JString,
    index: jint,
) -> jint {
    let name: String = env
        .get_string(&screen_class)
        .map(|s| s.into())
        .unwrap_or_default();
    button_at(&name, index as usize)
        .map(|b| b.rect.x)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftUiBridge_nativeButtonY(
    mut env: JNIEnv,
    _class: JClass,
    screen_class: JString,
    index: jint,
) -> jint {
    let name: String = env
        .get_string(&screen_class)
        .map(|s| s.into())
        .unwrap_or_default();
    button_at(&name, index as usize)
        .map(|b| b.rect.y)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftUiBridge_nativeButtonW(
    mut env: JNIEnv,
    _class: JClass,
    screen_class: JString,
    index: jint,
) -> jint {
    let name: String = env
        .get_string(&screen_class)
        .map(|s| s.into())
        .unwrap_or_default();
    button_at(&name, index as usize)
        .map(|b| b.rect.width)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftUiBridge_nativeButtonH(
    mut env: JNIEnv,
    _class: JClass,
    screen_class: JString,
    index: jint,
) -> jint {
    let name: String = env
        .get_string(&screen_class)
        .map(|s| s.into())
        .unwrap_or_default();
    button_at(&name, index as usize)
        .map(|b| b.rect.height)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftUiBridge_nativeButtonLabel(
    mut env: JNIEnv,
    _class: JClass,
    screen_class: JString,
    index: jint,
) -> jstring {
    let name: String = env
        .get_string(&screen_class)
        .map(|s| s.into())
        .unwrap_or_default();
    let label = button_at(&name, index as usize)
        .map(|b| b.label)
        .unwrap_or_default();
    env.new_string(label)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPressHandler_nativeOnButton(
    _env: JNIEnv,
    _class: JClass,
    button_id: jint,
) {
    if let Some(rt) = rsift_api::runtime::runtime() {
        rt.screen_registry().fire_button_by_id(button_id as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wave 209 HE: 静的先行登録の exact-spec ピン。登録されるのは検証済み
    /// 1.21.11 マッピング **1 件だけ** (クラス名・メソッド名・descriptor・
    /// 要素型の全精度)。将来スペックが変異 (クラス名の誤記・別メソッド混入等)
    /// すると本テストが RED = 登録内容の狙撃固定。
    /// なお register_f3_marker は冪等なのでテスト多重実行でも件数は変わらない。
    #[test]
    fn preregister_static_f3_specs_registers_verified_spec_only() {
        let class = "net/minecraft/client/gui/components/debug/DebugScreenEntryList";
        preregister_static_f3_specs();
        assert!(rsift_parser::f3_marker::is_f3_target(class));
        let specs = rsift_parser::f3_marker::specs_for(class).unwrap_or_default();
        assert_eq!(specs.len(), 1, "登録 spec は 1 件だけ");
        assert_eq!(specs[0].method_name, "getLines");
        assert_eq!(specs[0].method_descriptor, "()Ljava/util/List;");
        assert_eq!(format!("{:?}", specs[0].element), "String");
    }

    #[test]
    fn list_element_classification_is_precise() {
        use rsift_parser::ListElement;
        assert_eq!(
            classify_list_element("java.util.List<java.lang.String>"),
            Some(ListElement::String)
        );
        assert_eq!(
            classify_list_element("java.util.List<net.minecraft.network.chat.Component>"),
            Some(ListElement::Component)
        );
        assert_eq!(
            classify_list_element(
                "java.util.List<net.minecraft.client.gui.components.debug.DebugScreenEntry>"
            ),
            None,
            "ロジックリストへの混入は CCE リスク — 判別不能は None (= 注入しない)"
        );
        assert_eq!(
            classify_list_element("java.util.List"),
            None,
            "raw List は不注入"
        );
        assert_eq!(
            classify_list_element("java.util.List<java.lang.Integer>"),
            None
        );
    }

    #[test]
    fn f3_name_scoring_prefers_lines_then_f3_then_info() {
        assert!(f3_method_name_score("getCurrentlyEnabled") < f3_method_name_score("getF3Lines"));
        assert!(f3_method_name_score("getSystemInfo") > 0);
        assert!(f3_method_name_score("getLines") > f3_method_name_score("getSystemInfo"));
        assert_eq!(f3_method_name_score("size"), 0);
        // 大文字小文字は吸収
        assert_eq!(
            f3_method_name_score("GETLINES"),
            f3_method_name_score("getlines")
        );
    }

    #[test]
    fn title_marker_initially_unset() {
        // プロセス内一度成功したら二度目は true — 環境非依存の不変条件のみ検査。
        let _v = TITLE_MARKER_SET.load(Ordering::SeqCst);
    }

    #[test]
    fn java_vm_addr_is_null_or_aligned_pointer() {
        let p = java_vm_addr();
        assert!(p.is_null() || (p as usize) % std::mem::align_of::<usize>() == 0);
    }
}
