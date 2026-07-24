//! Software occlusion via CPU Hi-Z pyramid (Tier 6).
//!
//! CPU 側ソフトウェア Hi-Z: occluder AABB/三角形をスクリーン空間深度
//! ピラミッド (u16、0=最も近い / 65535=最も遠い) にラスタライズし、
//! チャンク AABB の遮蔽判定を GPU コマンド発行前に行う。
//!
//! ## 数学的契約 (2026-07-24 wave 82 再定式化)
//!
//! - **行列規約**: `view_proj` は本番経路
//!   [`crate::world_column_store::TerrainFrameConstants::from_camera`] と
//!   同一の行ベクトル規約 p x M (clip_i = Σ_j p_j * vp[j][i]、平行移動は
//!   row 3)。旧実装は `vp[row] . p_col` (転置規約 M x p) を使っており
//!   **射影空間が破壊的に歪んでいた** (平行移動が w 行に化け、w は視点 z
//!   ですらなかった — CF-1 で根治)。
//! - **深度**: この行列の ndc_z は DX12 式 [0,1] (near→0、far→1)。
//!   旧実装の `z*0.5+0.5` 再マップ (OpenGL [-1,1] 前提) を撤廃。
//! - **被覆保証**: texel 値 T は「その texel 全域が深度値 ≤ T の幾何で
//!   覆われる」ことの保証。occluder は box/三角形の**最遠**depth (max z)
//!   を書く (最近値を書くと保証が偽 = false hole、CD-3/CF-1 同型)。
//!   occludee は**最近**depth (min z) で比較し、strict な大なり
//!   (`test_depth > tile`) でのみ遮蔽成立。tile = 子の max
//!   (= 4 子 texel の最弱保証)。自己遮蔽は等値境界で不成立。
//! - **w 符号**: w <= 1e-6 の角は背面/退化として**必ず拒否**
//!   (旧実装は |w| のみ検査で背面角が鏡写しに混入しえた)。
//! - **ジャイター注記**: Halton(2,3) 生成器と 8 フレーム周期の
//!   jitter_index は実在するが、現行は射影へ**未適用** (保守性証明が
//!   未整備のため温存した設計予備。ヘッダの説明は誠実化: 旧版は
//!   あたかも適用済みかのように書かれていた)。
//! - **「SWAR」注記**: `rasterize_triangle_swar` の名前は歴史的経緯。
//!   実装はスカラーの重心中心サンプルフィル (SIMD なし) で、辺関数は
//!   3 つ全て**直接辺関数**で計算し、内側判定は同符号性 (全て >= 0 または
//!   全て <= 0) で行う巻き向き不変形。旧実装の w_i は真の barycentric の
//!   符号反転 (w_i ≡ -λ_i) で、`w2 = 1-w0-w1` (= 1+λ0+λ1) と
//!   「w_i >= -1e-4 ∀i」を要求したため、採用領域は λ0 <= 1e-4 かつ
//!   λ1 <= 1e-4 の v2 角の相対幅 1e-4 の楔だけだった (通常サイズの
//!   三角形では texel 中心が楔に入らず書込みゼロ、巨大三角形でも角の
//!   楔のみの誤記述 — テスト不在で潜在していた complete-dead 級欠陥、
//!   wave 82 CF-3 で根治)。
//! - **「lock-free」注記**: TemporalHysteresisBuffer は単一スレッド前提の
//!   フラットテーブルでアトミックは使わない (旧ヘッダの表現を誠実化)。
//!   ハッシュ衝突時はストリークが共有/リセットされる方向のみ
//!   (遮蔽が遅れる保守方向。&& 合成で HashMap 側が厳密なので過早発火は
//!   数学的に不可能)。

use std::collections::HashMap;

/// Halton (2, 3) sequence generator for temporal subpixel jitter.
/// 値域は [-0.5, +0.5] の決定論的低分散列 (基 2/基 3 の radical inverse)。
pub struct HaltonJitter;

impl HaltonJitter {
    #[inline]
    pub fn get(index: usize) -> [f32; 2] {
        let x = Self::halton_base(index as u32 + 1, 2) - 0.5;
        let y = Self::halton_base(index as u32 + 1, 3) - 0.5;
        [x, y]
    }

    fn halton_base(mut i: u32, base: u32) -> f32 {
        let mut f = 1.0;
        let mut r = 0.0;
        let b = base as f32;
        while i > 0 {
            f /= b;
            r += f * (i % base) as f32;
            i /= base;
        }
        r
    }
}

/// 衝突許容フラットテーブル型テンポラルヒステリシス。
/// 4096 スロット (2 冪) にハッシュで写像し、衝突キーは同じカウンタを
/// 共有する (衝突時は遮蔽判定が遅れる保守方向のみに働く)。
#[derive(Debug, Clone)]
pub struct TemporalHysteresisBuffer {
    table: Vec<u8>,
    mask: usize,
}

impl TemporalHysteresisBuffer {
    pub fn new(capacity_pow2: usize) -> Self {
        let cap = capacity_pow2.max(16).next_power_of_two();
        Self {
            table: vec![0; cap],
            mask: cap - 1,
        }
    }

    #[inline]
    fn hash_key(key: (i32, i32)) -> usize {
        let mut h = key.0 as u32 ^ (key.1 as u32).rotate_left(16);
        h ^= h >> 16;
        h = h.wrapping_mul(0x85ebca6b);
        h ^= h >> 13;
        h as usize
    }

