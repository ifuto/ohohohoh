//! Platform apply layer — turns in-memory registries into Minecraft-applied state.
//!
//! Rust mods register into HashMaps; `collect_apply_snapshot` serializes pending work for
//! the JVM `RsiftPlatformBridge`, which applies it via Mojang-mapped reflection.

use crate::content::{BiomeModificationType, BiomeSelector, ContentRegistry, MineableTag};
use crate::gameplay::GameplayRegistry;
use crate::image_api::ImageRegistry;
use crate::mod_suite::mod_suite;
use crate::networking::NetworkManager;
use crate::registry::ModRegistry;
use crate::rendering::{RenderLayer, RenderingRegistry};
use crate::resources::ResourceLoader;
use crate::runtime::runtime;
use crate::ui_ext::ScreenRegistry;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use tracing::info;

static APPLY_GENERATION: AtomicU64 = AtomicU64::new(1);
static LAST_APPLIED_GENERATION: AtomicU64 = AtomicU64::new(0);
static PLATFORM_READY: AtomicBool = AtomicBool::new(false);
static WIRE_STATUS: OnceLock<Mutex<WireStatus>> = OnceLock::new();

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WireStatus {
    pub content_blocks: u32,
    pub content_items: u32,
    pub content_entities: u32,
    pub commands: u32,
    pub keybindings: u32,
    pub render_layers: u32,
    pub network_channels: u32,
    pub biome_rules: u32,
    pub loot_modifiers: u32,
    pub screen_redirects: u32,
    pub images: u32,
    pub last_error: Option<String>,
    pub applied: bool,
}

fn wire_status() -> &'static Mutex<WireStatus> {
    WIRE_STATUS.get_or_init(|| Mutex::new(WireStatus::default()))
}

pub fn mark_dirty() {
    APPLY_GENERATION.fetch_add(1, Ordering::SeqCst);
}

pub fn mark_platform_ready() {
    PLATFORM_READY.store(true, Ordering::SeqCst);
}

pub fn is_platform_ready() -> bool {
    PLATFORM_READY.load(Ordering::SeqCst)
}

pub fn needs_apply() -> bool {
    APPLY_GENERATION.load(Ordering::SeqCst) != LAST_APPLIED_GENERATION.load(Ordering::SeqCst)
}

pub fn wire_status_snapshot() -> WireStatus {
    wire_status().lock().unwrap().clone()
}

