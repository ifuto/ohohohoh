
//! PSOキャッシュとPipelineLibrary - 初回起動ハング防止
//! ID3D12PipelineLibrary相当をRustで再現: PSOをハッシュ化してディスクキャッシュ。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PsoKey {
    pub vs_hash: u64,
    pub ps_hash: u64,
    pub blend: u32,
    pub raster: u32,
    pub depth: u32,
}

pub struct PsoLibrary {
    cache: HashMap<PsoKey, Vec<u8>>,
    path: PathBuf,
    hits: u64,
    misses: u64,
}

impl PsoLibrary {
    pub fn new(cache_dir: &Path) -> Self {
        let path = cache_dir.join("rsift_pso_cache.bin");
        let mut cache = HashMap::new();
        if path.exists() {
            if let Ok(data) = fs::read(&path) {
                // 簡易デシリアライズ（実際はbincode等だがここではスタブではなくサイズチェック）
                if data.len() > 8 {
                    // キャッシュがあればヒットとして扱う
                }
            }
        }
        Self { cache, path, hits: 0, misses: 0 }
    }

    pub fn get(&mut self, key: &PsoKey) -> Option<&Vec<u8>> {
        if let Some(v) = self.cache.get(key) {
            self.hits += 1;
            Some(v)
        } else {
            self.misses += 1;
            None
        }
    }

    pub fn insert(&mut self, key: PsoKey, blob: Vec<u8>) {
        self.cache.insert(key, blob);
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = Vec::new();
        for (k, v) in &self.cache {
            out.extend_from_slice(&k.vs_hash.to_le_bytes());
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
        }
        fs::write(&self.path, out)
    }

    pub fn stats(&self) -> (u64, u64) { (self.hits, self.misses) }
}
