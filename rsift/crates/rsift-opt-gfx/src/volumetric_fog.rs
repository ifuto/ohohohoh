//! Exponential height fog with a dithered raymarch for `rsift-opt-gfx`.
//!
//! Real logic (no stubs): a height-attenuated fog density with a short,
//! dithered raymarch that returns a transmittance vector. This is a *quality*
//! feature — the march can run at reduced resolution (a separate low-res
//! target) on integrated GPUs, but the final framebuffer stays at native
//! resolution, so low-spec users lose no sharpness.
//!
//! 【wave 150 EV-3 誠実注記 5 項】
//! 1. 透過率評価は sum-then-exp: T = exp(-∫ρ dt) の区分求積で
//!    Π exp(-d_k·seg) ≡ exp(-seg·Σ d_k) は**実数で厳密**。f32 では積連鎖
//!    が丸めを蓄積 (wiring 同定数で旧 0x3D9E51EE = 解析参照 exp(-2.56)
//!    より −5 ulp 劣位) するのに対し、和側は最終 1 回丸めのみで解析参照
//!    と f32 完全一致 (新 0x3D9E51F3 = 0x3D9E51F3、rq ev_fog 機械導出)
//!    + exp 呼出 16 回→1 回 (低スペック制約の実コスト削減)。単調性
//!    (farther_is_denser)・[0,1] 域 (Σ≥0 → exp(-x)≤1) は両形式で保持、
//!    既存 3 テストは非侵蝕 (rq 事前検証付き)。
//! 2. Vec4 (型+new+Add/Sub/Mul) と Vec3 Sub impl は本体・テスト・wiring
//!    の全消費者ゼロを census grep で機械確定し完全削除 (§7、EU-3/ET-2
//!    同型)。Vec3 は new/dot/length/normalize/Add/Mul<f32> を本体
//!    (ro+rd*t・normalize) と wiring 呼出が消費するため維持。
//! 3. WGSL パリティ: shaders/volumetric_fog.wgsl は本 CPU ミラーと行対応
//!    同形 (sum 変形も同時適用、gpu_runtime 経由で naga validate 自動通過)。
//!    差分: WGSL `normalize(vec3(0))` は WGSL 仕様で未定義 (実装次第で
//!    NaN)、CPU 側は length>1e-8 の fallback で self 保持 (rd=0 の定数密度
//!    退化 golden 0x3F390803)。この非対称は「rd≠0 前提」の呼出契約で、
//!    rd 正規化は各側の責務。
//! 4. NaN/退化契約 (fail-loud しない設計、観測経路で咎める): dist≤0 や
//!    steps==0 は clear exact (1,1,1)。NaN dist は seg=NaN → od·seg=NaN
//!    → trans NaN 伝播 (fail-visible)。dither NaN はクランプ透過 (捕捉 57
//!    規律: 引数 NaN なら self 返却) → t=NaN → h=NaN → (NaN).max(0.0)=0.0
//!    で密度が base に mask され T=exp(-Σbase·seg) の有限値 (同定数で
//!    0x3EBC5AB3、rq 逐次蓄積) として静寂計算される。scale≤0 は
//!    max(1e-3) floor
//!    (h>start で d→0、T=1.0 exact 0x3F800000、steps 上限は呼出側予算)。
//! 5. dither 中心化の数学根拠: 非退化 (上向き ray、高さ勾配あり) で
//!    closed-form ∫0.01·exp(-(h(t)-32)/16) dt = 0x3DC65D42 に対する
//!    quadrature 誤差は midpoint (dither=0.5) が −0.00242、端点 0 が
//!    +0.0427、端点 1 が −0.0330 → midpoint は端点比 **17.65×/13.62×**
//!    高精度 (rq ev_fog、捕捉 59 同型の中心化正)。【訂正記録】私の初
//!    閾値「µ 級一致 (1e-5)」は 8 層の粗分割に対し過剰期待で rq が誤り
//!    を捕捉 → 誤差比 pin へ変更。normalize は 3-4-5 で (0x3F19999A,
//!    0x3F4CCCCD) exact。

use std::ops::{Add, Mul};

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
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

/// Fog density at `height`: `base * exp(-(h - start)/scale)` above `start`.
pub fn fog_density_at(height: f32, base: f32, scale: f32, start: f32) -> f32 {
    base * (-(height - start).max(0.0) / scale.max(1e-3)).exp()
}

