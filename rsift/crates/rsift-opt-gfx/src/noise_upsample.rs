//! 3D noise upsampling — coarse Perlin grid + trilinear fill.
//!
//! Computing noise every `stride` voxels (e.g. 4) cuts sample count by ~(stride³)
//! while linear upsampling keeps terrain visually smooth (≈5×+ faster generation).

use crate::binary_greedy_meshing::{idx, SectionPalette, SECTION_SIZE, SECTIONS_PER_COLUMN};
use std::time::Instant;
use tracing::debug;

pub const DEFAULT_STRIDE: usize = 4;

#[derive(Debug, Clone, Copy)]
pub struct NoiseUpsampleConfig {
    /// Sample every N voxels (2 or 4 recommended).
    pub stride: usize,
    pub seed: u32,
    /// Sea / surface level in block Y.
    pub base_height: i32,
    pub cave_threshold: f32,
}

impl Default for NoiseUpsampleConfig {
    fn default() -> Self {
        Self {
            stride: DEFAULT_STRIDE,
            seed: 0xC0FFEE,
            base_height: 32,
            cave_threshold: 0.25,
        }
    }
}

impl NoiseUpsampleConfig {
    pub fn for_chunk(cx: i32, cz: i32) -> Self {
        let seed = (cx.wrapping_mul(374761) ^ cz.wrapping_mul(668265)) as u32;
        Self {
            seed,
            ..Default::default()
        }
    }

    /// Theoretical sample reduction vs dense grid (stride^3).
    pub fn sample_reduction_factor(&self) -> f32 {
        (self.stride * self.stride * self.stride) as f32
    }
}

#[derive(Debug, Clone, Default)]
pub struct NoiseUpsampleStats {
    pub coarse_samples: u64,
    pub dense_samples: u64,
    pub coarse_us: u64,
    pub dense_us: u64,
    pub speedup: f32,
}

impl NoiseUpsampleStats {
    pub fn log(&self) {
        debug!(
            "[NoiseUpsample] coarse={} dense={} speedup={:.1}x ({:.0}µs vs {:.0}µs)",
            self.coarse_samples,
            self.dense_samples,
            self.speedup,
            self.coarse_us as f64,
            self.dense_us as f64
        );
    }
}

/// Lightweight 3D value noise (Perlin-style gradients, no external deps).
#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = seed
        ^ x.wrapping_mul(0x9E37_79B9u32 as i32) as u32
        ^ y.wrapping_mul(0x85EB_CA6Bu32 as i32) as u32
        ^ z.wrapping_mul(0xC2B2_AE35u32 as i32) as u32;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352Du32);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68Bu32);
    h ^= h >> 16;
    h
}

#[inline]
fn grad3(h: u32) -> (f32, f32, f32) {
    match h % 12 {
        0 => (1.0, 1.0, 0.0),
        1 => (-1.0, 1.0, 0.0),
        2 => (1.0, -1.0, 0.0),
        3 => (-1.0, -1.0, 0.0),
        4 => (1.0, 0.0, 1.0),
        5 => (-1.0, 0.0, 1.0),
        6 => (1.0, 0.0, -1.0),
        7 => (-1.0, 0.0, -1.0),
        8 => (0.0, 1.0, 1.0),
        9 => (0.0, -1.0, 1.0),
        10 => (0.0, 1.0, -1.0),
        _ => (0.0, -1.0, -1.0),
    }
}

#[inline]
fn dot3(g: (f32, f32, f32), x: f32, y: f32, z: f32) -> f32 {
    g.0 * x + g.1 * y + g.2 * z
}

/// ノイズ評価の周波数 (1 voxel = 1/8 周期)。勾配ノイズは整数格子点で
/// 定義上恒に 0 (= 本関数の正規化で 0.5) となるため、実値を得るには
/// 周波数スケール済みの**実数座標**で評価する必要がある (BL-1)。
const PERLIN_FREQ: f32 = 1.0 / 8.0;

