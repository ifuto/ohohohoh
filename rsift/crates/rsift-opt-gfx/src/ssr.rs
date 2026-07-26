//! Screen-space reflections (SSR) for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): ray-march a view-space ray through a height field
//! given by a depth-sampling closure, with thickness-tested hit detection and a
//! reprojection-style disocclusion fallback. Works in any 3D field; the G-buffer
//! depth is supplied via a closure so the core math is unit-testable. This adds
//! reflections that screen-space techniques can reach — a quality improvement.
//!
//! ## 消費者の実効挙動 (監査 2026-07-26 DT-1)
//!
//! 唯一の Rust 側呼出 `full_graph_wiring.rs:1537-1568` は depth sampler に
//! 「AABB 内 = 0.0 / 外 = INFINITY」を与える。`march` の hit 条件は
//! `diff = surf - travelled ∈ [0, thickness]` で、surf=0.0 では travelled > 0
//! (step_size=0.1) より diff = -travelled < 0 で**構造的に永不発** → march は
//! 常に `None` を返す。かつ戻り値は `let _ssr_hit` で破棄される。恒等クラス
//! (bloom DM-2 / fxaa DQ-1 / smaa DS-1) とは別型の「常時 miss + 破棄」ゼロ効果
//! 構造。実効化は G-buffer 深度の実サンプラ配線の設計判断のため引継ぎ。GPU 側は
//! `ssr.wgsl` (gpu_runtime 登録) の別経路。
//!
//! ## Rust/WGSL 表現差 (監査 2026-07-26 DT-5)
//!
//! 空マーカーは Rust が `is_infinite()` (±∞ のみ) なのに対し WGSL は
//! `SSR_INF = 1e30` の有限比較。1e30 有限深度は Rust でも diff 経路で不発・継続と
//! なるため帰結は同等だが、判定経路差は公表する。WGSL の uv 写像
//! `pos.xy / res * 0.5 + 0.5` は真の投影ではない様式化であり、surf (view 線形深度)
//! を along-ray 距離として扱う近似 — 消費者警告。
//!
//! ## 保持 API (監査 2026-07-26 DT-4)
//!
//! `Vec4` は消費者ゼロだが WGSL パリティ API 面として意図的に保持する
//! (bloom / fxaa / atmospheric 等他モジュールと同方針)。

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn normalize(self) -> Vec3 {
        let l = self.length();
        if l > 1e-8 {
            self * (1.0 / l)
        } else {
            self
        }
    }
}
impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}
impl Vec4 {
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}
impl Add for Vec4 {
    type Output = Vec4;
    fn add(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x + o.x, self.y + o.y, self.z + o.z, self.w + o.w)
    }
}
impl Sub for Vec4 {
    type Output = Vec4;
    fn sub(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x - o.x, self.y - o.y, self.z - o.z, self.w - o.w)
    }
}
impl Mul<f32> for Vec4 {
    type Output = Vec4;
    fn mul(self, s: f32) -> Vec4 {
        Vec4::new(self.x * s, self.y * s, self.z * s, self.w * s)
    }
}

pub struct SsrParams {
    pub max_steps: u32,
    pub step_size: f32,
    pub thickness: f32,
    pub max_dist: f32,
}

impl Default for SsrParams {
    fn default() -> Self {
        Self {
            max_steps: 32,
            step_size: 0.1,
            thickness: 0.5,
            max_dist: 50.0,
        }
    }
}

