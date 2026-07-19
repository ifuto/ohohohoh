//! Billboard LOD for distant flora/trees (Tier 4).

#[derive(Debug, Clone, Copy)]
pub struct BillboardVert {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloraLod {
    FullMesh,
    CrossedPlanes,
    Billboard,
    Culled,
}

#[derive(Debug, Clone)]
pub struct BillboardLodSelector {
    pub full_mesh_dist: f32,
    pub crossed_dist: f32,
    pub billboard_dist: f32,
}

impl Default for BillboardLodSelector {
    fn default() -> Self {
        Self {
            full_mesh_dist: 48.0,
            crossed_dist: 96.0,
            billboard_dist: 192.0,
        }
    }
}

impl BillboardLodSelector {
    pub fn for_tier_scale(scale: f32) -> Self {
        let s = scale.clamp(0.5, 1.5);
        Self {
            full_mesh_dist: 48.0 * s,
            crossed_dist: 96.0 * s,
            billboard_dist: 192.0 * s,
        }
    }

    pub fn select(&self, dist: f32) -> FloraLod {
        if dist < self.full_mesh_dist {
            FloraLod::FullMesh
        } else if dist < self.crossed_dist {
            FloraLod::CrossedPlanes
        } else if dist < self.billboard_dist {
            FloraLod::Billboard
        } else {
            FloraLod::Culled
        }
    }

    /// Camera-facing quad centered at `center` with half-extents `half_w` / `half_h`.
    pub fn make_billboard(
        center: [f32; 3],
        half_w: f32,
        half_h: f32,
        cam_right: [f32; 3],
        cam_up: [f32; 3],
    ) -> [BillboardVert; 4] {
        let r = cam_right;
        let u = cam_up;
        let corners = [
            [-half_w, -half_h],
            [half_w, -half_h],
            [half_w, half_h],
            [-half_w, half_h],
        ];
        let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let mut out = [BillboardVert {
            pos: [0.0; 3],
            uv: [0.0; 2],
        }; 4];
        for i in 0..4 {
            let px = center[0] + r[0] * corners[i][0] + u[0] * corners[i][1];
            let py = center[1] + r[1] * corners[i][0] + u[1] * corners[i][1];
            let pz = center[2] + r[2] * corners[i][0] + u[2] * corners[i][1];
            out[i] = BillboardVert {
                pos: [px, py, pz],
                uv: uvs[i],
            };
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_bands() {
        let s = BillboardLodSelector::default();
        assert_eq!(s.select(10.0), FloraLod::FullMesh);
        assert_eq!(s.select(200.0), FloraLod::Culled);
        let v = BillboardLodSelector::make_billboard(
            [0.0, 1.0, 0.0],
            0.5,
            1.0,
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        );
        assert!((v[0].pos[0] + 0.5).abs() < 1e-5);
    }
}
