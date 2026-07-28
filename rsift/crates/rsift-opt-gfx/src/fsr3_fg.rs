//! FSR 3 Frame Generation — 実装: モーション誘導の中間フレーム補間。
//!
//! FSR3 の本質:
//! - prev/curr の color+depth+motion を入力に、α=0.5 の中間フレームを組み立てる
//! - 逆ワープ (curr → prev 空間) で両フレームを中間時刻へ引き戻す
//! - depth の不整合 = disocclusion → その箇所は curr 優先 (ゴースト防止)
//! - optical flow 不要の「motion vector + depth 整合」最小構成を正 API として提供
//!
//! CPU 参照実装 (動作検証用) + 本番 GPU 用 WGSL compute を一緒に収める。

/// 入力テクスチャ (CPU 参照用に Rgba8 / depth f32 / motion 2ch f16-as-f32)。
pub struct FrameInput {
    pub color: Vec<u32>, // RGBA8 pack
    pub depth: Vec<f32>,
    pub motion: Vec<[f32; 2]>, // ピクセル単位のみ (NDC 主義不要)
    pub width: u32,
    pub height: u32,
}

impl FrameInput {
    fn get_uv(&self, x: i32, y: i32) -> (u32, f32) {
        let x = x.clamp(0, self.width as i32 - 1) as u32;
        let y = y.clamp(0, self.height as i32 - 1) as u32;
        let i = (y * self.width + x) as usize;
        (self.color[i], self.depth[i])
    }
}

pub struct FrameInterpolator {
    /// 生成時刻 (0..1)。FSR3 CFG 既定では 0.5。
    pub alpha: f32,
    /// depth 不一致率がこの閾値超で disocclusion。
    pub depth_tolerance: f32,
    /// disocclusion 時の curr 寄せ下限。
    pub curr_bias_disocclusion: f32,
}

impl Default for FrameInterpolator {
    fn default() -> Self {
        Self { alpha: 0.5, depth_tolerance: 0.02, curr_bias_disocclusion: 1.0 }
    }
}