/// March a view-space ray `ro + t*rd`. `sample_depth` returns the
/// surface depth at a travelled position (or `f32::INFINITY` if empty).
/// Returns the hit position, or `None` if the ray exits / misses.
///
/// ## 契約 (監査 2026-07-26 DT-2/DT-6)
///
/// - hit 窓は**一方向符号付き** `diff = surf - travelled ∈ [0, thickness]`
///   (両端 inclusive)。ray が表面を通過済み (diff<0) の再接近は拾わない
///   (典型 SSR の両側 |diff| 窓とは異なる様式化)。
/// - `travelled > max_dist` は**厳密 `>`** (等値では早期終了しない)。
/// - `rd` は内部で `normalize` される (|rd|<=1e-8 は self 返却＝非正規化継続)。
/// - NaN 成分を持つ ray は normalize ガード・全比較とも false で、
///   誤 hit せず max_steps 走査後 `None` (fail-safe・早期終了もしない)。
/// - `step_size = 0.0` は位置不変 → `diff = surf` の原地判定を max_steps 反復、
///   `max_steps = 0` は無条件 `None`。
pub fn march(
    ro: Vec3,
    rd: Vec3,
    params: &SsrParams,
    sample_depth: &dyn Fn(Vec3) -> f32,
) -> Option<Vec3> {
    let rd = rd.normalize();
    let mut pos = ro;
    for _ in 0..params.max_steps {
        pos = pos + rd * params.step_size;
        let travelled = (pos - ro).length();
        if travelled > params.max_dist {
            return None;
        }
        let surf = sample_depth(pos);
        if surf.is_infinite() {
            continue;
        }
        // scene depth is measured along the ray; compare travelled vs surface.
        let diff = surf - travelled;
        if diff >= 0.0 && diff <= params.thickness {
            return Some(pos);
        }
    }
    None
}

/// Reflect a view direction about a normal.
///
/// ## 契約 (監査 2026-07-26 DT-3)
///
/// 入射・法線は内部で正規化され、結果も正規化して返す。ゼロ法線
/// (|n|<=1e-8) は normalize が self を返すため d=0 → 反射なし =
/// **正規化済み入射のパススルー** (安全側)。往復 reflect(reflect(i,n),n)
/// は一般に累積丸めを伴うが、|i|² が厳密 1.0 の構成では厳密に復元する
/// (テストで pin)。
pub fn reflect_dir(incident: Vec3, normal: Vec3) -> Vec3 {
    let n = normal.normalize();
    let i = incident.normalize();
    let d = i.dot(n);
    (i - n * (2.0 * d)).normalize()
}

pub fn wgsl_source() -> &'static str {
    SSR_WGSL
}

