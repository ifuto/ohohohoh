//! JNI bridge: RsiftChunkBridge → rsift-opt-gfx world column store.

use jni::objects::{JClass, JShortArray, JString, JValue};
use jni::sys::{jfloat, jint};
use jni::{JNIEnv, NativeMethod};
use tracing::debug;

use super::screen_inject;
use crate::agent_log::agent_log;

static mut CHUNK_BRIDGE_READY: bool = false;

pub fn ensure(env: &mut JNIEnv) -> bool {
    unsafe {
        if CHUNK_BRIDGE_READY {
            return true;
        }
    }
    let Some(loader) = screen_inject::game_class_loader(env) else {
        return false;
    };
    let Ok(name) = env.new_string("com.rsift.RsiftChunkBridge") else {
        return false;
    };
    let cls = match env.call_method(
        loader,
        "loadClass",
        "(Ljava/lang/String;)Ljava/lang/Class;",
        &[JValue::Object(&name)],
    ) {
        Ok(v) => match v.l() {
            Ok(o) => JClass::from(o),
            Err(_) => return false,
        },
        Err(_) => return false,
    };

    let natives = [
        NativeMethod {
            name: "nativeLog".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: Java_com_rsift_RsiftChunkBridge_nativeLog as *mut _,
        },
        NativeMethod {
            name: "nativeSetCamera".into(),
            sig: "(FFFFF)V".into(),
            fn_ptr: Java_com_rsift_RsiftChunkBridge_nativeSetCamera as *mut _,
        },
        NativeMethod {
            name: "nativeIngestColumn".into(),
            sig: "(III[SI)V".into(),
            fn_ptr: Java_com_rsift_RsiftChunkBridge_nativeIngestColumn as *mut _,
        },
        NativeMethod {
            name: "nativePrune".into(),
            sig: "(III)V".into(),
            fn_ptr: Java_com_rsift_RsiftChunkBridge_nativePrune as *mut _,
        },
    ];
    if env.register_native_methods(&cls, &natives).is_err() {
        return false;
    }
    let _ = env.call_static_method(&cls, "markNativesReady", "()V", &[]);
    // C ABI VTable を実インストール (実自己検証つき: ログに 1 行出る)。
    crate::c_abi_vtable::install_vtable();
    unsafe {
        CHUNK_BRIDGE_READY = true;
    }
    agent_log("[Rsift] RsiftChunkBridge natives ready");
    true
}

/// Reflectively pull ClientLevel columns into the native mesher store.
pub fn sync(env: &mut JNIEnv) {
    if !ensure(env) {
        return;
    }
    let mc = screen_inject::minecraft_instance(env);
    let ld = screen_inject::game_class_loader(env);
    use std::sync::atomic::{AtomicBool, Ordering};
    static SYNC_DIAG: AtomicBool = AtomicBool::new(false);
    if !SYNC_DIAG.swap(true, Ordering::SeqCst) {
        agent_log(&format!(
            "[ChunkBridge] sync: minecraft={} loader={}",
            mc.is_some(), ld.is_some()
        ));
    }
    let Some(minecraft) = mc else { return; };
    let Some(loader) = ld else { return; };
    let Ok(name) = env.new_string("com.rsift.RsiftChunkBridge") else {
        return;
    };
    let loader_ref = &loader;
    let cls = match env.call_method(
        loader_ref,
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
        "syncFromMinecraft",
        "(Ljava/lang/Object;Ljava/lang/ClassLoader;)V",
        &[JValue::Object(&minecraft), JValue::Object(loader_ref)],
    );
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftChunkBridge_nativeLog(
    mut env: JNIEnv,
    _class: JClass,
    line: JString,
) {
    if let Ok(s) = env.get_string(&line) {
        agent_log(&s.to_string_lossy());
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftChunkBridge_nativeSetCamera(
    _env: JNIEnv,
    _class: JClass,
    x: jfloat,
    y: jfloat,
    z: jfloat,
    yaw: jfloat,
    pitch: jfloat,
) {
    rsift_opt_gfx::set_world_camera(x, y, z, yaw, pitch);
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftChunkBridge_nativeIngestColumn(
    mut env: JNIEnv,
    _class: JClass,
    cx: jint,
    cz: jint,
    base_section_y: jint,
    blocks: JShortArray,
    section_count: jint,
) {
    if section_count <= 0 {
        return;
    }
    let len = match env.get_array_length(&blocks) {
        Ok(n) => n as usize,
        Err(_) => return,
    };
    let need = section_count as usize * 4096;
    if len < need {
        return;
    }
    let mut buf = vec![0i16; need];
    if env.get_short_array_region(&blocks, 0, &mut buf).is_err() {
        return;
    }
    let u16s: Vec<u16> = buf.iter().map(|&s| s as u16).collect();
    rsift_opt_gfx::ingest_world_column(cx, cz, base_section_y, &u16s, section_count as usize);
    debug!(
        "[ChunkBridge] ingest ({}, {}) base_sy={} sections={}",
        cx, cz, base_section_y, section_count
    );
}

#[no_mangle]
pub unsafe extern "system" fn Java_com_rsift_RsiftChunkBridge_nativePrune(
    _env: JNIEnv,
    _class: JClass,
    cx: jint,
    cz: jint,
    radius: jint,
) {
    rsift_opt_gfx::prune_world_columns(cx, cz, radius);
}