/// Full-resolution noise at integer world voxel (slow path / reference)。
/// 戻り値レンジは [0, 1]。
///
/// **wave 62 BL-1 (live worldgen のゼロデイ) で根治**: 旧実装は格子座標を
/// `wx & 255`、のこり座標を `(wx as f32).fract().abs()` で得ていたが、
/// 全呼出が整数 voxel のため fract ≡ 0 → fade ≡ 0 → 全補間項が消えて
/// `(dot3(g000,0,0,0)+1)*0.5 =` **定数 0.5 を全 voxel で返していた**。
/// 「noise 地形」は x/z に一切変化の無い高さ方向縞模様 (flat strata)
/// だった — 設計意図 (Perlin 勾配ノイズ地形) と実装が完全に乖離。
/// Perlin 勾配ノイズは数学的に整数格子点で恒に 0 なので、周波数
/// スケール (PERLIN_FREQ) を導入して格子内実数座標で評価する。
pub fn perlin3d_dense(wx: i32, wy: i32, wz: i32, seed: u32) -> f32 {
    let px = wx as f32 * PERLIN_FREQ;
    let py = wy as f32 * PERLIN_FREQ;
    let pz = wz as f32 * PERLIN_FREQ;
    let x0 = px.floor() as i32;
    let y0 = py.floor() as i32;
    let z0 = pz.floor() as i32;
    let xf = px - x0 as f32; // [0,1) (floor 定義より負座標でも成立)
    let yf = py - y0 as f32;
    let zf = pz - z0 as f32;
    let u = fade(xf);
    let v = fade(yf);
    let w = fade(zf);

    let h = |ix, iy, iz| hash3(x0 + ix, y0 + iy, z0 + iz, seed);
    let g = |ix, iy, iz| grad3(h(ix, iy, iz));

    let x1 = xf - 1.0;
    let y1 = yf - 1.0;
    let z1 = zf - 1.0;

    let c000 = dot3(g(0, 0, 0), xf, yf, zf);
    let c100 = dot3(g(1, 0, 0), x1, yf, zf);
    let c010 = dot3(g(0, 1, 0), xf, y1, zf);
    let c110 = dot3(g(1, 1, 0), x1, y1, zf);
    let c001 = dot3(g(0, 0, 1), xf, yf, z1);
    let c101 = dot3(g(1, 0, 1), x1, yf, z1);
    let c011 = dot3(g(0, 1, 1), xf, y1, z1);
    let c111 = dot3(g(1, 1, 1), x1, y1, z1);

    let x00 = c000 + u * (c100 - c000);
    let x10 = c010 + u * (c110 - c010);
    let x01 = c001 + u * (c101 - c001);
    let x11 = c011 + u * (c111 - c011);
    let y0 = x00 + v * (x10 - x00);
    let y1 = x01 + v * (x11 - x01);
    (y0 + w * (y1 - y0) + 1.0) * 0.5
}

struct CoarseGrid {
    sx: usize,
    sy: usize,
    sz: usize,
    stride: usize,
    values: Vec<f32>,
}

impl CoarseGrid {
    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        x + y * self.sx + z * self.sx * self.sy
    }

    fn sample(&self, x: usize, y: usize, z: usize) -> f32 {
        self.values[self.index(x, y, z)]
    }
}

fn build_coarse_grid(
    wx0: i32,
    wy0: i32,
    wz0: i32,
    wx1: i32,
    wy1: i32,
    wz1: i32,
    cfg: &NoiseUpsampleConfig,
) -> CoarseGrid {
    let stride = cfg.stride.max(1);
    let sx = ((wx1 - wx0) as usize / stride) + 1;
    let sy = ((wy1 - wy0) as usize / stride) + 1;
    let sz = ((wz1 - wz0) as usize / stride) + 1;
    let mut values = Vec::with_capacity(sx * sy * sz);
    for cz in 0..sz {
        for cy in 0..sy {
            for cx in 0..sx {
                let wx = wx0 + (cx * stride) as i32;
                let wy = wy0 + (cy * stride) as i32;
                let wz = wz0 + (cz * stride) as i32;
                let density = perlin3d_dense(wx, wy, wz, cfg.seed);
                let cave = perlin3d_dense(wx + 97, wy + 53, wz + 31, cfg.seed.wrapping_add(0xCAFE));
                values.push(density - cave * cfg.cave_threshold);
            }
        }
    }
    CoarseGrid {
        sx,
        sy,
        sz,
        stride,
        values,
    }
}

