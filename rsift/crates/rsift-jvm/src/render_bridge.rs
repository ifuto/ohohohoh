//! JNI render bridge — HWND extraction + DX12 frame submit (agent render thread only).

use jni::objects::{JClass, JObject, JValue};
use jni::JNIEnv;
use jni::NativeMethod;
use std::cell::RefCell;

use crate::agent_log::agent_log;

thread_local! {
    static DX12_ENGINE: RefCell<Option<rsift_dx12::Dx12Engine>> = const { RefCell::new(None) };
}

fn with_engine_mut<R>(f: impl FnOnce(&mut rsift_dx12::Dx12Engine) -> R) -> Option<R> {
    DX12_ENGINE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        borrow.as_mut().map(f)
    })
}

pub fn install_engine(engine: rsift_dx12::Dx12Engine) {
    DX12_ENGINE.with(|slot| {
        *slot.borrow_mut() = Some(engine);
    });
}

pub fn ensure_engine() {
    if DX12_ENGINE.with(|s| s.borrow().is_some()) {
        return;
    }
    // 2026-07-21: DX12 優先バックエンドラダー (rsift_render::backend) に従う。
    // 一度 GlPassthrough 確定したら DX12 init は再試行しない (プロセス実行中に
    // ドライバ供給状況は変わらないため。旧来は flip 毎に create を再試行していた)。
    if matches!(
        rsift_render::backend::realized_backend(),
        Some(rsift_render::backend::RenderBackendKind::GlPassthrough)
    ) {
        return;
    }
    let (force, force_reason) = std::env::var("rsift.render.backend")
        .ok()
        .map(|v| rsift_render::backend::parse_force_env(&v))
        .unwrap_or((rsift_render::backend::ForceMode::Auto, "env 未指定 → auto"));
    let sel = rsift_render::backend::select(
        &rsift_render::backend::BackendProbe::default(),
        force,
    );
    if sel.kind != rsift_render::backend::RenderBackendKind::Dx12 {
        // 強制 GL、または (present 未実装の) DX11/Vulkan 強制: 実 present は
        // バニラ GL に委ねる。未実装系の強制は黙って約束しないよう fail-loud。
        agent_log(&format!(
            "[RsiftRender] backend ladder: {} を要求 ({}) — 実 present はバニラ GL パススルーで継続します{}",
            sel.kind.label(),
            force_reason,
            if sel.kind.present_implemented() && sel.kind == rsift_render::backend::RenderBackendKind::GlPassthrough {
                ""
            } else {
                " (要求系統の present は未実装)"
            }
        ));
        rsift_render::backend::record_realized_backend(
            rsift_render::backend::RenderBackendKind::GlPassthrough,
        );
        super::glfw_hook::install_glfw_swap_hook();
        return;
    }
    let caps = rsift_api::engine_caps::EngineCaps::from_jvm_props().or_else(|| {
        let probe = rsift_api::engine_caps::GpuCapabilityProbe::probe();
        rsift_api::engine_caps::EngineCaps::install_default(&probe, None).ok()
    });
    if let Some(caps) = caps {
        match rsift_dx12::Dx12Engine::create(caps) {
            Ok(engine) => {
                install_engine(engine);
                rsift_render::proxy::global_proxy().enable();
                rsift_render::backend::record_realized_backend(
                    rsift_render::backend::RenderBackendKind::Dx12,
                );
                super::glfw_hook::install_glfw_swap_hook();
            }
            Err(e) => {
                // 旧来は失敗時に無言で GL に落ちていた。ラダー契約として fail-loud。
                agent_log(&format!(
                    "[RsiftRender] DX12 init FAILED ({e}) — 旧 DX/Vulkan は present 未実装のため、バニラ GL パススルーで描画を継続します (黒画面ではありません)"
                ));
                rsift_render::backend::record_realized_backend(
                    rsift_render::backend::RenderBackendKind::GlPassthrough,
                );
                super::glfw_hook::install_glfw_swap_hook();
            }
        }
    } else {
        agent_log(
            "[RsiftRender] DX12 caps 取得不可 — バニラ GL パススルーで描画を継続します",
        );
        rsift_render::backend::record_realized_backend(
            rsift_render::backend::RenderBackendKind::GlPassthrough,
        );
        super::glfw_hook::install_glfw_swap_hook();
    }
}

