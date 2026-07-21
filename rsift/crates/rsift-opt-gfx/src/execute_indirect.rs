//! ExecuteIndirect + GPU-Driven Chunk Batching & Draw Compaction Engine
//!
//! Microsoft Docs `indirect-drawing-and-gpu-culling` および `GL_ARB_indirect_parameters`
//! 仕様に基づく完全実装。GPU が Compute Shader あるいは CPU カリング部で
//! 可視判定したコマンドを即座にコンパクション（`compact_and_filter`）し、
//! `instance_count` が 0 の無駄なドローコールを排除した連続コマンド配列と
//! ドローカウンター（ドロー数バッファ）を生成します。

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
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

/// D3D12 / Vulkan draw counter buffer header (`GL_ARB_indirect_parameters`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct IndirectCommandHeader {
    pub draw_count: u32,
    pub _pad: [u32; 3],
}

pub struct IndirectBatcher {
    commands: Vec<ChunkDrawCommand>,
    max_commands: u32,
    // 注: 旧 `gpu_buffer_size` フィールドは max_commands×stride の導出値を
    // 保持したまま一度も読まれないデッド状態だったため削除 (2026-07-21 監査)。
}

impl IndirectBatcher {
    pub fn new(max_commands: u32) -> Self {
        Self {
            commands: Vec::with_capacity(max_commands as usize),
            max_commands,
        }
    }

    pub fn push(
        &mut self,
        chunk_id: u32,
        index_count: u32,
        start_index: u32,
        base_vertex: i32,
        material: u32,
    ) {
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

    pub fn clear(&mut self) {
        self.commands.clear();
    }

    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.commands)
    }

    pub fn count(&self) -> u32 {
        self.commands.len() as u32
    }

    pub fn merge_by_material(&mut self) {
        // 同一マテリアルでソートしてExecuteIndirectのステート変更を最小化
        self.commands.sort_by_key(|c| c.material_id);
    }

    /// Sort by material first, then by chunk locality (`chunk_id`) within each material batch
    /// to maximize GPU L1/L2 texture and vertex cache hit rates.
    pub fn sort_by_material_and_locality(&mut self) {
        self.commands.sort_unstable_by(|a, b| {
            a.material_id
                .cmp(&b.material_id)
                .then_with(|| a.chunk_id.cmp(&b.chunk_id))
        });
    }
}

/// GPU/CPU Draw Compactor engine for zero-wasted multi-draw indirect submissions.
pub struct DrawCompactor {
    pub compacted_commands: Vec<ChunkDrawCommand>,
    pub header: IndirectCommandHeader,
}

impl DrawCompactor {
    pub fn new(capacity: usize) -> Self {
        Self {
            compacted_commands: Vec::with_capacity(capacity),
            header: IndirectCommandHeader {
                draw_count: 0,
                _pad: [0; 3],
            },
        }
    }

    /// Filter out culled (`instance_count == 0` or invisible) commands and pack surviving ones
    /// into a tightly packed indirect buffer with an explicit draw count header.
    pub fn compact_and_filter(&mut self, source: &[ChunkDrawCommand], visibility_mask: &[bool]) -> u32 {
        self.compacted_commands.clear();
        let mut count = 0u32;
        for (idx, cmd) in source.iter().enumerate() {
            let visible = visibility_mask.get(idx).copied().unwrap_or(true);
            if visible && cmd.args.instance_count > 0 && cmd.args.index_count_per_instance > 0 {
                self.compacted_commands.push(*cmd);
                count += 1;
            }
        }
        self.header.draw_count = count;
        count
    }

    pub fn header_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(&self.header)
    }

    pub fn commands_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.compacted_commands)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_sort() {
        let mut b = IndirectBatcher::new(10);
        b.push(5, 100, 0, 0, 2);
        b.push(2, 100, 0, 0, 1);
        b.push(1, 100, 0, 0, 2);
        b.sort_by_material_and_locality();
        assert_eq!(b.commands[0].material_id, 1);
        assert_eq!(b.commands[1].material_id, 2);
        assert_eq!(b.commands[1].chunk_id, 1);
        assert_eq!(b.commands[2].chunk_id, 5);
    }

    #[test]
    fn test_compactor() {
        let mut compactor = DrawCompactor::new(10);
        let mut b = IndirectBatcher::new(10);
        b.push(10, 36, 0, 0, 1);
        b.push(11, 36, 0, 0, 1);
        b.push(12, 36, 0, 0, 1);
        let mask = [true, false, true];
        let count = compactor.compact_and_filter(&b.commands, &mask);
        assert_eq!(count, 2);
        assert_eq!(compactor.compacted_commands.len(), 2);
        assert_eq!(compactor.compacted_commands[0].chunk_id, 10);
        assert_eq!(compactor.compacted_commands[1].chunk_id, 12);
        assert_eq!(compactor.header.draw_count, 2);
    }
}
