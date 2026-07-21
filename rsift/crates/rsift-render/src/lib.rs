//! # Rsift Render Engine — DX12 優先バックエンドラダー。
//!
//! ポリシー (2026-07-21 ユーザー指示): DX12 → 旧 DX (未実装) → Vulkan (未実装)
//! → バニラ GL パススルー。選択ロジックは `backend` モジュール (純ロジック・
//! テスト済)。旧 "no OpenGL / Vulkan / wgpu" の説明は不正確だったため訂正:
//! DX12 が使えない環境ではバニラ GL 描画を通す安全網が存在する。

pub mod backend;
pub mod dx12_engine;
pub mod proxy;
pub mod recorder;

pub use backend::*;
pub use dx12_engine::*;
pub use proxy::*;
pub use recorder::*;
