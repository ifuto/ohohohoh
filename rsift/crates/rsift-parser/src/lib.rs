//! # Rsift-Parser (SIMD Ultra-Fast Class Analyzer & Mixin Engine)
//!
//! SIMD命令 (AVX2/NEON/memchr) を活用して、数千のJavaクラスファイルを
//! ナノ秒単位で並列スキャンし、ターゲットクラスの特定からバイトコードの書き換え、
//! および Fabric Mixin / ASM 相当のインジェクションを高速に執行する Rsift の独自パーサーエンジン。

pub mod simd_scan;
pub mod class_file;
pub mod class_rewriter;
pub mod patcher;
pub mod mixin_eq;
pub mod compute_redirect;
pub mod classfile_parser;

pub use simd_scan::*;
pub use class_file::*;
pub use class_rewriter::*;
pub use patcher::*;
pub use mixin_eq::*;
pub use compute_redirect::*;
pub use classfile_parser::*;
