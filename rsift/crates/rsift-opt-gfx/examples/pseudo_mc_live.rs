//! 疑似Minecraft ライブ計測 (headless pseudo-Minecraft LIVE benchmark)
//!
//! 画面出力の無い架空Minecraftを **実際にゲームループで走らせる**:
//! モブを大量スポーンさせ、徘徊AI・重力・衝突・水浮力・デスポーン/リスポーン・
//! ブロック踏み荒らし(セクション汚染)・定期オートセーブを 240 tick 実シミュレート
//! して「録画」し、その同一録画を 3 パイプラインがレンダリング側だけリプレイする。
//!
//! - **Pipe A (Vanilla系)**: 全実体を毎フレーム描画 (カリング無し) + 4-tap スムース
//!   ライティング 32B 頂点のセクション再メッシュ + zlib(deflate) 逐次オートセーブ
//! - **Pipe B (Sodium系)**: 距離+視錐台カリング + 面フラット光 20B 頂点 +
//!   セクションハッシュによる再メッシュスキップ + A と同じ zlib I/O
//! - **Pipe C (Rsift, ポテトPC既定)**: EntityCuller (距離ゲート64 + 近距離DDA遮蔽,
//!   tick分割 amortized) + セクション内頂点溶接 + 12B 量子化 (セクションローカル
//!   座標) + InternPool 形状共有 + zstd 並列リージョン I/O。
//!   Tipsify 頂点キャッシュ最適化は opt-in 品質機能 (GPU側の負荷軽減が目的で、
//!   headless CPU 計測ではコストしか出ない) のため対決からは外し、各シナリオ末に
//!   1 セクションの診断実行のみ行う (計測外)。
//!
//! シム時間は 3 者共通 (サーバ側コスト)。frame 時間 = sim + 各pipeのrender。
//! 実体描画は全pipeで 8ボーン行列階層+歩行アニメ+2点ライトを実計算
//! (実レンダラの実体パス相当)。スポーンの 40% は地下洞窟 (遮蔽環境を再現)。
//! ※ 各エンジンの公知アルゴリズムの再現モデルによる比較であり、実バイナリの
//!    CPU プロファイルそのものではない。画面出力は含まない (headless)。
//!
//! 実行: `cargo run --release -p rsift-opt-gfx --example pseudo_mc_live`

use std::collections::{HashMap, HashSet};
use std::hint::black_box;
use std::io::Write;
use std::time::Instant;

use rsift_opt_gfx::chunk_mesh::{Quantized12ByteVertex, VERTEX_STRIDE_BYTES};
use rsift_opt_gfx::entity_culling::{EntityCuller, FastEntityCuller, EntityTarget};
use rsift_opt_gfx::intern_pool::InternPool;
use rsift_opt_gfx::region_zstd::{CodecChoice, RegionCodec};
use rsift_opt_gfx::vertex_cache_opt::VertexCacheOptimizer;

// ---------------- 疑似ワールド ----------------

const CHUNKS_X: usize = 6;
const CHUNKS_Z: usize = 6;
const WORLD_X: usize = CHUNKS_X * 16; // 96
const WORLD_Z: usize = CHUNKS_Z * 16; // 96
const WORLD_Y: usize = 192;
const TICKS: usize = 240;
const SAVE_PERIOD: usize = 60;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum Block {
    Air = 0,
    Stone = 1,
    Dirt = 2,
    Grass = 3,
    Sand = 4,
    IronOre = 5,
    CoalOre = 6,
    Water = 7,
    Log = 8,
    Leaves = 9,
}

impl Block {
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

#[derive(Clone)]
struct PseudoWorld {
    blocks: Vec<u8>,
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
                        Block::Water
                    } else {
                        Block::Air
                    };
                    // 砂浜
                    if matches!(b, Block::Grass | Block::Dirt) && h >= 60 && h <= 64 {
                        b = if y == h { Block::Sand } else { b };
                    }
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
                    if matches!(b, Block::Stone) {
                        let r = rng.f64();
                        if r < 0.02 {
                            b = Block::CoalOre;
                        } else if y < 40 && r < 0.028 {
                            b = Block::IronOre;
                        }
                    }
                    blocks[Self::idx(x, y, z)] = b as u8;
                }
                if rng.f64() < 0.005 && h > 63 && h + 6 < WORLD_Y {
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

    fn world_hash(&self) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for (i, &b) in self.blocks.iter().enumerate() {
            h = (h ^ (b as u64).wrapping_add(i as u64)).wrapping_mul(0x100000001b3);
        }
        h
    }
}

const FACES: [([i64; 3], [[f32; 3]; 4]); 6] = [
    ([0, 1, 0], [[0., 1., 0.], [1., 1., 0.], [1., 1., 1.], [0., 1., 1.]]),
    ([0, -1, 0], [[0., 0., 0.], [0., 0., 1.], [1., 0., 1.], [1., 0., 0.]]),
    ([0, 0, 1], [[0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.]]),
    ([0, 0, -1], [[0., 0., 0.], [0., 1., 0.], [1., 1., 0.], [1., 0., 0.]]),
    ([1, 0, 0], [[1., 0., 0.], [1., 1., 0.], [1., 1., 1.], [1., 0., 1.]]),
    ([-1, 0, 0], [[0., 0., 0.], [0., 0., 1.], [0., 1., 1.], [0., 1., 0.]]),
];

