//! MP4 export — write real frames then ffmpeg. Fail-loud if pixels/ffmpeg missing.
//!
//! Blur 2 経路 (倍発火回避の利用側契約): ① OfflineRenderer.motion_blur_samples
//! (subframe 位相の真 shutter 積分・FFI rsreplay_export_mp4 が配線済) ②
//! ExportSettings.motion_blur (post-hoc box 近似・apply_motion_blur、crate 直接
//! 利用向け)。両者の同時有効は二重ブラーとなる (既定は双方 0 = 無効)。

use crate::playback::PlaybackEngine;
use crate::renderer::{OfflineRenderer, RenderedFrame};
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{info, warn};

pub struct ExportSettings {
    pub output_fps: u32,
    pub quality_crf: u8,
    pub codec: ExportCodec,
    pub motion_blur: f32,
}

#[derive(Debug, Clone, Copy)]
pub enum ExportCodec {
    H264,
    H265,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            output_fps: 60,
            quality_crf: 18,
            codec: ExportCodec::H264,
            motion_blur: 0.0,
        }
    }
}

pub struct Mp4Exporter {
    pub settings: ExportSettings,
}

impl Default for Mp4Exporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Mp4Exporter {
    pub fn new() -> Self {
        Self {
            settings: ExportSettings::default(),
        }
    }

