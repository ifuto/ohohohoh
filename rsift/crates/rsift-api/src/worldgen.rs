//! # Rsift World Generation (`worldgen`)
//!
//! カスタム `ChunkGenerator`（独自世界生成）の**第一級サポート**です。
//! Fabric や NeoForge のローダーは不要で、Rsift 単体のネイティブ Rust で動作します。
//!
//! - 決定論的シード付き 2D 値ノイズ（`ValueNoise2D` / fBm）による本物の地形生成。
//! - Mod は `ChunkGeneratorBuilder` で表面ブロック・海面・高度・鉱脈などを自由にカスタム可能。
//! - `ChunkGeneratorRegistry` で名前付き生成器を登録し、いずれかを「有効」にして
//!   実際のチャンク生成を差し替えられます（独自ディメンション／バイオーム対応の基盤）。

use crate::registry::RegistryKey;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

pub const CHUNK_SIZE_X: usize = 16;
pub const CHUNK_SIZE_Z: usize = 16;

/// Minecraft 互換のブロック状態（上位 12bit = block id、下位 4bit = meta）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockState(pub u16);

impl BlockState {
    pub const AIR: BlockState = BlockState(0);
    pub const STONE: BlockState = BlockState(16); // block id 1
    pub const GRASS: BlockState = BlockState(32); // block id 2
    pub const DIRT: BlockState = BlockState(48); // block id 3
    pub const WATER: BlockState = BlockState(144); // block id 9
    pub const IRON_ORE: BlockState = BlockState(240); // block id 15

    pub fn new(block_id: u16, meta: u8) -> Self {
        BlockState(((block_id & 0xFFF) << 4) | (meta as u16 & 0xF))
    }
    pub fn block_id(&self) -> u16 {
        self.0 >> 4
    }
    pub fn meta(&self) -> u8 {
        (self.0 & 0xF) as u8
    }
}

/// 生成された 1 チャンク分のブロック／高度／バイオーム。
#[derive(Debug, Clone)]
pub struct Chunk {
    pub min_y: i32,
    pub world_height: u32,
    /// インデックス = ((y - min_y) * CHUNK_SIZE_Z + z) * CHUNK_SIZE_X + x
    pub blocks: Vec<BlockState>,
    /// 各列の最高非空白 y（x, z 順）
    pub heightmap: [[i32; CHUNK_SIZE_X]; CHUNK_SIZE_Z],
    pub biome: RegistryKey,
}

impl Chunk {
    pub fn block(&self, x: usize, y: i32, z: usize) -> BlockState {
        self.blocks[chunk_index(x, y, z, self.min_y, self.world_height)]
    }
    pub fn count_block(&self, b: BlockState) -> usize {
        self.blocks.iter().filter(|&&s| s == b).count()
    }
}

#[inline]
fn chunk_index(x: usize, y: i32, z: usize, min_y: i32, world_height: u32) -> usize {
    ((y - min_y) as usize) * (CHUNK_SIZE_X * CHUNK_SIZE_Z) + z * CHUNK_SIZE_X + x
}

/// 決定論的・シード付き 2D 値ノイズ（滑らか補間）。
#[derive(Debug, Clone)]
pub struct ValueNoise2D {
    seed: u64,
}

impl ValueNoise2D {
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }

    fn hash(&self, x: i64, z: i64) -> f32 {
        let mut h = self.seed
            ^ (x.wrapping_mul(374761393) as u64)
            ^ (z.wrapping_mul(668265263) as u64);
        h = h.wrapping_mul(1274126177);
        h ^= h >> 13;
        ((h & 0xFFFF) as f32) / 65535.0
    }

    /// 単一オクターブの値ノイズ（[0,1)）。
    pub fn sample(&self, x: f32, z: f32) -> f32 {
        let x0 = x.floor() as i64;
        let z0 = z.floor() as i64;
        let xf = x - x0 as f32;
        let zf = z - z0 as f32;
        let v00 = self.hash(x0, z0);
        let v10 = self.hash(x0 + 1, z0);
        let v01 = self.hash(x0, z0 + 1);
        let v11 = self.hash(x0 + 1, z0 + 1);
        let u = smooth(xf);
        let w = smooth(zf);
        let a = lerp(v00, v10, u);
        let b = lerp(v01, v11, u);
        lerp(a, b, w)
    }

    /// フラクタル Brownian 運動（複数オクターブ、[0,1] に正規化）。
    pub fn fbm(&self, x: f32, z: f32, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
        let mut amp = 0.5f32;
        let mut freq = 1.0f32;
        let mut sum = 0.0f32;
        let mut norm = 0.0f32;
        for _ in 0..octaves {
            sum += amp * self.sample(x * freq, z * freq);
            norm += amp;
            amp *= gain;
            freq *= lacunarity;
        }
        if norm > 0.0 {
            sum / norm
        } else {
            0.0
        }
    }
}