// ---------------- モブシミュレーション ----------------

struct Mob {
    id: u64,
    pos: [f32; 3],
    vel: [f32; 3],
    yaw: f32,
    tgt: [f32; 3],
    retarget_in: u32,
    alive: bool,
}

/// 1 tick 分の録画データ
struct FrameRec {
    /// (id, pos, yaw) of alive mobs
    view: Vec<(u64, [f32; 3], f32)>,
    edits: Vec<(usize, usize, usize, u8)>,
    dirty_secs: Vec<(usize, usize, usize)>,
    dirty_chunks: Vec<(usize, usize)>,
    autosave: bool,
    cam: [f32; 3],
    fwd: [f32; 3],
}

struct SimOut {
    frames: Vec<FrameRec>,
    sim_ms: Vec<f64>,
    spawned_total: usize,
}

fn run_sim(world: &PseudoWorld, mob_count: usize, seed: u64) -> (SimOut, PseudoWorld) {
    let mut w = world.clone();
    let mut rng = XorShift(seed ^ 0x1DEA);
    let center = [WORLD_X as f32 / 2.0, 0.0, WORLD_Z as f32 / 2.0];

    let spawn_at = |rng: &mut XorShift, w: &PseudoWorld, cx: f32, cz: f32, id: u64| -> Mob {
        let ang = rng.f64() as f32 * std::f32::consts::TAU;
        let dist = 24.0 + rng.f64() as f32 * 56.0;
        let x = (cx + ang.cos() * dist).clamp(1.0, (WORLD_X - 2) as f32);
        let z = (cz + ang.sin() * dist).clamp(1.0, (WORLD_Z - 2) as f32);
        let surf = w.surface_at(x as usize, z as usize) as f32;
        // 40% は地下洞窟スポーン (モブファーム状況: 遮蔽カリングが本領を発揮する環境)
        let mut pos = [x, surf + 1.2, z];
        if rng.f64() < 0.4 {
            let mut y = surf as i64 - 4;
            let mut steps = 0;
            while y > 8 && steps < 48 {
                if !w.get(x as i64, y, z as i64).opaque()
                    && !w.get(x as i64, y + 1, z as i64).opaque()
                    && w.get(x as i64, y - 1, z as i64).opaque()
                {
                    pos = [x, y as f32 + 0.1, z];
                    break;
                }
                y -= 1;
                steps += 1;
            }
        }
        Mob {
            id,
            pos,
            vel: [0.0; 3],
            yaw: rng.f64() as f32 * std::f32::consts::TAU,
            tgt: pos,
            retarget_in: 20 + (rng.next() % 120) as u32,
            alive: true,
        }
    };

    let mut mobs: Vec<Mob> = Vec::with_capacity(mob_count + 64);
    let mut next_id = 0u64;
    let mut spawned_total = 0usize;
    for _ in 0..mob_count {
        mobs.push(spawn_at(&mut rng, &w, center[0], center[2], next_id));
        next_id += 1;
        spawned_total += 1;
    }

    let mut frames = Vec::with_capacity(TICKS);
    let mut sim_ms = Vec::with_capacity(TICKS);

    for tick in 0..TICKS {
        let t0 = Instant::now();
        // カメラ周回軌道 (高さ84、半径30)
        let ang = tick as f32 * 0.011;
        let cam = [
            center[0] + ang.cos() * 30.0,
            84.0,
            center[2] + ang.sin() * 30.0,
        ];
        let fwd = [-ang.sin(), 0.0, ang.cos()];

        let mut edits = Vec::new();
        let mut dirty_secs: Vec<(usize, usize, usize)> = Vec::new();
        let mut dirty_chunks: Vec<(usize, usize)> = Vec::new();
        let mut edit_budget = 3; // 1 tick あたりの世界改変 (踏み荒らし) 上限

        // モブキャップ維持: デスポーン判定 + 補充スポーン
        let mut alive_cnt = 0usize;
        for m in mobs.iter_mut() {
            if !m.alive {
                continue;
            }
            let dx = m.pos[0] - cam[0];
            let dz = m.pos[2] - cam[2];
            if dx * dx + dz * dz > 96.0 * 96.0 {
                m.alive = false; // 距離デスポーン
                continue;
            }
            alive_cnt += 1;
        }
        while alive_cnt < mob_count {
            mobs.push(spawn_at(&mut rng, &w, cam[0], cam[2], next_id));
            next_id += 1;
            spawned_total += 1;
            alive_cnt += 1;
        }

        // 個体更新: 徘徊ステアリング + 重力 + ブロック衝突
        for m in mobs.iter_mut() {
            if !m.alive {
                continue;
            }
            // リターゲット
            if m.retarget_in == 0 {
                let ang2 = rng.f64() as f32 * std::f32::consts::TAU;
                let d = 6.0 + rng.f64() as f32 * 22.0;
                let tx = (m.pos[0] + ang2.cos() * d).clamp(1.0, (WORLD_X - 2) as f32);
                let tz = (m.pos[2] + ang2.sin() * d).clamp(1.0, (WORLD_Z - 2) as f32);
                m.tgt = [tx, m.pos[1], tz];
                m.retarget_in = 40 + (rng.next() % 160) as u32;
            }
            m.retarget_in -= 1;
            // ステアリング
            let ddx = m.tgt[0] - m.pos[0];
            let ddz = m.tgt[2] - m.pos[2];
            let dh = (ddx * ddx + ddz * ddz).sqrt().max(1e-3);
            let speed = 0.14;
            m.vel[0] += (ddx / dh) * speed * 0.15;
            m.vel[2] += (ddz / dh) * speed * 0.15;
            m.yaw = ddz.atan2(ddx);
            // 水: 浮力
            let feet = w.get(m.pos[0] as i64, m.pos[1] as i64, m.pos[2] as i64);
            if feet == Block::Water {
                m.vel[1] += 0.14;
                if m.vel[1] > 0.06 {
                    m.vel[1] = 0.06;
                }
            } else {
                m.vel[1] -= 0.08; // 重力
            }
            if m.vel[1] < -0.5 {
                m.vel[1] = -0.5;
            }
            // 水平移動 + 衝突 (頭・足)
            for axis in [0usize, 2usize] {
                let mut np = m.pos;
                np[axis] += m.vel[axis];
                let bx = np[0].floor() as i64;
                let by = np[1].floor() as i64;
                let bz = np[2].floor() as i64;
                let head_clear = !w.get(bx, by + 1, bz).opaque();
                let feet_clear = !w.get(bx, by, bz).opaque();
                if head_clear && feet_clear {
                    m.pos[axis] = np[axis];
                } else {
                    m.vel[axis] = 0.0;
                    m.retarget_in = 0; // 詰まり → 即リターゲット
                }
            }
            // 垂直移動 + 床
            {
                let by_new = (m.pos[1] + m.vel[1]).floor() as i64;
                let bx = m.pos[0].floor() as i64;
                let bz = m.pos[2].floor() as i64;
                if m.vel[1] <= 0.0 {
                    if w.get(bx, by_new - 1, bz).opaque() || by_new <= 1 {
                        let ground = (by_new) as f32;
                        m.pos[1] = ground.max(1.0);
                        m.vel[1] = 0.0;
                    } else {
                        m.pos[1] += m.vel[1];
                    }
                } else {
                    if w.get(bx, by_new + 1, bz).opaque() {
                        m.vel[1] = 0.0;
                    } else {
                        m.pos[1] += m.vel[1];
                    }
                }
                // 速度減衰
                m.vel[0] *= 0.86;
                m.vel[2] *= 0.86;
            }
            // 踏み荒らし: 足元の草を土に (セクション汚染イベント)
            if edit_budget > 0 && (m.pos[0] - m.tgt[0]).abs() + (m.pos[2] - m.tgt[2]).abs() > 1.0 {
                let bx = m.pos[0].floor() as i64;
                let by = m.pos[1].floor() as i64 - 1;
                let bz = m.pos[2].floor() as i64;
                if w.get(bx, by, bz) == Block::Grass {
                    let (ux, uy, uz) = (bx as usize, by as usize, bz as usize);
                    w.blocks[PseudoWorld::idx(ux, uy, uz)] = Block::Dirt as u8;
                    edits.push((ux, uy, uz, Block::Dirt as u8));
                    let sec = (ux / 16, uy / 16, uz / 16);
                    if !dirty_secs.contains(&sec) {
                        dirty_secs.push(sec);
                    }
                    let ch = (ux / 16, uz / 16);
                    if !dirty_chunks.contains(&ch) {
                        dirty_chunks.push(ch);
                    }
                    edit_budget -= 1;
                }
            }
        }

        let view: Vec<(u64, [f32; 3], f32)> = mobs
            .iter()
            .filter(|m| m.alive)
            .map(|m| (m.id, m.pos, m.yaw))
            .collect();
        let autosave = tick % SAVE_PERIOD == SAVE_PERIOD - 1;
        sim_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        frames.push(FrameRec {
            view,
            edits,
            dirty_secs,
            dirty_chunks,
            autosave,
            cam,
            fwd,
        });
    }
    (SimOut { frames, sim_ms, spawned_total }, w)
}

