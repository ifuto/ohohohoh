//! # In-process Transfer Storage (NOT Fabric ItemStack/Fluid parity)
//!
//! `Storage<()>` / `SimpleStorage` are unit-resource counters for Rust-side budgets.
//! They are **not** wired to Minecraft ItemStack or FluidVariant. Do not advertise
//! as Fabric Transfer API compatibility.

use crate::hyper_opt::InternedKey;
use crate::mod_suite::mod_suite;
use crate::platform::mark_dirty;
use crate::registry::RegistryKey;
use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use tracing::{info, debug};

pub trait Storage<T>: Send + Sync {
    fn insert(&self, resource: &T, max_amount: u64, simulate: bool) -> u64;
    fn extract(&self, resource: &T, max_amount: u64, simulate: bool) -> u64;
    fn get_amount(&self) -> u64;
    fn get_capacity(&self) -> u64;
}

/// Simple in-process storage used by Transfer API parity.
#[derive(Default)]
pub struct SimpleStorage {
    amount: RwLock<u64>,
    capacity: u64,
}

impl SimpleStorage {
    pub fn new(capacity: u64) -> Self {
        Self {
            amount: RwLock::new(0),
            capacity,
        }
    }
}

impl Storage<()> for SimpleStorage {
    fn insert(&self, _resource: &(), max_amount: u64, simulate: bool) -> u64 {
        let mut a = self.amount.write().unwrap();
        let space = self.capacity.saturating_sub(*a);
        let n = max_amount.min(space);
        if !simulate {
            *a += n;
        }
        n
    }

    fn extract(&self, _resource: &(), max_amount: u64, simulate: bool) -> u64 {
        let mut a = self.amount.write().unwrap();
        let n = max_amount.min(*a);
        if !simulate {
            *a -= n;
        }
        n
    }

    fn get_amount(&self) -> u64 {
        *self.amount.read().unwrap()
    }

    fn get_capacity(&self) -> u64 {
        self.capacity
    }
}

#[derive(Debug, Clone)]
pub struct CustomModelDefinition {
    pub key: InternedKey,
    pub obj_or_gltf_data: Vec<u8>,
    pub texture_bindless_id: u32,
}

#[derive(Debug, Clone)]
pub struct ParticleFactoryDefinition {
    pub particle_type_key: InternedKey,
    pub dll_factory_symbol: String,
    pub always_render: bool,
}

#[derive(Debug, Clone)]
pub struct OptimizedBiomeModification {
    pub target_tag_hash: u64,
    pub feature_to_inject: InternedKey,
    pub generation_step: u32,
}

#[derive(Default, Clone)]
pub struct FabricApiOptimizedManager {
    pub custom_models: Arc<RwLock<HashMap<InternedKey, CustomModelDefinition>>>,
    pub particle_factories: Arc<RwLock<HashMap<InternedKey, ParticleFactoryDefinition>>>,
    pub biome_modifications: Arc<RwLock<Vec<OptimizedBiomeModification>>>,
    pub transfer_storages: Arc<RwLock<HashMap<String, Arc<SimpleStorage>>>>,
}

impl FabricApiOptimizedManager {
    pub fn new() -> Self {
        info!("[Fabric API Opt] Initializing optimized parity engine");
        Self::default()
    }

    pub fn register_model(&self, namespace: &str, name: &str, raw_data: Vec<u8>, tex_id: u32) -> InternedKey {
        let key_str = format!("{}:{}", namespace, name);
        let interned = InternedKey::from_str(&key_str);
        debug!("[Fabric API Opt] Model [{}] hash #{}", key_str, interned.hash);
        if let Ok(mut map) = self.custom_models.write() {
            map.insert(
                interned,
                CustomModelDefinition {
                    key: interned,
                    obj_or_gltf_data: raw_data,
                    texture_bindless_id: tex_id,
                },
            );
        }
        let ns = namespace.to_string();
        let nm = name.to_string();
        let suite = mod_suite();
        if let Ok(mut render) = suite.rendering.write() {
            render.register_model_appender(Arc::new(move |keys| {
                keys.push(RegistryKey::new(ns.clone(), nm.clone()));
            }));
        }
        mark_dirty();
        interned
    }

    pub fn register_particle(&self, namespace: &str, name: &str, symbol: &str, always: bool) {
        let key_str = format!("{}:{}", namespace, name);
        let interned = InternedKey::from_str(&key_str);
        debug!("[Fabric API Opt] Particle [{}] -> {}", key_str, symbol);
        if let Ok(mut map) = self.particle_factories.write() {
            map.insert(
                interned,
                ParticleFactoryDefinition {
                    particle_type_key: interned,
                    dll_factory_symbol: symbol.to_string(),
                    always_render: always,
                },
            );
        }
        let suite = mod_suite();
        if let Ok(mut render) = suite.rendering.write() {
            render.register_particle_factory(RegistryKey::new(namespace, name), symbol);
        }
        if let Ok(mut content) = suite.content.write() {
            let id = content.next_id_peek();
            content.register_particle(crate::content::ParticleDefinition {
                key: RegistryKey::new(namespace, name),
                id,
                always_show: always,
            });
        }
        mark_dirty();
    }

    pub fn modify_biome(&self, target_tag: &str, feature_ns: &str, feature_name: &str, step: u32) {
        let tag_hash = InternedKey::from_str(target_tag).hash;
        let feat_key = InternedKey::from_str(&format!("{}:{}", feature_ns, feature_name));
        debug!("[Fabric API Opt] Biome rule tag hash #{}", tag_hash);
        if let Ok(mut list) = self.biome_modifications.write() {
            list.push(OptimizedBiomeModification {
                target_tag_hash: tag_hash,
                feature_to_inject: feat_key,
                generation_step: step,
            });
        }
        let suite = mod_suite();
        if let Ok(mut content) = suite.content.write() {
            content.add_biome_modification(crate::content::BiomeModificationRule {
                selector: crate::content::BiomeSelector::Tag(target_tag.to_string()),
                modification: crate::content::BiomeModificationType::AddFeature {
                    step,
                    feature_key: RegistryKey::new(feature_ns, feature_name),
                },
            });
        }
        mark_dirty();
    }

    pub fn register_transfer_storage(&self, id: &str, capacity: u64) -> Arc<SimpleStorage> {
        let storage = Arc::new(SimpleStorage::new(capacity));
        self.transfer_storages
            .write()
            .unwrap()
            .insert(id.to_string(), storage.clone());
        mark_dirty();
        storage
    }
}

static FABRIC_OPT: std::sync::OnceLock<FabricApiOptimizedManager> = std::sync::OnceLock::new();

pub fn fabric_api_optimized() -> &'static FabricApiOptimizedManager {
    FABRIC_OPT.get_or_init(FabricApiOptimizedManager::new)
}