    pub fn export(
        &self,
        playback: &PlaybackEngine,
        renderer: &mut OfflineRenderer,
        output_path: &Path,
    ) -> Result<(), String> {
        let duration_us = playback
            .metadata
            .as_ref()
            .map(|m| m.duration_us)
            .unwrap_or(5_000_000);
        let total_frames = renderer.total_export_frames(duration_us).min(300);
        if total_frames == 0 {
            return Err("export refused: zero frames".into());
        }
        info!(
            "[RsReplay] Exporting {} frames @ {}fps → {:?}",
            total_frames, self.settings.output_fps, output_path
        );

        let temp_dir = output_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("_rsreplay_frames");
        std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;

        let mut wrote = 0u32;
        let blur_r = if self.settings.motion_blur == 0.0 {
            0
        } else {
            blur_radius(self.settings.motion_blur)
        };
        if blur_r == 0 {
            for i in 0..total_frames {
                let timestamp_us = i * 1_000_000 / self.settings.output_fps.max(1) as u64;
                let ui_snap = playback.eval_first_person_ui(timestamp_us);
                let frame = renderer.render_first_person_frame(i, &ui_snap);
                if frame.rgba.is_empty() {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return Err("export refused: empty RGBA frame".into());
                }
                let tga = temp_dir.join(format!("frame_{i:06}.tga"));
                frame.write_tga(&tga)?;
                wrote += 1;
            }
        } else {
            // post-hoc blur 経路: リングに 2r+1 フレームのみ保持し中心が揃い次第
            // TGA 書出し (全フレーム滞留を避ける低スペック配慮・注記 4)。
            let n = total_frames as usize;
            let win_cap = 2 * blur_r + 1;
            let mut ring: std::collections::VecDeque<RenderedFrame> =
                std::collections::VecDeque::with_capacity(win_cap);
            for i in 0..total_frames {
                let timestamp_us = i * 1_000_000 / self.settings.output_fps.max(1) as u64;
                let ui_snap = playback.eval_first_person_ui(timestamp_us);
                let frame = renderer.render_first_person_frame(i, &ui_snap);
                if frame.rgba.is_empty() {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return Err("export refused: empty RGBA frame".into());
                }
                ring.push_back(frame);
                if ring.len() > win_cap {
                    ring.pop_front();
                }
                let ci = i as i64 - blur_r as i64;
                if ci >= 0 {
                    let c = ci as usize;
                    emit_blurred_center(
                        &ring,
                        c.saturating_sub(blur_r),
                        i as usize,
                        c,
                        self.settings.motion_blur,
                        &temp_dir,
                    )?;
                    wrote += 1;
                }
            }
            for c in n.saturating_sub(blur_r)..n {
                emit_blurred_center(
                    &ring,
                    c.saturating_sub(blur_r),
                    n - 1,
                    c,
                    self.settings.motion_blur,
                    &temp_dir,
                )?;
                wrote += 1;
            }
        }
        if wrote == 0 {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err("export refused: no frames written".into());
        }

        match self.try_ffmpeg_encode(&temp_dir, output_path) {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&temp_dir);
                info!("[RsReplay] MP4 export complete: {:?}", output_path);
                Ok(())
            }
            Err(e) => {
                warn!("[RsReplay] ffmpeg failed: {e} — leaving TGA frames in {:?}", temp_dir);
                Err(format!(
                    "MP4 encode failed ({e}). Wrote {wrote} TGA frames to {:?} — not claiming MP4 success",
                    temp_dir
                ))
            }
        }
    }

    fn try_ffmpeg_encode(&self, frames_dir: &Path, output: &Path) -> Result<(), String> {
        let codec = match self.settings.codec {
            ExportCodec::H264 => "libx264",
            ExportCodec::H265 => "libx265",
        };
        // Convert TGA sequence; ffmpeg reads image2.
        let pattern = frames_dir.join("frame_%06d.tga");
        let status = Command::new("ffmpeg")
            .args([
                "-y",
                "-framerate",
                &self.settings.output_fps.to_string(),
                "-i",
                pattern.to_str().unwrap_or("frame_%06d.tga"),
                "-c:v",
                codec,
                "-crf",
                &self.settings.quality_crf.to_string(),
                "-pix_fmt",
                "yuv420p",
                output.to_str().unwrap_or("out.mp4"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("ffmpeg spawn: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("ffmpeg exit {:?}", status.code()))
        }
    }
}

/// post-hoc 時間方向モーションブラー の対称半径上限 (フレーム)。strength=1.0 で ±4。
pub const MAX_BLUR_RADIUS: usize = 4;

/// strength (0.0..=1.0、両端外は clamp) を対称半径 0..=MAX_BLUR_RADIUS へ写像。
/// 半値は持ち上げ (f32::round = half away from zero、0.125→1 は rq rp_mb で golden 化)。
/// 非有限 strength は契約違反で fail-loud panic (旧来は空関数で全入力を静寂呑み)。
pub fn blur_radius(strength: f32) -> usize {
    assert!(
        strength.is_finite(),
        "blur_radius 契約違反: strength が非有限 ({strength})"
    );
    (strength.clamp(0.0, 1.0) * MAX_BLUR_RADIUS as f32).round() as usize
}

/// post-hoc 時間方向モーションブラー (box カーネル一様露出の区級近似)。§7 修繕として
/// 旧空関数スタブを本実装し ExportSettings.motion_blur と共に export() へ実配線 (RP-1)。
///
/// 契約 (全て rq rp_mb で厳密導出・テスト pin):
/// - 各出力 out[i] = mean(frames[max(0,i-r)..=min(n-1,i+r)])、箱型一様重み、
///   端では利用フレームのみで再正規化 (重み和 1 を維持)。
/// - 丸め (sum + count/2)/count の整数 half-up、全チャンネル同一規則。
///   sum 最大 9*255=2295 < 256*9 より u8 飽和不要 (rq 証明 assert)。
/// - in-place でも汚染なし: 生値を全複製してから書戻す (注記 2)。
/// - 全フレームの width/height/rgba.len 一致を fail-loud assert、空・r=0 は no-op。
/// 誠実注記: 真の shutter 積分は OfflineRenderer.motion_blur_samples 経路が上位
/// (module doc 参照)。本関数はレンダ後 RGBA 列への近似フィルタで、renderer 経路と
/// 同時有効にすると二重ブラー。現行 FFI は exporter 側 strength を変更しない
/// (既定 0.0) ため GUI 経由出力では非発火 = crate 直接利用向け制御。
pub fn apply_motion_blur(frames: &mut [RenderedFrame], strength: f32) {
    if frames.is_empty() {
        return;
    }
    let r = blur_radius(strength);
    if r == 0 {
        return;
    }
    let w0 = frames[0].width;
    let h0 = frames[0].height;
    let npx = frames[0].rgba.len();
    for (i, f) in frames.iter().enumerate() {
        assert!(
            f.width == w0 && f.height == h0 && f.rgba.len() == npx,
            "apply_motion_blur 契約違反: frame {i} の寸法/バッファ長が frame 0 と不均一"
        );
    }
    let srcs: Vec<Vec<u8>> = frames.iter().map(|f| f.rgba.clone()).collect();
    let n = frames.len();
    for (i, f) in frames.iter_mut().enumerate() {
        let lo = i.saturating_sub(r);
        let hi = (i + r).min(n - 1);
        let count = (hi - lo + 1) as u32;
        let half = count / 2;
        for idx in 0..npx {
            let mut sum: u32 = 0;
            for s in &srcs[lo..=hi] {
                sum += s[idx] as u32;
            }
            f.rgba[idx] = ((sum + half) / count) as u8;
        }
    }
}

fn clone_frame(f: &RenderedFrame) -> RenderedFrame {
    RenderedFrame {
        width: f.width,
        height: f.height,
        rgba: f.rgba.clone(),
        frame_index: f.frame_index,
    }
}

/// リング (frame_index 連続) から [lo, hi] 閉区間を複製して post-hoc blur を適用し、
/// 中心フレームだけを TGA 書出しする export blur 経路のストリーミング段。
/// 窓全体を blur して中心のみ使う非効率は許容 (r ≤ 4 で最大 9 倍、注記 4)。
fn emit_blurred_center(
    ring: &std::collections::VecDeque<RenderedFrame>,
    lo: usize,
    hi: usize,
    center: usize,
    strength: f32,
    temp_dir: &Path,
) -> Result<(), String> {
    let first = ring
        .front()
        .ok_or_else(|| "blur window empty".to_string())?
        .frame_index as usize;
    let mut win: Vec<RenderedFrame> = ring
        .range((lo - first)..=(hi - first))
        .map(clone_frame)
        .collect();
    apply_motion_blur(&mut win, strength);
    let out = &win[center - lo];
    let tga = temp_dir.join(format!("frame_{:06}.tga", out.frame_index));
    out.write_tga(&tga)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v を全画素値に持つ 1x1 RGBA フレーム (alpha=255 固定)。
    fn frame1(v: u8) -> RenderedFrame {
        RenderedFrame {
            width: 1,
            height: 1,
            rgba: vec![v, 0, 0, 255],
            frame_index: 0,
        }
    }

    /// n=5 ramp (v = i*40)・strength 0.25 → r=1 の box 正規化 mean (端は利用フレームで再正規化)。
    #[test]
    fn mb_r1_box_exact_ramp() {
        let mut frames: Vec<RenderedFrame> = (0..5u8).map(|i| frame1(i * 40)).collect();
        apply_motion_blur(&mut frames, 0.25);
        let got: Vec<u8> = frames.iter().map(|f| f.rgba[0]).collect();
        assert_eq!(got, vec![20, 40, 80, 120, 140], "rq rp_mb 導出 r=1 golden");
    }

    /// 同 ramp・strength 0.5 → r=2。
    #[test]
    fn mb_r2_box_exact_ramp() {
        let mut frames: Vec<RenderedFrame> = (0..5u8).map(|i| frame1(i * 40)).collect();
        apply_motion_blur(&mut frames, 0.5);
        let got: Vec<u8> = frames.iter().map(|f| f.rgba[0]).collect();
        assert_eq!(got, vec![40, 60, 80, 100, 120], "rq rp_mb 導出 r=2 golden");
    }

    /// 定数フレーム列は不変 (box 正規化の恒等式)。
    #[test]
    fn mb_constant_frames_identity() {
        let mut frames: Vec<RenderedFrame> = (0..5).map(|_| frame1(200)).collect();
        apply_motion_blur(&mut frames, 1.0);
        for f in &frames {
            assert_eq!(f.rgba, vec![200, 0, 0, 255]);
        }
    }

    /// 丸め規則: (sum + count/2)/count = 0.5 半値持ち上げ (rq pin: hmean(1,2)==1)。
    #[test]
    fn mb_rounding_half_up() {
        let mut frames = vec![frame1(0), frame1(1)];
        apply_motion_blur(&mut frames, 1.0);
        assert_eq!(frames[0].rgba[0], 1, "0.5 → 1 (half-up)");
        assert_eq!(frames[1].rgba[0], 1);
    }

    /// in-place でも生値 (複製) から計算することの検査: [100,0,100] r=1 center = 67。
    /// 汚染実装 (左の blur 済み値を読む) なら center = 50 になる (rq で弁別導出)。
    #[test]
    fn mb_no_contamination_inplace_safe() {
        let mut frames = vec![frame1(100), frame1(0), frame1(100)];
        apply_motion_blur(&mut frames, 0.25);
        assert_eq!(frames[1].rgba[0], 67, "hmean(200,3)=67 (rq)");
        assert_eq!(frames[0].rgba[0], 50, "(100+0 +1)/2 = 50");
    }

    /// alpha=255 は任意 count で saturated-invariant: (count*255 + count/2)/count == 255 (rq)。
    #[test]
    fn mb_alpha_255_invariant() {
        let mut frames: Vec<RenderedFrame> = (0..9).map(|i| frame1(i as u8)).collect();
        apply_motion_blur(&mut frames, 1.0);
        for f in &frames {
            assert_eq!(f.rgba[3], 255, "alpha 不変");
        }
    }

    #[test]
    fn mb_strength_zero_noop() {
        let mut frames = vec![frame1(3), frame1(9)];
        apply_motion_blur(&mut frames, 0.0);
        assert_eq!(frames[0].rgba[0], 3);
        assert_eq!(frames[1].rgba[0], 9);
    }

    #[test]
    fn mb_empty_slice_noop() {
        let mut frames: Vec<RenderedFrame> = vec![];
        apply_motion_blur(&mut frames, 0.8);
        assert!(frames.is_empty());
    }

    #[test]
    #[should_panic(expected = "strength が非有限")]
    fn mb_nan_strength_panics() {
        let mut frames = vec![frame1(0)];
        apply_motion_blur(&mut frames, f32::NAN);
    }

    #[test]
    #[should_panic(expected = "不均一")]
    fn mb_hetero_dims_panics() {
        let a = frame1(1);
        let mut b = frame1(2);
        b.width = 2;
        b.rgba = vec![0; 8];
        let mut frames = vec![a, b];
        apply_motion_blur(&mut frames, 0.5);
    }

    /// §7 配線 golden: Mp4Exporter::export() が ExportSettings.motion_blur > 0 で
    /// TGA 出力へ post-hoc box blur を実適用すること。
    /// 64x64・fps=1 (5 フレーム)・strength 0.5 → r=2:
    ///   源パターン (1,1) r-ch = 3 ^ phase(frame) = [3,2,1,0,7] (rq 導出)
    ///   out[0]=mean(f0..f2)=2・out[1]=mean(f0..f3)=2・out[2]=mean(f0..f4)=3 (half-up)
    ///   out[3]=mean(f1..f4)=3・out[4]=mean(f2..f4)=3, g=3・b=80 不変。
    /// ffmpeg 非存在環境では export() は Err で TGA を残す → 画素検査。
    /// ffmpeg 存在環境 (りリース含む) では Ok で TGA 削除のため検査を skip (誠実注記)。
    #[test]
    fn mb_export_wiring_blur_golden_frames() {
        let dir = std::env::temp_dir().join("rsreplay_mb_wiring_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.mp4");
        let playback = crate::playback::PlaybackEngine::new();
        let mut renderer = crate::renderer::OfflineRenderer::new();
        renderer.settings.width = 64;
        renderer.settings.height = 64;
        renderer.settings.output_fps = 1;
        let mut exporter = Mp4Exporter::new();
        exporter.settings.motion_blur = 0.5; // → r=2 (rq 半径写像)
        exporter.settings.output_fps = 1;
        let res = exporter.export(&playback, &mut renderer, &out);
        if res.is_ok() {
            let _ = std::fs::remove_dir_all(&dir);
            return; // ffmpeg 存在環境: encode 成功で TGA 除去済み (検査不能・誠実 skip)
        }
        let err = res.unwrap_err();
        assert!(
            err.contains("ffmpeg"),
            "想定外エラー (ffmpeg 起因以外は失敗): {err}"
        );
        let tdir = dir.join("_rsreplay_frames");
        {
            // TGA 18B 標準ヘッダ pin (RP-2: 旧 20B 非標準の回帰防止)
            let head = std::fs::read(tdir.join("frame_000000.tga")).unwrap();
            assert_eq!(head.len(), 18 + 64 * 64 * 3, "TGA 18B ヘッダ + 64x64x3");
            assert_eq!(head[12], 64, "TGA width LE lo");
            assert_eq!(head[13], 0, "TGA width LE hi");
            assert_eq!(head[16], 24, "TGA bpp");
        }
        let px = |idx: u32, x: u32, y: u32| -> [u8; 3] {
            let bytes = std::fs::read(tdir.join(format!("frame_{idx:06}.tga"))).unwrap();
            let row = 63 - y; // TGA bottom-up
            let off = 18 + ((row * 64 + x) * 3) as usize;
            [bytes[off], bytes[off + 1], bytes[off + 2]]
        };
        assert_eq!(px(0, 1, 1), [80, 3, 2], "out[0] mean(f0..f2) rq golden");
        assert_eq!(
            px(1, 1, 1),
            [80, 3, 2],
            "out[1] mean(f0..f3) half-up (6+2)/4"
        );
        assert_eq!(
            px(2, 1, 1),
            [80, 3, 3],
            "out[2] mean(f0..f4) half-up (13+2)/5"
        );
        assert_eq!(
            px(3, 1, 1),
            [80, 3, 3],
            "out[3] mean(f1..f4) half-up (10+2)/4"
        );
        assert_eq!(px(4, 1, 1), [80, 3, 3], "out[4] mean(f2..f4) (8+1)/3");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 既定 (strength 0.0) はブラー非発火: TGA は生パターン (frame i の r = 3^i)。
    /// 配線が既定経路を侵蝕しないことの pin (FFI rsreplay_export_mp4 経路と同形状)。
    #[test]
    fn mb_export_wiring_default_off_raw_frames() {
        let dir = std::env::temp_dir().join("rsreplay_mb_wiring_off_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.mp4");
        let playback = crate::playback::PlaybackEngine::new();
        let mut renderer = crate::renderer::OfflineRenderer::new();
        renderer.settings.width = 64;
        renderer.settings.height = 64;
        renderer.settings.output_fps = 1;
        let mut exporter = Mp4Exporter::new();
        exporter.settings.output_fps = 1; // motion_blur は既定 0.0
        let res = exporter.export(&playback, &mut renderer, &out);
        if res.is_ok() {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let err = res.unwrap_err();
        assert!(err.contains("ffmpeg"), "想定外エラー: {err}");
        let tdir = dir.join("_rsreplay_frames");
        {
            // TGA 18B 標準ヘッダ pin (RP-2: 旧 20B 非標準の回帰防止)
            let head = std::fs::read(tdir.join("frame_000000.tga")).unwrap();
            assert_eq!(head.len(), 18 + 64 * 64 * 3, "TGA 18B ヘッダ + 64x64x3");
            assert_eq!(head[12], 64, "TGA width LE lo");
            assert_eq!(head[13], 0, "TGA width LE hi");
            assert_eq!(head[16], 24, "TGA bpp");
        }
        let px = |idx: u32, x: u32, y: u32| -> [u8; 3] {
            let bytes = std::fs::read(tdir.join(format!("frame_{idx:06}.tga"))).unwrap();
            let row = 63 - y;
            let off = 18 + ((row * 64 + x) * 3) as usize;
            [bytes[off], bytes[off + 1], bytes[off + 2]]
        };
        assert_eq!(px(0, 1, 1), [80, 3, 3], "f0 生パターン (3^0=3)");
        assert_eq!(px(2, 1, 1), [80, 3, 1], "f2 生パターン (3^2=1)");
        assert_eq!(px(4, 1, 1), [80, 3, 7], "f4 生パターン (3^4=7)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 半径写像 golden (rq rp_mb 導出) + 既定 strength 0.0 = オフ pin。
    #[test]
    fn mb_radius_mapping_and_default_off() {
        assert_eq!(blur_radius(0.0), 0);
        assert_eq!(blur_radius(0.124), 0);
        assert_eq!(blur_radius(0.125), 1, "0.5→1 half-away-from-zero (rq)");
        assert_eq!(blur_radius(0.25), 1);
        assert_eq!(blur_radius(0.5), 2);
        assert_eq!(blur_radius(0.75), 3);
        assert_eq!(blur_radius(1.0), 4);
        assert_eq!(blur_radius(1.4), 4, "clamp");
        assert_eq!(blur_radius(-0.3), 0, "負は clamp で 0 = off");
        let e = Mp4Exporter::new();
        assert_eq!(
            e.settings.motion_blur, 0.0,
            "既定はオフ (FFI 経路と二重発火しない)"
        );
    }
}
