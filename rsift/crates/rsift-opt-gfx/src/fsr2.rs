//! FSR 2.0 — FidelityFX Super Resolution (temporal). Jitter + motion-vector
//! reprojection + history-dominant accumulation.
//!
//! Renders the scene at a lower internal resolution and reconstructs the display
//! resolution across frames. Pure shader math — no special hardware — so it
//! works on Intel Iris/Arc and Apple Silicon iGPUs. After FSR1, this is the
//! biggest quality/perf lever for integrated GPUs.
//!
//! 【wave 153 EY 監査注記】
//! 1. **捕捉 66 [中] (blend 反転、doc/FSR2 標準/実装・WGSL の三方不一致)**:
//!    旧 `resolve` は `current*a + history*(1-a)` (a=max(history_blend,
//!    disocclusion)) で a=0.9 = **current 90%・history 10%**。doc「history
//!    blend: Higher = more temporal stability」(= 高いほど history 寄り) とも、
//!    GPUOpen FSR2 公式「current frame は relatively low blend factor で
//!    混ぜる」(一次情報、time-travel 蓄積が本質) とも正反対で、WGSL 側も
//!    `select(0.95, 1.0, reset); mix(hist, cur, a)` で同じく current 95%
//!    逆転だった。根治: `history*h + current*(1-h)` (h=blend*(1−disocc))、
//!    既定 0.9 = history 90%、disocclusion=1 → h=0 = current 100% (reset
//!    整合)。WGSL は `select(0.9, 0.0, reset); mix(cur, hist, a)` と同形化。
//!    TDD RED 4 件 (dominant/stable/mid/NaN) 機械記録済。
//! 2. **捕捉 67 [小] (dims 4 フィールドが全経路未消費 + `let _ = reprojected`
//!    破棄)**: `Fsr2::new` は in/out 解像度を格納するのみで一切読まなかった
//!    (§7 未配線)。根治: `jitter_uv` (px → uv 正規化で input_w/h を実消費、
//!    wiring の `*0.002` ハードコード撲滅) + wiring で reprojected を report
//!    `fsr2_reproj_uv`、scale を `fsr2_scale` (= output/input、GPU Uniforms
//!    inRes/outRes と同一次元の監視値) へ実配線 (§7 消化 16)。
//! 3. **§7 不可能証明削除**: `neighborhood_clamp` (CPU 参照 fn) は wiring の
//!    現在帧データが単一 solid 値のみ (fsr1 reconstruct が 4 入力同一色の
//!    恒等演) で 3x3 AABB を構築する実データ源が存在せず、同一色×9 の
//!    擬似 AABB 接続は vacuous = 偽装禁止抵触 → 証明の上完全削除 (WGSL
//!    側の `clamp(hist, mn, mx)` は真データ経路として残存、注記 5)。
//!    `wgsl_source` メソッド (dead accessor、gpu_runtime 登録は const
//!    FSR2_WGSL 直接参照)・`Vec3::clamp`/`Vec3 Sub` impl (消費者ゼロ
//!    census) も同時削除 (vec_min/vec_max は clamp 削除に付随)。
//! 4. **契約注記 (到達不可性証明・規律変更)**: (a) d=NaN は f32::clamp が
//!    NaN を透過し結果 NaN 伝播 — 旧式は `blend.max(NaN)` が f32::max の
//!    片側 NaN 破棄で a=blend 静寂吸収していた (規律変更、wave 系統の NaN
//!    伝播契約に整合)。(b) `halton(i=0)` は max(1) で先頭値に矯正 (doc
//!    1-based 防御)。(c) `jitter(frame+1)` は frame=u32::MAX で +1 wrap
//!    debug panic しうるが、wiring は `frame_index as u32` (frame_index 実
//!    漸増、2^32 frame ≈ 60fps で 2.2 年) で到達不可。(d) jitter は
//!    halton<1 厳密ゆえ [−0.5, +0.5) の片側非対称 (+0.5 端は分布上到達
//!    しない、捕捉 59 系の分布注記として記録のみ)。
//! 5. **WGSL parity 注記**: WGSL は CPU 参照と同じ history-dominant 構造に
//!    同形化 (`select(0.9, 0.0, reset)` の 0.9 は CPU history_blend 既定と
//!    一致)。WGSL には加えて (i) 3x3 neighborhood clamp (真 texture 経路、
//!    CPU 参照 fn は注記 3 で削除)、(ii) 8-bit banding 崩しの dither hash
//!    (GPU のみ、CPU 経路には非対応 = 既知の系統差) がある。naga 自動
//!    validate (gpu_runtime) 連続 PASS、content pin (`select(0.9, 0.0` /
//!    `mix(cur, hist, a)` / `clamp(hist, mn, mx)` 含有) を module test で財産化。

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}
impl Vec3 {
    pub fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }
}
impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.r + o.r, self.g + o.g, self.b + o.b)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.r * s, self.g * s, self.b * s)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Fsr2 {
    pub input_w: usize,
    pub input_h: usize,
    pub output_w: usize,
    pub output_h: usize,
    /// Base EMA blend of **history** (0..1)。高いほど history 重みが大きく
    /// temporal stability が増す (捕捉 66 根治後の契約、GPUOpen FSR2 標準
    /// = current は low blend factor)。
    pub history_blend: f32,
    /// Jitter amplitude as a fraction of a pixel (0.5 = full pixel jitter).
    pub jitter_scale: f32,
}
impl Fsr2 {
    pub fn new(input_w: usize, input_h: usize, output_w: usize, output_h: usize) -> Self {
        Self {
            input_w,
            input_h,
            output_w,
            output_h,
            history_blend: 0.9,
            jitter_scale: 0.5,
        }
    }

