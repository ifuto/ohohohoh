//! Unified JVM ↔ native mod event dispatch (stable ABI — mods never touch this directly).

use crate::mod_suite::mod_suite;
use crate::runtime::runtime;
use tracing::{debug, warn};

/// Dispatch a network packet to all loaded mod `rsift_mod_on_packet` handlers.
pub fn dispatch_packet(packet_id: u32, buf_ptr: i64, buf_len: i32) -> bool {
    let Some(rt) = runtime() else {
        debug!("[ModDispatch] packet dropped (runtime not ready) id={:#x}", packet_id);
        return true;
    };
    if !rt.mods_loaded() {
        return true;
    }
    rt.dispatch_packet(packet_id, buf_ptr, buf_len)
}

/// Dispatch a client render frame to all loaded mod `rsift_mod_on_render` handlers.
pub fn dispatch_render(width: u32, height: u32, delta_time: f32) {
    let Some(rt) = runtime() else {
        return;
    };
    if !rt.mods_loaded() {
        return;
    }
    rt.dispatch_render(width, height, delta_time);
    mod_suite().on_client_render(width, height, delta_time);
}

/// Query the combined FOV scale declared by mods (wave 201: RsZoom).
/// Runtime 未到達 / mod 未ロード / export 無し mod のみ、のいずれでも恒等 1.0。
/// 消費者はレンダースレッド (opt-gfx camera_zoom → DX12/wgpu 射影) のみ。
pub fn query_fov_scale() -> f32 {
    let Some(rt) = runtime() else {
        return 1.0;
    };
    if !rt.mods_loaded() {
        return 1.0;
    }
    rt.query_fov_scale()
}

/// Generic dispatch entry for Java `RsiftModBridge.nativeDispatch(op, a, b, c)`.
pub fn dispatch_op(op: &str, a: i64, b: i64, c: i32) {
    // セキュリティガード (wave 195): JNI ブリッジは呼出 Mod を帰属できないため、
    // ホスト特権系予約接頭辞 `host.` の op は構造的に拒否する。既存 op
    // (packet/render/client_tick/channel) はゲーム API 表面として無制限を維持。
    if !crate::mod_security::jni_op_allowed(op) {
        crate::mod_security::SecurityGate::note_jni_host_op_refusal(op);
        warn!(
            "[modsec] JNI host-privileged op refused (bridge cannot attribute caller): {}",
            op
        );
        return;
    }
    match op {
        "packet" => {
            let _ = dispatch_packet(a as u32, b, c);
        }
        "render" => {
            let delta = f32::from_bits(c as u32);
            dispatch_render(a as u32, b as u32, delta);
        }
        "client_tick" => {
            mod_suite().on_client_tick();
        }
        "channel" => {
            // a unused; b=ptr; c=len; channel name arrives via separate path — treat as raw packet id hash
            let sender = crate::networking::PlatformPacketSender;
            let _ = mod_suite().networking.dispatch_raw(
                "rsift:custom",
                b,
                c,
                true,
                &sender,
            );
        }
        other => debug!("[ModDispatch] unknown op: {}", other),
    }
}
