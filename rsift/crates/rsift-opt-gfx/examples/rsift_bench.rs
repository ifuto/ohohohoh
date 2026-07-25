//! `rsift_bench` — Wave-5 モジュール群の代表ワークロードを HDR 計測する CLI。
//!
//! 使い方（ユーザー環境・Windows）:
//! ```bat
//! cargo run --release -p rsift-opt-gfx --example rsift_bench -- --medium
//! ```
//! 出力は CSV（`bench_out/rsift_bench_<ts>.csv`）+ markdown 表を stdout に出す。
//!
//! `--light`（短時間・CI / ノート） / `--medium`（標準） / `--heavy`（深夜に流す）。

use rsift_opt_gfx::bench_harness::{csv_header, run_timed, BenchResult};
use rsift_opt_gfx::gpu_arena::{ArenaHandle, GpuArena};
use rsift_opt_gfx::intern_pool::ShapeCache;
use rsift_opt_gfx::mesh_compactor::{
    compact_draws, CompactPolicy, DrawCandidate, FrustumPlanes, IndirectDrawCmd,
};
use rsift_opt_gfx::palette_pack::{PackedSection, SECTION_VOLUME};
use rsift_opt_gfx::quality_governor::{GovernorConfig, QualityGovernor};
use rsift_opt_gfx::region_zstd::{CodecChoice, RegionCodec};
use rsift_opt_gfx::stutter_guard::{FrameArena, TimeSlice};
use std::time::Instant;

