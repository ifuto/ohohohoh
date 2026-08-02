//! Game classloader discovery + RsiftUiBridge bootstrap (agentpath mode).

use jni::objects::{GlobalRef, JClass, JObject, JValue};
use jni::JNIEnv;
use jni::NativeMethod;
use std::path::PathBuf;
use std::sync::Mutex;

use super::{
    Java_com_rsift_RsiftPressHandler_nativeOnButton, Java_com_rsift_RsiftScreenHooks_nativeLog,
    Java_com_rsift_RsiftUiBridge_nativeButtonCount, Java_com_rsift_RsiftUiBridge_nativeButtonH,
    Java_com_rsift_RsiftUiBridge_nativeButtonId, Java_com_rsift_RsiftUiBridge_nativeButtonLabel,
    Java_com_rsift_RsiftUiBridge_nativeButtonW, Java_com_rsift_RsiftUiBridge_nativeButtonX,
    Java_com_rsift_RsiftUiBridge_nativeButtonY, Java_com_rsift_RsiftUiBridge_nativePrepareScreen,
};
use crate::agent_log::{agent_log, agent_log_step, agent_log_warn};
use crate::agent_opts;

static BRIDGE_CLASS: Mutex<Option<GlobalRef>> = Mutex::new(None);
static HOOKS_CLASS: Mutex<Option<GlobalRef>> = Mutex::new(None);
static PRESS_HANDLER_CLASS: Mutex<Option<GlobalRef>> = Mutex::new(None);
static GAME_LOADER: Mutex<Option<GlobalRef>> = Mutex::new(None);
static SCREEN_HOOKS_READY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn hooks_class<'local>(env: &mut JNIEnv<'local>) -> Option<JClass<'local>> {
    let guard = HOOKS_CLASS.lock().ok()?;
    let g = guard.as_ref()?;
    env.new_local_ref(g.as_obj()).ok().map(JClass::from)
}

pub fn bridge_class<'local>(env: &mut JNIEnv<'local>) -> Option<JClass<'local>> {
    let guard = BRIDGE_CLASS.lock().ok()?;
    let g = guard.as_ref()?;
    env.new_local_ref(g.as_obj()).ok().map(JClass::from)
}

pub fn game_class_loader<'local>(env: &mut JNIEnv<'local>) -> Option<JObject<'local>> {
    let guard = GAME_LOADER.lock().ok()?;
    let g = guard.as_ref()?;
    env.new_local_ref(g.as_obj()).ok()
}

pub fn clear_pending_exception(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}

pub fn cache_game_loader_from_java<'local>(env: &mut JNIEnv<'local>, loader: &JObject<'local>) {
    if loader.as_raw().is_null() {
        return;
    }
    cache_game_loader(env, loader);
}

pub fn ensure_screen_hooks(env: &mut JNIEnv) -> bool {
    if SCREEN_HOOKS_READY.load(std::sync::atomic::Ordering::Relaxed) {
        return hooks_class(env).is_some();
    }
    if find_game_class_loader(env).is_none() {
        return false;
    }
    match load_screen_hooks_minimal(env) {
        Ok(_) => {
            SCREEN_HOOKS_READY.store(true, std::sync::atomic::Ordering::Relaxed);
            true
        }
        Err(e) => {
            dump_pending_exception(env, "screen_hooks load (pending)");
            agent_log_warn("screen_hooks", &format!("load failed: {}", e));
            false
        }
    }
}

/// Load `RsiftPressHandler` from bootstrap jar and register JNI natives.
pub fn ensure_press_handler_class<'local>(
    env: &mut JNIEnv<'local>,
    game_loader: &JObject<'local>,
) -> Result<JClass<'local>, String> {
    if let Ok(guard) = PRESS_HANDLER_CLASS.lock() {
        if let Some(g) = guard.as_ref() {
            if let Ok(local) = env.new_local_ref(g.as_obj()) {
                return Ok(JClass::from(local));
            }
        }
    }
    if let Ok(cls) = env.find_class("com/rsift/RsiftPressHandler") {
        return Ok(cls);
    }
    clear_pending_exception(env);
    let jar = bootstrap_jar_path().ok_or("bootstrap jar path unknown")?;
    if !jar.is_file() {
        return Err(format!("bootstrap jar missing: {:?}", jar));
    }
    let ucl = url_classloader_for_jar(env, game_loader, &jar)?;
    let name = env
        .new_string("com.rsift.RsiftPressHandler")
        .map_err(|e| format!("{:?}", e))?;
    let handler_obj = env
        .call_method(
            &ucl,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )
        .map_err(|e| format!("load PressHandler: {:?}", e))?
        .l()
        .map_err(|e| format!("{:?}", e))?;
    let handler_jclass = JClass::from(handler_obj);
    register_press_handler_natives(env, &handler_jclass)?;
    let global = env
        .new_global_ref(&handler_jclass)
        .map_err(|e| format!("global ref handler: {:?}", e))?;
    if let Ok(mut slot) = PRESS_HANDLER_CLASS.lock() {
        *slot = Some(global);
    }
    Ok(handler_jclass)
}

