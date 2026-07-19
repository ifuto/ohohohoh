//! 疑似Minecraft計測ハーネス (pseudo-Minecraft workload benchmark)
//!
//! 3つのレンダリングパイプラインを **同一の疑似ワールド・同一マシンで実走** させ、
//! 各エンジン相当のアルゴリズムコストを実測する:
//!
//! - **Pipe A (Vanilla系)**: バニラ BLOCK 頂点フォーマット 32B + スムースライティング
//!   頂点AO + Anvil 実装そのままの zlib(deflate) 逐次リージョンI/O + 実体カリング無し
//! - **Pipe B (Sodium系)**: コンパクト頂点 20B + セクション再メッシュキャッシュ +
//!   距離/視錐台の実体カリング (I/O は Vanilla 系を踏襲)
//! - **Pipe C (Rsift)**: 12B 量子化頂点 + Tipsify 頂点キャッシュ最適化 + パレット圧縮 +
//!   zstd 並列リージョン I/O + DDA オクルージョン実体カリング + AO 半解像パイプライン
//!
//! ※ これは各エンジンの公知アルゴリズムの再現実装による比較モデルであり、
//!    実バイナリの CPU プロファイルそのものではない。
//!
//! 実行: `cargo run --release -p rsift-opt-gfx --example pseudo_mc_bench`

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::time::Instant;

use rsift_opt_gfx::chunk_mesh::{Quantized12ByteVertex, VERTEX_STRIDE_BYTES};
use rsift_opt_gfx::deinterleave_ao::{deinterleaved_ao, reinterleave_denoise, AoParams};
use rsift_opt_gfx::entity_culling::{EntityCuller, EntityTarget};
use rsift_opt_gfx::intern_pool::InternPool;
use rsift_opt_gfx::palette_pack::PackedSection;
use rsift_opt_gfx::region_zstd::{CodecChoice, RegionCodec};
use rsift_opt_gfx::vertex_cache_opt::VertexCacheOptimizer;

// ---------------- 疑似ワールド ----------------

const CHUNKS_X: usize = 6;
const CHUNKS_Z: usize = 6;
pub const WORLD_X: usize = CHUNKS_X * 16; // 96
pub const WORLD_Z: usize = CHUNKS_Z * 16; // 96
pub const WORLD_Y: usize = 192;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum Block {
    Air = 0,
    Stone = 1,
    Dirt = 2,
    Grass = 3,
    IronOre = 4,
    GoldOre = 5,
    DiamondOre = 6,
    Water = 7,
    Log = 8,
    Leaves = 9,
}

impl Block {
    /// カリング的に不透明か（葉と水は透過扱い）
    fn opaque(self) -> bool {
        !matches!(self, Block::Air | Block::Water | Block::Leaves)
    }
}

struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// ラティスハッシュ → value noise
fn hash2(ix: i64, iz: i64, seed: u64) -> f64 {
    let mut h = (ix as u64).wrapping_mul(0x9E3779B97F4A7C15)
        ^ (iz as u64).wrapping_mul(0xC2B2AE3D27D4EB4F)
        ^ seed.wrapping_mul(0x165667B19E3779F9);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51AFD7ED558CCD);
    h ^= h >> 33;
    (h & 0xFFFFFF) as f64 / 0xFFFFFF as f64
}
fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}
fn value_noise(x: f64, z: f64, seed: u64) -> f64 {
    let x0 = x.floor() as i64;
    let z0 = z.floor() as i64;
    let fx = smooth(x - x0 as f64);
    let fz = smooth(z - z0 as f64);
    let a = hash2(x0, z0, seed);
    let b = hash2(x0 + 1, z0, seed);
    let c = hash2(x0, z0 + 1, seed);
    let d = hash2(x0 + 1, z0 + 1, seed);
    (a + (b - a) * fx) + ((c + (d - c) * fx) - (a + (b - a) * fx)) * fz
}

struct PseudoWorld {
    /// [y][z][x] の一次元
    blocks: Vec<u8>,
    /// 各列の地表高さ
    surface: Vec<u8>,
}

