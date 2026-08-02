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
    agent_log_step(
        "agent_premain",
        &format!("loading mods args={}", agent_args),
    );
    let mod_dir = agent_opts::resolve_mod_dir(agent_args);
    agent_log_step("agent_premain", &format!("mod_dir={:?}", mod_dir));
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
                if attempt == 0 || (!hook_ok && attempt % 25 == 0) {
                    agent_log_step(
                        "deferred_init",
                        &format!(
                            "JVMTI CFLH install (attached thread, attempt {}) = {}",
                            attempt, hook_ok
                        ),
                    );
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

/// (1) Retransform 追撃 (2) F3 マーカー登録+Retransform (3) タイトルマーカー。
/// 個別失敗はログのみでゲーム継続 (どれも致命傷でない設計)。
fn post_bridge_boot(env: &mut JNIEnv, vm_addr: usize) {
    retransform_loaded_targets(env, vm_addr);
    install_f3_marker(env, vm_addr);
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

/// F3 マーカー (wave 205 ユーザー提案採用): 実行時リフレクションで
/// 「0 引数・java.util.List 返り値・要素型 = String または Component」の
/// F3 行メソッドを特定 → TAIL 注入 spec を登録 → 対象クラスを Retransform。
/// 要件に合うメソッドが無い場合は理由ログつきで静黙スキップ (嘘の注入禁止)。
fn install_f3_marker(env: &mut JNIEnv, vm_addr: usize) {
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
    if TITLE_MARKER_SET.load(Ordering::SeqCst) {
        return;
    }
    let run = (|| -> Result<(), String> {
        if inst.as_raw().is_null() {
            return Err("Minecraft instance is null (client not ready)".into());
        }
        let window = env
            .call_method(
                inst,
                "getWindow",
                "()Lcom/mojang/blaze3d/platform/Window;",
                &[],
            )
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
        env.call_method(
            &window,
            "setTitle",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&title_j)],
        )
        .map_err(|e| format!("setTitle: {:?}", e))?;
        agent_log_step("title_marker", &format!("window title set: {}", title));
        Ok(())
    })();
    match run {
        Ok(()) => TITLE_MARKER_SET.store(true, Ordering::SeqCst),
        Err(e) => {
            screen_inject::clear_pending_exception(env);
            agent_log_step("title_marker", &format!("skipped this attempt: {}", e));
        }
    }
}

pub fn transform_class(class_name: &str, data: &[u8]) -> Option<Vec<u8>> {
    let internal = class_name.replace('.', "/");
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
    let inst = match env.call_static_method(
        minecraft,
        "getInstance",
        "()Lnet/minecraft/client/Minecraft;",
        &[],
    ) {
        Ok(v) => v.l().ok(),
        Err(_) => None,
    };
    let inst = match inst {
        Some(o) => o,
        None => return,
    };
    // wave 205: インスタンス確定後に確実にタイトルマーカーを入れる (冪等)。
    maybe_set_window_title(env, &inst);

    let screen = match env.call_method(
        &inst,
        "screen",
        "()Lnet/minecraft/client/gui/screens/Screen;",
        &[],
    ) {
        Ok(v) => v.l().ok(),
        Err(_) => env
            .call_method(
                &inst,
                "getScreen",
                "()Lnet/minecraft/client/gui/screens/Screen;",
                &[],
            )
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

    if button_at(&name_str, 0).is_some() || name_str.contains("TitleScreen") {
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
        &[NativeMethod {
            name: "nativeOnHook".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: Java_com_rsift_RsiftHooks_nativeOnHook as *mut _,
        }],
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
    if rsift_parser::BytecodePatcher::is_target_class(&name) {
        jni::sys::JNI_TRUE
    } else {
        jni::sys::JNI_FALSE
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftHooks_nativeOnHook(
    mut env: JNIEnv,
    _class: JClass,
    name: JString,
) {
    let hook: String = env.get_string(&name).map(|s| s.into()).unwrap_or_default();
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
