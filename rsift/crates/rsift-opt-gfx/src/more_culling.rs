//! MoreCulling 逆輸入 — 看板テキスト / 額縁 / 雨 / 葉の追加カリング規則。
//!
//! どれも描画呼び出し側がフレームごとに呼ぶだけの純粋関数群 (副作用なし)。

/// 看板テキスト: ブロックの正面法線とカメラ方向の内積で背面判定。
/// `front_dot_cos` はカリング閾値 (cos角度)。MoreCulling 既定 ≒ cos(110°)。
pub fn sign_text_visible(face_normal: [f32; 3], sign_center: [f32; 3], cam: [f32; 3]) -> bool {
    let to_cam = [
        cam[0] - sign_center[0],
        cam[1] - sign_center[1],
        cam[2] - sign_center[2],
    ];
    let nlen = n3_len(face_normal);
    let clen = n3_len(to_cam);
    if nlen < 1e-6 || clen < 1e-6 {
        return true;
    }
    let dot = n3_dot(face_normal, to_cam) / (nlen * clen);
    // 110° → cos = -0.34。法線が視線側を向いていれば描画。
    dot > -0.342
}

/// 額縁 (Item Frame): 背面向き + 遠距離小面積判定。
pub fn item_frame_visible(
    facing: [f32; 3],
    frame_center: [f32; 3],
    footprint_px: f32,
    cam: [f32; 3],
    min_pixel_size: f32,
) -> bool {
    if footprint_px < min_pixel_size {
        return false; // 遠すぎて描画意味なし (MoreCulling の entity size cull)
    }
    sign_text_visible(facing, frame_center, cam)
}

/// 画面占有率 (投影像素数) 推定。fov_tan_half: tan(fov/2)、screen_h: 縦ピクセル。
pub fn screen_footprint_px(size: f32, dist: f32, fov_tan_half: f32, screen_h: f32) -> f32 {
    if dist < 1e-3 {
        return screen_h;
    }
    (size * screen_h) / (2.0 * dist * fov_tan_half)
}

/// 雨/しぶき: その XZ で露天の column なら描画。
/// `top_opaque_y` は (x,z) の最上 opaque Y (chunk heightmap)。depth-1 まで許容。
pub fn rain_visible(drop_y: f32, top_opaque_y: u16) -> bool {
    drop_y >= (top_opaque_y as f32) - 1.0
}

/// 葉ブロック裏面カリング (fast graphics 相当): 隣接両側が不透明ならその面は描かない。
/// `dir` は面の外向きインデックス (0=+X,1=-X,2=+Y,3=-Y,4=+Z,5=-Z)。
pub fn leaf_face_needed(neighbor_opaque: [bool; 6], dir: usize) -> bool {
    !neighbor_opaque[dir]
}

/// 6 近傍を並べる補助。実体側で (x±1…) を評価して入れる。
pub fn neighbor_mask<F: Fn(i32, i32, i32) -> bool>(opaque: &F, x: i32, y: i32, z: i32) -> [bool; 6] {
    [
        opaque(x + 1, y, z),
        opaque(x - 1, y, z),
        opaque(x, y + 1, z),
        opaque(x, y - 1, z),
        opaque(x, y, z + 1),
        opaque(x, y, z - 1),
    ]
}

/// もっと強い内部面削減 (MoreCulling "cull invisible faces beyond vanilla"):
/// 半透明ブロックでも、両隣が同種ガラス等 (culling される透過) なら面を削る。
pub fn shared_layer_face_needed(a_transparent: bool, b_transparent: bool, same_type: bool) -> bool {
    match (a_transparent, b_transparent) {
        (true, true) => !same_type, // 同透過同士は面不要
        (true, false) => false,     // 透過の裏に不透過 → 面不要 (透過側から見た面のみ描画)
        _ => true,
    }
}

#[inline]
fn n3_dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn n3_len(a: [f32; 3]) -> f32 {
    n3_dot(a, a).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_backface_culled() {
        // 法線が -Z → 看板の「表」は -Z 側。+Z 側は背後なのでカリング対象。
        let normal = [0.0, 0.0, -1.0];
        let center = [0.0, 64.0, 0.0];
        let cam_behind = [0.0, 64.0, 4.0]; // 法線の裏側 (+Z)
        assert!(!sign_text_visible(normal, center, cam_behind));
        let cam_front = [0.0, 64.0, -4.0]; // 法線と同じ側 (-Z)
        assert!(sign_text_visible(normal, center, cam_front));
    }

    #[test]
    fn rain_height_rule() {
        assert!(rain_visible(64.0, 63));
        assert!(!rain_visible(61.0, 63));
    }

    #[test]
    fn footprint_shorter() {
        let near = screen_footprint_px(1.0, 4.0, 0.7, 1080.0);
        let far = screen_footprint_px(1.0, 400.0, 0.7, 1080.0);
        assert!(near > far);
        assert!(far < 2.0);
    }
}
