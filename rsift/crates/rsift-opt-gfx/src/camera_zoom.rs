//! # Camera Zoom Consumer (`camera_zoom`) — wave 201: RsZoom
//!
//! mod 宣言の FOV スケール (`rsift_mod_get_fov_scale`) を射影入力へ適用する
//! 実消費者。消費点は 2 箇所のみ:
//! 1. `render_pipeline::production_frame_constants` — DX12 terrain pass の
//!    view_proj 定数 (実 present 経路)。
//! 2. `render_pipeline` tick の `FrameWiringInputs.camera_fov_y` — culling /
//!    nanite 収束の実 FOV 入力。
//!
//! どちらも `effective_camera` 1 関数を通るため、ズーム中も両経路の射影が
//! 乖離しない (片方だけズームする設計破綻を構造で排除)。

use crate::hzb_2d::CameraState;

/// 実効 FOV の絶対域 (rad)。ズーム 100x でも 0.0007 より小さくはしない下限
/// (行列特異・z fighting 爆発の防止) と、縮小 0.01x の魚眼化を止める上限。
pub const FOV_EFFECTIVE_MIN: f32 = 0.001;
pub const FOV_EFFECTIVE_MAX: f32 = 2.967;

/// mod 宣言スケールを畳んだ実効カメラ。
/// - `scale` は `rsift_api::mod_dispatch::query_fov_scale()` の無害化済み値。
/// - fov_y を scale で除算 (=倍率ズーム)。クランプで射影破綻を構造排除。
/// - pos/yaw/pitch/aspect には触れない (RsZoom は FOV のみを変える契約)。
pub fn effective_camera(cam: &CameraState, scale: f32) -> CameraState {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let mut out = *cam;
    out.fov_y = (cam.fov_y / scale).clamp(FOV_EFFECTIVE_MIN, FOV_EFFECTIVE_MAX);
    out
}

/// 現在フレームの実効カメラ。runtime 未到達・mod 無しでは入力そのまま。
pub fn current_effective_camera(cam: &CameraState) -> CameraState {
    effective_camera(cam, rsift_api::mod_dispatch::query_fov_scale())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> CameraState {
        CameraState {
            x: 8.0,
            y: 64.0,
            z: -8.0,
            yaw: 0.5,
            pitch: -0.1,
            fov_y: 1.2217, // 70°
            aspect: 16.0 / 9.0,
        }
    }

    #[test]
    fn identity_scale_returns_input_unchanged() {
        let c = cam();
        let e = effective_camera(&c, 1.0);
        assert_eq!(e.fov_y, c.fov_y);
        assert_eq!(e.aspect, c.aspect);
        assert_eq!(
            (e.x, e.y, e.z, e.yaw, e.pitch),
            (c.x, c.y, c.z, c.yaw, c.pitch)
        );
    }

    #[test]
    fn zoom_in_divides_fov_and_keeps_pose() {
        let e = effective_camera(&cam(), 4.0);
        assert!((e.fov_y - 1.2217 / 4.0).abs() < 1e-6);
        assert_eq!(e.yaw, 0.5);
    }

    #[test]
    fn extreme_scales_are_clamped_safely() {
        // 100x ズーム → 下限 0.001 までは通常除算、それ以上はクランプ。
        let e = effective_camera(&cam(), 100.0);
        assert!((e.fov_y - 1.2217 / 100.0).abs() < 1e-6);
        let huge = effective_camera(&cam(), 1e9);
        assert_eq!(huge.fov_y, FOV_EFFECTIVE_MIN);
        let fisheye = effective_camera(&cam(), 0.01);
        assert_eq!(fisheye.fov_y, FOV_EFFECTIVE_MAX);
    }

    #[test]
    fn broken_scale_falls_back_to_identity() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -2.0] {
            assert_eq!(effective_camera(&cam(), bad).fov_y, 1.2217);
        }
    }

    #[test]
    fn query_path_is_identity_without_mods() {
        // sandbox では runtime 未初期化 → mod 無し → 恒等 (実消費路の安全性)。
        let c = cam();
        assert_eq!(current_effective_camera(&c).fov_y, c.fov_y);
    }
}
