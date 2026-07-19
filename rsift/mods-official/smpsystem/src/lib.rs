//! # Rsift SMP System (`smpsystem.dll`) — Economy Tuning & Rare Loot Modifier
//!
//! Intercepts `BlockDispenseLootEvent` when a Vault (`minecraft:vault`) dispenses
//! a Heavy Core (`minecraft:heavy_core`), applying a 25% removal/cancellation probability
//! to increase the rarity and value of Heavy Cores on multiplayer servers.
//!
//! NOTE: Completely silent execution (`ログとかは一切出さなくていい`) — zero console
//! or tracing logs are emitted during event checks or loot modification.

use rsift_api::{gameplay::BlockDispenseLootEvent, ModContext, RsiftStatus};
use std::sync::atomic::{AtomicU64, Ordering};

static SEED_STATE: AtomicU64 = AtomicU64::new(0x9E3779B97F4A7C15);

/// Fast deterministic pseudo-random roll (0..99) using Xoroshiro-style bit mixing.
#[inline(always)]
fn roll_percent(x: i32, y: i32, z: i32, salt: u64) -> u32 {
    let mut s = SEED_STATE.fetch_add(0xBF58476D1CE4E5B9, Ordering::Relaxed);
    s ^= x as u64 ^ ((y as u64) << 16) ^ ((z as u64) << 32) ^ salt;
    s = (s ^ (s >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    s = (s ^ (s >> 27)).wrapping_mul(0x94D049BB133111EB);
    s ^= s >> 31;
    (s % 100) as u32
}

#[no_mangle]
pub extern "C" fn rsift_mod_init(_ctx: &mut ModContext) -> i32 {
    // Completely silent startup/init — NO logging as requested
    RsiftStatus::Success as i32
}

/// Core interception entry point for `BlockDispenseLootEvent`.
/// Returns `true` if the event was modified/cancelled (Heavy Core removed with 25% chance),
/// `false` if unchanged.
#[no_mangle]
pub extern "C" fn smpsystem_on_block_dispense_loot(event: &mut BlockDispenseLootEvent) -> bool {
    // Check if the block is a Vault (`minecraft:vault`) and the item is Heavy Core (`minecraft:heavy_core`)
    if event.is_heavy_core_from_vault() {
        // 25% probability roll (`0..24`) -> exactly 25 out of 100 chance to vanish/cancel!
        let roll = roll_percent(event.pos[0], event.pos[1], event.pos[2], event.item_count as u64);
        if roll < 25 {
            event.set_cancelled(true);
            event.item_count = 0;
            // Zero logs emitted — silently vanishes to raise rarity!
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heavy_core_dispense_25_percent_chance() {
        let mut cancelled_count = 0;
        let total = 10000;
        for i in 0..total {
            let mut event = BlockDispenseLootEvent::new(
                "minecraft:vault",
                [i, 64, i * 2],
                "minecraft:heavy_core",
                1,
            );
            if smpsystem_on_block_dispense_loot(&mut event) {
                assert!(event.cancelled);
                assert_eq!(event.item_count, 0);
                cancelled_count += 1;
            }
        }
        let rate = cancelled_count as f64 / total as f64;
        assert!((rate - 0.25).abs() < 0.03, "Should vanish with ~25% probability, got rate={:.3}", rate);
    }

    #[test]
    fn test_non_heavy_core_untouched() {
        let mut event = BlockDispenseLootEvent::new(
            "minecraft:vault",
            [10, 64, 10],
            "minecraft:diamond",
            1,
        );
        assert!(!smpsystem_on_block_dispense_loot(&mut event));
        assert!(!event.cancelled);
        assert_eq!(event.item_count, 1);
    }
}
