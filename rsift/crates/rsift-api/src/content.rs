//! # Rsift Content & Registry System (Fabric Registry & Object Builder Parity)
//!
//! Fabricで提供されるブロック、アイテム、ブロックエンティティ、液体、
//! バイオーム変更、ディメンション、サウンド、パーティクル、エンチャント、
//! ステータス効果、採掘タグ、村人取引の完全なレジストリとビルダー群を実装します。

use crate::registry::RegistryKey;
use std::collections::HashMap;
use tracing::info;

/// 採掘適正レベルタグ (`FabricMineableTags`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MineableTag {
    Pickaxe,
    Axe,
    Shovel,
    Hoe,
    Sword,
    RequiresStoneTool,
    RequiresIronTool,
    RequiresDiamondTool,
    RequiresNetheriteTool,
}

/// ブロック特性ビルダー (`FabricBlockSettings` / `BlockStateBuilder`)
#[derive(Debug, Clone)]
pub struct BlockDefinition {
    pub key: RegistryKey,
    pub raw_id: u32,
    pub hardness: f32,
    pub resistance: f32,
    pub luminance: u8,
    pub slipperiness: f32,
    pub is_collidable: bool,
    pub mineable_tags: Vec<MineableTag>,
    pub custom_drop_table: Option<String>,
}

impl BlockDefinition {
    pub fn builder(namespace: &str, name: &str) -> BlockBuilder {
        BlockBuilder::new(namespace, name)
    }
}

pub struct BlockBuilder {
    namespace: String,
    name: String,
    hardness: f32,
    resistance: f32,
    luminance: u8,
    slipperiness: f32,
    is_collidable: bool,
    tags: Vec<MineableTag>,
}

impl BlockBuilder {
    pub fn new(namespace: &str, name: &str) -> Self {
        Self {
            namespace: namespace.to_string(),
            name: name.to_string(),
            hardness: 1.5,
            resistance: 6.0,
            luminance: 0,
            slipperiness: 0.6,
            is_collidable: true,
            tags: Vec::new(),
        }
    }

    pub fn strength(mut self, hardness: f32, resistance: f32) -> Self {
        self.hardness = hardness;
        self.resistance = resistance;
        self
    }

    pub fn luminance(mut self, lum: u8) -> Self {
        self.luminance = lum;
        self
    }

    pub fn tag(mut self, tag: MineableTag) -> Self {
        self.tags.push(tag);
        self
    }

    pub fn build(self, id: u32) -> BlockDefinition {
        BlockDefinition {
            key: RegistryKey::new(self.namespace, self.name),
            raw_id: id,
            hardness: self.hardness,
            resistance: self.resistance,
            luminance: self.luminance,
            slipperiness: self.slipperiness,
            is_collidable: self.is_collidable,
            mineable_tags: self.tags,
            custom_drop_table: None,
        }
    }
}

/// ブロックエンティティ定義 (`FabricBlockEntityTypeBuilder`)
#[derive(Debug, Clone)]
pub struct BlockEntityDefinition {
    pub key: RegistryKey,
    pub block_entity_id: u32,
    pub valid_blocks: Vec<RegistryKey>,
    pub dll_tick_symbol: String,
}

/// 液体定義 (`FluidRegistry` / `FabricFluidSettings`)
#[derive(Debug, Clone)]
pub struct FluidDefinition {
    pub key: RegistryKey,
    pub fluid_id: u32,
    pub flow_speed: u8,
    pub level_decrease_per_block: u8,
    pub is_infinite: bool,
}

/// バイオーム修正 API (`BiomeModifications` & `BiomeSelectors`)
#[derive(Debug, Clone)]
pub enum BiomeSelector {
    All,
    Overworld,
    Nether,
    TheEnd,
    Tag(String),
    Specific(RegistryKey),
}

#[derive(Debug, Clone)]
pub enum BiomeModificationType {
    AddFeature { step: u32, feature_key: RegistryKey },
    AddSpawn { entity_key: RegistryKey, weight: u32, min: u32, max: u32 },
}

#[derive(Debug, Clone)]
pub struct BiomeModificationRule {
    pub selector: BiomeSelector,
    pub modification: BiomeModificationType,
}

/// ディメンション定義 (`CustomDimensionRegistry`)
#[derive(Debug, Clone)]
pub struct DimensionDefinition {
    pub key: RegistryKey,
    pub dimension_id: u32,
    pub has_skylight: bool,
    pub has_ceiling: bool,
    pub ambient_light: f32,
    pub portal_block: RegistryKey,
}

/// サウンド・パーティクル・ステータス効果・エンチャント・取引定義
#[derive(Debug, Clone)]
pub struct SoundDefinition { pub key: RegistryKey, pub id: u32, pub range: f32 }
#[derive(Debug, Clone)]
pub struct ParticleDefinition { pub key: RegistryKey, pub id: u32, pub always_show: bool }
#[derive(Debug, Clone)]
pub struct StatusEffectDefinition { pub key: RegistryKey, pub id: u32, pub is_beneficial: bool, pub color_rgb: u32 }
#[derive(Debug, Clone)]
pub struct EnchantmentDefinition { pub key: RegistryKey, pub id: u32, pub max_level: u32, pub is_treasure: bool }
#[derive(Debug, Clone)]
pub struct TradeOffer { pub villager_profession: String, pub level: u32, pub cost_item: RegistryKey, pub cost_count: u32, pub result_item: RegistryKey, pub result_count: u32 }

