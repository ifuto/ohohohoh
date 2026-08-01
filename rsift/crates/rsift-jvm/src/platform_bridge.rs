//! JVM platform bridge — flushes Rust registries into Minecraft via RsiftPlatformBridge.

use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jfloat, jint, jlong, jstring};
use jni::JNIEnv;
use jni::NativeMethod;
use std::sync::Mutex;

use crate::agent_log::agent_log;
use super::screen_inject;

static PLATFORM_CLASS: Mutex<Option<jni::objects::GlobalRef>> = Mutex::new(None);

pub fn ensure_platform_bridge(env: &mut JNIEnv) -> bool {
    if platform_class(env).is_some() {
        return true;
    }
    if screen_inject::find_game_class_loader(env).is_none() {
        return false;
    }
    match load_platform(env) {
        Ok(_) => {
            agent_log("[Rsift] RsiftPlatformBridge loaded");
            true
        }
        Err(e) => {
            agent_log(&format!("[Rsift] WARN PlatformBridge load failed: {}", e));
            false
        }
    }
}

pub fn platform_class<'local>(env: &mut JNIEnv<'local>) -> Option<JClass<'local>> {
    let guard = PLATFORM_CLASS.lock().ok()?;
    let g = guard.as_ref()?;
    env.new_local_ref(g.as_obj()).ok().map(JClass::from)
}

fn load_platform(env: &mut JNIEnv) -> Result<(), String> {
    let jar = screen_inject::bootstrap_jar().ok_or("bootstrap jar path unknown")?;
    if !jar.is_file() {
        return Err(format!("bootstrap jar missing: {:?}", jar));
    }
    let parent = screen_inject::find_game_class_loader(env).ok_or("game ClassLoader not found")?;
    let ucl = screen_inject::url_classloader_for_jar(env, &parent, &jar)?;
    let name = env
        .new_string("com.rsift.RsiftPlatformBridge")
        .map_err(|e| format!("{:?}", e))?;
    let cls_obj = env
        .call_method(
            &ucl,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )
        .map_err(|e| format!("loadClass PlatformBridge: {:?}", e))?
        .l()
        .map_err(|e| format!("{:?}", e))?;
    let jclass = JClass::from(cls_obj);
    let methods = [
        NativeMethod {
            name: "nativeLog".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeLog as *mut _,
        },
        NativeMethod {
            name: "nativeNotifyReady".into(),
            sig: "()V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeNotifyReady as *mut _,
        },
        NativeMethod {
            name: "nativeRequestApply".into(),
            sig: "()V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeRequestApply as *mut _,
        },
        NativeMethod {
            name: "nativeLifecycle".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;JJJ)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeLifecycle as *mut _,
        },
        NativeMethod {
            name: "nativeScreenOpened".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeScreenOpened as *mut _,
        },
        NativeMethod {
            name: "nativePrepareHostButtons".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Z)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativePrepareHostButtons as *mut _,
        },
        NativeMethod {
            name: "nativeHandleRedirect".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeHandleRedirect as *mut _,
        },
        NativeMethod {
            name: "nativeTakeOutbound".into(),
            sig: "()[Ljava/lang/String;".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeTakeOutbound as *mut _,
        },
        NativeMethod {
            name: "nativeRegisterCommand".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;ILjava/lang/String;)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeRegisterCommand as *mut _,
        },
        NativeMethod {
            name: "nativeRegisterKey".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeRegisterKey as *mut _,
        },
        NativeMethod {
            name: "nativeRegisterBiomeRule".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;III)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeRegisterBiomeRule as *mut _,
        },
        NativeMethod {
            name: "nativeRegisterScreenHandler".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;I)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeRegisterScreenHandler as *mut _,
        },
        NativeMethod {
            name: "nativeRegisterLoot".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;III)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeRegisterLoot as *mut _,
        },
        NativeMethod {
            name: "nativeRegisterEntity".into(),
            sig: "(Ljava/lang/String;FF)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeRegisterEntity as *mut _,
        },
        NativeMethod {
            name: "nativeCustomPayload".into(),
            sig: "(Ljava/lang/String;JI)V".into(),
            fn_ptr: Java_com_rsift_RsiftPlatformBridge_nativeCustomPayload as *mut _,
        },
    ];
    env.register_native_methods(&jclass, &methods)
        .map_err(|e| format!("register PlatformBridge: {:?}", e))?;
    let global = env.new_global_ref(&jclass).map_err(|e| format!("{:?}", e))?;
    if let Ok(mut slot) = PLATFORM_CLASS.lock() {
        *slot = Some(global);
    }
    Ok(())
}