fn load_screen_hooks_minimal(env: &mut JNIEnv) -> Result<(), String> {
    if hooks_class(env).is_some() {
        return Ok(());
    }
    let jar = bootstrap_jar_path().ok_or("dll directory unknown")?;
    if !jar.is_file() {
        return Err(format!("bootstrap jar missing: {:?}", jar));
    }
    let parent_loader =
        find_game_class_loader(env).ok_or("game ClassLoader not found (is Minecraft running?)")?;
    let ucl_res = url_classloader_for_jar(env, &parent_loader, &jar);
    if ucl_res.is_err() {
        // wave 207 診断強化: pending exception の中身 (class+message) を必ず
        // 1 行で残す (#4 では「URL class: JavaException」だけ 188 秒連発で
        // root cause 不在だった反省)。
        dump_pending_exception(env, "url_classloader_for_jar (ScreenHooks)");
        let e = ucl_res.unwrap_err();
        return Err(e);
    }
    let ucl = ucl_res.unwrap();

    let hooks_name = env
        .new_string("com.rsift.RsiftScreenHooks")
        .map_err(|e| format!("{:?}", e))?;
    let hooks_res = env.call_method(
        &ucl,
        "loadClass",
        "(Ljava/lang/String;)Ljava/lang/Class;",
        &[JValue::Object(&hooks_name)],
    );
    if hooks_res.is_err() {
        dump_pending_exception(
            env,
            "loadClass com.rsift.RsiftScreenHooks via URLClassLoader",
        );
    }
    let hooks_obj = hooks_res
        .map_err(|e| format!("load ScreenHooks: {:?}", e))?
        .l()
        .map_err(|e| format!("{:?}", e))?;
    let hooks_jclass = JClass::from(hooks_obj);
    register_hooks_natives(env, &hooks_jclass)?;

    let _ = ensure_press_handler_class(env, &parent_loader)?;

    let bridge_name = env
        .new_string("com.rsift.RsiftUiBridge")
        .map_err(|e| format!("{:?}", e))?;
    if let Ok(bridge_obj) = env.call_method(
        &ucl,
        "loadClass",
        "(Ljava/lang/String;)Ljava/lang/Class;",
        &[JValue::Object(&bridge_name)],
    ) {
        if let Ok(bridge_l) = bridge_obj.l() {
            let bridge_jclass = JClass::from(bridge_l);
            let _ = register_bridge_natives(env, &bridge_jclass);
            if let Ok(global) = env.new_global_ref(bridge_jclass) {
                if let Ok(mut slot) = BRIDGE_CLASS.lock() {
                    if slot.is_none() {
                        *slot = Some(global);
                    }
                }
            }
        }
    }
    clear_pending_exception(env);

    let hooks_global = env
        .new_global_ref(hooks_jclass)
        .map_err(|e| format!("global ref hooks: {:?}", e))?;
    if let Ok(mut slot) = HOOKS_CLASS.lock() {
        *slot = Some(hooks_global);
    }
    agent_log("[Rsift] RsiftScreenHooks ready (minimal bootstrap)");
    Ok(())
}

/// pending exception を「いま pending のまま」取り込み、class+message を 1 行
/// ログ化してから pending 状態を**呼出前どおりに戻す** (wave 207 診断強化)。
/// 仕組み: exception_occurred → exception_clear → 問合せ (getClass.getName /
/// getMessage / toString) → 最後に env.throw(exc) で pending を戻す。
/// これで「pending 中は JNI 禁止」の制約で getMessage も失敗して
/// `<dump failed>` に沈黙する状態を根治する。呼出後の exception_clear は
/// 呼出側の契約どおり (dump が勝手に clear しない)。
/// #4 実機では「URL class: JavaException」だけが 188 秒連発して root cause が
/// 読めなかった反省から、JavaException 経路には必ずこれを添える。
pub fn dump_pending_exception(env: &mut JNIEnv, tag: &str) {
    if !env.exception_check().unwrap_or(false) {
        return;
    }
    let exc = match env.exception_occurred() {
        Ok(e) => e,
        Err(_) => return,
    };
    let _ = env.exception_clear();
    let summary: Option<String> = (|| {
        let s = env
            .call_method(&exc, "toString", "()Ljava/lang/String;", &[])
            .ok()?
            .l()
            .ok()?;
        let js: jni::objects::JString = s.into();
        let out: String = env.get_string(&js).ok()?.into();
        Some(out)
    })();
    // pending を元に戻す (throw は失敗しても pending のまま = 呼出側が clear する)
    let _ = env.throw(exc);
    match summary {
        Some(s) => agent_log_warn("jni_exc", &format!("{}: {}", tag, s)),
        None => agent_log_warn(
            "jni_exc",
            &format!("{}: <pending exception, dump failed>", tag),
        ),
    }
}