    pub fn update(&mut self, key: (i32, i32), occluded: bool, threshold: u8) -> bool {
        let idx = Self::hash_key(key) & self.mask;
        let entry = &mut self.table[idx];
        if occluded {
            *entry = entry.saturating_add(1);
        } else {
            *entry = 0;
        }
        *entry >= threshold
    }
}

/// hysteresis HashMap の上限エントリ数 (wave 82 で導入)。
/// 超過時は全クリア (ストリークリセットのみの保守方向) でメモリ定数上界化。
/// 16384 エントリ ≒ 682 列 x 24 断面分で実用上界として十分大きい。
const MAX_HYSTERESIS_ENTRIES: usize = 16384;

#[derive(Debug, Clone)]
pub struct SoftwareOcclusion {
    pub width: u32,
    pub height: u32,
    /// Mip 0 = full res depth (0 = near, 65535 = far)。texel 値は被覆保証 T
    /// (その texel 全域が深度値 ≤ T の幾何で覆われる)。上位 mip は子の max
    /// (= 4 子の最弱保証、遮蔽判定が保守方向にのみ解れる)。
    pub mips: Vec<Vec<u16>>,
    /// Frames each chunk key has been continuously occluded.
    pub hysteresis: HashMap<(i32, i32), u8>,
    /// 遮蔽判定に必要な連続フレーム数。0 は 1 と同じ即時発火として扱う
    /// (内部で `max(1)` に丸める。負方向の休符期間シュリンクは起きない)。
    pub hysteresis_frames: u8,
    pub fast_hysteresis: TemporalHysteresisBuffer,
    /// 設計予備: Halton(2,3) ジッター適用のための周期インデックス。
    /// 現行は射影に適用していない (ヘッダ注記参照)。
    pub jitter_index: usize,
}

impl SoftwareOcclusion {
    pub fn new(width: u32, height: u32) -> Self {
        let w = width.max(16).next_power_of_two();
        let h = height.max(16).next_power_of_two();
        let mut mips = Vec::new();
        let mut cw = w;
        let mut ch = h;
        loop {
            mips.push(vec![65535u16; (cw * ch) as usize]);
            if cw == 1 && ch == 1 {
                break;
            }
            cw = (cw / 2).max(1);
            ch = (ch / 2).max(1);
        }
        Self {
            width: w,
            height: h,
            mips,
            hysteresis: HashMap::new(),
            hysteresis_frames: 3,
            fast_hysteresis: TemporalHysteresisBuffer::new(4096),
            jitter_index: 0,
        }
    }

    pub fn clear_far(&mut self) {
        for mip in &mut self.mips {
            mip.fill(65535);
        }
        self.jitter_index = (self.jitter_index + 1) & 7;
    }

    fn mip_size(&self, level: usize) -> (u32, u32) {
        let w = (self.width >> level).max(1);
        let h = (self.height >> level).max(1);
        (w, h)
    }