// ---------------- 描画側ユーティリティ ----------------

/// 実体1体の描画更新相当: ライト取得 + 8ボーン (root/body/head/四肢x2/尾) の
/// 行列階層コンポーズ + 歩行アニメ。実レンダラの実体パスが毎フレームやる計算。
fn render_mob(scratch: &mut Vec<[f32; 16]>, w: &PseudoWorld, pos: [f32; 3], yaw: f32, phase: f32) {
    // バニラ実体ライティング近似: 足元と頭の2サンプル平均
    let l0 = w.light_at(pos[0] as i64, pos[1] as i64, pos[2] as i64) as f32;
    let l1 = w.light_at(pos[0] as i64, pos[1] as i64 + 1, pos[2] as i64) as f32;
    let light = (l0 + l1) * 0.5 / 15.0;
    let (sy, cy) = yaw.sin_cos();
    let walk = phase * 6.0;
    // 8ボーン: (親からの局所オフセット, 揺れ位相)
    const BONES: [([f32; 3], f32); 8] = [
        ([0.0, 0.0, 0.0], 0.0),      // root
        ([0.0, 0.6, 0.0], 0.0),      // body
        ([0.0, 1.35, 0.0], 0.6),     // head
        ([-0.25, 0.05, 0.0], 0.0),   // leg L
        ([0.25, 0.05, 0.0], 3.14),   // leg R
        ([-0.35, 0.9, 0.0], 3.14),   // arm L
        ([0.35, 0.9, 0.0], 0.0),     // arm R
        ([0.0, 0.55, -0.4], 1.2),    // tail
    ];
    for (off, ph) in BONES {
        let swing = (walk + ph).sin() * 0.35;
        let (ss, cs) = swing.sin_cos();
        // world = root(yaw回転) * bone(局所揺れ) — 実際の階層行列合成
        #[rustfmt::skip]
        let bm = [
            cy * cs, -sy, cy * ss, 0.0,
            sy * cs * light, cy, sy * ss, swing * 0.02,
            -ss, 0.0, cs, 0.0,
            pos[0] + off[0] * cy - off[2] * sy,
            pos[1] + off[1],
            pos[2] + off[0] * sy + off[2] * cy,
            1.0,
        ];
        scratch.push(bm);
    }
}