pub fn ensure_injector_loaded(env: &mut JNIEnv) -> bool {
    if bridge_class(env).is_some() {
        return true;
    }
    if ensure_screen_hooks(env) && bridge_class(env).is_some() {
        return true;
    }
    if find_game_class_loader(env).is_none() {
        return false;
    }
    match load_bridge(env) {
        Ok(_) => {
            agent_log("[Rsift] RsiftUiBridge loaded");
            true
        }
        Err(e) => {
            agent_log(&format!("[Rsift] WARN RsiftUiBridge load failed: {}", e));
            false
        }
    }
}

fn bootstrap_jar_path() -> Option<PathBuf> {
    agent_opts::dll_directory().map(|d| d.join("rsift-bootstrap.jar"))
}

pub fn bootstrap_jar() -> Option<PathBuf> {
    bootstrap_jar_path()
}

pub fn url_classloader_for_jar<'local>(
    env: &mut JNIEnv<'local>,
    parent: &JObject<'local>,
    jar: &PathBuf,
) -> Result<JObject<'local>, String> {
    let url_cls = env
        .find_class("java/net/URL")
        .map_err(|e| format!("URL class: {:?}", e))?;
    let file_url = format!("file:///{}", jar.display().to_string().replace('\\', "/"));
    let url_str = env
        .new_string(&file_url)
        .map_err(|e| format!("url string: {:?}", e))?;
    let url = env
        .new_object(
            url_cls,
            "(Ljava/lang/String;)V",
            &[JValue::Object(&url_str)],
        )
        .map_err(|e| format!("URL ctor: {:?}", e))?;
    let url_array = env
        .new_object_array(1, "java/net/URL", &url)
        .map_err(|e| format!("URL[]: {:?}", e))?;
    env.new_object(
        "java/net/URLClassLoader",
        "([Ljava/net/URL;Ljava/lang/ClassLoader;)V",
        &[JValue::Object(&url_array), JValue::Object(parent)],
    )
    .map_err(|e| format!("URLClassLoader: {:?}", e))
}

pub fn minecraft_instance<'local>(env: &mut JNIEnv<'local>) -> Option<JObject<'local>> {
    let mc = find_minecraft_class(env)?;
    env.call_static_method(mc, "getInstance", "()Lnet/minecraft/client/Minecraft;", &[])
        .ok()
        .and_then(|v| v.l().ok())
}

fn load_bridge(env: &mut JNIEnv) -> Result<(), String> {
    let jar = bootstrap_jar_path().ok_or("dll directory unknown")?;
    if !jar.is_file() {
        return Err(format!("bootstrap jar missing: {:?}", jar));
    }

    let parent_loader =
        find_game_class_loader(env).ok_or("game ClassLoader not found (is Minecraft running?)")?;

    let global_loader = env
        .new_global_ref(&parent_loader)
        .map_err(|e| format!("global ref loader: {:?}", e))?;
    if let Ok(mut slot) = GAME_LOADER.lock() {
        *slot = Some(global_loader);
    }

    let url_cls = env
        .find_class("java/net/URL")
        .map_err(|e| format!("URL class: {:?}", e))?;
    let file_url = format!("file:///{}", jar.display().to_string().replace('\\', "/"));
    let url_str = env
        .new_string(&file_url)
        .map_err(|e| format!("url string: {:?}", e))?;
    let url = env
        .new_object(
            url_cls,
            "(Ljava/lang/String;)V",
            &[JValue::Object(&url_str)],
        )
        .map_err(|e| format!("URL ctor: {:?}", e))?;

    let url_array = env
        .new_object_array(1, "java/net/URL", &url)
        .map_err(|e| format!("URL[]: {:?}", e))?;

    let ucl = env
        .new_object(
            "java/net/URLClassLoader",
            "([Ljava/net/URL;Ljava/lang/ClassLoader;)V",
            &[JValue::Object(&url_array), JValue::Object(&parent_loader)],
        )
        .map_err(|e| format!("URLClassLoader: {:?}", e))?;

    let bridge_name = env
        .new_string("com.rsift.RsiftUiBridge")
        .map_err(|e| format!("bridge name: {:?}", e))?;
    let bridge_obj = env
        .call_method(
            &ucl,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&bridge_name)],
        )
        .map_err(|e| format!("loadClass UiBridge: {:?}", e))?
        .l()
        .map_err(|e| format!("loadClass result: {:?}", e))?;

    let handler_name = env
        .new_string("com.rsift.RsiftPressHandler")
        .map_err(|e| format!("handler name: {:?}", e))?;
    let handler_obj = env
        .call_method(
            &ucl,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&handler_name)],
        )
        .map_err(|e| format!("loadClass PressHandler: {:?}", e))?
        .l()
        .map_err(|e| format!("handler load result: {:?}", e))?;

    let hooks_name = env
        .new_string("com.rsift.RsiftScreenHooks")
        .map_err(|e| format!("hooks name: {:?}", e))?;
    let hooks_obj = env
        .call_method(
            &ucl,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&hooks_name)],
        )
        .map_err(|e| format!("loadClass ScreenHooks: {:?}", e))?
        .l()
        .map_err(|e| format!("hooks load result: {:?}", e))?;

    let bridge_jclass = JClass::from(bridge_obj);
    let handler_jclass = JClass::from(handler_obj);
    let hooks_jclass = JClass::from(hooks_obj);
    register_bridge_natives(env, &bridge_jclass)?;
    register_press_handler_natives(env, &handler_jclass)?;
    register_hooks_natives(env, &hooks_jclass)?;

    let global = env
        .new_global_ref(bridge_jclass)
        .map_err(|e| format!("global ref bridge: {:?}", e))?;
    let hooks_global = env
        .new_global_ref(hooks_jclass)
        .map_err(|e| format!("global ref hooks: {:?}", e))?;
    if let Ok(mut slot) = BRIDGE_CLASS.lock() {
        *slot = Some(global);
    }
    if let Ok(mut slot) = HOOKS_CLASS.lock() {
        *slot = Some(hooks_global);
    }
    Ok(())
}

