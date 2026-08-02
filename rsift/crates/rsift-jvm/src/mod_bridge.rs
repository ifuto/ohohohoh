//! RsiftModBridge + runtime hooks (packet/render dispatch to mod DLLs).

use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::JNIEnv;
use jni::NativeMethod;
use std::sync::Mutex;

use super::screen_inject;
use crate::agent_log::agent_log;

static MOD_BRIDGE: Mutex<Option<GlobalRef>> = Mutex::new(None);
static HOOKS_CLASS: Mutex<Option<GlobalRef>> = Mutex::new(None);

pub fn ensure_mod_bridge(env: &mut JNIEnv) -> bool {
    if mod_bridge_class(env).is_some() {
        return true;
    }
    if screen_inject::find_game_class_loader(env).is_none() {
        return false;
    }
    match load_mod_bridge(env) {
        Ok(_) => {
            agent_log("[Rsift] RsiftModBridge loaded");
            true
        }
        Err(e) => {
            agent_log(&format!("[Rsift] WARN RsiftModBridge load failed: {}", e));
            false
        }
    }
}

pub fn mod_bridge_class<'local>(env: &mut JNIEnv<'local>) -> Option<JClass<'local>> {
    let guard = MOD_BRIDGE.lock().ok()?;
    let g = guard.as_ref()?;
    env.new_local_ref(g.as_obj()).ok().map(JClass::from)
}

fn load_mod_bridge(env: &mut JNIEnv) -> Result<(), String> {
    let jar = screen_inject::bootstrap_jar().ok_or("bootstrap jar path unknown")?;
    if !jar.is_file() {
        return Err(format!("bootstrap jar missing: {:?}", jar));
    }
    let parent = screen_inject::find_game_class_loader(env).ok_or("game ClassLoader not found")?;
    let ucl = match screen_inject::url_classloader_for_jar(env, &parent, &jar) {
        Ok(u) => u,
        Err(e) => {
            // wave 207 診断強化: pending exception の中身を残してから返す
            crate::screen_inject::dump_pending_exception(
                env,
                "url_classloader_for_jar (ModBridge)",
            );
            return Err(e);
        }
    };
    load_and_register_bridge(env, &ucl)?;
    load_hooks_classes(env, &ucl)?;
    Ok(())
}

fn load_and_register_bridge(env: &mut JNIEnv, ucl: &JObject) -> Result<(), String> {
    let name = env
        .new_string("com.rsift.RsiftModBridge")
        .map_err(|e| format!("{:?}", e))?;
    let cls_obj = env
        .call_method(
            ucl,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )
        .map_err(|e| format!("loadClass ModBridge: {:?}", e))?
        .l()
        .map_err(|e| format!("{:?}", e))?;
    let jclass = JClass::from(cls_obj);
    let methods = [NativeMethod {
        name: "nativeDispatch".into(),
        sig: "(Ljava/lang/String;JJI)V".into(),
        fn_ptr: Java_com_rsift_RsiftModBridge_nativeDispatch as *mut _,
    }];
    env.register_native_methods(&jclass, &methods)
        .map_err(|e| format!("register ModBridge: {:?}", e))?;
    let global = env.new_global_ref(jclass).map_err(|e| format!("{:?}", e))?;
    if let Ok(mut slot) = MOD_BRIDGE.lock() {
        *slot = Some(global);
    }
    Ok(())
}

fn load_hooks_classes(env: &mut JNIEnv, ucl: &JObject) -> Result<(), String> {
    let hooks_name = env
        .new_string("com.rsift.RsiftRuntimeHooks")
        .map_err(|e| format!("{:?}", e))?;
    let hooks_obj = env
        .call_method(
            ucl,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&hooks_name)],
        )
        .map_err(|e| format!("loadClass Hooks: {:?}", e))?
        .l()
        .map_err(|e| format!("{:?}", e))?;
    let hooks_jclass = JClass::from(hooks_obj);
    env.register_native_methods(
        &hooks_jclass,
        &[NativeMethod {
            name: "nativeLog".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: Java_com_rsift_RsiftRuntimeHooks_nativeLog as *mut _,
        }],
    )
    .map_err(|e| format!("register Hooks: {:?}", e))?;
    let global = env
        .new_global_ref(hooks_jclass)
        .map_err(|e| format!("{:?}", e))?;
    if let Ok(mut slot) = HOOKS_CLASS.lock() {
        *slot = Some(global);
    }

    // パケットタップは任意機能 (同梱 jar に未収録の場合あり) のためベスト
    // エフォート。失敗時は pending 例外を必ずクリアする — 残すと同スレッドの
    // 後続 JNI 呼出しが全て暗黙失敗する (wave 204: 構造的欠陥の根治)。
    let tap_name = env
        .new_string("com.rsift.RsiftPacketTap")
        .map_err(|e| format!("{:?}", e))?;
    if env
        .call_method(
            ucl,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&tap_name)],
        )
        .is_err()
    {
        let _ = env.exception_clear();
        crate::agent_log::agent_log(
            "[Rsift] RsiftPacketTap は同梱 bootstrap jar に未収録のためパケットタップは無効 (継続)",
        );
    }
    Ok(())
}