    /// Halton sequence value for `i` (1-based) with the given `base`, in [0,1).
    pub fn halton(i: u32, base: u32) -> f32 {
        let mut f = 1.0f32;
        let mut r = 0.0f32;
        let mut n = i.max(1);
        while n > 0 {
            f /= base as f32;
            r += f * (n % base) as f32;
            n /= base;
        }
        r
    }

    /// Sub-pixel jitter offset (in pixels) for `frame` (0-based), in
    /// [-jitter_scale, +jitter_scale) for each axis (halton<1 厳密で +半端は
    /// 未到達、注記 4d)。
    pub fn jitter(&self, frame: u32) -> (f32, f32) {
        let x = Self::halton(frame + 1, 2) - 0.5;
        let y = Self::halton(frame + 1, 3) - 0.5;
        (x * 2.0 * self.jitter_scale, y * 2.0 * self.jitter_scale)
    }

    /// 【wave 153 EY-2 捕捉 67 根治】px 空間 jitter を uv 空間に正規化
    /// (input_w/h を実消費 = §7 消化 16 の dims 消費者)。旧 wiring の
    /// `*0.002` ハードコード (1/500 相当、640px 基準で 25% 過大) を根治。
    pub fn jitter_uv(&self, frame: u32) -> (f32, f32) {
        let (jx, jy) = self.jitter(frame);
        (
            jx * (1.0 / self.input_w as f32),
            jy * (1.0 / self.input_h as f32),
        )
    }

    /// Reproject a current-frame UV to previous-frame UV via a motion vector
    /// expressed in UV space (prevUV - curUV).
    pub fn reproject(&self, uv: (f32, f32), mv: (f32, f32)) -> (f32, f32) {
        let px = (uv.0 + mv.0).clamp(0.0, 1.0);
        let py = (uv.1 + mv.1).clamp(0.0, 1.0);
        (px, py)
    }

    /// 【wave 153 EY-1 捕捉 66 根治】Resolve: **history-dominant** EMA
    /// (doc「stability」/GPUOpen FSR2 標準整合)。h = history_blend *
    /// (1−disocclusion): 既定 0.9 = history 90%。disocclusion=1 (no usable
    /// history) → h=0 → current 100% (reset 整合、WGSL 同形)。旧式
    /// `current*a + history*(1-a)` (a=max(blend, disoc)) は current 90%
    /// 逆転で doc と正反対だった (TDD RED 4 機械記録)。
    pub fn resolve(&self, current: Vec3, history: Vec3, disocclusion: f32) -> Vec3 {
        let h = self.history_blend * (1.0 - disocclusion.clamp(0.0, 1.0));
        history * h + current * (1.0 - h)
    }
}

