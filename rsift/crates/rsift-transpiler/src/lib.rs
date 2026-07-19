//! # Rsift Bytecode-to-Native Transpiler (`rsift-transpiler`)
//!
//! Zero-Compatibility-Loss AOT/JIT engine: Java stack machine -> native SSA registers
//! with JVM-compatible object header preservation.

pub mod object_layout;
pub mod aot_engine;
pub mod domains;
pub mod parity;
pub mod world_mirror;
pub mod vanilla_tick;
pub mod compute_runtime;
pub mod jni_bridge;
pub mod frame_arena;
pub mod simd_dispatch;
pub mod const_lut_gen;

pub use object_layout::*;
pub use aot_engine::*;
pub use domains::*;
pub use parity::*;
pub use world_mirror::*;
pub use vanilla_tick::*;
pub use compute_runtime::*;
pub use jni_bridge::*;
pub use frame_arena::*;
pub use simd_dispatch::*;
pub use const_lut_gen::*;
