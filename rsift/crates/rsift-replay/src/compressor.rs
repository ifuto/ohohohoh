//! ZSTD compression of the raw .rsr packet stream.
//!
//! 注: 旧来のモジュール説明は "with Minecraft packet dictionary" を謳い、
//! 訓練済み辞書シード定数 (PACKET_DICT_SEED) も定義されていたが、実圧縮は
//! `zstd::bulk::compress` 素呼び出しで辞書は一度も使われていなかった
//! (ドキュメントと実装の乖離)。2026-07-21 監査で説明とデッド定数を除去。
//! 辞書圧縮の導入は実測効果の検証付きで別途行う場合のみ再有効化すること。

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tracing::info;

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
