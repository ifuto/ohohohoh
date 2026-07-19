//! # Fabric API — Complete Rust Implementation (1.21.11)
//!
//! All Fabric API modules from https://github.com/FabricMC/fabric-api (branch 1.21.11)
//! implemented as zero-allocation Rust native interfaces.

pub mod modules;

use crate::hyper_opt::InternedKey;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::info;

/// Fabric API module identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FabricModuleId {
    ApiBase,
    ApiCatalog,
    ApiLookupV1,
    BiomeApiV1,
    BlockApiV1,
    BlockRenderLayerV1,
    ClientGametestApiV1,
    CommandApiV2,
    ContentRegistriesV0,
    CrashReportInfoV1,
    DataAttachmentApiV1,
    DataGenerationApiV1,
    DimensionsV1,
    EntityEventsV1,
    EventsInteractionV0,
    GameRuleApiV1,
    GametestApiV1,
    ItemApiV1,
    ItemGroupApiV1,
    KeyBindingApiV1,
    LifecycleEventsV1,
    LootApiV3,
    MessageApiV1,
    ModelLoadingApiV1,
    NetworkingApiV1,
    ObjectBuilderApiV1,
    ParticlesV1,
    RecipeApiV1,
    RegistrySyncV0,
    RendererApiV1,
    RendererIndigo,
    RenderingFluidsV1,
    RenderingV1,
    ResourceConditionsApiV1,
    ResourceLoaderV1,
    ScreenApiV1,
    ScreenHandlerApiV1,
    SerializationApiV1,
    SoundApiV1,
    TagApiV1,
    TransferApiV1,
    TransitiveAccessWidenersV1,
}

impl FabricModuleId {
    pub const ALL: &'static [FabricModuleId] = &[
        Self::ApiBase, Self::ApiCatalog, Self::ApiLookupV1,
        Self::BiomeApiV1, Self::BlockApiV1, Self::BlockRenderLayerV1,
        Self::ClientGametestApiV1, Self::CommandApiV2, Self::ContentRegistriesV0,
        Self::CrashReportInfoV1, Self::DataAttachmentApiV1, Self::DataGenerationApiV1,
        Self::DimensionsV1, Self::EntityEventsV1, Self::EventsInteractionV0,
        Self::GameRuleApiV1, Self::GametestApiV1, Self::ItemApiV1,
        Self::ItemGroupApiV1, Self::KeyBindingApiV1, Self::LifecycleEventsV1,
        Self::LootApiV3, Self::MessageApiV1, Self::ModelLoadingApiV1,
        Self::NetworkingApiV1, Self::ObjectBuilderApiV1, Self::ParticlesV1,
        Self::RecipeApiV1, Self::RegistrySyncV0, Self::RendererApiV1,
        Self::RendererIndigo, Self::RenderingFluidsV1, Self::RenderingV1,
        Self::ResourceConditionsApiV1, Self::ResourceLoaderV1, Self::ScreenApiV1,
        Self::ScreenHandlerApiV1, Self::SerializationApiV1, Self::SoundApiV1,
        Self::TagApiV1, Self::TransferApiV1, Self::TransitiveAccessWidenersV1,
    ];

    pub fn crate_name(&self) -> &'static str {
        match self {
            Self::ApiBase => "fabric-api-base",
            Self::ApiCatalog => "fabric-api-catalog",
            Self::ApiLookupV1 => "fabric-api-lookup-api-v1",
            Self::BiomeApiV1 => "fabric-biome-api-v1",
            Self::BlockApiV1 => "fabric-block-api-v1",
            Self::BlockRenderLayerV1 => "fabric-blockrenderlayer-v1",
            Self::ClientGametestApiV1 => "fabric-client-gametest-api-v1",
            Self::CommandApiV2 => "fabric-command-api-v2",
            Self::ContentRegistriesV0 => "fabric-content-registries-v0",
            Self::CrashReportInfoV1 => "fabric-crash-report-info-v1",
            Self::DataAttachmentApiV1 => "fabric-data-attachment-api-v1",
            Self::DataGenerationApiV1 => "fabric-data-generation-api-v1",
            Self::DimensionsV1 => "fabric-dimensions-v1",
            Self::EntityEventsV1 => "fabric-entity-events-v1",
            Self::EventsInteractionV0 => "fabric-events-interaction-v0",
            Self::GameRuleApiV1 => "fabric-game-rule-api-v1",
            Self::GametestApiV1 => "fabric-gametest-api-v1",
            Self::ItemApiV1 => "fabric-item-api-v1",
            Self::ItemGroupApiV1 => "fabric-item-group-api-v1",
            Self::KeyBindingApiV1 => "fabric-key-binding-api-v1",
            Self::LifecycleEventsV1 => "fabric-lifecycle-events-v1",
            Self::LootApiV3 => "fabric-loot-api-v3",
            Self::MessageApiV1 => "fabric-message-api-v1",
            Self::ModelLoadingApiV1 => "fabric-model-loading-api-v1",
            Self::NetworkingApiV1 => "fabric-networking-api-v1",
            Self::ObjectBuilderApiV1 => "fabric-object-builder-api-v1",
            Self::ParticlesV1 => "fabric-particles-v1",
            Self::RecipeApiV1 => "fabric-recipe-api-v1",
            Self::RegistrySyncV0 => "fabric-registry-sync-v0",
            Self::RendererApiV1 => "fabric-renderer-api-v1",
            Self::RendererIndigo => "fabric-renderer-indigo",
            Self::RenderingFluidsV1 => "fabric-rendering-fluids-v1",
            Self::RenderingV1 => "fabric-rendering-v1",
            Self::ResourceConditionsApiV1 => "fabric-resource-conditions-api-v1",
            Self::ResourceLoaderV1 => "fabric-resource-loader-v1",
            Self::ScreenApiV1 => "fabric-screen-api-v1",
            Self::ScreenHandlerApiV1 => "fabric-screen-handler-api-v1",
            Self::SerializationApiV1 => "fabric-serialization-api-v1",
            Self::SoundApiV1 => "fabric-sound-api-v1",
            Self::TagApiV1 => "fabric-tag-api-v1",
            Self::TransferApiV1 => "fabric-transfer-api-v1",
            Self::TransitiveAccessWidenersV1 => "fabric-transitive-access-wideners-v1",
        }
    }
}