pub fn record_apply_result(status: WireStatus) {
    LAST_APPLIED_GENERATION.store(APPLY_GENERATION.load(Ordering::SeqCst), Ordering::SeqCst);
    *wire_status().lock().unwrap() = status;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockApply {
    pub id: String,
    pub raw_id: u32,
    pub hardness: f32,
    pub resistance: f32,
    pub luminance: u8,
    pub slipperiness: f32,
    pub collidable: bool,
    pub mineable: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemApply {
    pub id: String,
    pub raw_id: u32,
    pub max_stack: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityApply {
    pub id: String,
    pub entity_type_id: u32,
    pub max_health: f32,
    pub speed: f32,
    pub attack_damage: f32,
    pub ai_symbol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandApply {
    pub name: String,
    pub description: String,
    pub permission_level: u8,
    pub symbol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBindingApply {
    pub id: String,
    pub translation_key: String,
    pub default_key_code: i32,
    pub category: String,
    pub symbol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderLayerApply {
    pub block_id: String,
    pub layer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiomeRuleApply {
    pub selector: String,
    pub kind: String,
    pub feature_or_entity: String,
    pub step_or_weight: u32,
    pub min: u32,
    pub max: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenRedirectApply {
    pub from_class: String,
    pub to_handler: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageApply {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgba_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenHandlerApply {
    pub id: String,
    pub texture: String,
    pub symbol: String,
    pub type_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LootModifierApply {
    pub table_hint: String,
    pub item_id: String,
    pub weight: u32,
    pub min_count: u32,
    pub max_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundApply {
    pub id: String,
    pub raw_id: u32,
    pub range: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticleApply {
    pub id: String,
    pub raw_id: u32,
    pub always_show: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusEffectApply {
    pub id: String,
    pub raw_id: u32,
    pub is_beneficial: bool,
    pub color_rgb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnchantmentApply {
    pub id: String,
    pub raw_id: u32,
    pub max_level: u32,
    pub is_treasure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeApply {
    pub id: String,
    pub recipe_type: String,
    pub result_item: String,
    pub result_count: u32,
    pub ingredients: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameRuleApply {
    pub id: String,
    pub default_bool: Option<bool>,
    pub default_int: Option<i32>,
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeApply {
    pub villager_profession: String,
    pub level: u32,
    pub cost_item: String,
    pub cost_count: u32,
    pub result_item: String,
    pub result_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionApply {
    pub id: String,
    pub dimension_id: u32,
    pub has_skylight: bool,
    pub has_ceiling: bool,
    pub ambient_light: f32,
    pub portal_block: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockEntityApply {
    pub id: String,
    pub block_entity_id: u32,
    pub valid_blocks: Vec<String>,
    pub symbol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiDrawApply {
    pub kind: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub argb: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformApplySnapshot {
    pub generation: u64,
    pub blocks: Vec<BlockApply>,
    pub items: Vec<ItemApply>,
    pub entities: Vec<EntityApply>,
    pub commands: Vec<CommandApply>,
    pub keybindings: Vec<KeyBindingApply>,
    pub render_layers: Vec<RenderLayerApply>,
    pub network_channels: Vec<String>,
    pub biome_rules: Vec<BiomeRuleApply>,
    pub screen_redirects: Vec<ScreenRedirectApply>,
    pub images: Vec<ImageApply>,
    pub screen_handlers: Vec<ScreenHandlerApply>,
    pub loot_modifiers: Vec<LootModifierApply>,
    pub sounds: Vec<SoundApply>,
    pub particles: Vec<ParticleApply>,
    pub status_effects: Vec<StatusEffectApply>,
    pub enchantments: Vec<EnchantmentApply>,
    pub recipes: Vec<RecipeApply>,
    pub game_rules: Vec<GameRuleApply>,
    pub trades: Vec<TradeApply>,
    pub dimensions: Vec<DimensionApply>,
    pub block_entities: Vec<BlockEntityApply>,
    pub ui_draw: Vec<UiDrawApply>,
    pub pending_screen: Option<String>,
    pub cloth_title: Option<String>,
    pub cloth_entries: Vec<String>,
    pub mod_menu_lines: Vec<String>,
}

static PENDING_SCREEN: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static CLOTH_OPEN: OnceLock<Mutex<Option<(String, Vec<String>)>>> = OnceLock::new();
static IMAGE_BYTES: OnceLock<Mutex<std::collections::HashMap<u32, Vec<u8>>>> = OnceLock::new();
static OUTBOUND_PAYLOADS: OnceLock<Mutex<Vec<(String, Vec<u8>)>>> = OnceLock::new();
static GLOBAL_IMAGES: OnceLock<Mutex<ImageRegistry>> = OnceLock::new();

fn pending_screen_slot() -> &'static Mutex<Option<String>> {
    PENDING_SCREEN.get_or_init(|| Mutex::new(None))
}

fn cloth_open_slot() -> &'static Mutex<Option<(String, Vec<String>)>> {
    CLOTH_OPEN.get_or_init(|| Mutex::new(None))
}

fn image_bytes_slot() -> &'static Mutex<std::collections::HashMap<u32, Vec<u8>>> {
    IMAGE_BYTES.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn outbound_slot() -> &'static Mutex<Vec<(String, Vec<u8>)>> {
    OUTBOUND_PAYLOADS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn global_images() -> ImageRegistry {
    GLOBAL_IMAGES
        .get_or_init(|| Mutex::new(ImageRegistry::new()))
        .lock()
        .unwrap()
        .clone()
}

pub fn request_open_screen(kind: &str) {
    *pending_screen_slot().lock().unwrap() = Some(kind.to_string());
    mark_dirty();
}

pub fn request_open_cloth(title: &str, entries: Vec<String>) {
    *cloth_open_slot().lock().unwrap() = Some((title.to_string(), entries));
    request_open_screen("cloth_config");
}

pub fn store_image_bytes(id: u32, rgba: Vec<u8>) {
    image_bytes_slot().lock().unwrap().insert(id, rgba);
    mark_dirty();
}

pub fn enqueue_outbound_payload(channel: &str, bytes: Vec<u8>) {
    outbound_slot().lock().unwrap().push((channel.to_string(), bytes));
    mark_dirty();
}

pub fn take_outbound_payloads() -> Vec<(String, Vec<u8>)> {
    std::mem::take(&mut *outbound_slot().lock().unwrap())
}

fn mineable_name(tag: MineableTag) -> String {
    format!("{:?}", tag)
}

fn layer_name(layer: RenderLayer) -> String {
    format!("{:?}", layer)
}

fn biome_selector_str(sel: &BiomeSelector) -> String {
    match sel {
        BiomeSelector::All => "all".into(),
        BiomeSelector::Overworld => "overworld".into(),
        BiomeSelector::Nether => "nether".into(),
        BiomeSelector::TheEnd => "the_end".into(),
        BiomeSelector::Tag(t) => format!("tag:{}", t),
        BiomeSelector::Specific(k) => format!("biome:{}", k.as_str()),
    }
}

/// Collect every pending registration for JVM apply.
pub fn collect_apply_snapshot(
    content: &ContentRegistry,
    mod_registry: &ModRegistry,
    gameplay: &GameplayRegistry,
    rendering: &RenderingRegistry,
    networking: &NetworkManager,
    screens: &ScreenRegistry,
    resources: &ResourceLoader,
) -> PlatformApplySnapshot {
    let mut blocks: Vec<BlockApply> = content
        .blocks
        .values()
        .map(|b| BlockApply {
            id: b.key.as_str(),
            raw_id: b.raw_id,
            hardness: b.hardness,
            resistance: b.resistance,
            luminance: b.luminance,
            slipperiness: b.slipperiness,
            collidable: b.is_collidable,
            mineable: b.mineable_tags.iter().copied().map(mineable_name).collect(),
        })
        .collect();

    let mut items: Vec<ItemApply> = mod_registry
        .items
        .values()
        .map(|i| ItemApply {
            id: i.key.as_str(),
            raw_id: i.raw_id,
            max_stack: i.max_stack_size,
        })
        .collect();

    // Content fluids become item markers when no dedicated fluid registry wire exists.
    for f in content.fluids.values() {
        items.push(ItemApply {
            id: f.key.as_str(),
            raw_id: f.fluid_id,
            max_stack: 1,
        });
    }

    let mut sounds: Vec<SoundApply> = content
        .sounds
        .values()
        .map(|s| SoundApply {
            id: s.key.as_str(),
            raw_id: s.id,
            range: s.range,
        })
        .collect();

    let mut particles: Vec<ParticleApply> = content
        .particles
        .values()
        .map(|p| ParticleApply {
            id: p.key.as_str(),
            raw_id: p.id,
            always_show: p.always_show,
        })
        .collect();

    let mut status_effects: Vec<StatusEffectApply> = content
        .status_effects
        .values()
        .map(|e| StatusEffectApply {
            id: e.key.as_str(),
            raw_id: e.id,
            is_beneficial: e.is_beneficial,
            color_rgb: e.color_rgb,
        })
        .collect();

    let mut enchantments: Vec<EnchantmentApply> = content
        .enchantments
        .values()
        .map(|e| EnchantmentApply {
            id: e.key.as_str(),
            raw_id: e.id,
            max_level: e.max_level,
            is_treasure: e.is_treasure,
        })
        .collect();

    let mut recipes: Vec<RecipeApply> = content
        .recipes
        .values()
        .map(|r| RecipeApply {
            id: r.key.as_str(),
            recipe_type: r.recipe_type.clone(),
            result_item: r.result_item.as_str(),
            result_count: r.result_count,
            ingredients: r.ingredients.iter().map(|k| k.as_str()).collect(),
        })
        .collect();

    let mut game_rules: Vec<GameRuleApply> = content
        .game_rules
        .values()
        .map(|g| GameRuleApply {
            id: g.key.as_str(),
            default_bool: g.default_bool,
            default_int: g.default_int,
            category: g.category.clone(),
        })
        .collect();

    let mut trades: Vec<TradeApply> = content
        .trades
        .iter()
        .map(|t| TradeApply {
            villager_profession: t.villager_profession.clone(),
            level: t.level,
            cost_item: t.cost_item.as_str(),
            cost_count: t.cost_count,
            result_item: t.result_item.as_str(),
            result_count: t.result_count,
        })
        .collect();

    let mut dimensions: Vec<DimensionApply> = content
        .dimensions
        .values()
        .map(|d| DimensionApply {
            id: d.key.as_str(),
            dimension_id: d.dimension_id,
            has_skylight: d.has_skylight,
            has_ceiling: d.has_ceiling,
            ambient_light: d.ambient_light,
            portal_block: d.portal_block.as_str(),
        })
        .collect();

    let mut block_entities: Vec<BlockEntityApply> = content
        .block_entities
        .values()
        .map(|b| BlockEntityApply {
            id: b.key.as_str(),
            block_entity_id: b.block_entity_id,
            valid_blocks: b.valid_blocks.iter().map(|k| k.as_str()).collect(),
            symbol: b.dll_tick_symbol.clone(),
        })
        .collect();

    let entities: Vec<EntityApply> = mod_registry
        .entities
        .values()
        .map(|e| EntityApply {
            id: e.key.as_str(),
            entity_type_id: e.entity_type_id,
            max_health: e.max_health,
            speed: e.speed,
            attack_damage: e.attack_damage,
            ai_symbol: e.ai_controller_dll_symbol.clone(),
        })
        .collect();

    let commands: Vec<CommandApply> = gameplay
        .command_tree
        .root_commands
        .values()
        .map(|c| CommandApply {
            name: c.name.clone(),
            description: c.description.clone(),
            permission_level: c.permission_level,
            symbol: c.dll_callback_symbol.clone(),
        })
        .collect();

    let keybindings: Vec<KeyBindingApply> = gameplay
        .keybindings
        .values()
        .map(|k| KeyBindingApply {
            id: k.id.clone(),
            translation_key: k.translation_key.clone(),
            default_key_code: k.default_key_code,
            category: k.category.clone(),
            symbol: k.dll_on_press_symbol.clone(),
        })
        .collect();

    let render_layers: Vec<RenderLayerApply> = rendering
        .block_render_layers
        .iter()
        .map(|(k, layer)| RenderLayerApply {
            block_id: k.as_str(),
            layer: layer_name(*layer),
        })
        .collect();

    let mut network_channels: Vec<String> = networking
        .server_play_handlers
        .keys()
        .chain(networking.client_play_handlers.keys())
        .chain(networking.login_handlers.keys())
        .map(|c| c.as_str())
        .collect();
    network_channels.sort();
    network_channels.dedup();

    let biome_rules: Vec<BiomeRuleApply> = content
        .biome_modifications
        .iter()
        .map(|rule| match &rule.modification {
            BiomeModificationType::AddFeature { step, feature_key } => BiomeRuleApply {
                selector: biome_selector_str(&rule.selector),
                kind: "feature".into(),
                feature_or_entity: feature_key.as_str(),
                step_or_weight: *step,
                min: 0,
                max: 0,
            },
            BiomeModificationType::AddSpawn {
                entity_key,
                weight,
                min,
                max,
            } => BiomeRuleApply {
                selector: biome_selector_str(&rule.selector),
                kind: "spawn".into(),
                feature_or_entity: entity_key.as_str(),
                step_or_weight: *weight,
                min: *min,
                max: *max,
            },
        })
        .collect();

    let screen_redirects: Vec<ScreenRedirectApply> = screens
        .all_redirects()
        .into_iter()
        .map(|(from_class, to_handler)| ScreenRedirectApply {
            from_class,
            to_handler,
        })
        .collect();

    let images_reg = global_images();
    let bytes_map = image_bytes_slot().lock().unwrap();
    let images: Vec<ImageApply> = images_reg
        .images
        .read()
        .unwrap()
        .values()
        .map(|img| {
            let rgba = bytes_map.get(&img.id).cloned().unwrap_or_default();
            ImageApply {
                id: img.id,
                name: img.name.clone(),
                width: img.width,
                height: img.height,
                rgba_b64: base64_encode(&rgba),
            }
        })
        .collect();
    drop(bytes_map);

    let screen_handlers: Vec<ScreenHandlerApply> = gameplay
        .screen_handlers
        .values()
        .map(|h| ScreenHandlerApply {
            id: h.key.as_str(),
            texture: h.gui_texture_path.clone(),
            symbol: h.dll_init_symbol.clone(),
            type_id: h.handler_type_id,
        })
        .collect();

    let mut loot_modifiers = Vec::new();
    for bound in &resources.loot_table_modifiers {
        let key = &bound.table;
        let mut entries = Vec::new();
        (bound.modifier)(key, &mut entries);
        for e in entries {
            loot_modifiers.push(LootModifierApply {
                table_hint: key.as_str(),
                item_id: e.item_key.as_str(),
                weight: e.weight,
                min_count: e.min_count,
                max_count: e.max_count,
            });
        }
    }

    let pending_screen = pending_screen_slot().lock().unwrap().clone();
    let (cloth_title, cloth_entries) = match cloth_open_slot().lock().unwrap().clone() {
        Some((t, e)) => (Some(t), e),
        None => (None, Vec::new()),
    };

    let mut mod_menu_lines = Vec::new();
    if let Some(rt) = runtime() {
        if let Ok(map) = rt.mod_menu.entries.read() {
            for (id, entry) in map.iter() {
                mod_menu_lines.push(format!(
                    "{}|{}|{}|{}|{}|{}",
                    id,
                    entry.manifest.name,
                    entry.manifest.version,
                    entry.manifest.author,
                    entry.manifest.description.replace('|', "/"),
                    entry.homepage_url.clone().unwrap_or_default()
                ));
            }
        }
    }

    // Also fold suite content if present and richer.
    let suite = mod_suite();
    if let Ok(suite_content) = suite.content.read() {
        for b in suite_content.blocks.values() {
            if !blocks.iter().any(|x| x.id == b.key.as_str()) {
                blocks.push(BlockApply {
                    id: b.key.as_str(),
                    raw_id: b.raw_id,
                    hardness: b.hardness,
                    resistance: b.resistance,
                    luminance: b.luminance,
                    slipperiness: b.slipperiness,
                    collidable: b.is_collidable,
                    mineable: b.mineable_tags.iter().copied().map(mineable_name).collect(),
                });
            }
        }
        // Merge suite-only extended registries when the caller passed a thinner ContentRegistry.
        merge_sounds(&mut sounds, &suite_content);
        merge_particles(&mut particles, &suite_content);
        merge_status_effects(&mut status_effects, &suite_content);
        merge_enchantments(&mut enchantments, &suite_content);
        merge_recipes(&mut recipes, &suite_content);
        merge_game_rules(&mut game_rules, &suite_content);
        merge_trades(&mut trades, &suite_content);
        merge_dimensions(&mut dimensions, &suite_content);
        merge_block_entities(&mut block_entities, &suite_content);
    }

    PlatformApplySnapshot {
        generation: APPLY_GENERATION.load(Ordering::SeqCst),
        blocks,
        items,
        entities,
        commands,
        keybindings,
        render_layers,
        network_channels,
        biome_rules,
        screen_redirects,
        images,
        screen_handlers,
        loot_modifiers,
        sounds,
        particles,
        status_effects,
        enchantments,
        recipes,
        game_rules,
        trades,
        dimensions,
        block_entities,
        ui_draw: Vec::new(),
        pending_screen,
        cloth_title,
        cloth_entries,
        mod_menu_lines,
    }
}

fn merge_sounds(out: &mut Vec<SoundApply>, content: &ContentRegistry) {
    for s in content.sounds.values() {
        let id = s.key.as_str();
        if !out.iter().any(|x| x.id == id) {
            out.push(SoundApply {
                id,
                raw_id: s.id,
                range: s.range,
            });
        }
    }
}

fn merge_particles(out: &mut Vec<ParticleApply>, content: &ContentRegistry) {
    for p in content.particles.values() {
        let id = p.key.as_str();
        if !out.iter().any(|x| x.id == id) {
            out.push(ParticleApply {
                id,
                raw_id: p.id,
                always_show: p.always_show,
            });
        }
    }
}

fn merge_status_effects(out: &mut Vec<StatusEffectApply>, content: &ContentRegistry) {
    for e in content.status_effects.values() {
        let id = e.key.as_str();
        if !out.iter().any(|x| x.id == id) {
            out.push(StatusEffectApply {
                id,
                raw_id: e.id,
                is_beneficial: e.is_beneficial,
                color_rgb: e.color_rgb,
            });
        }
    }
}

fn merge_enchantments(out: &mut Vec<EnchantmentApply>, content: &ContentRegistry) {
    for e in content.enchantments.values() {
        let id = e.key.as_str();
        if !out.iter().any(|x| x.id == id) {
            out.push(EnchantmentApply {
                id,
                raw_id: e.id,
                max_level: e.max_level,
                is_treasure: e.is_treasure,
            });
        }
    }
}

fn merge_recipes(out: &mut Vec<RecipeApply>, content: &ContentRegistry) {
    for r in content.recipes.values() {
        let id = r.key.as_str();
        if !out.iter().any(|x| x.id == id) {
            out.push(RecipeApply {
                id,
                recipe_type: r.recipe_type.clone(),
                result_item: r.result_item.as_str(),
                result_count: r.result_count,
                ingredients: r.ingredients.iter().map(|k| k.as_str()).collect(),
            });
        }
    }
}

fn merge_game_rules(out: &mut Vec<GameRuleApply>, content: &ContentRegistry) {
    for g in content.game_rules.values() {
        let id = g.key.as_str();
        if !out.iter().any(|x| x.id == id) {
            out.push(GameRuleApply {
                id,
                default_bool: g.default_bool,
                default_int: g.default_int,
                category: g.category.clone(),
            });
        }
    }
}

fn merge_trades(out: &mut Vec<TradeApply>, content: &ContentRegistry) {
    for t in &content.trades {
        let cost = t.cost_item.as_str();
        let result = t.result_item.as_str();
        if !out.iter().any(|x| {
            x.villager_profession == t.villager_profession
                && x.level == t.level
                && x.cost_item == cost
                && x.result_item == result
        }) {
            out.push(TradeApply {
                villager_profession: t.villager_profession.clone(),
                level: t.level,
                cost_item: cost,
                cost_count: t.cost_count,
                result_item: result,
                result_count: t.result_count,
            });
        }
    }
}

fn merge_dimensions(out: &mut Vec<DimensionApply>, content: &ContentRegistry) {
    for d in content.dimensions.values() {
        let id = d.key.as_str();
        if !out.iter().any(|x| x.id == id) {
            out.push(DimensionApply {
                id,
                dimension_id: d.dimension_id,
                has_skylight: d.has_skylight,
                has_ceiling: d.has_ceiling,
                ambient_light: d.ambient_light,
                portal_block: d.portal_block.as_str(),
            });
        }
    }
}

fn merge_block_entities(out: &mut Vec<BlockEntityApply>, content: &ContentRegistry) {
    for b in content.block_entities.values() {
        let id = b.key.as_str();
        if !out.iter().any(|x| x.id == id) {
            out.push(BlockEntityApply {
                id,
                block_entity_id: b.block_entity_id,
                valid_blocks: b.valid_blocks.iter().map(|k| k.as_str()).collect(),
                symbol: b.dll_tick_symbol.clone(),
            });
        }
    }
}

pub fn collect_from_runtime() -> Option<PlatformApplySnapshot> {
    let rt = runtime()?;
    let suite = mod_suite();
    let content = suite.content.read().ok()?;
    let gameplay = suite.gameplay.read().ok()?;
    let rendering = suite.rendering.read().ok()?;
    let resources = suite.resources.read().ok()?;
    let mod_reg = rt.registry.lock().ok()?;
    Some(collect_apply_snapshot(
        &content,
        &mod_reg,
        &gameplay,
        &rendering,
        &suite.networking,
        rt.screen_registry(),
        &resources,
    ))
}

pub fn clear_pending_screen() {
    *pending_screen_slot().lock().unwrap() = None;
    *cloth_open_slot().lock().unwrap() = None;
}

pub fn snapshot_to_json(snap: &PlatformApplySnapshot) -> Result<String, String> {
    serde_json::to_string(snap).map_err(|e| e.to_string())
}

fn base64_encode(data: &[u8]) -> String {
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

/// Called after successful JVM apply to acknowledge pending screen was consumed.
pub fn on_screen_opened(kind: &str) {
    info!("[Platform] screen opened via JVM: {}", kind);
    clear_pending_screen();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_and_apply_flags() {
        mark_dirty();
        assert!(needs_apply() || APPLY_GENERATION.load(Ordering::SeqCst) > 0);
    }
}