    /// Write a screen-space rect with depth (smaller = closer).
    /// 完全に画面外の rect は early-out (**clamp 前に** intersection を
    /// 判定する。旧実装は画面外 rect を端列/端行に誤記述し、画面外の
    /// occluder が辺縁の遮蔽を偽装しえた — wave 82 で根治)。
    pub fn rasterize_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, depth: u16) {
        let (w, h) = (self.width as i32, self.height as i32);
        if x1 < 0 || y1 < 0 || x0 >= w || y0 >= h {
            return; // 交差なし (clamp すると端へ誤記述する)
        }
        let xa = x0.clamp(0, w - 1);
        let xb = x1.clamp(0, w - 1);
        let ya = y0.clamp(0, h - 1);
        let yb = y1.clamp(0, h - 1);
        if xa > xb || ya > yb {
            return;
        }
        let mip0 = &mut self.mips[0];
        for y in ya..=yb {
            let row = (y as u32 * self.width) as usize;
            for x in xa..=xb {
                let i = row + x as usize;
                if depth < mip0[i] {
                    mip0[i] = depth;
                }
            }
        }
    }

    /// スカラー重心中心サンプルの三角形ラスタライズ (高分解能 occluder 用)。
    /// 内側判定は 3 辺関数の**同符号性** (全て >= 0 または全て <= 0) で
    /// 行う — barycentric 内側 ⟺ λ_i >= 0 ∀i に等価で巻き向き不変。
    /// 辺関数は全て直接計算する: `w2 = 1-w0-w1` の恒等式は w_i が真の
    /// barycentric と符号整合する場合にのみ成立する。このコードの定義は
    /// w_i ≡ -λ_i (常に符号反転) なので、旧実装の判定「w0 >= -1e-4、
    /// w1 >= -1e-4、w2 = 1-w0-w1 >= -1e-4」は λ0 <= 1e-4 かつ λ1 <= 1e-4
    /// に帰着し、v2 角の相対幅 1e-4 の楔にしか書けなかった (通常サイズの
    /// 三角形では書込みゼロ = complete-dead 級、テスト不在で潜在。
    /// wave 82 CF-3 で根治)。
    /// texel 中心が厳密に三角形内のものだけに被覆保証
    /// T = 3 頂点の最大 ndc_z (クランプ [0,1]) を書く
    /// (中心サンプルは部分被覆 texel を拾わない = 保証は真)。
    /// いずれかの頂点が w <= 1e-6 (背面/退化) なら全沉默して棄却、
    /// 全頂点が far 超過 (nz > 1) でも棄却 (どちらも undercoverage = 保守)。
    pub fn rasterize_triangle_swar(
        &mut self,
        v0: [f32; 3],
        v1: [f32; 3],
        v2: [f32; 3],
        view_proj: &[[f32; 4]; 4],
    ) {
        let (sx0, sy0, sz0, ok0) = project_screen(v0, view_proj, self.width, self.height);
        let (sx1, sy1, sz1, ok1) = project_screen(v1, view_proj, self.width, self.height);
        let (sx2, sy2, sz2, ok2) = project_screen(v2, view_proj, self.width, self.height);
        if !ok0 || !ok1 || !ok2 {
            return;
        }
        if sz0.min(sz1).min(sz2) > 1.0 {
            return; // 全頂点が far 超過 → 画面には何も描かれない (棄却 = 保守)
        }

        let min_x = sx0.min(sx1).min(sx2).max(0.0) as i32;
        let max_x = sx0.max(sx1).max(sx2).min((self.width - 1) as f32) as i32;
        let min_y = sy0.min(sy1).min(sy2).max(0.0) as i32;
        let max_y = sy0.max(sy1).max(sy2).min((self.height - 1) as f32) as i32;
        if min_x > max_x || min_y > max_y {
            return;
        }

        let area = (sx2 - sx0) * (sy1 - sy0) - (sy2 - sy0) * (sx1 - sx0);
        if area.abs() < 1e-4 {
            return; // 縮退 (面積ほぼゼロ): 被覆の証拠不十分 → 棄却
        }
        let inv_area = 1.0 / area;
        let depth_val = ((sz0.max(sz1).max(sz2)).clamp(0.0, 1.0) * 65535.0) as u16;
        let mip0 = &mut self.mips[0];

        for y in min_y..=max_y {
            let fy = y as f32 + 0.5;
            let row = (y as u32 * self.width) as usize;
            for x in min_x..=max_x {
                let fx = x as f32 + 0.5;
                // 3 辺関数全てを直接計算 (恒等式 w2=1-w0-w1 は不成立、CF-3)。
                let w0 = ((sx2 - sx1) * (fy - sy1) - (sy2 - sy1) * (fx - sx1)) * inv_area;
                let w1 = ((sx0 - sx2) * (fy - sy2) - (sy0 - sy2) * (fx - sx2)) * inv_area;
                let w2 = ((sx1 - sx0) * (fy - sy0) - (sy1 - sy0) * (fx - sx0)) * inv_area;
                // 同符号性 = barycentric 内側と等価 (巻き向き不変)。
                let inside =
                    (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0);
                if inside {
                    let idx = row + x as usize;
                    if depth_val < mip0[idx] {
                        mip0[idx] = depth_val;
                    }
                }
            }
        }
    }

    /// 2D 凸包 (monotone chain、f64、CCW)。透视射影は w > 0 半空間で
    /// 凸性を保存するので、これは AABB の厳密な射影 silhouette である。
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

    /// 本番行列 (p x M 規約) で AABB を厳密 rasterize する。
    /// 被覆保証 T = 8 隅の最大 ndc_z。 silhouette (8 隅の 2D 凸包) に
    /// **完全内包される texel のみ**に書く (部分被覆には保証を与えられない
    /// — 書き損ねは遮蔽過小 = 保守方向)。
    /// 棄却条件: いずれかの隅が w <= 1e-6 (背面跨ぎ)、または全隅が
    /// far 超過 (nz > 1)。画面外にはみ出す部分は字幅の切詰めで
    /// 交差部分のみが埋まる (捨てるのは保守)。
    pub fn rasterize_aabb(&mut self, min: [f32; 3], max: [f32; 3], view_proj: &[[f32; 4]; 4]) {
        let mut pixels = [(0.0f64, 0.0f64); 8];
        let mut z_min = f32::INFINITY;
        let mut z_max = f32::NEG_INFINITY;
        let mut n = 0;
        for c in Self::aabb_corners(min, max) {
            // project_screen を使う (ガードバンド非適用): 画面に大きく写る
            // occluder の |ndc|>1.2 の隅で全沉默しないため — CF-5。
            let (sx, sy, z, ok) = project_screen(c, view_proj, self.width, self.height);
            if !ok {
                return; // 背面跨ぎ → 保守フォールバック (全沉默して棄却)
            }
            z_min = z_min.min(z);
            z_max = z_max.max(z);
            pixels[n] = (sx as f64, sy as f64);
            n += 1;
        }
        if z_min > 1.0 {
            return; // 全隅が far 超過 → 描かれない occluder なので棄却
        }
        let depth = (z_max.clamp(0.0, 1.0) * 65535.0) as u16;
        let (hull, hull_len) = Self::convex_hull(&pixels);
        self.fill_hull_fully_inside(&hull, hull_len, depth);
    }

    /// 凸包 scanline: 完全内包 texel のみに T を書く。
    /// 帯 [y,y+1] 上下端の交差区間の狭い側を ceil/floor で内側 texel 化。
    /// 極端 ndc (|ndc|>>1) の座標オーバーフローを ±4 画面の防御 clamp で防ぐ
    /// (画面外成分は書込み範囲の切詰めで自然に消える)。
    fn fill_hull_fully_inside(&mut self, hull: &[(f64, f64); 8], hull_len: usize, depth: u16) {
        let n = hull_len;
        if n < 3 {
            return; // 縮退化: 面積の保証がない → 書かない
        }
        let (w2, h2) = (self.width as f64, self.height as f64);
        let (xc, yc) = (4.0 * w2, 4.0 * h2);
        let mut ymin = f64::INFINITY;
        let mut ymax = f64::NEG_INFINITY;
        for i in 0..n {
            ymin = ymin.min(hull[i].1);
            ymax = ymax.max(hull[i].1);
        }
        let first = ((ymin.clamp(-yc, yc)).ceil() as i64).max(0);
        let last = (((ymax.clamp(-yc, yc)).ceil() as i64) - 1).min(self.height as i64 - 1);
        let width_i = self.width as i64;
        for row in first..=last {
            let span = |yy: f64| -> Option<(f64, f64)> {
                let mut lo = f64::INFINITY;
                let mut hi = f64::NEG_INFINITY;
                for i in 0..n {
                    let (x1, y1) = hull[i];
                    let (x2, y2) = hull[(i + 1) % n];
                    let (ey_lo, ey_hi) = if y1 <= y2 { (y1, y2) } else { (y2, y1) };
                    if ey_lo <= yy && yy < ey_hi {
                        let t = (yy - y1) / (y2 - y1);
                        let x = x1 + t * (x2 - x1);
                        lo = lo.min(x);
                        hi = hi.max(x);
                    }
                }
                if lo <= hi {
                    Some((lo.clamp(-xc, xc), hi.clamp(-xc, xc)))
                } else {
                    None
                }
            };
            let (s0, s1) = (span(row as f64), span(row as f64 + 1.0));
            let (xa, xb) = match (s0, s1) {
                (Some(a), Some(b)) => (a.0.max(b.0), a.1.min(b.1)),
                _ => continue, // 片側のみ交差は保守に書かない
            };
            let x_start = (xa.ceil() as i64).max(0);
            let x_end = ((xb.ceil() as i64) - 1).min(width_i - 1);
            let mip0 = &mut self.mips[0];
            for col in x_start..=x_end {
                let i = row as usize * self.width as usize + col as usize;
                if depth < mip0[i] {
                    mip0[i] = depth;
                }
            }
        }
    }

    fn aabb_corners(min: [f32; 3], max: [f32; 3]) -> [[f32; 3]; 8] {
        [
            [min[0], min[1], min[2]],
            [max[0], min[1], min[2]],
            [min[0], max[1], min[2]],
            [max[0], max[1], min[2]],
            [min[0], min[1], max[2]],
            [max[0], min[1], max[2]],
            [min[0], max[1], max[2]],
            [max[0], max[1], max[2]],
        ]
    }

    pub fn build_pyramid(&mut self) {
        for level in 1..self.mips.len() {
            let (pw, ph) = self.mip_size(level - 1);
            let (w, h) = self.mip_size(level);
            let (left_slice, right_slice) = self.mips.split_at_mut(level);
            let prev = &left_slice[level - 1];
            let cur = &mut right_slice[0];

            for y in 0..h {
                for x in 0..w {
                    let x0 = x * 2;
                    let y0 = y * 2;
                    let s0 = prev[(y0.min(ph - 1) * pw + x0.min(pw - 1)) as usize];
                    let s1 = prev[(y0.min(ph - 1) * pw + (x0 + 1).min(pw - 1)) as usize];
                    let s2 = prev[((y0 + 1).min(ph - 1) * pw + x0.min(pw - 1)) as usize];
                    let s3 = prev[((y0 + 1).min(ph - 1) * pw + (x0 + 1).min(pw - 1)) as usize];
                    cur[(y * w + x) as usize] = s0.max(s1).max(s2).max(s3);
                }
            }
        }
    }

    /// Returns true if AABB is occluded (behind Hi-Z).
    /// occludee 側は 8 隅の**最小** ndc_z (その箱が提示しうる最大の近さ)
    /// で比較し、tile max との strict な大なりでのみ遮蔽成立
    /// (等値 = 安全に隠れているとは言えない → 不成立、これは自己遮蔽を
    /// 数学的に不可能にする境界規則)。いずれかの隅がガードバンド
    /// (|ndc| <= 1.2) または w <= 1e-6 に外れる場合は「部分的に画面外で
    /// 完全被覆を保証できない」ので遮蔽不可 (保守的に可視扱い)。
    pub fn test_aabb_occluded(
        &self,
        min: [f32; 3],
        max: [f32; 3],
        view_proj: &[[f32; 4]; 4],
    ) -> bool {
        let mut sx0 = f32::MAX;
        let mut sy0 = f32::MAX;
        let mut sx1 = f32::MIN;
        let mut sy1 = f32::MIN;
        let mut nearest_z = f32::MAX;
        let mut any = false;
        for c in Self::aabb_corners(min, max) {
            let (sx, sy, z, ok) = project(c, view_proj, self.width, self.height);
            if !ok {
                return false; // partially offscreen / 背面跨ぎ → not safely occluded
            }
            any = true;
            sx0 = sx0.min(sx);
            sy0 = sy0.min(sy);
            sx1 = sx1.max(sx);
            sy1 = sy1.max(sy);
            nearest_z = nearest_z.min(z);
        }
        if !any {
            return false;
        }
        let test_depth = (nearest_z.clamp(0.0, 1.0) * 65535.0) as u16;
        let rw = (sx1 - sx0).max(1.0);
        let rh = (sy1 - sy0).max(1.0);
        let mut level = 0usize;
        while level + 1 < self.mips.len() {
            let (mw, _) = self.mip_size(level);
            let span = rw.max(rh) * (mw as f32 / self.width as f32);
            if span <= 2.0 {
                break;
            }
            level += 1;
        }
        let (mw, mh) = self.mip_size(level);
        let mx0 = ((sx0 / self.width as f32) * mw as f32) as u32;
        let my0 = ((sy0 / self.height as f32) * mh as f32) as u32;
        let mx1 = ((sx1 / self.width as f32) * mw as f32) as u32;
        let my1 = ((sy1 / self.height as f32) * mh as f32) as u32;
        let mip = &self.mips[level];
        let mut farthest = 0u16;
        for y in my0..=my1.min(mh - 1) {
            for x in mx0..=mx1.min(mw - 1) {
                farthest = farthest.max(mip[(y * mw + x) as usize]);
            }
        }
        test_depth > farthest
    }

    /// ヒステリシス更新。戻り値は「正確な per-key 連続カウント (HashMap)
    /// と衝突許容フラットテーブルの AND」: 過早発火は数学的に不可能で、
    /// ハッシュ圧で発火が**遅れる**方向のみに働く (保守)。HashMap は
    /// MAX_HYSTERESIS_ENTRIES で定数上界 (超過時は全クリア = 全体を
    /// 遅らせる保守方向)。threshold は `hysteresis_frames.max(1)`。
    pub fn update_hysteresis(&mut self, key: (i32, i32), occluded: bool) -> bool {
        if self.hysteresis.len() >= MAX_HYSTERESIS_ENTRIES {
            self.hysteresis.clear();
        }
        let threshold = self.hysteresis_frames.max(1);
        let entry = self.hysteresis.entry(key).or_insert(0);
        if occluded {
            *entry = entry.saturating_add(1);
        } else {
            *entry = 0;
        }
        let map_res = *entry >= threshold;
        let fast_res = self.fast_hysteresis.update(key, occluded, threshold);
        map_res && fast_res
    }

    /// Test + temporal hysteresis (hide only after N consecutive occluded frames).
    pub fn is_occluded_hysteresis(
        &mut self,
        key: (i32, i32),
        min: [f32; 3],
        max: [f32; 3],
        view_proj: &[[f32; 4]; 4],
    ) -> bool {
        let occluded = self.test_aabb_occluded(min, max, view_proj);
        self.update_hysteresis(key, occluded)
    }
}