impl FrameInterpolator {
    /// 中間フレームを CPU で生成 (GPU 版と一致する参照実装)。
    ///
    /// **契約 (wave 159 FE 捕捉 85)**: 全入力バッファは `width*height` 長、
    /// `out` は `width*height` 以上であること (長さ違反は契約メッセージで
    /// fail-loud。旧実装は検査なしで深部の index OOB panic に流出)。
    pub fn interpolate_cpu(&self, prev: &FrameInput, curr: &FrameInput, out: &mut [u32]) {
        assert_eq!(prev.width, curr.width);
        assert_eq!(prev.height, curr.height);
        let px = curr.width as u64 * curr.height as u64;
        assert_eq!(
            curr.color.len() as u64,
            px,
            "契約: curr.color は width*height 長"
        );
        assert_eq!(
            curr.depth.len() as u64,
            px,
            "契約: curr.depth は width*height 長"
        );
        assert_eq!(
            curr.motion.len() as u64,
            px,
            "契約: curr.motion は width*height 長"
        );
        assert_eq!(
            prev.color.len() as u64,
            px,
            "契約: prev.color は width*height 長"
        );
        assert_eq!(
            prev.depth.len() as u64,
            px,
            "契約: prev.depth は width*height 長"
        );
        assert!(out.len() as u64 >= px, "契約: out は width*height 以上");
        let w = curr.width as i32;
        let h = curr.height as i32;
        let a = self.alpha;

        for y in 0..h {
            for x in 0..w {
                let i = (y as u32 * curr.width + x as u32) as usize;
                let mv = curr.motion[i];
                // 中間時刻 X から両方向へ引く
                let x_prev = x as f32 - mv[0] * a;
                let y_prev = y as f32 - mv[1] * a;
                let c_prev = bilinear_color(prev, x_prev, y_prev);
                let d_prev = bilinear_depth(prev, x_prev, y_prev);

                // 【捕捉 82 根治 (wave 159 FE)】旧実装はここで d_next =
                // bilinear_depth(prev.clone_shallow(), x_next, y_next) を
                // 計算して `let _ =` で即破棄していた — 消費者ゼロの死計算
                // (§7) で、恒等返却の clone_shallow もそのためだけの no-op
                // helper だった → 不可能証明の上両方削除
                // (GPU WGSL にも対応概念なし = 不変式層でも死)。
                let d_curr = curr.depth[i];
                let d_expect = lerp(d_prev, bilinear_depth(curr, x as f32, y as f32), a);
                let disoccluded = (d_curr - d_expect).abs() > self.depth_tolerance * d_curr.max(1e-3);

                let c_curr = curr.color[i];
                out[i] = if disoccluded {
                    // FSR3 FigmaFig: disocclusion curr 側に振る (残像を出さない)
                    blend_u32(c_curr, c_prev, 1.0 - self.curr_bias_disocclusion)
                } else {
                    lerp_u32(c_prev, c_curr, a)
                };
            }
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn bilinear_depth(inp: &FrameInput, fx: f32, fy: f32) -> f32 {
    let x0 = fx.floor() as i32;
    let y0 = fy.floor() as i32;
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let d00 = inp.get_uv(x0, y0).1;
    let d10 = inp.get_uv(x0 + 1, y0).1;
    let d01 = inp.get_uv(x0, y0 + 1).1;
    let d11 = inp.get_uv(x0 + 1, y0 + 1).1;
    lerp(lerp(d00, d10, tx), lerp(d01, d11, tx), ty)
}

fn bilinear_color(inp: &FrameInput, fx: f32, fy: f32) -> u32 {
    let x0 = fx.floor() as i32;
    let y0 = fy.floor() as i32;
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let c00 = inp.get_uv(x0, y0).0;
    let c10 = inp.get_uv(x0 + 1, y0).0;
    let c01 = inp.get_uv(x0, y0 + 1).0;
    let c11 = inp.get_uv(x0 + 1, y0 + 1).0;
    let top = lerp_u32(c00, c10, tx);
    let bot = lerp_u32(c01, c11, tx);
    lerp_u32(top, bot, ty)
}

/// u32 RGBA8 pack ごとの lerp (チャンネル独立)。
fn lerp_u32(a: u32, b: u32, t: f32) -> u32 {
    let mix = |sa: u32, sb: u32| -> u32 {
        let fa = sa as f32;
        let fb = sb as f32;
        (fa + (fb - fa) * t).round().clamp(0.0, 255.0) as u32
    };
    let aa = a as u8 as u32;
    let ab = b as u8 as u32;
    let ga = ((a >> 8) & 0xFF) as u32;
    let gb = ((b >> 8) & 0xFF) as u32;
    let ba = ((a >> 16) & 0xFF) as u32;
    let bb = ((b >> 16) & 0xFF) as u32;
    let pa = ((a >> 24) & 0xFF) as u32;
    let pb = ((b >> 24) & 0xFF) as u32;
    mix(aa, ab) | (mix(ga, gb) << 8) | (mix(ba, bb) << 16) | (mix(pa, pb) << 24)
}

/// ブレンド with `w`: out = (1-w)*a + w*b。
fn blend_u32(a: u32, b: u32, w: f32) -> u32 {
    lerp_u32(a, b, w)
}

/// 本番 GPU 用: FSR3 準拠の中間フレーム合成 compute (WGSL)。
/// エンジン側の run は `fsr3_fg_gpu.wgsl` 相当として同梱。
///
/// 【捕捉 81 根治 (wave 159 FE)】CPU 参照 (interpolate_cpu) との数学的同一
/// 語彙に修正。旧実装の 3 乖離: (1) motion 空間 — CPU は texel 単位
/// (`x − mv·a`) なのに WGSL は `uv − mv·a` の uv 単位で解像数倍の誤
/// サンプル (1920 幅で 8texel 動きが 7680texel/フレームと誤読、rq (6))、
/// (2) texel 中心 — CPU bilinear floor 系に対し WGSL は +0.5 中心が半
/// テクセルずれ、(3) curr_bias_disocclusion が uniform に未配管で disoc
/// 時curr 100% 固定 (CPU の bias 設定と乖離)。(1)(2) は
/// `(center − mv·a) / res2`、(3) は `select(a, cfg.curr_bias, disoc)` で
/// CPU 式と厳密一致 (uniform Fg0 は +1 field で 20B→32B パディング、
/// gpu_runtime は文字列登録のみで CPU 側 buffer サイズ契約と結合しない
/// — 結合時は Fg0 レイアウト全体を naga 突合対象とする将来注記)。
pub const FSR3_FG_WGSL: &str = r#"
struct Fg0 {
  w:u32, h:u32, alpha:f32, depth_tol:f32, curr_bias:f32,
}
@group(0) @binding(0) var<uniform> cfg: Fg0;
@group(0) @binding(1) var prev_color:  texture_2d<f32>;
@group(0) @binding(2) var curr_color:  texture_2d<f32>;
@group(0) @binding(3) var prev_depth:  texture_2d<f32>;
@group(0) @binding(4) var curr_depth:  texture_2d<f32>;
@group(0) @binding(5) var motion_tex:  texture_2d<f32>;
@group(0) @binding(6) var out_tex:    texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(7) var smp:         sampler;

@compute @workgroup_size(8,8) fn cs_main(@builtin(global_invocation_id) g: vec3<u32>) {
  let px = vec2<i32>(g.xy);
  let res = vec2<i32>(i32(cfg.w), i32(cfg.h));
  if (px.x >= res.x || px.y >= res.y) { return; }
  let res2 = vec2<f32>(res);
  // texel 中心 (+0.5)。CPU の bilinear floor 系と同一語彙 (捕捉 81-2)。
  let center = vec2<f32>(px) + vec2<f32>(0.5);
  let uv0 = center / res2;
  let mv = textureSampleLevel(motion_tex, smp, uv0, 0.0).rg; // texel 単位 (捕捉 81-1)
  let a = cfg.alpha;
  let c_prev = textureSampleLevel(prev_color, smp, (center - mv * a) / res2, 0.0);
  let c_curr = textureLoad(curr_color, px, 0);
  let d_prev = textureSampleLevel(prev_depth, smp, (center - mv * a) / res2, 0.0).r;
  let d_curr = textureLoad(curr_depth, px, 0).r;
  let disoc = abs(d_curr - mix(d_prev, d_curr, a)) > cfg.depth_tol * max(d_curr, 0.001);
  let w = select(a, cfg.curr_bias, disoc); // 捕捉 81-3: CPU の bias 語彙と一致
  textureStore(out_tex, px, mix(c_prev, c_curr, vec4<f32>(w)));
}
"#;

/// GPU 実行時に必要なバッファ大小計算。
///
/// 【捕捉 84 根治 (wave 159 FE)】旧実装は `width * height` を u32 で乗算し
/// てから u64 へ cast していたため、`w*h ≥ 2^32` (例: 65536²) で真の画素数
/// が 0 へ wrap し全長 0 のバッファサイズを静寂計上した → u64 昇格後乗算。
pub fn fsr3_required_buffers(width: u32, height: u32) -> (u64, u64, u64) {
    let px = width as u64 * height as u64;
    (px * 4, px * 4, px * 8) // color_prev+color_curr, depth_prev+depth_curr, motion
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_frame(c: u32, d: f32, w: u32, h: u32) -> FrameInput {
        FrameInput {
            color: vec![c; (w * h) as usize],
            depth: vec![d; (w * h) as usize],
            motion: vec![[0.0; 2]; (w * h) as usize],
            width: w,
            height: h,
        }
    }

    #[test]
    fn identical_frames_blend_identically() {
        let f = solid_frame(0x80403020, 0.5, 8, 8);
        let interp = FrameInterpolator::default();
        let mut out = vec![0u32; 64];
        interp.interpolate_cpu(&f, &f, &mut out);
        assert_eq!(out[27], 0x80403020);
    }

    #[test]
    fn disocclusion_prefers_curr() {
        let prev = solid_frame(0xFF000000, 0.5, 8, 8);
        let mut curr = solid_frame(0xFFFFFFFF, 0.5, 8, 8);
        // 前景深さだけ変えて disocclusion を誘発
        curr.depth[10] = 0.1;
        let interp = FrameInterpolator::default();
        let mut out = vec![0u32; 64];
        interp.interpolate_cpu(&prev, &curr, &mut out);
        assert_eq!(out[10], 0xFFFFFFFF); // curr側
    }

    /// 【wave 159 FE 捕捉 81 補強 pin】非 disoc 経路の warp+bilinear 厳密
    /// golden (整数厳密系): 横グラデーション byte0=16x、mv=[1,0]、a=0.5。
    /// warp: x_prev = x−0.5 → R = (16(x−1)+16x)/2 = 16x−8 (x≥1)、
    /// x=0 はクランプ 0。blend with curr (16x) で 16x−4。
    #[test]
    fn warp_bilinear_golden_row_exact() {
        let grad = |x: usize| (x as u32) * 16;
        let mk = || FrameInput {
            color: (0..64).map(|i| grad(i % 8)).collect(),
            depth: vec![0.5; 64],
            motion: vec![[1.0, 0.0]; 64],
            width: 8,
            height: 8,
        };
        let (prev, curr) = (mk(), mk());
        let mut out = vec![0u32; 64];
        FrameInterpolator::default().interpolate_cpu(&prev, &curr, &mut out);
        let expect = [0u32, 12, 28, 44, 60, 76, 92, 108]; // 16x−4
        for x in 0..8usize {
            assert_eq!(out[x], expect[x], "row0 x={x}: byte0 golden");
            assert_eq!(out[8 + x], expect[x], "row1 x={x}: y 次元は mv=0 で不変");
        }
    }

    /// alpha 端点は厳密にソースフレーム: 0 → prev、1 → curr (非 disoc)。
    #[test]
    fn alpha_endpoints_are_exact_source_frames() {
        let prev = FrameInput {
            color: (0..64).map(|i| i as u32 * 3 + 1).collect(),
            depth: vec![0.5; 64],
            motion: vec![[0.5, 0.25]; 64],
            width: 8,
            height: 8,
        };
        let curr = FrameInput {
            color: (0..64).map(|i| (255 - i) as u32).collect(),
            depth: vec![0.5; 64],
            motion: vec![[0.5, 0.25]; 64],
            width: 8,
            height: 8,
        };
        let mut out0 = vec![0u32; 64];
        FrameInterpolator {
            alpha: 0.0,
            ..Default::default()
        }
        .interpolate_cpu(&prev, &curr, &mut out0);
        assert_eq!(out0, prev.color, "alpha=0: out ≡ prev (warp 0 距離)");
        let mut out1 = vec![0u32; 64];
        FrameInterpolator {
            alpha: 1.0,
            ..Default::default()
        }
        .interpolate_cpu(&prev, &curr, &mut out1);
        assert_eq!(out1, curr.color, "alpha=1: out ≡ curr (t=1 で warp 無関係)");
    }

    /// disoc 閾値の境界等号は非 disoc (`>` 厳密)。dyadic 完全厳密系:
    /// d_prev=1/8, d_curr=1/4, a=1/2 → Δ = |dc−dp|·(1−a) = 1/16 = 0.0625。
    /// tol=1/4 → thr = 1/4·1/4 = 0.0625 ちょうど → 非 disoc (blend golden
    /// 0x88804422、round(135.5)=136 由来)、tol=1/8 → thr=0.03125 → disoc。
    #[test]
    fn disocclusion_threshold_boundary_equality_is_not_disoccluded() {
        let prev = solid_frame(0x10F0_0804, 0.125, 8, 8);
        let curr = solid_frame(0xFF10_8040, 0.25, 8, 8);
        let mut out_eq = vec![0u32; 64];
        FrameInterpolator {
            depth_tolerance: 0.25,
            ..Default::default()
        }
        .interpolate_cpu(&prev, &curr, &mut out_eq);
        assert!(
            out_eq.iter().all(|&o| o == 0x8880_4422),
            "境界等号は非 disoc: blend(0x04,0x40)=(4+64)/2=0x22 等 golden"
        );
        let mut out_gt = vec![0u32; 64];
        FrameInterpolator {
            depth_tolerance: 0.125,
            ..Default::default()
        }
        .interpolate_cpu(&prev, &curr, &mut out_gt);
        assert!(
            out_gt.iter().all(|&o| o == 0xFF10_8040),
            "閾値超過は disoc → curr 厳密 (bias 既定 1.0)"
        );
    }

    /// disoc 時の curr_bias 任意値 [0,1] の厳密 blend (bias=0.5 → 0.5 mix)。
    #[test]
    fn disocclusion_curr_bias_blends_exact_fraction() {
        let prev = solid_frame(0x10F0_0804, 0.125, 8, 8);
        let curr = solid_frame(0xFF10_8040, 0.25, 8, 8);
        let it = FrameInterpolator {
            depth_tolerance: 0.125,
            curr_bias_disocclusion: 0.5,
            ..Default::default()
        };
        let mut out = vec![0u32; 64];
        it.interpolate_cpu(&prev, &curr, &mut out);
        assert!(
            out.iter().all(|&o| o == 0x8880_4422),
            "bias=0.5: byte0 (64+4)/2=34, byte3 round(135.5)=136 → 0x88804422"
        );
    }

    /// fsr3_required_buffers: 1920×1080 golden + 捕捉 84 (u32 wrap) 根治 pin。
    #[test]
    fn required_buffers_golden_and_no_u32_wrap() {
        assert_eq!(
            fsr3_required_buffers(1920, 1080),
            (8_294_400, 8_294_400, 16_588_800),
            "2073600 px (rq fe_fsr3 (4))"
        );
        // 旧実装は `width * height` を u32 で乗算 → 65536² = 2^32 が 0 に
        // wrap し全長 0 のバッファを誤計上。u64 昇格後は真値 (rq (5))。
        assert_eq!(
            fsr3_required_buffers(65536, 65536),
            (17_179_869_184, 17_179_869_184, 34_359_738_368),
            "2^32 px: u32 wrap ではなく真値"
        );
    }

    /// 捕捉 85: バッファ長契約 fail-loud (短い motion は契約メッセージで拒絶)。
    #[test]
    #[should_panic(expected = "契約: curr.motion は width*height 長")]
    fn buffer_length_contract_motion_fail_loud() {
        let prev = solid_frame(0, 0.5, 8, 8);
        let mut curr = solid_frame(0, 0.5, 8, 8);
        curr.motion = vec![[0.0; 2]; 4];
        FrameInterpolator::default().interpolate_cpu(&prev, &curr, &mut vec![0u32; 64]);
    }

    /// 捕捉 85: out 短絡も契約メッセージ拒絶 (旧: index OOB panic のみ)。
    #[test]
    #[should_panic(expected = "契約: out は width*height 以上")]
    fn buffer_length_contract_out_fail_loud() {
        let prev = solid_frame(0, 0.5, 8, 8);
        let curr = solid_frame(0, 0.5, 8, 8);
        FrameInterpolator::default().interpolate_cpu(&prev, &curr, &mut vec![0u32; 8]);
    }

    /// 捕捉 81: GPU WGSL は CPU 参照と数学的同一語彙 (3 乖離の根治 pin)。
    #[test]
    fn wgsl_matches_cpu_reference_vocabulary() {
        assert!(FSR3_FG_WGSL.contains("curr_bias"), "uniform bias 配管");
        assert!(
            FSR3_FG_WGSL.contains("(center - mv * a) / res2"),
            "motion は texel 単位、texel 中心 +0.5 保持"
        );
        assert!(
            FSR3_FG_WGSL.contains("select(a, cfg.curr_bias, disoc)"),
            "disoc 時の bias 選択 (旧 select(a,1.0,_) 固定ではない)"
        );
        assert!(
            !FSR3_FG_WGSL.contains("uv - mv * a"),
            "旧 uv 空間 motion 式の残存禁止 (捕捉 81 回帰)"
        );
    }
}
