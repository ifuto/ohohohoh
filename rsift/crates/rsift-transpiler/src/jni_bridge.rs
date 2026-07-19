//! JNI C ABI bridge — JVM calls these; parity gate ensures zero spec change.
//! Return PARITY_OK (0) = native handled; PARITY_JVM_FALLBACK (1) = vanilla runs.

use crate::compute_runtime::ComputeRuntime;
use crate::parity::{ParityGate, ParityVerdict, PARITY_JVM_FALLBACK, PARITY_OK};
use crate::vanilla_tick::{vanilla_chunk_tick, vanilla_entity_travel, vanilla_mob_ai_step, vanilla_redstone_calculate, execute_vanilla_phases};
use crate::world_mirror::{JvmChunkState, JvmEntityState, JvmRedstoneState, WorldMirror};
use rsift_api::AdaptivePerfEngine;
use std::sync::{Mutex, OnceLock};
use tracing::info;

static RUNTIME: OnceLock<Mutex<Option<ComputeRuntime>>> = OnceLock::new();
static MIRROR: OnceLock<Mutex<WorldMirror>> = OnceLock::new();
static GATE: OnceLock<ParityGate> = OnceLock::new();

fn runtime_guard() -> &'static Mutex<Option<ComputeRuntime>> {
    RUNTIME.get_or_init(|| Mutex::new(None))
}

fn mirror_guard() -> &'static Mutex<WorldMirror> {
    MIRROR.get_or_init(|| Mutex::new(WorldMirror::new()))
}

fn gate() -> &'static ParityGate {
    GATE.get_or_init(|| ParityGate::new(true))
}

fn ensure_runtime_ready() {
    let _ = rsift_native_init();
}

/// Initialize RsCalc from JVM agent or launcher (idempotent)
#[no_mangle]
pub extern "C" fn rsift_native_init() -> i32 {
    let cp = AdaptivePerfEngine::compute_profile(AdaptivePerfEngine::hardware());
    if let Ok(mut guard) = runtime_guard().lock() {
        if guard.is_none() {
            let mut rt = ComputeRuntime::from_profile(cp);
            if let Err(e) = rt.init() {
                tracing::error!("[JNI] RsCalc init failed: {}", e);
                return 1;
            }
            *guard = Some(rt);
            info!("[JNI] RsCalc native bridge initialized (parity strict mode ON)");
        }
    }
    PARITY_OK
}

/// Sync world state from JVM DirectBuffer before tick
#[no_mangle]
pub extern "C" fn rsift_jvm_sync_world(ptr: i64, len: i32) -> i32 {
    if let Ok(mut guard) = runtime_guard().lock() {
        if let Some(ref mut rt) = *guard {
            rt.note_world_activity();
        }
    }
    if let Ok(mut mirror) = mirror_guard().lock() {
        match unsafe { mirror.sync_from_jni(ptr, len) } {
            Ok(()) => PARITY_OK,
            Err(e) => {
                gate().force_fallback(e);
                PARITY_JVM_FALLBACK
            }
        }
    } else {
        PARITY_JVM_FALLBACK
    }
}

/// Write native results back to JVM buffer
#[no_mangle]
pub extern "C" fn rsift_jvm_write_back(ptr: i64, len: i32) -> i32 {
    if let Ok(mirror) = mirror_guard().lock() {
        match unsafe { mirror.write_back_to_jni(ptr, len) } {
            Ok(()) => PARITY_OK,
            Err(_) => PARITY_JVM_FALLBACK,
        }
    } else {
        PARITY_JVM_FALLBACK
    }
}

/// Full server tick — vanilla phase order, parity-gated
#[no_mangle]
pub extern "C" fn rsift_native_server_tick(delta_ms: f32) -> i32 {
    ensure_runtime_ready();
    let entity_count = mirror_guard().lock().map(|m| m.entity_count()).unwrap_or(0);
    let has_mirror = mirror_guard().lock().map(|m| m.has_jvm_data()).unwrap_or(false);
    if !has_mirror && entity_count == 0 {
        return PARITY_OK;
    }

    let mut fallbacks = 0u32;

    // 1. Vanilla phases on JVM mirror (if synced)
    if let Ok(mut mirror) = mirror_guard().lock() {
        if mirror.has_jvm_data() {
            fallbacks += execute_vanilla_phases(&mut mirror, gate());
        }
    }

    // 2. Domain pipeline — ingest from mirror, tick, write-back
    if let Ok(mut guard) = runtime_guard().lock() {
        if let Some(ref mut rt) = *guard {
            let result = if let Ok(mut mirror) = mirror_guard().lock() {
                rt.tick_with_mirror(delta_ms, &mut mirror)
            } else {
                rt.tick_with_mirror_count(delta_ms, entity_count)
            };
            if result.budget_exceeded {
                fallbacks += 1;
            }
        }
    }

    if fallbacks > 0 {
        PARITY_JVM_FALLBACK
    } else {
        PARITY_OK
    }
}