#[derive(Default)]
struct PipeOut {
    frame_ms: Vec<f64>,
    cull_ms: f64,
    mob_ms: f64,
    mesh_ms: f64,
    io_ms: f64,
    rendered_mobs: u64,
    mob_vtx_bytes: u64,
    mesh_vtx_bytes: u64,
    rays_total: u64,
    far_total: u64,
    checksum: f32,
    final_hash: u64,
}

/// 描画モブ1体あたりの頂点データ量 (簡易モデル: 9クアッド=36頂点)
const MOB_VERTS: usize = 36;

// ---- Pipe A: Vanilla 系 ----

fn mesh_section_vanilla(w: &PseudoWorld, sx: usize, sy: usize, sz: usize, scratch: &mut Vec<u8>) -> usize {
    let start = scratch.len();
    for y in sy * 16..sy * 16 + 16 {
        for z in sz * 16..sz * 16 + 16 {
            for x in sx * 16..sx * 16 + 16 {
                let b = w.get(x as i64, y as i64, z as i64);
                if b == Block::Air {
                    continue;
                }
                for (n, quad) in FACES {
                    let nb = w.get(x as i64 + n[0], y as i64 + n[1], z as i64 + n[2]);
                    if nb.opaque() {
                        continue;
                    }
                    for v in quad {
                        let vx = x as f32 + v[0];
                        let vy = y as f32 + v[1];
                        let vz = z as f32 + v[2];
                        let mut sum = 0u32;
                        for k in 0..4 {
                            let sx2 = vx as i64 + ((k & 1) as i64) + n[0];
                            let sy2 = vy as i64 + ((k >> 1) as i64) + n[1];
                            let sz2 = vz as i64 + (k as i64 ^ (k >> 1) as i64) + n[2];
                            sum += w.light_at(sx2, sy2, sz2) as u32;
                        }
                        let light = (sum / 4) as u8;
                        // 32B Vanilla BLOCK 頂点: pos3f color4 uv2f light4 normal3b pad1
                        let mut vtx = [0u8; 32];
                        vtx[0..4].copy_from_slice(&vx.to_le_bytes());
                        vtx[4..8].copy_from_slice(&vy.to_le_bytes());
                        vtx[8..12].copy_from_slice(&vz.to_le_bytes());
                        vtx[12..16].copy_from_slice(&[200, 220, 160, 255]);
                        vtx[16..20].copy_from_slice(&0.5f32.to_le_bytes());
                        vtx[20..24].copy_from_slice(&0.5f32.to_le_bytes());
                        vtx[24] = light;
                        vtx[26] = light;
                        scratch.extend_from_slice(&vtx);
                    }
                }
            }
        }
    }
    let n = scratch.len() - start;
    if scratch.len() > 1 << 20 {
        scratch.truncate(0);
    }
    n
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

fn zlib_save(w: &PseudoWorld, dirty: &HashSet<(usize, usize)>) -> (u64, f64) {
    let t0 = Instant::now();
    let mut bytes = 0u64;
    for &(cx, cz) in dirty {
        let raw = chunk_raw(w, cx, cz);
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
        enc.write_all(&raw).unwrap();
        bytes += enc.finish().unwrap().len() as u64;
    }
    (bytes, t0.elapsed().as_secs_f64() * 1000.0)
}

// ---- Pipe B: Sodium 系 ----

fn section_hash(w: &PseudoWorld, sx: usize, sy: usize, sz: usize) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for dy in 0..16 {
        for dz in 0..16 {
            for dx in 0..16 {
                let b = w.blocks[PseudoWorld::idx(sx * 16 + dx, sy * 16 + dy, sz * 16 + dz)] as u64;
                h = (h ^ b).wrapping_mul(0x100000001b3);
            }
        }
    }
    h
}

