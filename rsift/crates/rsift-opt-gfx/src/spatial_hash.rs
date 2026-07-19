//! 3D spatial hash for entity queries (Tier 5).

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SpatialHashGrid3D {
    cell: f32,
    cells: HashMap<(i32, i32, i32), Vec<u32>>,
    /// entity_id → cell key for O(1) remove/move
    locations: HashMap<u32, (i32, i32, i32)>,
}

impl SpatialHashGrid3D {
    pub fn new(cell_size: f32) -> Self {
        Self {
            cell: cell_size.max(0.5),
            cells: HashMap::new(),
            locations: HashMap::new(),
        }
    }

    #[inline]
    fn key(&self, x: f32, y: f32, z: f32) -> (i32, i32, i32) {
        (
            (x / self.cell).floor() as i32,
            (y / self.cell).floor() as i32,
            (z / self.cell).floor() as i32,
        )
    }

    pub fn insert(&mut self, id: u32, x: f32, y: f32, z: f32) {
        self.remove(id);
        let k = self.key(x, y, z);
        self.cells.entry(k).or_default().push(id);
        self.locations.insert(id, k);
    }

    pub fn remove(&mut self, id: u32) {
        if let Some(k) = self.locations.remove(&id) {
            if let Some(list) = self.cells.get_mut(&k) {
                list.retain(|&e| e != id);
                if list.is_empty() {
                    self.cells.remove(&k);
                }
            }
        }
    }

    pub fn move_entity(&mut self, id: u32, x: f32, y: f32, z: f32) {
        let nk = self.key(x, y, z);
        if self.locations.get(&id) == Some(&nk) {
            return;
        }
        self.insert(id, x, y, z);
    }

    pub fn query_aabb(&self, min: [f32; 3], max: [f32; 3]) -> Vec<u32> {
        let k0 = self.key(min[0], min[1], min[2]);
        let k1 = self.key(max[0], max[1], max[2]);
        let mut out = Vec::new();
        for x in k0.0..=k1.0 {
            for y in k0.1..=k1.1 {
                for z in k0.2..=k1.2 {
                    if let Some(list) = self.cells.get(&(x, y, z)) {
                        out.extend(list.iter().copied());
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn query_radius(&self, cx: f32, cy: f32, cz: f32, radius: f32) -> Vec<u32> {
        let r = radius.max(0.0);
        self.query_aabb(
            [cx - r, cy - r, cz - r],
            [cx + r, cy + r, cz + r],
        )
    }

    pub fn len(&self) -> usize {
        self.locations.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_query() {
        let mut g = SpatialHashGrid3D::new(8.0);
        g.insert(1, 0.0, 0.0, 0.0);
        g.insert(2, 100.0, 0.0, 0.0);
        let near = g.query_radius(0.0, 0.0, 0.0, 16.0);
        assert!(near.contains(&1));
        assert!(!near.contains(&2));
    }
}