/// 本番行列規約 (p x M) の射影: clip_i = Σ_j p_j * vp[j][i]。
/// w <= 1e-6 は背面/退化として拒否 (**w > 0 を必須**とし、旧 |w| 検査が
/// 黙認していた背面角の鏡写し混入を根絶 — wave 82 CF-1)。
/// ndc は x,y をガードバンド [-1.2, 1.2] で検査する (test 側専用の保守
/// 規則: 一部でも遠く画面外の occludee は「完全被覆が保証できない」ので
/// 遮蔽不可 = 保守的に可視扱い。rasterize は band を掛けない
/// `project_screen` を使う — CF-5)。
/// z は本番行列の ndc_z をそのまま返す (DX12 式 [0,1]、near→0、far→1。
/// 旧実装の [-1,1] 前提 `*0.5+0.5` 再マップは撤廃)。
fn project(p: [f32; 3], vp: &[[f32; 4]; 4], w: u32, h: u32) -> (f32, f32, f32, bool) {
    let x = vp[0][0] * p[0] + vp[1][0] * p[1] + vp[2][0] * p[2] + vp[3][0];
    let y = vp[0][1] * p[0] + vp[1][1] * p[1] + vp[2][1] * p[2] + vp[3][1];
    let z = vp[0][2] * p[0] + vp[1][2] * p[1] + vp[2][2] * p[2] + vp[3][2];
    let ww = vp[0][3] * p[0] + vp[1][3] * p[1] + vp[2][3] * p[2] + vp[3][3];
    if !(ww > 1e-6) {
        return (0.0, 0.0, 0.0, false); // NaN/背面/退化を一括拒否
    }
    let ndc_x = x / ww;
    let ndc_y = y / ww;
    let ndc_z = z / ww;
    if !(-1.2..=1.2).contains(&ndc_x) || !(-1.2..=1.2).contains(&ndc_y) {
        return (0.0, 0.0, 0.0, false);
    }
    let sx = (ndc_x * 0.5 + 0.5) * w as f32;
    let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * h as f32;
    (sx, sy, ndc_z, true)
}