/// Win32 HWND via GLFW (`org.lwjgl.glfw.GLFWNativeWin32.glfwGetWin32Window`).
pub fn read_window_hwnd(env: &mut JNIEnv, minecraft: &JObject) -> Option<i64> {
    let window = env
        .call_method(
            minecraft,
            "getWindow",
            "()Lcom/mojang/blaze3d/platform/Window;",
            &[],
        )
        .ok()
        .and_then(|v| v.l().ok())?;

    let glfw = env
        .call_method(&window, "getWindow", "()J", &[])
        .ok()
        .and_then(|v| v.j().ok())
        .or_else(|| {
            env.call_method(&window, "getHandle", "()J", &[])
                .ok()
                .and_then(|v| v.j().ok())
        })?;

    let win32 = env
        .find_class("org/lwjgl/glfw/GLFWNativeWin32")
        .ok()
        .or_else(|| {
            let loader = super::screen_inject::game_class_loader(env)?;
            let name = env.new_string("org.lwjgl.glfw.GLFWNativeWin32").ok()?;
            env.call_method(
                &loader,
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[JValue::Object(&name)],
            )
            .ok()
            .and_then(|v| v.l().ok())
            .map(JClass::from)
        })?;

    let hwnd = env
        .call_static_method(win32, "glfwGetWin32Window", "(J)J", &[JValue::Long(glfw)])
        .ok()
        .and_then(|v| v.j().ok())?;

    if hwnd != 0 {
        Some(hwnd)
    } else {
        None
    }
}

pub fn schedule_dx12_present(env: &mut JNIEnv, minecraft: &JObject) {
    ensure_engine();
    let _ = ensure_render_hooks(env);
    if let Some(cls) = render_hooks_class(env) {
        let _ = env.call_static_method(
            cls,
            "scheduleFlip",
            "(Ljava/lang/Object;)V",
            &[JValue::Object(minecraft)],
        );
        super::screen_inject::clear_pending_exception(env);
    }
}

fn render_hooks_class<'local>(env: &mut JNIEnv<'local>) -> Option<JClass<'local>> {
    if let Ok(cls) = env.find_class("com/rsift/RsiftRenderHooks") {
        return Some(cls);
    }
    super::screen_inject::clear_pending_exception(env);
    let jar = super::screen_inject::bootstrap_jar()?;
    let parent = super::screen_inject::find_game_class_loader(env)?;
    let ucl = super::screen_inject::url_classloader_for_jar(env, &parent, &jar).ok()?;
    let name = env.new_string("com.rsift.RsiftRenderHooks").ok()?;
    let obj = env
        .call_method(
            &ucl,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )
        .ok()
        .and_then(|v| v.l().ok())?;
    Some(JClass::from(obj))
}

