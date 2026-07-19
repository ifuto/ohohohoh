//! TitleScreen / GUI button injection — Rust JNI + render-thread scheduling.

use jni::objects::{JClass, JObject, JValue};
use jni::JNIEnv;

use crate::agent_bridge::{is_screen_injected, mark_screen_injected};
use crate::agent_log::{agent_log, agent_log_step};

/// Schedule injection on the Minecraft client thread (`minecraft.execute`).
pub fn inject_current_screen_on_render_thread(env: &mut JNIEnv, minecraft: &JObject) {
    if super::screen_inject::ensure_screen_hooks(env) {
        if let Some(hooks) = super::screen_inject::hooks_class(env) {
            let _ = env.call_static_method(
                hooks,
                "injectCurrentScreen",
                "(Ljava/lang/Object;)V",
                &[JValue::Object(minecraft)],
            );
            super::screen_inject::clear_pending_exception(env);
            return;
        }
    }
    inject_screen_now_rust(env, minecraft);
}

fn inject_screen_now_rust(env: &mut JNIEnv, minecraft: &JObject) {
    let screen = env
        .call_method(
            minecraft,
            "screen",
            "()Lnet/minecraft/client/gui/screens/Screen;",
            &[],
        )
        .ok()
        .and_then(|v| v.l().ok())
        .or_else(|| {
            env.call_method(
                minecraft,
                "getScreen",
                "()Lnet/minecraft/client/gui/screens/Screen;",
                &[],
            )
            .ok()
            .and_then(|v| v.l().ok())
        });
    let Some(screen) = screen else {
        return;
    };
    let Some(name) = screen_class_name(env, &screen) else {
        return;
    };
    if !is_injectable_screen(&name) {
        return;
    }
    let key = screen.as_raw() as usize;
    if is_screen_injected(key) {
        return;
    }
    let Some(loader) = super::screen_inject::game_class_loader(env)
        .or_else(|| super::screen_inject::find_game_class_loader(env))
    else {
        return;
    };
    match inject_buttons_rust(env, &screen, &name, &loader) {
        Ok(n) if n > 0 => {
            mark_screen_injected(key);
            agent_log_step("screen_buttons", &format!("OK {} +{} on {}", name, n, key));
        }
        Ok(_) => {}
        Err(e) => agent_log(&format!("[ScreenButtons] FAIL {}: {}", name, e)),
    }
}

/// Inject registered buttons onto `screen` (must run on client/render thread).
pub fn inject_buttons_rust<'local>(
    env: &mut JNIEnv<'local>,
    screen: &JObject<'local>,
    screen_class: &str,
    loader: &JObject<'local>,
) -> Result<u32, String> {
    let Some(rt) = rsift_api::runtime::runtime() else {
        return Err("runtime not ready".into());
    };
    let (reg_key, buttons) = rt
        .screen_registry()
        .buttons_for_screen_matched(screen_class);
    if buttons.is_empty() {
        return Ok(0);
    }

    let button_cls = load_class(env, loader, "net.minecraft.client.gui.components.Button")?;
    let component_cls = load_class(env, loader, "net.minecraft.network.chat.Component")?;
    let on_press_cls = load_class(
        env,
        loader,
        "net.minecraft.client.gui.components.Button$OnPress",
    )?;
    let press_handler_cls = super::screen_inject::ensure_press_handler_class(env, loader)?;

    let sw = call_int(env, screen, "width")?;
    let sh = call_int(env, screen, "height")?;

    let mut added = 0u32;
    for (i, desc) in buttons.iter().enumerate() {
        let mut x = desc.rect.x;
        let mut y = desc.rect.y;
        if screen_class.contains("TitleScreen") || reg_key.contains("TitleScreen") {
            x = sw / 2 - desc.rect.width / 2;
            y = sh / 4 + 72 + (i as i32) * 24;
        }

        let label = env
            .new_string(&desc.label)
            .map_err(|e| format!("label: {:?}", e))?;
        let text = env
            .call_static_method(
                &component_cls,
                "literal",
                "(Ljava/lang/String;)Lnet/minecraft/network/chat/Component;",
                &[JValue::Object(&JObject::from(label))],
            )
            .map_err(|e| format!("literal: {:?}", e))?
            .l()
            .map_err(|e| format!("{:?}", e))?;

        let on_press_obj = create_press_proxy(
            env,
            loader,
            &press_handler_cls,
            &on_press_cls,
            desc.id,
        )?;

        let builder_obj = env
            .call_static_method(
                &button_cls,
                "builder",
                "(Lnet/minecraft/network/chat/Component;Lnet/minecraft/client/gui/components/Button$OnPress;)Lnet/minecraft/client/gui/components/Button$Builder;",
                &[JValue::Object(&text), JValue::Object(&on_press_obj)],
            )
            .map_err(|e| format!("builder: {:?}", e))?
            .l()
            .map_err(|e| format!("{:?}", e))?;

        let builder = JObject::from(builder_obj);
        let bounded = env
            .call_method(
                &builder,
                "bounds",
                "(IIII)Lnet/minecraft/client/gui/components/Button$Builder;",
                &[
                    JValue::Int(x),
                    JValue::Int(y),
                    JValue::Int(desc.rect.width),
                    JValue::Int(desc.rect.height),
                ],
            )
            .map_err(|e| format!("bounds: {:?}", e))?
            .l()
            .map_err(|e| format!("{:?}", e))?;

        let btn_obj = env
            .call_method(
                &JObject::from(bounded),
                "build",
                "()Lnet/minecraft/client/gui/components/Button;",
                &[],
            )
            .map_err(|e| format!("build: {:?}", e))?
            .l()
            .map_err(|e| format!("{:?}", e))?;

        env.call_method(
            screen,
            "addRenderableWidget",
            "(Lnet/minecraft/client/gui/components/AbstractWidget;)Lnet/minecraft/client/gui/components/AbstractWidget;",
            &[JValue::Object(&JObject::from(btn_obj))],
        )
        .map_err(|e| format!("addRenderableWidget: {:?}", e))?;

        added += 1;
    }

    let _ = env.call_method(screen, "repositionElements", "()V", &[]);
    agent_log(&format!(
        "[ScreenButtons] injected {} button(s) on {} ({}x{})",
        added, screen_class, sw, sh
    ));
    Ok(added)
}

