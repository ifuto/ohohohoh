//! Screen-space shadows (SSS) for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): march a ray toward the light through a depth field
//! (supplied via closure so the core math is unit-testable) and report
//! visibility. SSS recovers contact shadows that cascaded shadow maps miss —
//! a *quality* improvement. Cost is bounded by `max_steps`, so it is cheap on
//! integrated GPUs (and can be run at half resolution then upscaled).
//!
//! 【wave 151 EW-2 誠実注記 5 項】
//! 1. `sample_depth` 契約の明確化 + 捕捉 63 [高] 経緯: 点 `p` が occluder
//!    上なら **`pos` からの進行距離** (`(p - pos).length()` 同形) を返し、
//!    空/到達不能は `f32::INFINITY` (WGSL 側は ≥1e30)。遮蔽判定は
//!    diff = surf − travelled ∈ [0, 2·step_size] の厚み内接触影のみ
//!    (diff<0 = 通過済み背面は lit、diff>2·step = 遠方非接触は lit)。
//!    **捕捉 63**: 旧 wiring の `sss_depth` は AABB 内で定数 0.0 を供給
//!    → diff = −travelled < 0 で全 16 step 常時非遮蔽、AABB 外は
//!    INFINITY で skip → **SSS は wiring 経路で全入力で常時 lit=1.0 の
//!    構造的全沈黙** (捕捉 62 と対称)。closure が (p−origin).length() を
//!    返すよう契約整合 (同式同入力で diff==0.0 exact → 接触影が実効)。
//!    wiring 版は chunk AABB 包含を占有 proxy とする coarse 近似 (16³ の
//!    空隙を無視する低スペック質 proxy、精細な depth field は GPU WGSL
//!    側 texture 供給)。
//! 2. Vec4 (型+new+Add/Sub/Mul) は本体・テスト・wiring 全消費者ゼロを
//!    census grep で機械確定し完全削除 (§7、EU-3/EV-2 同型)。Vec3 は
//!    new/dot/length/normalize/Add/Sub/Mul<f32> を**全て**本体消費
//!    (`(v - pos).length()` で Sub も使用中) のため完全維持。
//! 3. `travelled` は `(v - pos).length()` の f32 実系列評価: chunked
//!    同定数 step1 では 0x3DF5C28E (0.11999999) であり解析値 0.12·|ld|
//!    =0.11924764 (0x3DF4381B) とは成分個別丸めで一致しない (rq ew_sss
//!    機械導出) — しかし閉形式置換はせず、closure 側が同一 f32 系列を
//!    踏襲する限り diff==0.0 exact が構造保証される (捕捉 63 修正の核心)。
//! 4. NaN/退化契約 (fail-loud しない設計、G-buffer 品質契約): NaN depth
//!    は is_infinite=false → diff=NaN → 比較 false → 非遮蔽 (lit 側静寂、
//!    影が消える向き)。light_dir=0 は normalize fallback で v=pos 不動
//!    (sample 16 回、INF closure で lit)。step_size=0 は同点 16 回。
//!    NaN step/max_dist は比較 false 連鎖で lit。max_steps=0 は即 lit。
//!    bias はライト方向 push の自己影 (acne) 抑制で、負 bias は逆行。
//! 5. WGSL/CPU パリティ (shaders/screen_space_shadow.wgsl): 行対応同形
//!    (normalize→bias push→travelled 判定→diff 厚み判定)。差分: GPU 側は
//!    depth texture (uv 写像) 供給・INF 相当は `surf >= 1e30`、CPU は
//!    closure。両者とも NaN は lit 方向 (注記 4)。境界は diff=0/diff=
//!    2·step inclusive (同値 closure で diff==0.0 exact pin)、travelled>
//!    max_dist の等値は行進継続 (0.30000001>0.30000001=false、rq ew_sss)。

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

pub struct SssParams {
    pub max_steps: u32,
    pub step_size: f32,
    pub max_dist: f32,
    pub bias: f32,
}
impl Default for SssParams {
    fn default() -> Self {
        Self {
            max_steps: 16,
            step_size: 0.1,
            max_dist: 10.0,
            bias: 0.02,
        }
    }
}