fn fract(seed: u64) -> f32 {
    let mut s = seed;
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    (s & 0xFFFFFF) as f32 / 16_777_216.0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.iter().position(|a| a == "--medium" || a == "--heavy" || a == "--light")
        .map(|i| args[i].as_str())
        .unwrap_or("--medium");
    let (ms_per, max_iters) = match mode {
        "--light" => (80u64, 400u64),
        "--heavy" => (1_500u64, 30_000u64),
        _ => (350u64, 4_000u64),
    };
    println!("# rsift_bench mode={mode} (per-bench: {ms_per}ms / cap {max_iters})");
    println!("{}", csv_header());
    let mut results: Vec<BenchResult> = Vec::new();

    // ---- 1. GpuArena: ランダム割当・解放パターン --------------------------------
    results.push(run_timed("gpu_arena.alloc_free_mix", ms_per, max_iters, |tick| {
        let mut arena = GpuArena::new(64 << 20, 16);
        let mut live: Vec<ArenaHandle> = Vec::new();
        let mut s = *tick + 0xDEAD;
        for _ in 0..1024 {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            if live.len() < 200 && (s >> 63) == 0 {
                if let Some(h) = arena.alloc(256 + (s % 65_536)) {
                    live.push(h);
                }
            } else if !live.is_empty() {
                let i = (s as usize) % live.len();
                arena.free(live.swap_remove(i));
            }
        }
        std::hint::black_box(arena.used_bytes());
        *tick += 1;
    }));

    // ---- 2. mesh_compactor: 10k セクションのフルカリング+圧縮 --------------------
    results.push(run_timed("mesh_compactor.10k_sections", ms_per, max_iters, |tick| {
        let m = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let f = FrustumPlanes::from_view_proj(&m);
        let cands: Vec<DrawCandidate> = (0u64..10_000)
            .map(|i| {
                let a = fract(i.wrapping_mul(7919) + *tick);
                let b = fract(i.wrapping_mul(104729) + 3);
                let c = fract(i.wrapping_mul(15485863) + 7);
                DrawCandidate {
                    center: [(a - 0.5) * 400.0, (b - 0.5) * 64.0, (c - 0.5) * 400.0 + 0.5],
                    half_extents: [8.0, 8.0, 8.0],
                    vertex_count: if i % 7 == 0 { 0 } else { 72 + (i % 1000) as u32 },
                    first_vertex: (i * 128) as u32,
                    visible_prev: i % 11 != 0,
                    pass_key: (i % 4) as u16,
                }
            })
            .collect();
        let mut out: Vec<IndirectDrawCmd> = Vec::new();
        let n = compact_draws(
            &cands,
            &f,
            [0.0, 0.0, 0.0],
            &CompactPolicy::default(),
            &mut out,
        );
        std::hint::black_box(n);
    }));

    // ---- 3. palette_pack: 4096 ブロックの pack+random access ----------------------
    results.push(run_timed("palette_pack.encode4096", ms_per, max_iters, |tick| {
        let mut blocks = [0u16; SECTION_VOLUME];
        let mut s = *tick + 1;
        for (i, b) in blocks.iter_mut().enumerate() {
            s = s.wrapping_mul(2862933555777941757).wrapping_add(3037000493);
            if (s >> 61) == 0 {
                *b = (s % 64) as u16;
            }
            let _ = i;
        }
        let sec = PackedSection::from_blocks(&blocks);
        // ランダムアクセスも測る
        let mut acc = 0u64;
        for i in (0..SECTION_VOLUME).step_by(17) {
            acc += sec.get(i & 15, (i >> 8) & 15, (i >> 4) & 15) as u64;
        }
        std::hint::black_box(acc);
    }));

    // ---- 4. intern_pool: フェライトコア式形状キャッシュ -----------------------------
    results.push(run_timed("intern_pool.shape_cache_10k", ms_per, max_iters, |tick| {
        let mut sc = ShapeCache::new(0.001);
        // 10000 状態だがユニーク形状は 20 種だけ（典型的な重複状況）
        for i in 0..10_000u64 {
            let k = (i.wrapping_mul(2654435761) + *tick) % 20;
            let h = 1.0 + (k as f32) * 0.125;
            let aabb = [[0.0, 0.0, 0.0, 1.0, h, 1.0]];
            let id = sc.intern_shape(&aabb);
            std::hint::black_box(id);
        }
        std::hint::black_box(sc.pool().unique_count());
    }));

    // ---- 5. stutter_guard: フレームアリーナ alloc + reset -------------------------
    results.push(run_timed("stutter_guard.frame_sim_4096alloc", ms_per, max_iters, |tick| {
        let arena = FrameArena::new(1 << 20);
        let mut acc = 0u64;
        for i in 0..4096 {
            if let Some(p) = arena.alloc_pod_slice::<u64>((i % 24) + 1) {
                acc = acc.wrapping_add(p.as_ptr() as u64);
            }
        }
        arena.reset();
        std::hint::black_box(acc);
    }));

    // ---- 6. quality_governor: 重い区間→降段→軽くなる→戻る の 1 シナリオ -----------
    results.push(run_timed("quality_governor.scenario", ms_per, max_iters, |tick| {
        let mut g = QualityGovernor::new(GovernorConfig::default());
        let mut frames = 0u64;
        for i in 0..400u32 {
            let us = if (80..180).contains(&i) { 55_000 } else { 9_000 };
            let _ = g.observe(us);
            frames += 1;
        }
        std::hint::black_box(g.quality_score());
    }));

    // ---- 7. region_zstd: 1 チャンク (64KiB) 圧縮+展開 ------------------------------
    results.push(run_timed("region_zstd.enc_dec_64k", ms_per, max_iters, |tick| {
        let mut raw = vec![0u8; 64 * 1024];
        for i in 0..2048 {
            raw[i] = ((i as u32).wrapping_mul(2654435761) >> 24) as u8;
        }
        raw[4096] = (*tick & 0xFF) as u8;
        let mut rc = RegionCodec::new(CodecChoice::ZstdBalanced);
        rc.put_chunk(1, 1, &raw);
        let back = rc.get_chunk(1, 1).unwrap();
        std::hint::black_box(back.len());
    }));

    // ---- 8. AO half-res pipeline (128x72 でワークロード固定) -----------------------
    results.push(run_timed("deinterleave_ao.128x72_scene", ms_per, max_iters, |tick| {
        let w = 128;
        let h = 72;
        let mut d = vec![1.0f32; w * h];
        // 地形っぽい深度
        for y in 0..h {
            for x in 0..w {
                let u = x as f32 / w as f32;
                let v = y as f32 / h as f32;
                d[y * w + x] = 1.0 + 0.3 * (u * 12.0).sin() * (v * 9.0).cos()
                    + fract((x * 73856093 ^ y * 19349663) as u64 + *tick) * 0.02;
            }
        }
        let halves = rsift_opt_gfx::deinterleave_ao::deinterleaved_ao(
            &d,
            w,
            h,
            &rsift_opt_gfx::deinterleave_ao::AoParams::default(),
        );
        let full = rsift_opt_gfx::deinterleave_ao::reinterleave_denoise(
            &halves,
            &d,
            w,
            h,
            &rsift_opt_gfx::deinterleave_ao::AoParams::default(),
        );
        std::hint::black_box(full[full.len() / 2]);
    }));

    // ---- 9. TimeSlice のスループット ----------------------------------------------
    results.push(run_timed("time_slice.drain_budget8ms", ms_per, max_iters, |tick| {
        let mut ts: TimeSlice<u32> = TimeSlice::new(8_000);
        for i in 0..5_000 {
            ts.push(i);
        }
        let mut acc = 0u64;
        // 1 フレーム: 8ms 分を処理
        ts.run_frame(|v| {
            acc += *v as u64;
            2 // 2us/個で 4000 個/フレーム上限
        });
        std::hint::black_box(acc + ts.backlog_len() as u64);
    }));

    // ---- 出力 ---------------------------------------------------------------------
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let dir = std::path::Path::new("bench_out");
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(format!("rsift_bench_{ts}.csv"));
    let mut csv = String::new();
    csv.push_str(csv_header());
    csv.push('\n');
    csv.push_str("# rsift wave-5 module bench (single process, HDR log-resolution)\n");
    let mut md = String::new();
    md.push_str("| bench | iters | mean | p50 | p95 | p99 | throughput |\n|---|---|---|---|---|---|---|\n");
    for r in &results {
        csv.push_str(&r.to_csv_row());
        csv.push('\n');
        md.push_str(&r.to_markdown_row());
        md.push('\n');
    }
    // per-bench の分布も markdown に残す (Hdr::ascii の実消費者 — wave 104 DD-5 で配線)。
    // p50/95/99 だけでは見えないバケット偏り (bimodal や tail 厚み) を report から判読可能にする。
    md.push_str("\n#### per-bench bucket histogram (log-scale buckets)\n\n");
    for r in &results {
        md.push_str(&format!("`{}`\n```\n{}```\n\n", r.name, r.hdr.ascii(40)));
    }
    std::fs::write(&path, &csv).expect("csv write");
    println!("\n--- markdown ---\n{}", md);
    println!("csv: {}", path.display());

    // 目標フレーム16.6ms で「1フレームあたり何回回せるか」も併記
    println!("\n--- per-frame capacity at 16.6ms ---");
    for r in &results {
        let mean = r.hdr.mean_us();
        if mean > 0.1 {
            println!(
                "{:<38} {:>10.2} calls/frame",
                r.name,
                16_666.0f64 / mean
            );
        }
    }
}