fn mesh_section_sodium(w: &PseudoWorld, sx: usize, sy: usize, sz: usize, scratch: &mut Vec<u8>) -> usize {
    let start = scratch.len();
    for y in sy * 16..sy * 16 + 16 {
        for z in sz * 16..sz * 16 + 16 {
            for x in sx * 16..sx * 16 + 16 {
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
                    for v in quad {
                        // 20B コンパクト頂点: pos(3u16) color(4) uv(2u16) light(2) normal+pad(4)
                        let mut vtx = [0u8; 20];
                        let qx = ((x as f32 + v[0]) * 2048.0) as u16;
                        let qy = ((y as f32 + v[1]) * 2048.0) as u16;
                        let qz = ((z as f32 + v[2]) * 2048.0) as u16;
                        vtx[0..2].copy_from_slice(&qx.to_le_bytes());
                        vtx[2..4].copy_from_slice(&qy.to_le_bytes());
                        vtx[4..6].copy_from_slice(&qz.to_le_bytes());
                        vtx[6..10].copy_from_slice(&[180, 235, 170, 255]);
                        vtx[14] = light * 16;
                        vtx[15] = light * 16;
                        scratch.extend_from_slice(&vtx);
                    }
                }
            }
        }
    }
    let n = scratch.len() - start;
    if scratch.len() > 1 << 20 {
        scratch.truncate(0); // working buffer は溜め込まない
    }
    n
}

// ---- Pipe C: Rsift ----

struct RsiftPipe {
    shape_pool: InternPool<u64>,
    v_hits: u64,
    v_misses: u64,
    culler: FastEntityCuller,
    codec: RegionCodec,
    sec_cache: HashMap<(usize, usize, usize), u64>,
    pending_dirty: HashSet<(usize, usize)>,
    sample_acmr: (f32, f32),
    sample_weld: (usize, usize),
    last_indices: Option<Vec<u32>>, // シナリオ末に Tipsify 診断 (計測外)
}

/// セクション1枚のメッシュ生成: セクション・スコープの InternPool で頂点溶接
/// (位置を含む頂点のグローバルインターンはユニークだらけで無意味 — FerriteCore
/// の思想通り「位置を含まない形状」だけはグローバル形状プールに流す)。
/// 戻り値: (頂点bytes, 溶接後頂点数, 溶接indices, 頂点インターン hit/miss)
fn mesh_section_rsift(
    w: &PseudoWorld,
    sx: usize,
    sy: usize,
    sz: usize,
    shape_pool: &mut InternPool<u64>,
) -> (usize, usize, Vec<u32>, u64, u64) {
    let mut vpool = InternPool::<[u8; 12]>::new(); // セクション・スコープ
    let mut indices: Vec<u32> = Vec::new();
    for y in sy * 16..sy * 16 + 16 {
        for z in sz * 16..sz * 16 + 16 {
            for x in sx * 16..sx * 16 + 16 {
                let b = w.get(x as i64, y as i64, z as i64);
                if b == Block::Air {
                    continue;
                }
                let mut mask = 0u64;
                for (fi, (n, quad)) in FACES.iter().enumerate() {
                    let nb = w.get(x as i64 + n[0], y as i64 + n[1], z as i64 + n[2]);
                    if nb.opaque() {
                        continue;
                    }
                    mask |= 1u64 << fi;
                    let mut qidx = [0u32; 4];
                    for (vi, v) in quad.iter().enumerate() {
                        // セクションローカル座標 (量子化フォーマットの正しいドメイン。
                        // ワールド座標を渡すと u16 固定小数点が 64 で一周して縮退する)
                        let vx = (x & 15) as f32 + v[0];
                        let vy = (y & 15) as f32 + v[1];
                        let vz = (z & 15) as f32 + v[2];
                        let q = Quantized12ByteVertex::encode(
                            vx, vy, vz, n[0] as f32, n[1] as f32, n[2] as f32, 0.5, 0.5,
                        );
                        let raw: [u8; 12] =
                            <[u8; 12]>::try_from(bytemuck::bytes_of(&q)).unwrap();
                        qidx[vi] = vpool.intern(raw).0; // 溶接: 同一頂点は再利用
                    }
                    indices.extend_from_slice(&[
                        qidx[0], qidx[1], qidx[2], qidx[0], qidx[2], qidx[3],
                    ]);
                }
                if mask != 0 {
                    // FerriteCore 式: 位置に依らない「露出マスク x ブロック種」の形状を
                    // グローバルプールで正規化 (ユニーク数は ~ 種類 x 64 に収まる)
                    shape_pool.intern(((b as u64) << 6) | mask);
                }
            }
        }
    }
    let (vh, vm) = (vpool.hits, vpool.misses);
    let welded_verts = vpool.unique_count();
    // Tipsify はここでは走らせない (ポテトPC既定では opt-in 品質機能: 別途診断計測)
    (welded_verts * VERTEX_STRIDE_BYTES, welded_verts, indices, vh, vm)
}