pub fn tick(env: &mut JNIEnv) {
    if !ensure_platform_bridge(env) {
        return;
    }
    let Some(minecraft) = screen_inject::minecraft_instance(env) else {
        return;
    };
    let Some(loader) = screen_inject::game_class_loader(env) else {
        return;
    };
    let Some(cls) = platform_class(env) else {
        return;
    };
    let _ = env.call_static_method(
        cls,
        "onClientTick",
        "(Ljava/lang/Object;Ljava/lang/ClassLoader;)V",
        &[JValue::Object(&minecraft), JValue::Object(&loader)],
    );
}

pub fn flush_apply(env: &mut JNIEnv) {
    if !rsift_api::platform::needs_apply() && rsift_api::platform::is_platform_ready() {
        // Still allow first apply after ready.
        if rsift_api::platform::wire_status_snapshot().applied {
            return;
        }
    }
    let Some(snap) = rsift_api::platform::collect_from_runtime() else {
        return;
    };
    let Ok(json) = rsift_api::platform::snapshot_to_json(&snap) else {
        return;
    };
    let Some(cls) = platform_class(env) else {
        return;
    };
    let Ok(jjson) = env.new_string(&json) else {
        return;
    };
    let result = env.call_static_method(
        cls,
        "applySnapshotJson",
        "(Ljava/lang/String;)Ljava/lang/String;",
        &[JValue::Object(&jjson)],
    );
    match result {
        Ok(v) => {
            if let Ok(obj) = v.l() {
                let js = JString::from(obj);
                let status_str = match env.get_string(&js) {
                    Ok(s) => {
                        let owned: String = s.into();
                        owned
                    }
                    Err(_) => return,
                };
                apply_status_json(&status_str);
            }
        }
        Err(e) => agent_log(&format!("[Platform] applySnapshot failed: {:?}", e)),
    }
}

fn apply_status_json(json: &str) {
    // Minimal parse of status fields written by Java.
    let mut status = rsift_api::platform::WireStatus::default();
    status.applied = json.contains("\"applied\":true");
    status.content_blocks = extract_u32(json, "content_blocks");
    status.content_items = extract_u32(json, "content_items");
    status.content_entities = extract_u32(json, "content_entities");
    status.commands = extract_u32(json, "commands");
    status.keybindings = extract_u32(json, "keybindings");
    status.render_layers = extract_u32(json, "render_layers");
    status.network_channels = extract_u32(json, "network_channels");
    status.biome_rules = extract_u32(json, "biome_rules");
    status.screen_redirects = extract_u32(json, "screen_redirects");
    status.images = extract_u32(json, "images");
    status.loot_modifiers = extract_u32(json, "loot_modifiers");
    if let Some(err) = extract_string(json, "last_error") {
        if err != "null" {
            status.last_error = Some(err);
        }
    }
    rsift_api::platform::record_apply_result(status);
}

fn extract_u32(json: &str, key: &str) -> u32 {
    let needle = format!("\"{}\":", key);
    if let Some(pos) = json.find(&needle) {
        let rest = &json[pos + needle.len()..];
        let num: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        return num.parse().unwrap_or(0);
    }
    0
}