#[inline]
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// カスタム鉱脈（オレ生成）の指定。
#[derive(Debug, Clone)]
pub struct OreVein {
    pub block: BlockState,
    pub min_y: i32,
    pub max_y: i32,
    /// 列ごとの生成確率（0..1）。1.0 なら該当列は必ず 1 ブロック生成。
    pub chance_per_column: f32,
}

/// チャンク生成器（vanilla ライクな地形を、Mod がカスタマイズ可能）。
#[derive(Debug, Clone)]
pub struct ChunkGenerator {
    pub id: RegistryKey,
    pub seed: u64,
    pub min_y: i32,
    pub world_height: u32,
    pub base_height: i32,
    pub height_variation: i32,
    pub sea_level: i32,
    pub surface_block: BlockState,
    pub subsurface_block: BlockState,
    pub stone_block: BlockState,
    pub biome: RegistryKey,
    pub ore_veins: Vec<OreVein>,
    noise: ValueNoise2D,
    biome_noise: ValueNoise2D,
}

impl ChunkGenerator {
    pub fn default_for(id: RegistryKey, seed: u64) -> Self {
        ChunkGeneratorBuilder::new(id, seed).build()
    }

    pub fn builder(id: RegistryKey, seed: u64) -> ChunkGeneratorBuilder {
        ChunkGeneratorBuilder::new(id, seed)
    }

    fn biome_for(&self, chunk_x: i32, chunk_z: i32) -> RegistryKey {
        let b = self
            .biome_noise
            .fbm(chunk_x as f32 * 0.1, chunk_z as f32 * 0.1, 2, 2.0, 0.5);
        if b > 0.6 {
            RegistryKey::new("minecraft", "desert")
        } else if b < 0.4 {
            RegistryKey::new("minecraft", "taiga")
        } else {
            self.biome.clone()
        }
    }

    #[inline]
    fn hash_f32(&self, wx: i64, wz: i64, salt: i64) -> f32 {
        let mut h = self.seed
            ^ ((wx.wrapping_mul(2654435761) ^ wz.wrapping_mul(40503) ^ salt) as u64);
        h = h.wrapping_mul(1274126177);
        h ^= h >> 13;
        ((h & 0xFFFF) as f32) / 65535.0
    }

    /// 指定チャンク (chunk_x, chunk_z) を生成する（本物の地形生成）。
    pub fn generate(&self, chunk_x: i32, chunk_z: i32) -> Chunk {
        let cap = (self.world_height as usize) * CHUNK_SIZE_X * CHUNK_SIZE_Z;
        let mut blocks = vec![BlockState::AIR; cap];
        let mut heightmap = [[0i32; CHUNK_SIZE_X]; CHUNK_SIZE_Z];
        let scale = 0.02f32;

        for lx in 0..CHUNK_SIZE_X as i32 {
            for lz in 0..CHUNK_SIZE_Z as i32 {
                let wx = chunk_x * CHUNK_SIZE_X as i32 + lx;
                let wz = chunk_z * CHUNK_SIZE_Z as i32 + lz;
                let n = self
                    .noise
                    .fbm(wx as f32 * scale, wz as f32 * scale, 4, 2.0, 0.5);
                let h = self.base_height
                    + ((n - 0.5) * 2.0 * self.height_variation as f32).round() as i32;
                let top = h.clamp(self.min_y, self.min_y + self.world_height as i32 - 1);
                heightmap[lz as usize][lx as usize] = top;

                // 地表から下へ: 表面→土→石
                for y in self.min_y..=top {
                    let state = if y == top {
                        self.surface_block
                    } else if y >= top - 3 {
                        self.subsurface_block
                    } else {
                        self.stone_block
                    };
                    let idx = chunk_index(lx as usize, y, lz as usize, self.min_y, self.world_height);
                    blocks[idx] = state;
                }

                // 海面より低いところは水で満たす
                if top < self.sea_level {
                    for y in (top + 1)..=self.sea_level {
                        if y >= self.min_y && y < self.min_y + self.world_height as i32 {
                            let idx =
                                chunk_index(lx as usize, y, lz as usize, self.min_y, self.world_height);
                            blocks[idx] = BlockState::WATER;
                        }
                    }
                }

                // カスタム鉱脈の注入（石ブロックのみ置換）
                for v in &self.ore_veins {
                    let r = self.hash_f32(wx as i64, wz as i64, v.block.0 as i64);
                    if r < v.chance_per_column {
                        let yy = (top - 4).max(self.min_y);
                        if yy < top && yy >= self.min_y {
                            let idx = chunk_index(
                                lx as usize,
                                yy,
                                lz as usize,
                                self.min_y,
                                self.world_height,
                            );
                            if blocks[idx] == self.stone_block {
                                blocks[idx] = v.block;
                            }
                        }
                    }
                }
            }
        }

        Chunk {
            min_y: self.min_y,
            world_height: self.world_height,
            blocks,
            heightmap,
            biome: self.biome_for(chunk_x, chunk_z),
        }
    }
}