fn register_hooks_natives(env: &mut JNIEnv, class: &JClass) -> Result<(), String> {
    let methods = [NativeMethod {
        name: "nativeLog".into(),
        sig: "(Ljava/lang/String;)V".into(),
        fn_ptr: Java_com_rsift_RsiftScreenHooks_nativeLog as *mut _,
    }];
    env.register_native_methods(class, &methods)
        .map_err(|e| format!("register ScreenHooks natives: {:?}", e))
}

fn register_bridge_natives(env: &mut JNIEnv, class: &JClass) -> Result<(), String> {
    let methods = [
        NativeMethod {
            name: "nativePrepareScreen".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: Java_com_rsift_RsiftUiBridge_nativePrepareScreen as *mut _,
        },
        NativeMethod {
            name: "nativeButtonCount".into(),
            sig: "(Ljava/lang/String;)I".into(),
            fn_ptr: Java_com_rsift_RsiftUiBridge_nativeButtonCount as *mut _,
        },
        NativeMethod {
            name: "nativeButtonId".into(),
            sig: "(Ljava/lang/String;I)I".into(),
            fn_ptr: Java_com_rsift_RsiftUiBridge_nativeButtonId as *mut _,
        },
        NativeMethod {
            name: "nativeButtonLabel".into(),
            sig: "(Ljava/lang/String;I)Ljava/lang/String;".into(),
            fn_ptr: Java_com_rsift_RsiftUiBridge_nativeButtonLabel as *mut _,
        },
        NativeMethod {
            name: "nativeButtonX".into(),
            sig: "(Ljava/lang/String;I)I".into(),
            fn_ptr: Java_com_rsift_RsiftUiBridge_nativeButtonX as *mut _,
        },
        NativeMethod {
            name: "nativeButtonY".into(),
            sig: "(Ljava/lang/String;I)I".into(),
            fn_ptr: Java_com_rsift_RsiftUiBridge_nativeButtonY as *mut _,
        },
        NativeMethod {
            name: "nativeButtonW".into(),
            sig: "(Ljava/lang/String;I)I".into(),
            fn_ptr: Java_com_rsift_RsiftUiBridge_nativeButtonW as *mut _,
        },
        NativeMethod {
            name: "nativeButtonH".into(),
            sig: "(Ljava/lang/String;I)I".into(),
            fn_ptr: Java_com_rsift_RsiftUiBridge_nativeButtonH as *mut _,
        },
    ];
    env.register_native_methods(class, &methods)
        .map_err(|e| format!("register UiBridge natives: {:?}", e))?;
    let log_methods = [NativeMethod {
        name: "nativeLog".into(),
        sig: "(Ljava/lang/String;)V".into(),
        fn_ptr: Java_com_rsift_RsiftScreenHooks_nativeLog as *mut _,
    }];
    env.register_native_methods(class, &log_methods)
        .map_err(|e| format!("register UiBridge nativeLog: {:?}", e))
}

fn register_press_handler_natives(env: &mut JNIEnv, class: &JClass) -> Result<(), String> {
    let methods = [NativeMethod {
        name: "nativeOnButton".into(),
        sig: "(I)V".into(),
        fn_ptr: Java_com_rsift_RsiftPressHandler_nativeOnButton as *mut _,
    }];
    env.register_native_methods(class, &methods)
        .map_err(|e| format!("register PressHandler natives: {:?}", e))
}