/// 【wave 188 GH-1】AABB 群の占有判定に基づく occlusion depth 供給の唯一実装。
/// 点 `p` が何れかの AABB 内 (全端点閉区間) なら `(p−origin).length()`
/// (`cast_sss` の travelled と同式同入力で diff==0.0 exact、捕捉 63 修正の
/// 核心)、外なら `f32::INFINITY`。
/// wiring 側の inline closure は module テスト同型の**別複製**で、その値は
/// report 非属のため wiring 層 adversarial (EW (a) revert 変異) が構造的に
/// 非検出だった — 検出空白の根は「同じ意味論を 2 か所に複製保持」した
/// 構造にある。module/wiring を本関数に一本化し、変異が必ず module golden
/// に到達する形へ根治 (検出空白クラスの構造的除去 = EW (a) 最終判定)。
/// 実消費者: full_graph_wiring の sss_depth closure (経由先)。
pub fn aabb_occupancy_depth(p: Vec3, origin: Vec3, aabbs: &[([f32; 3], [f32; 3])]) -> f32 {
    for (mn, mx) in aabbs {
        if p.x >= mn[0]
            && p.x <= mx[0]
            && p.y >= mn[1]
            && p.y <= mx[1]
            && p.z >= mn[2]
            && p.z <= mx[2]
        {
            return (p - origin).length();
        }
    }
    f32::INFINITY
}

/// March toward the light; `sample_depth` は点 `p` が occluder 上なら
/// **`pos` からの進行距離** ((p−pos).length() 同形) を返し、空/到達不能は
/// `f32::INFINITY` を返す (EW-2 注記 1: 旧 doc「nearest occluder までの
/// 進行距離」は曖昧で、wiring が 0.0 定数を供給する捕捉 63 の温床だった)。
/// Returns 1.0 (lit) / 0.0 (shadowed)。
pub fn cast_sss(
    pos: Vec3,
    light_dir: Vec3,
    params: &SssParams,
    sample_depth: &dyn Fn(Vec3) -> f32,
) -> f32 {
    let ld = light_dir.normalize();
    let mut v = pos + ld * params.bias;
    for _ in 0..params.max_steps {
        v = v + ld * params.step_size;
        let travelled = (v - pos).length();
        if travelled > params.max_dist {
            return 1.0;
        }
        let surf = sample_depth(v);
        if surf.is_infinite() {
            continue;
        }
        let diff = surf - travelled;
        if diff >= 0.0 && diff <= params.step_size * 2.0 {
            return 0.0;
        }
    }
    1.0
}

pub fn wgsl_source() -> &'static str {
    SCREEN_SPACE_SHADOW_WGSL
}