#[derive(Debug, Clone)]
pub struct RecipeDefinition {
    pub key: RegistryKey,
    pub recipe_type: String,
    pub result_item: RegistryKey,
    pub result_count: u32,
    pub ingredients: Vec<RegistryKey>,
}

#[derive(Debug, Clone)]
pub struct GameRuleDefinition {
    pub key: RegistryKey,
    pub default_bool: Option<bool>,
    pub default_int: Option<i32>,
    pub category: String,
}

/// 統合コンテンツレジストリ（全Fabricコンテンツ拡張を保持）
#[derive(Default, Debug)]
pub struct ContentRegistry {
    pub blocks: HashMap<RegistryKey, BlockDefinition>,
    pub block_entities: HashMap<RegistryKey, BlockEntityDefinition>,
    pub fluids: HashMap<RegistryKey, FluidDefinition>,
    pub biome_modifications: Vec<BiomeModificationRule>,
    pub dimensions: HashMap<RegistryKey, DimensionDefinition>,
    pub sounds: HashMap<RegistryKey, SoundDefinition>,
    pub particles: HashMap<RegistryKey, ParticleDefinition>,
    pub status_effects: HashMap<RegistryKey, StatusEffectDefinition>,
    pub enchantments: HashMap<RegistryKey, EnchantmentDefinition>,
    pub trades: Vec<TradeOffer>,
    pub recipes: HashMap<RegistryKey, RecipeDefinition>,
    pub game_rules: HashMap<RegistryKey, GameRuleDefinition>,
    pub next_id: u32,
}

impl ContentRegistry {
    pub fn new() -> Self {
        Self {
            next_id: 10000,
            ..Default::default()
        }
    }

    pub fn next_id_peek(&self) -> u32 {
        self.next_id
    }

    pub fn register_block(&mut self, builder: BlockBuilder) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let def = builder.build(id);
        info!("Registering Fabric-parity Block: {} (ID: {})", def.key.as_str(), id);
        self.blocks.insert(def.key.clone(), def);
        crate::platform::mark_dirty();
        id
    }

    pub fn register_block_entity(&mut self, namespace: &str, name: &str, valid_blocks: Vec<RegistryKey>, symbol: &str) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let key = RegistryKey::new(namespace, name);
        let def = BlockEntityDefinition { key: key.clone(), block_entity_id: id, valid_blocks, dll_tick_symbol: symbol.to_string() };
        info!("Registering BlockEntity: {} (ID: {})", key.as_str(), id);
        self.block_entities.insert(key, def);
        crate::platform::mark_dirty();
        id
    }

    pub fn add_biome_modification(&mut self, rule: BiomeModificationRule) {
        info!("Adding BiomeModification rule: selector={:?}", rule.selector);
        self.biome_modifications.push(rule);
        crate::platform::mark_dirty();
    }

    pub fn register_fluid(&mut self, def: FluidDefinition) -> u32 {
        let id = def.fluid_id;
        info!("Registering Fluid: {}", def.key.as_str());
        self.fluids.insert(def.key.clone(), def);
        crate::platform::mark_dirty();
        id
    }

    pub fn register_dimension(&mut self, def: DimensionDefinition) -> u32 {
        let id = def.dimension_id;
        info!("Registering Dimension: {}", def.key.as_str());
        self.dimensions.insert(def.key.clone(), def);
        crate::platform::mark_dirty();
        id
    }

    pub fn register_sound(&mut self, def: SoundDefinition) -> u32 {
        let id = def.id;
        self.sounds.insert(def.key.clone(), def);
        crate::platform::mark_dirty();
        id
    }

    pub fn register_particle(&mut self, mut def: ParticleDefinition) -> u32 {
        if def.id < 10000 {
            def.id = self.next_id;
            self.next_id += 1;
        }
        let id = def.id;
        self.particles.insert(def.key.clone(), def);
        crate::platform::mark_dirty();
        id
    }

    pub fn register_status_effect(&mut self, def: StatusEffectDefinition) -> u32 {
        let id = def.id;
        self.status_effects.insert(def.key.clone(), def);
        crate::platform::mark_dirty();
        id
    }

    pub fn register_enchantment(&mut self, def: EnchantmentDefinition) -> u32 {
        let id = def.id;
        self.enchantments.insert(def.key.clone(), def);
        crate::platform::mark_dirty();
        id
    }

    pub fn register_trade(&mut self, trade: TradeOffer) {
        self.trades.push(trade);
        crate::platform::mark_dirty();
    }

    pub fn register_recipe(&mut self, recipe: RecipeDefinition) {
        info!("Registering Recipe: {}", recipe.key.as_str());
        self.recipes.insert(recipe.key.clone(), recipe);
        crate::platform::mark_dirty();
    }

    pub fn register_game_rule(&mut self, rule: GameRuleDefinition) {
        info!("Registering GameRule: {}", rule.key.as_str());
        self.game_rules.insert(rule.key.clone(), rule);
        crate::platform::mark_dirty();
    }
}