/// Legacy agent-thread present (avoid — use `schedule_dx12_present`).
pub fn submit_dx12_frame(env: &mut JNIEnv, minecraft: &JObject, width: u32, height: u32) {
    ensure_engine();
    let hwnd = match read_window_hwnd(env, minecraft) {
        Some(h) => h,
        None => return,
    };

    #[cfg(windows)]
    let presented = {
        use windows::Win32::Foundation::HWND;
        let hwnd = HWND(hwnd as *mut _);
        let presented = rsift_opt_gfx::with_gpu_quad_bytes(|quads| {
            with_engine_mut(|engine| {
                let _ = rsift_dx12::ensure_swap_chain(engine, hwnd, width, height);
                rsift_dx12::present_frame(engine, Some(quads))
                    .map(|s| s.presented)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            with_engine_mut(|engine| {
                let _ = rsift_dx12::ensure_swap_chain(engine, hwnd, width, height);
                rsift_dx12::present_frame(engine, None)
                    .map(|s| s.presented)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
        });
        presented
    };

    #[cfg(not(windows))]
    let presented = false;

    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if presented && !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let (frames, draws) = rsift_render::frame_counters();
        agent_log(&format!(
            "[RsiftRender] DX12 present OK — frames={} draws={} (Rust native, not LWJGL)",
            frames, draws
        ));
    }
}

pub fn ensure_render_hooks(env: &mut JNIEnv) -> bool {
    if !super::mod_bridge::ensure_mod_bridge(env) {
        return false;
    }
    load_render_hooks(env).is_ok()
}

fn load_render_hooks(env: &mut JNIEnv) -> Result<(), String> {
    static LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if LOADED.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }

    let jar = super::screen_inject::bootstrap_jar().ok_or("no bootstrap jar")?;
    let parent = super::screen_inject::find_game_class_loader(env).ok_or("no game classloader")?;
    let ucl = super::screen_inject::url_classloader_for_jar(env, &parent, &jar)?;

    let name = env
        .new_string("com.rsift.RsiftRenderHooks")
        .map_err(|e| format!("{:?}", e))?;
    let cls_obj = env
        .call_method(
            &ucl,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )
        .map_err(|e| format!("loadClass RenderHooks: {:?}", e))?
        .l()
        .map_err(|e| format!("{:?}", e))?;
    let jclass = JClass::from(cls_obj);

    let methods = [
        NativeMethod {
            name: "nativeOnFlip".into(),
            sig: "(JII)V".into(),
            fn_ptr: Java_com_rsift_RsiftRenderHooks_nativeOnFlip as *mut _,
        },
        NativeMethod {
            name: "nativeGlDraw".into(),
            sig: "()Z".into(),
            fn_ptr: Java_com_rsift_RsiftRenderHooks_nativeGlDraw as *mut _,
        },
        NativeMethod {
            name: "nativeGlSwap".into(),
            sig: "()V".into(),
            fn_ptr: Java_com_rsift_RsiftRenderHooks_nativeGlSwap as *mut _,
        },
        // Transpiler が HEAD 挿入した vanilla レンダーフックの実測カウンタ。
        NativeMethod {
            name: "nativeGetQuadsHook".into(),
            sig: "()J".into(),
            fn_ptr: Java_com_rsift_RsiftRenderHooks_nativeGetQuadsHook as *mut _,
        },
        NativeMethod {
            name: "nativeChunkLayerHook".into(),
            sig: "()J".into(),
            fn_ptr: Java_com_rsift_RsiftRenderHooks_nativeChunkLayerHook as *mut _,
        },
    ];
    env.register_native_methods(&jclass, &methods)
        .map_err(|e| format!("register RenderHooks: {:?}", e))?;

    LOADED.store(true, std::sync::atomic::Ordering::Relaxed);
    agent_log("[Rsift] RsiftRenderHooks natives registered");
    Ok(())
}

pub fn install_render_hook_on_minecraft(env: &mut JNIEnv, minecraft: &JObject) {
    let _ = ensure_render_hooks(env);
    let Some(loader) = super::screen_inject::game_class_loader(env) else {
        return;
    };
    let jar = match super::screen_inject::bootstrap_jar() {
        Some(j) => j,
        None => return,
    };
    let ucl = match super::screen_inject::url_classloader_for_jar(env, &loader, &jar) {
        Ok(u) => u,
        Err(_) => return,
    };
    let name = match env.new_string("com.rsift.RsiftRenderHooks") {
        Ok(s) => s,
        Err(_) => return,
    };
    let cls = match env.call_method(
        &ucl,
        "loadClass",
        "(Ljava/lang/String;)Ljava/lang/Class;",
        &[JValue::Object(&name)],
    ) {
        Ok(v) => match v.l() {
            Ok(o) => JClass::from(o),
            Err(_) => return,
        },
        Err(_) => return,
    };
    let _ = env.call_static_method(
        cls,
        "ensureInstalled",
        "(Ljava/lang/Object;Ljava/lang/ClassLoader;)V",
        &[JValue::Object(minecraft), JValue::Object(&loader)],
    );
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftRenderHooks_nativeOnFlip(
    _env: JNIEnv,
    _class: JClass,
    hwnd: jni::sys::jlong,
    width: jni::sys::jint,
    height: jni::sys::jint,
) {
    ensure_engine();
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        let hwnd = HWND(hwnd as *mut _);
        let _ = with_engine_mut(|engine| {
            let _ = rsift_dx12::ensure_swap_chain(
                engine,
                hwnd,
                width.max(0) as u32,
                height.max(0) as u32,
            );
            if let Some(result) = rsift_opt_gfx::with_gpu_quad_bytes(|quads| {
                let cb = rsift_opt_gfx::production_frame_constants().map(|c| {
                    rsift_dx12::terrain_pass::TerrainFrameCb {
                        view_proj: c.view_proj,
                        chunk_origin: c.chunk_origin,
                    }
                });
                rsift_dx12::present_frame_with_cb(engine, Some(quads), cb.as_ref())
            }) {
                let _ = result;
            } else {
                let _ = rsift_dx12::present_frame(engine, None);
            }
        });
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftRenderHooks_nativeGlDraw(
    _env: JNIEnv,
    _class: JClass,
) -> jni::sys::jboolean {
    rsift_render::proxy::rsift_gl_should_draw() as jni::sys::jboolean
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftRenderHooks_nativeGlSwap(
    _env: JNIEnv,
    _class: JClass,
) {
    rsift_render::proxy::rsift_gl_on_swap_buffers();
}

/// Transpiler (BakedModel.getQuads HEAD) → 実測カウンタ記録。
#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftRenderHooks_nativeGetQuadsHook(
    _env: JNIEnv,
    _class: JClass,
) -> jni::sys::jlong {
    rsift_opt_gfx::note_vanilla_get_quads_hook() as jni::sys::jlong
}

/// Transpiler (LevelRenderer.renderChunkLayer HEAD) → 実測カウンタ記録。
#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftRenderHooks_nativeChunkLayerHook(
    _env: JNIEnv,
    _class: JClass,
) -> jni::sys::jlong {
    rsift_opt_gfx::note_vanilla_chunk_layer_hook() as jni::sys::jlong
}
