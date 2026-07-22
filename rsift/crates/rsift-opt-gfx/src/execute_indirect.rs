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

    /// MDI 1 パス上限に達した状態での push は**呼び出し側の容量設計ミス**
    /// (fail-loud; 旧実装は超過分を静寂 drop し、可視チャンクが誰にも
    /// 知らされず永久欠落した — 2026-07-22 wave 35 で根治)。
    ///
    /// ## wire 契約
    /// `start_instance_location` には **chunk_id** を搬送する
    /// (標準の instance 起点ではなく、VS 側で instance→chunk 参照する
    /// 本エンジン独自の割り当て)。
    pub fn push(
        &mut self,
        chunk_id: u32,
        index_count: u32,
        start_index: u32,
        base_vertex: i32,
        material: u32,
    ) {
        assert!(
            self.commands.len() < self.max_commands as usize,
            "IndirectBatcher 契約違反: max_commands={} 超過 push (len={})",
            self.max_commands,
            self.commands.len()
        );
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

    /// material 単一キーの**安定**ソート (同 material 内は入力順を保持。
    /// 位相順や sort キー順の pin が効くため、決定性を要求する経路はこちら)。
    pub fn merge_by_material(&mut self) {
        // 同一マテリアルでソートしてExecuteIndirectのステート変更を最小化
        self.commands.sort_by_key(|c| c.material_id);
    }

    /// Sort by material first, then by chunk locality (`chunk_id`) within each material batch
    /// to maximize GPU L1/L2 texture and vertex cache hit rates.
    ///
    /// キー全一致 (同一 material かつ同一 chunk_id) の要素順は `sort_unstable`
    /// のため非指定 (重複 chunk 投入時のみ影響)。同一バイナリ・同一入力では
    /// 決定的だが、言語保証が要る経路は `merge_by_material` (安定) を使う。
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
    /// MDI 1 パスの draw 上限 (GPU 側の固定長コマンドバッファに対応する
    /// 契約値)。`new(capacity)` で保持し `compact_and_filter` が fail-loud
    /// 強制する (旧実装は capacity を with_capacity のみに使い捨てて
    /// 超過を許容しており、固定長バッファ前提の将来配線で欠落/溢れの
    /// 温床だった — 2026-07-22 wave 35 で契約化)。
    pub capacity: usize,
}

impl DrawCompactor {
    pub fn new(capacity: usize) -> Self {
        Self {
            compacted_commands: Vec::with_capacity(capacity),
            header: IndirectCommandHeader {
                draw_count: 0,
                _pad: [0; 3],
            },
            capacity,
        }
    }

    /// Filter out culled (`instance_count == 0` or invisible) commands and pack surviving ones
    /// into a tightly packed indirect buffer with an explicit draw count header.
    ///
    /// **契約**: 生存コマンド数が `capacity` を超える場合は呼び出し側の
    /// 容量設計ミスとして fail-loud (wave 30 AF の棚卸し事項を閉じるため
    /// 超過の静寂通過を廃止)。`visibility_mask` の欠損要素は可視扱い
    /// (azdo::execute_azdo_pass が入口で等長を保証する)。
    pub fn compact_and_filter(&mut self, source: &[ChunkDrawCommand], visibility_mask: &[bool]) -> u32 {
        self.compacted_commands.clear();
        let mut count = 0u32;
        for (idx, cmd) in source.iter().enumerate() {
            let visible = visibility_mask.get(idx).copied().unwrap_or(true);
            if visible && cmd.args.instance_count > 0 && cmd.args.index_count_per_instance > 0 {
                assert!(
                    (count as usize) < self.capacity,
                    "DrawCompactor 契約違反: 生存 draw 数が capacity={} 超過 (source={} 件)",
                    self.capacity,
                    source.len()
                );
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

    /// wave 35-1: wire レイアウトの厳密サイズピン (repr(C) + Pod を前提とする
    /// GPU コマンドバッファとの契約)。
    #[test]
    fn wire_layout_exact_sizes() {
        assert_eq!(std::mem::size_of::<DrawIndexedIndirectArgs>(), 20);
        assert_eq!(std::mem::size_of::<ChunkDrawCommand>(), 36);
        assert_eq!(std::mem::size_of::<IndirectCommandHeader>(), 16);
        assert_eq!(std::mem::align_of::<ChunkDrawCommand>(), 4);
    }

    /// wave 35-2: batcher 上限ちょうどは受理、超過 push は fail-loud。
    #[test]
    fn batcher_capacity_boundary_exact() {
        let mut b = IndirectBatcher::new(2);
        b.push(1, 36, 0, 0, 0);
        b.push(2, 36, 36, 0, 0);
        assert_eq!(b.count(), 2);
        assert_eq!(b.as_bytes().len(), 2 * 36);
        // wire 契約: start_instance_location には chunk_id が搬送される
        assert_eq!(b.commands[0].args.start_instance_location, 1);
        assert_eq!(b.commands[1].args.start_instance_location, 2);
    }

    #[test]
    #[should_panic(expected = "IndirectBatcher 契約違反")]
    fn batcher_overflow_panics() {
        let mut b = IndirectBatcher::new(1);
        b.push(1, 36, 0, 0, 0);
        b.push(2, 36, 36, 0, 0); // 2 件目で capacity=1 超過 → panic
    }

    /// wave 35-3: compactor 容量契約 (上限ちょうど OK、超過 panic)。
    #[test]
    fn compactor_capacity_boundary_exact() {
        let mut src_batcher = IndirectBatcher::new(8);
        src_batcher.push(10, 36, 0, 0, 1);
        src_batcher.push(11, 36, 36, 0, 1);
        let mut c = DrawCompactor::new(2);
        let n = c.compact_and_filter(&src_batcher.commands, &[true, true]);
        assert_eq!(n, 2);
        assert_eq!(c.header.draw_count, 2);
        // 生存 <= capacity なら mask 欠損 (=可視扱い) でも受理される契約をピン
        let n2 = c.compact_and_filter(&src_batcher.commands, &[]);
        assert_eq!(n2, 2);
    }

    #[test]
    #[should_panic(expected = "DrawCompactor 契約違反")]
    fn compactor_overflow_panics() {
        let mut src_batcher = IndirectBatcher::new(8);
        src_batcher.push(10, 36, 0, 0, 1);
        src_batcher.push(11, 36, 36, 0, 1);
        src_batcher.push(12, 36, 72, 0, 1);
        let mut c = DrawCompactor::new(2); // capacity=2 < 3 生存
        let _ = c.compact_and_filter(&src_batcher.commands, &[true, true, true]);
    }

    /// wave 35-4: merge_by_material は安定ソート (同 material 内は入力順保持)。
    #[test]
    fn merge_by_material_is_stable() {
        let mut b = IndirectBatcher::new(8);
        b.push(5, 36, 0, 0, 2);
        b.push(2, 36, 36, 0, 1);
        b.push(9, 36, 72, 0, 2); // material 2 が 2 件 (chunk 5, 9)
        b.push(4, 36, 108, 0, 1);
        b.merge_by_material();
        let order: Vec<u32> = b.commands.iter().map(|c| c.chunk_id).collect();
        assert_eq!(
            order,
            vec![2, 4, 5, 9],
            "material 1: [2,4] / material 2: [5,9] 各入力順"
        );
    }
}
