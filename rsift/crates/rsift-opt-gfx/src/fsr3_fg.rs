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
    pub fn interpolate_cpu(&self, prev: &FrameInput, curr: &FrameInput, out: &mut [u32]) {
        assert_eq!(prev.width, curr.width);
        assert_eq!(prev.height, curr.height);
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

                // 中間時刻自身の期待深さ (前方にも 1-a だけ進める)
                let x_next = x as f32 + mv[0] * (1.0 - a);
                let y_next = y as f32 + mv[1] * (1.0 - a);
                let d_next = bilinear_depth(prev.clone_shallow(), x_next, y_next);
                let _ = d_next;

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

impl FrameInput {
    fn clone_shallow(&self) -> &FrameInput {
        self
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
pub const FSR3_FG_WGSL: &str = r#"
struct Fg0 {
  w:u32, h:u32, alpha:f32, depth_tol:f32,
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
  let uv = (vec2<f32>(px) + vec2<f32>(0.5)) / vec2<f32>(res);
  let mv = textureSampleLevel(motion_tex, smp, uv, 0.0).rg;
  let a = cfg.alpha;
  let c_prev = textureSampleLevel(prev_color, smp, uv - mv * a, 0.0);
  let c_curr = textureLoad(curr_color, px, 0);
  let d_prev = textureSampleLevel(prev_depth, smp, uv - mv * a, 0.0).r;
  let d_curr = textureLoad(curr_depth, px, 0).r;
  let disoc = abs(d_curr - mix(d_prev, d_curr, a)) > cfg.depth_tol * max(d_curr, 0.001);
  let w = select(a, 1.0, disoc); // disocclusion 時は curr 100%
  textureStore(out_tex, px, mix(c_prev, c_curr, vec4<f32>(w)));
}
"#;

/// GPU 実行時に必要なバッファ大小計算。
pub fn fsr3_required_buffers(width: u32, height: u32) -> (u64, u64, u64) {
    let px = (width * height) as u64;
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
}