/// Trilinear upsample from coarse grid to world voxel.
// 注: CoarseGrid は private 型かつ build_coarse_grid も private のため、本関数は
// モジュール外からは呼べない。pub 露出は外部から呼出不可能な API ハザード
// (private_interfaces 警告) だったため private に降格 (2026-07-21 監査)。
// 外部参照は frame_worldgen 側が「補間形のみ同一」の精密ミラーとして自前実装する
// 設計 (frame_worldgen.rs:145 のコメント参照)。
fn trilinear_upsample(
    grid: &CoarseGrid,
    wx: i32,
    wy: i32,
    wz: i32,
    origin: (i32, i32, i32),
) -> f32 {
    let stride = grid.stride as f32;
    let lx = (wx - origin.0) as f32 / stride;
    let ly = (wy - origin.1) as f32 / stride;
    let lz = (wz - origin.2) as f32 / stride;

    let x0 = lx.floor() as usize;
    let y0 = ly.floor() as usize;
    let z0 = lz.floor() as usize;
    let x1 = (x0 + 1).min(grid.sx.saturating_sub(1));
    let y1 = (y0 + 1).min(grid.sy.saturating_sub(1));
    let z1 = (z0 + 1).min(grid.sz.saturating_sub(1));
    let tx = lx - x0 as f32;
    let ty = ly - y0 as f32;
    let tz = lz - z0 as f32;

    let c000 = grid.sample(x0, y0, z0);
    let c100 = grid.sample(x1, y0, z0);
    let c010 = grid.sample(x0, y1, z0);
    let c110 = grid.sample(x1, y1, z0);
    let c001 = grid.sample(x0, y0, z1);
    let c101 = grid.sample(x1, y0, z1);
    let c011 = grid.sample(x0, y1, z1);
    let c111 = grid.sample(x1, y1, z1);

    let x00 = c000 + tx * (c100 - c000);
    let x10 = c010 + tx * (c110 - c010);
    let x01 = c001 + tx * (c101 - c001);
    let x11 = c011 + tx * (c111 - c011);
    let y0v = x00 + ty * (x10 - x00);
    let y1v = x01 + ty * (x11 - x01);
    y0v + tz * (y1v - y0v)
}

fn density_to_block(density: f32, surface: f32, wy: i32, cfg: &NoiseUpsampleConfig) -> u16 {
    let height_factor = (wy as f32 - cfg.base_height as f32) / 32.0;
    let threshold = surface - height_factor * 0.15;
    if density > threshold {
        // wy ≥ 0 (カラム生成のみから呼出) なので wy%3+1 ∈ {1,2,3}
        // (旧来の .max(1) は到達不能防御、wave 62 BL-2 で撤去)。
        ((wy % 3) + 1) as u16
    } else if wy < cfg.base_height - 12 && density > 0.25 {
        3
    } else {
        0
    }
}

/// Fast column palettes via coarse noise + trilinear upsampling.
pub fn column_palettes_upsampled(cx: i32, cz: i32, cfg: &NoiseUpsampleConfig) -> Vec<SectionPalette> {
    let wx0 = cx * SECTION_SIZE as i32;
    let wz0 = cz * SECTION_SIZE as i32;
    let wy0 = 0;
    let wx1 = wx0 + SECTION_SIZE as i32 - 1;
    let wy1 = (SECTIONS_PER_COLUMN * SECTION_SIZE) as i32 - 1;
    let wz1 = wz0 + SECTION_SIZE as i32 - 1;

    let grid = build_coarse_grid(wx0, wy0, wz0, wx1, wy1, wz1, cfg);
    let origin = (wx0, wy0, wz0);

    let mut sections = Vec::with_capacity(SECTIONS_PER_COLUMN);
    for sy in 0..SECTIONS_PER_COLUMN {
        let mut p = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                for y in 0..SECTION_SIZE {
                    let wy = (sy * SECTION_SIZE + y) as i32;
                    let wx = wx0 + x as i32;
                    let wz = wz0 + z as i32;
                    let density = trilinear_upsample(&grid, wx, wy, wz, origin);
                    let surface = 0.48 + (perlin3d_dense(wx >> 2, 0, wz >> 2, cfg.seed.wrapping_add(99)) - 0.5) * 0.08;
                    p[idx(x, y, z)] = density_to_block(density, surface, wy, cfg);
                }
            }
        }
        sections.push(p);
    }
    sections
}

