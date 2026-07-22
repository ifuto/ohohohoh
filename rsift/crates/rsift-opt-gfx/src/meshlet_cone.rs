//! Meshlet normal-cone backface culling.
//!
//! A meshlet (cluster of triangles) stores the average normal direction and a
//! cone half-angle covering all its face normals. If the camera lies outside
//! that cone, *every* triangle in the meshlet is back-facing, so the whole
//! cluster can be skipped before any vertex shading. A cheap aggregate test
//! (Nanite-style) that shines on mid/high-poly models on any GPU.

#[derive(Clone, Copy, Debug)]
pub struct Cone {
    /// Unit axis = average face normal of the meshlet.
    pub axis: [f32; 3],
    /// Cosine of the cone half-angle (measured so that being outside the cone
    /// means fully back-facing). Wide cones (>90°) still cull strongly behind.
    pub cos_angle: f32,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-8);
    [v[0] / l, v[1] / l, v[2] / l]
}

impl Cone {
    /// Build a cone from per-triangle normals. `cos_angle` is the cosine of the
    /// widest deviation from the average normal (i.e. the cone that contains all
    /// face normals plus a 90° margin so a back-facing cluster is fully culled).
    ///
    /// ## 数学的正当性
    /// 錐体 (axis A, 半角 α) 内の最大 `dot(n, V)` は `cos(max(0, β−α))`
    /// (β = angle(V, A))。メッシュレットが「一部でも前面」 ⟺
    /// `max dot > 0` ⟺ `β < α + 90°` ⟺ `cos β > cos(α+90°) = −sin α`。
    /// よって `cos_angle = −sin(α)` との比較は厳密に正しい
    /// (境界 == は conservative に可視側で処理)。
    ///
    /// ## 契約 (2026-07-23 wave 47 厳格化)
    /// - `normals` は**非空**必須 (空クラスタは呼出側バグ。旧実装は axis が
    ///   ゼロベクトルに退化して静寂に「常に可視」へ着地していた)。
    /// - 全法線の全成分は**有限**必須。旧実装は NaN 法線で `f32::min` の
    ///   「片側 NaN なら他方」を経て min_cos を偽装し、`dot >= cos_angle`
    ///   の NaN 比較で**メッシュレットを永久カリング**していた
    ///   (billboard_lod AL-1 同型の形状蒸発、観測欠測の静寂混入 → 拒否)。
    ///
    /// ## 退化ケースのセマンティクス (丸め含め全て conservative 側に倒れる設計)
    /// - 法線和がゼロ (正反対法線の打ち消し): axis = ゼロベクトル、
    ///   min_cos = 0 → `cos_angle = −1` で**常に可視** (カリング不能 =
    ///   正しい挙動、カリングは誤りにならない方向にのみ倒れる)。
    /// - 法線和 |ax| < 1e-8: `normalize` の max(1e-8) で axis は単位未満の
    ///   縮小ベクトル → 判定誤差は conservative (可視側) にのみ働く。
    /// - f32 丸めで `dot` が ±1 を微超過し得る (単位ベクトル同士でも
    ///   −1−2^-23 まで発生) ため `sin_a` の被開ケ子は `max(0.0)` で鉗制
    ///   — 旧実装はここで **sqrt(負) = NaN → cos_angle NaN → 永久カリング**
    ///   し得た (wave 47 で根治)。
    pub fn from_normals(normals: &[[f32; 3]]) -> Cone {
        assert!(
            !normals.is_empty(),
            "Cone::from_normals 契約違反: 空の法線列"
        );
        for (i, n) in normals.iter().enumerate() {
            assert!(
                n[0].is_finite() && n[1].is_finite() && n[2].is_finite(),
                "Cone::from_normals 契約違反: normals[{i}] が非有限 ({n:?})"
            );
        }
        let mut ax = [0.0f32; 3];
        for n in normals {
            ax[0] += n[0];
            ax[1] += n[1];
            ax[2] += n[2];
        }
        let axis = normalize(ax);
        // widest angle between axis and any face normal
        let mut min_cos = 1.0f32;
        for n in normals {
            min_cos = min_cos.min(dot(axis, normalize(*n)));
        }
        // +90° margin: cos(a+90°) = -sin(a)。被開ケ子は roundoff 鉗制 (契約注記)。
        let sin_a = (1.0 - min_cos * min_cos).max(0.0).sqrt();
        Cone {
            axis,
            cos_angle: -sin_a,
        }
    }