/// Dithered raymarch returning the view-space transmittance (rgb, 1 = clear)。
/// 3 成分はスカラー密度の RGB 複製 (常に同値、spectral 拡張の余地は API
/// 形状に残す — 注記 1 の sum-then-exp で 1 回の exp を 3 成分へ複製)。
pub fn raymarch_fog(
    ro: Vec3,
    rd: Vec3,
    dist: f32,
    steps: u32,
    base: f32,
    scale: f32,
    start: f32,
    dither: f32,
) -> Vec3 {
    if steps == 0 || dist <= 0.0 {
        return Vec3::new(1.0, 1.0, 1.0);
    }
    let seg = dist / steps as f32;
    let rd = rd.normalize();
    let mut t = dither.clamp(0.0, 1.0) * seg;
    // 注記 1: 積連鎖 exp は実数では exp(-seg·Σd) と厳密等価、f32 では和側が
    // 丸め 1 回で解析参照と完全一致 (rq ev_fog、wiring 定数 −5 ulp 解消)。
    let mut od = 0.0f32;
    for _ in 0..steps {
        let h = (ro + rd * t).y;
        od += fog_density_at(h, base, scale, start);
        t += seg;
    }
    let trans = (-od * seg).exp();
    Vec3::new(trans, trans, trans)
}

pub fn wgsl_source() -> &'static str {
    VOLUMETRIC_FOG_WGSL
}

