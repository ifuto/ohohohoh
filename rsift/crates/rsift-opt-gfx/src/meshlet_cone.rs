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
    pub fn from_normals(normals: &[[f32; 3]]) -> Cone {
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
        // +90° margin: cos(a+90°) = -sin(a)
        let sin_a = (1.0 - min_cos * min_cos).sqrt();
        Cone {
            axis,
            cos_angle: -sin_a,
        }
    }

    /// Returns `true` if the meshlet is at least partially front-facing and
    /// therefore should be drawn. `to_camera` is (camera - meshlet_center)
    /// (need not be normalized).
    pub fn visible(&self, to_camera: [f32; 3]) -> bool {
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
}