/// 流れるようなビルダー。Mod が独自の世界生成パラメータを指定するための入口。
pub struct ChunkGeneratorBuilder {
    g: ChunkGenerator,
}

impl ChunkGeneratorBuilder {
    pub fn new(id: RegistryKey, seed: u64) -> Self {
        let g = ChunkGenerator {
            id,
            seed,
            min_y: -64,
            world_height: 384,
            base_height: 64,
            height_variation: 24,
            sea_level: 62,
            surface_block: BlockState::GRASS,
            subsurface_block: BlockState::DIRT,
            stone_block: BlockState::STONE,
            biome: RegistryKey::new("minecraft", "plains"),
            ore_veins: Vec::new(),
            noise: ValueNoise2D::new(seed),
            biome_noise: ValueNoise2D::new(seed ^ 0x9E37_79B9),
        };
        Self { g }
    }
    pub fn min_y(mut self, y: i32) -> Self {
        self.g.min_y = y;
        self
    }
    pub fn world_height(mut self, h: u32) -> Self {
        self.g.world_height = h;
        self
    }
    pub fn base_height(mut self, h: i32) -> Self {
        self.g.base_height = h;
        self
    }
    pub fn height_variation(mut self, v: i32) -> Self {
        self.g.height_variation = v;
        self
    }
    pub fn sea_level(mut self, s: i32) -> Self {
        self.g.sea_level = s;
        self
    }
    pub fn surface_block(mut self, b: BlockState) -> Self {
        self.g.surface_block = b;
        self
    }
    pub fn subsurface_block(mut self, b: BlockState) -> Self {
        self.g.subsurface_block = b;
        self
    }
    pub fn stone_block(mut self, b: BlockState) -> Self {
        self.g.stone_block = b;
        self
    }
    pub fn biome(mut self, b: RegistryKey) -> Self {
        self.g.biome = b;
        self
    }
    pub fn ore_vein(mut self, v: OreVein) -> Self {
        self.g.ore_veins.push(v);
        self
    }
    pub fn build(self) -> ChunkGenerator {
        self.g
    }
}

/// カスタム生成器のレジストリ（Mod が名前付き生成器を登録し、いずれかを有効化）。
#[derive(Default)]
pub struct ChunkGeneratorRegistry {
    generators: HashMap<RegistryKey, Arc<ChunkGenerator>>,
    active: Option<RegistryKey>,
}

