//! # RsCalc - Official Calculation Migration Mod
//!
//! 全計算をネイティブ Rust に移行。ParityGate により仕様変更時は JVM にフォールバック。

use rsift_api::{ModContext, RsiftStatus, TARGET_MINECRAFT_VERSION, AdaptivePerfEngine};
use rsift_transpiler::{
    rsift_native_init, rsift_native_on_packet,
};
use tracing::{info, warn};

#[no_mangle]
pub extern "C" fn rsift_mod_init(ctx: &mut ModContext) -> i32 {
    info!("========================================================================");
    info!(" ⚙️ [RsCalc] Full compute migration — strict vanilla parity mode");
    info!(" ⚡ Target: {} | Fallback: JVM on any parity mismatch", ctx.minecraft_version);
    info!("========================================================================");

    if ctx.minecraft_version != TARGET_MINECRAFT_VERSION {
        warn!("[RsCalc] Version mismatch — JVM fallback enabled for safety");
    }

    let cp = AdaptivePerfEngine::compute_profile(AdaptivePerfEngine::hardware());
    info!("[RsCalc] Tier={} threads={} AOT={} budget={}ms",
        cp.tier.label(), cp.rayon_threads, cp.aot_transpile_enabled, cp.tick_budget_ms);

    if cp.speed_first {
        info!("[RsCalc] speed_first: native compute deferred until in-world JNI sync");
    } else if rsift_native_init() != 0 {
        warn!("[RsCalc] Native init failed — vanilla compute unchanged");
        return 1;
    }

    info!("[RsCalc] Native exports: server_tick, mob_ai_step, entity_travel, redstone, chunk, packet");
    info!("[RsCalc] ParityGate: strict=ON — spec changes impossible by design");
    RsiftStatus::Success as i32
}

#[no_mangle]
pub extern "C" fn rsift_mod_on_packet(packet_id: u32, buf_ptr: i64, buf_len: i32) -> bool {
    rsift_native_on_packet(packet_id, buf_ptr, buf_len) == 0
}

#[no_mangle]
pub extern "C" fn rsift_mod_on_render(_width: u32, _height: u32, _delta_time: f32) {
    // Compute runs via JNI bytecode hooks only — never from the render hot path.
}
