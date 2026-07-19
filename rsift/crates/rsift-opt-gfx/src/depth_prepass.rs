//! Depth prepass + translucent / water dedicated passes (Tier 4).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PassId {
    DepthPrepass,
    OpaqueColor,
    Water,
    TranslucentBackToFront,
    Particles,
    Composite,
}

#[derive(Debug, Clone)]
pub struct PlannedPass {
    pub id: PassId,
    pub estimated_cost: u32,
    pub writes_color: bool,
    pub writes_depth: bool,
}

#[derive(Debug)]
pub struct DepthPrepassPlanner {
    pub enable_depth_prepass: bool,
    pub dedicated_water: bool,
    pub sort_translucent: bool,
}

impl DepthPrepassPlanner {
    pub fn new(enable_depth_prepass: bool, dedicated_water: bool) -> Self {
        Self {
            enable_depth_prepass,
            dedicated_water,
            sort_translucent: true,
        }
    }

    pub fn for_low_spec() -> Self {
        // Skip depth prepass on weak GPUs (extra geometry pass hurts more than it helps)
        Self::new(false, true)
    }

    pub fn for_high_spec() -> Self {
        Self::new(true, true)
    }

    pub fn plan(&self) -> Vec<PlannedPass> {
        let mut passes = Vec::new();
        if self.enable_depth_prepass {
            passes.push(PlannedPass {
                id: PassId::DepthPrepass,
                estimated_cost: 3,
                writes_color: false,
                writes_depth: true,
            });
            passes.push(PlannedPass {
                id: PassId::OpaqueColor,
                estimated_cost: 4,
                writes_color: true,
                writes_depth: false, // early-Z reject
            });
        } else {
            passes.push(PlannedPass {
                id: PassId::OpaqueColor,
                estimated_cost: 5,
                writes_color: true,
                writes_depth: true,
            });
        }
        if self.dedicated_water {
            passes.push(PlannedPass {
                id: PassId::Water,
                estimated_cost: 3,
                writes_color: true,
                writes_depth: true,
            });
        }
        passes.push(PlannedPass {
            id: PassId::TranslucentBackToFront,
            estimated_cost: if self.sort_translucent { 4 } else { 2 },
            writes_color: true,
            writes_depth: false,
        });
        passes.push(PlannedPass {
            id: PassId::Particles,
            estimated_cost: 2,
            writes_color: true,
            writes_depth: false,
        });
        passes.push(PlannedPass {
            id: PassId::Composite,
            estimated_cost: 1,
            writes_color: true,
            writes_depth: false,
        });
        passes
    }

    pub fn total_cost(&self) -> u32 {
        self.plan().iter().map(|p| p.estimated_cost).sum()
    }
}

/// Sort translucent quad centers back-to-front relative to camera.
pub fn sort_translucent_indices(
    centers: &[[f32; 3]],
    cam: [f32; 3],
    indices: &mut [u32],
) {
    indices.sort_by(|&a, &b| {
        let da = dist2(centers[a as usize], cam);
        let db = dist2(centers[b as usize], cam);
        db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_skips_prepass() {
        let p = DepthPrepassPlanner::for_low_spec().plan();
        assert!(!p.iter().any(|x| x.id == PassId::DepthPrepass));
    }

    #[test]
    fn sort_back_to_front() {
        let centers = [[0.0, 0.0, 0.0], [0.0, 0.0, 10.0], [0.0, 0.0, 5.0]];
        let mut idx = [0u32, 1, 2];
        sort_translucent_indices(&centers, [0.0, 0.0, -1.0], &mut idx);
        assert_eq!(idx[0], 1); // farthest first
    }
}
