//! Clustered (forward+) light assignment.
//!
//! Subdivides the view frustum into a 3D grid of clusters and, for each one,
//! lists the lights whose sphere intersects it. The forward shader then only
//! loops over the handful of lights in its pixel's cluster instead of all of
//! them — a big win with many dynamic lights on mid-spec GPUs.

#[derive(Clone, Copy, Debug)]
pub struct Light {
    pub position: [f32; 3],
    pub radius: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct ClusterGrid {
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub slices: u32,
}

impl ClusterGrid {
    pub fn new(tiles_x: u32, tiles_y: u32, slices: u32) -> Self {
        Self {
            tiles_x,
            tiles_y,
            slices,
        }
    }

    /// Linear index of the cluster at `(cx, cy, cz)`.
    pub fn index(&self, cx: u32, cy: u32, cz: u32) -> usize {
        (cz * self.tiles_y * self.tiles_x + cy * self.tiles_x + cx) as usize
    }

    /// World-space AABB (in a unit cube [0,1]^3) of cluster `(cx,cy,cz)`.
    pub fn aabb(&self, cx: u32, cy: u32, cz: u32) -> ([f32; 3], [f32; 3]) {
        let sx = 1.0 / self.tiles_x as f32;
        let sy = 1.0 / self.tiles_y as f32;
        let sz = 1.0 / self.slices as f32;
        let min = [cx as f32 * sx, cy as f32 * sy, cz as f32 * sz];
        let max = [(cx + 1) as f32 * sx, (cy + 1) as f32 * sy, (cz + 1) as f32 * sz];
        (min, max)
    }

    /// Assign each light to every cluster whose AABB it intersects.
    /// Returns `lights_per_cluster[index]` = list of light indices.
    pub fn assign_lights(&self, lights: &[Light]) -> Vec<Vec<u32>> {
        let n = (self.tiles_x * self.tiles_y * self.slices) as usize;
        let mut out: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (li, light) in lights.iter().enumerate() {
            for cz in 0..self.slices {
                for cy in 0..self.tiles_y {
                    for cx in 0..self.tiles_x {
                        let (mn, mx) = self.aabb(cx, cy, cz);
                        if sphere_intersects_aabb(light.position, light.radius, mn, mx) {
                            out[self.index(cx, cy, cz)].push(li as u32);
                        }
                    }
                }
            }
        }
        out
    }

    pub fn wgsl_source(&self) -> &'static str {
        CLUSTERED_LIGHTING_WGSL
    }
}

fn sphere_intersects_aabb(c: [f32; 3], r: f32, mn: [f32; 3], mx: [f32; 3]) -> bool {
    let mut d = 0.0f32;
    for i in 0..3 {
        let v = c[i].clamp(mn[i], mx[i]);
        let diff = c[i] - v;
        d += diff * diff;
    }
    d <= r * r
}

pub const CLUSTERED_LIGHTING_WGSL: &str = include_str!("../shaders/clustered_lighting.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_is_unique_and_in_range() {
        let g = ClusterGrid::new(4, 4, 4);
        let mut seen = std::collections::HashSet::new();
        for cz in 0..4u32 {
            for cy in 0..4u32 {
                for cx in 0..4u32 {
                    let i = g.index(cx, cy, cz);
                    assert!(i < 64);
                    assert!(seen.insert(i));
                }
            }
        }
        assert_eq!(seen.len(), 64);
    }
    #[test]
    fn light_assigned_to_overlapping_clusters_only() {
        let g = ClusterGrid::new(4, 4, 4);
        // unit cube split into 4 per axis => each cluster is 0.25 wide.
        // A light at center (0.5,0.5,0.5) radius 0.1 hits only the middle cluster.
        let lights = vec![Light {
            position: [0.5, 0.5, 0.5],
            radius: 0.1,
        }];
        let assigned = g.assign_lights(&lights);
        let middle = g.index(2, 2, 2); // 0.5..0.75
        assert!(assigned[middle].contains(&0));
        // a far cluster must NOT contain it
        let far = g.index(0, 0, 0);
        assert!(!assigned[far].contains(&0));
    }
    #[test]
    fn big_light_fills_many_clusters() {
        let g = ClusterGrid::new(4, 4, 4);
        let lights = vec![Light {
            position: [0.5, 0.5, 0.5],
            radius: 10.0,
        }];
        let assigned = g.assign_lights(&lights);
        let count: usize = assigned.iter().map(|v| v.len()).sum();
        assert_eq!(count, 64); // covers everything
    }
    #[test]
    fn sphere_aabb_edge_cases() {
        assert!(sphere_intersects_aabb([0.0, 0.0, 0.0], 1.0, [0.5, 0.5, 0.5], [1.0, 1.0, 1.0]));
        assert!(!sphere_intersects_aabb([0.0, 0.0, 0.0], 0.1, [0.5, 0.5, 0.5], [1.0, 1.0, 1.0]));
    }
}
