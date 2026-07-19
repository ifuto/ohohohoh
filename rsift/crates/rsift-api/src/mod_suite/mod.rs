//! # Rsift Mod Suite — unified mod platform API (1.21.11)
//!
//! Fabric/NeoForge 相当のモジュール群を Rsift ネイティブ実装として提供します。
//! 公開 API 名に外部ローダー名は含めません。各モジュールは JVM ブリッジ経由で Minecraft に届きます。

pub mod modules;

use crate::lifecycle::EventBus;
use crate::networking::NetworkManager;
use crate::rendering::RenderingRegistry;
use crate::content::ContentRegistry;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use tracing::info;

/// Platform module identifiers (mirrors common mod-loader API surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuiteModuleId {
    LoaderCore,
    ApiCatalog,
    ApiLookup,
    Biome,
    Block,
    BlockRenderLayer,
    ClientGametest,
    Command,
    ContentRegistries,
    CrashReportInfo,
    DataAttachment,
    DataGeneration,
    Dimensions,
    EntityEvents,
    EventsInteraction,
    GameRule,
    Gametest,
    Item,
    ItemGroup,
    KeyBinding,
    LifecycleEvents,
    Loot,
    Message,
    ModelLoading,
    Networking,
    ObjectBuilder,
    Particles,
    Recipe,
    RegistrySync,
    RendererApi,
    RendererBackend,
    RenderingFluids,
    ClientRendering,
    ResourceConditions,
    ResourceLoader,
    Screen,
    ScreenHandler,
    Serialization,
    Sound,
    Tag,
    Transfer,
    TransitiveAccess,
}

impl SuiteModuleId {
    pub const ALL: &'static [SuiteModuleId] = &[
        Self::LoaderCore,
        Self::ApiCatalog,
        Self::ApiLookup,
        Self::Biome,
        Self::Block,
        Self::BlockRenderLayer,
        Self::ClientGametest,
        Self::Command,
        Self::ContentRegistries,
        Self::CrashReportInfo,
        Self::DataAttachment,
        Self::DataGeneration,
        Self::Dimensions,
        Self::EntityEvents,
        Self::EventsInteraction,
        Self::GameRule,
        Self::Gametest,
        Self::Item,
        Self::ItemGroup,
        Self::KeyBinding,
        Self::LifecycleEvents,
        Self::Loot,
        Self::Message,
        Self::ModelLoading,
        Self::Networking,
        Self::ObjectBuilder,
        Self::Particles,
        Self::Recipe,
        Self::RegistrySync,
        Self::RendererApi,
        Self::RendererBackend,
        Self::RenderingFluids,
        Self::ClientRendering,
        Self::ResourceConditions,
        Self::ResourceLoader,
        Self::Screen,
        Self::ScreenHandler,
        Self::Serialization,
        Self::Sound,
        Self::Tag,
        Self::Transfer,
        Self::TransitiveAccess,
    ];

    pub fn binding(self) -> &'static str {
        match self {
            Self::LoaderCore => "rsift_api::lifecycle::ModLoaderEnvironment",
            Self::LifecycleEvents => "rsift_api::lifecycle::EventBus",
            Self::Networking => "rsift_api::networking::NetworkManager",
            Self::ClientRendering => "rsift_api::rendering::RenderingRegistry",
            Self::RendererApi => "rsift_api::mod_suite::modules::RendererApi",
            Self::ResourceLoader => "rsift_api::resources::ResourceLoader",
            Self::ContentRegistries => "rsift_api::content::ContentRegistry",
            Self::Screen => "rsift_api::ui_ext::ScreenRegistry",
            Self::Transfer => "rsift_api::gameplay::GameplayRegistry",
            Self::Tag => "rsift_api::registry::RegistryKey",
            Self::Command => "rsift_api::gameplay::GameplayRegistry",
            Self::KeyBinding => "rsift_api::gameplay::GameplayRegistry",
            _ => "rsift_api::mod_suite::SuiteModuleHandle",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::LoaderCore => "loader-core",
            Self::RendererApi => "renderer-api",
            Self::ClientRendering => "client-rendering",
            Self::Networking => "networking",
            Self::LifecycleEvents => "lifecycle-events",
            Self::Transfer => "transfer",
            Self::Screen => "screen",
            Self::ResourceLoader => "resource-loader",
            Self::RegistrySync => "registry-sync",
            Self::DataGeneration => "data-generation",
            _ => "module",
        }
    }
}

