//! 2D Hierarchical-Z occlusion — conservative CPU Hi-Z with temporal stability.
//!
//! ## 数学的契約 (2026-07-24 wave 80 再定式化)
//!
//! 深度値は「大きいほどカメラに近い」単調量 `1/dist` (`[0,1]` clamp)。
//! 各 texel が保持する値 v は、「その texel が**全域にわたり**少なくとも深度値 v
//! 以上の幾何で覆われる」ことの保証である。これを実現するため:
//!
//! - **occluder rasterize**: AABB の厳密最遠隅距離 `dist_max` (凸体の外部点からの
//!   最遠点は必ず頂点に達する) の逆数 `1/dist_max` を、射影 silhouette (8 角の
//!   2D 凸包 = 透視射影の厳密な像、凸性保存による) に**完全内包される texel のみ**
//!   に書く。部分的にしか覆わない texel には保証を与えられないため書かない
//!   (書き損ねは遮蔽過小 = 保守方向)。
//! - **occludee test**: AABB の厳密最近距離 `dist_min` (clamp 点距離、内部なら 0)
//!   の逆数 `1/dist_min` (その箱が画面に提示しうる最大の深度値) が
//!   `+ DEPTH_BIAS` をもってなお tile max を下回るときのみ遮蔽と判定する。
//!
//! 単調性不変量: 同一ボックスで `v_raster <= v_test` (dist_min <= dist_max と
//! 同一 clamp から帰結) ゆえ、**自己遮蔽は数学的に不可能**。
//!
//! 近似の適用範囲 (誠実な限界): chunk を AABB ソリッドとみなす。実メッシュに
//! 空隙があっても AABB 全面で覆われた扱いになるのは Hi-Z の標準的仮定であり、
//! その仮定の内側では上記は厳密に保守的である。時間安定化 (連続
//! `OCCLUDED_FRAMES_REQUIRED` フレーム遮蔽でのみ非表示化) とテレポート時の
//! 1 フレーム skip は、この AABB 近似の外側の残存誤差に対するヒステリシス。

use crate::gpu_culling::ChunkBoundingBox;
use std::collections::HashMap;
use tracing::trace;

/// 12B 量子化頂点の stride 整合ピン (chunk_mesh::Quantized12ByteVertex 参照)。
/// Hi-Z 本体は頂点フォーマットに依らないが、本 crate の公開定数として
/// `lib.rs` 経由で露出している歴史的経緯からここに置く (テストで真値と連動)。
pub const VERTEX_BYTES: usize = 12;

/// Minimum consecutive occluded frames before hiding a chunk.
const OCCLUDED_FRAMES_REQUIRED: i8 = 4;
/// Depth bias (0..1) — larger = more conservative (fewer false occlusions).
const DEPTH_BIAS: f32 = 0.0025;
/// Camera move (blocks) that disables Hi-Z for one frame. 3D 距離で判定する
/// (鉛直テレポートでもピラミッドが stale になるのは同じため)。
const CAMERA_TELEPORT_BLOCKS: f32 = 2.0;
/// 射影パラメタ (fov/aspect) の変化がこの絶対値を超えたらテレポート同等扱い。
/// ズームで画角が変わればスクリーン矩形とピラミッド整合が崩れるため。
const PROJECTION_CHANGE_EPS: f32 = 0.001;
/// ニア平面。いずれかの隅の view_z がこれ以下ならボックスは保守フォールバック
/// (常時可視・非 raster)。透視除算の特異点を避ける厳密な棄却線。
const NEAR_PLANE: f32 = 0.5;
/// temporal ストリーク表の上限エントリ数。超過時は全クリア (ストリークが
/// リセットされるだけの保守方向) でメモリを定数上界に抑える。
/// 16384 エントリ ≒ 682 チャンク列 x 24 断面分で、実用上界として十分大きい。
const MAX_TEMPORAL_ENTRIES: usize = 16384;

#[derive(Debug, Clone, Copy, Default)]
pub struct CameraState {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub aspect: f32,
}

impl CameraState {
    /// 前フレームからの有意な移動か。3D 距離・回転・射影パラメタのいずれかの
    /// 閾値超過で真。非有限成分を含む場合は全比較が偽となり「動いていない」
    /// を返すが、非有限カメラは [`Hzb2D::cull_boxes`] が事前に拒否するため
    /// そちらの経路では到達しない (直接呼ぶ場合の契約として本仕様を固定)。
    pub fn moved_significantly(&self, prev: &CameraState) -> bool {
        let dx = self.x - prev.x;
        let dy = self.y - prev.y;
        let dz = self.z - prev.z;
        (dx * dx + dy * dy + dz * dz).sqrt() > CAMERA_TELEPORT_BLOCKS
            || (self.yaw - prev.yaw).abs() > 0.08
            || (self.pitch - prev.pitch).abs() > 0.08
            || (self.fov_y - prev.fov_y).abs() > PROJECTION_CHANGE_EPS
            || (self.aspect - prev.aspect).abs() > PROJECTION_CHANGE_EPS
    }

    /// 全成分が有限か。非有限カメラは観測欠測として拒否する (呼出側責務)。
    fn is_finite(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.z.is_finite()
            && self.yaw.is_finite()
            && self.pitch.is_finite()
            && self.fov_y.is_finite()
            && self.aspect.is_finite()
    }
}

/// temporal ストリークのキー: AABB min 3 成分の f32 bit 列。
/// bit 完全一致 = 数学的恒等であり、i32 変換の切捨て衝突 (負座標で
/// `-0.9 as i32 == 0.9 as i32 == 0`) や、同一 (x,z) 列の異なる断面
/// (min_y 違い) のキー共有 (= 1 フレームに複数回ストリークが進み
/// `OCCLUDED_FRAMES_REQUIRED` の意味論が破壊される旧欠陥) を排除する。
type TemporalKey = (u32, u32, u32);