/// Mob.aiStep(entity_id) — per-entity hook from bytecode redirect
#[no_mangle]
pub extern "C" fn rsift_native_mob_ai_step(entity_id: i32) -> i32 {
    ensure_runtime_ready();
    if let Ok(mut guard) = runtime_guard().lock() {
        if let Some(ref mut rt) = *guard {
            rt.note_world_activity();
        }
    }
    if let Ok(mut mirror) = mirror_guard().lock() {
        if let Some(e) = mirror.entities.iter_mut().find(|e| e.entity_id == entity_id) {
            return vanilla_mob_ai_step(e, gate()).as_code();
        }
    }
    gate().force_fallback("entity not in mirror").as_code()
}

/// Entity.travel(entity_id)
#[no_mangle]
pub extern "C" fn rsift_native_entity_travel(entity_id: i32) -> i32 {
    if let Ok(mut mirror) = mirror_guard().lock() {
        if let Some(e) = mirror.entities.iter_mut().find(|e| e.entity_id == entity_id) {
            return vanilla_entity_travel(e, gate()).as_code();
        }
    }
    gate().force_fallback("entity not in mirror").as_code()
}

/// RedstoneWireBlock.calculate(block index in mirror)
#[no_mangle]
pub extern "C" fn rsift_native_redstone_calculate(index: u32) -> i32 {
    if let Ok(mut mirror) = mirror_guard().lock() {
        if let Some(w) = mirror.redstone.get_mut(index as usize) {
            let neighbors = [w.strength; 4];
            return vanilla_redstone_calculate(w, &neighbors, gate()).as_code();
        }
    }
    gate().force_fallback("redstone index out of range").as_code()
}

/// LevelChunk.tick(chunk index)
#[no_mangle]
pub extern "C" fn rsift_native_chunk_tick(index: u32) -> i32 {
    if let Ok(mut mirror) = mirror_guard().lock() {
        if let Some(c) = mirror.chunks.get_mut(index as usize) {
            return vanilla_chunk_tick(c, gate()).as_code();
        }
    }
    gate().force_fallback("chunk index out of range").as_code()
}

/// Packet hook — zero-copy ingress; forwards to RsCalc mirror + all mod handlers.
#[no_mangle]
pub extern "C" fn rsift_native_on_packet(packet_id: u32, ptr: i64, len: i32) -> i32 {
    if let Some(rt) = rsift_api::runtime::runtime() {
        if rt.mods_loaded() && !rt.dispatch_packet(packet_id, ptr, len) {
            return PARITY_JVM_FALLBACK;
        }
    }
    if let Ok(guard) = runtime_guard().lock() {
        if let Some(ref rt) = *guard {
            if !rt.on_packet(packet_id, ptr, len) {
                return PARITY_JVM_FALLBACK;
            }
        }
    }
    // Ingest spawn/position packets into mirror
    if packet_id == 0x1A || packet_id == 0x22 {
        ingest_packet_to_mirror(packet_id, ptr, len);
    }
    PARITY_OK
}

fn ingest_packet_to_mirror(packet_id: u32, ptr: i64, len: i32) {
    use rsift_api::packet::{DirectBufferSlice, SpawnEntityPacket, PlayerPositionPacket};
    let Ok(slice) = (unsafe { DirectBufferSlice::from_raw_jni(ptr, len) }) else { return };
    if let Ok(mut mirror) = mirror_guard().lock() {
        match packet_id {
            0x22 => {
                if let Ok(spawn) = slice.as_pod::<SpawnEntityPacket>() {
                    mirror.entities.push(JvmEntityState {
                        entity_id: spawn.entity_id,
                        entity_type: spawn.entity_type,
                        pos_x: spawn.x, pos_y: spawn.y, pos_z: spawn.z,
                        vel_x: spawn.velocity_x as f64,
                        vel_y: spawn.velocity_y as f64,
                        vel_z: spawn.velocity_z as f64,
                        ..Default::default()
                    });
                    mirror.synced_from_jvm = true;
                }
            }
            0x1A => {
                if let Ok(pos) = slice.as_pod::<PlayerPositionPacket>() {
                    if let Some(e) = mirror.entities.first_mut() {
                        e.pos_x = pos.x; e.pos_y = pos.y; e.pos_z = pos.z;
                    }
                }
            }
            _ => {}
        }
    }
}

/// Get shared runtime for launcher (no duplicate init)
pub fn shared_runtime() -> Option<std::sync::MutexGuard<'static, Option<ComputeRuntime>>> {
    runtime_guard().lock().ok()
}

pub fn ensure_runtime_initialized() -> Result<(), String> {
    if rsift_native_init() != PARITY_OK {
        return Err("JNI runtime init failed".into());
    }
    Ok(())
}