pub const SCREEN_SPACE_SHADOW_WGSL: &str = include_str!("../shaders/screen_space_shadow.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clear_path_is_lit() {
        let vis = cast_sss(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            &SssParams::default(),
            &|_p: Vec3| f32::INFINITY,
        );
        assert_eq!(vis, 1.0);
    }
    #[test]
    fn occluder_in_path_is_shadowed() {
        // Occluder at travelled ~0.5 within the march.
        let vis = cast_sss(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            &SssParams {
                max_steps: 16,
                step_size: 0.1,
                max_dist: 10.0,
                bias: 0.02,
            },
            &|p: Vec3| 0.05 + p.y, // occluder just ahead of the ray (within thickness)
        );
        assert_eq!(vis, 0.0);
    }
    #[test]
    fn beyond_max_dist_is_lit() {
        let vis = cast_sss(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            &SssParams {
                max_steps: 4,
                step_size: 0.1,
                max_dist: 0.3,
                bias: 0.0,
            },
            &|_p: Vec3| 5.0,
        );
        assert_eq!(vis, 1.0);
    }

    // ---- wave 151 EW-3 strict 群 (全値 rq ew_sss 事前導出) ----

    /// 捕捉 63 golden (修正後構造): wiring 同型 closure (chunked: AABB 内
    /// 点は (p−origin).length()、外は INF) で step1 から diff==0.0 exact
    /// → shadow=0.0 + closure call 1 回 (即 return)。rq: ld=(0x3EB454A2,
    /// 0x3F0DB037, 0x3F41361C)、travelled step1=0x3DF5C28E、v.x=0x4100AD1E。
    #[test]
    fn ew_chunked_isomorphic_shadow_golden() {
        let origin = Vec3::new(8.0, 8.0, 8.0);
        let calls = std::cell::Cell::new(0u32);
        let depth = |p: Vec3| -> f32 {
            calls.set(calls.get() + 1);
            // wiring 修正版同型: [0,16)³ AABB 包含 → (p−origin).length()
            if p.x >= 0.0 && p.x <= 16.0 && p.y >= 0.0 && p.y <= 16.0 && p.z >= 0.0 && p.z <= 16.0 {
                (p - origin).length()
            } else {
                f32::INFINITY
            }
        };
        let vis = cast_sss(
            origin,
            Vec3::new(0.35, 0.55, 0.75),
            &SssParams::default(),
            &depth,
        );
        assert_eq!(vis, 0.0, "契約整合後は step1 で diff==0.0 exact → shadow");
        assert_eq!(calls.get(), 1, "即 return で sample 1 回のみ (rq)");
    }

    /// 【wave 188 GH-1】dedup 共有契約 golden: wiring/module を一本化した
    /// `aabb_occupancy_depth` 経由で EW-4 golden (vis=0.0・sample 1 回即
    /// return) が bit 同等に再現される (文レベル同一抽出のため IEEE 厳密
    /// 一致)。共有本体の捕捉 63 revert (0.0 定数化) は本 golden が RED 化
    /// (vis→1.0・calls 16) — EW (a) 検出空白クラスは構造的除去済。
    #[test]
    fn gh_shared_occupancy_depth_golden() {
        let aabbs = [([0.0f32, 0.0, 0.0], [16.0f32, 16.0, 16.0])];
        let origin = Vec3::new(8.0, 8.0, 8.0);
        let calls = std::cell::Cell::new(0u32);
        let depth = |p: Vec3| -> f32 {
            calls.set(calls.get() + 1);
            aabb_occupancy_depth(p, origin, &aabbs)
        };
        let vis = cast_sss(
            origin,
            Vec3::new(0.35, 0.55, 0.75),
            &SssParams::default(),
            &depth,
        );
        assert_eq!(vis, 0.0, "共有本体経由でも EW-4 golden と同一 (rq ew_sss)");
        assert_eq!(calls.get(), 1, "共有本体経由でも即 return (sample 1 回)");
    }

    /// 【wave 188 GH-1】aabb_occupancy_depth 直接契約 pin: AABB 包含は全
    /// 端点閉区間 (16.0 包含)、内部点は (p−origin).length()、外部/空 AABB
    /// は INF。bits は python struct.pack 機械値 (8.0=0x41000000・1.0=
    /// 0x3F800000・16.0=0x41800000)。
    #[test]
    fn gh_aabb_occupancy_depth_direct_contract() {
        let aabbs = [([0.0f32, 0.0, 0.0], [16.0f32, 16.0, 16.0])];
        let origin = Vec3::new(8.0, 8.0, 8.0);
        // 閉区間境界: x=16.0 丁度は包含、(8−origin)=8.0 で正負対称
        let d1 = aabb_occupancy_depth(Vec3::new(16.0, 8.0, 8.0), origin, &aabbs);
        assert_eq!(d1.to_bits(), 0x4100_0000, "境界包含 8.0 (機械 bits)");
        let d2 = aabb_occupancy_depth(Vec3::new(8.0, 7.0, 8.0), origin, &aabbs);
        assert_eq!(d2.to_bits(), 0x3F80_0000, "単位オフセット 1.0 (機械 bits)");
        // 外部点 (z>16) と空 AABB は INF
        let d3 = aabb_occupancy_depth(Vec3::new(8.0, 8.0, 16.5), origin, &aabbs);
        assert!(d3.is_infinite(), "外部点は INFINITY");
        let empty: [([f32; 3], [f32; 3]); 0] = [];
        let d4 = aabb_occupancy_depth(Vec3::new(8.0, 8.0, 8.0), origin, &empty);
        assert!(d4.is_infinite(), "空 AABB は INFINITY");
    }

    /// 捕捉 63 再現 pin (旧構造の記録): AABB 内点に定数 0.0 を供給する旧
    /// closure では diff=−travelled<0 が 16 step 連鎖 → 常時 lit=1.0。
    /// wiring 層は report 非属で検出空白のため、旧挙動は module 層で固定。
    #[test]
    fn ew_legacy_zero_contract_all_lit_dual() {
        let origin = Vec3::new(8.0, 8.0, 8.0);
        let calls = std::cell::Cell::new(0u32);
        let legacy = |p: Vec3| -> f32 {
            calls.set(calls.get() + 1);
            if p.x >= 0.0 && p.x <= 16.0 && p.y >= 0.0 && p.y <= 16.0 && p.z >= 0.0 && p.z <= 16.0 {
                0.0 // 捕捉 63: 旧 wiring 供給値
            } else {
                f32::INFINITY
            }
        };
        let vis = cast_sss(
            origin,
            Vec3::new(0.35, 0.55, 0.75),
            &SssParams::default(),
            &legacy,
        );
        assert_eq!(
            vis, 1.0,
            "旧 0.0 契約では全 step 非遮蔽 → lit (捕捉 63 再現)"
        );
        assert_eq!(calls.get(), 16, "非遮蔽で全 16 step 走査 (rq)");
    }

    /// travelled>max_dist の等値は行進継続 (注記 5): 0.30000001>0.30000001
    /// = false で sample 3 回、t4 (0.4) で return → 計 3 sample。
    #[test]
    fn ew_boundary_maxdist_sample_count() {
        let calls = std::cell::Cell::new(0u32);
        let vis = cast_sss(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            &SssParams {
                max_steps: 4,
                step_size: 0.1,
                max_dist: 0.3,
                bias: 0.0,
            },
            &|_p: Vec3| {
                calls.set(calls.get() + 1);
                5.0
            },
        );
        assert_eq!(vis, 1.0);
        assert_eq!(
            calls.get(),
            3,
            "等値継続 3 sample (rq: 0.3f32 同値は 0.3>0.3 false)"
        );
    }

    /// 厚み窓 (注記 1): diff=surf−travelled ∈ [0, 2·step] の内側/外側を
    /// 0.01 安全域で pin (rq: L=0.52 で 0.19 供給 diff=0x3E428F5C ≤0.2、
    /// 0.21 供給 diff=0x3E570A3C >0.2)。
    #[test]
    fn ew_thickness_window() {
        let origin = Vec3::new(0.0, 0.0, 0.0);
        let mk = |extra: f32| {
            move |p: Vec3| -> f32 {
                let l = (p - origin).length();
                if l > 0.09 {
                    l + extra
                } else {
                    f32::INFINITY
                }
            }
        };
        let params = SssParams {
            max_steps: 16,
            step_size: 0.1,
            max_dist: 10.0,
            bias: 0.0,
        };
        let ld = Vec3::new(0.0, 1.0, 0.0);
        let inside = cast_sss(origin, ld, &params, &mk(0.19));
        assert_eq!(inside, 0.0, "厚み +0.19 (<0.2) は接触影 (rq)");
        let outside = cast_sss(origin, ld, &params, &mk(0.21));
        assert_eq!(outside, 1.0, "厚み +0.21 (>0.2) は非接触 lit (rq)");
    }

    /// NaN depth 契約 (注記 4): is_infinite=false → diff=NaN → 比較 false
    /// → 非遮蔽 = lit 側静寂 (影が消える向き)、全 16 sample 走査。
    #[test]
    fn ew_nan_depth_fails_to_lit() {
        let calls = std::cell::Cell::new(0u32);
        let vis = cast_sss(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            &SssParams::default(),
            &|_p: Vec3| {
                calls.set(calls.get() + 1);
                f32::NAN
            },
        );
        assert_eq!(vis, 1.0, "NaN depth は lit 側静寂 (注記 4)");
        assert_eq!(calls.get(), 16);
    }

    /// 退化 2 系統 (注記 4): light_dir=0 (normalize fallback で v=pos 不動)
    /// / max_steps=0 (即 lit、sample 0 回)。
    #[test]
    fn ew_degenerate_inputs() {
        let calls = std::cell::Cell::new(0u32);
        let vis = cast_sss(
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(0.0, 0.0, 0.0),
            &SssParams::default(),
            &|_p: Vec3| {
                calls.set(calls.get() + 1);
                f32::INFINITY
            },
        );
        assert_eq!(vis, 1.0);
        assert_eq!(calls.get(), 16, "v 不動でも同点 16 回打診 (注記 4)");
        let calls2 = std::cell::Cell::new(0u32);
        let vis2 = cast_sss(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            &SssParams {
                max_steps: 0,
                step_size: 0.1,
                max_dist: 10.0,
                bias: 0.02,
            },
            &|_p: Vec3| {
                calls2.set(calls2.get() + 1);
                0.0
            },
        );
        assert_eq!(vis2, 1.0);
        assert_eq!(calls2.get(), 0, "max_steps=0 は即 lit (未打診)");
    }

    /// bias 初回位置 (注記 4): pos.y=2.0 +bias 0.5 +step 0.25 → 初回
    /// sample は y=2.75 exact (0x40300000、rq)。
    #[test]
    fn ew_bias_first_sample_position() {
        let seen_y = std::cell::Cell::new(f32::NAN);
        cast_sss(
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(0.0, 1.0, 0.0),
            &SssParams {
                max_steps: 16,
                step_size: 0.25,
                max_dist: 10.0,
                bias: 0.5,
            },
            &|p: Vec3| {
                if seen_y.get().is_nan() {
                    seen_y.set(p.y);
                }
                f32::INFINITY
            },
        );
        assert_eq!(
            seen_y.get().to_bits(),
            0x4030_0000,
            "初回 sample は bias+step 位置 2.75 exact (rq)"
        );
    }

    /// Vec3 契約 pin (注記 2): Sub も本体消費 ((v−pos).length()) で全 op
    /// 維持、Vec4 は完全削除。wgsl identity は &str 内容比較 (ET 規律)。
    #[test]
    fn ew_vec3_and_wgsl_contract() {
        let n = Vec3::new(3.0, 4.0, 0.0).normalize();
        assert_eq!(n.x.to_bits(), 0x3F19_999A, "3/5 (rq)");
        assert_eq!(n.y.to_bits(), 0x3F4C_CCCD, "4/5 (rq)");
        let d = Vec3::new(1.0, 4.0, 5.0) - Vec3::new(1.0, 1.0, 1.0);
        // (0,3,4) → 5.0 exact (私の初版は (4,6,8)→12 と sqrt(116)=10.77 の
        // 暗算誤りで本テストが RED 捕捉 → ピタゴラス triple へ訂正、誠実記録)
        assert_eq!(
            d.length().to_bits(),
            5.0f32.to_bits(),
            "Sub 本体面 (0,3,4)→5"
        );
        assert_eq!(
            wgsl_source(),
            SCREEN_SPACE_SHADOW_WGSL,
            "include_str identity"
        );
        assert!(
            wgsl_source().contains("diff >= 0.0 && diff <= step_size * 2.0"),
            "WGSL 厚み判定同形 (注記 5)"
        );
    }

    /// 【wave 185 GE フェーズ2 回収】dead code 系 3 例目 (wave 151 EW adversarial (b)
    /// Vec4 復活 revert 非検出、EW-2 で型+全 trait 不可能証明削除済) の lexeme pin 化。
    /// Vec3 側演算 (Add/Sub/Mul) は本体消費で維持中 (宣言形は Vec4 側のみ検出)。
    /// 同宣言形の将来復活を静寂に通さない。
    #[test]
    fn ge_removed_vec4_lexeme() {
        let src = include_str!("screen_space_shadow.rs");
        for lex in [
            concat!("struct ", "Vec4"),
            concat!("impl ", "Vec4"),
            concat!("Add for ", "Vec4"),
            concat!("Sub for ", "Vec4"),
            concat!("Mul<f32> for ", "Vec4"),
        ] {
            assert!(
                !src.contains(lex),
                "dead code 系削除語彙の宣言形復活を検出 (wave 185 GE lexeme pin)"
            );
        }
    }
}