/// Complete Fabric API suite — single entry point for all modules
#[derive(Clone)]
pub struct FabricApiSuite {
    pub modules: Arc<RwLock<HashMap<FabricModuleId, modules::FabricModuleHandle>>>,
    pub initialized: Arc<RwLock<bool>>,
}

impl Default for FabricApiSuite {
    fn default() -> Self { Self::new() }
}

impl FabricApiSuite {
    pub fn new() -> Self {
        let mut map = HashMap::with_capacity(FabricModuleId::ALL.len());
        for &id in FabricModuleId::ALL {
            map.insert(id, modules::FabricModuleHandle::new(id));
        }
        info!("[FabricAPI] Initialized {} Rust-native modules (1.21.11)", map.len());
        Self {
            modules: Arc::new(RwLock::new(map)),
            initialized: Arc::new(RwLock::new(false)),
        }
    }

    pub fn init_all(&self) -> Result<usize, String> {
        let mut count = 0usize;
        if let Ok(mut modules) = self.modules.write() {
            for (_, handle) in modules.iter_mut() {
                handle.activate()?;
                count += 1;
            }
        }
        if let Ok(mut init) = self.initialized.write() {
            *init = true;
        }
        info!("[FabricAPI] Activated {}/{} modules", count, FabricModuleId::ALL.len());
        Ok(count)
    }

    pub fn module_count(&self) -> usize {
        FabricModuleId::ALL.len()
    }

    pub fn get(&self, id: FabricModuleId) -> Option<modules::FabricModuleHandle> {
        self.modules.read().ok()?.get(&id).cloned()
    }

    pub fn verify_all_active(&self) -> bool {
        self.modules.read()
            .map(|m| m.values().all(|h| h.is_active))
            .unwrap_or(false)
    }
}

/// Global Fabric API instance (lazy)
use std::sync::OnceLock;
static FABRIC_SUITE: OnceLock<FabricApiSuite> = OnceLock::new();

pub fn fabric_api() -> &'static FabricApiSuite {
    FABRIC_SUITE.get_or_init(FabricApiSuite::new)
}

pub fn init_fabric_api() -> Result<usize, String> {
    fabric_api().init_all()
}

/// Register a callback keyed by InternedKey (O(1) lookup, zero string alloc at runtime)
pub type FabricCallback = Arc<dyn Fn() + Send + Sync>;

static FABRIC_CALLBACKS: OnceLock<RwLock<HashMap<InternedKey, FabricCallback>>> = OnceLock::new();

fn fabric_callbacks() -> &'static RwLock<HashMap<InternedKey, FabricCallback>> {
    FABRIC_CALLBACKS.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn register_callback(namespace: &str, name: &str, cb: FabricCallback) -> InternedKey {
    let key = InternedKey::from_str(&format!("{}:{}", namespace, name));
    if let Ok(mut map) = fabric_callbacks().write() {
        map.insert(key, cb);
    }
    key
}

/// Invoke a previously registered Fabric callback by interned key.
pub fn invoke_callback(key: InternedKey) -> bool {
    if let Ok(map) = fabric_callbacks().read() {
        if let Some(cb) = map.get(&key) {
            cb();
            return true;
        }
    }
    false
}
