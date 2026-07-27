//! Subgroup / wavefront operation helpers for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): a CPU simulation of the wave ops modern GPUs expose
//! (reduce-add across a fixed wave, and a ballot mask). These back the
//! GPU-driven culling / prefix-sum passes — they let one invocation see its
//! neighbours' data, cutting bandwidth on integrated GPUs.
//!
//! 誠実注記 (wave 140 EN-3):
//! 1. `subgroup_reduce_add` は f32 の**非結合**加算を wave 内 index 順に
//!    逐次実行する — GPU 実 subgroup reduce の加算順序は実装依存 (木構造
//!    reduce 等) で、CPU シミュレーションの値とは一般に bit 一致しない
//!    (rq 機械導出: [1e20; 32] の逐次和 0x632D78EB は一括乗算
//!    0x632D78EC と 1 ulp 差異、1e20 の wave では 1.0×31 個加えても
//!    ulp 未満で全消失 0x60AD78EC 不変)。本値は順序確定の参照値であり、
//!    GPU 実行結果の同一ビット保証ではない。
//! 2. `subgroup_ballot` は **`j >= 64` の lane を静寂に切り捨てる**
//!    (u64 mask の写像域外)。GPU wave は ≤64 lane (AMD wave64) のため
//!    写像としては整合するが、CPU シミュレーション固有の切捨てであり、
//!    wiring 実引数 (emissive lights 数) が 64 を超えると上位 lane の
//!    true は mask に現れない (pin 済)。なお現行 wiring 供給は
//!    emissive cap 32 のため切捨て経路は実運用で未到達 (wave 140)。
//! 3. `WAVE_WIDTH = 32` 固定 — AMD wave64 等の別幅は別定数を要する。
//! 4. 旧 `Vec3`/`Vec4` (+ Add/Sub/Mul trait 実装) は crate+workspace
//!    全体で消費者完全ゼロ (新指令 §7 違反) の完全装飾だったため削除
//!    (EJ-2 async_compute Vec3/Vec4 削除と同型、grep 機械確定)。

/// Width of a hardware "wave"/"subgroup" (e.g. 32 on NVIDIA, 32/64 on AMD).
pub const WAVE_WIDTH: usize = 32;

/// Per-lane result of a wave-wide reduce-add: every lane in a wave gets the
/// sum of all lanes in that wave.
pub fn subgroup_reduce_add(values: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; values.len()];
    let mut i = 0usize;
    while i < values.len() {
        let wave_start = (i / WAVE_WIDTH) * WAVE_WIDTH;
        let wave_end = (((i / WAVE_WIDTH) + 1) * WAVE_WIDTH).min(values.len());
        let mut s = 0.0f32;
        for j in wave_start..wave_end {
            s += values[j];
        }
        for j in wave_start..wave_end {
            out[j] = s;
        }
        i = wave_end;
    }
    out
}

/// Ballot mask: bit `j` is set when lane `j` is true. Returns the full mask for
/// all lanes supplied.
pub fn subgroup_ballot(cond: &[bool]) -> u64 {
    let mut mask = 0u64;
    for (j, &c) in cond.iter().enumerate() {
        if c && j < 64 {
            mask |= 1u64 << (j as u32);
        }
    }
    mask
}

pub fn wgsl_source() -> &'static str {
    SUBGROUP_WGSL
}

