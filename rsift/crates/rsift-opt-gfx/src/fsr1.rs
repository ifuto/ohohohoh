//! FSR 1.0 — EASU (Edge-Aware Spatial Upsampling) + RCAS (Robust CAS) の CPU 参照実装。
//!
//! EASU は周辺 4 テクセルの輝度勾配からエッジを検出し、エッジに直交する方向の
//! サンプリング位置を中央へ寄せて（シャープにして）アップサンプリングする。
//! RCAS はその後ろのコントラスト適応シャープ化パス。

#[derive(Debug, Clone, Copy)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
}
impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

#[inline]
pub fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// EASU の 4-tap エッジ感知再構成。
/// `c` 中心、`nw/ne/sw/se` は 4 近傍。`fx,fy` は低解像度グリッド内の小数位置(0..1)。
pub fn easu_reconstruct(
    _c: [f32; 3],
    nw: [f32; 3],
    ne: [f32; 3],
    sw: [f32; 3],
    se: [f32; 3],
    fx: f32,
    fy: f32,
) -> [f32; 3] {
    let ln = luma(nw);
    let le = luma(ne);
    let ls = luma(sw);
    let lf = luma(se);
    // 水平・垂直の輝度勾配
    let gx = ((le + lf) - (ln + ls)).abs();
    let gy = ((ln + le) - (ls + lf)).abs();
    // エッジ強度 (0..~1)
    let ex = gx / (gx + 0.5);
    let ey = gy / (gy + 0.5);
    // エッジ沿いではサンプル位置を中心(0.5)に寄せてシャープ化
    let fx2 = fx + (0.5 - fx) * ex;
    let fy2 = fy + (0.5 - fy) * ey;
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let top = nw[i] + (ne[i] - nw[i]) * fx2;
        let bot = sw[i] + (se[i] - sw[i]) * fx2;
        out[i] = top + (bot - top) * fy2;
    }
    out
}

/// RCAS (Robust Contrast Adaptive Sharpening) — 簡約実装。
/// ラプラシアンに基づきコントラスト適応でシャープ化。
pub fn rcas(
    p: [f32; 3],
    n: [f32; 3],
    s: [f32; 3],
    e: [f32; 3],
    w: [f32; 3],
    sharpness: f32,
) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let lap = ((n[i] + s[i] + e[i] + w[i]) * 0.25) - p[i];
        // CAS は近傍平均から「遠ざける」方向がシャープ化 (加算だとブラーになる)。
        out[i] = (p[i] - lap * sharpness).clamp(0.0, 1.0);
    }
    out
}

pub struct Fsr1 {
    pub sharpness: f32,
}
impl Default for Fsr1 {
    fn default() -> Self {
        Self { sharpness: 0.2 }
    }
}
impl Fsr1 {
    pub fn reconstruct(
        &self,
        c: [f32; 3],
        nw: [f32; 3],
        ne: [f32; 3],
        sw: [f32; 3],
        se: [f32; 3],
        fx: f32,
        fy: f32,
    ) -> [f32; 3] {
        easu_reconstruct(c, nw, ne, sw, se, fx, fy)
    }
    pub fn wgsl_source(&self) -> &'static str {
        FSR1_WGSL
    }
}

pub const FSR1_WGSL: &str = include_str!("../shaders/fsr1.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_region_returns_input() {
        let c = [0.5; 3];
        let same = [0.5; 3];
        let out = easu_reconstruct(c, same, same, same, same, 0.3, 0.7);
        for i in 0..3 {
            assert!((out[i] - 0.5).abs() < 1e-5, "channel {}", i);
        }
    }

    #[test]
    fn edge_is_preserved_sharp() {
        // 水平エッジ: 左半明、右半暗
        let nw = [1.0, 1.0, 1.0];
        let ne = [0.0, 0.0, 0.0];
        let sw = [1.0, 1.0, 1.0];
        let se = [0.0, 0.0, 0.0];
        // fx が中央付近なら、エッジ強度により 0.5 に近い値になる（過剰ブレンドせず）
        let out = easu_reconstruct([0.5; 3], nw, ne, sw, se, 0.5, 0.5);
        assert!(out[0] > 0.3 && out[0] < 0.7, "edge pixel should stay mid: {}", out[0]);
    }

    #[test]
    fn rcas_increases_contrast_on_edge() {
        // 明側の画素が暗い近傍からさらに明へ押し出されるか (CAS の伸長方向)。
        // 明暗対称の近傍だとラプラシアンが 0 に相殺され何も起きないので、
        // 方向が一意に決まる非対称配置で検証する。
        let p = [0.8; 3];
        let dark = [0.2; 3];
        let out = rcas(p, dark, dark, dark, dark, 0.5);
        assert!(out[0] > 0.8, "rcas should push toward bright side: {}", out[0]);
    }

    #[test]
    fn rcas_flat_is_identity() {
        let p = [0.4; 3];
        let out = rcas(p, p, p, p, p, 0.5);
        for i in 0..3 {
            assert!((out[i] - 0.4).abs() < 1e-6);
        }
    }
}
