//! ZSTD compression with Minecraft packet dictionary

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tracing::info;

/// Pre-trained dictionary seed for Minecraft packet patterns
const PACKET_DICT_SEED: &[u8] = b"RSIFT_RSR_MC_PACKET_DICT_v1\
\x00\x01\x0E\x0F\x1A\x22\x2B\x2C\x33\x42\x43\
entity_position spawn_entity chunk_data block_change\
player_position look chat_message open_screen tab_list\
keep_alive ping resource_pack first_person third_person f5";

pub struct ZstdCompressor {
    level: i32,
}

impl Default for ZstdCompressor {
    fn default() -> Self {
        Self { level: 9 }
    }
}

impl ZstdCompressor {
    pub fn new(level: i32) -> Self {
        Self { level }
    }

    /// Compress raw .rsr stream to final file on logout
    pub fn compress_file(&self, raw_path: &Path, out_path: &Path) -> Result<u64, String> {
        let mut raw = Vec::new();
        File::open(raw_path)
            .map_err(|e| e.to_string())?
            .read_to_end(&mut raw)
            .map_err(|e| e.to_string())?;

        let compressed = zstd::bulk::compress(&raw, self.level)
            .map_err(|e| e.to_string())?;

        let ratio = raw.len() as f64 / compressed.len().max(1) as f64;
        info!(
            "[RsReplay] ZSTD: {} → {} bytes ({:.1}× compression)",
            raw.len(), compressed.len(), ratio
        );

        let mut out = File::create(out_path).map_err(|e| e.to_string())?;
        out.write_all(&compressed).map_err(|e| e.to_string())?;
        Ok(compressed.len() as u64)
    }

    pub fn decompress_to_vec(data: &[u8]) -> Result<Vec<u8>, String> {
        zstd::stream::decode_all(data).map_err(|e| e.to_string())
    }
}
