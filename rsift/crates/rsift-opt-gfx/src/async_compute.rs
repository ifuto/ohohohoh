//! Async compute overlap scheduler for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): given a list of render passes tagged as running on
//! the graphics queue or the compute queue, compute the overlapped frame time
//! `max(graphics_total, compute_total)` that is achievable when the two queue
//! families run concurrently. Then provide a frame pipeliner that shifts a late
//! post-processing pass (e.g. bloom/SSR denoise) so that it executes during the
//! *next* frame's shadow-map pass, hiding its cost behind shadow rasterization.
//! This is a pure CPU planning primitive; the WGSL mirrors the same idea for the
//! GPU submit side. Quality is unchanged — only scheduling/overlap improves.
//!
//! 【wave 136 EJ-2 (2026-07-26)】消費者: `full_graph_wiring` の経済モデル配線
//! (作業量 proxy 写像→ plan/overlap → report 実フィールド 3 件)。proxy 値は
//! **実測 ms ではない**作業量の無量綱 proxy であり絶対値解釈は不可、
//! `saved_pct` (相対比率) のみ物理的意味を持つ (wiring 側 doc と二重明記)。
//! Vec3/Vec4 のローカル再定義は本モジュール・crate・テストの全消費者が
//! 存在しなかった (機械 grep: 使用箇所 0、planner 本体はスカラー演算のみ)
//! ため、新指令 §7「消費者なし一切禁止」により削除 (bloom/cas/fsr2 等の
//! 現用モジュール群だけが同形ローカル数学型の様式を維持)。
//! WGSL 側 `QueueTag` struct は未定義参照かつ未使用 (ワールド契約語彙ピン
//! 内の孤立定義) として棚卸し公表 — WGSL 変更は `gpu_runtime` のデバイス
//! コンパイル検証経路に影響しうるため wave 外スコープとして本 wave では
//! コード不変のまま構造公表のみ行う。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queue {
    Graphics,
    Compute,
}

#[derive(Clone, Debug)]
pub struct Pass {
    pub name: &'static str,
    pub queue: Queue,
    pub cost_ms: f32,
}

/// Plan overlapped execution of a frame's passes.
pub struct AsyncComputePlanner {
    pub passes: Vec<Pass>,
}

impl AsyncComputePlanner {
    pub fn new(passes: Vec<Pass>) -> Self {
        Self { passes }
    }

    /// Frame time assuming graphics and compute queues overlap freely.
    ///
    /// 【wave 136 EJ-2 捕捉 55】`passes` が空 (または全 cost=0) のとき
    /// `Iterator::sum::<f32>` の空集約は **-0.0 (0x80000000)** を返し、
    /// `(-0.0).max(-0.0)` は -0.0 を透過する (rustc 1.94.1 実測、zeroprobe
    /// で機械確定)。det 比較 (`to_bits`) の toolchain 依存を排除するため、
    /// 結果は `+0.0` へ正規化して返す (動機は決定性、数値は不変)。
    pub fn overlap_time(&self) -> f32 {
        let g: f32 = self
            .passes
            .iter()
            .filter(|p| p.queue == Queue::Graphics)
            .map(|p| p.cost_ms)
            .sum();
        let c: f32 = self
            .passes
            .iter()
            .filter(|p| p.queue == Queue::Compute)
            .map(|p| p.cost_ms)
            .sum();
        g.max(c).max(0.0)
    }

