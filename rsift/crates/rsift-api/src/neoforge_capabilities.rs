//! # NeoForge Data Attachments & Capability Rework
//!
//! NeoForge 20.3 / 1.21.x で完全刷新されたアタッチメントシステム
//! (`AttachmentType<T>`, `ATTACHMENT_TYPES`) および新ケーパビリティシステムを実装します。

use crate::registry::RegistryKey;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentCodec {
    Int,
    Float,
    String,
    CompoundNbt,
    None,
}

pub struct AttachmentTypeBuilder<T> {
    default_supplier: Arc<dyn Fn() -> T + Send + Sync>,
    codec: AttachmentCodec,
    copy_on_death: bool,
}

impl<T: Clone + Send + Sync + 'static> AttachmentTypeBuilder<T> {
    pub fn new(supplier: impl Fn() -> T + Send + Sync + 'static) -> Self {
        Self {
            default_supplier: Arc::new(supplier),
            codec: AttachmentCodec::None,
            copy_on_death: false,
        }
    }

    pub fn serialize(mut self, codec: AttachmentCodec) -> Self {
        self.codec = codec;
        self
    }

    pub fn copy_on_death(mut self) -> Self {
        self.copy_on_death = true;
        self
    }

    pub fn build(self) -> AttachmentType<T> {
        AttachmentType {
            default_supplier: self.default_supplier,
            codec: self.codec,
            copy_on_death: self.copy_on_death,
        }
    }
}

#[derive(Clone)]
pub struct AttachmentType<T> {
    pub default_supplier: Arc<dyn Fn() -> T + Send + Sync>,
    pub codec: AttachmentCodec,
    pub copy_on_death: bool,
}

impl<T: Clone + Send + Sync + 'static> AttachmentType<T> {
    pub fn builder(supplier: impl Fn() -> T + Send + Sync + 'static) -> AttachmentTypeBuilder<T> {
        AttachmentTypeBuilder::new(supplier)
    }

    pub fn get_default(&self) -> T {
        (self.default_supplier)()
    }
}

pub trait AttachmentHolder: Send + Sync {
    fn get_attachment_raw(&self, key: &str) -> Option<Vec<u8>>;
    fn set_attachment_raw(&mut self, key: &str, data: Vec<u8>);
}

pub trait IEnergyStorage: Send + Sync {
    fn receive_energy(&mut self, max_receive: u32, simulate: bool) -> u32;
    fn extract_energy(&mut self, max_extract: u32, simulate: bool) -> u32;
    fn get_energy_stored(&self) -> u32;
    fn get_max_energy_stored(&self) -> u32;
    fn can_extract(&self) -> bool;
    fn can_receive(&self) -> bool;
}

pub trait IFluidHandler: Send + Sync {
    fn get_tanks(&self) -> usize;
    fn get_fluid_in_tank(&self, tank: usize) -> Option<(RegistryKey, u32)>;
    fn fill(&mut self, resource: RegistryKey, amount: u32, simulate: bool) -> u32;
    fn drain(&mut self, max_drain: u32, simulate: bool) -> Option<(RegistryKey, u32)>;
}

pub trait IItemHandler: Send + Sync {
    fn get_slots(&self) -> usize;
    fn get_stack_in_slot(&self, slot: usize) -> Option<(RegistryKey, u32)>;
    fn insert_item(&mut self, slot: usize, item: RegistryKey, count: u32, simulate: bool) -> u32;
    fn extract_item(
        &mut self,
        slot: usize,
        amount: u32,
        simulate: bool,
    ) -> Option<(RegistryKey, u32)>;
}

pub struct RegisterCapabilitiesEvent {
    pub block_capabilities: Arc<RwLock<HashMap<RegistryKey, String>>>,
    pub item_capabilities: Arc<RwLock<HashMap<RegistryKey, String>>>,
    pub entity_capabilities: Arc<RwLock<HashMap<RegistryKey, String>>>,
}

impl Default for RegisterCapabilitiesEvent {
    fn default() -> Self {
        Self::new()
    }
}

impl RegisterCapabilitiesEvent {
    pub fn new() -> Self {
        Self {
            block_capabilities: Arc::new(RwLock::new(HashMap::new())),
            item_capabilities: Arc::new(RwLock::new(HashMap::new())),
            entity_capabilities: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register_block_entity(
        &self,
        cap_type: &str,
        block_entity_key: RegistryKey,
        provider_symbol: &str,
    ) {
        info!(
            "Registering NeoForge Capability [{}] for BlockEntity {}",
            cap_type,
            block_entity_key.as_str()
        );
        if let Ok(mut map) = self.block_capabilities.write() {
            map.insert(
                block_entity_key,
                format!("{}:{}", cap_type, provider_symbol),
            );
        }
        crate::platform::mark_dirty();
    }

    pub fn register_item(&self, cap_type: &str, item_key: RegistryKey, provider_symbol: &str) {
        info!(
            "Registering NeoForge Capability [{}] for Item {}",
            cap_type,
            item_key.as_str()
        );
        if let Ok(mut map) = self.item_capabilities.write() {
            map.insert(item_key, format!("{}:{}", cap_type, provider_symbol));
        }
        crate::platform::mark_dirty();
    }
}