fn temporal_key(b: &ChunkBoundingBox) -> TemporalKey {
    (
        b.min_xyz[0].to_bits(),
        b.min_xyz[1].to_bits(),
        b.min_xyz[2].to_bits(),
    )
}

/// ボックスが有限かつ min <= max の正規 AABB か。非正規は観測欠測として
/// カリング対象のまま可視扱い (raster/temporal へは登録しない)。
fn box_is_wellformed(b: &ChunkBoundingBox) -> bool {
    (0..3).all(|i| {
        b.min_xyz[i].is_finite() && b.max_xyz[i].is_finite() && b.min_xyz[i] <= b.max_xyz[i]
    })
}

#[derive(Debug, Clone, Copy)]
struct ScreenRect {
    /// テスト用 bbox 画素範囲 [x0,x1) x [y0,y1)。8 角射影の厳密外接矩形
    /// (過大方向のみで、max 走査の保守性を損なわない)。
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    /// テスト深度値: 1/厳密最近距離 (大きいほど近い)。
    v_test: f32,
    /// ラスタ深度値: 1/厳密最遠隅距離。`v_raster <= v_test` が不変量。
    v_raster: f32,
    /// 8 角射影の 2D 凸包 (monotone chain、CCW)。画素座標、f64。
    hull: [(f64, f64); 8],
    hull_len: usize,
}

/// Full-res + mip pyramid of farthest occluder depth per tile.
#[derive(Debug)]
pub struct Hzb2D {
    width: usize,
    height: usize,
    mips: Vec<Vec<f32>>,
    mip_w: Vec<usize>,
    mip_h: Vec<usize>,
    enabled: bool,
    culled: u64,
    tested: u64,
    last_camera: CameraState,
    temporal: HashMap<TemporalKey, i8>,
    skip_frame: bool,
}

impl Hzb2D {
    pub fn new(screen_w: u32, screen_h: u32, enabled: bool) -> Self {
        let width = screen_w.clamp(64, 512) as usize;
        let height = screen_h.clamp(64, 512) as usize;
        let mut mips = vec![vec![0.0f32; width * height]];
        let mut mip_w = vec![width];
        let mut mip_h = vec![height];
        let mut w = width;
        let mut h = height;
        while w > 1 || h > 1 {
            // ceil 除算: 奇数幅でも最終列/最終行の texel が必ず次 mip に
            // 伝播する (floor 除算では w0-1 列がどの (x*2+dx).min(w0-1) にも
            // 現れず死亡列となっていた — 2026-07-24 wave 80 で根治)。
            w = w.div_ceil(2).max(1);
            h = h.div_ceil(2).max(1);
            mips.push(vec![0.0f32; w * h]);
            mip_w.push(w);
            mip_h.push(h);
        }
        Self {
            width,
            height,
            mips,
            mip_w,
            mip_h,
            enabled,
            culled: 0,
            tested: 0,
            last_camera: CameraState::default(),
            temporal: HashMap::new(),
            skip_frame: false,
        }
    }

    pub fn adaptive(screen_w: u32, screen_h: u32) -> Self {
        let rp = rsift_api::AdaptivePerfEngine::render_profile(
            rsift_api::AdaptivePerfEngine::hardware(),
        );
        Self::new(
            screen_w,
            screen_h,
            rp.hzb_occlusion || rp.cpu_masked_occlusion,
        )
    }

    pub fn begin_frame(&mut self, camera: CameraState) {
        if self.last_camera.moved_significantly(&camera) {
            self.skip_frame = true;
            self.temporal.clear();
        } else {
            self.skip_frame = false;
        }
        self.last_camera = camera;
        if let Some(m0) = self.mips.first_mut() {
            m0.fill(0.0);
        }
    }

