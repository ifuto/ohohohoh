//! Fabric API per-module Rust handles

use super::FabricModuleId;
use crate::{
    content::ContentRegistry,
    gameplay::GameplayRegistry,
    lifecycle::EventBus,
    networking::NetworkManager,
    rendering::RenderingRegistry,
    resources::ResourceLoader,
};
use std::sync::Arc;
use tracing::debug;

#[derive(Clone)]
pub struct FabricModuleHandle {
    pub id: FabricModuleId,
    pub crate_name: String,
    pub is_active: bool,
    pub rsift_binding: String,
}

impl FabricModuleHandle {
    pub fn new(id: FabricModuleId) -> Self {
        Self {
            id,
            crate_name: id.crate_name().to_string(),
            is_active: false,
            rsift_binding: Self::binding_for(id),
        }
    }

    pub fn activate(&mut self) -> Result<(), String> {
        // Catalog registration only — does not wire Fabric JVM subsystems.
        // Mark active for discovery, but binding string documents the Rust owner.
        self.is_active = true;
        self.rsift_binding = format!(
            "{} [catalog-only: EventBus/Registry live under rsift-api; not Fabric JAR]",
            Self::binding_for(self.id)
        );
        debug!(
            "[FabricAPI] Catalog-activated {} → {}",
            self.crate_name, self.rsift_binding
        );
        Ok(())
    }

    fn binding_for(id: FabricModuleId) -> String {
        match id {
            FabricModuleId::LifecycleEventsV1 => "rsift_api::lifecycle::EventBus".into(),
            FabricModuleId::NetworkingApiV1 => "rsift_api::networking::NetworkManager".into(),
            FabricModuleId::RenderingV1 | FabricModuleId::RendererApiV1 | FabricModuleId::RendererIndigo
            | FabricModuleId::BlockRenderLayerV1 | FabricModuleId::RenderingFluidsV1
            | FabricModuleId::ModelLoadingApiV1 | FabricModuleId::ParticlesV1 => {
                "rsift_api::rendering::RenderingRegistry + RsGraphics".into()
            }
            FabricModuleId::CommandApiV2 | FabricModuleId::KeyBindingApiV1
            | FabricModuleId::ScreenHandlerApiV1 | FabricModuleId::ItemGroupApiV1 => {
                "rsift_api::gameplay::GameplayRegistry".into()
            }
            FabricModuleId::ContentRegistriesV0 | FabricModuleId::ObjectBuilderApiV1
            | FabricModuleId::ItemApiV1 | FabricModuleId::BlockApiV1 | FabricModuleId::RegistrySyncV0 => {
                "rsift_api::content::ContentRegistry".into()
            }
            FabricModuleId::BiomeApiV1 | FabricModuleId::DimensionsV1 => {
                "rsift_api::content::BiomeModificationRule".into()
            }
            FabricModuleId::ResourceLoaderV1 | FabricModuleId::ResourceConditionsApiV1
            | FabricModuleId::LootApiV3 => "rsift_api::resources::ResourceLoader".into(),
            FabricModuleId::EntityEventsV1 | FabricModuleId::EventsInteractionV0 => {
                "rsift_api::lifecycle::PlayerAndEntityEvents".into()
            }
            FabricModuleId::TransferApiV1 => "rsift_api::fabric_api_optimized::Storage".into(),
            FabricModuleId::TagApiV1 => "rsift_api::registry::RegistryKey + InternedKey".into(),
            FabricModuleId::MessageApiV1 => "rsift_api::networking::ChannelId".into(),
            FabricModuleId::ScreenApiV1 => "rsift_api::ui_ext::ScreenRegistry".into(),
            FabricModuleId::SoundApiV1 => "rsift_api::content::SoundDefinition".into(),
            FabricModuleId::RecipeApiV1 => "rsift_api::content::RecipeDefinition".into(),
            FabricModuleId::GameRuleApiV1 => "rsift_api::content::GameRuleDefinition".into(),
            FabricModuleId::DataAttachmentApiV1 => "rsift_api::neoforge_capabilities::AttachmentType".into(),
            FabricModuleId::SerializationApiV1 => "rsift_api::packet::DirectBufferSlice (Pod)".into(),
            FabricModuleId::GametestApiV1 | FabricModuleId::ClientGametestApiV1 => {
                "rsift_api::lifecycle::GametestHooks".into()
            }
            FabricModuleId::DataGenerationApiV1 => "rsift_api::resources::DatagenExporter".into(),
            FabricModuleId::CrashReportInfoV1 => "rsift_api::os_integ + tracing".into(),
            FabricModuleId::TransitiveAccessWidenersV1 => "rsift_parser::mixin_eq::MixinInjector".into(),
            FabricModuleId::ApiBase | FabricModuleId::ApiCatalog | FabricModuleId::ApiLookupV1 => {
                "rsift_api::registry::ModRegistry".into()
            }
        }
    }
}

/// Runtime subsystem bundle wired to Fabric API modules
pub struct FabricRuntime {
    pub events: EventBus,
    pub content: ContentRegistry,
    pub network: NetworkManager,
    pub rendering: RenderingRegistry,
    pub resources: ResourceLoader,
    pub gameplay: GameplayRegistry,
}

impl Default for FabricRuntime {
    fn default() -> Self { Self::new() }
}

impl FabricRuntime {
    pub fn new() -> Self {
        Self {
            events: EventBus::new(),
            content: ContentRegistry::new(),
            network: NetworkManager::new(),
            rendering: RenderingRegistry::new(),
            resources: ResourceLoader::new(),
            gameplay: GameplayRegistry::new(),
        }
    }
}

pub type FabricEventListener = Arc<dyn Fn(f32) + Send + Sync>;