pub const SUBGROUP_WGSL: &str = include_str!("../shaders/subgroup.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reduce_sums_each_wave() {
        let v: Vec<f32> = (0..33).map(|x| x as f32).collect();
        let out = subgroup_reduce_add(&v);
        // First 32 lanes share one wave -> sum 0..=31 = 496.
        assert!((out[0] - 496.0).abs() < 1e-6);
        assert!((out[31] - 496.0).abs() < 1e-6);
        // Lane 32 is its own wave -> sum = 32.
        assert!((out[32] - 32.0).abs() < 1e-6);
    }
    #[test]
    fn ballot_sets_true_lanes() {
        let cond = [true, false, true, false];
        let m = subgroup_ballot(&cond);
        assert_eq!(m, 0b0101);
    }
    #[test]
    fn ballot_empty_is_zero() {
        let cond = [false; 8];
        assert_eq!(subgroup_ballot(&cond), 0);
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    /// EN-4: reduce golden・部分 wave 境界 (rq 導出 bits:
    /// 496.0=0x43F80000・32.0=0x42000000・1520.0=0x44BE0000・
    /// 64.0=0x42800000。整数 ≤ 2^24 の f32 和は厳密)。
    #[test]
    fn reduce_golden_bits_partial_waves() {
        let v33: Vec<f32> = (0..33).map(|x| x as f32).collect();
        let out33 = subgroup_reduce_add(&v33);
        assert_eq!(out33.len(), 33);
        assert_eq!(out33[0].to_bits(), 0x43F80000, "wave0 sum 496.0");
        assert_eq!(out33[31].to_bits(), 0x43F80000);
        assert_eq!(out33[32].to_bits(), 0x42000000, "部分 wave1 単独 32.0");

        let v64: Vec<f32> = (0..64).map(|x| x as f32).collect();
        let out64 = subgroup_reduce_add(&v64);
        assert_eq!(out64[32].to_bits(), 0x44BE0000, "wave1 sum 1520.0");

        let v65: Vec<f32> = (0..65).map(|x| x as f32).collect();
        let out65 = subgroup_reduce_add(&v65);
        assert_eq!(out65.len(), 65);
        assert_eq!(out65[64].to_bits(), 0x42800000, "部分 wave2 単独 64.0");

        assert!(subgroup_reduce_add(&[]).is_empty(), "空入力は空");
    }

    /// EN-4: f32 非結合の機械 pin (rq 導出) — [1e20; 32] の wave 内
    /// index 順逐次和は 0x632D78EB で、一括乗算 (1e20*32.0) 0x632D78EC
    /// とは **1 ulp 差異**。reduce の順序確定参照値であることを固定。
    #[test]
    fn reduce_sequential_rounding_order_pin() {
        let out = subgroup_reduce_add(&[1.0e20f32; 32]);
        assert_eq!(out[0].to_bits(), 0x632D78EB, "逐次和 (rq 導出)");
        assert_eq!(
            (1.0e20f32 * 32.0).to_bits(),
            0x632D78EC,
            "一括乗算は 1 ulp 上"
        );
        assert_ne!(out[0].to_bits(), (1.0e20f32 * 32.0).to_bits());
    }

    /// EN-4: 順序消失の機械 pin (rq 導出) — 先頭 1e20 の後は
    /// 1.0×31 個加えても ulp 未満で全消失、和は 1e20 不変。
    #[test]
    fn reduce_large_lane_swallows_small_lanes() {
        let mut v = vec![1.0e20f32];
        v.extend(std::iter::repeat(1.0f32).take(31));
        let out = subgroup_reduce_add(&v);
        assert_eq!(out[0].to_bits(), 0x60AD78EC, "≡ 1e20 (rq 導出)");
        assert_eq!(out[31].to_bits(), 0x60AD78EC, "wave 全 lane 同一値");
    }

    /// EN-3-2 pin: ballot は j>=64 を静寂切捨て (65 lane 全 true でも
    /// 上位 lane は mask に現れない = u64::MAX のまま)。
    #[test]
    fn ballot_truncates_above_64_lanes() {
        let t64 = [true; 64];
        assert_eq!(subgroup_ballot(&t64), u64::MAX);
        let t65 = [true; 65];
        assert_eq!(
            subgroup_ballot(&t65),
            u64::MAX,
            "65 番目 lane (index 64) は切捨て"
        );
        let f65 = [false; 65];
        assert_eq!(subgroup_ballot(&f65), 0);
        // index 63 (最終有効 lane) のみ true → 最上位ビット
        let mut lane63 = [false; 64];
        lane63[63] = true;
        assert_eq!(subgroup_ballot(&lane63), 1u64 << 63);
    }

    /// EN-3-3 pin: WAVE_WIDTH 固定 32 + reduce 内部で使用される契約。
    #[test]
    fn wave_width_contract() {
        assert_eq!(WAVE_WIDTH, 32);
        // 32 要素 = ちょうど 1 wave: 全 lane 同一 sum
        let one: Vec<f32> = std::iter::repeat(2.0f32).take(32).collect();
        let out = subgroup_reduce_add(&one);
        assert!(out.iter().all(|&x| x == 64.0), "2.0×32 = 64.0");
        assert_eq!(out[0].to_bits(), 0x42800000);
    }
}
