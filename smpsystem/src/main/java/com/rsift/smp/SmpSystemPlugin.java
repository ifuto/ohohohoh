package com.rsift.smp;

import org.bukkit.Material;
import org.bukkit.event.EventHandler;
import org.bukkit.event.EventPriority;
import org.bukkit.event.Listener;
import org.bukkit.event.block.BlockDispenseLootEvent;
import org.bukkit.inventory.ItemStack;
import org.bukkit.plugin.java.JavaPlugin;
import java.util.concurrent.ThreadLocalRandom;

/**
 * SMPSystem (Version 1.0.2) — Official Paper Server Plugin for Rsift 1.21.11
 * 
 * Features:
 * - Economy Tuning & Rare Loot Modifier (`BlockDispenseLootEvent`)
 * - When a Vault (`Material.VAULT`) dispenses a Heavy Core (`Material.HEAVY_CORE`),
 *   applies a 25% probability to cancel/clear the drop, raising its economic rarity.
 * - AntiXRay features removed/disabled as requested (`AntiXRayの機能は消して`).
 * - Zero console or server logs emitted (`ログとかは一切出さなくていい`).
 */
public class SmpSystemPlugin extends JavaPlugin implements Listener {

    @Override
    public void onEnable() {
        // Register BlockDispenseLootEvent listener with zero logging
        getServer().getPluginManager().registerEvents(this, this);
    }

    @Override
    public void onDisable() {
        // Silent shutdown
    }

    @EventHandler(priority = EventPriority.HIGHEST, ignoreCancelled = true)
    public void onBlockDispenseLoot(BlockDispenseLootEvent event) {
        // Detect Vault dispensing Heavy Core
        if (event.getBlock().getType() == Material.VAULT) {
            ItemStack loot = event.getDispensedLoot();
            if (loot != null && loot.getType() == Material.HEAVY_CORE) {
                // 25% probability roll (`nextInt(100) < 25`) to cancel drop
                if (ThreadLocalRandom.current().nextInt(100) < 25) {
                    event.setCancelled(true);
                    event.setDispensedLoot(null);
                    // Zero logs emitted (`ログとかは一切出さなくていい`)
                }
            }
        }
    }
}
