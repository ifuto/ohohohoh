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

    /// Execute one zero-driver-overhead pass.
    ///
    /// 現状の CPU 参照実装が行うのは **compaction のみ**: `visibility_mask` /
    /// `instance_count == 0` / `index_count == 0` で不可視コマンドを除去し、
    /// 生存者を `compactor.compacted_commands` に密詰めする。
    /// 「persistent ring への push」「MDI コマンドリストの準備」は本パスでは
    /// **未配線** (`vbo_pool` / `batcher` は将来の GPU パス用に保持されている
    /// だけでここでは使われない) — 2026-07-22 wave 30 で正直化。
    ///
    /// ## 契約
    /// - `commands.len() == visibility_mask.len()` 必須 (fail-loud。
    ///   `DrawCompactor` は短い mask の欠損要素を可視扱いするため、ずれを
    ///   ここで検出する)。
    /// - メトリクス `driver_overhead_saved_ns` は「全コマンドを naive に
    ///   個別 draw 発行した場合」との比較: 生存 count > 0 なら AZDO は MDI
    ///   1 発行、count == 0 なら 0 発行とみなし `len - (count>0)` 件 ×
    ///   1500 ns を加算する (1500 ns/draw は推定値。旧実装は常に len-1 で
    ///   count == 0 のとき 1 件過小だった — wave 30 で根治)。
    pub fn execute_azdo_pass(&mut self, commands: &[ChunkDrawCommand], visibility_mask: &[bool]) -> u32 {
        assert_eq!(
            commands.len(),
            visibility_mask.len(),
            "execute_azdo_pass 契約違反: commands と visibility_mask は等長必須 ({} vs {})",
            commands.len(),
            visibility_mask.len()
        );
        let count = self.compactor.compact_and_filter(commands, visibility_mask);
        // Track estimated driver API call overhead avoided: each draw call ~1,500 ns
        let saved_calls = commands.len() as u64 - u64::from(count > 0);
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

    /// wave 30 追加テスト用: 有効な描画コマンド (instance/index > 0)。
    fn live_cmd() -> ChunkDrawCommand {
        ChunkDrawCommand {
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
        }
    }

    /// wave 30-1: naive ベースラインの節約メトリクスを厳密ピン。
    /// - 空パス: +0 (AZDO 0 発行 vs naive 0 → 節約 0)
    /// - 2 生存: MDI 1 発行 vs naive 2 → +1×1500
    /// - 全不見: AZDO 0 発行 vs naive 2 → +2×1500 (旧実装は len-1 で 1 件過小)
    #[test]
    fn overhead_metric_matches_naive_baseline_exact() {
        let mut azdo = AzdoOrchestrator::new(100, 4);
        assert_eq!(azdo.execute_azdo_pass(&[], &[]), 0);
        assert_eq!(azdo.driver_overhead_saved_ns.load(Ordering::Relaxed), 0);
        let two = [live_cmd(), live_cmd()];
        assert_eq!(azdo.execute_azdo_pass(&two, &[true, true]), 2);
        assert_eq!(azdo.execute_azdo_pass(&two, &[true, true]), 2);
        assert_eq!(azdo.driver_overhead_saved_ns.load(Ordering::Relaxed), 3000);
        assert_eq!(azdo.execute_azdo_pass(&two, &[false, false]), 0);
        assert_eq!(azdo.driver_overhead_saved_ns.load(Ordering::Relaxed), 6000);
        assert_eq!(azdo.total_overhead_saved_ms(), 0.006);
    }

    /// wave 30-2: mask 等長契約 fail-loud (欠損=可視の暗黙仕様を入口で拒否)。
    #[test]
    #[should_panic(expected = "execute_azdo_pass 契約違反")]
    fn visibility_mask_length_mismatch_panics() {
        let mut azdo = AzdoOrchestrator::new(100, 4);
        let two = [live_cmd(), live_cmd()];
        let _ = azdo.execute_azdo_pass(&two, &[true]); // 1 件短い
    }
}
