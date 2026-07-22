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
    /// CPU 参照実装の責務は **compaction + MDI コマンドリスト準備**:
    /// 1. `visibility_mask` / `instance_count == 0` / `index_count == 0` で
    ///    不可視コマンドを除去し、生存者を `compactor.compacted_commands`
    ///    に密詰めする。
    /// 2. 生存コマンドを `batcher` に積み直し、`batcher.as_bytes()` で
    ///    **GPU 間接バッファへそのまま上傳可能な決定的バイト列**を提供する
    ///    (2026-07-23 wave 45: 「MDI コマンドリストの準備 未配線」の
    ///    closing。生存数 ≤ compactor 容量 = batcher 容量なので push の
    ///    容量 assert は構造上不発)。
    ///
    /// `vbo_pool` (persistent ring への頂点 push) は**本パスの責務外**:
    /// 本パスの入力は draw コマンドのみでメッシュデータ (`BuiltChunkMesh`)
    /// を持たない。頂点常駐は upload 経路
    /// (`upload_mesh` / `release` / `rebuild_mdi`) が別フェーズとして行う
    /// 設計であり、「draw 圧縮パス」と「メッシュ常駐パス」の分離は意図的
    /// (スタブではなく責務境界 — 2026-07-23 wave 45 で明文化)。
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
        // MDI コマンドリスト準備 (wave 45): 生存集合を batcher に積み直す。
        // 順序は compactor と一致 (安定)、wire は IndirectBatcher::push の
        // 契約 (start_instance_location = chunk_id) に従う。
        let compactor = &self.compactor;
        let batcher = &mut self.batcher;
        batcher.clear();
        for cmd in &compactor.compacted_commands {
            batcher.push(
                cmd.chunk_id,
                cmd.args.index_count_per_instance,
                cmd.args.start_index_location,
                cmd.args.base_vertex_location,
                cmd.material_id,
            );
        }
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

    /// wave 45 用ヘルパ: フィールドを区別できる有効コマンド。
    fn distinct_cmd(chunk_id: u32, ic: u32, il: u32, bv: i32, mat: u32) -> ChunkDrawCommand {
        ChunkDrawCommand {
            args: DrawIndexedIndirectArgs {
                index_count_per_instance: ic,
                instance_count: 1,
                start_index_location: il,
                base_vertex_location: bv,
                start_instance_location: 0, // batcher が chunk_id で上書きする設計
            },
            chunk_id,
            material_id: mat,
            _pad: [0; 2],
        }
    }

    /// wave 45-1: 生存集合が batcher へ**順序保持**で配線され、wire 契約
    /// (start_instance_location = chunk_id, instance_count = 1) とバイト列
    /// が厳密一致する (MDI 準備 closing の機械ピン)。
    #[test]
    fn pass_wires_survivors_into_batcher_byte_exact() {
        let mut azdo = AzdoOrchestrator::new(100, 4);
        let a = distinct_cmd(10, 36, 100, 5, 2);
        let b = distinct_cmd(11, 36, 200, 5, 2); // mask により除去
        let c = distinct_cmd(12, 60, 400, -3, 7); // base_vertex 負値 (i32) も通す
        let count = azdo.execute_azdo_pass(&[a, b, c], &[true, false, true]);
        assert_eq!(count, 2);
        assert_eq!(azdo.batcher.count(), 2, "batcher は生存数と一致");
        // as_bytes → Pod 逆変換で 36B × 2 の厳密構造
        let cmds: &[ChunkDrawCommand] = bytemuck::cast_slice(azdo.batcher.as_bytes());
        assert_eq!(cmds.len(), 2);
        // 順序保持 (安定): a → c
        assert_eq!(cmds[0].chunk_id, 10);
        assert_eq!(cmds[1].chunk_id, 12);
        // wire 契約: start_instance_location = chunk_id、instance_count = 1 正規化
        assert_eq!(cmds[0].args.start_instance_location, 10);
        assert_eq!(cmds[1].args.start_instance_location, 12);
        assert_eq!(cmds[0].args.instance_count, 1);
        // 引き継ぎフィールド厳密
        assert_eq!(cmds[1].args.index_count_per_instance, 60);
        assert_eq!(cmds[1].args.start_index_location, 400);
        assert_eq!(cmds[1].args.base_vertex_location, -3);
        assert_eq!(cmds[1].material_id, 7);
    }

    /// wave 45-2: 連続呼出で batcher は clear され残滓を持たない
    /// (重複蓄積バグの機械排除)。
    #[test]
    fn pass_batcher_is_cleared_between_calls() {
        let mut azdo = AzdoOrchestrator::new(100, 4);
        let a = distinct_cmd(10, 36, 100, 5, 2);
        let c = distinct_cmd(12, 60, 400, -3, 7);
        assert_eq!(azdo.execute_azdo_pass(&[a, c], &[true, true]), 2);
        assert_eq!(azdo.batcher.count(), 2);
        // 2 回目は 1 件のみ生存
        assert_eq!(azdo.execute_azdo_pass(&[a, c], &[true, false]), 1);
        assert_eq!(azdo.batcher.count(), 1, "前回分の残滓なし");
        let cmds: &[ChunkDrawCommand] = bytemuck::cast_slice(azdo.batcher.as_bytes());
        assert_eq!(cmds[0].chunk_id, 10);
        // 全除去 — 生存 0 でも batcher は空整合
        assert_eq!(azdo.execute_azdo_pass(&[a], &[false]), 0);
        assert_eq!(azdo.batcher.count(), 0);
        assert!(azdo.batcher.as_bytes().is_empty());
    }

    /// wave 45-3: compactor と batcher の内容一致 (同一生存集合の二写しで
    /// あることを、 wire 契約差分 (instance_count 正規化) を除き機械検証)。
    #[test]
    fn batcher_mirrors_compactor_surviving_set() {
        let mut azdo = AzdoOrchestrator::new(100, 4);
        let cmds: Vec<ChunkDrawCommand> = (0..32)
            .map(|i| distinct_cmd(100 + i, 36 + i, 1000 + 64 * i, 0, 3))
            .collect();
        // 偶数 index のみ可視
        let mask: Vec<bool> = (0..32).map(|i| i % 2 == 0).collect();
        let n = azdo.execute_azdo_pass(&cmds, &mask);
        assert_eq!(n, 16);
        let batched: &[ChunkDrawCommand] = bytemuck::cast_slice(azdo.batcher.as_bytes());
        assert_eq!(batched.len(), azdo.compactor.compacted_commands.len());
        for (b, k) in batched.iter().zip(azdo.compactor.compacted_commands.iter()) {
            assert_eq!(b.chunk_id, k.chunk_id, "生存集合・順序一致");
            assert_eq!(
                b.args.index_count_per_instance,
                k.args.index_count_per_instance
            );
            assert_eq!(b.args.start_index_location, k.args.start_index_location);
            assert_eq!(b.args.base_vertex_location, k.args.base_vertex_location);
            assert_eq!(b.material_id, k.material_id);
            assert_eq!(b.args.start_instance_location, b.chunk_id);
        }
        // 生存者は chunk_id 100,102,...,130 (偶数 index 16 件)
        let ids: Vec<u32> = batched.iter().map(|c| c.chunk_id).collect();
        assert_eq!(ids, (0..16).map(|i| 100 + 2 * i).collect::<Vec<u32>>());
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