pub fn client_game_tick(env: &mut JNIEnv) {
    if !ensure_mod_bridge(env) {
        return;
    }
    try_install_runtime_hooks(env);
    super::platform_bridge::tick(env);
    super::platform_bridge::flush_apply(env);
    super::chunk_bridge::sync(env);
    dispatch_render_frame(env);
    call_mod_bridge_client_tick(env);
}

pub fn dispatch_render_only(env: &mut JNIEnv) {
    if mod_bridge_class(env).is_some() {
        dispatch_render_frame(env);
    }
}

fn try_install_runtime_hooks(env: &mut JNIEnv) {
    let Some(minecraft) = screen_inject::minecraft_instance(env) else {
        return;
    };
    let Some(loader) = screen_inject::game_class_loader(env) else {
        return;
    };
    let hooks = HOOKS_CLASS.lock().ok().and_then(|g| g.as_ref().cloned());
    let Some(hooks_ref) = hooks else {
        return;
    };
    let Some(hooks_cls) = env.new_local_ref(hooks_ref.as_obj()).ok().map(JClass::from) else {
        return;
    };
    let _ = env.call_static_method(
        hooks_cls,
        "ensureInstalled",
        "(Ljava/lang/Object;Ljava/lang/ClassLoader;)V",
        &[JValue::Object(&minecraft), JValue::Object(&loader)],
    );
}

fn dispatch_render_frame(env: &mut JNIEnv) {
    let Some(rt) = rsift_api::runtime::runtime() else {
        return;
    };
    if !rt.has_render_handlers() {
        return;
    }
    let Some(minecraft) = screen_inject::minecraft_instance(env) else {
        return;
    };
    let (width, height) = match read_window_size(env, &minecraft) {
        Some(v) => v,
        None => return,
    };
    if width == 0 || height == 0 {
        return;
    }
    let delta = read_delta_seconds(env, &minecraft).unwrap_or(1.0 / 60.0);

    super::render_bridge::install_render_hook_on_minecraft(env, &minecraft);

    rsift_api::mod_dispatch::dispatch_render(width, height, delta);

    super::render_bridge::schedule_dx12_present(env, &minecraft);

    let _ = super::render_bridge::ensure_render_hooks(env);
}

fn read_window_size(env: &mut JNIEnv, minecraft: &JObject) -> Option<(u32, u32)> {
    let window = env
        .call_method(
            minecraft,
            "getWindow",
            "()Lcom/mojang/blaze3d/platform/Window;",
            &[],
        )
        .ok()
        .and_then(|v| v.l().ok())?;
    let w = env
        .call_method(&window, "getWidth", "()I", &[])
        .ok()
        .and_then(|v| v.i().ok())?;
    let h = env
        .call_method(&window, "getHeight", "()I", &[])
        .ok()
        .and_then(|v| v.i().ok())?;
    Some((w.max(0) as u32, h.max(0) as u32))
}

fn read_delta_seconds(env: &mut JNIEnv, minecraft: &JObject) -> Option<f32> {
    let timer = env
        .call_method(
            minecraft,
            "getDeltaTracker",
            "()Lnet/minecraft/client/DeltaTracker;",
            &[],
        )
        .ok()
        .and_then(|v| v.l().ok())
        .or_else(|| {
            env.call_method(minecraft, "getTimer", "()Lnet/minecraft/client/Timer;", &[])
                .ok()
                .and_then(|v| v.l().ok())
        })?;
    let ms = env
        .call_method(&timer, "getRealtimeDeltaTicks", "()F", &[])
        .ok()
        .and_then(|v| v.f().ok())
        .or_else(|| {
            env.call_method(&timer, "msPerTick", "()F", &[])
                .ok()
                .and_then(|v| v.f().ok())
        })?;
    Some((ms / 20.0).max(0.0001))
}

fn call_mod_bridge_client_tick(env: &mut JNIEnv) {
    let Some(bridge) = mod_bridge_class(env) else {
        return;
    };
    let _ = env.call_static_method(bridge, "onClientTick", "()V", &[]);
    rsift_api::mod_dispatch::dispatch_op("client_tick", 0, 0, 0);
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftModBridge_nativeDispatch(
    mut env: JNIEnv,
    _class: JClass,
    op: JString,
    a: jni::sys::jlong,
    b: jni::sys::jlong,
    c: jni::sys::jint,
) {
    let op_str: String = env.get_string(&op).map(|s| s.into()).unwrap_or_default();
    rsift_api::mod_dispatch::dispatch_op(&op_str, a, b, c);
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftRuntimeHooks_nativeLog(
    mut env: JNIEnv,
    _class: JClass,
    line: JString,
) {
    let msg: String = env.get_string(&line).map(|s| s.into()).unwrap_or_default();
    agent_log(&msg);
}
