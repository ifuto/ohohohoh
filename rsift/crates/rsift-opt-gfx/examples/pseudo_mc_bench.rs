//! 疑似Minecraft計測ハーネス (基準スケール 6x6 chunks = 36 chunks / 432 セクション)
//!
//! 測定ロジックは `shared/pseudo_mc_core.rs` に単一ソースで保持 (重複禁止) —
//! `CHUNKS_X/CHUNKS_Z` だけを差し替えて `include!` で同一内容を再コンパイル
//! する。実行:
//! `cargo run --release -p rsift-opt-gfx --example pseudo_mc_bench`

const CHUNKS_X: usize = 6;
const CHUNKS_Z: usize = 6;

include!("shared/pseudo_mc_core.rs");