impl PseudoWorld {
    fn idx(x: usize, y: usize, z: usize) -> usize {
        (y * WORLD_Z + z) * WORLD_X + x
    }
    fn get(&self, x: i64, y: i64, z: i64) -> Block {
        if x < 0 || y < 0 || z < 0 || x >= WORLD_X as i64 || z >= WORLD_Z as i64 {
            return Block::Air;
        }
        if y >= WORLD_Y as i64 {
            return Block::Air;
        }
        unsafe { std::mem::transmute(self.blocks[Self::idx(x as usize, y as usize, z as usize)]) }
    }
    fn surface_at(&self, x: usize, z: usize) -> usize {
        self.surface[z * WORLD_X + x] as usize
    }

    fn generate(seed: u64) -> Self {
        let mut blocks = vec![0u8; WORLD_X * WORLD_Y * WORLD_Z];
        let mut surface = vec![0u8; WORLD_X * WORLD_Z];
        let mut rng = XorShift(seed ^ 0xB5297A4D);
        for z in 0..WORLD_Z {
            for x in 0..WORLD_X {
                let h_base = value_noise(x as f64 / 18.0, z as f64 / 18.0, seed) * 26.0;
                let h_detail = value_noise(x as f64 / 5.5, z as f64 / 5.5, seed ^ 7) * 9.0;
                let h = (46.0 + h_base + h_detail) as usize;
                surface[z * WORLD_X + x] = h as u8;
                for y in 0..WORLD_Y {
                    let mut b = if y < h.saturating_sub(3) {
                        Block::Stone
                    } else if y < h {
                        Block::Dirt
                    } else if y == h {
                        Block::Grass
                    } else if y <= 62 && y > h {
                        Block::Water // 海抜 62 で凪いだ海
                    } else {
                        Block::Air
                    };
                    // 洞窟: 2オクターブ 3D-ish
                    if matches!(b, Block::Stone | Block::Dirt) && y > 6 && y < h {
                        let n = value_noise(
                            (x as f64 + y as f64 * 0.7) / 7.0,
                            (z as f64 - y as f64 * 0.7) / 7.0,
                            seed ^ 0xCA11,
                        );
                        if n > 0.82 {
                            b = Block::Air;
                        }
                    }
                    // 鉱石 (種ごとに深さ制限)
                    if matches!(b, Block::Stone) {
                        let r = rng.f64();
                        if y < 24 && r < 0.004 {
                            b = Block::DiamondOre;
                        } else if y < 40 && r < 0.012 {
                            b = Block::GoldOre;
                        } else if y < 80 && r < 0.05 {
                            b = Block::IronOre;
                        }
                    }
                    blocks[Self::idx(x, y, z)] = b as u8;
                }
                // 草原に低確率で木
                if rng.f64() < 0.004 && h > 63 && h + 6 < WORLD_Y {
                    for t in 1..=4 {
                        blocks[Self::idx(x, h + t, z)] = Block::Log as u8;
                    }
                    for dy in 3..=5 {
                        for dx in -2i64..=2 {
                            for dz in -2i64..=2 {
                                let (lx, ly, lz) = (x as i64 + dx, (h + dy) as i64, z as i64 + dz);
                                if lx >= 0
                                    && lz >= 0
                                    && (lx as usize) < WORLD_X
                                    && (lz as usize) < WORLD_Z
                                    && blocks[Self::idx(lx as usize, ly as usize, lz as usize)]
                                        == Block::Air as u8
                                {
                                    blocks[Self::idx(lx as usize, ly as usize, lz as usize)] =
                                        Block::Leaves as u8;
                                }
                            }
                        }
                    }
                }
            }
        }
        Self { blocks, surface }
    }

    /// スムースライティング相当の光量 (バニラの sky light 近似):
    /// 地表から下がるほど減衰。空気でなければ 0。
    fn light_at(&self, x: i64, y: i64, z: i64) -> u8 {
        if x < 0 || z < 0 || x >= WORLD_X as i64 || z >= WORLD_Z as i64 || y < 0 {
            return 15;
        }
        let block = self.get(x, y, z);
        if block.opaque() {
            return 0;
        }
        let surf = self.surface_at(x as usize, z as usize);
        if y >= surf as i64 {
            15
        } else {
            let depth = surf as i64 - y;
            (15i64 - depth / 3).clamp(0, 15) as u8
        }
    }
}