/// rasterize 専用の射影 (`project` と同一の p x M 規約・w > 1e-6 必須・
/// ndc_z 直接返却) だが、ガードバンド [-1.2, 1.2] は**適用しない**。
/// 画面の遥か外 (|ndc| >> 1) の角は convex_hull → ±4 画面防御 clamp →
/// 書込み範囲の切詰めで自然に処理できる。band で拒否すると画面に大きく
/// 写る occluder (眼前の壁など) が全沉默して深度ピラミッドが欠落し、
/// 遮蔽が**非保守方向**に崩れる — CF-5 で project (test 用) と分割。
fn project_screen(p: [f32; 3], vp: &[[f32; 4]; 4], w: u32, h: u32) -> (f32, f32, f32, bool) {
    let x = vp[0][0] * p[0] + vp[1][0] * p[1] + vp[2][0] * p[2] + vp[3][0];
    let y = vp[0][1] * p[0] + vp[1][1] * p[1] + vp[2][1] * p[2] + vp[3][1];
    let z = vp[0][2] * p[0] + vp[1][2] * p[1] + vp[2][2] * p[2] + vp[3][2];
    let ww = vp[0][3] * p[0] + vp[1][3] * p[1] + vp[2][3] * p[2] + vp[3][3];
    if !(ww > 1e-6) {
        return (0.0, 0.0, 0.0, false); // NaN/背面/退化を一括拒否
    }
    let ndc_x = x / ww;
    let ndc_y = y / ww;
    let ndc_z = z / ww;
    let sx = (ndc_x * 0.5 + 0.5) * w as f32;
    let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * h as f32;
    (sx, sy, ndc_z, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hzb_2d::CameraState;
    use crate::world_column_store::TerrainFrameConstants;

    /// 検算スクリプト (/tmp/cf_verify*.py、f32 往復厳密化) と同一の
    /// 本番カメラ: eye (0,64,0)、yaw 0、pitch 0、fov 1.0、aspect 1.0、
    /// near 0.05、far 512。
    fn vp() -> [[f32; 4]; 4] {
        let cam = CameraState {
            x: 0.0,
            y: 64.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 1.0,
            aspect: 1.0,
        };
        TerrainFrameConstants::from_camera(&cam, [0, 0, 0]).view_proj
    }

    #[test]
    fn hierarchy_builds() {
        let mut o = SoftwareOcclusion::new(64, 64);
        o.clear_far();
        o.rasterize_rect(10, 10, 20, 20, 100);
        o.build_pyramid();
        assert!(o.mips.len() > 3);
        assert_eq!(o.hysteresis_frames, 3);
    }

    #[test]
    fn test_halton_jitter() {
        let j = HaltonJitter::get(0);
        assert!((-0.5..=0.5).contains(&j[0]));
        assert!((-0.5..=0.5).contains(&j[1]));
    }

    /// CF-7: Halton(2,3) の先頭 4 項を検算スクリプトの f32 厳密値でピン
    /// (radical inverse の理論値: 1/2, 1/4, 3/4, 1/8 と 1/3, 2/3, 1/9, 4/9)。
    #[test]
    fn halton_first_four_exact() {
        let expect: [(u32, u32); 4] = [
            (0x00000000, 0xBE2AAAAA), // [+0.0, -1/6]
            (0xBE800000, 0x3E2AAAAC), // [-1/4, +1/6]
            (0x3E800000, 0xBEC71C72), // [+1/4, -7/18]
            (0xBEC00000, 0xBD638E38), // [-3/8, -1/18]
        ];
        for (i, (bx, by)) in expect.iter().enumerate() {
            let j = HaltonJitter::get(i);
            assert_eq!(j[0].to_bits(), *bx, "get({i})[0]");
            assert_eq!(j[1].to_bits(), *by, "get({i})[1]");
        }
    }

    /// CF-1: project は本番行列 (p x M 規約) と一致する。正面中央点は
    /// 画面中央 (32,32) に写り、w は前方距離 16、z は nz(16)=0x3F7F3994。
    /// 旧転置規約では平行移動が脱落した w = -64f*y になるため即座に検出。
    #[test]
    fn project_matches_production_matrix_convention() {
        let vp = vp();
        let (sx, sy, z, ok) = project([0.0, 64.0, 16.0], &vp, 64, 64);
        assert!(ok);
        assert_eq!(sx, 32.0, "正面中央 x");
        assert_eq!(sy, 32.0, "正面中央 y");
        assert_eq!(z.to_bits(), 0x3F7F3994, "nz(16) の f32 一意値");
        // 背面点は w <= 0 で拒否 (旧 |w| 検査なら鏡写しで受容していた)。
        let (_, _, _, ok_back) = project([0.0, 64.0, -10.0], &vp, 64, 64);
        assert!(!ok_back, "背面は必ず拒否");
        // 遠方点は nz > 1 (クランプしない = far 超過が検出可能)。
        let (_, _, z_far, ok_far) = project([0.0, 64.0, 600.0], &vp, 64, 64);
        assert!(ok_far);
        assert!(z_far > 1.0, "far 超過を保つ");
    }

    /// CF-4: 画面外 rect は clamp 前 early-out で一切書かない。
    /// 旧実装は左外 (x1<0) を列 0 に誤記述して辺縁遮蔽を偽装しえた。
    #[test]
    fn rect_early_out_never_writes_offscreen() {
        let mut o = SoftwareOcclusion::new(64, 64);
        o.rasterize_rect(-5, 10, -2, 20, 100); // 完全左外
        o.rasterize_rect(70, 10, 90, 20, 100); // 完全右外
        o.rasterize_rect(10, -9, 20, -3, 100); // 完全上外
        o.rasterize_rect(10, 65, 20, 99, 100); // 完全下外
        assert!(
            o.mips[0].iter().all(|&v| v == 65535),
            "完全画面外の 4 rect は何も書かない"
        );
        // 一部交差は交差部分だけ書く。
        o.rasterize_rect(-3, 10, 2, 12, 100);
        assert_eq!(o.mips[0][10 * 64 + 0], 100, "交差列 0 に書く");
        assert_eq!(o.mips[0][10 * 64 + 2], 100);
        assert_eq!(o.mips[0][10 * 64 + 3], 65535, "範囲外は書かない");
        // 深度は min (より近い T だけが上書きする)。
        o.rasterize_rect(0, 10, 2, 12, 50);
        o.rasterize_rect(0, 10, 2, 12, 150);
        assert_eq!(o.mips[0][10 * 64 + 1], 50, "最強保証 (最小 T) を保持");
    }

    /// CF-2/CF-5: rasterize_aabb は最遠 depth (T=65404) を凸包完全内包
    /// texel のみに書く。検算: 本番 box [(-8,60,8),(8,68,24)] の凸包は
    /// 行 3..=60 (58 行) x 全 64 列 = 3712 texel を覆い、行 2 は含まない。
    #[test]
    fn rasterize_aabb_farthest_depth_fully_inside_texels_only() {
        let mut o = SoftwareOcclusion::new(64, 64);
        o.rasterize_aabb([-8.0, 60.0, 8.0], [8.0, 68.0, 24.0], &vp());
        let filled = o.mips[0].iter().filter(|&&v| v == 65404).count();
        assert_eq!(filled, 3712, "凸包完全内包 texel 数 (58 行 x 64 列)");
        assert_eq!(o.mips[0][3 * 64], 65404, "行 3 列 0 は内包");
        assert_eq!(o.mips[0][60 * 64 + 63], 65404, "行 60 末列は内包");
        assert_eq!(o.mips[0][2 * 64], 65535, "行 2 は内包外 (書かない)");
        assert_eq!(o.mips[0][61 * 64], 65535, "行 61 は内包外");
        assert!(
            o.mips[0].iter().all(|&v| v == 65404 || v == 65535),
            "最遠値 T 以外は書いていない (旧最近値 65131 は存在しない)"
        );
    }

    /// CF-5: 背面 box / 跨ぎ box / 全 far 超過 box は rasterize を棄却
    /// (undercoverage = 保守方向、深度バッファ無汚染)。
    #[test]
    fn rasterize_aabb_rejects_behind_straddling_and_beyond_far() {
        let mut o = SoftwareOcclusion::new(64, 64);
        o.rasterize_aabb([-8.0, 60.0, -10.0], [8.0, 68.0, -4.0], &vp()); // 全背面
        o.rasterize_aabb([-8.0, 60.0, -2.0], [8.0, 68.0, 10.0], &vp()); // 跨ぎ
        o.rasterize_aabb([-8.0, 60.0, 600.0], [8.0, 68.0, 610.0], &vp()); // 全 far 超過
        assert!(
            o.mips[0].iter().all(|&v| v == 65535),
            "背面/跨ぎ/far 超過は何も書かない"
        );
    }

    /// CF-2 統合: 遮蔽は occludee の最近値が tile max を strict に超えた
    /// ときのみ。等値境界 (min_z=24 → test_depth 65404 == T) は成立せず、
    /// F box (test_depth 65432 > 65404) のみ成立 → 3 フレーム連続で発火。
    #[test]
    fn occlusion_requires_strictly_farthest_nearest_depth() {
        let vp = vp();
        let mut o = SoftwareOcclusion::new(64, 64);
        o.rasterize_aabb([-8.0, 60.0, 8.0], [8.0, 68.0, 24.0], &vp);
        o.build_pyramid();
        // 等値境界: min_z=24 の箱は「同じ深さで隣接」なので絶対に遮蔽されない。
        for _ in 0..4 {
            assert!(
                !o.is_occluded_hysteresis((9, 9), [-8.0, 60.0, 24.0], [8.0, 68.0, 40.0], &vp),
                "等値境界は strict 大なりで不成立"
            );
        }
        // F box: 65432 > 65404 で遮蔽成立 → 3 回目で発火。
        assert!(!o.is_occluded_hysteresis((7, 7), [-8.0, 60.0, 30.0], [8.0, 68.0, 46.0], &vp));
        assert!(!o.is_occluded_hysteresis((7, 7), [-8.0, 60.0, 30.0], [8.0, 68.0, 46.0], &vp));
        assert!(
            o.is_occluded_hysteresis((7, 7), [-8.0, 60.0, 30.0], [8.0, 68.0, 46.0], &vp),
            "3 フレーム目で発火 (hysteresis_frames=3)"
        );
    }

    /// CF-3: 三角形ラスタは巻き向き不変 (旧実装は片巻きを全域拒否)。
    /// T = 3 頂点の max nz (65336)。中心サンプル w_i >= 0 厳密形の
    /// 内側/外側セルは検算ミラー確定値どおり。
    #[test]
    fn triangle_rasterize_winding_invariant_exact_cells() {
        let vp = vp();
        let v0 = [0.0, 68.0, 16.0];
        let v1 = [-8.0, 60.0, 16.0];
        let v2 = [8.0, 60.0, 16.0];
        let inside = [(32, 40), (10, 46), (32, 46), (32, 18), (45, 35), (4, 46)];
        let outside = [(57, 40), (33, 17), (32, 47), (62, 40), (2, 40), (1, 46)];
        // 両巻き向きで同一のフィル結果を 2 インスタンスで直接比較。
        let mut a = SoftwareOcclusion::new(64, 64);
        a.rasterize_triangle_swar(v0, v1, v2, &vp);
        let mut b = SoftwareOcclusion::new(64, 64);
        b.rasterize_triangle_swar(v0, v2, v1, &vp); // 逆巻き
        for (x, y) in inside {
            assert_eq!(a.mips[0][y * 64 + x], 65336, "inside ({x},{y})");
            assert_eq!(b.mips[0][y * 64 + x], 65336, "inside 逆巻き ({x},{y})");
        }
        for (x, y) in outside {
            assert_eq!(a.mips[0][y * 64 + x], 65535, "outside ({x},{y})");
            assert_eq!(b.mips[0][y * 64 + x], 65535, "outside 逆巻き ({x},{y})");
        }
    }

    /// CF-3 厳密度: same-sign 判定はトレランスを持たない。上辺から
    /// 1.2e-3 (w2 = +1.9074e-05、bits 0x37A000C8) だけ厳密に外側の
    /// texel 中心 (32.5, 0.5) は w 符号混在 (w0<0, w1<0, w2>0) で
    /// 書かない。旧実装 (w_i ≡ -λ_i への >= -1e-4 判定) の採用領域は
    /// v2 角の相対幅 1e-4 の楔のみ = 「漏れ」を起こさない代わりに
    /// 通常では何も書かない complete-dead 級ルールだった。恒等 vp で
    /// 世界→画面座標を直接制御。全値は検算ミラー (f32 往復) 確定。
    #[test]
    fn triangle_strict_center_rule_rejects_barely_outside_center() {
        let ident: [[f32; 4]; 4] = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        // 上辺 sy_e = 0.5012016296386719: ny_e = f32(0x3F7BFD8A) から
        // sy=(1-(ny*0.5+0.5))*64 の f32 厳密評価で導出。
        let ny_e = f32::from_bits(0x3F7BFD8A); // 0.9843374490737915
        let v0 = [0.96875, ny_e, 0.5]; // (sx,sy)=(63.0, sy_e)
        let v1 = [f32::from_bits(0xBF3C0000), ny_e, 0.5]; // (8.5, sy_e)
        let v2 = [0.0, -0.984375, 0.5]; // (32.0, 63.5)
        let mut o = SoftwareOcclusion::new(64, 64);
        o.rasterize_triangle_swar(v0, v1, v2, &ident);
        assert_eq!(
            o.mips[0][32], 65535,
            "セル (32,0): 中心 (32.5,0.5) は上辺の 1.2e-3 上で厳密外 (w 符号混在) → 書かない"
        );
        assert_eq!(
            o.mips[0][64 + 32],
            32767,
            "セル (32,1): 内側、T=0.5*65535 as u16"
        );
        assert_eq!(o.mips[0][30 * 64 + 32], 32767, "セル (32,30) も内側");
    }

    /// CF-6: hysteresis_frames=0/1 の即時発火規約と、&& 合成の非過早性。
    #[test]
    fn hysteresis_threshold_semantics() {
        let mut o = SoftwareOcclusion::new(64, 64);
        o.hysteresis_frames = 0;
        assert!(
            o.update_hysteresis((1, 1), true),
            "0 は max(1) で即時発火 (1 回の正値で発火)"
        );
        let mut o = SoftwareOcclusion::new(64, 64);
        o.hysteresis_frames = 1;
        assert!(o.update_hysteresis((2, 2), true), "1 も即時発火");
        // 可視フレームはリセット。
        let mut o = SoftwareOcclusion::new(64, 64);
        o.hysteresis_frames = 3;
        assert!(!o.update_hysteresis((3, 3), true));
        assert!(!o.update_hysteresis((3, 3), false), "可視挿入は発火しない");
        assert!(!o.update_hysteresis((3, 3), true));
        assert!(
            !o.update_hysteresis((3, 3), true),
            "連続 3 回未満で発火しない"
        );
        assert!(o.update_hysteresis((3, 3), true), "連続 3 回で発火");
    }

    /// CF-6: HashMap 16384 cap (超過時は全クリア = 保守全リセット)。
    #[test]
    fn hysteresis_map_is_capped() {
        let mut o = SoftwareOcclusion::new(64, 64);
        for i in 0..MAX_HYSTERESIS_ENTRIES as i32 {
            o.update_hysteresis((i, 0), true);
        }
        assert_eq!(o.hysteresis.len(), MAX_HYSTERESIS_ENTRIES);
        o.update_hysteresis((i32::MAX, 1), true);
        assert_eq!(o.hysteresis.len(), 1, "上限超過 → 全クリア後に 1 件");
    }

    /// CF-6: fast テーブルのハッシュ衝突は発火を**遅らせる**方向のみに
    /// 働くことをピン (&& 合成のため過早発火は不可能)。同一スロットの
    /// 衝突キー k2 を可視挿入すると共有スロットがリセットされ、
    /// k1 は map 3 到達後も fast が追いつくまで最大 +2 フレーム遅れる。
    /// 衝突ペアは同一ハッシュ関数で機械探索。
    #[test]
    fn hysteresis_collision_delays_never_accelerates() {
        let mask = 4095usize;
        let k1 = (0i32, 0i32);
        let k2 = (1..)
            .map(|i| (i as i32, 0i32))
            .find(|&k| {
                TemporalHysteresisBuffer::hash_key(k) & mask
                    == TemporalHysteresisBuffer::hash_key(k1) & mask
            })
            .expect("4096 スロットで衝突キーが必ず見つかる");
        assert_ne!(k1, k2);
        let mut o = SoftwareOcclusion::new(64, 64);
        assert!(!o.update_hysteresis(k1, true)); // map 1, slot 1
        assert!(!o.update_hysteresis(k1, true)); // map 2, slot 2
                                                 // 衝突キーを可視挿入 → 共有スロット 0 リセット (map(k2) は独立)。
        assert!(!o.update_hysteresis(k2, false));
        // k1 は map 3 到達するが fast が 1 から積み直し → +2 フレーム遅延。
        assert!(!o.update_hysteresis(k1, true), "map 3 だが slot 1 → 遅延");
        assert!(!o.update_hysteresis(k1, true), "slot 2 → まだ遅延");
        assert!(
            o.update_hysteresis(k1, true),
            "slot 3 で発火 (正確に +2 遅延)"
        );
    }
}