pub const FSR2_WGSL: &str = include_str!("../shaders/fsr2.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    fn solid(v: f32) -> Vec3 {
        Vec3::new(v, v, v)
    }
    #[test]
    fn halton_in_unit() {
        for i in 1..16u32 {
            let h = Fsr2::halton(i, 2);
            assert!((0.0..1.0).contains(&h), "halton out of range");
        }
    }
    #[test]
    fn halton_deterministic() {
        assert!((Fsr2::halton(1, 2) - 0.5).abs() < 1e-6);
        assert!((Fsr2::halton(2, 2) - 0.25).abs() < 1e-6);
    }
    #[test]
    fn jitter_in_range() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        for fr in 0..8u32 {
            let (x, y) = f.jitter(fr);
            assert!(x.abs() <= f.jitter_scale + 1e-6);
            assert!(y.abs() <= f.jitter_scale + 1e-6);
        }
    }
    #[test]
    fn reproject_moves() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let p = f.reproject((0.5, 0.5), (-0.1, 0.02));
        assert!((p.0 - 0.4).abs() < 1e-6);
        assert!((p.1 - 0.52).abs() < 1e-6);
    }
    #[test]
    fn resolve_disocclusion_uses_current() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let r = f.resolve(solid(0.7), solid(0.1), 1.0);
        assert!((r.r - 0.7).abs() < 1e-6);
    }
    #[test]
    fn resolve_stable_blends() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let r = f.resolve(solid(0.6), solid(0.4), 0.0);
        // 【wave 153 EY-1 捕捉 66 根治後 golden】h=0.9 history 重み:
        // 0.4*0.9 + 0.6*(1-0.9) = 0x3ED70A3E (rq ey_fsr2)。旧式は反転
        // (current 0.9 → 0x3F147AE2) で doc「stability」と FSR2 標準に矛盾。
        assert_eq!(r.r.to_bits(), 0x3ED70A_3Eu32, "blends (rq ey_fsr2)");
    }

    /// 【wave 153 EY-1 捕捉 66】doc「history_blend: Higher = more temporal
    /// stability」と GPUOpen FSR2 公式 (current は low blend factor) に
    /// 対し、旧実装は current*a+history*(1-a) で a=0.9 = current 90% と
    /// **正反対**。cur=0/hist=1/d=0 → history 重み 0.9 (0x3F666666) を pin。
    #[test]
    fn resolve_history_dominant_capture66_strict() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let r = f.resolve(solid(0.0), solid(1.0), 0.0);
        assert_eq!(
            r.r.to_bits(),
            0x3F66_6666u32,
            "blend 0.9 は history 重み (doc/FSR2 標準、rq ey_fsr2)、旧式は 0x3DCCCCD0 (0.1 系反転)"
        );
    }

    /// 【wave 153 EY-1】中間 disocclusion golden: d=0.5 → h=0.9*0.5
    /// (exact halving 0x3EE66666)、hist=0.2/cur=0.8 → 0x3F07AE15 (rq)。
    #[test]
    fn resolve_mid_disocclusion_golden_strict() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let r = f.resolve(solid(0.8), solid(0.2), 0.5);
        assert_eq!(r.r.to_bits(), 0x3F07_AE15u32, "d=0.5 (rq ey_fsr2)");
    }

    /// 【wave 153 EY-1】d=NaN は NaN 伝播 (規律変更: 旧式は
    /// `blend.max(NaN)` で f32::max が NaN を捨て a=0.9 静寂吸収)。
    /// NaN 伝播規約は wave 系統の契約に整合 (捕捉 57 系)。
    #[test]
    fn resolve_disocclusion_nan_propagates_pin() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let r = f.resolve(solid(0.8), solid(0.2), f32::NAN);
        assert!(
            r.r.is_nan() && r.g.is_nan() && r.b.is_nan(),
            "NaN は伝播 (静寂吸収撲滅)"
        );
    }

    /// 【wave 153 EY-1】d>1 の clamp: d=2.0 → 1.0 に clamp → h=0 →
    /// current 100% (0x3F333333=0.7、rq ey_fsr2)。
    #[test]
    fn resolve_disocclusion_above_one_clamps_pin() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let r = f.resolve(solid(0.7), solid(0.1), 2.0);
        assert_eq!(r.r.to_bits(), 0x3F33_3333u32, "d>1 → current (rq ey_fsr2)");
    }

    /// 【wave 153 EY-2】jitter の bit 厳密 golden (rq ey_fsr2):
    /// jitter_scale=0.5 なので x*2*0.5 は f32 恒等 → j=halton−0.5。
    /// frame0: (0, 0xBE2AAAAA)、frame1: (0xBE800000, 0x3E2AAAAC)。
    #[test]
    fn jitter_bit_golden_frames_strict() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let (x0, y0) = f.jitter(0);
        assert_eq!(
            (x0.to_bits(), y0.to_bits()),
            (0x0000_0000u32, 0xBE2A_AAAAu32),
            "frame0 (rq)"
        );
        let (x1, y1) = f.jitter(1);
        assert_eq!(
            (x1.to_bits(), y1.to_bits()),
            (0xBE80_0000u32, 0x3E2A_AAACu32),
            "frame1 (rq)"
        );
    }

    /// 【wave 153 EY-2 捕捉 67 根治 pin】jitter_uv は dims で正規化
    /// (input_w/h 実消費): frame1 → mvx=0xB9CCCCCD, mvy=0x39F2B9D9
    /// (rq ey_fsr2、旧 0.002 ハードコード 1/500 は 640px 基準 25% 過大)。
    #[test]
    fn jitter_uv_dims_golden_strict() {
        let f = Fsr2::new(640, 360, 1280, 720);
        let (mx, my) = f.jitter_uv(1);
        assert_eq!(
            (mx.to_bits(), my.to_bits()),
            (0xB9CC_CCCDu32, 0x39F2_B9D9u32),
            "dims 正規化 golden (rq ey_fsr2)"
        );
    }

    /// 【wave 153 EY-1 契約 pin】halton の i=0 は max(1) で 1 化
    /// (doc 1-based の防御、frame_index u64→u32 潰れ wrap 0 でも
    /// 先頭値に収まる)。wrap 注記は doc 注記 4c 参照。
    #[test]
    fn halton_index_zero_coerces_to_one_pin() {
        assert_eq!(Fsr2::halton(0, 2).to_bits(), Fsr2::halton(1, 2).to_bits());
        assert_eq!(Fsr2::halton(0, 3).to_bits(), Fsr2::halton(1, 3).to_bits());
    }

    /// 【wave 153 EY-1 境界 pin】reproject の clamp 飽和: mv が範囲外なら
    /// [0,1] 端に exact 飽和 (0.0/1.0 は f32 厳密値)。
    #[test]
    fn reproject_saturates_golden_strict() {
        let f = Fsr2::new(960, 540, 1920, 1080);
        let p = f.reproject((0.5, 0.5), (-0.6, 0.9));
        assert_eq!(
            (p.0.to_bits(), p.1.to_bits()),
            (0u32, 0x3F80_0000u32),
            "clamp 境界 exact"
        );
    }

    /// 【wave 153 EY-1/EY-3 WGSL parity pin】捕捉 66 根治の GPU 側同形・
    /// WGSL 真経路の neighborhood clamp・naga validate 通過の内容 pin。
    #[test]
    fn fsr2_wgsl_history_dominant_contract_pin() {
        assert!(
            FSR2_WGSL.contains("select(0.9, 0.0, u.reset > 0.5)"),
            "WGSL も history-dominant (0.9) 同形 (捕捉 66 parity)"
        );
        assert!(
            FSR2_WGSL.contains("mix(cur, hist, a)"),
            "reset → a=0 で pure current、通常は hist 90%"
        );
        assert!(
            FSR2_WGSL.contains("clamp(hist, mn, mx)"),
            "GPU 真経路の neighborhood clamp 残存 (注記 3/5)"
        );
    }

    /// 【wave 185 GE フェーズ2 回収】dead code 系 5 例目 (wave 153 EY adversarial (b)
    /// 削除系 4 構造 (CPU 側 neighborhood clamp / wgsl_source / Vec3::clamp / Sub+
    /// vec_min/max) 復活 非検出、不可能証明削除済) の lexeme pin 化。GPU 真経路の
    /// WGSL 側 clamp は別資産に残存 (注記 3/5)。同宣言形の将来復活を静寂に通さない。
    #[test]
    fn ge_removed_fsr2_helpers_lexeme() {
        let src = include_str!("fsr2.rs");
        for lex in [
            concat!("fn neighborhood", "_clamp"),
            concat!("fn wgsl_", "source"),
            concat!("fn cl", "amp"),
            concat!("fn vec_", "min"),
            concat!("fn vec_", "max"),
        ] {
            assert!(
                !src.contains(lex),
                "dead code 系削除語彙の宣言形復活を検出 (wave 185 GE lexeme pin)"
            );
        }
    }
}