pub fn jni_exception_message(env: &mut JNIEnv) -> Option<String> {
    if !env.exception_check().ok()? {
        return None;
    }
    let exc = env.exception_occurred().ok()?;
    let _ = env.exception_clear();
    let class_obj = env
        .call_method(&exc, "getClass", "()Ljava/lang/Class;", &[])
        .ok()?
        .l()
        .ok()?;
    let class_name = env
        .call_method(&class_obj, "getName", "()Ljava/lang/String;", &[])
        .ok()?
        .l()
        .ok()?;
    let cn: String = env.get_string((&class_name).into()).ok()?.into();
    let msg_obj = env
        .call_method(&exc, "getMessage", "()Ljava/lang/String;", &[])
        .ok()
        .and_then(|v| v.l().ok());
    let msg: String = msg_obj
        .and_then(|m| env.get_string((&m).into()).ok().map(|s| s.into()))
        .unwrap_or_default();
    Some(if msg.is_empty() {
        cn
    } else {
        format!("{}: {}", cn, msg)
    })
}

/// Like `find_game_class_loader` but logs which strategy succeeded/failed (for crash diagnosis).
pub fn find_game_class_loader_verbose<'local>(
    env: &mut JNIEnv<'local>,
    attempt: u32,
) -> Option<JObject<'local>> {
    let verbose = attempt == 0 || attempt % 10 == 0;
    if let Some(cached) = game_class_loader(env) {
        if verbose {
            agent_log_step("classloader", "using cached game ClassLoader");
        }
        return Some(cached);
    }

    // 戦略 0 (wave 205 根治): JVMTI CFLH が捕捉した game loader を直接利用。
    // スレッド歩きに依存しない本命経路 (install は attach 前に移動済)。
    if let Some(loader) = loader_from_jvmti_capture(env) {
        cache_game_loader(env, &loader);
        agent_log_step("classloader", "FOUND via JVMTI CFLH captured loader");
        return Some(loader);
    }

    // Minecraft client classes are not loadable for several seconds after JVM start.
    // Calling getAllStackTraces repeatedly this early can overflow the agent thread stack.
    if attempt < 5 {
        if verbose {
            agent_log_step(
                "classloader",
                &format!(
                    "attempt {} — game still booting, deferring JNI probes",
                    attempt
                ),
            );
        }
        return None;
    }

    if javaagent_bootstrap_active() {
        if verbose {
            agent_log_step(
                "classloader",
                "probe: RsiftAgentState.gameLoader (javaagent cached loader)",
            );
        }
        if let Some(loader) = loader_from_java_agent_state(env) {
            cache_game_loader(env, &loader);
            agent_log_step("classloader", "FOUND via RsiftAgentState.gameLoader");
            return Some(loader);
        }
        if verbose {
            log_jni_exception(env, "java_agent_state");
        }
    }

    if verbose {
        agent_log_step(
            "classloader",
            "probe: Render/Game thread context ClassLoader",
        );
    }
    if let Some(loader) = loader_from_priority_threads(env, verbose) {
        cache_game_loader(env, &loader);
        agent_log_step("classloader", "FOUND via named thread context ClassLoader");
        return Some(loader);
    }
    if verbose {
        log_jni_exception(env, "priority_threads");
    }

    // 決定的代替 (wave 205): JVMTI GetLoadedClasses 全走査。
    // 10 秒周期に抑制 (全ロードクラスへの JNI 往復は重いため)。
    if attempt >= 10 && attempt % 10 == 0 {
        if verbose {
            agent_log_step("classloader", "probe: JVMTI GetLoadedClasses sweep");
        }
        if let Some(loader) = loader_from_loaded_classes(env) {
            cache_game_loader(env, &loader);
            agent_log_step("classloader", "FOUND via JVMTI GetLoadedClasses");
            return Some(loader);
        }
    }

    if attempt >= 15 {
        if verbose {
            agent_log_step(
                "classloader",
                "probe: Minecraft.getInstance() via cached loader",
            );
        }
        if let Some(loader) = loader_from_minecraft_get_instance(env) {
            cache_game_loader(env, &loader);
            agent_log_step("classloader", "FOUND via Minecraft instance");
            return Some(loader);
        }
        if verbose {
            log_jni_exception(env, "minecraft_instance");
            agent_log_warn("classloader", "all strategies failed this probe");
        }
    }
    None
}

fn log_jni_exception(env: &mut JNIEnv, phase: &str) {
    if let Some(msg) = jni_exception_message(env) {
        agent_log_warn("classloader", &format!("{} JNI exception: {}", phase, msg));
    }
}

