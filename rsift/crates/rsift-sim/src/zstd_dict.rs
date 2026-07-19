//! Phase 3 — Zstd dictionary compression for cold region archives.

use std::io::{Read, Write};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DictError {
    #[error("zstd: {0}")]
    Zstd(String),
    #[error("empty samples")]
    Empty,
}

/// Trained dictionary held for encode/decode of similar chunk payloads.
#[derive(Clone)]
pub struct ZstdDictionary {
    bytes: Arc<Vec<u8>>,
    level: i32,
}

impl ZstdDictionary {
    /// Train from sample chunk payloads (Anvil/NBT-like).
    pub fn train(samples: &[Vec<u8>], max_dict_size: usize) -> Result<Self, DictError> {
        if samples.is_empty() {
            return Err(DictError::Empty);
        }
        let dict = zstd::dict::from_samples(samples, max_dict_size)
            .map_err(|e| DictError::Zstd(e.to_string()))?;
        Ok(Self {
            bytes: Arc::new(dict),
            level: 3,
        })
    }

    pub fn from_bytes(bytes: Vec<u8>, level: i32) -> Self {
        Self {
            bytes: Arc::new(bytes),
            level,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn compress(&self, raw: &[u8]) -> Result<Vec<u8>, DictError> {
        let mut out = Vec::new();
        let mut enc = zstd::stream::write::Encoder::with_dictionary(&mut out, self.level, &self.bytes)
            .map_err(|e| DictError::Zstd(e.to_string()))?;
        enc.write_all(raw)
            .map_err(|e| DictError::Zstd(e.to_string()))?;
        enc.finish()
            .map_err(|e| DictError::Zstd(e.to_string()))?;
        Ok(out)
    }

    pub fn decompress(&self, compressed: &[u8]) -> Result<Vec<u8>, DictError> {
        let mut dec = zstd::stream::read::Decoder::with_dictionary(compressed, &self.bytes)
            .map_err(|e| DictError::Zstd(e.to_string()))?;
        let mut out = Vec::new();
        dec.read_to_end(&mut out)
            .map_err(|e| DictError::Zstd(e.to_string()))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_roundtrip() {
        let samples: Vec<Vec<u8>> = (0..32)
            .map(|i| format!("chunk-payload-{:04}-aaaaaaaa", i).into_bytes())
            .collect();
        let dict = ZstdDictionary::train(&samples, 1024).expect("train");
        let raw = b"chunk-payload-0099-aaaaaaaa";
        let c = dict.compress(raw).unwrap();
        let d = dict.decompress(&c).unwrap();
        assert_eq!(d, raw);
    }
}