    /// Compare a naive schedule (post FX runs on the compute queue in the same
    /// frame) against a pipelined schedule where the post FX pass is overlapped
    /// with the next frame's shadow-map (graphics) pass. `post` is the cost of
    /// the post pass (compute), `shadow` the cost of a shadow pass (graphics)
    /// it can hide behind.
    pub fn plan(&self, post: f32, shadow: f32) -> PlanResult {
        let g: f32 = self
            .passes
            .iter()
            .filter(|p| p.queue == Queue::Graphics)
            .map(|p| p.cost_ms)
            .sum();
        let c: f32 = self
            .passes
            .iter()
            .filter(|p| p.queue == Queue::Compute)
            .map(|p| p.cost_ms)
            .sum();
        // Naive: post FX runs on the compute queue in the same frame, overlapping
        // the graphics work, so the compute total becomes `c + post`.
        let naive = g.max(c + post).max(0.0);
        // Pipelined: post FX is shifted so it executes behind the next frame's
        // shadow (graphics) pass, hiding up to `shadow` ms of its cost.
        let hidden = (c + (post - shadow).max(0.0)).max(0.0);
        let pipelined = g.max(hidden).max(0.0);
        // 【捕捉 55 対応】上記同様の ±0 正規化 (.max(0.0)) で det 比較に
        // toolchain 依存の -0.0 bits (0x80000000) が紛れ込む経路を構造排除。
        let saved = if naive > 1e-6 {
            (naive - pipelined) / naive
        } else {
            0.0
        };
        PlanResult {
            naive,
            pipelined,
            saved_pct: saved * 100.0,
        }
    }
}

pub struct PlanResult {
    pub naive: f32,
    pub pipelined: f32,
    pub saved_pct: f32,
}

pub fn wgsl_source() -> &'static str {
    ASYNC_COMPUTE_WGSL
}