    fn build_pyramid(&mut self) {
        for level in 0..self.mips.len().saturating_sub(1) {
            let (w0, h0) = (self.mip_w[level], self.mip_h[level]);
            let (w1, h1) = (self.mip_w[level + 1], self.mip_h[level + 1]);
            // split_at_mut で借用分割し、旧実装の段ごとの src.clone()
            // (全画素アロケーション) を排除。逐語演算は同一で出力 bit 不変。
            let (head, tail) = self.mips.split_at_mut(level + 1);
            let src = &head[level];
            let dst = &mut tail[0];
            for y in 0..h1 {
                for x in 0..w1 {
                    let mut max_d = 0.0f32;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let sx = (x * 2 + dx).min(w0 - 1);
                            let sy = (y * 2 + dy).min(h0 - 1);
                            max_d = max_d.max(src[sy * w0 + sx]);
                        }
                    }
                    dst[y * w1 + x] = max_d;
                }
            }
        }
    }

    /// 2D 凸包 (monotone chain、CCW)。入力 8 点を f64 でソート・重複除去
    /// (f64 は全プラットフォームで演算が一意)、collinear 中間点は除外。
    /// 透視射影は z > 0 半空間で凸性を保存するので、これは AABB の厳密な
    /// 射影 silhouette である。
    fn convex_hull(pts: &[(f64, f64); 8]) -> ([(f64, f64); 8], usize) {
        let mut sorted = *pts;
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
            (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
        };
        let mut lower: Vec<(f64, f64)> = Vec::with_capacity(8);
        for &p in &sorted {
            if lower.last() == Some(&p) {
                continue; // 重複除去
            }
            while lower.len() >= 2
                && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0
            {
                lower.pop();
            }
            lower.push(p);
        }
        let mut upper: Vec<(f64, f64)> = Vec::with_capacity(8);
        for &p in sorted.iter().rev() {
            if upper.last() == Some(&p) {
                continue;
            }
            while upper.len() >= 2
                && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0
            {
                upper.pop();
            }
            upper.push(p);
        }
        let mut out = [(0.0, 0.0); 8];
        let mut len = 0;
        for &p in lower
            .iter()
            .take(lower.len().saturating_sub(1))
            .chain(upper.iter().take(upper.len().saturating_sub(1)))
        {
            if len < 8 {
                out[len] = p;
                len += 1;
            }
        }
        (out, len)
    }

    /// AABB の厳密射影。8 隅を view 空間へ変換し、全隅が NEAR_PLANE の手前に
    /// ない場合のみ、透視除算後の NDC bbox・厳密深度値・2D 凸包を返す。
    /// いずれかの隅が near を跨ぐ場合は保守フォールバックとして None
    /// (常時可視・非 raster — 旧実装の中心点のみの near 判定は、角が背後に
    /// ある箱で破綻する矩形を生成しえた)。
    fn project_aabb(&self, box_: &ChunkBoundingBox, cam: &CameraState) -> Option<ScreenRect> {
        let yaw = cam.yaw;
        let pitch = cam.pitch;
        let cos_y = yaw.cos();
        let sin_y = yaw.sin();
        let cos_p = pitch.cos();
        let sin_p = pitch.sin();
        let tan_half = (cam.fov_y * 0.5).tan();

        let mut pixels = [(0.0f64, 0.0f64); 8];
        let mut min_view_z = f32::INFINITY;
        let mut dist_max_sq = 0.0f32;
        let mut n = 0;
        // 角の列挙順は検算スクリプト (f32 往復厳密化) と同一: X, Y, Z の順。
        for &cx in &[box_.min_xyz[0], box_.max_xyz[0]] {
            for &cy in &[box_.min_xyz[1], box_.max_xyz[1]] {
                for &cz in &[box_.min_xyz[2], box_.max_xyz[2]] {
                    let dx = cx - cam.x;
                    let dy = cy - cam.y;
                    let dz = cz - cam.z;
                    let view_x = dx * cos_y + dz * sin_y;
                    let view_y = dy * cos_p + (dx * -sin_y + dz * cos_y) * sin_p;
                    let view_z = (dx * -sin_y + dz * cos_y) * cos_p - dy * sin_p;
                    min_view_z = min_view_z.min(view_z);
                    let d2 = dx * dx + dy * dy + dz * dz;
                    dist_max_sq = dist_max_sq.max(d2);
                    let ndc_x = (view_x / view_z) / (tan_half * cam.aspect);
                    let ndc_y = (view_y / view_z) / tan_half;
                    pixels[n] = (
                        ((ndc_x * 0.5 + 0.5) * self.width as f32) as f64,
                        ((-ndc_y * 0.5 + 0.5) * self.height as f32) as f64,
                    );
                    n += 1;
                }
            }
        }
        if min_view_z <= NEAR_PLANE {
            return None;
        }
        let sx: Vec<f64> = pixels.iter().map(|p| p.0).collect();
        let sy: Vec<f64> = pixels.iter().map(|p| p.1).collect();
        let min_sx = sx.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_sx = sx.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min_sy = sy.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_sy = sy.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let x0 = (min_sx.floor().max(0.0)) as usize;
        let y0 = (min_sy.floor().max(0.0)) as usize;
        let x1 = (max_sx.ceil().min(self.width as f64)) as usize;
        let y1 = (max_sy.ceil().min(self.height as f64)) as usize;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        // 厳密最近距離: カメラを AABB に clamp した点との距離 (内部なら 0)。
        let mut dmin_sq = 0.0f32;
        for i in 0..3 {
            let c = box_.min_xyz[i]
                .max([cam.x, cam.y, cam.z][i])
                .min(box_.max_xyz[i]);
            let d = c - [cam.x, cam.y, cam.z][i];
            dmin_sq += d * d;
        }
        let v_test = (1.0 / dmin_sq.sqrt().max(NEAR_PLANE)).clamp(0.0, 1.0);
        let v_raster = (1.0 / dist_max_sq.sqrt().max(NEAR_PLANE)).clamp(0.0, 1.0);
        let (hull, hull_len) = Self::convex_hull(&pixels);
        assert!(
            v_raster <= v_test,
            "Hi-Z 単調性不変量違反: v_raster={v_raster} > v_test={v_test} (dist_min <= dist_max から帰結するはず)"
        );
        Some(ScreenRect {
            x0,
            y0,
            x1,
            y1,
            v_test,
            v_raster,
            hull,
            hull_len,
        })
    }

    fn hzb_max_in_rect(&self, rect: &ScreenRect) -> f32 {
        let rw = rect.x1 - rect.x0;
        let rh = rect.y1 - rect.y0;
        let level = (rw.max(rh) as f32).log2().floor().max(0.0) as usize;
        let level = level.min(self.mips.len().saturating_sub(1));
        let w = self.mip_w[level];
        let h = self.mip_h[level];
        let scale_x = w as f32 / self.width as f32;
        let scale_y = h as f32 / self.height as f32;
        let x0 = (rect.x0 as f32 * scale_x).floor() as usize;
        let y0 = (rect.y0 as f32 * scale_y).floor() as usize;
        let x1 = (rect.x1 as f32 * scale_x).ceil() as usize;
        let y1 = (rect.y1 as f32 * scale_y).ceil() as usize;
        let mut max_d = 0.0f32;
        let mip = &self.mips[level];
        for y in y0..y1.min(h) {
            for x in x0..x1.min(w) {
                max_d = max_d.max(mip[y * w + x]);
            }
        }
        max_d
    }

    /// 凸包に完全内包される texel のみに v_raster を書く scanline フィル。
    /// 完全内包: texel [x,x+1]x[y,y+1] が帯内交差区間 [xa,xb] (上下端の狭い側)
    /// を満たす ceil(xa) <= x かつ x+1 <= xb のもの。部分被覆 texel に保証値を
    /// 書くと「その画素の全域が覆われる」主張が偽になるため書かない (遮蔽過小
    /// = 保守方向)。交差計算は f64 (全プラットフォームで一意)。
    fn rasterize_occluder(&mut self, rect: &ScreenRect) {
        let n = rect.hull_len;
        if n < 3 {
            return; // 縮退化: 面積の保証がない → 証拠不十分として書かない
        }
        let v = rect.v_raster;
        let mut ymin = f64::INFINITY;
        let mut ymax = f64::NEG_INFINITY;
        for i in 0..n {
            ymin = ymin.min(rect.hull[i].1);
            ymax = ymax.max(rect.hull[i].1);
        }
        let first = (ymin.ceil() as i64).max(0);
        let last = ((ymax.ceil() as i64) - 1).min(self.height as i64 - 1);
        let width = self.width as i64;
        for row in first..=last {
            let span = |yy: f64| -> Option<(f64, f64)> {
                let mut lo = f64::INFINITY;
                let mut hi = f64::NEG_INFINITY;
                for i in 0..n {
                    let (x1, y1) = rect.hull[i];
                    let (x2, y2) = rect.hull[(i + 1) % n];
                    let (ey_lo, ey_hi) = if y1 <= y2 { (y1, y2) } else { (y2, y1) };
                    // 半開区間 [lo,hi) で辺-水平線交差 (頂点重複を機械的に安定化)
                    if ey_lo <= yy && yy < ey_hi {
                        let t = (yy - y1) / (y2 - y1);
                        let x = x1 + t * (x2 - x1);
                        lo = lo.min(x);
                        hi = hi.max(x);
                    }
                }
                if lo <= hi {
                    Some((lo, hi))
                } else {
                    None
                }
            };
            let (s0, s1) = (span(row as f64), span(row as f64 + 1.0));
            let (xa, xb) = match (s0, s1) {
                (Some(a), Some(b)) => (a.0.max(b.0), a.1.min(b.1)),
                // 片側のみ交差 (ymax 頂点ちょうど等) は保守に書かない
                _ => continue,
            };
            let x_start = (xa.ceil() as i64).max(0);
            let x_end = ((xb.ceil() as i64) - 1).min(width - 1);
            let m0 = &mut self.mips[0];
            for col in x_start..=x_end {
                let i = row as usize * self.width + col as usize;
                if v > m0[i] {
                    m0[i] = v;
                }
            }
        }
    }

    /// Conservative test: occluded only if nearest depth is clearly behind Hi-Z.
    fn test_occluded(&self, rect: &ScreenRect) -> bool {
        let hzb = self.hzb_max_in_rect(rect);
        if hzb < 0.001 {
            return false; // 空バッファの走査省略 (hzb=0 なら不等式は不成立、冗長だが高速経路)
        }
        rect.v_test + DEPTH_BIAS < hzb
    }

    /// 上限ガード付きストリークエントリ。超過時は全クリア (保守方向) で
    /// HashMap のメモリを定数上界に抑える。
    fn temporal_entry(&mut self, key: TemporalKey) -> &mut i8 {
        if self.temporal.len() >= MAX_TEMPORAL_ENTRIES {
            self.temporal.clear();
        }
        self.temporal.entry(key).or_insert(0)
    }

    fn temporal_cull(&mut self, chunk_key: TemporalKey, occluded_now: bool) -> bool {
        let streak = self.temporal_entry(chunk_key);
        if !occluded_now {
            *streak = 1;
            return false;
        }
        if *streak > 0 {
            *streak = -1;
        } else {
            // 下限 -OCCLUDED_FRAMES_REQUIRED に clamp: 単調減少のままだと
            // i8 が 129 フレーム連続遮蔽でアンダーフローし debug panic /
            // release wrap していた (2026-07-24 wave 80 で根治)。判定式
            // `<= -REQUIRED` に対し clamp は数学的に無影響 (値域を [-4,1] に限定)。
            *streak = (*streak - 1).max(-OCCLUDED_FRAMES_REQUIRED);
        }
        *streak <= -OCCLUDED_FRAMES_REQUIRED
    }

    /// Front-to-back pass: rasterize occluders, then test remaining boxes.
    pub fn cull_boxes(&mut self, boxes: &[ChunkBoundingBox], camera: CameraState) -> Vec<usize> {
        if !self.enabled {
            return (0..boxes.len()).collect();
        }
        if !camera.is_finite() {
            // 非有限カメラは観測欠測として拒否: last_camera・temporal・
            // ピラミッド・帳簿を一切更新せず保守全可視を返す。旧実装は NaN が
            // f32::max/min の NaN 伝播規則経由で「偶然」全可視になっていたが、
            // それは仕様ではなく振る舞いの副産物だったため明示化する。
            return (0..boxes.len()).collect();
        }
        if self.skip_frame {
            // 消費型 1 フレームスキップ (CAMERA_TELEPORT_BLOCKS の意図通り)。
            // begin_frame は本メソッド内でしか呼ばれないため、ここで解除しないと
            // 一度のテレポートで skip_frame が真のまま固まり Hi-Z が**永久無効化**
            // していた (render_pipeline.rs は begin_frame を外部呼出ししない)。
            // 旧実装は return のみ — 2026-07-22 監査で摘出・修正。
            self.skip_frame = false;
            return (0..boxes.len()).collect();
        }

        self.begin_frame(camera);
        self.tested += boxes.len() as u64;

        let mut order: Vec<usize> = (0..boxes.len()).collect();
        order.sort_by(|&a, &b| {
            let da = dist_sq(&boxes[a], &camera);
            let db = dist_sq(&boxes[b], &camera);
            // 非有限ボックスの距離は NaN になり得るが、stable sort かつ
            // Equal フォールバックなので順序は決定的 (非有限箱は後段で drop)。
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut visible = Vec::new();
        for i in order {
            let b = &boxes[i];
            if !box_is_wellformed(b) {
                // 非有限または min > max の逆転: 観測欠測 drop — 可視扱いと
                // するが raster/temporal には一切登録しない (NaN 汚染遮断)。
                visible.push(i);
                continue;
            }
            let key = temporal_key(b);
            let rect = match self.project_aabb(b, &camera) {
                Some(r) => r,
                None => {
                    *self.temporal_entry(key) = 1;
                    visible.push(i);
                    continue;
                }
            };

            let occluded = self.test_occluded(&rect);
            if self.temporal_cull(key, occluded) {
                self.culled += 1;
                trace!("[HiZ2D] culled chunk {:?}", key);
                continue;
            }
            visible.push(i);
            self.rasterize_occluder(&rect);
        }

        self.build_pyramid();
        visible
    }

    pub fn stats(&self) -> (u64, u64) {
        (self.culled, self.tested)
    }
}

/// 中心間の 3D 距離二乗 (front-to-back ソート用ヒューリスティック。
/// 正確性には関与しないが、カメラ直上/真下の列を正しく近いとみなすため
/// y 成分を含める)。raster 順序は max 更新の可換性から mip0 に影響せず、
/// 遮蔽判定への影響は「同フレーム暫定 mip0」経由のみで front-to-back が
/// 最良の近似順序となる。
fn dist_sq(b: &ChunkBoundingBox, cam: &CameraState) -> f32 {
    let cx = (b.min_xyz[0] + b.max_xyz[0]) * 0.5;
    let cy = (b.min_xyz[1] + b.max_xyz[1]) * 0.5;
    let cz = (b.min_xyz[2] + b.max_xyz[2]) * 0.5;
    let dx = cx - cam.x;
    let dy = cy - cam.y;
    let dz = cz - cam.z;
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(min: [f32; 3], max: [f32; 3], i: u32) -> ChunkBoundingBox {
        ChunkBoundingBox {
            min_xyz: min,
            is_visible: 1,
            max_xyz: max,
            chunk_index: i,
            bindless_texture_id: 0,
            _pad: [0; 3],
        }
    }

    const CAM0: CameraState = CameraState {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        yaw: 0.0,
        pitch: 0.0,
        fov_y: 1.0,
        aspect: 1.0,
    };

    /// 検算スクリプト (/tmp/cd_verify*.py、f32 往復厳密化) と同じウォーム手順:
    /// 初回呼出しは default camera との差分で skip を武装し、2 回目で消費する。
    fn warmed(cam: CameraState, w: u32, hgt: u32) -> Hzb2D {
        let mut hz = Hzb2D::new(w, hgt, true);
        hz.cull_boxes(&[], cam); // skip 武装 (last=default → moved)
        hz.cull_boxes(&[], cam); // skip 消費
        hz
    }

    #[test]
    fn vertex_stride_is_12() {
        assert_eq!(
            std::mem::size_of::<crate::chunk_mesh::Quantized12ByteVertex>(),
            12
        );
        assert_eq!(VERTEX_BYTES, 12);
    }

    #[test]
    fn temporal_requires_multiple_frames() {
        let mut hzb = Hzb2D::new(128, 128, true);
        hzb.begin_frame(CAM0);
        for _ in 0..OCCLUDED_FRAMES_REQUIRED - 1 {
            assert!(!hzb.temporal_cull((0, 0, 0), true));
        }
        assert!(hzb.temporal_cull((0, 0, 0), true));
    }

    /// CD-1 (2026-07-24): 129 フレーム連続遮蔽で i8 streak がアンダーフローし
    /// debug panic / release wrap していた。clamp 後は 300 フレームでも panic せず
    /// ストリークは -OCCLUDED_FRAMES_REQUIRED に留まり、可視遷移で 1 に戻る。
    #[test]
    fn temporal_streak_saturates_never_underflows() {
        let mut hzb = Hzb2D::new(128, 128, true);
        hzb.begin_frame(CAM0);
        let key = (16.0f32.to_bits(), 0.0f32.to_bits(), 32.0f32.to_bits());
        for i in 0..300 {
            let cull = hzb.temporal_cull(key, true);
            assert_eq!(cull, i + 1 >= OCCLUDED_FRAMES_REQUIRED as usize);
        }
        assert_eq!(*hzb.temporal.get(&key).unwrap(), -OCCLUDED_FRAMES_REQUIRED);
        // 可視フレームでリセットされ、再び 4 フレーム必要になる。
        assert!(!hzb.temporal_cull(key, false));
        assert_eq!(*hzb.temporal.get(&key).unwrap(), 1);
        for _ in 0..3 {
            assert!(!hzb.temporal_cull(key, true));
        }
        assert!(hzb.temporal_cull(key, true));
    }

    /// CD-2: temporal キーは min 3 成分の to_bits 3 連 = 断面 (y 違い) を厳密に
    /// 区別する。同一 (x,z)・異 y の 2 キーが独立にストリークを進めることを
    /// 直接ピンする (旧 (i32 切捨て, i32) キーでは共有され 2 倍速で発火した)。
    #[test]
    fn temporal_keys_distinguish_column_sections() {
        let mut hzb = Hzb2D::new(128, 128, true);
        hzb.begin_frame(CAM0);
        // キーは実ボックスから導出関数経由で取る (導出関数そのもののピン)。
        let lo = temporal_key(&mk([0.0, 60.0, 0.0], [16.0, 76.0, 16.0], 0));
        let hi = temporal_key(&mk([0.0, 76.0, 0.0], [16.0, 92.0, 16.0], 1));
        assert_ne!(lo, hi, "min_y 違いで別キー (導出関数契約)");
        // 交互に 3 回ずつ true: 合計 6 回でもまだ双方未発火でなければならない。
        for _ in 0..3 {
            assert!(!hzb.temporal_cull(lo, true));
            assert!(!hzb.temporal_cull(hi, true));
        }
        // 4 回目で各々発火。
        assert!(hzb.temporal_cull(lo, true));
        assert!(hzb.temporal_cull(hi, true));
    }

    /// CD-4: 8 角厳密射影の rect・深度値を検算スクリプト独立導出値でピン。
    /// dist_min² = 4²+0+10² = 116 (整数) → v_test = 1/√116。
    /// dist_max² = 6²+2²+12² = 184 → v_raster = 1/√184。
    #[test]
    fn project_aabb_exact_unit_box() {
        let hzb = Hzb2D::new(128, 128, true);
        let r = hzb
            .project_aabb(&mk([4.0, -2.0, 10.0], [6.0, 2.0, 12.0], 0), &CAM0)
            .expect("near 非跨ぎ・画面内");
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (103, 40, 128, 88));
        assert_eq!(r.v_test.to_bits(), 0x3DBE26EB, "1/√116 の f32 一意値");
        assert_eq!(r.v_raster.to_bits(), 0x3D96FB06, "1/√184 の f32 一意値");
        assert!(r.v_raster <= r.v_test, "単調性不変量");
        // 遠近 2 矩形 (z=10 面が外向き大、z=12 面が小) の凸包はオフアクシス
        // (x 4..6) ゆえ 6 頂点 — 検算スクリプト monotone chain で確定。
        assert_eq!(r.hull_len, 6);
    }

    /// CD-4: 旧実装が欠落させていた fov/aspect 因子の厳密な影響をピン。
    /// 同一ボックスで fov=2.0 なら矩形は (77,55)-(89,73)、16:9 なら (85,40)-(104,88)。
    /// (旧式の half_w は /(tan·aspect) が無く、zoom で 2.2 倍過大 = 穴方向、
    /// 広角で 2.8 倍過小 = 誤遮蔽混在だった。)
    #[test]
    fn project_aabb_respects_fov_and_aspect_exactly() {
        let hzb = Hzb2D::new(128, 128, true);
        let wide = CameraState { fov_y: 2.0, ..CAM0 };
        let r = hzb
            .project_aabb(&mk([4.0, -2.0, 10.0], [6.0, 2.0, 12.0], 0), &wide)
            .unwrap();
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (77, 55, 89, 73));
        let hd = CameraState {
            aspect: 16.0 / 9.0,
            ..CAM0
        };
        let r = hzb
            .project_aabb(&mk([4.0, -2.0, 10.0], [6.0, 2.0, 12.0], 0), &hd)
            .unwrap();
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (85, 40, 104, 88));
    }

    /// CD-4: near 跨ぎは厳密 None (保守)。角 z=0.4 で None、全隅 z>=0.6 で Some。
    /// 旧実装は中心点のみで判定し、角が背後の箱で破綻矩形を生成しえた。
    #[test]
    fn near_plane_straddle_is_conservative_none() {
        let hzb = Hzb2D::new(128, 128, true);
        assert!(hzb
            .project_aabb(&mk([-1.0, -1.0, 0.4], [1.0, 1.0, 4.0], 0), &CAM0)
            .is_none());
        assert!(hzb
            .project_aabb(&mk([-1.0, -1.0, 0.6], [1.0, 1.0, 4.0], 0), &CAM0)
            .is_some());
    }

    /// CD-5: ceil ピラミッド。幅 65 の段列は [65,33,17,9,5,3,2,1] で、
    /// 最終列 (x=64) の texel が mip1 へ伝播する (floor 版 [65,32,16,...] では
    /// w0-1 列が (x*2+dx).min(w0-1) に現れず死亡列だった)。
    #[test]
    fn ceil_mip_chain_and_last_column_propagates() {
        let mut hzb = Hzb2D::new(65, 65, true);
        assert_eq!(hzb.mip_w, vec![65, 33, 17, 9, 5, 3, 2, 1]);
        assert_eq!(hzb.mip_h, vec![65, 33, 17, 9, 5, 3, 2, 1]);
        // 最終列だけを覆う手作り rect (完全内包列は x=64 のみ)。
        let rect = ScreenRect {
            x0: 64,
            y0: 0,
            x1: 65,
            y1: 65,
            v_test: 0.5,
            v_raster: 0.5,
            hull: [
                (64.0, 0.0),
                (65.0, 0.0),
                (65.0, 65.0),
                (64.0, 65.0),
                (0.0, 0.0),
                (0.0, 0.0),
                (0.0, 0.0),
                (0.0, 0.0),
            ],
            hull_len: 4,
        };
        hzb.rasterize_occluder(&rect);
        assert_eq!(hzb.mips[0][64], 0.5, "mip0 最終列に v_raster");
        hzb.build_pyramid();
        // mip1 幅 33 の列 32 が texel 63,64 をカバー (ceil → 64 も到達)。
        assert_eq!(hzb.mips[1][32], 0.5, "ceil 縮退で最終列が mip1 へ伝播する");
    }

    /// CD-3: rasterize は occluder の最遠深度値 (1/√804) を書く。
    /// 旧実装は最近値 1/dist_center (=0.0556) を書いており、「tile 全域が
    /// その深度値で覆われる」保証が偽 (= false hole 方向) だった。
    #[test]
    fn rasterize_writes_farthest_depth_guarantee() {
        let mut hzb = Hzb2D::new(128, 128, true);
        let r = hzb
            .project_aabb(&mk([-8.0, -8.0, 10.0], [8.0, 8.0, 26.0], 0), &CAM0)
            .unwrap();
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (0, 0, 128, 128));
        assert_eq!(r.v_raster.to_bits(), 0x3D10746C, "1/√804 の f32 一意値");
        hzb.rasterize_occluder(&r);
        let v = f32::from_bits(0x3D10746C);
        // 正面矩形の凸包で画面全域が完全内包 → 全 texel 同値。
        assert_eq!(hzb.mips[0][0], v);
        assert_eq!(hzb.mips[0][64 * 128 + 64], v);
        assert_eq!(hzb.mips[0][127 * 128 + 127], v);
    }

    /// CD-3/CD-4: 凸包 scanline は bbox 角が silhouette 外にある部分被覆 texel
    /// を書かない。45° yaw の箱で左端列 x=77 (hull 左辺が x=77.02) が
    /// 完全内包外となり 0.0 のまま残る (旧 bbox raster は書いていた)。
    #[test]
    fn hull_rasterize_skips_partially_covered_texels() {
        let mut hzb = Hzb2D::new(128, 128, true);
        let cam45 = CameraState {
            yaw: std::f32::consts::FRAC_PI_4,
            ..CAM0
        };
        let r = hzb
            .project_aabb(&mk([-8.0, -8.0, 10.0], [8.0, 8.0, 26.0], 0), &cam45)
            .unwrap();
        assert_eq!((r.x0, r.y0), (77, 0), "bbox 左端 (検算 f64 凸包由来)");
        hzb.rasterize_occluder(&r);
        assert_eq!(
            hzb.mips[0][77], 0.0,
            "bbox 角だが hull 外 (部分被覆) → 非記述"
        );
        assert_eq!(hzb.mips[0][127 * 128 + 77], 0.0, "最下行でも列 77 は非記述");
        let v = r.v_raster;
        assert_eq!(hzb.mips[0][78], v, "列 78 は完全内包 → 記述");
        assert_eq!(hzb.mips[0][103], v, "中央列は完全内包");
    }

    /// CD-6: moved_significantly は 3D 距離 + 射影パラメタ変化を見る。
    /// (旧実装は XZ のみ → 鉛直テレポート・ズームで stale ピラミッドを使い続けた。)
    #[test]
    fn moved_significantly_3d_and_projection_changes() {
        let base = CAM0;
        assert!(
            CameraState { y: 3.0, ..base }.moved_significantly(&base),
            "鉛直 3.0 > 2.0"
        );
        assert!(
            !CameraState { y: 1.0, ..base }.moved_significantly(&base),
            "ジャンプ 1.25 級 1.0 は閾値内"
        );
        assert!(
            CameraState {
                fov_y: base.fov_y + 0.01,
                ..base
            }
            .moved_significantly(&base),
            "Δfov 0.01 > 1e-3"
        );
        assert!(!CameraState {
            fov_y: base.fov_y + 0.0005,
            ..base
        }
        .moved_significantly(&base));
        assert!(CameraState {
            aspect: base.aspect + 0.01,
            ..base
        }
        .moved_significantly(&base));
        assert!(!base.moved_significantly(&base), "同一カメラは不動");
    }

    /// CD-7: 非有限カメラは保守全可視かつ無記録 (帳簿・last_camera 不変)。
    /// NaN カメラの次フレームが通常カメラに戻っても「skip 武装しない」ことで
    /// last_camera が NaN フレームを記録していないことを観測する。
    #[test]
    fn non_finite_camera_is_rejected_without_recording() {
        let cam = CameraState { y: 64.0, ..CAM0 };
        let mut h = warmed(cam, 128, 128);
        let boxes = vec![mk([-8.0, -8.0, 10.0], [8.0, 8.0, 26.0], 0)];
        h.cull_boxes(&boxes, cam);
        assert_eq!(h.stats().1, 1);
        let nan_cam = CameraState { x: f32::NAN, ..cam };
        let vis = h.cull_boxes(&boxes, nan_cam);
        assert_eq!(vis, vec![0], "非有限カメラ → 保守全可視");
        assert_eq!(h.stats().1, 1, "帳簿を進めない");
        // last_camera が汚染されていない: 同一カメラ再突入は不動判定
        // (NaN フレームを記録していれば NaN との比較で必ず不動 = skip 非武装、
        //  正常 cam を記録し続けていれば同じく不動 — 区別のため y を大きく
        //  変えた cam2 を投げ、skip が武装される (=last が妥当) ことを見る)。
        let cam2 = CameraState { y: 200.0, ..cam };
        h.cull_boxes(&boxes, cam2); // moved (|200-64|>2) → 武装 + 実行
        assert_eq!(h.stats().1, 2, "武装フレームは通常通り帳簿が進む");
        let vis = h.cull_boxes(&boxes, cam2); // skip 消費
        assert_eq!(vis.len(), 1);
        assert_eq!(h.stats().1, 2, "消費フレームは帳簿が進まない");
    }

    /// CD-7: 非有限・逆転ボックスは可視パススルーするが raster/temporal に
    /// 登録されず、深度バッファを汚染しない。
    #[test]
    fn malformed_boxes_pass_through_without_recording() {
        let mut h = warmed(CAM0, 128, 128);
        let boxes = vec![
            mk([f32::NAN, -8.0, 10.0], [8.0, 8.0, 26.0], 0), // NaN
            mk([8.0, -8.0, 10.0], [-8.0, 8.0, 26.0], 1),     // min > max 逆転
        ];
        let vis = h.cull_boxes(&boxes, CAM0);
        assert_eq!(vis, vec![0, 1], "非正規ボックスも可視 (保守)");
        assert!(h.temporal.is_empty(), "temporal 非登録");
        assert!(h.mips[0].iter().all(|&v| v == 0.0), "深度バッファ無汚染");
    }

    /// CD-9: temporal 表は MAX_TEMPORAL_ENTRIES で定数上界。超過挿入は
    /// 保守全クリア (ストリークリセット) でメモリ単調増を防ぐ。
    #[test]
    fn temporal_map_is_capped() {
        let mut hzb = Hzb2D::new(128, 128, true);
        hzb.begin_frame(CAM0);
        for i in 0..MAX_TEMPORAL_ENTRIES as u32 {
            hzb.temporal_cull((i, 0, 0), true);
        }
        assert_eq!(hzb.temporal.len(), MAX_TEMPORAL_ENTRIES);
        hzb.temporal_cull((u32::MAX, 1, 1), true);
        assert_eq!(hzb.temporal.len(), 1, "上限超過 → 全クリア後に 1 件");
    }

    /// CD-3 不変量の統合表現: 単一ボックスは何フレーム経っても自己を
    /// 遮蔽しない (v_raster <= v_test + BIAS の関係から必然的に不成立)。
    #[test]
    fn self_occlusion_is_impossible() {
        let mut h = warmed(CAM0, 128, 128);
        let boxes = vec![mk([-8.0, -8.0, 10.0], [8.0, 8.0, 26.0], 0)];
        for frame in 0..8 {
            let vis = h.cull_boxes(&boxes, CAM0);
            assert_eq!(vis, vec![0], "frame {frame}: 自己遮蔽は発生しえない");
        }
        assert_eq!(h.stats().0, 0, "culled は常に 0");
    }

    /// CD-3 統合 (深度逆転の根治): occluder の最近フロンティアより遠いが
    /// 最遠深度値より近い occludee は**遮蔽されない**。旧実装は raster が
    /// 最近値 (1/18=0.0556) を書いたため、この occludee (1/27+BIAS=0.0395)
    /// を 4 フレームで誤カリングしていた。
    #[test]
    fn near_frontier_does_not_falsely_occlude() {
        let mut h = warmed(CAM0, 128, 128);
        let boxes = vec![
            mk([-8.0, -8.0, 10.0], [8.0, 8.0, 26.0], 0), // occluder
            mk([-8.0, -4.0, 27.0], [8.0, 4.0, 43.0], 1), // 近すぎて遮蔽不成立
        ];
        for frame in 0..6 {
            let vis = h.cull_boxes(&boxes, CAM0);
            assert_eq!(
                vis,
                vec![0, 1],
                "frame {frame}: 保守判定で occludee は常に可視"
            );
        }
        assert_eq!(h.stats().0, 0);
    }

    /// CD-2 統合 (キー衝突の根治): 同一 (x,z) 列・異 y の 2 断面 A/B が
    /// 正確に 4 フレーム連続遮蔽でのみ非表示化される。旧 2D キーでは
    /// 1 フレームに 2 回ストリークが進み 2 フレームで発火 (= フリッカー穴)。
    /// 深度値は検算スクリプトで事前確認済: A 1/40+BIAS=0.0275 < 1/√804=0.03527、
    /// B 1/√2276+BIAS=0.02346 < 0.03527 (いずれも遮蔽成立側)。
    #[test]
    fn column_sections_cull_after_exactly_four_frames() {
        let mut h = warmed(CAM0, 128, 128);
        let boxes = vec![
            mk([-8.0, -8.0, 10.0], [8.0, 8.0, 26.0], 0), // 0: occluder
            mk([-8.0, -6.0, 40.0], [8.0, 10.0, 56.0], 1), // 1: 断面 A
            mk([-8.0, 26.0, 40.0], [8.0, 42.0, 56.0], 2), // 2: 断面 B (同列・異 y)
        ];
        // フレーム系列: frame0 はピラミッド空で occluded_now=false
        // (streak=1)、frame1..4 で連続遮蔽 -1..-4 → frame4 で発火。
        for frame in 0..4 {
            let vis = h.cull_boxes(&boxes, CAM0);
            assert_eq!(
                vis,
                vec![0, 1, 2],
                "frame {frame}: 断面 A/B は発火前の 4 フレーム可視 (旧衝突キーなら frame2 で B が落ちる)"
            );
        }
        let vis = h.cull_boxes(&boxes, CAM0);
        assert_eq!(
            vis,
            vec![0],
            "5 フレーム目 (連続遮蔽 4 回達成) で A/B が同時発火"
        );
        assert_eq!(h.stats().0, 2);
    }

    /// 回帰 (2026-07-22): skip_frame が解除されず Hi-Z が永久無効化していた。
    /// stats.1 (tested) の増分で「スキップ/回復」を観測する:
    /// スキップフレームは早期 return で tested が進まない。
    /// 2026-07-24 (CD-6): 3D 距離化により default(0,0,0) → cam_a(y=64) の
    /// 初回もテレポート武装する仕様に変更。カウンタ列を新仕様へ訂正。
    #[test]
    fn teleport_skip_is_consumed_and_recovers() {
        let boxes = vec![
            mk([-8.0, 60.0, 8.0], [8.0, 76.0, 24.0], 0),
            mk([100.0, 60.0, 100.0], [116.0, 76.0, 116.0], 1),
        ];
        let cam_a = CameraState {
            x: 0.0,
            y: 64.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 1.0,
            aspect: 1.0,
        };
        let cam_b = CameraState { x: 100.0, ..cam_a }; // 移動 100 > 2 (テレポート)

        let mut h = Hzb2D::new(256, 256, true);
        let n = boxes.len() as u64;
        h.cull_boxes(&boxes, cam_a); // 初回: default→y64 で武装 + 実行
        assert_eq!(h.stats().1, n);
        h.cull_boxes(&boxes, cam_a); // skip 消費 (tested 据置)
        assert_eq!(h.stats().1, n);
        h.cull_boxes(&boxes, cam_a); // 通常フレーム
        assert_eq!(h.stats().1, 2 * n);

        h.cull_boxes(&boxes, cam_b); // 着弾フレーム: begin が skip を武装、カリング自体は実行
        assert_eq!(h.stats().1, 3 * n);
        let skipped = h.cull_boxes(&boxes, cam_b); // skip 消費: 保守的に全可視
        assert_eq!(skipped, (0..boxes.len()).collect::<Vec<_>>());
        assert_eq!(h.stats().1, 3 * n, "skip フレームは tested を進めない");
        h.cull_boxes(&boxes, cam_b); // 回復: カリング再開
        assert_eq!(
            h.stats().1,
            4 * n,
            "回帰: 旧実装は skip_frame が解除されず tested が永久に進まなかった"
        );
    }
}
