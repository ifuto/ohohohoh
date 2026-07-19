//! JVM World Mirror — zero-copy Pod layout synced from JNI/DirectBuffer.
//! Native compute reads/writes mirror; JVM reads results back unchanged.

use bytemuck::{Pod, Zeroable};
use std::sync::RwLock;
use tracing::debug;

/// Header for bulk world sync from JVM (prepended to entity/block arrays)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Zeroable)]
pub struct JvmWorldSyncHeader {
    pub magic: u32,       // 0x52534946 = "RSIF"
    pub version: u32,     // 1
    pub entity_count: u32,
    pub chunk_count: u32,
    pub redstone_count: u32,
    pub block_entity_count: u32,
    pub game_time: u64,
    pub tick_number: u64,
    pub delta_ms: f32,
    pub _pad: u32,
}

pub const WORLD_SYNC_MAGIC: u32 = 0x5253_4946;
pub const WORLD_SYNC_VERSION: u32 = 1;

unsafe impl Pod for JvmWorldSyncHeader {}

/// Entity state mirror — layout matches JVM write-back expectations
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Zeroable)]
pub struct JvmEntityState {
    pub entity_id: i32,
    pub entity_type: u32,
    pub pos_x: f64,
    pub pos_y: f64,
    pub pos_z: f64,
    pub vel_x: f64,
    pub vel_y: f64,
    pub vel_z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub on_ground: u8,
    pub removed: u8,
    pub flags: u32, // portal, passenger, no_gravity
    pub ai_tick_counter: u32,
}

unsafe impl Pod for JvmEntityState {}

/// Redstone wire mirror — vanilla strength 0..=15
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Zeroable)]
pub struct JvmRedstoneState {
    pub block_x: i32,
    pub block_y: i32,
    pub block_z: i32,
    pub strength: u8,
    pub connected_north: u8,
    pub connected_south: u8,
    pub connected_east: u8,
    pub connected_west: u8,
    pub removed: u8,
    pub _pad: [u8; 3],
}

unsafe impl Pod for JvmRedstoneState {}

/// Chunk tick mirror
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Zeroable)]
pub struct JvmChunkState {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub loaded: u8,
    pub in_spawn: u8,
    pub random_ticks_remaining: u32,
    pub block_tick_queue: u32,
}

unsafe impl Pod for JvmChunkState {}

/// In-memory mirror of JVM world state for native compute
pub struct WorldMirror {
    pub header: JvmWorldSyncHeader,
    pub entities: Vec<JvmEntityState>,
    pub redstone: Vec<JvmRedstoneState>,
    pub chunks: Vec<JvmChunkState>,
    pub synced_from_jvm: bool,
}

impl Default for WorldMirror {
    fn default() -> Self { Self::new() }
}

impl WorldMirror {
    pub fn new() -> Self {
        Self {
            header: JvmWorldSyncHeader::default(),
            entities: Vec::new(),
            redstone: Vec::new(),
            chunks: Vec::new(),
            synced_from_jvm: false,
        }
    }

    /// Sync from JNI direct buffer: [header][entities...][redstone...][chunks...]
    pub unsafe fn sync_from_jni(&mut self, ptr: i64, len: i32) -> Result<(), &'static str> {
        if ptr == 0 || len < std::mem::size_of::<JvmWorldSyncHeader>() as i32 {
            return Err("Invalid world sync buffer");
        }
        let slice = std::slice::from_raw_parts(ptr as *const u8, len as usize);
        let header: &JvmWorldSyncHeader = bytemuck::try_from_bytes(
            &slice[..std::mem::size_of::<JvmWorldSyncHeader>()],
        ).map_err(|_| "Header alignment error")?;

        if header.magic != WORLD_SYNC_MAGIC {
            return Err("Invalid world sync magic");
        }

        let mut offset = std::mem::size_of::<JvmWorldSyncHeader>();
        let entity_size = std::mem::size_of::<JvmEntityState>();

        self.entities.clear();
        for _ in 0..header.entity_count {
            if offset + entity_size > slice.len() { break; }
            if let Ok(e) = bytemuck::try_from_bytes::<JvmEntityState>(&slice[offset..offset + entity_size]) {
                self.entities.push(*e);
            }
            offset += entity_size;
        }

        let rs_size = std::mem::size_of::<JvmRedstoneState>();
        self.redstone.clear();
        for _ in 0..header.redstone_count {
            if offset + rs_size > slice.len() { break; }
            if let Ok(r) = bytemuck::try_from_bytes::<JvmRedstoneState>(&slice[offset..offset + rs_size]) {
                self.redstone.push(*r);
            }
            offset += rs_size;
        }

        let ch_size = std::mem::size_of::<JvmChunkState>();
        self.chunks.clear();
        for _ in 0..header.chunk_count {
            if offset + ch_size > slice.len() { break; }
            if let Ok(c) = bytemuck::try_from_bytes::<JvmChunkState>(&slice[offset..offset + ch_size]) {
                self.chunks.push(*c);
            }
            offset += ch_size;
        }

        self.header = *header;
        self.synced_from_jvm = true;
        debug!(
            "[WorldMirror] synced: entities={} redstone={} chunks={} tick={}",
            self.entities.len(), self.redstone.len(), self.chunks.len(), header.tick_number
        );
        Ok(())
    }

    /// Write results back to JVM buffer (same layout, in-place update)
    pub unsafe fn write_back_to_jni(&self, ptr: i64, len: i32) -> Result<(), &'static str> {
        if ptr == 0 || !self.synced_from_jvm {
            return Err("Cannot write back: no sync");
        }
        let slice = std::slice::from_raw_parts_mut(ptr as *mut u8, len as usize);
        let header_size = std::mem::size_of::<JvmWorldSyncHeader>();
        if slice.len() < header_size {
            return Err("Buffer too small");
        }
        slice[..header_size].copy_from_slice(bytemuck::bytes_of(&self.header));

        let mut offset = header_size;
        let entity_size = std::mem::size_of::<JvmEntityState>();
        for e in &self.entities {
            if offset + entity_size > slice.len() { break; }
            slice[offset..offset + entity_size].copy_from_slice(bytemuck::bytes_of(e));
            offset += entity_size;
        }

        let rs_size = std::mem::size_of::<JvmRedstoneState>();
        for r in &self.redstone {
            if offset + rs_size > slice.len() { break; }
            slice[offset..offset + rs_size].copy_from_slice(bytemuck::bytes_of(r));
            offset += rs_size;
        }
        Ok(())
    }

    pub fn entity_count(&self) -> usize { self.entities.len() }
    pub fn has_jvm_data(&self) -> bool {
        self.synced_from_jvm
            && (!self.entities.is_empty() || !self.chunks.is_empty() || !self.redstone.is_empty())
    }
}