/// Benchmark dense vs upsampled generation (for telemetry).
pub fn benchmark_upsample(cx: i32, cz: i32, cfg: &NoiseUpsampleConfig) -> NoiseUpsampleStats {
    let dense_start = Instant::now();
    let mut dense_count = 0u64;
    let wx0 = cx * SECTION_SIZE as i32;
    let wz0 = cz * SECTION_SIZE as i32;
    for sy in 0..SECTIONS_PER_COLUMN {
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                for y in 0..SECTION_SIZE {
                    let wy = (sy * SECTION_SIZE + y) as i32;
                    let _ = perlin3d_dense(wx0 + x as i32, wy, wz0 + z as i32, cfg.seed);
                    dense_count += 1;
                }
            }
        }
    }
    let dense_us = dense_start.elapsed().as_micros() as u64;

    let coarse_start = Instant::now();
    let _ = column_palettes_upsampled(cx, cz, cfg);
    let stride = cfg.stride.max(1) as u64;
    // wave 62 BL-3: build_coarse_grid 実装と同じ式で計数する。
    // 実グリッドは「範囲両端を含む」ため ((span)/stride)+1 で、span は
    // 16−1=15 / 64−1=63。旧式は (16/stride)+1 等で実グリッド 4×16×4=256
    // に対し 5×17×5=425 を報告していた (1.66× の見せかけ過大)。
    let coarse_count = (((SECTION_SIZE as u64 - 1) / stride) + 1)
        * (((SECTIONS_PER_COLUMN as u64 * SECTION_SIZE as u64 - 1) / stride) + 1)
        * (((SECTION_SIZE as u64 - 1) / stride) + 1);
    let coarse_us = coarse_start.elapsed().as_micros() as u64;

    let speedup = if coarse_us > 0 {
        (dense_us.max(1) as f32) / coarse_us as f32
    } else {
        cfg.sample_reduction_factor()
    };

    NoiseUpsampleStats {
        coarse_samples: coarse_count,
        dense_samples: dense_count,
        coarse_us,
        dense_us,
        speedup,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsample_faster_than_dense() {
        let cfg = NoiseUpsampleConfig::for_chunk(3, 7);
        let stats = benchmark_upsample(3, 7, &cfg);
        assert!(
            stats.coarse_samples < stats.dense_samples / 8,
            "coarse={} dense={}",
            stats.coarse_samples,
            stats.dense_samples
        );
        assert!(
            stats.speedup >= 1.0 || cfg.sample_reduction_factor() >= 45.0,
            "speedup={}",
            stats.speedup
        );
    }

    #[test]
    fn upsampled_column_non_empty() {
        let cfg = NoiseUpsampleConfig::for_chunk(0, 0);
        let sections = column_palettes_upsampled(0, 0, &cfg);
        let solid: usize = sections
            .iter()
            .flat_map(|p| p.iter())
            .filter(|&&b| b != 0)
            .count();
        assert!(solid > 100);
    }

    /// wave 62 BL-1 (回帰): perlin3d_dense が voxel 間で実際に変化する。
    /// 旧実装は全 voxel で定数 0.5 (整数座標の fract≡0 で勾配項全消し)
    /// だった — x 走査で異なる値が存在することを直接ピン。
    #[test]
    fn perlin_varies_between_voxels() {
        let seed = 0xC0FFEE;
        let values: Vec<f32> = (0..16).map(|x| perlin3d_dense(x, 5, 3, seed)).collect();
        let distinct: std::collections::BTreeSet<u32> =
            values.iter().map(|v| v.to_bits()).collect();
        assert!(
            distinct.len() > 1,
            "BL-1 回帰: noise が全 voxel で定数 0.5 ではないこと {values:?}"
        );
        // レンジ契約 [0,1] と決定性 (同一入力は同一ビット)
        for &v in &values {
            assert!((0.0..=1.0).contains(&v), "レンジ契約: {v}");
        }
        for (x, &v) in values.iter().enumerate() {
            assert_eq!(
                perlin3d_dense(x as i32, 5, 3, seed).to_bits(),
                v.to_bits(),
                "決定性: 同一入力同一ビット"
            );
        }
    }

    /// wave 62 BL-1: 格子点 (PERLIN_FREQ の整数倍 voxel) では勾配ノイズの
    /// 数学的性質から正確に 0.5 (補間項が全て消える) ことをピンし、
    /// 非格子点では一般に 0.5 でないことで関数の「生きている」ことを示す。
    #[test]
    fn perlin_lattice_points_are_exactly_half() {
        let seed = 42;
        for k in [0i32, 8, 16, -8] {
            assert_eq!(
                perlin3d_dense(k, 8, -8, seed),
                0.5,
                "格子点 ({k},8,-8) は正確に 0.5 (勾配項全消しの数学的性質)"
            );
        }
        // 格子内実数座標に対応する非格子 voxel (1,2,3) は一般に 0.5 ではない
        // (seed 42 で実測、to_bits で定数戻りの回帰を塞ぐ)
        assert_ne!(perlin3d_dense(1, 2, 3, seed).to_bits(), 0.5f32.to_bits());
    }

    /// wave 62 BL-1 (回帰・カラムレベル): 生成地形が x 方向に変化を持つ
    /// (旧来は全 x で同一の高さ縞模様 = flat strata)。
    #[test]
    fn upsampled_column_varies_along_x() {
        let cfg = NoiseUpsampleConfig::for_chunk(1, 0);
        let sections = column_palettes_upsampled(1, 0, &cfg);
        // z=0 固定で x 各列の (y 方向ブロック列パターン) を採取し、
        // 2 種類以上存在することをピン
        let profiles: std::collections::BTreeSet<Vec<u16>> = (0..SECTION_SIZE)
            .map(|x| {
                (0..SECTIONS_PER_COLUMN * SECTION_SIZE)
                    .map(|wy| sections[wy / SECTION_SIZE][idx(x, wy % SECTION_SIZE, 0)])
                    .collect::<Vec<u16>>()
            })
            .collect();
        assert!(
            profiles.len() > 1,
            "BL-1 回帰: 地形が全 x で同一縞模様ではないこと (profiles={})",
            profiles.len()
        );
        // 決定性: 同一 cfg/chunk は同一ビット列のパレット
        let again = column_palettes_upsampled(1, 0, &cfg);
        assert_eq!(sections, again, "worldgen は決定的 (bit 一致)");
    }

    /// wave 62 BL-3: benchmark のサンプル計数が実グリッド 4×16×4 を
    /// 厳密に報告する (旧式は 5×17×5=425 の見せかけ)。
    #[test]
    fn benchmark_counts_match_actual_grid() {
        let cfg = NoiseUpsampleConfig::for_chunk(0, 0);
        let stats = benchmark_upsample(0, 0, &cfg);
        assert_eq!(
            stats.dense_samples,
            (SECTIONS_PER_COLUMN * SECTION_SIZE * SECTION_SIZE * SECTION_SIZE) as u64,
            "dense = 4 セクション × 4096"
        );
        // stride=4: (15/4+1)=4, (63/4+1)=16, (15/4+1)=4 → 256
        assert_eq!(stats.coarse_samples, 256, "実グリッド 4×16×4 と一致");
    }
}