/// Complete mod platform suite — mods access via `ModContext::suite()`.
#[derive(Clone)]
pub struct ModSuite {
    pub modules: Arc<RwLock<HashMap<SuiteModuleId, modules::SuiteModuleHandle>>>,
    pub events: Arc<EventBus>,
    pub networking: Arc<NetworkManager>,
    pub rendering: Arc<RwLock<RenderingRegistry>>,
    pub resources: Arc<RwLock<crate::resources::ResourceLoader>>,
    pub gameplay: Arc<RwLock<crate::gameplay::GameplayRegistry>>,
    pub content: Arc<RwLock<ContentRegistry>>,
    pub renderer_api: modules::RendererApi,
    pub networking_api: modules::NetworkingApi,
    initialized: Arc<RwLock<bool>>,
}

impl Default for ModSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl ModSuite {
    pub fn new() -> Self {
        let mut map = HashMap::with_capacity(SuiteModuleId::ALL.len());
        for &id in SuiteModuleId::ALL {
            map.insert(id, modules::SuiteModuleHandle::new(id));
        }
        info!("[ModSuite] {} platform modules registered", map.len());
        Self {
            modules: Arc::new(RwLock::new(map)),
            events: Arc::new(EventBus::new()),
            networking: Arc::new(NetworkManager::new()),
            rendering: Arc::new(RwLock::new(RenderingRegistry::new())),
            resources: Arc::new(RwLock::new(crate::resources::ResourceLoader::new())),
            gameplay: Arc::new(RwLock::new(crate::gameplay::GameplayRegistry::new())),
            content: Arc::new(RwLock::new(ContentRegistry::new())),
            renderer_api: modules::RendererApi::default(),
            networking_api: modules::NetworkingApi::default(),
            initialized: Arc::new(RwLock::new(false)),
        }
    }

    pub fn activate_all(&self) -> Result<usize, String> {
        let mut count = 0usize;
        if let Ok(mut mods) = self.modules.write() {
            for handle in mods.values_mut() {
                handle.activate()?;
                count += 1;
            }
        }
        *self.initialized.write().unwrap() = true;
        info!("[ModSuite] activated {}/{} modules (JVM-wired)", count, SuiteModuleId::ALL.len());
        Ok(count)
    }

    pub fn module(&self, id: SuiteModuleId) -> Option<modules::SuiteModuleHandle> {
        self.modules.read().ok()?.get(&id).cloned()
    }

    pub fn is_active(&self, id: SuiteModuleId) -> bool {
        self.module(id).map(|h| h.is_active).unwrap_or(false)
    }

    /// Client tick — dispatched from JVM `RsiftModBridge` every frame.
    pub fn on_client_tick(&self) {
        self.events.dispatch_client_tick();
    }

    /// After render handlers — HUD / screen draw callbacks.
    pub fn on_client_render(&self, width: u32, height: u32, delta: f32) {
        let reg = self.rendering.read().unwrap();
        for cb in &reg.hud_callbacks {
            cb(width, height, delta);
        }
    }
}

static MOD_SUITE: OnceLock<ModSuite> = OnceLock::new();

pub fn mod_suite() -> &'static ModSuite {
    MOD_SUITE.get_or_init(|| {
        let suite = ModSuite::new();
        let _ = suite.activate_all();
        suite
    })
}

pub fn init_mod_suite() -> Result<usize, String> {
    mod_suite().activate_all()
}
