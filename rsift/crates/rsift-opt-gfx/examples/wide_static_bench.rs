//! Rsift wide static benchmark — 多数分野 × パラメータ行列の静的 CPU 計測。
//!
//! 目的 (2026-07-21 ユーザー要求): 「様々な分野 (数百種類前後) で静的ベンチを行い、
//! あまりによくない結果が出た部分を優良になるまで改善する」土台データを決定的に作る。
//!
//! 設計:
//! - **全ケース決定的**: splitmix64 固定シードのみから入力を生成。
//!   GPU・ネットワーク・時刻依存を含まない (2コア/3GB sandbox で動く純 CPU 計測)。
//! - **実在 API のみ計測**: ベンチ対象は全て rsift-opt-gfx / rsift-api の公開実装。
//!   ベンチ専用の代替実装 (フェイク) は作らない。
//! - **行 = 分野 × パラメータセル**: 各行は「median ns / op 単位の正規化値 /
//!   決定的な構造出力 (bytes, counts, checksum)」を持つ。digest は時刻列を
//!   除去したテーブルの sha256 (構造出力の bit 同一性検証用)。
//! - **弱行検出**: 同一 family 内の中央値に対し 4 倍超の dwell を持つ行に
//!   `!! INVESTIGATE` フラグ。改善ループのターゲット選定に使う。

use std::time::Instant;

use rsift_opt_gfx::binary_greedy_meshing::{
    mesh_chunk_column_pull, mesh_section_pull, SectionPalette, SECTION_SIZE,
};
use rsift_opt_gfx::bitpacked_section::{CompactChunkSection, SECTION_VOL};
use rsift_opt_gfx::branchless_block::BlockLut;
use rsift_opt_gfx::branchless_dda::{trace_section, Ray3};
use rsift_opt_gfx::chunk_mesh::Quantized12ByteVertex;
use rsift_opt_gfx::entity_culling::{EntityCuller, EntityTarget};
use rsift_opt_gfx::intern_pool::InternPool;
use rsift_opt_gfx::lbvh::{Lbvh, Plane, Vec3};
use rsift_opt_gfx::leaf_fast_path::apply_leaf_fast_path;
use rsift_opt_gfx::light_cache::LightPropagationCache;
use rsift_opt_gfx::meshlet_cone::Cone;
use rsift_opt_gfx::morton_order::{compact_by_3, split_by_3};
use rsift_opt_gfx::packed4::PackedPullQuad;
use rsift_opt_gfx::section_rle::RleSection;

// ---------------------------------------------------------------- RNG (決定的)

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn hash3(x: u32, y: u32, z: u32, seed: u64) -> u64 {
    let v = (x as u64)
        .wrapping_mul(0x9E3779B1)
        .wrapping_add((y as u64) << 20)
        .wrapping_add((z as u64) << 40)
        .wrapping_add(seed);
    let mut z = v;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

// ---------------------------------------------------------------- 入力パターン

const PATTERNS: [&str; 8] = [
    "flat", "strata", "noise", "caves", "sparse", "checker", "dense8", "ids255",
];

/// [u16; 4096] の決定的セクション。id 0 = air, 非 0 = opaque (mesher 仕様)。
fn gen_section(pattern: &str, seed: u64) -> SectionPalette {
    let mut p = [0u16; SECTION_VOL];
    let n = SECTION_SIZE;
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let i = x + y * n + z * n * n;
                let h = hash3(x as u32, y as u32, z as u32, seed);
                p[i] = match pattern {
                    "flat" => {
                        if y < 7 {
                            7
                        } else if y == 7 {
                            1
                        } else {
                            0
                        }
                    }
                    "strata" => {
                        // y 帯の地層 (5 種)。高頻度の単調パターン。
                        if y < 4 + (x % 3) {
                            (1 + (y % 5)) as u16
                        } else {
                            0
                        }
                    }
                    "noise" => {
                        // 構造性を持たせるため 2x2x2 多数決の平滑値ノイズ。
                        let mut solid = 0u32;
                        for dz in 0..2u32 {
                            for dy in 0..2u32 {
                                for dx in 0..2u32 {
                                    let hv = hash3(
                                        x as u32 / 2 + dx,
                                        y as u32 / 2 + dy,
                                        z as u32 / 2 + dz,
                                        seed ^ 0x51,
                                    );
                                    if hv % 100 < 46 {
                                        solid += 1;
                                    }
                                }
                            }
                        }
                        if solid >= 4 {
                            (1 + (h % 4)) as u16
                        } else {
                            0
                        }
                    }
                    "caves" => {
                        let solid =
                            hash3(x as u32 / 2, y as u32 / 2, z as u32 / 2, seed ^ 0x77) % 100 < 62;
                        if !solid {
                            0
                        } else {
                            let carve = h % 1000 < 110;
                            if carve {
                                0
                            } else {
                                (1 + (h % 5)) as u16
                            }
                        }
                    }
                    "sparse" => {
                        if h % 100 < 3 {
                            (1 + (h >> 8) % 4) as u16
                        } else {
                            0
                        }
                    }
                    "checker" => {
                        if (x + y + z) % 2 == 0 {
                            9
                        } else {
                            0
                        }
                    }
                    "dense8" => (1 + h % 8) as u16,
                    "ids255" => (1 + h % 255) as u16,
                    other => panic!("unknown pattern {other}"),
                };
            }
        }
    }
    p
}