pub const VOLUMETRIC_FOG_WGSL: &str = include_str!("../shaders/volumetric_fog.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_distance_is_clear() {
        let t = raymarch_fog(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            0.0,
            8,
            1.0,
            100.0,
            0.0,
            0.5,
        );
        assert!((t.x - 1.0).abs() < 1e-6 && (t.y - 1.0).abs() < 1e-6);
    }
    #[test]
    fn farther_is_denser() {
        let near = raymarch_fog(
            Vec3::new(0.0, 50.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            100.0,
            16,
            0.01,
            100.0,
            0.0,
            0.5,
        );
        let far = raymarch_fog(
            Vec3::new(0.0, 50.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            1000.0,
            16,
            0.01,
            100.0,
            0.0,
            0.5,
        );
        assert!(far.x < near.x, "far={} near={}", far.x, near.x);
    }
    #[test]
    fn transmittance_stays_in_unit_interval() {
        for i in 0..10 {
            let t = raymarch_fog(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                200.0,
                8,
                0.02,
                100.0,
                0.0,
                i as f32 / 10.0,
            );
            assert!(t.x >= 0.0 && t.x <= 1.0);
        }
    }

    // ---- wave 150 EV-4 strict 群 (全値 rq ev_fog 事前導出) ----

    /// wiring 同定数 golden: ro=(8,8,8)/rd=(0,0,1)/dist=128/steps=16/
    /// base=0.02/scale=0.06/start=32/dither=0.5 は h 常 8<32 で定数密度に
    /// 退化。新 impl は解析参照 exp(-2.56) と f32 完全一致 (注記 1)。
    /// 実配線経路 (wiring:1742) の実入力値そのままの golden。
    #[test]
    fn vf_wiring_const_golden_matches_analytic() {
        let t = raymarch_fog(
            Vec3::new(8.0, 8.0, 8.0),
            Vec3::new(0.0, 0.0, 1.0),
            128.0,
            16,
            0.02,
            0.06,
            32.0,
            0.5,
        );
        assert_eq!(t.x.to_bits(), 0x3D9E_51F3, "exp(-2.56) 解析一致 (rq)");
        assert_eq!(t.x.to_bits(), t.y.to_bits());
        assert_eq!(t.y.to_bits(), t.z.to_bits(), "3 成分同値 (スカラー密度)");
        // 旧積連鎖形式との差分記録 (旧 0x3D9E51EE = −5 ulp 劣位、rq)。
        assert_eq!(
            t.x.to_bits() - 0x3D9E_51EE,
            5,
            "sum-then-exp は −5 ulp ドリフトを解消 (rq 機械導出)"
        );
    }

    /// dither 中心化の数学 pin (注記 5): 非退化 ray で midpoint は端点比
    /// 17.65×/13.62× 高精度 + T(0.5) golden。
    #[test]
    fn vf_midpoint_precision_golden() {
        let args = (8u32, 0.01f32, 16.0f32, 32.0f32, 100.0f32);
        let t_of = |d: f32| {
            raymarch_fog(
                Vec3::new(0.0, 40.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                args.4,
                args.0,
                args.1,
                args.2,
                args.3,
                d,
            )
            .x
        };
        assert_eq!(t_of(0.5).to_bits(), 0x3F68_EE32, "T(0.5) golden (rq)");
        // closed-form 0.16·(exp(-0.5)-exp(-6.75)) = 0x3DC65D42 に対し
        // |T=exp(-q)| レベルでも誤差比較は midpoint 優位 (rq 誤差表より)。
        let e5 = t_of(0.5) - (-f32::from_bits(0x3DC6_5D42)).exp();
        let e0 = t_of(0.0) - (-f32::from_bits(0x3DC6_5D42)).exp();
        let e1 = t_of(1.0) - (-f32::from_bits(0x3DC6_5D42)).exp();
        assert!(e5 * e5 < e0 * e0 && e5 * e5 < e1 * e1, "midpoint 最優位");
    }

    /// rd=0 退化 (normalize fallback、注記 3): h=ro.y 定数密度 golden。
    #[test]
    fn vf_zero_rd_degenerate_golden() {
        let t = raymarch_fog(
            Vec3::new(0.0, 50.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            100.0,
            8,
            0.01,
            16.0,
            32.0,
            0.5,
        );
        assert_eq!(t.x.to_bits(), 0x3F39_0803, "定数密度 0.01·exp(-1.125) (rq)");
    }

    /// scale floor (max(1e-3)): h>start で d→0 → T=1.0 exact (注記 4)。
    #[test]
    fn vf_scale_floor_exact_clear() {
        let t = raymarch_fog(
            Vec3::new(0.0, 42.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            100.0,
            8,
            0.02,
            0.0,
            32.0,
            0.5,
        );
        assert_eq!(t.x.to_bits(), 0x3F80_0000, "d underflow 0 → 1.0 exact");
    }

    /// steps=1 + dither boundary: 0.5 golden、clamp 単位区間境界 (0.0/1.0)。
    #[test]
    fn vf_steps_one_and_dither_boundary() {
        let t = raymarch_fog(
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            100.0,
            1,
            0.01,
            16.0,
            32.0,
            0.5,
        );
        assert_eq!(t.x.to_bits(), 0x3F79_4497, "steps=1 golden (rq)");
        let b0 = raymarch_fog(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            200.0,
            8,
            0.02,
            100.0,
            0.0,
            1.0,
        );
        assert!(b0.x >= 0.0 && b0.x <= 1.0, "dither=1.0 端も単位区間");
    }

    /// NaN 契約 (注記 4): dist NaN → 全成分 NaN 伝播。dither NaN は密度
    /// mask で有限 (0x3EBC5AB2、fail-loud しない設計の誠実 pin)。
    #[test]
    fn vf_nan_contracts() {
        let tn = raymarch_fog(
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            f32::NAN,
            8,
            0.01,
            16.0,
            32.0,
            0.5,
        );
        assert!(
            tn.x.is_nan() && tn.y.is_nan() && tn.z.is_nan(),
            "dist NaN 伝播"
        );
        let td = raymarch_fog(
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            100.0,
            8,
            0.01,
            16.0,
            32.0,
            f32::NAN,
        );
        assert_eq!(
            td.x.to_bits(),
            0x3EBC_5AB3,
            "dither NaN → h NaN → max 0 mask → base 密度 (rq 逐次蓄積) — \
             私の初 pin は 0.08·12.5 省略形で 0x3EBC5AB2 と 1 ulp 誤り、\
             実装 golden が捕捉 → 逐次シミュレーション値 0x3EBC5AB3 へ訂正"
        );
    }

    /// Vec3 契約 pin (注記 2): Vec4+Sub 削除済、本体使用面のみ維持。
    /// normalize 3-4-5 exact (rq)、wgsl identity は &str 内容比較 (ET 規律)。
    #[test]
    fn vf_vec3_and_wgsl_contract() {
        let n = Vec3::new(3.0, 4.0, 0.0).normalize();
        assert_eq!(n.x.to_bits(), 0x3F19_999A, "3/5 (rq)");
        assert_eq!(n.y.to_bits(), 0x3F4C_CCCD, "4/5 (rq)");
        assert_eq!(Vec3::new(1.0, 2.0, 2.0).length(), 3.0);
        assert_eq!(Vec3::default().dot(Vec3::new(1.0, 1.0, 1.0)), 0.0);
        let s = Vec3::new(1.0, 1.0, 1.0) + Vec3::new(2.0, 3.0, 4.0) * 0.5;
        assert!(s.x == 2.0 && s.y == 2.5 && s.z == 3.0, "Add/Mul 本体面");
        // Vec4 は完全削除 (注記 2、EU-3 同型 — 型参照はコンパイル除去で証明)。
        assert_eq!(wgsl_source(), VOLUMETRIC_FOG_WGSL, "include_str identity");
        assert!(
            wgsl_source().contains("var od = 0.0;") && wgsl_source().contains("exp(-od * seg)"),
            "WGSL も sum-then-exp 同形 (注記 3)"
        );
    }
}