fn run_pipe_a(w0: &PseudoWorld, sim: &SimOut) -> PipeOut {
    let mut w = w0.clone();
    let mut out = PipeOut::default();
    let mut scratch: Vec<[f32; 16]> = Vec::with_capacity(4096);
    let mut vtx_scratch: Vec<u8> = Vec::with_capacity(1 << 16);
    let mut pending_dirty: HashSet<(usize, usize)> = HashSet::new();
    for rec in &sim.frames {
        let t0 = Instant::now();
        for &(ux, uy, uz, nb) in &rec.edits {
            w.blocks[PseudoWorld::idx(ux, uy, uz)] = nb;
        }
        // モブ描画: カリング無しで全頭
        let mt = Instant::now();
        scratch.clear();
        for (id, pos, yaw) in &rec.view {
            render_mob(&mut scratch, &w, *pos, *yaw, *id as f32 + rec.cam[0]);
            out.mob_vtx_bytes += (MOB_VERTS * 32) as u64;
        }
        out.rendered_mobs += rec.view.len() as u64;
        out.mob_ms += mt.elapsed().as_secs_f64() * 1000.0;
        // 汚染セクション再メッシュ (全量再構築)
        let mst = Instant::now();
        for &(sx, sy, sz) in &rec.dirty_secs {
            out.mesh_vtx_bytes += mesh_section_vanilla(&w, sx, sy, sz, &mut vtx_scratch) as u64;
        }
        out.mesh_ms += mst.elapsed().as_secs_f64() * 1000.0;
        for &ch in &rec.dirty_chunks {
            pending_dirty.insert(ch);
        }
        if rec.autosave {
            let iot = Instant::now();
            let (_b, _ms) = zlib_save(&w, &pending_dirty);
            out.io_ms += iot.elapsed().as_secs_f64() * 1000.0;
            pending_dirty.clear();
        }
        out.frame_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    for m in &scratch {
        out.checksum += m[12] * 1e-6;
    }
    black_box(out.checksum);
    out.final_hash = w.world_hash();
    out
}

fn run_pipe_b(w0: &PseudoWorld, sim: &SimOut) -> PipeOut {
    let mut w = w0.clone();
    let mut out = PipeOut::default();
    let mut scratch: Vec<[f32; 16]> = Vec::with_capacity(4096);
    let mut sec_cache: HashMap<(usize, usize, usize), u64> = HashMap::new();
    let mut pending_dirty: HashSet<(usize, usize)> = HashSet::new();
    let mut vtx_scratch: Vec<u8> = Vec::with_capacity(1 << 16);
    let mut cum_dist: f32 = 0.0;
    for rec in &sim.frames {
        let t0 = Instant::now();
        for &(ux, uy, uz, nb) in &rec.edits {
            w.blocks[PseudoWorld::idx(ux, uy, uz)] = nb;
        }
        let ct = Instant::now();
        let mut visible_idx: Vec<usize> = Vec::with_capacity(rec.view.len());
        for (i, (_, pos, _)) in rec.view.iter().enumerate() {
            let d = [pos[0] - rec.cam[0], pos[1] - rec.cam[1], pos[2] - rec.cam[2]];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if dist > 96.0 {
                continue;
            }
            cum_dist += dist;
            let cos = (d[0] * rec.fwd[0] + d[1] * rec.fwd[1] + d[2] * rec.fwd[2]) / dist.max(1e-4);
            if dist > 2.0 && cos < 0.35 {
                continue;
            }
            visible_idx.push(i);
        }
        out.cull_ms += ct.elapsed().as_secs_f64() * 1000.0;
        let mt = Instant::now();
        scratch.clear();
        for &i in &visible_idx {
            let (id, pos, yaw) = &rec.view[i];
            render_mob(&mut scratch, &w, *pos, *yaw, *id as f32);
            out.mob_vtx_bytes += (MOB_VERTS * 20) as u64;
        }
        out.rendered_mobs += visible_idx.len() as u64;
        out.mob_ms += mt.elapsed().as_secs_f64() * 1000.0;
        let mst = Instant::now();
        for &(sx, sy, sz) in &rec.dirty_secs {
            let h = section_hash(&w, sx, sy, sz);
            if sec_cache.get(&(sx, sy, sz)) == Some(&h) {
                continue; // 内容不変 → 再メッシュスキップ
            }
            sec_cache.insert((sx, sy, sz), h);
            out.mesh_vtx_bytes += mesh_section_sodium(&w, sx, sy, sz, &mut vtx_scratch) as u64;
        }
        out.mesh_ms += mst.elapsed().as_secs_f64() * 1000.0;
        for &ch in &rec.dirty_chunks {
            pending_dirty.insert(ch);
        }
        if rec.autosave {
            let iot = Instant::now();
            let (_b, _ms) = zlib_save(&w, &pending_dirty);
            out.io_ms += iot.elapsed().as_secs_f64() * 1000.0;
            pending_dirty.clear();
        }
        out.frame_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    for m in &scratch {
        out.checksum += m[12] * 1e-6;
    }
    black_box(cum_dist);
    black_box(out.checksum);
    out.final_hash = w.world_hash();
    out
}

fn run_pipe_c(w0: &PseudoWorld, sim: &SimOut) -> PipeOut {
    let mut w = w0.clone();
    let mut out = PipeOut::default();
    let mut scratch: Vec<[f32; 16]> = Vec::with_capacity(4096);
    let mut pipe = RsiftPipe {
        shape_pool: InternPool::<u64>::new(),
        v_hits: 0,
        v_misses: 0,
        culler: FastEntityCuller::new(128, 96.0),
        codec: RegionCodec::new(CodecChoice::auto(2, false)),
        sec_cache: HashMap::new(),
        pending_dirty: HashSet::new(),
        sample_acmr: (0.0, 0.0),
        sample_weld: (0, 0),
        last_indices: None,
    };
    // 本番ポテトPC設定: 階層チャンクゲート + SIMDパケットDDA + ビットマスク
    pipe.culler.period_ticks = 10;
    pipe.culler.budget_per_tick = 128;
    let mut sampled = false;
    for rec in &sim.frames {
        let t0 = Instant::now();
        for &(ux, uy, uz, nb) in &rec.edits {
            w.blocks[PseudoWorld::idx(ux, uy, uz)] = nb;
        }
        let opaque = |x: i32, y: i32, z: i32| w.get(x as i64, y as i64, z as i64).opaque();
        // カリング (V2 高速階層＆SIMDパケットDDAオクルージョン)
        let ct = Instant::now();
        let targets: Vec<EntityTarget> = rec
            .view
            .iter()
            .map(|(id, pos, _)| EntityTarget {
                id: *id,
                min: [pos[0] - 0.4, pos[1] - 0.9, pos[2] - 0.4],
                max: [pos[0] + 0.4, pos[1] + 0.9, pos[2] + 0.4],
                is_block_entity: false,
            })
            .collect();
        pipe.culler.replace_targets_fast(&targets);
        let (mask, st) = pipe.culler.cull_fast_mask(rec.cam, rec.fwd, &opaque);
        out.rays_total += st.rays_cast as u64;
        out.far_total += st.skipped_far as u64;
        out.cull_ms += ct.elapsed().as_secs_f64() * 1000.0;
        let mt = Instant::now();
        scratch.clear();
        for (i, (id, pos, yaw)) in rec.view.iter().enumerate() {
            if FastEntityCuller::is_visible_bit(mask, i) {
                render_mob(&mut scratch, &w, *pos, *yaw, *id as f32);
                out.mob_vtx_bytes += (MOB_VERTS * VERTEX_STRIDE_BYTES) as u64;
                out.rendered_mobs += 1;
            }
        }
        out.mob_ms += mt.elapsed().as_secs_f64() * 1000.0;
        let mst = Instant::now();
        for &(sx, sy, sz) in &rec.dirty_secs {
            let h = section_hash(&w, sx, sy, sz);
            if pipe.sec_cache.get(&(sx, sy, sz)) == Some(&h) {
                continue;
            }
            pipe.sec_cache.insert((sx, sy, sz), h);
            let (bytes, wverts, indices, vh, vm) =
                mesh_section_rsift(&w, sx, sy, sz, &mut pipe.shape_pool);
            pipe.v_hits += vh;
            pipe.v_misses += vm;
            if pipe.sample_weld.1 == 0 {
                pipe.sample_weld = (indices.len() / 6 * 4, wverts);
            }
            pipe.last_indices = Some(indices);
            out.mesh_vtx_bytes += bytes as u64;
        }
        out.mesh_ms += mst.elapsed().as_secs_f64() * 1000.0;
        for &ch in &rec.dirty_chunks {
            pipe.pending_dirty.insert(ch);
        }
        if rec.autosave {
            let iot = Instant::now();
            for &(cx, cz) in &pipe.pending_dirty {
                pipe.codec.put_chunk(cx, cz, &chunk_raw(&w, cx, cz));
            }
            let f = pipe.codec.build_file();
            black_box(f.len());
            pipe.pending_dirty.clear();
            out.io_ms += iot.elapsed().as_secs_f64() * 1000.0;
        }
        out.frame_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    for m in &scratch {
        out.checksum += m[12] * 1e-6;
    }
    black_box(out.checksum);
    let vt_rate = if pipe.v_hits + pipe.v_misses > 0 {
        pipe.v_hits as f64 / (pipe.v_hits + pipe.v_misses) as f64 * 100.0
    } else {
        0.0
    };
    println!(
        "  [C] 溶接サンプル: 未溶接 {} 頂点 → 溶接後 {} 頂点 / Tipsify ACMR(溶接メッシュ): {:.3} → {:.3} / 頂点溶接 hit率 {:.1}% / 形状プール ユニーク {} 種",
        pipe.sample_weld.0,
        pipe.sample_weld.1,
        pipe.sample_acmr.0,
        pipe.sample_acmr.1,
        vt_rate,
        pipe.shape_pool.unique_count()
    );
    // Tipsify 診断 (opt-in 品質機能: GPU側の頂点シェーダ負荷を下げる機能。
    // headless CPU 計測ではコストしか出ないので 3 者対決からは外し、最後の
    // セクションで 1 回だけ実行して効果とコストを報告する)
    if let Some(idx) = &pipe.last_indices {
        let t_opt = Instant::now();
        let before = VertexCacheOptimizer::acmr(idx, 16);
        let reord = VertexCacheOptimizer::new(16).optimize(idx);
        let after = VertexCacheOptimizer::acmr(&reord, 16);
        black_box(&reord);
        pipe.sample_acmr = (before, after);
        println!(
            "  [C-diag] Tipsify (opt-in, 計測外): {} tris を {:?} で最適化 → ACMR {:.3} → {:.3}",
            idx.len() / 3,
            t_opt.elapsed(),
            before,
            after
        );
    }
    out.final_hash = w.world_hash();
    out
}

// ---------------- 集計 ----------------

fn pct_sorted(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64) * p).min(sorted.len() as f64 - 1.0) as usize;
    sorted[i]
}

fn report(name: &str, sim_ms: &[f64], out: &PipeOut) -> (f64, f64, f64) {
    let mut totals: Vec<f64> = out
        .frame_ms
        .iter()
        .zip(sim_ms)
        .map(|(r, s)| r + s)
        .collect();
    let steady_start = totals.len() / 2;
    let steady_avg = totals[steady_start..].iter().sum::<f64>()
        / (totals.len() - steady_start) as f64;
    totals.sort_by(|a, b| a.total_cmp(b));
    let avg = totals.iter().sum::<f64>() / totals.len() as f64;
    let p99 = pct_sorted(&totals, 0.99);
    let fps = 1000.0 / avg;
    let low1 = 1000.0 / p99;
    let steady_fps = 1000.0 / steady_avg;
    println!(
        "| {} | {:.2} ms | **{:.0}** | {:.0} | {:.0} | {:.1} | cull {:.2} / mob {:.2} / mesh {:.2} / io {:.2} ms |",
        name,
        avg,
        fps,
        low1,
        steady_fps,
        out.rendered_mobs as f64 / TICKS as f64,
        out.cull_ms / TICKS as f64,
        out.mob_ms / TICKS as f64,
        out.mesh_ms / TICKS as f64,
        out.io_ms / TICKS as f64
    );
    (fps, low1, steady_fps)
}

fn run_scenario(mob_count: usize, genesis: &PseudoWorld) -> [(f64, f64, f64); 3] {
    println!("\n# シナリオ: モブ {} 体 / {} tick", mob_count, TICKS);
    let t0 = Instant::now();
    let (sim, _final) = run_sim(genesis, mob_count, 0xD15E);
    let sim_avg = sim.sim_ms.iter().sum::<f64>() / TICKS as f64;
    println!(
        "シム録画完了: {:?} (総スポーン {} 体, sim avg {:.2} ms/tick → サーバ律速上限 TPS {:.0})",
        t0.elapsed(),
        sim.spawned_total,
        sim_avg,
        1000.0 / sim_avg
    );
    println!("| pipe | frame avg (sim+render) | 実効FPS | 1% low | 定常FPS(後半) | 平均描画モブ | render内訳/tick |");
    println!("|---|---|---|---|---|---|---|");
    let a = run_pipe_a(genesis, &sim);
    let b = run_pipe_b(genesis, &sim);
    let c = run_pipe_c(genesis, &sim);
    assert_eq!(a.final_hash, b.final_hash, "world diverged A vs B");
    assert_eq!(a.final_hash, c.final_hash, "world diverged A vs C");
    let ra = report("A Vanilla系", &sim.sim_ms, &a);
    let rb = report("B Sodium系", &sim.sim_ms, &b);
    let rc = report("C Rsift", &sim.sim_ms, &c);
    println!(
        "  内訳累計: A mesh {:.0}ms io {:.0}ms / B cull {:.2}ms mesh {:.0}ms io {:.0}ms / C cull {:.1}ms (rays {} far {}) mesh {:.0}ms io {:.0}ms",
        a.mesh_ms, a.io_ms,
        b.cull_ms, b.mesh_ms, b.io_ms,
        c.cull_ms, c.rays_total, c.far_total, c.mesh_ms, c.io_ms
    );
    println!(
        "  頂点データ量/tick: A {:.1} KB / B {:.1} KB / C {:.1} KB (モブ) | mesh累計: A {} B {} C {} bytes",
        a.mob_vtx_bytes as f64 / TICKS as f64 / 1024.0,
        b.mob_vtx_bytes as f64 / TICKS as f64 / 1024.0,
        c.mob_vtx_bytes as f64 / TICKS as f64 / 1024.0,
        a.mesh_vtx_bytes, b.mesh_vtx_bytes, c.mesh_vtx_bytes
    );
    [ra, rb, rc]
}

fn main() {
    println!("# pseudo-Minecraft LIVE: ヘッドレス実走 FPS 対決");
    println!("(同一ワールド {}x{}x{} / 同一モブ軌跡 / {} tick / release build)", WORLD_X, WORLD_Z, WORLD_Y, TICKS);
    let t0 = Instant::now();
    let genesis = PseudoWorld::generate(0x5253494654);
    println!("world gen: {:?}\n", t0.elapsed());

    // メモリ常駐: 240f x 6400 モブでも各 pipe の工作域 + 録画 ~ 百MB弱 (2GB 砂場で安全)
    let mut results: Vec<(usize, [(f64, f64, f64); 3])> = Vec::new();
    for &mobs in &[400usize, 1600, 3200, 6400] {
        let r = run_scenario(mobs, &genesis);
        results.push((mobs, r));
    }

    println!("\n# 🏁 総合: 実効FPS対決 (avg FPS / 1% low)");
    println!("| モブ数 | A Vanilla系 | B Sodium系 | C Rsift | B vs A | C vs A |");
    println!("|---|---|---|---|---|---|");
    for (mobs, r) in &results {
        let (ra, rb, rc) = (r[0], r[1], r[2]);
        println!(
            "| {} | {:.0} / {:.0} | {:.0} / {:.0} | **{:.0} / {:.0}** | {:.2}x | **{:.2}x** |",
            mobs,
            ra.0, ra.1,
            rb.0, rb.1,
            rc.0, rc.1,
            rb.0 / ra.0,
            rc.0 / ra.0
        );
    }
    println!("\nnote: シム時間は 3 者共通 (サーバ側モブAI/物理コスト)。render 差分がエンジン差。");
    println!("C は冷起動 (カリング catch-up) を含む。定常FPS 参照のこと。");
    println!("done.");
}
