//! 疑似Minecraft計測ハーネス RD24 シナリオ (49x49 chunks = 2,401 chunks / 28,812 セクション) — CaffeineMC 公式ベンチ条件 (描画距離 24 チャンク → (2*24+1)^2 = 2,401 chunks) と同一ロード規模の再現
//!
//! 測定ロジックは `shared/pseudo_mc_core.rs` に単一ソースで保持 (重複禁止) —
//! `CHUNKS_X/CHUNKS_Z` だけを差し替えて `include!` で同一内容を再コンパイル
//! する。実行:
//! `cargo run --release -p rsift-opt-gfx --example pseudo_mc_rd24`

const CHUNKS_X: usize = 49;
const CHUNKS_Z: usize = 49;

include!("shared/pseudo_mc_core.rs");