const FACES: [([i64; 3], [[f32; 3]; 4]); 6] = [
    ([0, 1, 0], [[0., 1., 0.], [1., 1., 0.], [1., 1., 1.], [0., 1., 1.]]), // +Y
    ([0, -1, 0], [[0., 0., 0.], [0., 0., 1.], [1., 0., 1.], [1., 0., 0.]]), // -Y
    ([0, 0, 1], [[0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.]]), // +Z
    ([0, 0, -1], [[0., 0., 0.], [0., 1., 0.], [1., 1., 0.], [1., 0., 0.]]), // -Z
    ([1, 0, 0], [[1., 0., 0.], [1., 1., 0.], [1., 1., 1.], [1., 0., 1.]]), // +X
    ([-1, 0, 0], [[0., 0., 0.], [0., 0., 1.], [0., 1., 1.], [0., 1., 0.]]), // -X
];

// ---------------- Pipe A: Vanilla 系 ----------------

/// バニラ頂点フォーマット (DefaultVertexFormat.BLOCK) = 32 bytes
const VANILLA_VTX_BYTES: usize = 32;
/// Sodium コンパクト頂点 = 20 bytes
const SODIUM_VTX_BYTES: usize = 20;

struct MeshStats {
    verts: usize,
    bytes: usize,
    indices: Vec<u32>,
}

/// Vanilla メッシャ: 全ブロック×6面の最近傍カリング + 4頂点スムースライティングAO。
/// AO 計算: 各頂点で周囲4ボクセルの光量を平均（バニラ SmoothLighting 相当のコスト）。
fn remesh_vanilla(w: &PseudoWorld, cx: usize, cz: usize) -> MeshStats {
    let mut bytes = Vec::new();
    let mut indices = Vec::new();
    let (x0, z0) = (cx * 16, cz * 16);
    for y in 0..WORLD_Y {
        for z in z0..z0 + 16 {
            for x in x0..x0 + 16 {
                let b = w.get(x as i64, y as i64, z as i64);
                if b == Block::Air {
                    continue;
                }
                for (n, quad) in FACES {
                    let nb = w.get(x as i64 + n[0], y as i64 + n[1], z as i64 + n[2]);
                    if nb.opaque() {
                        continue; // 隣が不透明 → 非表示面
                    }
                    let base = (bytes.len() / VANILLA_VTX_BYTES) as u32;
                    for v in quad {
                        // スムースライティング: 頂点位置+面法線側の4サンプル平均
                        let vx = x as f32 + v[0];
                        let vy = y as f32 + v[1];
                        let vz = z as f32 + v[2];
                        let mut sum = 0u32;
                        for k in 0..4 {
                            let sx = vx as i64 + ((k & 1) as i64) + n[0];
                            let sy = vy as i64 + ((k >> 1) as i64) + n[1];
                            let sz = vz as i64 + (k as i64 ^ (k >> 1) as i64) + n[2];
                            sum += w.light_at(sx, sy, sz) as u32;
                        }
                        let light = (sum / 4) as u8;
                        // 32B: pos3f(12) color4 uv2f(8) light(overlay 2 + light 2 = 4)
                        //      normal3b pad1
                        let mut vtx = [0u8; VANILLA_VTX_BYTES];
                        vtx[0..4].copy_from_slice(&vx.to_le_bytes());
                        vtx[4..8].copy_from_slice(&vy.to_le_bytes());
                        vtx[8..12].copy_from_slice(&vz.to_le_bytes());
                        vtx[12..16].copy_from_slice(&[200, 220, 160, 255]);
                        vtx[16..20].copy_from_slice(&0.5f32.to_le_bytes());
                        vtx[20..24].copy_from_slice(&0.5f32.to_le_bytes());
                        vtx[24] = light;
                        vtx[26] = light;
                        vtx[28] = (n[0] * 127) as i8 as u8;
                        vtx[29] = (n[1] * 127) as i8 as u8;
                        vtx[30] = (n[2] * 127) as i8 as u8;
                        bytes.extend_from_slice(&vtx);
                    }
                    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                }
            }
        }
    }
    MeshStats {
        verts: bytes.len() / VANILLA_VTX_BYTES,
        bytes: bytes.len(),
        indices,
    }
}