/// Find Minecraft's application ClassLoader (JNI FindClass fails on agent threads).
pub fn find_game_class_loader<'local>(env: &mut JNIEnv<'local>) -> Option<JObject<'local>> {
    if let Some(cached) = game_class_loader(env) {
        return Some(cached);
    }
    // 戦略 0 (wave 205): JVMTI CFLH 捕捉 — 最廉価なので常時チェック。
    if let Some(loader) = loader_from_jvmti_capture(env) {
        cache_game_loader(env, &loader);
        return Some(loader);
    }
    if javaagent_bootstrap_active() {
        if let Some(loader) = loader_from_java_agent_state(env) {
            cache_game_loader(env, &loader);
            return Some(loader);
        }
    }
    if let Some(loader) = loader_from_priority_threads(env, false) {
        cache_game_loader(env, &loader);
        return Some(loader);
    }
    if let Some(loader) = loader_from_minecraft_get_instance(env) {
        cache_game_loader(env, &loader);
        return Some(loader);
    }
    None
}

/// 戦略 0 (wave 205): CFLH コールバックが NewGlobalRef 化した game loader。
/// loader 自体の裏付けは CFLH 側 (net/minecraft クラスの defining loader) で済。
fn loader_from_jvmti_capture<'local>(env: &mut JNIEnv<'local>) -> Option<JObject<'local>> {
    let raw = crate::jvmti_events::captured_game_loader_raw()?;
    // SAFETY: raw は CFLH 内で NewGlobalRef 済みの生存中 global ref。
    let obj = unsafe { JObject::from_raw(raw as jni::sys::jobject) };
    let local = env.new_local_ref(&obj).ok()?;
    if local.as_raw().is_null() {
        return None;
    }
    Some(local)
}

/// 決定的代替 (wave 205): JVMTI GetLoadedClasses 全走査。
/// 最初に見つかった net.minecraft.* クラスの getClassLoader() を返す。
/// (getAllStackTraces 不要・スレッド名不一致に左右されない経路)
fn loader_from_loaded_classes<'local>(env: &mut JNIEnv<'local>) -> Option<JObject<'local>> {
    let vm = crate::agent_bridge::java_vm_addr();
    if vm.is_null() {
        return None;
    }
    let classes = unsafe { crate::jvmti_events::get_loaded_classes(vm) }?;
    let total = classes.len();
    let mut netmc_seen = 0usize;
    for jclass in classes {
        if jclass.is_null() {
            continue;
        }
        // SAFETY: jclass は attach 済み本スレッドの local ref (GetLoadedClasses 規格)。
        let jobj = unsafe { JObject::from_raw(jclass as jni::sys::jobject) };
        let name_j = match env
            .call_method(&jobj, "getName", "()Ljava/lang/String;", &[])
            .and_then(|v| v.l())
        {
            Ok(o) => o,
            Err(_) => {
                clear_pending_exception(env);
                continue;
            }
        };
        let name: String = match env.get_string((&name_j).into()) {
            Ok(s) => s.into(),
            Err(_) => {
                clear_pending_exception(env);
                continue;
            }
        };
        if !name.starts_with("net.minecraft.") {
            continue;
        }
        netmc_seen += 1;
        let loader = match env
            .call_method(&jobj, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
            .and_then(|v| v.l())
        {
            Ok(o) => o,
            Err(_) => {
                clear_pending_exception(env);
                continue;
            }
        };
        if loader.as_raw().is_null() {
            // bootstrap ロードの minecraft クラスは unpacker 状況としては異常だが
            // ログの一次情報として残して次候補へ。
            agent_log_step(
                "classloader",
                &format!("GetLoadedClasses: {} has null loader — skipping", name),
            );
            continue;
        }
        agent_log_step(
            "classloader",
            &format!(
                "GetLoadedClasses: {} loaded classes, first game class {} -> loader OK",
                total, name
            ),
        );
        return Some(loader);
    }
    agent_log_step(
        "classloader",
        &format!(
            "GetLoadedClasses: {} loaded, net.minecraft seen={}, no loader resolved",
            total, netmc_seen
        ),
    );
    None
}

fn javaagent_bootstrap_active() -> bool {
    crate::agent_opts::dll_directory()
        .map(|d| d.join(".rsift-javaagent-ok").is_file())
        .unwrap_or(false)
}

fn loader_from_java_agent_state<'local>(env: &mut JNIEnv<'local>) -> Option<JObject<'local>> {
    let state = env.find_class("com/rsift/RsiftAgentState").ok()?;
    clear_pending_exception(env);
    let loader = env
        .call_static_method(state, "getGameLoader", "()Ljava/lang/ClassLoader;", &[])
        .ok()
        .and_then(|v| v.l().ok())?;
    clear_pending_exception(env);
    if loader.as_raw().is_null() {
        return None;
    }
    if load_class_with_loader(env, &loader, "net.minecraft.client.Minecraft").is_some() {
        return Some(loader);
    }
    clear_pending_exception(env);
    None
}

