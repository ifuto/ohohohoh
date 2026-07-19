//! Distance-banded entity tick scheduling (Tier 5).

use crate::spatial_hash::SpatialHashGrid3D;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickBand {
    EveryTick,
    Every2,
    Every4,
    Every8,
    RenderOnly,
}

#[derive(Debug)]
pub struct EntityTickScheduler {
    pub near_dist: f32,
    pub mid_dist: f32,
    pub far_dist: f32,
    pub cull_dist: f32,
    tick: u64,
}

impl Default for EntityTickScheduler {
    fn default() -> Self {
        Self {
            near_dist: 32.0,
            mid_dist: 64.0,
            far_dist: 128.0,
            cull_dist: 256.0,
            tick: 0,
        }
    }
}

impl EntityTickScheduler {
    pub fn advance(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn band_for_distance(&self, dist: f32) -> TickBand {
        if dist < self.near_dist {
            TickBand::EveryTick
        } else if dist < self.mid_dist {
            TickBand::Every2
        } else if dist < self.far_dist {
            TickBand::Every4
        } else if dist < self.cull_dist {
            TickBand::Every8
        } else {
            TickBand::RenderOnly
        }
    }

    pub fn should_tick(&self, band: TickBand) -> bool {
        match band {
            TickBand::EveryTick => true,
            TickBand::Every2 => self.tick % 2 == 0,
            TickBand::Every4 => self.tick % 4 == 0,
            TickBand::Every8 => self.tick % 8 == 0,
            TickBand::RenderOnly => false,
        }
    }

    /// Returns entity ids that should AI-tick this frame.
    pub fn select_tickable(
        &self,
        positions: &[(u32, f32, f32, f32)],
        player: [f32; 3],
    ) -> Vec<u32> {
        let mut out = Vec::new();
        for &(id, x, y, z) in positions {
            let dx = x - player[0];
            let dy = y - player[1];
            let dz = z - player[2];
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            let band = self.band_for_distance(dist);
            if self.should_tick(band) {
                out.push(id);
            }
        }
        out
    }

    pub fn select_tickable_hashed(
        &self,
        hash: &SpatialHashGrid3D,
        positions: &[(u32, f32, f32, f32)],
        player: [f32; 3],
    ) -> Vec<u32> {
        let candidates = hash.query_radius(player[0], player[1], player[2], self.cull_dist);
        let mut pos_map: std::collections::HashMap<u32, [f32; 3]> = positions
            .iter()
            .map(|&(id, x, y, z)| (id, [x, y, z]))
            .collect();
        let mut out = Vec::new();
        for id in candidates {
            if let Some(p) = pos_map.remove(&id) {
                let dx = p[0] - player[0];
                let dy = p[1] - player[1];
                let dz = p[2] - player[2];
                let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                if self.should_tick(self.band_for_distance(dist)) {
                    out.push(id);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_entity_skips() {
        let mut s = EntityTickScheduler::default();
        s.tick = 1; // odd → Every2 skips
        let ids = s.select_tickable(&[(1, 50.0, 0.0, 0.0)], [0.0, 0.0, 0.0]);
        assert!(ids.is_empty());
        s.tick = 2;
        let ids = s.select_tickable(&[(1, 50.0, 0.0, 0.0)], [0.0, 0.0, 0.0]);
        assert_eq!(ids, vec![1]);
    }
}