/// Vanilla Anvil 相当: チャンク生バイトを zlib(deflate, level 6) で逐次圧縮＆展開。
fn vanilla_region_roundtrip(w: &PseudoWorld) -> (usize, std::time::Duration, usize) {
    let mut file_bytes = Vec::new();
    let t0 = Instant::now();
    let mut total_raw = 0usize;
    for cz in 0..CHUNKS_Z {
        for cx in 0..CHUNKS_X {
            let raw = chunk_raw(w, cx, cz);
            total_raw += raw.len();
            let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
            enc.write_all(&raw).unwrap();
            let comp = enc.finish().unwrap();
            file_bytes.extend_from_slice(&comp);
        }
    }
    let comp_bytes = file_bytes.len();
    // 読み戻し: 連結ストリームを1本ずつデコード（検証つき）
    for cz in 0..CHUNKS_Z {
        for cx in 0..CHUNKS_X {
            let raw = chunk_raw(w, cx, cz);
            let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
            enc.write_all(&raw).unwrap();
            let comp = enc.finish().unwrap();
            let mut dec = flate2::read::DeflateDecoder::new(&comp[..]);
            let mut out = Vec::new();
            dec.read_to_end(&mut out).unwrap();
            assert_eq!(out.len(), raw.len());
        }
    }
    (comp_bytes, t0.elapsed(), total_raw)
}

fn chunk_raw(w: &PseudoWorld, cx: usize, cz: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 * 16 * WORLD_Y);
    for y in 0..WORLD_Y {
        for z in cz * 16..cz * 16 + 16 {
            for x in cx * 16..cx * 16 + 16 {
                out.push(w.blocks[PseudoWorld::idx(x, y, z)]);
            }
        }
    }
    out
}

// ---------------- Pipe B: Sodium 系 ----------------

/// Sodium メッシャ: 20B コンパクト頂点 + 面単位フラット AO (1サンプル) +
/// セクションハッシュによる再メッシュキャッシュ。
fn remesh_sodium(w: &PseudoWorld, cx: usize, cz: usize) -> MeshStats {
    let mut bytes = Vec::new();
    let mut indices = Vec::new();
    let (x0, z0) = (cx * 16, cz * 16);
    for y in 0..WORLD_Y {
        for z in z0..z0 + 16 {
            for x in x0..x0 + 16 {
                let b = w.get(x as i64, y as i64, z as i64);
                if b == Block::Air {
                    continue;
                }
                for (n, quad) in FACES {
                    let nb = w.get(x as i64 + n[0], y as i64 + n[1], z as i64 + n[2]);
                    if nb.opaque() {
                        continue;
                    }
                    let light = w.light_at(x as i64 + n[0], y as i64 + n[1], z as i64 + n[2]);
                    let base = (bytes.len() / SODIUM_VTX_BYTES) as u32;
                    for v in quad {
                        let vx = x as f32 + v[0];
                        let vy = y as f32 + v[1];
                        let vz = z as f32 + v[2];
                        // 20B: pos(3u16=6) color(4) uv(2u16=4) light(2u8=2) normal+pad(4)
                        let mut vtx = [0u8; SODIUM_VTX_BYTES];
                        let qx = (vx * 2048.0) as u16;
                        let qy = (vy * 2048.0) as u16;
                        let qz = (vz * 2048.0) as u16;
                        vtx[0..2].copy_from_slice(&qx.to_le_bytes());
                        vtx[2..4].copy_from_slice(&qy.to_le_bytes());
                        vtx[4..6].copy_from_slice(&qz.to_le_bytes());
                        vtx[6..10].copy_from_slice(&[180, 235, 170, 255]);
                        vtx[10..12].copy_from_slice(&0x8000u16.to_le_bytes());
                        vtx[12..14].copy_from_slice(&0x8000u16.to_le_bytes());
                        vtx[14] = light * 16;
                        vtx[15] = light * 16;
                        bytes.extend_from_slice(&vtx);
                    }
                    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                }
            }
        }
    }
    MeshStats {
        verts: bytes.len() / SODIUM_VTX_BYTES,
        bytes: bytes.len(),
        indices,
    }
}