/// 各スレッド probe の結果区分 (verbose 時に自己記述ログ化、wave 205)。
enum ThreadLoaderOutcome<'local> {
    Found(JObject<'local>),
    NullLoader(String),
    ProbeFailed(String, String),
}

impl ThreadLoaderOutcome<'_> {
    fn describe(&self) -> String {
        match self {
            ThreadLoaderOutcome::Found(_) => "loader OK".into(),
            ThreadLoaderOutcome::NullLoader(t) => {
                format!("thread '{}': context ClassLoader is null", t)
            }
            ThreadLoaderOutcome::ProbeFailed(t, m) => {
                format!("thread '{}': loadClass probe failed: {}", t, m)
            }
        }
    }
}

/// One `getAllStackTraces` call — try Render/Game/main threads in priority order.
/// `verbose` 時は見えたスレッド名一覧と各優先スレッドの probe 内訳を残す
/// (旧来「all strategies failed」だけで理由が読めなかった欠陥の根治、wave 205)。
fn loader_from_priority_threads<'local>(
    env: &mut JNIEnv<'local>,
    verbose: bool,
) -> Option<JObject<'local>> {
    const PRIORITY: &[&str] = &["Render thread", "Game thread", "Server thread", "main"];
    let thread_cls = env.find_class("java/lang/Thread").ok()?;
    clear_pending_exception(env);
    let map = env
        .call_static_method(thread_cls, "getAllStackTraces", "()Ljava/util/Map;", &[])
        .ok()?
        .l()
        .ok()?;
    clear_pending_exception(env);
    let key_set = env
        .call_method(&map, "keySet", "()Ljava/util/Set;", &[])
        .ok()?
        .l()
        .ok()?;
    let iter = env
        .call_method(&key_set, "iterator", "()Ljava/util/Iterator;", &[])
        .ok()?
        .l()
        .ok()?;
    let mut named: Vec<(usize, JObject<'local>)> = Vec::new();
    let mut all_names: Vec<String> = Vec::new();
    while env
        .call_method(&iter, "hasNext", "()Z", &[])
        .ok()?
        .z()
        .ok()?
    {
        let thread = env
            .call_method(&iter, "next", "()Ljava/lang/Object;", &[])
            .ok()?
            .l()
            .ok()?;
        let name_obj = env
            .call_method(&thread, "getName", "()Ljava/lang/String;", &[])
            .ok()?
            .l()
            .ok()?;
        let name: String = env.get_string((&name_obj).into()).ok()?.into();
        if let Some(priority) = PRIORITY.iter().position(|want| *want == name) {
            named.push((priority, thread));
        }
        all_names.push(name);
    }
    if verbose {
        let mut sorted = all_names.clone();
        sorted.sort();
        agent_log_step(
            "classloader",
            &format!(
                "getAllStackTraces: {} threads [{}]",
                sorted.len(),
                sorted.join(", ")
            ),
        );
    }
    named.sort_by_key(|(priority, _)| *priority);
    for (_, thread) in named {
        match loader_from_thread_report(env, &thread) {
            ThreadLoaderOutcome::Found(loader) => return Some(loader),
            outcome => {
                if verbose {
                    agent_log_step("classloader", &format!("probe: {}", outcome.describe()));
                }
            }
        }
    }
    None
}

/// Resolve loader from a live Minecraft client — uses cached/priority-thread loader only (no recursion).
fn loader_from_minecraft_get_instance<'local>(env: &mut JNIEnv<'local>) -> Option<JObject<'local>> {
    let loader = game_class_loader(env).or_else(|| loader_from_priority_threads(env, false))?;
    let mc = load_class_with_loader(env, &loader, "net.minecraft.client.Minecraft")?;
    clear_pending_exception(env);
    let inst = env
        .call_static_method(mc, "getInstance", "()Lnet/minecraft/client/Minecraft;", &[])
        .ok()
        .and_then(|v| v.l().ok())?;
    clear_pending_exception(env);
    if inst.as_raw().is_null() {
        return None;
    }
    loader_from_instance(env, &inst)
}

/// thread 単体の loader probe。結果区分 (見つかった/null/検証失敗+理由) を返す。
fn loader_from_thread_report<'local>(
    env: &mut JNIEnv<'local>,
    thread: &JObject<'local>,
) -> ThreadLoaderOutcome<'local> {
    let tname: String = env
        .call_method(thread, "getName", "()Ljava/lang/String;", &[])
        .and_then(|v| v.l())
        .ok()
        .and_then(|o| env.get_string((&o).into()).ok().map(|s| s.into()))
        .unwrap_or_else(|| "?".into());
    let loader = match env.call_method(
        thread,
        "getContextClassLoader",
        "()Ljava/lang/ClassLoader;",
        &[],
    ) {
        Ok(v) => match v.l() {
            Ok(o) => o,
            Err(_) => {
                let _ = env.exception_clear();
                return ThreadLoaderOutcome::NullLoader(tname);
            }
        },
        Err(_) => {
            let _ = env.exception_clear();
            return ThreadLoaderOutcome::NullLoader(tname);
        }
    };
    if loader.as_raw().is_null() {
        return ThreadLoaderOutcome::NullLoader(tname);
    }
    if load_class_with_loader(env, &loader, "net.minecraft.client.Minecraft").is_some() {
        return ThreadLoaderOutcome::Found(loader);
    }
    let msg =
        jni_exception_message(env).unwrap_or_else(|| "loadClass returned no class".to_string());
    ThreadLoaderOutcome::ProbeFailed(tname, msg)
}

