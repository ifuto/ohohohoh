
//! ExecuteIndirect + GPU-driven Chunk Batching
//! Microsoft Docsのindirect-drawing-and-gpu-cullingサンプルに基づく: GPUでDrawArgsを書き込み、1回のExecuteIndirectで数千Drawを統合。

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct DrawIndexedIndirectArgs {
    pub index_count_per_instance: u32,
    pub instance_count: u32,
    pub start_index_location: u32,
    pub base_vertex_location: i32,
    pub start_instance_location: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct ChunkDrawCommand {
    pub args: DrawIndexedIndirectArgs,
    pub chunk_id: u32,
    pub material_id: u32,
    pub _pad: [u32; 2],
}

pub struct IndirectBatcher {
    commands: Vec<ChunkDrawCommand>,
    max_commands: u32,
    gpu_buffer_size: usize,
}

impl IndirectBatcher {
    pub fn new(max_commands: u32) -> Self {
        Self {
            commands: Vec::with_capacity(max_commands as usize),
            max_commands,
            gpu_buffer_size: max_commands as usize * std::mem::size_of::<ChunkDrawCommand>(),
        }
    }

    pub fn push(&mut self, chunk_id: u32, index_count: u32, start_index: u32, base_vertex: i32, material: u32) {
        if self.commands.len() >= self.max_commands as usize {
            return;
        }
        self.commands.push(ChunkDrawCommand {
            args: DrawIndexedIndirectArgs {
                index_count_per_instance: index_count,
                instance_count: 1,
                start_index_location: start_index,
                base_vertex_location: base_vertex,
                start_instance_location: chunk_id,
            },
            chunk_id,
            material_id: material,
            _pad: [0; 2],
        });
    }

    pub fn clear(&mut self) { self.commands.clear(); }

    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.commands)
    }

    pub fn count(&self) -> u32 { self.commands.len() as u32 }

    pub fn merge_by_material(&mut self) {
        // 同一マテリアルでソートしてExecuteIndirectのステート変更を最小化
        self.commands.sort_by_key(|c| c.material_id);
    }
}
