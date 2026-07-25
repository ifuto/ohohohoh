//! # Rsift JVM Engine (Invocation API & JVMTI Bridge)
//!
//! Rustプロセス内部に Minecraft 1.21.11 JVM を動的生成し、
//! メモリ管理とスレッド主導権を掌握するコアモジュール。

pub mod invoker;
pub mod jvmti_hook;
pub mod agent_log;
pub mod agent_opts;
pub mod agent_bridge;
pub mod chunk_bridge;
pub mod mod_bridge;
pub mod platform_bridge;
pub mod render_bridge;
pub mod screen_inject;
pub mod screen_buttons;
pub mod glfw_hook;
pub mod jvmti_events;

// Category 4: JVM Interop 高速化 - First Proposal Full Implementation
pub mod jni_critical;
pub mod direct_byte_buffer;
pub mod ring_buffer_shared;
pub mod c_abi_vtable;
pub mod method_id_cache;
pub mod symbol_table;
pub mod detour_hook;
pub mod bytecode_transpiler;
pub mod wasm_sandbox;
pub mod abi_stable;

pub use invoker::*;
pub use jvmti_hook::*;
pub use agent_bridge::*;