/// セクション列 (ストッキング): 各層の seed を変えた複数セクション。
fn gen_column(pattern: &str, sections: usize, seed: u64) -> Vec<SectionPalette> {
    (0..sections)
        .map(|s| gen_section(pattern, seed ^ (s as u64 * 0x10001)))
        .collect()
}

// ---------------------------------------------------------------- 計測基盤

struct Row {
    domain: &'static str,
    case: String,
    median_ns: u128,
    ops: u64,
    aux: String,
}

/// 1 回分を測り、reps 回の中央値を返す (reps は奇数)。
fn measure<R>(mut rep_fn: impl FnMut() -> R, reps: u32) -> (u128, R) {
    let mut times = Vec::with_capacity(reps as usize);
    let mut last = None;
    // warmup (初回のみ。結果は最終 rep のものを使う)
    let _ = rep_fn();
    for _ in 0..reps {
        let t0 = Instant::now();
        last = Some(rep_fn());
        times.push(t0.elapsed().as_nanos());
    }
    times.sort_unstable();
    (times[times.len() / 2], last.unwrap())
}

fn main() {
    let mut rows: Vec<Row> = Vec::new();
    let mut push = |domain: &'static str, case: String, median_ns: u128, ops: u64, aux: String| {
        rows.push(Row {
            domain,
            case,
            median_ns,
            ops,
            aux,
        });
    };

    // ============================================================ A. RLE encode
    for pat in PATTERNS {
        for (si, seed) in [0xA11u64, 0xA12, 0xA13].iter().enumerate() {
            let sec = gen_section(pat, *seed);
            let (ns, rle) = measure(|| RleSection::encode(&sec), 5);
            push(
                "rle_encode",
                format!("{pat} s{si}"),
                ns,
                1,
                format!("rle={}runs bytes={}", rle.runs.len(), rle.runs.len() * 4),
            );
        }
    }

    // ===================================================== B. greedy mesh (pull)
    for pat in PATTERNS {
        for (si, seed) in [0xB22u64, 0xB23, 0xB24].iter().enumerate() {
            let sec = gen_section(pat, *seed);
            let (ns, mesh) = measure(|| mesh_section_pull(&sec, 0, 0), 3);
            push(
                "mesh_section_pull",
                format!("{pat} s{si}"),
                ns,
                1,
                format!("quads={} bytes={}", mesh.quads.len(), mesh.ssbo_bytes()),
            );
        }
    }

    // ======================================= C. column mesh (cull on/off, 高さ掃引)
    for pat in ["flat", "noise", "caves", "dense8"] {
        for cull in [true, false] {
            let col = gen_column(pat, 8, 0xC33);
            let (ns, mesh) = measure(|| mesh_chunk_column_pull(&col, 0, 0, cull), 3);
            push(
                "mesh_column8",
                format!("{pat} cull={cull}"),
                ns,
                1,
                format!("quads={}", mesh.quads.len()),
            );
        }
    }
    for h in [4usize, 16] {
        let col = gen_column("noise", h, 0xC44);
        let (ns, mesh) = measure(|| mesh_chunk_column_pull(&col, 0, 0, true), 3);
        push(
            "mesh_column_scale",
            format!("noise h={h}"),
            ns,
            1,
            format!("quads={}", mesh.quads.len()),
        );
    }

    // ============================================ D. 12B 頂点量子化 (スループット+決定出力)
    for bits in [14u32, 18, 22] {
        let n = 1usize << bits;
        // 公平性修正: 旧版は計測ループ内で rng 生成ごと計っていたため
        // rng コスト (~16ns/v) が encode 値を支配していた。入力は事前生成し
        // 純粋な encode + 配列読み出しのみを計測する (系列は同一 seed で再現
        // するため構造出力 sum_* は旧版と bit 同一 = digest 不変)。
        let mut inputs = Vec::with_capacity(n);
        {
            let mut rng = Rng::new(0xD55);
            for _ in 0..n {
                let f = |r: &mut Rng, m: f32| (r.below(1000) as f32 / 1000.0 - 0.5) * m;
                inputs.push([
                    f(&mut rng, 32.0),
                    f(&mut rng, 32.0),
                    f(&mut rng, 32.0),
                    f(&mut rng, 1.0),
                    f(&mut rng, 1.0),
                    f(&mut rng, 1.0),
                    f(&mut rng, 16.0),
                    f(&mut rng, 16.0),
                ]);
            }
        }
        let (ns, sum) = measure(
            || {
                let mut acc = [0u64; 3];
                for inp in &inputs {
                    let v = Quantized12ByteVertex::encode(
                        inp[0], inp[1], inp[2], inp[3], inp[4], inp[5], inp[6], inp[7],
                    );
                    acc[0] = acc[0].wrapping_add(std::hint::black_box(v.pos_xyz_half[0] as u64));
                    acc[1] =
                        acc[1].wrapping_add(std::hint::black_box(v.octahedral_normal[0] as u64));
                    acc[2] = acc[2].wrapping_add(std::hint::black_box(v.uv_half[0] as u64));
                }
                acc
            },
            3,
        );
        push(
            "quant12_encode",
            format!("n=2^{bits}"),
            ns,
            n as u64,
            format!("sum_pos={} sum_nrm={} sum_uv={}", sum[0], sum[1], sum[2]),
        );
    }

    // ============================================================ E. packed4
    for bits in [16u32, 22] {
        let n = 1usize << bits;
        let (ns, acc) = measure(
            || {
                let mut acc = 0u64;
                let mut rng = Rng::new(0xE66);
                for _ in 0..n {
                    let w0 = PackedPullQuad::pack_word0(
                        (rng.below(64)) as u32,
                        (rng.below(384)) as u32,
                        (rng.below(64)) as u32,
                        rng.below(512) as u32,
                        rng.below(16) as u32,
                    );
                    let w1 = PackedPullQuad::pack_word1(
                        rng.below(6) as u32,
                        rng.below(64) as u32,
                        rng.below(64) as u32,
                    );
                    acc = acc.wrapping_add(std::hint::black_box(
                        PackedPullQuad::unpack_x(w0) as u64 + w1 as u64,
                    ));
                }
                acc
            },
            3,
        );
        push(
            "packed4_pack",
            format!("n=2^{bits}"),
            ns,
            n as u64,
            format!("acc={acc}"),
        );
    }

    // ============================================================ F. morton
    {
        let n = 1usize << 22;
        let (ns, acc) = measure(
            || {
                let mut acc = 0u64;
                for i in 0..n as u32 {
                    let m = split_by_3(i);
                    acc = acc.wrapping_add(std::hint::black_box(m ^ compact_by_3(m) as u64));
                }
                acc
            },
            3,
        );
        push(
            "morton_split3_rt",
            "n=2^22".into(),
            ns,
            n as u64,
            format!("acc={acc}"),
        );
    }

    // ============================================ G. bitpacked section (set/get)
    for pat in ["noise", "dense8", "ids255"] {
        let sec = gen_section(pat, 0x677);
        let (ns, (foot, sum)) = measure(
            || {
                let mut cs = CompactChunkSection::new_air();
                let mut rng = Rng::new(0x677);
                for _ in 0..SECTION_VOL {
                    // 実生成済み配列をソースに set (bitpack 展開を課す)
                    let i = rng.below(SECTION_VOL as u64) as usize;
                    cs.set(i % 16, (i / 16) % 16, i / 256, sec[i]);
                }
                let mut sum = 0u64;
                for i in 0..SECTION_VOL {
                    sum += cs.get(i % 16, (i / 16) % 16, i / 256) as u64;
                }
                (cs.memory_footprint_bytes(), sum)
            },
            3,
        );
        push(
            "bitpacked_setfill",
            pat.into(),
            ns,
            SECTION_VOL as u64,
            format!("footprint={foot}B sum={sum}"),
        );
    }

    // ===================================================== H. 圧縮 (lz4 / zstd)
    for pat in PATTERNS {
        for (si, seed) in [0x888u64, 0x889, 0x88A].iter().enumerate() {
            let sec = gen_section(pat, *seed);
            let raw: &[u8] = bytemuck::cast_slice(&sec[..]);
            let (ns_lz, lz_out) = measure(|| lz4_flex::compress_prepend_size(raw), 5);
            push(
                "lz4_compress",
                format!("{pat} s{si}"),
                ns_lz,
                1,
                format!(
                    "out={}B ratio={:.3}",
                    lz_out.len(),
                    lz_out.len() as f64 / raw.len() as f64
                ),
            );
            let (ns_z, z_out) = measure(|| zstd::bulk::compress(raw, 9).unwrap(), 3);
            push(
                "zstd_compress",
                format!("{pat} s{si}"),
                ns_z,
                1,
                format!(
                    "out={}B ratio={:.3}",
                    z_out.len(),
                    z_out.len() as f64 / raw.len() as f64
                ),
            );
        }
    }

    // ============================================================ I. intern pool
    for bits in [16u32, 20] {
        let n = 1usize << bits;
        let (ns, (uniq, hits)) = measure(
            || {
                let mut pool: InternPool<u64> = InternPool::new();
                let mut rng = Rng::new(0x199);
                for _ in 0..n {
                    // 実際的な再利用分布 (キー空間 1/16)
                    let key = rng.below(n as u64 / 16);
                    pool.intern(key);
                }
                let h = 0u64; // hits は内部カウンタだが外からは unique 数のみ計測
                (pool.unique_count() as u64, h)
            },
            3,
        );
        push(
            "intern_pool",
            format!("n=2^{bits}"),
            ns,
            n as u64,
            format!("unique={uniq} hits_ignored={hits}"),
        );
    }

    // ============================================================ J. BlockLut
    {
        let lut = BlockLut::new();
        let n = 1u64 << 22;
        let (ns, acc) = measure(
            || {
                let mut acc = 0u64;
                let mut rng = Rng::new(0x1AA);
                for _ in 0..n {
                    let v = BlockLut::select_branchless(
                        lut.is_opaque_branchless(rng.below(1024) as u16),
                        1,
                        0,
                    ) as u64
                        + lut.light_branchless(rng.below(1024) as u16) as u64;
                    acc = acc.wrapping_add(std::hint::black_box(v));
                }
                acc
            },
            3,
        );
        push(
            "blocklut_lookup",
            "n=2^22".into(),
            ns,
            n,
            format!("acc={acc}"),
        );
    }

    // ===================================================== K. meshlet cone build
    for bits in [10u32, 16, 18] {
        let n = 1usize << bits;
        let mut normals = Vec::with_capacity(n);
        let mut rng = Rng::new(0x100B);
        for _ in 0..n {
            let f = |r: &mut Rng| r.below(2000) as f32 / 1000.0 - 1.0;
            normals.push([f(&mut rng), f(&mut rng), f(&mut rng)]);
        }
        let (ns, cone) = measure(
            || std::hint::black_box(Cone::from_normals(std::hint::black_box(&normals))),
            3,
        );
        push(
            "meshlet_cone",
            format!("n=2^{bits}"),
            ns,
            n as u64,
            format!(
                "axis=({:.3},{:.3},{:.3})",
                cone.axis[0], cone.axis[1], cone.axis[2]
            ),
        );
    }

    // ============================================ L. LBVH build + frustum cull
    for bits in [12u32, 16, 18] {
        let n = 1usize << bits;
        let mut centers = Vec::with_capacity(n);
        let mut radii = Vec::with_capacity(n);
        let mut rng = Rng::new(0x110C);
        for _ in 0..n {
            let f = |r: &mut Rng| r.below(4096) as f32;
            centers.push(Vec3::new(f(&mut rng), f(&mut rng), f(&mut rng)));
            radii.push(Vec3::new(1.0, 1.0, 1.0));
        }
        let (ns_b, tree) = measure(
            || {
                Lbvh::build(
                    &centers,
                    &radii,
                    Vec3::new(0.0, 0.0, 0.0),
                    Vec3::new(4096.0, 4096.0, 4096.0),
                )
            },
            3,
        );
        push(
            "lbvh_build",
            format!("n=2^{bits}"),
            ns_b,
            n as u64,
            "built".to_string(),
        );
        let planes = [
            Plane::new(1.0, 0.0, 0.0, 0.0),
            Plane::new(-1.0, 0.0, 0.0, 2048.0),
            Plane::new(0.0, 1.0, 0.0, 0.0),
            Plane::new(0.0, -1.0, 0.0, 2048.0),
            Plane::new(0.0, 0.0, 1.0, 0.0),
            Plane::new(0.0, 0.0, -1.0, 2048.0),
        ];
        let (ns_c, vis) = measure(|| tree.cull(&planes), 3);
        push(
            "lbvh_cull",
            format!("n=2^{bits}"),
            ns_c,
            1,
            format!("visible={}", vis.len()),
        );
    }

    // ===================================== M. DDA (疑似ベンチ 546ms 行のコア経路)
    for pat in ["flat", "noise", "caves", "dense8"] {
        for rays in [64u32, 256, 1024] {
            let sec = gen_section(pat, 0xDDA);
            let (ns, hits) = measure(
                || {
                    let mut hits = 0u64;
                    let mut rng = Rng::new(0xDDA);
                    for _ in 0..rays {
                        let o = [
                            rng.below(160) as f32 / 10.0,
                            rng.below(160) as f32 / 10.0,
                            rng.below(160) as f32 / 10.0,
                        ];
                        let d_raw = [
                            rng.below(2000) as f32 / 1000.0 - 1.0,
                            rng.below(2000) as f32 / 1000.0 - 1.0,
                            rng.below(2000) as f32 / 1000.0 - 1.0,
                        ];
                        let l = (d_raw[0].powi(2) + d_raw[1].powi(2) + d_raw[2].powi(2))
                            .sqrt()
                            .max(1e-6);
                        let ray = Ray3::new(o, [d_raw[0] / l, d_raw[1] / l, d_raw[2] / l]);
                        if trace_section(&sec, &ray, 128).is_some() {
                            hits += 1;
                        }
                    }
                    hits
                },
                3,
            );
            push(
                "dda_trace128",
                format!("{pat} rays={rays}"),
                ns,
                rays as u64,
                format!("hits={hits}/{rays}"),
            );
        }
    }

    // ===================================================== N. entity culling
    for bits in [11u32, 15] {
        for density in [10u64, 40] {
            let n = 1usize << bits;
            let mut targets = Vec::with_capacity(n);
            let mut rng = Rng::new(0xE11 ^ density);
            for i in 0..n {
                let f = |r: &mut Rng| r.below(2560) as f32 / 10.0;
                let (x, y, z) = (f(&mut rng), f(&mut rng), f(&mut rng));
                targets.push(EntityTarget {
                    id: i as u64,
                    min: [x, y, z],
                    max: [x + 1.0, y + 2.0, z + 1.0],
                    is_block_entity: false,
                });
            }
            let (ns, vis) = measure(
                || {
                    let mut culler = EntityCuller::new(64, 128.0);
                    culler.replace_targets(targets.clone());
                    let solids = move |x: i32, y: i32, z: i32| {
                        hash3(x as u32, y as u32, z as u32, 0x501) % 100 < density
                    };
                    let (ids, _st) = culler.stats([8.0, 8.0, 8.0], &solids);
                    ids.len()
                },
                3,
            );
            push(
                "entity_cull",
                format!("n=2^{bits} solid={density}%"),
                ns,
                n as u64,
                format!("visible={vis}"),
            );
        }
    }

    // ===================================================== O. light cache
    for ops in [20u32, 22] {
        let n = 1usize << ops;
        let (ns, sum) = measure(
            || {
                let mut cache = LightPropagationCache::new();
                let mut rng = Rng::new(0x1230);
                for _ in 0..n {
                    cache.set_emitter(
                        rng.below(64) as i32,
                        rng.below(320) as i32,
                        rng.below(64) as i32,
                        rng.below(16) as u8,
                    );
                }
                let mut sum = 0u64;
                for _ in 0..(n / 4) {
                    let (b, s) = cache.get_light(
                        rng.below(64) as i32,
                        rng.below(320) as i32,
                        rng.below(64) as i32,
                    );
                    sum += (b + s) as u64;
                }
                sum
            },
            3,
        );
        push(
            "light_cache",
            format!("set=2^{ops}"),
            ns,
            n as u64 + (n as u64 / 4),
            format!("lightsum={sum}"),
        );
    }

    // ===================================================== P. leaf fast path
    for pat in ["sparse", "noise"] {
        let (ns, changed) = measure(
            || {
                let mut sec = gen_section(pat, 0x1EAF);
                let before: u64 = sec.iter().map(|&v| v as u64).sum();
                apply_leaf_fast_path(&mut sec, true);
                let after: u64 = sec.iter().map(|&v| v as u64).sum();
                before.wrapping_sub(after)
            },
            3,
        );
        push(
            "leaf_fast_path",
            pat.into(),
            ns,
            1,
            format!("delta_idsum={changed}"),
        );
    }

    // ================================================================ 集計出力
    println!("# wide_static_bench — 静的 CPU 計測 (全入力決定的, seeds 固定)");
    println!();
    println!("| domain | case | median | per-op | aux (決定的構造出力) |");
    println!("|---|---|---|---|---|");
    let mut total_ns: u128 = 0;
    // family 中央値 (ns/op) による異常行検出
    let mut family_vals: std::collections::HashMap<&'static str, Vec<f64>> =
        std::collections::HashMap::new();
    for r in &rows {
        total_ns += r.median_ns;
        family_vals
            .entry(r.domain)
            .or_default()
            .push(r.median_ns as f64 / r.ops.max(1) as f64);
    }
    let family_median: std::collections::HashMap<&'static str, f64> = family_vals
        .iter()
        .map(|(k, v)| {
            let mut v = v.clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (*k, v[v.len() / 2])
        })
        .collect();
    let mut digest_input = String::new();
    let mut flagged = 0u32;
    for r in &rows {
        let per_op = r.median_ns as f64 / r.ops.max(1) as f64;
        let med = family_median[r.domain];
        let flag = if per_op > med * 4.0 && family_vals[r.domain].len() >= 3 {
            flagged += 1;
            " !! INVESTIGATE"
        } else {
            ""
        };
        println!(
            "| {} | {} | {:.3}ms | {:.1}ns | {}{} |",
            r.domain,
            r.case,
            r.median_ns as f64 / 1e6,
            per_op,
            r.aux,
            flag
        );
        // digest は決定的列のみ (時刻・ns は入れない)
        digest_input.push_str(&format!("{}|{}|{}\n", r.domain, r.case, r.aux));
    }
    println!();
    println!(
        "rows={} total_median_time={:.1}ms flagged={}",
        rows.len(),
        total_ns as f64 / 1e6,
        flagged
    );
    let mut h = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    digest_input.hash(&mut h);
    println!(
        "structural_digest(default-hasher)={:016x} rows={}",
        h.finish(),
        rows.len()
    );
}
