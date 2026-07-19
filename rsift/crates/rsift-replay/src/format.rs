//! .rsr file format — raw binary stream + metadata

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

pub const RSR_MAGIC: [u8; 4] = *b"RSR1";
pub const RSR_VERSION: u32 = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct RsrFileHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub player_uuid: [u8; 16],
    pub duration_us: u64,
    pub packet_count: u64,
    pub view_mode_changes: u32,
    pub compressed: u8,
    pub _pad: [u8; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsrMetadata {
    pub filename: String,
    pub player_name: String,
    pub player_uuid: String,
    pub recorded_at: String,
    pub duration_display: String,
    pub duration_us: u64,
    pub packet_count: u64,
    pub file_size_bytes: u64,
    pub server_address: String,
    pub minecraft_version: String,
}

impl RsrFileHeader {
    pub fn new(player_uuid: [u8; 16]) -> Self {
        Self {
            magic: RSR_MAGIC,
            version: RSR_VERSION,
            player_uuid,
            duration_us: 0,
            packet_count: 0,
            view_mode_changes: 0,
            compressed: 0,
            _pad: [0; 3],
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == RSR_MAGIC && self.version == RSR_VERSION
    }
}

pub fn format_duration(us: u64) -> String {
    let total_secs = us / 1_000_000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{}:{:02}", mins, secs)
}