fn create_press_proxy<'local>(
    env: &mut JNIEnv<'local>,
    loader: &JObject<'local>,
    handler_cls: &JClass<'local>,
    on_press_iface: &JClass<'local>,
    button_id: u32,
) -> Result<JObject<'local>, String> {
    let handler = env
        .new_object(handler_cls, "(I)V", &[JValue::Int(button_id as i32)])
        .map_err(|e| format!("handler new: {:?}", e))?;
    let handler_obj: JObject = handler.into();

    let proxy_cls = load_class(env, loader, "java.lang.reflect.Proxy")?;
    let class_arr = env
        .new_object_array(1, "java/lang/Class", JObject::null())
        .map_err(|e| format!("{:?}", e))?;
    env.set_object_array_element(&class_arr, 0, on_press_iface)
        .map_err(|e| format!("{:?}", e))?;

    let proxy = env
        .call_static_method(
            proxy_cls,
            "newProxyInstance",
            "(Ljava/lang/ClassLoader;[Ljava/lang/Class;Ljava/lang/reflect/InvocationHandler;)Ljava/lang/Object;",
            &[
                JValue::Object(loader),
                JValue::Object(&JObject::from(class_arr)),
                JValue::Object(&handler_obj),
            ],
        )
        .map_err(|e| format!("newProxyInstance: {:?}", e))?
        .l()
        .map_err(|e| format!("{:?}", e))?;
    Ok(JObject::from(proxy))
}

fn load_class<'local>(
    env: &mut JNIEnv<'local>,
    loader: &JObject<'local>,
    dotted: &str,
) -> Result<JClass<'local>, String> {
    let name = env
        .new_string(dotted)
        .map_err(|e| format!("{:?}", e))?;
    let obj = env
        .call_method(
            loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&JObject::from(name))],
        )
        .map_err(|e| format!("loadClass {}: {:?}", dotted, e))?
        .l()
        .map_err(|e| format!("{:?}", e))?;
    Ok(JClass::from(obj))
}

fn call_int(env: &mut JNIEnv, obj: &JObject, method: &str) -> Result<i32, String> {
    env.call_method(obj, method, "()I", &[])
        .map_err(|e| format!("{}: {:?}", method, e))?
        .i()
        .map_err(|e| format!("{:?}", e))
}

fn screen_class_name(env: &mut JNIEnv, screen: &JObject) -> Option<String> {
    let class_obj = env
        .call_method(screen, "getClass", "()Ljava/lang/Class;", &[])
        .ok()?
        .l()
        .ok()?;
    let name = env
        .call_method(&class_obj, "getName", "()Ljava/lang/String;", &[])
        .ok()?
        .l()
        .ok()?;
    env.get_string((&name).into()).ok().map(|s| s.into())
}

fn is_injectable_screen(name: &str) -> bool {
    name.contains("TitleScreen")
        || name.contains("PauseScreen")
        || rsift_api::runtime::runtime()
            .map(|rt| {
                !rt
                    .screen_registry()
                    .buttons_for_screen_matched(name)
                    .1
                    .is_empty()
            })
            .unwrap_or(false)
}