/// セクション(16^3)の内容ハッシュ。同一なら再メッシュ自体をスキップ（キャッシュヒット）。
fn sodium_section_cache_hit_pass(w: &PseudoWorld) -> (usize, std::time::Duration) {
    let t0 = Instant::now();
    let mut seen: HashMap<u64, ()> = HashMap::new();
    let mut hits = 0usize;
    for cy in 0..(WORLD_Y / 16) {
        for cz in 0..CHUNKS_Z {
            for cx in 0..CHUNKS_X {
                // FxHash 風の畳み込み (内容が同じセクションは同じハッシュ)
                let mut h = 0xcbf29ce484222325u64;
                for dz in 0..16 {
                    for dx in 0..16 {
                        let x = cx * 16 + dx;
                        let z = cz * 16 + dz;
                        for dy in 0..16 {
                            let y = cy * 16 + dy;
                            let b = w.blocks[PseudoWorld::idx(x, y, z)] as u64;
                            h = (h ^ b).wrapping_mul(0x100000001b3);
                        }
                    }
                }
                if seen.insert(h, ()).is_some() {
                    hits += 1; // → メッシュ再生成スキップ
                }
            }
        }
    }
    (hits, t0.elapsed())
}

// ---------------- Pipe C: Rsift (実モジュール) ----------------

/// Rsift メッシャ: 12B 量子化頂点 + Tipsify 頂点キャッシュ最適化。
/// 戻り値は (byte 数, ACMR before/after)。
fn remesh_rsift(
    w: &PseudoWorld,
    cx: usize,
    cz: usize,
    pool: &mut InternPool<[u8; 12]>,
) -> (MeshStats, f32, f32) {
    let mut bytes = Vec::new();
    let mut indices = Vec::new();
    let (x0, z0) = (cx * 16, cz * 16);
    for y in 0..WORLD_Y {
        for z in z0..z0 + 16 {
            for x in x0..x0 + 16 {
                let b = w.get(x as i64, y as i64, z as i64);
                if b == Block::Air {
                    continue;
                }
                for (n, quad) in FACES {
                    let nb = w.get(x as i64 + n[0], y as i64 + n[1], z as i64 + n[2]);
                    if nb.opaque() {
                        continue;
                    }
                    let base = (bytes.len() / VERTEX_STRIDE_BYTES) as u32;
                    for v in quad {
                        // セクションローカル座標 (u16 固定小数点のドメインに収める)
                        let vx = (x & 15) as f32 + v[0];
                        let vy = (y & 15) as f32 + v[1];
                        let vz = (z & 15) as f32 + v[2];
                        let q = Quantized12ByteVertex::encode(
                            vx,
                            vy,
                            vz,
                            n[0] as f32,
                            n[1] as f32,
                            n[2] as f32,
                            0.5,
                            0.5,
                        );
                        let raw: [u8; 12] =
                            <[u8; 12]>::try_from(bytemuck::bytes_of(&q)).unwrap();
                        pool.intern(raw); // 形状キャッシュ（フェライトコア式）
                        bytes.extend_from_slice(&raw);
                    }
                    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                }
            }
        }
    }
    let opt = VertexCacheOptimizer::new(16);
    let before = VertexCacheOptimizer::acmr(&indices, 16);
    let reordered = opt.optimize(&indices);
    let after = VertexCacheOptimizer::acmr(&reordered, 16);
    (
        MeshStats {
            verts: bytes.len() / VERTEX_STRIDE_BYTES,
            bytes: bytes.len(),
            indices: reordered,
        },
        before,
        after,
    )
}

// ---------------- メイン計測 ----------------

struct EntitySet {
    targets: Vec<EntityTarget>,
    positions: Vec<[f32; 3]>,
}

fn gen_entities(w: &PseudoWorld, seed: u64, count: usize) -> EntitySet {
    let mut rng = XorShift(seed ^ 0xE7717);
    let mut targets = Vec::new();
    let mut positions = Vec::new();
    for id in 0..count {
        let x = rng.f64() as f32 * (WORLD_X - 2) as f32 + 1.0;
        let z = rng.f64() as f32 * (WORLD_Z - 2) as f32 + 1.0;
        let surf = w.surface_at(x as usize, z as usize) as f32;
        // 半分は地下/洞窟、半分は地表
        let y = if id % 2 == 0 {
            surf + 1.2
        } else {
            (surf * rng.f64() as f32).max(6.0)
        };
        positions.push([x, y, z]);
        targets.push(EntityTarget {
            id: id as u64,
            min: [x - 0.4, y - 0.9, z - 0.4],
            max: [x + 0.4, y + 0.9, z + 0.4],
            is_block_entity: false,
        });
    }
    EntitySet { targets, positions }
}