pub const SSR_WGSL: &str = include_str!("../shaders/ssr.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn march_hits_plane_heightfield() {
        // A flat "scene" at travelled distance 5.0 (infinite plane).
        let scene = |_p: Vec3| 5.0f32;
        let hit = march(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            &SsrParams {
                max_steps: 200,
                step_size: 0.05,
                thickness: 0.05,
                max_dist: 50.0,
            },
            &scene,
        );
        assert!(hit.is_some());
        let h = hit.unwrap();
        assert!((h.z - 5.0).abs() < 0.2, "hit z = {}", h.z);
    }
    #[test]
    fn march_misses_empty_scene() {
        let scene = |_p: Vec3| f32::INFINITY;
        let hit = march(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            &SsrParams::default(),
            &scene,
        );
        assert!(hit.is_none());
    }
    #[test]
    fn reflect_bounces_off_normal() {
        // Ray going +z, normal facing +z -> reflection goes -z.
        let r = reflect_dir(Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0));
        assert!(r.z < -0.99, "reflected z = {}", r.z);
    }

    /// DT-1: full_graph_wiring:1537-1568 と同型の sampler (AABB 内=0.0/外=∞)。
    /// surf=0.0 では diff = -travelled < 0 (travelled>0) で hit 条件 `diff >= 0`
    /// が構造的に永不発 → march は常に None。戻り値破棄 (`let _ssr_hit`) と
    /// 併せて「常時 miss + 破棄」のゼロ効果構造であることを確定記録する。
    /// 実効化は G-buffer 深度の実サンプラ配線の設計判断 (引継ぎ)。
    #[test]
    fn wiring_zero_semantics_make_march_structurally_miss() {
        let calls = std::cell::Cell::new(0u32);
        let sampler = |p: Vec3| -> f32 {
            calls.set(calls.get() + 1);
            if p.x >= 0.0 && p.x <= 1.0 && p.y >= 0.0 && p.y <= 1.0 && p.z >= 0.0 && p.z <= 1.0 {
                0.0
            } else {
                f32::INFINITY
            }
        };
        // カメラを AABB 中心 (0.5,0.5,0.5) に配置: 最初の数 step は surf=0.0、
        // 抜けた後は INFINITY。どちらでも hit しないことを全走査で証明。
        let hit = march(
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(0.0, 0.0, 1.0),
            &SsrParams::default(),
            &sampler,
        );
        assert!(
            hit.is_none(),
            "wiring 同型 sampler は構造的に miss: {hit:?}"
        );
        assert_eq!(calls.get(), 32, "default max_steps を全走査 (早期終了なし)");
    }

    /// DT-2: hit 窓 [0, thickness] の両端 inclusive と派生境界の厳密 pin。
    /// 格子点上の正確表現値のみで構成 (rq 検算済: 4.5=0x40900000, 5.0=0x40A00000)。
    #[test]
    fn march_hit_window_inclusive_bounds_exact_bits() {
        let scene = |_p: Vec3| 5.0f32;
        let rd = Vec3::new(0.0, 0.0, 1.0);
        let mk = |thickness: f32| SsrParams {
            max_steps: 200,
            step_size: 0.5,
            thickness,
            max_dist: 50.0,
        };
        // 上端 inclusive: k=9 で travelled=4.5, diff=0.5 == thickness → hit。
        let h = march(Vec3::new(0.0, 0.0, 0.0), rd, &mk(0.5), &scene).unwrap();
        assert_eq!(h.z.to_bits(), 0x40900000, "上端 inclusive hit (0,0,4.5)");
        // thickness を 1 ulp 下げると k=9 は窓外 → k=10 で diff=0 (下端 inclusive) → hit。
        let h2 = march(
            Vec3::new(0.0, 0.0, 0.0),
            rd,
            &mk(f32::from_bits(0x3EFFFFFF)),
            &scene,
        )
        .unwrap();
        assert_eq!(h2.z.to_bits(), 0x40A00000, "下端 inclusive hit (0,0,5.0)");
        // step_size=0.0 の退化: 位置不変 → diff=surf の原地判定 (1 step 目で hit)。
        let h0 = march(
            Vec3::new(1.0, 2.0, 3.0),
            rd,
            &SsrParams {
                max_steps: 8,
                step_size: 0.0,
                thickness: 0.5,
                max_dist: 50.0,
            },
            &|_p: Vec3| 0.3f32,
        )
        .unwrap();
        assert_eq!(
            (h0.x.to_bits(), h0.y.to_bits(), h0.z.to_bits()),
            (0x3F800000, 0x40000000, 0x40400000),
            "原地 hit は ro と厳密一致"
        );
        // max_steps=0 は無条件 None。
        assert!(march(
            Vec3::new(0.0, 0.0, 0.0),
            rd,
            &SsrParams {
                max_steps: 0,
                step_size: 0.5,
                thickness: 0.5,
                max_dist: 50.0
            },
            &scene
        )
        .is_none());
    }

    /// DT-2: `travelled > max_dist` は厳密 `>`。等値ではサンプルが実行される
    /// ことをコール回数で pin (travelled=1.0 は格子点で厳密)。
    #[test]
    fn march_max_dist_strict_boundary_sample_count() {
        let calls = std::cell::Cell::new(0u32);
        let scene = |_p: Vec3| {
            calls.set(calls.get() + 1);
            f32::INFINITY
        };
        let rd = Vec3::new(0.0, 0.0, 1.0);
        let params = SsrParams {
            max_steps: 10,
            step_size: 0.5,
            thickness: 0.5,
            max_dist: 1.0,
        };
        assert!(march(Vec3::new(0.0, 0.0, 0.0), rd, &params, &scene).is_none());
        assert_eq!(
            calls.get(),
            2,
            "k=1 (0.5) と k=2 (1.0 == max_dist) では sample 実行、k=3 (1.5) は超過で return"
        );
        // 1 ulp 下げると k=2 (travelled=1.0) が超過 → sample 1 回のみ。
        let calls2 = std::cell::Cell::new(0u32);
        let scene2 = |_p: Vec3| {
            calls2.set(calls2.get() + 1);
            f32::INFINITY
        };
        let params2 = SsrParams {
            max_dist: f32::from_bits(0x3F7FFFFF),
            ..params
        };
        assert!(march(Vec3::new(0.0, 0.0, 0.0), rd, &params2, &scene2).is_none());
        assert_eq!(
            calls2.get(),
            1,
            "travelled=1.0 > 0.99999994 で k=2 は return"
        );
    }

    /// DT-2: NaN 成分を持つ ray は normalize ガード・全比較が false で
    /// 誤 hit せず max_steps 走査後 None (fail-safe。早期終了もしない)。
    #[test]
    fn march_nan_ray_fail_safe_contract() {
        let calls = std::cell::Cell::new(0u32);
        let scene = |_p: Vec3| {
            calls.set(calls.get() + 1);
            5.0f32
        };
        let hit = march(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(f32::NAN, 0.0, 1.0),
            &SsrParams::default(),
            &scene,
        );
        assert!(hit.is_none(), "NaN ray は誤 hit しない: {hit:?}");
        assert_eq!(calls.get(), 32, "NaN でも全走査 (max_steps) を継続");
    }

    /// DT-6: 一方向符号付き窓の公表 pin。surf を跨いだ後の再接近 (diff<0)
    /// は拾わない。step=2.0・surf=5.5 で k=3 の |diff|=0.5 は両側窓なら hit
    /// する位置 — 片側契約では通過して miss することを固定。
    #[test]
    fn march_one_sided_window_pass_through_contract() {
        let scene = |_p: Vec3| 5.5f32;
        let params = SsrParams {
            max_steps: 32,
            step_size: 2.0,
            thickness: 0.5,
            max_dist: 50.0,
        };
        assert!(
            march(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
                &params,
                &scene
            )
            .is_none(),
            "片側窓: 通過後の再接近 (diff=-0.5) は hit しない"
        );
    }

    /// DT-3: reflect_dir の厳密 bit pin (rq 検算済) とゼロ法線パススルー契約。
    #[test]
    fn reflect_exact_bits_zero_normal_and_roundtrip() {
        // 軸: 入射=法線 → 完全反転 (厳密)。
        let r = reflect_dir(Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(
            (r.x.to_bits(), r.y.to_bits(), r.z.to_bits()),
            (0, 0, 0xBF800000),
            "軸反射 (0,0,-1)"
        );
        // 斜め: i=(0.6,-0.8,0) は |i|² ≡ 1.0 厳密 → 全演算厳密で (0.6, 0.8, 0)。
        let n = Vec3::new(0.0, 1.0, 0.0);
        let r2 = reflect_dir(Vec3::new(0.6, -0.8, 0.0), n);
        assert_eq!(
            (r2.x.to_bits(), r2.y.to_bits(), r2.z.to_bits()),
            (0x3F19999A, 0x3F4CCCCD, 0),
            "鏡面反射厳密 bits"
        );
        // 往復: 同一構成なら厳密に正規化済み入射へ復元。
        let r3 = reflect_dir(r2, n);
        assert_eq!(
            (r3.x.to_bits(), r3.y.to_bits(), r3.z.to_bits()),
            (0x3F19999A, 0xBF4CCCCD, 0),
            "往復 = 正規化済み入射 (0.6,-0.8,0)"
        );
        // 検出空白の補完 (adversarial (e) 対応): |pre| が 1.0 から 1 ulp ずれる
        // 構成で末尾 normalize の実効を pin (rq 事前導出: pre=(0x3E88D677,
        // 0xBF08D677, 0x3F4D41B2), |pre|=0x3F7FFFFF → 正規化後は x/y +1 ulp,
        // z +2 ulp)。
        let rd = reflect_dir(Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(
            (rd.x.to_bits(), rd.y.to_bits(), rd.z.to_bits()),
            (0x3E88D678, 0xBF08D678, 0x3F4D41B4),
            "末尾 normalize が効いた厳密 bits"
        );
        // ゼロ法線: d=0 → 反射なし = 正規化済み入射のパススルー。
        let rz = reflect_dir(Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(
            (rz.x.to_bits(), rz.y.to_bits(), rz.z.to_bits()),
            (0, 0, 0x3F800000),
            "ゼロ法線 → (0,0,1) パススルー"
        );
    }

    /// DT-5: Rust/WGSL の表現差と様式化の走査 pin。
    /// 空マーカーは Rust `is_infinite()` vs WGSL 有限 1e30 比較、hit 窓は
    /// 両側ではなく一方向 [0, thickness] (WGSL 側も同型であることを固定)。
    #[test]
    fn wgsl_empty_marker_and_one_sided_scan_pin() {
        assert!(
            SSR_WGSL.contains("const SSR_INF: f32 = 1e30;"),
            "WGSL 空マーカーは有限 1e30 (Rust の is_infinite と表現差・公表済)"
        );
        assert!(
            SSR_WGSL.contains("diff >= 0.0 && diff <= p.thickness"),
            "WGSL も一方向符号付き窓 [0, thickness] を維持"
        );
    }
}
