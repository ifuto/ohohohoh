//! DDGI (Dynamic Diffuse Global Illumination) probe volume sampling for
//! `rsift-opt-gfx`.
//!
//! Real logic (no stubs): octahedral encode/decode of probe directions,
//! trilinear probe coordinate lookup, and Chebyshev (VSM-style) visibility
//! used to reduce light leaking. This adds *bounce* lighting that screen-space
//! techniques miss — a quality improvement, not a resolution change.
//!
//! 【wave 160 FF (2026-07-28)】消費者: oct 対は `frame_ddgi` の WGSL ミラー
//! (oct_encode_wgsl/oct_decode_wgsl) が委譲する唯一の CPU 参照 (式ツリー同一
//! = corpus bit 同一を strict pin)、`chebyshev_visibility` は
//! `frame_ddgi::sample_visibility`、`DdgiVolume`/`probe_coord`/`probe_count` は
//! `full_graph_wiring` の report 実フィールド (ddgi_probe_coord/count)、
//! `wgsl_source` は `gpu_runtime` 登録。捕捉 86 [中]: 旧 oct 対は下半球の
//! wrap が成分 swap を欠く非標準 fold で WGSL `shaders/ddgi.wgsl` と対角鏡像
//! に乖離 (自己整合ペアのため旧 roundtrip 試験は潜伏)。捕捉 88 [小]:
//! probe_count の u32 積 wrap (rq ff_ddgi (3)) を usize 昇格で根治。
//! 捕捉 87: 消費者ゼロを機械 grep 証明した `Vec4` (型+演算 3 実装、crate
//! 全域 0 使用) と `Vec3::{normalize, Add, Mul<f32>}` (修正後 0 使用) は
//! async_compute EJ-2 判例に倣い不可能証明削除 (新指令 §7)。

use std::ops::Sub;

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

/// Octahedral encode of a unit direction -> [-1,1]^2.
pub fn oct_encode_unit(v: Vec3) -> (f32, f32) {
    // WGSL `shaders/ddgi.wgsl octEncode` と同一式ツリー (捕捉 86 根治):
    // pre-normalize は持たない (射影 s が既に正規化を兼ねる)。下半球の
    // fold は標準 diamond swap ((1-|oy|)*sgn(ox), (1-|ox|)*sgn(oy))。
    let s = (v.x.abs() + v.y.abs() + v.z.abs()).max(1e-8);
    let mut ox = v.x / s;
    let mut oy = v.y / s;
    if v.z < 0.0 {
        let sx = if ox >= 0.0 { 1.0 } else { -1.0 };
        let sy = if oy >= 0.0 { 1.0 } else { -1.0 };
        // タプルで旧 (ox,oy) から同時評価 (逐語代入で ox 更新を
        // oy 項へ漏らさない = WGSL vec2 構築と同じ評価語彙)。
        let e = ((1.0 - oy.abs()) * sx, (1.0 - ox.abs()) * sy);
        ox = e.0;
        oy = e.1;
    }
    (ox, oy)
}

/// Octahedral decode of [-1,1]^2 -> unit direction.
pub fn oct_decode_unit(f: (f32, f32)) -> Vec3 {
    // WGSL `octDecode` 語彙: 符号付き fold 復元後に無条件 normalize。
    // Sigma|n_i| = a+|1-a| >= 1 (rq ff_ddgi (2)) より分母 0 は到達不能
    // (旧 1e-8 ガードは数学的に死んでいた。a=|fx|+|fy|)。
    let ox = f.0;
    let oy = f.1;
    let mut n = Vec3::new(ox, oy, 1.0 - ox.abs() - oy.abs());
    if n.z < 0.0 {
        let sx = if ox >= 0.0 { 1.0 } else { -1.0 };
        let sy = if oy >= 0.0 { 1.0 } else { -1.0 };
        let tx = (1.0 - oy.abs()) * sx;
        let ty = (1.0 - ox.abs()) * sy;
        n.x = tx;
        n.y = ty;
    }
    let l = n.length();
    Vec3::new(n.x / l, n.y / l, n.z / l)
}

/// Chebyshev / Variance Shadow Map visibility in [0,1].
/// `m1` = mean depth, `m2` = mean depth^2, `d` = receiver depth.
pub fn chebyshev_visibility(m1: f32, m2: f32, d: f32) -> f32 {
    if d <= m1 {
        return 1.0;
    }
    let variance = (m2 - m1 * m1).max(0.0);
    let diff = d - m1;
    let p = variance / (variance + diff * diff);
    p.clamp(0.0, 1.0)
}

