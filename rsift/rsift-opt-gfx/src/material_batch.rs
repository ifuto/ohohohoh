//! Material batching + instanced flora + opaque/transparent split (Tier 1).
//! Sodium-style: group by material to kill texture binds; flora as one instanced draw.

use std::collections::BTreeMap;

/// One drawable quad/instance after material grouping.
#[derive(Debug, Clone, Copy)]
pub struct BatchedDraw {
    pub material_id: u16,
    pub first_quad: u32,
    pub quad_count: u32,
    pub translucent: bool,
}

/// Packed flora instance: xyz (16.16 fixed) + scale8 + yaw8.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FloraInstance {
    pub packed_xz: u32, // x:u16 z:u16 in block*256
    pub packed_y_scale_yaw: u32, // y:u16 scale:u8 yaw:u8
}

impl FloraInstance {
    pub fn new(x: f32, y: f32, z: f32, scale: f32, yaw_rad: f32) -> Self {
        let px = (x * 256.0).clamp(0.0, 65535.0) as u16;
        let pz = (z * 256.0).clamp(0.0, 65535.0) as u16;
        let py = (y * 256.0).clamp(0.0, 65535.0) as u16;
        let sc = (scale * 64.0).clamp(1.0, 255.0) as u8;
        let yaw = ((yaw_rad / std::f32::consts::TAU) * 255.0).round() as u8;
        Self {
            packed_xz: (px as u32) | ((pz as u32) << 16),
            packed_y_scale_yaw: (py as u32) | ((sc as u32) << 16) | ((yaw as u32) << 24),
        }
    }

    pub fn decode_xyz(&self) -> (f32, f32, f32) {
        let x = (self.packed_xz & 0xffff) as f32 / 256.0;
        let z = (self.packed_xz >> 16) as f32 / 256.0;
        let y = (self.packed_y_scale_yaw & 0xffff) as f32 / 256.0;
        (x, y, z)
    }
}

#[derive(Debug, Default)]
pub struct MaterialBatcher {
    /// material_id → list of quad indices
    groups: BTreeMap<u16, Vec<u32>>,
    translucent: BTreeMap<u16, Vec<u32>>,
}

impl MaterialBatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.groups.clear();
        self.translucent.clear();
    }

    pub fn push_quad(&mut self, material_id: u16, quad_index: u32, translucent: bool) {
        let map = if translucent {
            &mut self.translucent
        } else {
            &mut self.groups
        };
        map.entry(material_id).or_default().push(quad_index);
    }

    /// Merge consecutive quads into draw ranges (minimizes draw calls).
    pub fn build_draws(&self) -> (Vec<BatchedDraw>, Vec<BatchedDraw>) {
        (
            Self::compress_map(&self.groups, false),
            Self::compress_map(&self.translucent, true),
        )
    }

    fn compress_map(map: &BTreeMap<u16, Vec<u32>>, translucent: bool) -> Vec<BatchedDraw> {
        let mut out = Vec::with_capacity(map.len());
        for (&material_id, indices) in map {
            if indices.is_empty() {
                continue;
            }
            let mut sorted = indices.clone();
            sorted.sort_unstable();
            let mut start = sorted[0];
            let mut count = 1u32;
            let mut prev = sorted[0];
            for &idx in &sorted[1..] {
                if idx == prev + 1 {
                    count += 1;
                    prev = idx;
                } else {
                    out.push(BatchedDraw {
                        material_id,
                        first_quad: start,
                        quad_count: count,
                        translucent,
                    });
                    start = idx;
                    count = 1;
                    prev = idx;
                }
            }
            out.push(BatchedDraw {
                material_id,
                first_quad: start,
                quad_count: count,
                translucent,
            });
        }
        out
    }

    pub fn draw_call_count(&self) -> usize {
        let (o, t) = self.build_draws();
        o.len() + t.len()
    }
}

#[derive(Debug, Default)]
pub struct InstancedFloraRenderer {
    pub instances: Vec<FloraInstance>,
}

impl InstancedFloraRenderer {
    pub fn push(&mut self, x: f32, y: f32, z: f32, scale: f32, yaw: f32) {
        self.instances.push(FloraInstance::new(x, y, z, scale, yaw));
    }

    /// One draw call for all flora (instance_count = len).
    pub fn instance_count(&self) -> u32 {
        self.instances.len() as u32
    }

    pub fn bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.instances)
    }
}

/// Split mesh quads by opacity bit in material flags.
pub fn split_opaque_transparent(
    material_ids: &[u16],
    translucent_mats: &[bool],
) -> (Vec<u32>, Vec<u32>) {
    let mut opaque = Vec::new();
    let mut translucent = Vec::new();
    for (i, &mid) in material_ids.iter().enumerate() {
        let t = translucent_mats
            .get(mid as usize)
            .copied()
            .unwrap_or(false);
        if t {
            translucent.push(i as u32);
        } else {
            opaque.push(i as u32);
        }
    }
    (opaque, translucent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_merges_runs() {
        let mut b = MaterialBatcher::new();
        for i in 0..5 {
            b.push_quad(1, i, false);
        }
        b.push_quad(1, 10, false);
        let (opaque, _) = b.build_draws();
        assert_eq!(opaque.len(), 2);
        assert_eq!(opaque[0].quad_count, 5);
        assert_eq!(opaque[1].first_quad, 10);
    }

    #[test]
    fn flora_roundtrip() {
        let f = FloraInstance::new(3.5, 64.0, 8.25, 1.0, 0.0);
        let (x, y, z) = f.decode_xyz();
        assert!((x - 3.5).abs() < 0.01);
        assert!((y - 64.0).abs() < 0.01);
        assert!((z - 8.25).abs() < 0.01);
    }
}