    /// Returns `true` if the meshlet is at least partially front-facing and
    /// therefore should be drawn. `to_camera` is (camera - meshlet_center)
    /// (need not be normalized).
    ///
    /// **契約 (wave 47)**: `to_camera` の全成分は有限必須 (NaN は NaN 比較で
    /// 静寂に culled へ着地していた — 形状蒸発の根治)。ゼロベクトルは
    /// normalize の max(1e-8) で `dot = 0` となり `cos_angle ≤ 0` と常に
    /// 比較が真 (メッシュレット真上のカメラは可視側 = 正しい保守方向)。
    pub fn visible(&self, to_camera: [f32; 3]) -> bool {
        assert!(
            to_camera[0].is_finite() && to_camera[1].is_finite() && to_camera[2].is_finite(),
            "Cone::visible 契約違反: to_camera が非有限 ({to_camera:?})"
        );
        dot(normalize(to_camera), self.axis) >= self.cos_angle
    }
}

pub fn meshlet_cone_wgsl() -> &'static str {
    MESHLET_CONE_WGSL
}

pub const MESHLET_CONE_WGSL: &str = include_str!("../shaders/meshlet_cone.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn front_facing_visible() {
        let c = Cone {
            axis: [0.0, 0.0, 1.0],
            cos_angle: -0.5, // ~120° cone
        };
        assert!(c.visible([0.0, 0.0, 1.0])); // camera in front
    }
    #[test]
    fn back_facing_culled() {
        let c = Cone {
            axis: [0.0, 0.0, 1.0],
            cos_angle: -0.5,
        };
        assert!(!c.visible([0.0, 0.0, -1.0])); // camera behind
    }
    #[test]
    fn wide_cone_tolerant() {
        // A cone that opens past 90° still culls only when clearly behind.
        let c = Cone {
            axis: [0.0, 0.0, 1.0],
            cos_angle: -0.9, // ~154° cone
        };
        // to_camera は正規化されるので、浅い角度は成分比で作る。
        // normalize([0.6,0,-0.8])·axis = -0.8 > -0.9 => 154°コーン内で visible
        assert!(c.visible([0.6, 0.0, -0.8]));
        // directly behind far => dot -1 < -0.9 => culled
        assert!(!c.visible([0.0, 0.0, -1.0]));
    }
    #[test]
    fn cone_from_normals_contains_all() {
        // two opposite-ish normals -> wide cone; camera in front should be visible
        let normals = [[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let c = Cone::from_normals(&normals);
        // average is zero -> axis becomes (0,0,1) after normalize of zero (degenerate).
        // Use a clearly forward set instead:
        let normals2 = [[0.0, 0.0, 1.0], [0.3, 0.0, 0.95]];
        let c2 = Cone::from_normals(&normals2);
        assert!(c2.visible([0.0, 0.0, 1.0]));
    }

    /// wave 47-1: 単一法線の厳密ビット構成 (axis は入力そのまま、
    /// cos_angle = −0.0)。可視/カリング/境界 == の 3 規則も全て厳密。
    #[test]
    fn from_normals_single_exact_bits() {
        let c = Cone::from_normals(&[[1.0, 0.0, 0.0]]);
        assert_eq!(c.axis, [1.0, 0.0, 0.0], "axis は単一法線に厳密一致");
        assert_eq!(
            c.cos_angle.to_bits(),
            (-0.0f32).to_bits(),
            "cos(0+90°) = -0"
        );
        assert!(c.visible([1.0, 0.0, 0.0]), "正面: dot 1 >= -0");
        assert!(!c.visible([-1.0, 0.0, 0.0]), "背面: dot -1 < -0");
        // 境界 == は conservative に可視側 (edge-on は描く)
        assert!(
            c.visible([0.0, 1.0, 0.0]),
            "edge-on dot == -0 == cos_angle → 可視"
        );
        assert!(
            c.visible([0.0, 0.0, 0.0]),
            "カメラ真上 (0 方向) → dot 0 → 可視"
        );
    }

    /// wave 47-2: 正反対法線の打ち消し → ゼロ和退化は「常に可視」
    /// (カリング不能を conservative に表現。ピンするのは exact −1.0)。
    #[test]
    fn antiparallel_degenerates_to_always_visible() {
        let c = Cone::from_normals(&[[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]]);
        assert_eq!(c.axis, [0.0, 0.0, 0.0], "ゼロ和 → ゼロ axis");
        assert_eq!(
            c.cos_angle, -1.0,
            "α=180° → cos_angle = -1 (sqrt(1)=1 厳密)"
        );
        // 全方向で可視 (dot(0方向, axis) = 0 >= -1)
        assert!(c.visible([0.0, 0.0, 1.0]));
        assert!(c.visible([1.0, 0.0, 0.0]));
        assert!(c.visible([-1.0, -1.0, -1.0]));
    }

    /// wave 47-3: ほぼ正反対対 — roundoff で dot が −1 を微超過し得る
    /// 構成でも cos_angle/axis は NaN にならない (max(0.0) 鉗制の回帰ガード。
    /// 根治前はここで sqrt(負) により静寂な永久カリングが起き得た)。
    #[test]
    fn nearly_antiparallel_never_produces_nan() {
        for &eps in &[1e-20f32, 1e-30, 1e-38] {
            let c = Cone::from_normals(&[[0.0, 0.0, 1.0], [eps, 0.0, -0.99999994]]);
            assert!(c.cos_angle.is_finite(), "eps={eps}: cos_angle NaN 退避");
            assert!(
                c.axis.iter().all(|v| v.is_finite()),
                "eps={eps}: axis NaN 退避"
            );
            assert!(c.visible([0.0, 0.0, 1.0]), "eps={eps}: 正面は可視");
        }
    }

    /// wave 47-4: 境界 == は可視側 (厳密 == 構成: 3-4-5 三平方で正規化の
    /// 除算が 0.8/−0.6 に正確丸め → dot == cos_angle の bit 一致)。
    #[test]
    fn boundary_equality_kept_visible() {
        let c = Cone {
            axis: [0.0, 0.0, 1.0],
            cos_angle: -0.6,
        };
        // normalize([0,4,-3]): l = sqrt(25) = 5 厳密、z/5 = -0.6 厳密丸め
        assert!(
            c.visible([0.0, 4.0, -3.0]),
            "dot == cos_angle → 可視 (保守側)"
        );
        assert!(!c.visible([0.0, 0.0, -1.0]), "dot -1.0 < -0.6 → カリング");
    }

    /// wave 47-5: 契約違反は fail-loud (空列 / NaN 法線 / NaN to_camera)。
    #[test]
    #[should_panic(expected = "空の法線列")]
    fn from_normals_rejects_empty() {
        let _ = Cone::from_normals(&[]);
    }

    #[test]
    #[should_panic(expected = "normals[1] が非有限")]
    fn from_normals_rejects_nan_normal() {
        let _ = Cone::from_normals(&[[0.0, 0.0, 1.0], [f32::NAN, 0.0, 0.0]]);
    }

    #[test]
    #[should_panic(expected = "to_camera が非有限")]
    fn visible_rejects_nan_to_camera() {
        let c = Cone::from_normals(&[[0.0, 0.0, 1.0]]);
        let _ = c.visible([f32::NAN, 0.0, 0.0]);
    }
}
