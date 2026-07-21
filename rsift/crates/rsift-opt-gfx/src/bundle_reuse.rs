
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(lod: u8) -> BundleKey {
        BundleKey { chunk_x: 1, chunk_z: 2, lod }
    }

    #[test]
    fn miss_creates_and_hit_reuses_with_stats() {
        let mut c = BundleCache::new();
        let k = key(0);
        let b = c.get_or_create(k, || {
            vec![
                BundleCommand::SetPipeline { pso_id: 7 },
                BundleCommand::DrawIndexed { index_count: 36, start: 0, base: 0 },
                BundleCommand::DrawIndexed { index_count: 24, start: 36, base: 0 },
            ]
        });
        assert_eq!(b.vertex_count, 60); // DrawIndexed のみ加算
        assert_eq!(b.commands.len(), 3);
        assert_eq!(b.reused, 1); // 生成時の get でも +1
        assert_eq!(c.stats(), (0, 1));
        let b2 = c.get_or_create(k, || panic!("hit では create は呼ばれない"));
        assert_eq!(b2.reused, 2);
        assert_eq!(c.stats(), (1, 1));
        let _ = c.get_or_create(key(1), Vec::new); // 別キーは別 miss
        assert_eq!(c.stats(), (1, 2));
    }

    #[test]
    fn commands_stored_verbatim() {
        let mut c = BundleCache::new();
        let b = c.get_or_create(key(3), || {
            vec![
                BundleCommand::SetRootConstants { data: [1.5; 16] },
                BundleCommand::DrawIndexed { index_count: 6, start: 4, base: -1 },
            ]
        });
        match &b.commands[0] {
            BundleCommand::SetRootConstants { data } => assert_eq!(data[0], 1.5),
            other => panic!("unexpected {other:?}"),
        }
        match &b.commands[1] {
            BundleCommand::DrawIndexed { index_count, start, base } => {
                assert_eq!((*index_count, *start, *base), (6, 4, -1));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(b.vertex_count, 6); // RootConstants は頂点計上なし
    }

    #[test]
    fn distinct_lod_keys_are_independent() {
        let mut c = BundleCache::new();
        let _ = c.get_or_create(key(0), || {
            vec![BundleCommand::DrawIndexed { index_count: 3, start: 0, base: 0 }]
        });
        let _ = c.get_or_create(key(9), || {
            vec![BundleCommand::DrawIndexed { index_count: 30, start: 0, base: 0 }]
        });
        assert_eq!(c.get_or_create(key(0), Vec::new).vertex_count, 3);
        assert_eq!(c.get_or_create(key(9), Vec::new).vertex_count, 30);
        assert_eq!(c.stats(), (2, 2));
    }
}