/// A DDGI probe grid.
pub struct DdgiVolume {
    pub origin: Vec3,
    pub cell_size: Vec3,
    pub dims: (u32, u32, u32),
}

impl DdgiVolume {
    pub fn new(origin: Vec3, cell_size: Vec3, dims: (u32, u32, u32)) -> Self {
        Self { origin, cell_size, dims }
    }

    /// Fractional probe coordinate of a world position.
    pub fn probe_coord(&self, p: Vec3) -> Vec3 {
        let inv = Vec3::new(
            1.0 / self.cell_size.x.max(1e-3),
            1.0 / self.cell_size.y.max(1e-3),
            1.0 / self.cell_size.z.max(1e-3),
        );
        let d = p - self.origin;
        Vec3::new(d.x * inv.x, d.y * inv.y, d.z * inv.z)
    }

    /// Number of probes in the grid.
    /// 【wave 160 FF 捕捉 88】u32 積は 1626^3≈4.3e9 超で暗黙 wrap (debug
    /// ビルドでは overflow panic; rq ff_ddgi (3): 3000^3=27e9 → wrap
    /// 1,230,196,224)。usize 積へ昇格して真値を返す。
    pub fn probe_count(&self) -> usize {
        self.dims.0 as usize * self.dims.1 as usize * self.dims.2 as usize
    }
}

pub fn wgsl_source() -> &'static str {
    DDGI_WGSL
}