fn loader_from_instance<'local>(
    env: &mut JNIEnv<'local>,
    instance: &JObject<'local>,
) -> Option<JObject<'local>> {
    let class_obj = env
        .call_method(instance, "getClass", "()Ljava/lang/Class;", &[])
        .ok()
        .and_then(|v| v.l().ok())?;
    let loader = env
        .call_method(
            &class_obj,
            "getClassLoader",
            "()Ljava/lang/ClassLoader;",
            &[],
        )
        .ok()
        .and_then(|v| v.l().ok())?;
    if loader.as_raw().is_null() {
        return None;
    }
    Some(loader)
}

fn cache_game_loader<'local>(env: &mut JNIEnv<'local>, loader: &JObject<'local>) {
    if let Ok(global) = env.new_global_ref(loader) {
        if let Ok(mut slot) = GAME_LOADER.lock() {
            if slot.is_none() {
                *slot = Some(global);
                agent_log("[Rsift] game ClassLoader cached");
            }
        }
    }
}

pub fn load_class_with_loader<'local>(
    env: &mut JNIEnv<'local>,
    loader: &JObject<'local>,
    dotted: &str,
) -> Option<JClass<'local>> {
    let name = env.new_string(dotted).ok()?;
    let res = env.call_method(
        loader,
        "loadClass",
        "(Ljava/lang/String;)Ljava/lang/Class;",
        &[JValue::Object(&name)],
    );
    if res.is_err() {
        // wave 207 診断強化: loadClass 失敗の理由 (CNF/NCDFE 等) を 1 行で残す。
        // caller 側では静黙 skip する設計のため、ここが唯一の診断点になる。
        // wave 209 HE: 実機 #5 で同一クラスの CNFE が 250ms 周期で 135 秒
        // (800 行超) 連発しログを埋没させたため、同名は 2 秒に 1 行へ絞る
        // (診断情報は残しつつ可読性を確保。間引きは失敗側のみ)。
        if should_log_class_failure(dotted) {
            dump_pending_exception(
                env,
                &format!(
                    "loadClass({}) via game loader — returning None to caller",
                    dotted
                ),
            );
        } else {
            clear_pending_exception(env);
        }
    }
    let cls = res.ok().and_then(|v| v.l().ok())?;
    Some(JClass::from(cls))
}

static CLASS_FAIL_LOG_TS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// wave 209 HE: クラスロード失敗ログのスロットル判定 (同名 2 秒に 1 行)。
fn should_log_class_failure(dotted: &str) -> bool {
    let now = std::time::Instant::now();
    let mut map = match CLASS_FAIL_LOG_TS.lock() {
        Ok(m) => m,
        Err(_) => return true,
    };
    match map.get(dotted) {
        Some(t) if now.duration_since(*t).as_secs() < 2 => false,
        _ => {
            map.insert(dotted.to_string(), now);
            true
        }
    }
}

pub fn find_minecraft_class<'local>(env: &mut JNIEnv<'local>) -> Option<JClass<'local>> {
    // wave 209 HE: ClassLoad jcache を最優先 (ローダー非依存 — Prism ラッパーの
    // 子ローダー分離でも確実に届く。実機 #5 の CNFE 永続の根治)。
    if let Some(raw) = crate::jvmti_events::wanted_class_raw("net/minecraft/client/Minecraft") {
        // SAFETY: raw は ClassLoad コールバック内で NewGlobalRef 済みの生存中 jclass。
        let obj = unsafe { JObject::from_raw(raw as jni::sys::jobject) };
        if let Ok(local) = env.new_local_ref(&obj) {
            if !local.as_raw().is_null() {
                return Some(JClass::from(local));
            }
        }
    }
    if let Ok(c) = env.find_class("net/minecraft/client/Minecraft") {
        return Some(c);
    }
    clear_pending_exception(env);
    // Use cached loader only — find_game_class_loader() must not be called from here
    // (minecraft_instance → find_minecraft_class → find_game_class_loader → minecraft_instance).
    let loader = game_class_loader(env)?;
    load_class_with_loader(env, &loader, "net.minecraft.client.Minecraft")
}

pub fn minecraft_class_ready(env: &mut JNIEnv) -> bool {
    find_minecraft_class(env).is_some()
}
