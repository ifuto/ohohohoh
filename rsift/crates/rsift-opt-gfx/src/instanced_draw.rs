
//! Instanced Rendering for Non-cube Blocks - 草花作物は同一メッシュをDrawInstanced
//! TransformはStructuredBufferでGPUへ

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct InstanceData {
    pub world_pos: [f32; 3],
    pub _pad0: f32,
    pub tex_index: u32,
    pub ao: u32,
    pub _pad1: [u32; 2],
}

pub struct InstancedGroup {
    pub mesh_id: u32,
    pub instances: Vec<InstanceData>,
}

pub struct InstancedCollector {
    groups: std::collections::HashMap<u32, InstancedGroup>,
}

impl InstancedCollector {
    pub fn new() -> Self { Self { groups: std::collections::HashMap::new() } }

    pub fn add(&mut self, mesh_id: u32, pos: [f32;3], tex: u32, ao: u32) {
        let g = self.groups.entry(mesh_id).or_insert_with(|| InstancedGroup { mesh_id, instances: Vec::new() });
        g.instances.push(InstanceData { world_pos: pos, _pad0: 0.0, tex_index: tex, ao, _pad1: [0;2] });
    }

    pub fn groups(&self) -> impl Iterator<Item=&InstancedGroup> { self.groups.values() }

    pub fn total_instances(&self) -> usize { self.groups.values().map(|g| g.instances.len()).sum() }

    pub fn as_bytes(&self, mesh_id: u32) -> Option<&[u8]> {
        self.groups.get(&mesh_id).map(|g| bytemuck::cast_slice(&g.instances))
    }
}