pub const ASYNC_COMPUTE_WGSL: &str = include_str!("../shaders/async_compute.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlap_is_max_not_sum() {
        let p = AsyncComputePlanner::new(vec![
            Pass { name: "gbuffer", queue: Queue::Graphics, cost_ms: 6.0 },
            Pass { name: "shadows", queue: Queue::Graphics, cost_ms: 3.0 },
            Pass { name: "cull", queue: Queue::Compute, cost_ms: 5.0 },
        ]);
        // graphics = 9, compute = 5 -> overlapped = 9
        assert!((p.overlap_time() - 9.0).abs() < 1e-6);
    }
    #[test]
    fn pipeline_hides_post_behind_shadow() {
        let p = AsyncComputePlanner::new(vec![
            Pass { name: "gbuffer", queue: Queue::Graphics, cost_ms: 6.0 },
            Pass { name: "shadows", queue: Queue::Graphics, cost_ms: 3.0 },
            Pass { name: "cull", queue: Queue::Compute, cost_ms: 5.0 },
        ]);
        // post=4 (compute), shadow=3 (graphics)
        // naive = max(9, 5+4)=9 ; pipelined = max(9, 5 + max(0,4-3)=1)=9 -> no save here
        let r = p.plan(4.0, 3.0);
        assert!((r.naive - 9.0).abs() < 1e-6);
        // Now make compute bound so overlap > graphics:
        let p2 = AsyncComputePlanner::new(vec![
            Pass { name: "gbuffer", queue: Queue::Graphics, cost_ms: 6.0 },
        ]);
        // naive = max(6, 0+4)=6 ; pipelined = max(6, max(0,4-3)=1)=6 still graphics bound
        let r2 = p2.plan(4.0, 3.0);
        assert!((r2.naive - 6.0).abs() < 1e-6);
        // Make it compute bound:
        let p3 = AsyncComputePlanner::new(vec![
            Pass { name: "gbuffer", queue: Queue::Graphics, cost_ms: 4.0 },
        ]);
        // compute other = 0, post=4, shadow=3
        // naive = max(4, 4)=4 ; pipelined = max(4, max(0,4-3)=1)=4 -> still 4
        let r3 = p3.plan(4.0, 3.0);
        assert!((r3.naive - 4.0).abs() < 1e-6);
        // Genuinely compute bound with other compute work:
        let p4 = AsyncComputePlanner::new(vec![
            Pass { name: "gbuffer", queue: Queue::Graphics, cost_ms: 6.0 },
            Pass { name: "cull", queue: Queue::Compute, cost_ms: 5.0 },
        ]);
        // post=6 (compute), shadow=3 (graphics)
        // naive = max(6, 5+6)=11 ; pipelined = max(6, 5+max(0,6-3)=3)=8 -> save (11-8)/11 ~27%
        let r4 = p4.plan(6.0, 3.0);
        assert!((r4.naive - 11.0).abs() < 1e-6);
        assert!(r4.saved_pct > 20.0 && r4.saved_pct < 35.0);
    }
    #[test]
    fn saved_pct_exact_bits_rq_prederived() {
        // 【wave 136 EJ-2】rq 事前導出 golden (ej_planner_pin.rq):
        // g=9, c=5, post=6, shadow=3 → naive=11 (0x41300000)、hidden=5+max(3,0)=8、
        // pipelined=max(9,8)=9、saved=((11-9)/11)*100 の f32 演算列 = bits 0x4191745D。
        // 旧テストのレンジ `20<x<35` では本ケースは構造非拘束 (18.18 は範囲外) —
        // ここで厳密値に pin し、演算列 (naive-pipelined)/naive*100 の bit 契約を固定。
        let p = AsyncComputePlanner::new(vec![
            Pass { name: "gbuffer", queue: Queue::Graphics, cost_ms: 6.0 },
            Pass { name: "shadows", queue: Queue::Graphics, cost_ms: 3.0 },
            Pass { name: "cull", queue: Queue::Compute, cost_ms: 5.0 },
        ]);
        let r = p.plan(6.0, 3.0);
        assert_eq!(r.naive.to_bits(), 0x4130_0000, "naive=11 exact");
        assert_eq!(r.pipelined.to_bits(), 0x4110_0000, "pipelined=9 exact");
        assert_eq!(
            r.saved_pct.to_bits(),
            0x4191_745D,
            "saved_pct f32 bits (rq ej_planner_pin 導出、暗算禁止の機械検算)"
        );
        assert!(
            (r.saved_pct - 18.181818).abs() < 1e-6,
            "十進表示 18.181818%"
        );
    }
    #[test]
    fn naive_le_pipeline_invariant_and_shadow_zero_degenerate() {
        // 不変式: shadow ≥ post なら pipelined == overlap(=max(g,c)) 完全隠蔽、
        // saved_pct ≥ 0。shadow=0 なら pipelined == naive で saved == 0。
        let p = AsyncComputePlanner::new(vec![
            Pass {
                name: "gbuffer",
                queue: Queue::Graphics,
                cost_ms: 5.0,
            },
            Pass {
                name: "cull",
                queue: Queue::Compute,
                cost_ms: 3.0,
            },
        ]);
        let full_hide = p.plan(2.0, 5.0);
        assert_eq!(full_hide.pipelined.to_bits(), p.overlap_time().to_bits());
        let no_hide = p.plan(2.0, 0.0);
        assert_eq!(no_hide.pipelined.to_bits(), no_hide.naive.to_bits());
        assert_eq!(
            no_hide.saved_pct.to_bits(),
            0,
            "shadow=0 → 隠蔽なし → saved=0 exact"
        );
        assert!(p.plan(2.5, 1.0).saved_pct >= 0.0);
    }
    #[test]
    fn overlap_is_empty_passes_zero() {
        // 境界: pass 空 → overlap = 0、plan の saved は naive ≤ 1e-6 分岐 → 0 (静寂 NaN 否定)。
        let p = AsyncComputePlanner::new(vec![]);
        assert_eq!(p.overlap_time().to_bits(), 0);
        let r = p.plan(2.0, 3.0);
        assert_eq!(r.naive.to_bits(), 0x4000_0000, "naive = max(0, 0+2) = 2");
        // rq ej_empty_pin 導出: saved = (2-0)/2*100 = 100.0 = 0x42C80000
        // (手書き暗算で 0x41500000=13.0 を一時記入 → rq 機械検算で事前捕捉)。
        assert_eq!(
            r.saved_pct.to_bits(),
            0x42C8_0000,
            "saved = (2-0)/2*100 = 100 exact"
        );
    }
}