pub const DDGI_WGSL: &str = include_str!("../shaders/ddgi.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oct_roundtrip() {
        let dirs = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.577, 0.577, 0.577),
            Vec3::new(-0.3, 0.8, -0.5),
        ];
        for d in dirs {
            let e = oct_encode_unit(d);
            let r = oct_decode_unit(e);
            let l = d.length();
            let dot = r.dot(Vec3::new(d.x / l, d.y / l, d.z / l));
            assert!(dot > 0.999, "oct roundtrip dot = {}", dot);
        }
    }
    #[test]
    fn chebyshev_fully_lit() {
        // Receiver depth smaller than mean -> fully visible.
        assert!((chebyshev_visibility(5.0, 26.0, 3.0) - 1.0).abs() < 1e-6);
    }
    #[test]
    fn chebyshev_attenuates_distinct_depth() {
        let v = chebyshev_visibility(5.0, 26.0, 12.0);
        assert!(v < 1.0 && v > 0.0, "visibility = {}", v);
    }
    #[test]
    fn probe_coord_scales() {
        let v = DdgiVolume::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 4.0, 4.0), (8, 4, 4));
        let c = v.probe_coord(Vec3::new(8.0, 0.0, 0.0));
        assert!((c.x - 2.0).abs() < 1e-6);
        assert_eq!(v.probe_count(), 128);
    }

    // ==================== wave 160 (FF) strict ====================

    /// 捕捉 86 [中]: oct encode の下半球 wrap は WGSL `shaders/ddgi.wgsl`
    /// octEncode の標準 diamond swap fold ((1-|oy|)*sgn(ox), (1-|ox|)*sgn(oy))
    /// と bit 一致しなければならない (旧実装は成分 swap を欠く非標準 fold で
    /// 下半球が対角鏡像に乖離)。golden は実機 probe (rustc -O) 確定値。
    #[test]
    fn ff_oct_encode_lower_hemisphere_wgsl_swap_golden() {
        // n=(1,2,-2) raw: 射影 (0.2,0.4) → swap fold で (0.6,0.8)
        let e = oct_encode_unit(Vec3::new(1.0, 2.0, -2.0));
        assert_eq!(e.0.to_bits(), 0x3f19999a, "encode x == 0.6 (WGSL truth)");
        assert_eq!(e.1.to_bits(), 0x3f4ccccd, "encode y == 0.8 (WGSL truth)");
        // unit dir (-0.3,0.8,-0.5): (-0.5, 0.8125)
        let e2 = oct_encode_unit(Vec3::new(-0.3, 0.8, -0.5));
        assert_eq!(e2.0.to_bits(), 0xbf000000, "encode x == -0.5");
        assert_eq!(e2.1.to_bits(), 0x3f500000, "encode y == 0.8125");
    }

    /// 捕捉 86: decode 側も同一 swap fold で (0.6,0.8) → ~(1/3,2/3,-2/3)
    /// (rq ff_ddgi (1) の実数厳密 roundtrip を fp 実機 golden で pin)。
    #[test]
    fn ff_oct_decode_wgsl_golden() {
        let d = oct_decode_unit((0.6, 0.8));
        assert_eq!(d.x.to_bits(), 0x3eaaaaaa);
        assert_eq!(d.y.to_bits(), 0x3f2aaaaa);
        assert_eq!(d.z.to_bits(), 0xbf2aaaab);
    }

    /// 捕捉 87 §7: frame_ddgi の WGSL ミラーは ddgi 本家へ委譲され、
    /// 式ツリー同一のため corpus 全域で **bit 同一** でなければならない
    /// (実機 probe: enc 8/8・dec 14/14 bit 一致)。
    #[test]
    fn ff_oct_pair_bit_identical_to_frame_ddgi_mirror() {
        let corpus: [[f32; 3]; 8] = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.577, 0.577, 0.577],
            [-0.3, 0.8, -0.5],
            [1.0, 2.0, -2.0],
            [3.0, 4.0, -12.0],
            [-1.0, -1.0, -1.0],
        ];
        for n in corpus {
            let a = oct_encode_unit(Vec3::new(n[0], n[1], n[2]));
            let b = crate::frame_ddgi::oct_encode_wgsl(n);
            assert_eq!(
                (a.0.to_bits(), a.1.to_bits()),
                (b[0].to_bits(), b[1].to_bits()),
                "encode bit divergence at {:?}",
                n
            );
            let da = oct_decode_unit(a);
            let db = crate::frame_ddgi::oct_decode_wgsl(b);
            assert_eq!(
                (da.x.to_bits(), da.y.to_bits(), da.z.to_bits()),
                (db[0].to_bits(), db[1].to_bits(), db[2].to_bits()),
                "decode bit divergence at {:?}",
                n
            );
        }
    }

    /// 捕捉 88 [小]: dims 積は u32 だと 1626^3≈4.3e9 超で暗黙 wrap
    /// (rq ff_ddgi (3): 3000^3=27e9 → wrap 1,230,196,224)。usize 積で真値。
    #[test]
    fn ff_probe_count_no_u32_wrap() {
        let v = DdgiVolume::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            (3000, 3000, 3000),
        );
        assert_eq!(v.probe_count(), 27_000_000_000usize);
    }

    /// probe_coord の厳密 binary pin (cell は全て 2 の冪 → 除算厳密、
    /// 実機 probe: 1.5=0x3fc00000 / 1.0=0x3f800000 / 3.0=0x40400000 /
    /// -2.5=0xc0200000 / -1.0=0xbf800000)。wiring 既定体積と同一構成。
    #[test]
    fn ff_probe_coord_exact_power2_cell() {
        let v = DdgiVolume::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(16.0, 8.0, 16.0),
            (16, 4, 16),
        );
        let c = v.probe_coord(Vec3::new(24.0, 8.0, 48.0));
        assert_eq!(c.x.to_bits(), 0x3fc00000);
        assert_eq!(c.y.to_bits(), 0x3f800000);
        assert_eq!(c.z.to_bits(), 0x40400000);
        let n = v.probe_coord(Vec3::new(-40.0, -8.0, -16.0));
        assert_eq!(n.x.to_bits(), 0xc0200000);
        assert_eq!(n.y.to_bits(), 0xbf800000);
        assert_eq!(n.z.to_bits(), 0xbf800000);
    }

    /// decode の normalize は数学的に安全側のみ働く: 任意の (fx,fy) で
    /// Sigma|n_i| = a + |1-a| >= 1 (rq ff_ddgi (2) grid 証明、実数最小値 1)。
    /// fp 厳密 binary 格子 (i/64) での実装語彙 sanity (fp 丸め込みで >= 0.999)。
    #[test]
    fn ff_decode_sigma_bound_grid() {
        let mut min_e = f32::INFINITY;
        for i in -64i32..=64 {
            for j in -64i32..=64 {
                let fx = i as f32 / 64.0;
                let fy = j as f32 / 64.0;
                let e = fx.abs() + fy.abs() + (1.0 - fx.abs() - fy.abs()).abs();
                min_e = min_e.min(e);
            }
        }
        assert!(min_e >= 0.999, "min Sigma|n_i| = {}", min_e);
    }

    /// 【wave 185 GE フェーズ2 回収】dead code 系 9 例目 (wave 160 FF adversarial (d)
    /// Vec4 死コード再追加 非検出、FF-2 で型+3 演算 impl 不可能証明削除済) の
    /// lexeme pin 化。同宣言形の将来復活を静寂に通さない。
    #[test]
    fn ge_removed_vec4_lexeme() {
        let src = include_str!("ddgi.rs");
        for lex in [concat!("struct ", "Vec4")] {
            assert!(
                !src.contains(lex),
                "dead code 系削除語彙の宣言形復活を検出 (wave 185 GE lexeme pin)"
            );
        }
    }
}