impl ChunkGeneratorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, g: ChunkGenerator) -> Result<(), String> {
        if self.generators.contains_key(&g.id) {
            return Err(format!("chunk generator '{}' already registered", g.id.as_str()));
        }
        info!("Registering ChunkGenerator: {}", g.id.as_str());
        self.generators.insert(g.id.clone(), Arc::new(g));
        crate::platform::mark_dirty();
        Ok(())
    }

    /// アクティブな生成器を設定（None ならバニラ生成にフォールバック）。
    pub fn set_active(&mut self, id: &RegistryKey) -> Result<(), String> {
        if !self.generators.contains_key(id) {
            return Err(format!("unknown chunk generator '{}'", id.as_str()));
        }
        self.active = Some(id.clone());
        crate::platform::mark_dirty();
        Ok(())
    }

    pub fn clear_active(&mut self) {
        self.active = None;
        crate::platform::mark_dirty();
    }

    pub fn active_id(&self) -> Option<RegistryKey> {
        self.active.clone()
    }

    pub fn get(&self, id: &RegistryKey) -> Option<Arc<ChunkGenerator>> {
        self.generators.get(id).cloned()
    }

    pub fn active(&self) -> Option<Arc<ChunkGenerator>> {
        self.active.as_ref().and_then(|id| self.generators.get(id).cloned())
    }

    /// アクティブな生成器でチャンクを生成（未設定なら None）。
    pub fn generate_active(&self, chunk_x: i32, chunk_z: i32) -> Option<Chunk> {
        self.active().map(|g| g.generate(chunk_x, chunk_z))
    }

    pub fn len(&self) -> usize {
        self.generators.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_sample_in_unit_range() {
        let n = ValueNoise2D::new(12345);
        for i in 0..200 {
            let v = n.sample(i as f32 * 0.37, (i * 7) as f32 * 0.13);
            assert!(v >= 0.0 && v < 1.0, "sample out of range: {}", v);
        }
    }

    #[test]
    fn fbm_in_unit_range() {
        let n = ValueNoise2D::new(99);
        for i in 0..50 {
            let v = n.fbm(i as f32, (i * 3) as f32, 4, 2.0, 0.5);
            assert!(v >= 0.0 && v <= 1.0, "fbm out of range: {}", v);
        }
    }

    #[test]
    fn builder_sets_fields() {
        let g = ChunkGenerator::builder(RegistryKey::new("mod", "gen"), 7)
            .base_height(40)
            .sea_level(30)
            .surface_block(BlockState::DIRT)
            .build();
        assert_eq!(g.base_height, 40);
        assert_eq!(g.sea_level, 30);
        assert_eq!(g.surface_block, BlockState::DIRT);
    }

    #[test]
    fn generate_dimensions_correct() {
        let g = ChunkGenerator::default_for(RegistryKey::new("mod", "g"), 1);
        let c = g.generate(0, 0);
        assert_eq!(
            c.blocks.len(),
            g.world_height as usize * CHUNK_SIZE_X * CHUNK_SIZE_Z
        );
        assert_eq!(c.heightmap.len(), CHUNK_SIZE_Z);
        assert_eq!(c.heightmap[0].len(), CHUNK_SIZE_X);
    }

    #[test]
    fn column_layering_is_correct() {
        let g = ChunkGenerator::default_for(RegistryKey::new("mod", "g"), 5);
        let c = g.generate(2, 3);
        let top = c.heightmap[0][0];
        assert_eq!(c.block(0, top, 0), g.surface_block);
        assert_eq!(c.block(0, top - 1, 0), g.subsurface_block);
        assert_eq!(c.block(0, top - 4, 0), g.stone_block);
    }

    #[test]
    fn water_fills_below_sea_level() {
        let g = ChunkGenerator::builder(RegistryKey::new("mod", "ocean"), 3)
            .base_height(50)
            .height_variation(4)
            .sea_level(62)
            .build();
        let c = g.generate(0, 0);
        assert!(c.count_block(BlockState::WATER) > 0, "expected water below sea level");
    }

    #[test]
    fn generation_is_deterministic() {
        let g = ChunkGenerator::default_for(RegistryKey::new("mod", "g"), 11);
        let a = g.generate(1, 1);
        let b = g.generate(1, 1);
        assert_eq!(a.blocks, b.blocks);
        assert!(a.count_block(BlockState::AIR) < a.blocks.len()); // not all air
    }

    #[test]
    fn ore_vein_injects_blocks() {
        let g = ChunkGenerator::builder(RegistryKey::new("mod", "ore"), 4)
            .ore_vein(OreVein {
                block: BlockState::IRON_ORE,
                min_y: -64,
                max_y: 64,
                chance_per_column: 1.0,
            })
            .build();
        let c = g.generate(0, 0);
        assert!(
            c.count_block(BlockState::IRON_ORE) > 0,
            "expected iron ore veins to be injected"
        );
    }

    #[test]
    fn ore_vein_zero_chance_injects_nothing() {
        let g = ChunkGenerator::builder(RegistryKey::new("mod", "noore"), 4)
            .ore_vein(OreVein {
                block: BlockState::IRON_ORE,
                min_y: -64,
                max_y: 64,
                chance_per_column: 0.0,
            })
            .build();
        let c = g.generate(0, 0);
        assert_eq!(c.count_block(BlockState::IRON_ORE), 0);
    }

    #[test]
    fn custom_surface_block_reflected() {
        let g = ChunkGenerator::builder(RegistryKey::new("mod", "custom"), 6)
            .surface_block(BlockState::DIRT)
            .build();
        let c = g.generate(0, 0);
        let top = c.heightmap[5][5];
        assert_eq!(c.block(5, top, 5), BlockState::DIRT);
    }

    #[test]
    fn registry_register_set_active_generate() {
        let mut reg = ChunkGeneratorRegistry::new();
        let g = ChunkGenerator::default_for(RegistryKey::new("mod", "myworld"), 21);
        assert!(reg.register(g).is_ok());
        // duplicate rejected
        let dup = ChunkGenerator::default_for(RegistryKey::new("mod", "myworld"), 21);
        assert!(reg.register(dup).is_err());
        // set unknown -> error
        assert!(reg
            .set_active(&RegistryKey::new("mod", "nope"))
            .is_err());
        assert!(reg.set_active(&RegistryKey::new("mod", "myworld")).is_ok());
        let c = reg.generate_active(0, 0);
        assert!(c.is_some());
        assert_eq!(c.unwrap().blocks.len(), 384 * 256);
        reg.clear_active();
        assert!(reg.generate_active(0, 0).is_none());
    }

    #[test]
    fn biome_is_assigned() {
        let g = ChunkGenerator::default_for(RegistryKey::new("mod", "g"), 8);
        let c = g.generate(4, 4);
        assert!(!c.biome.as_str().is_empty());
    }
}