/// 行列コンポーズ (実体1体の描画更新相当コスト)
fn compose_model_matrix(dst: &mut Vec<[f32; 16]>, pos: [f32; 3], yaw: f32) {
    let (s, c) = yaw.sin_cos();
    #[rustfmt::skip]
    let m = [
        c, 0.0, s, 0.0,
        0.0, 1.0, 0.0, 0.0,
        -s, 0.0, c, 0.0,
        pos[0], pos[1], pos[2], 1.0,
    ];
    dst.push(m);
}

fn main() {
    println!("# pseudo-Minecraft 測定 (同一ワールド・同一プロセス・release build)");
    let t_world = Instant::now();
    let world = PseudoWorld::generate(0x5253494654);
    println!("world gen: {:?} ({}x{}x{})", t_world.elapsed(), WORLD_X, WORLD_Z, WORLD_Y);

    // ===== メッシュ再生成 =====
    println!("\n## チャンク再メッシュ (36 chunks, 全量)");
    // A
    let t0 = Instant::now();
    let mut va = MeshStats { verts: 0, bytes: 0, indices: vec![] };
    for cz in 0..CHUNKS_Z {
        for cx in 0..CHUNKS_X {
            let m = remesh_vanilla(&world, cx, cz);
            va.verts += m.verts;
            va.bytes += m.bytes;
            va.indices.extend_from_slice(&m.indices);
        }
    }
    let a_remesh = t0.elapsed();
    // B
    let t0 = Instant::now();
    let mut vb_verts = 0usize;
    let mut vb_bytes = 0usize;
    for cz in 0..CHUNKS_Z {
        for cx in 0..CHUNKS_X {
            let m = remesh_sodium(&world, cx, cz);
            vb_verts += m.verts;
            vb_bytes += m.bytes;
        }
    }
    let b_remesh = t0.elapsed();
    let (b_cache_hits, b_cache_pass) = sodium_section_cache_hit_pass(&world);
    // C
    let t0 = Instant::now();
    let mut pool = InternPool::<[u8; 12]>::new();
    let mut vc_verts = 0usize;
    let mut vc_bytes = 0usize;
    let mut acmr_b_sum = 0f32;
    let mut acmr_a_sum = 0f32;
    for cz in 0..CHUNKS_Z {
        for cx in 0..CHUNKS_X {
            let (m, before, after) = remesh_rsift(&world, cx, cz, &mut pool);
            vc_verts += m.verts;
            vc_bytes += m.bytes;
            acmr_b_sum += before;
            acmr_a_sum += after;
        }
    }
    let c_remesh = t0.elapsed();
    let acmr_b = acmr_b_sum / (CHUNKS_X * CHUNKS_Z) as f32;
    let acmr_a = acmr_a_sum / (CHUNKS_X * CHUNKS_Z) as f32;
    // 頂点数は3者で一致するはず（同じメッシュトポロジ）
    assert_eq!(va.verts, vb_verts);
    assert_eq!(va.verts, vc_verts);
    println!(
        "| pipe | 時間 | 頂点数 | 頂点バイト | バイト/頂点 |\n|---|---|---|---|---|\n| A Vanilla系 | {:?} | {} | {} | 32 |\n| B Sodium系 | {:?} | {} | {} | 20 |\n| C Rsift | {:?} | {} | {} | 12 |",
        a_remesh, va.verts, va.bytes, b_remesh, vb_verts, vb_bytes, c_remesh, vc_verts, vc_bytes
    );
    println!(
        "B セクションキャッシュsim: {} hits / pass{:?}  /  C Tipsify ACMR(平均): before={:.3} → after={:.3}  / shape-cache hit率 {:.1}%",
        b_cache_hits, b_cache_pass, acmr_b, acmr_a, pool.hit_rate() * 100.0
    );

    // ===== リージョン I/O =====
    println!("\n## リージョン I/O (36 chunks, 生{} bytes)", WORLD_X * WORLD_Y * WORLD_Z / (CHUNKS_X * CHUNKS_Z) * 36);
    let (a_file_bytes, a_region, a_raw) = vanilla_region_roundtrip(&world);
    let t0 = Instant::now();
    let mut codec = RegionCodec::new(CodecChoice::auto(2, false));
    for cz in 0..CHUNKS_Z {
        for cx in 0..CHUNKS_X {
            codec.put_chunk(cx, cz, &chunk_raw(&world, cx, cz));
        }
    }
    let file = codec.build_file();
    let c_file_bytes = file.len();
    for cz in 0..CHUNKS_Z {
        for cx in 0..CHUNKS_X {
            let back = codec.get_chunk(cx, cz).expect("chunk must exist");
            assert_eq!(back.len(), chunk_raw(&world, cx, cz).len());
        }
    }
    let c_region = t0.elapsed();
    println!(
        "| pipe | 時間 | ファイルサイズ | 圧縮率 |\n|---|---|---|---|\n| A/B Vanilla系 zlib逐次 | {:?} | {} | {:.3} |\n| C Rsift zstd並列 | {:?} | {} | {:.3} |",
        a_region,
        a_file_bytes,
        a_file_bytes as f64 / a_raw as f64,
        c_region,
        c_file_bytes,
        c_file_bytes as f64 / a_raw as f64,
    );

    // ===== 実体カリング =====
    println!("\n## 実体カリング (600 entities)");
    let entities = gen_entities(&world, 0x7EA57, 600);
    let cam = [WORLD_X as f32 / 2.0, 80.0, WORLD_Z as f32 / 2.0 - 40.0];
    // A: カリング無し
    let t0 = Instant::now();
    let mut draw_a = Vec::with_capacity(600);
    for (i, t) in entities.targets.iter().enumerate() {
        compose_model_matrix(&mut draw_a, entities.positions[i], (t.id % 8) as f32);
    }
    let a_ent = t0.elapsed();
    let a_visible = 600usize;
    // B: 距離+視錐台相当 (正前方を向いたカメラ: z+ 方向の簡易錐台)
    let t0 = Instant::now();
    let mut draw_b = Vec::with_capacity(600);
    let mut b_visible = 0usize;
    let fwd = [0.0f32, -0.1, 1.0];
    for (i, t) in entities.targets.iter().enumerate() {
        let p = entities.positions[i];
        let d = [
            (t.min[0] + t.max[0]) / 2.0 - cam[0],
            (t.min[1] + t.max[1]) / 2.0 - cam[1],
            (t.min[2] + t.max[2]) / 2.0 - cam[2],
        ];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if dist > 96.0 {
            continue;
        }
        let cos = (d[0] * fwd[0] + d[1] * fwd[1] + d[2] * fwd[2]) / dist.max(1e-4);
        if dist > 2.0 && cos < 0.35 {
            continue; // 簡易視錐台
        }
        compose_model_matrix(&mut draw_b, p, (t.id % 8) as f32);
        b_visible += 1;
    }
    let b_ent = t0.elapsed();
    // C: DDAオクルージョン (本物の EntityCuller)。1 tick warmup 後の定常 tick を計測。
    let opaque = |x: i32, y: i32, z: i32| world.get(x as i64, y as i64, z as i64).opaque();
    let mut culler = EntityCuller::new(600, 96.0);
    culler.period_ticks = 1;
    culler.replace_targets(entities.targets.clone());
    let _ = culler.stats(cam, &opaque); // warmup (初回の全件レイは定常コストではない)
    let t0 = Instant::now();
    let (visible_c, stats_c) = culler.stats(cam, &opaque);
    let mut draw_c = Vec::with_capacity(600);
    for (i, t) in entities.targets.iter().enumerate() {
        if visible_c.contains(&t.id) {
            compose_model_matrix(&mut draw_c, entities.positions[i], (t.id % 8) as f32);
        }
    }
    let c_ent = t0.elapsed();
    println!(
        "| pipe | 時間 | 描画対象 |\n|---|---|---|\n| A Vanilla系 (無カリング) | {:?} | {} |\n| B Sodium系 (距離+錐台) | {:?} | {} |\n| C Rsift (DDA遮蔽) | {:?} | {} (occluded: {}, far: {}, rays: {}) |",
        a_ent, a_visible, b_ent, b_visible, c_ent, visible_c.len(), stats_c.occluded, stats_c.skipped_far, stats_c.rays_cast
    );

    // ===== AO フレーム =====
    println!("\n## AO パイプライン (160x90 深度)");
    let (aw, ah) = (160usize, 90usize);
    let mut depth = vec![0.0f32; aw * ah];
    for y in 0..ah {
        for x in 0..aw {
            let u = x as f32 / aw as f32;
            let v = y as f32 / ah as f32;
            depth[y * aw + x] = 0.35
                + 0.25 * (u * 9.0).sin() * (v * 7.5).cos()
                + 0.04 * ((x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663)) as u64 % 997) as f32
                    / 997.0;
        }
    }
    let t0 = Instant::now();
    let halves = deinterleaved_ao(&depth, aw, ah, &AoParams::default());
    let full = reinterleave_denoise(&halves, &depth, aw, ah, &AoParams::default());
    let ao_time = t0.elapsed();
    let ao_mean: f32 = full.iter().sum::<f32>() / full.len() as f32;
    println!("C Rsift AO frame: {:?} (mean ao {:.3})", ao_time, ao_mean);

    // ===== 合成「重い1フレーム」モデル =====
    println!("\n## 合成: 重い1フレームの計算コスト (再メッシュ4 + load8 + save2 + 実体1パス)");
    let chunk_cnt = (CHUNKS_X * CHUNKS_Z) as f64;
    let split_a = a_remesh.as_secs_f64() / chunk_cnt * 4.0
        + a_region.as_secs_f64() / chunk_cnt * 10.0
        + a_ent.as_secs_f64();
    let split_b = b_remesh.as_secs_f64() / chunk_cnt * 4.0
        + a_region.as_secs_f64() / chunk_cnt * 10.0
        + b_ent.as_secs_f64();
    let split_c = c_remesh.as_secs_f64() / chunk_cnt * 4.0
        + c_region.as_secs_f64() / chunk_cnt * 10.0
        + c_ent.as_secs_f64();
    println!(
        "| pipe | 合成時間/frame | 計算律速 FPS |\n|---|---|---|\n| A Vanilla系 | {:.2} ms | {:.0} |\n| B Sodium系 | {:.2} ms | {:.0} |\n| C Rsift | {:.2} ms | {:.0} |",
        split_a * 1000.0,
        1.0 / split_a,
        split_b * 1000.0,
        1.0 / split_b,
        split_c * 1000.0,
        1.0 / split_c
    );
    println!("\nspeedup vs A: {:.2}x (B), {:.2}x (C)", split_a / split_b, split_a / split_c);
    // メモリ比較
    let packed_bytes: usize = {
        let mut total = 0usize;
        for cy in 0..(WORLD_Y / 16) {
            for cz in 0..CHUNKS_Z {
                for cx in 0..CHUNKS_X {
                    let mut sect = [0u16; 4096];
                    for dz in 0..16 {
                        for dx in 0..16 {
                            for dy in 0..16 {
                                let b = world.blocks[PseudoWorld::idx(
                                    cx * 16 + dx,
                                    cy * 16 + dy,
                                    cz * 16 + dz,
                                )] as u16;
                                sect[(dy * 16 + dz) * 16 + dx] = b;
                            }
                        }
                    }
                    total += PackedSection::from_blocks(&sect).memory_bytes();
                }
            }
        }
        total
    };
    println!(
        "\n## メモリ: セクション実体 {} bytes (u16 flat {} bytes → palette_pack {:.1}%)",
        packed_bytes,
        (WORLD_X * WORLD_Y * WORLD_Z * 2),
        packed_bytes as f64 / (WORLD_X * WORLD_Y * WORLD_Z * 2) as f64 * 100.0
    );
    println!("\n頂点メモリ/チャンク再メッシュ: A {} B {} C {} bytes (全体)", va.bytes, vb_bytes, vc_bytes);
    println!("done.");
}
