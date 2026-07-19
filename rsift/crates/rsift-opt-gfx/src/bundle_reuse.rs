
//! Bundle再利用 - 静的チャンク描画コマンドの録画・再利用
//! ID3D12GraphicsCommandListをBundleとして再利用、遠景再描画時はExecuteBundleのみ。

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BundleKey {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub lod: u8,
}

pub struct Bundle {
    pub key: BundleKey,
    pub commands: Vec<BundleCommand>,
    pub vertex_count: u32,
    pub reused: u64,
}

#[derive(Debug, Clone)]
pub enum BundleCommand {
    SetPipeline { pso_id: u32 },
    SetRootConstants { data: [f32; 16] },
    DrawIndexed { index_count: u32, start: u32, base: i32 },
}

pub struct BundleCache {
    bundles: HashMap<BundleKey, Bundle>,
    hits: u64,
    misses: u64,
}

impl BundleCache {
    pub fn new() -> Self {
        Self { bundles: HashMap::new(), hits: 0, misses: 0 }
    }

    pub fn get_or_create<F>(&mut self, key: BundleKey, create: F) -> &Bundle
    where F: FnOnce() -> Vec<BundleCommand>
    {
        if self.bundles.contains_key(&key) {
            self.hits += 1;
        } else {
            self.misses += 1;
            let cmds = create();
            let vc = cmds.iter().map(|c| match c {
                BundleCommand::DrawIndexed { index_count, .. } => *index_count,
                _ => 0,
            }).sum();
            self.bundles.insert(key, Bundle { key, commands: cmds, vertex_count: vc, reused: 0 });
        }
        self.bundles.get_mut(&key).map(|b| { b.reused += 1; }).unwrap();
        self.bundles.get(&key).unwrap()
    }

    pub fn stats(&self) -> (u64, u64) { (self.hits, self.misses) }
}
