//! # 18. AZDO (`Approaching Zero Driver Overhead` Orchestrator)
//!
//! Persistent Mapped Buffers, Multi-Draw Indirect, Direct State Access を
//! 統合し、ドライバーの API オーバーヘッドと CPU 専有率を極限までゼロへ削減する。

use crate::execute_indirect::{ChunkDrawCommand, DrawCompactor, IndirectBatcher};
use crate::persistent_vbo_pool::PersistentVboPool;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct AzdoOrchestrator {
    pub vbo_pool: PersistentVboPool,
    pub batcher: IndirectBatcher,
    pub compactor: DrawCompactor,
    pub driver_overhead_saved_ns: AtomicU64,
}

impl AzdoOrchestrator {
    /// `max_commands`: MDI コマンド上限。`vbo_capacity_mb`: persistent VBO
    /// プール容量 (**MiB 単位** — `PersistentVboPool::new` と同一仕様。
    /// 以前の引数名 `vbo_capacity_bytes` は実態と乖離しており、
    /// `1 << 20` 等を渡すと 768 GiB 要求で異常アロケーションとなった)。
    pub fn new(max_commands: u32, vbo_capacity_mb: usize) -> Self {
        Self {
            vbo_pool: PersistentVboPool::new(vbo_capacity_mb),
            batcher: IndirectBatcher::new(max_commands),
            compactor: DrawCompactor::new(max_commands as usize),
            driver_overhead_saved_ns: AtomicU64::new(0),
        }
    }

    /// Execute one zero-driver-overhead pass: push draws straight to Persistent Mapped ring,
    /// compact invisible commands, and prepare 1 single Multi-Draw Indirect command list.
    pub fn execute_azdo_pass(&mut self, commands: &[ChunkDrawCommand], visibility_mask: &[bool]) -> u32 {
        let count = self.compactor.compact_and_filter(commands, visibility_mask);
        // Track estimated driver API call overhead avoided: each draw call ~1,500 ns
        let saved_calls = commands.len().saturating_sub(1) as u64;
        self.driver_overhead_saved_ns
            .fetch_add(saved_calls * 1500, Ordering::Relaxed);
        count
    }

    pub fn total_overhead_saved_ms(&self) -> f64 {
        self.driver_overhead_saved_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execute_indirect::{ChunkDrawCommand, DrawIndexedIndirectArgs};

    #[test]
    fn test_azdo_orchestrator_execution() {
        // 第2引数は MiB (PersistentVboPool 仕様)。以前の `1 << 20` は
        // bytes 意図の誤用で 1048576 MiB (=1 TiB プール → vertex staging
        // 768 GiB) を要求し、テストが OOM SIGABRT で死んでいた。
        let mut azdo = AzdoOrchestrator::new(100, 4);
        // `compact_and_filter` は instance_count==0 / index_count==0 の
        // コマンドを正しく除去する (AZDO 設計)。Zeroable::zeroed() の
        // 全ゼロ args は除去対象なので、ここでは有効な描画コマンドを渡す。
        let cmd = ChunkDrawCommand {
            args: DrawIndexedIndirectArgs {
                index_count_per_instance: 36,
                instance_count: 1,
                start_index_location: 0,
                base_vertex_location: 0,
                start_instance_location: 0,
            },
            chunk_id: 1,
            material_id: 1,
            _pad: [0; 2],
        };
        let count = azdo.execute_azdo_pass(&[cmd, cmd], &[true, true]);
        assert_eq!(count, 2);
    }
}