fn extract_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\":", key);
    let pos = json.find(&needle)?;
    let rest = json[pos + needle.len()..].trim_start();
    if rest.starts_with("null") {
        return Some("null".into());
    }
    if !rest.starts_with('"') {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn jstr(env: &mut JNIEnv, s: JString) -> String {
    env.get_string(&s).map(|v| v.into()).unwrap_or_default()
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeLog(
    mut env: JNIEnv,
    _class: JClass,
    line: JString,
) {
    agent_log(&jstr(&mut env, line));
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeNotifyReady(
    _env: JNIEnv,
    _class: JClass,
) {
    rsift_api::platform::mark_platform_ready();
    agent_log("[Platform] ready");
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeRequestApply(
    mut env: JNIEnv,
    _class: JClass,
) {
    flush_apply(&mut env);
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeLifecycle(
    mut env: JNIEnv,
    _class: JClass,
    op: JString,
    a: JString,
    b: JString,
    n0: jlong,
    n1: jlong,
    n2: jlong,
) {
    let op_s = jstr(&mut env, op);
    let a_s = jstr(&mut env, a);
    let b_s = jstr(&mut env, b);
    rsift_api::mod_suite::mod_suite()
        .events
        .dispatch_named(&op_s, &a_s, &b_s, n0, n1, n2);
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeScreenOpened(
    mut env: JNIEnv,
    _class: JClass,
    kind: JString,
) {
    let k = jstr(&mut env, kind);
    rsift_api::platform::on_screen_opened(&k);
    if let Some(rt) = rsift_api::runtime::runtime() {
        if k == "mod_menu" || k == "mod_menu_detail" {
            // keep open flag
            let _ = rt.mod_menu.is_open.write().map(|mut g| *g = true);
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativePrepareHostButtons(
    mut env: JNIEnv,
    _class: JClass,
    title: JString,
    lines: JString,
    mod_menu: jboolean,
) {
    let title_s = jstr(&mut env, title);
    let lines_s = jstr(&mut env, lines);
    let Some(rt) = rsift_api::runtime::runtime() else {
        return;
    };
    let screen_key = "net.minecraft.client.gui.screens.PauseScreen";
    // ホスト画面は PauseScreen 単一キー運用のため、再構築の度に既存登録を
    // 明示クリアする (旧実装は追記のみで、一覧↔詳細の往復で行が累積重複した)。
    rt.screen_registry().clear_buttons_for(screen_key);
    let mut y = 40;
    rt.screen_registry().add_button(
        screen_key,
        &rsift_api::mc_style::title_text(&title_s),
        20,
        y,
        rsift_api::mc_style::BUTTON_W_FULL,
        20,
        Some("Rsift host screen"),
        |_| {},
    );
    y += 24;
    for (idx, line) in lines_s.lines().take(12).enumerate() {
        // Mod Menu 行群は行プロトコル (mod|..., info|..., act:*|...) で
        // 届くため、表示文言を剥がして使う。それ以外 (cloth_config 等) は
        // 素の行テキストをそのまま表示する従来挙動。
        let display = if mod_menu != 0 {
            rsift_api::mod_menu::row_label(line)
        } else {
            // 設定画面行はバニラ設定ボタン風ラベル ("<項目>: <値><単位>")。
            // raw の `int:min:max:cur` 仕様は表示に出さない (wave 201)。
            rsift_api::cloth_config::cloth_display_label(line)
        };
        // 行頭 40 chars で省略。旧実装は &line[..40] のバイト切断で、
        // マルチバイト文字の途中を割るとパニックする潜伏バグがあった。
        let label: String = if display.chars().count() > 40 {
            format!("{}…", display.chars().take(40).collect::<String>())
        } else {
            display
        };
        let is_mod = mod_menu != 0;
        let callback_line = line.to_string();
        rt.screen_registry().add_button(
            screen_key,
            &label,
            20,
            y,
            280,
            20,
            Some(line),
            move |_id| {
                if is_mod {
                    if let Some(rt) = rsift_api::runtime::runtime() {
                        match rsift_api::mod_menu::parse_row(&callback_line) {
                            rsift_api::mod_menu::ModRowAction::Select { id } => {
                                rt.mod_menu.select_mod(&id);
                                rsift_api::platform::request_open_screen("mod_menu_detail");
                            }
                            rsift_api::mod_menu::ModRowAction::OpenConfig => {
                                rt.mod_menu.open_selected_config();
                            }
                            rsift_api::mod_menu::ModRowAction::OpenHomepage => {
                                rt.mod_menu.open_selected_homepage();
                            }
                            rsift_api::mod_menu::ModRowAction::BackToList => {
                                rsift_api::platform::request_open_screen("mod_menu");
                            }
                            rsift_api::mod_menu::ModRowAction::Info
                            | rsift_api::mod_menu::ModRowAction::Unknown => {}
                        }
                    }
                } else {
                    // 設定画面行: 値サイクル → on_change(永続化) → 再描画要求。
                    rsift_api::cloth_config::press_row(&callback_line);
                }
                let _ = idx;
            },
        );
        y += 22;
    }
    rt.screen_registry().add_button(
        screen_key,
        "Back",
        20,
        y + 8,
        100,
        20,
        Some("Close"),
        |_| {
            if let Some(rt) = rsift_api::runtime::runtime() {
                rt.mod_menu.close_screen();
                // ホスト画面を閉じたら登録も撤去: 後で開くバニラ PauseScreen
                // へホスト行が混入しないようにする (旧実装では残留した)。
                rt.screen_registry()
                    .clear_buttons_for("net.minecraft.client.gui.screens.PauseScreen");
            }
        },
    );
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeHandleRedirect(
    mut env: JNIEnv,
    _class: JClass,
    from: JString,
    to: JString,
) {
    let from_s = jstr(&mut env, from);
    let to_s = jstr(&mut env, to);
    agent_log(&format!("[Platform] screen redirect {} -> {}", from_s, to_s));
    // Open cloth/mod menu or request render ticks for DLL symbol handlers.
    if to_s.contains("mod_menu") || to_s.contains("ModMenu") {
        if let Some(rt) = rsift_api::runtime::runtime() {
            rt.mod_menu.open_screen();
        }
    } else if to_s.contains("cloth") || to_s.contains("config") || to_s.contains("settings") {
        rsift_api::platform::request_open_screen("cloth_config");
    } else if let Some(rt) = rsift_api::runtime::runtime() {
        rt.request_render_ticks();
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeTakeOutbound(
    mut env: JNIEnv,
    _class: JClass,
) -> jni::sys::jobjectArray {
    let payloads = rsift_api::platform::take_outbound_payloads();
    let arr = env
        .new_object_array(
            payloads.len() as i32,
            "java/lang/String",
            JObject::null(),
        )
        .unwrap();
    for (i, (channel, bytes)) in payloads.into_iter().enumerate() {
        let b64 = encode_b64(&bytes);
        let line = format!("{}|{}", channel, b64);
        if let Ok(js) = env.new_string(&line) {
            let _ = env.set_object_array_element(&arr, i as i32, js);
        }
    }
    arr.into_raw()
}

fn encode_b64(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeRegisterCommand(
    mut env: JNIEnv,
    _class: JClass,
    name: JString,
    desc: JString,
    perm: jint,
    symbol: JString,
) {
    let name_s = jstr(&mut env, name);
    let desc_s = jstr(&mut env, desc);
    let sym = jstr(&mut env, symbol);
    let suite = rsift_api::mod_suite::mod_suite();
    if let Ok(mut gp) = suite.gameplay.write() {
        gp.command_tree.register(&name_s, &desc_s, perm as u8, &sym);
    }
    // Command tree upsert — skip mark_dirty to avoid apply echo loops.
    agent_log(&format!("[Platform] command /{} wired (perm={})", name_s, perm));
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeRegisterKey(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    symbol: JString,
) {
    let id_s = jstr(&mut env, id);
    let sym = jstr(&mut env, symbol);
    let suite = rsift_api::mod_suite::mod_suite();
    if let Ok(mut gp) = suite.gameplay.write() {
        gp.keybindings.insert(
            id_s.clone(),
            rsift_api::gameplay::KeyBindingDefinition {
                id: id_s.clone(),
                translation_key: id_s.clone(),
                default_key_code: 0,
                category: "key.categories.misc".into(),
                dll_on_press_symbol: sym.clone(),
            },
        );
    }
    agent_log(&format!("[Platform] keybinding {} -> {}", id_s, sym));
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeRegisterBiomeRule(
    mut env: JNIEnv,
    _class: JClass,
    selector: JString,
    kind: JString,
    feature: JString,
    step: jint,
    min: jint,
    max: jint,
) {
    let selector_s = jstr(&mut env, selector);
    let kind_s = jstr(&mut env, kind);
    let feature_s = jstr(&mut env, feature);
    let suite = rsift_api::mod_suite::mod_suite();
    if let Ok(mut content) = suite.content.write() {
        let biome_selector = match selector_s.as_str() {
            "all" => rsift_api::content::BiomeSelector::All,
            "overworld" => rsift_api::content::BiomeSelector::Overworld,
            "nether" => rsift_api::content::BiomeSelector::Nether,
            "the_end" => rsift_api::content::BiomeSelector::TheEnd,
            s if s.starts_with("tag:") => {
                rsift_api::content::BiomeSelector::Tag(s[4..].to_string())
            }
            s if s.starts_with("biome:") => {
                let parts: Vec<&str> = s[6..].splitn(2, ':').collect();
                if parts.len() == 2 {
                    rsift_api::content::BiomeSelector::Specific(
                        rsift_api::registry::RegistryKey::new(parts[0], parts[1]),
                    )
                } else {
                    rsift_api::content::BiomeSelector::All
                }
            }
            _ => rsift_api::content::BiomeSelector::All,
        };
        let feat_parts: Vec<&str> = feature_s.splitn(2, ':').collect();
        let feat_key = if feat_parts.len() == 2 {
            rsift_api::registry::RegistryKey::new(feat_parts[0], feat_parts[1])
        } else {
            rsift_api::registry::RegistryKey::new("rsift", &feature_s)
        };
        let already = content.biome_modifications.iter().any(|rule| {
            let sel_match = match (&rule.selector, &biome_selector) {
                (rsift_api::content::BiomeSelector::All, rsift_api::content::BiomeSelector::All) => true,
                (rsift_api::content::BiomeSelector::Overworld, rsift_api::content::BiomeSelector::Overworld) => true,
                (rsift_api::content::BiomeSelector::Nether, rsift_api::content::BiomeSelector::Nether) => true,
                (rsift_api::content::BiomeSelector::TheEnd, rsift_api::content::BiomeSelector::TheEnd) => true,
                (rsift_api::content::BiomeSelector::Tag(a), rsift_api::content::BiomeSelector::Tag(b)) => a == b,
                (rsift_api::content::BiomeSelector::Specific(a), rsift_api::content::BiomeSelector::Specific(b)) => a == b,
                _ => false,
            };
            if !sel_match {
                return false;
            }
            match &rule.modification {
                rsift_api::content::BiomeModificationType::AddFeature { step: s, feature_key } => {
                    kind_s != "spawn" && *s == step as u32 && feature_key == &feat_key
                }
                rsift_api::content::BiomeModificationType::AddSpawn {
                    entity_key,
                    weight,
                    min: mn,
                    max: mx,
                } => {
                    kind_s == "spawn"
                        && entity_key == &feat_key
                        && *weight == step as u32
                        && *mn == min as u32
                        && *mx == max as u32
                }
            }
        });
        if !already {
            let modification = if kind_s == "spawn" {
                rsift_api::content::BiomeModificationType::AddSpawn {
                    entity_key: feat_key,
                    weight: step as u32,
                    min: min as u32,
                    max: max as u32,
                }
            } else {
                rsift_api::content::BiomeModificationType::AddFeature {
                    step: step as u32,
                    feature_key: feat_key,
                }
            };
            // Direct push — avoid add_biome_modification's mark_dirty (apply echo).
            content.biome_modifications.push(rsift_api::content::BiomeModificationRule {
                selector: biome_selector,
                modification,
            });
        }
    }
    agent_log(&format!(
        "[Platform] biome rule {} {} {} step={} {}..{}",
        selector_s, kind_s, feature_s, step, min, max
    ));
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeRegisterScreenHandler(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    texture: JString,
    symbol: JString,
    type_id: jint,
) {
    let id_s = jstr(&mut env, id);
    let tex = jstr(&mut env, texture);
    let sym = jstr(&mut env, symbol);
    let suite = rsift_api::mod_suite::mod_suite();
    if let Ok(mut gp) = suite.gameplay.write() {
        let parts: Vec<&str> = id_s.splitn(2, ':').collect();
        let key = if parts.len() == 2 {
            rsift_api::registry::RegistryKey::new(parts[0], parts[1])
        } else {
            rsift_api::registry::RegistryKey::new("rsift", &id_s)
        };
        let handler_type_id = if type_id > 0 {
            type_id as u32
        } else {
            gp.screen_handlers.len() as u32 + 2000
        };
        gp.screen_handlers.insert(
            key.clone(),
            rsift_api::gameplay::ScreenHandlerDefinition {
                key,
                handler_type_id,
                gui_texture_path: tex.clone(),
                dll_init_symbol: sym.clone(),
            },
        );
    }
    agent_log(&format!(
        "[Platform] screen handler {} tex={} sym={} id={}",
        id_s, tex, sym, type_id
    ));
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeRegisterLoot(
    mut env: JNIEnv,
    _class: JClass,
    table: JString,
    item: JString,
    weight: jint,
    min: jint,
    max: jint,
) {
    let table_s = jstr(&mut env, table);
    let item_s = jstr(&mut env, item);
    let w = weight as u32;
    let min_c = min as u32;
    let max_c = max as u32;
    let suite = rsift_api::mod_suite::mod_suite();
    if let Ok(mut resources) = suite.resources.write() {
        let table_parts: Vec<&str> = table_s.splitn(2, ':').collect();
        let table_key = if table_parts.len() == 2 {
            rsift_api::registry::RegistryKey::new(table_parts[0], table_parts[1])
        } else {
            rsift_api::registry::RegistryKey::new("minecraft", &table_s)
        };
        let item_parts: Vec<&str> = item_s.splitn(2, ':').collect();
        let item_key = if item_parts.len() == 2 {
            rsift_api::registry::RegistryKey::new(item_parts[0], item_parts[1])
        } else {
            rsift_api::registry::RegistryKey::new("minecraft", &item_s)
        };
        // Probe existing modifiers for this table to avoid apply-loop duplication.
        let mut probe = Vec::new();
        resources.modify_loot_table(&table_key, &mut probe);
        let already = probe.iter().any(|e| {
            e.item_key == item_key && e.weight == w && e.min_count == min_c && e.max_count == max_c
        });
        if !already {
            resources.loot_table_modifiers.push(rsift_api::resources::BoundLootTableModifier {
                table: table_key,
                modifier: std::sync::Arc::new(move |_key, entries| {
                    entries.push(rsift_api::resources::LootPoolEntry {
                        item_key: item_key.clone(),
                        weight: w,
                        min_count: min_c,
                        max_count: max_c,
                    });
                }),
            });
        }
    }
    agent_log(&format!(
        "[Platform] loot {} += {} w={} {}..{}",
        table_s, item_s, weight, min, max
    ));
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeRegisterEntity(
    mut env: JNIEnv,
    _class: JClass,
    id: JString,
    hp: jfloat,
    speed: jfloat,
) {
    let id_s = jstr(&mut env, id);
    if let Some(rt) = rsift_api::runtime::runtime() {
        if let Ok(mut reg) = rt.registry.lock() {
            let parts: Vec<&str> = id_s.split(':').collect();
            if parts.len() == 2 {
                let _ = reg.register_entity(parts[0], parts[1], hp, speed);
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftPlatformBridge_nativeCustomPayload(
    mut env: JNIEnv,
    _class: JClass,
    channel: JString,
    ptr: jlong,
    len: jint,
) {
    let ch = jstr(&mut env, channel);
    let sender = rsift_api::networking::PlatformPacketSender;
    let _ = rsift_api::mod_suite::mod_suite().networking.dispatch_raw(
        &ch,
        ptr,
        len,
        true,
        &sender,
    );
}
