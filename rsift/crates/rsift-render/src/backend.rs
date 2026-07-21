//! Render backend selection ladder — **DX12 優先ポリシー** (2026-07-21 ユーザー指示)。
//!
//! 選択順 (ユーザー指定のポリシーをそのままコード化):
//! 1. **DX12** (DirectX 12 Agility; 現行の唯一の実 present 経路)
//! 2. 旧世代 DX (DX11。ランタイム差し替え設計のみ、**present 未実装**)
//! 3. Vulkan (wgpu 経由を想定。**present 未実装** — opt-gfx の wgpu 資産は
//!    examples/launcher 経路にのみ存在し、JNI フレーム経路へは未配線)
//! 4. **GL パススルー** (バニラ Minecraft の LWJGL OpenGL 描画をそのまま通す
//!    安全網。Rsift 自身の描画ではないが黒画面を避ける)
//!
//! 本モジュールは**純ロジック** (副作用なし・GPU/JNI 不要) として選択判定を行い、
//! 初期化の実成否は呼び出し側 (rsift-jvm::render_bridge) が
//! [`record_realized_backend`] で記録する。選択できなかった候補が静かに
//! スキップされないよう、フォールバック時は呼び出し側が必ずログを発行する
//! (fail-loud 契約)。

use std::sync::atomic::{AtomicU8, Ordering};

/// 候補バックエンド種別。優先度は定義順 (DX12 が最優先)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderBackendKind {
    /// DirectX 12 Agility (rsift-dx12 の実装済み present 経路)。
    Dx12,
    /// 旧世代 DirectX (DX11)。**present 未実装** (2026-07-21 時点)。
    Dx11,
    /// Vulkan (wgpu 想定)。**present 未実装** (2026-07-21 時点)。
    VulkanWgpu,
    /// バニラ GL パススルー (LWJGL を遮断しない = バニラ描画そのまま)。
    GlPassthrough,
}

impl RenderBackendKind {
    /// present 経路が実装済みか (2026-07-21 時点の事実)。
    /// GlPassthrough は「Rsift が何もしない = 確実に描画される」ので true。
    pub fn present_implemented(self) -> bool {
        matches!(self, Self::Dx12 | Self::GlPassthrough)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Dx12 => "DX12 Agility",
            Self::Dx11 => "DX11 (未実装)",
            Self::VulkanWgpu => "Vulkan/wgpu (present 未実装)",
            Self::GlPassthrough => "バニラ GL パススルー",
        }
    }
}

/// 各候補の利用可否 (実行環境プローブの結果)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeState {
    /// 利用可能と判明。
    Available,
    /// 利用不可と判明 (ドライバ/OS/初期化失敗)。
    Unavailable,
    /// 未プローブ / 判定不能。
    Unknown,
    /// 実装が存在しない (コード未配備)。
    Unsupported,
}

/// 環境プローブ入力 (純データ。取得は呼び出し側)。
#[derive(Debug, Clone, Copy)]
pub struct BackendProbe {
    pub dx12: ProbeState,
    pub dx11: ProbeState,
    pub vulkan: ProbeState,
}

impl Default for BackendProbe {
    /// 現在の実装事実: DX11/Vulkan は present 未実装のため Unsupported 固定。
    /// dx12 は Unknown (実際の成否は init 試行で確定する)。
    fn default() -> Self {
        Self {
            dx12: ProbeState::Unknown,
            dx11: ProbeState::Unsupported,
            vulkan: ProbeState::Unsupported,
        }
    }
}

/// 強制指定モード (`rsift.render.backend` 環境変数由来)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForceMode {
    /// ポリシーラダー順に自動選択 (既定)。
    Auto,
    /// 指定 1 種に固定。初期化に失敗した場合の挙動は呼び出し側の
    /// fail-loud 契約に委ねる (ここで黙って別系統には落とさない)。
    Force(RenderBackendKind),
}

/// `rsift.render.backend` の値を解釈する。
/// 既知語彙: `dx12` / `dx11` / `vulkan` / `gl` / `vanilla` / `auto`。
/// 未知値は Auto (黙って解釈を変えないため reason に残す)。
pub fn parse_force_env(value: &str) -> (ForceMode, &'static str) {
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "" | "auto" => (ForceMode::Auto, "auto (既定ラダー)"),
        "dx12" | "d3d12" => (
            ForceMode::Force(RenderBackendKind::Dx12),
            "rsift.render.backend=dx12 (強制)",
        ),
        "dx11" | "d3d11" => (
            ForceMode::Force(RenderBackendKind::Dx11),
            "rsift.render.backend=dx11 (強制・present 未実装)",
        ),
        "vulkan" | "vk" | "wgpu" => (
            ForceMode::Force(RenderBackendKind::VulkanWgpu),
            "rsift.render.backend=vulkan (強制・present 未実装)",
        ),
        "gl" | "opengl" | "vanilla" => (
            ForceMode::Force(RenderBackendKind::GlPassthrough),
            "rsift.render.backend=gl (強制・バニラ描画)",
        ),
        _ => (ForceMode::Auto, "未知の rsift.render.backend 値 → auto にフォールバック"),
    }
}

/// 選択結果。
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    pub kind: RenderBackendKind,
    pub forced: bool,
    pub reason: &'static str,
}

/// ポリシーラダーに従って選択する純粋関数。
pub fn select(probe: &BackendProbe, force: ForceMode) -> Selection {
    if let ForceMode::Force(kind) = force {
        return Selection {
            kind,
            forced: true,
            reason: "環境変数による強制指定",
        };
    }
    // DX12 が最優先。利用可能性が Unknown の場合は「試行して確定」に回すため
    // Unavailable と確定した場合のみ次候補へ進む。
    if !matches!(probe.dx12, ProbeState::Unavailable) {
        return Selection {
            kind: RenderBackendKind::Dx12,
            forced: false,
            reason: "ポリシー最優先: DX12 Agility を試行 (失敗時は呼び出し側が次候補へ降格)",
        };
    }
    if matches!(probe.dx11, ProbeState::Available) {
        return Selection {
            kind: RenderBackendKind::Dx11,
            forced: false,
            reason: "DX12 不可のため旧 DX を選択",
        };
    }
    if matches!(probe.vulkan, ProbeState::Available) {
        return Selection {
            kind: RenderBackendKind::VulkanWgpu,
            forced: false,
            reason: "DX12/旧 DX 不可のため Vulkan を選択",
        };
    }
    Selection {
        kind: RenderBackendKind::GlPassthrough,
        forced: false,
        reason: "DX12/旧 DX/Vulkan のいずれも使えないためバニラ GL パススルー (fail-loud 対象)",
    }
}

// ---- 実現バックエンドの記録 (初期化成否の確定値) ----

const UNSET: u8 = 0;
static REALIZED: AtomicU8 = AtomicU8::new(UNSET);

fn kind_to_u8(k: RenderBackendKind) -> u8 {
    match k {
        RenderBackendKind::Dx12 => 1,
        RenderBackendKind::Dx11 => 2,
        RenderBackendKind::VulkanWgpu => 3,
        RenderBackendKind::GlPassthrough => 4,
    }
}

/// 初期化試行の実結果を記録する (render_bridge から呼ばれる)。
/// 一度確定したら上書きしない (降格は新規プロセスまで保持)。
pub fn record_realized_backend(k: RenderBackendKind) {
    let v = kind_to_u8(k);
    let _ = REALIZED.compare_exchange(UNSET, v, Ordering::SeqCst, Ordering::SeqCst);
}

/// 記録済みの実現バックエンド (未確定なら None)。
pub fn realized_backend() -> Option<RenderBackendKind> {
    match REALIZED.load(Ordering::SeqCst) {
        1 => Some(RenderBackendKind::Dx12),
        2 => Some(RenderBackendKind::Dx11),
        3 => Some(RenderBackendKind::VulkanWgpu),
        4 => Some(RenderBackendKind::GlPassthrough),
        _ => None,
    }
}

/// 現在の選択プラン + 実現状態を 1 行で返す (ステータスログ用)。
pub fn status_line() -> String {
    let probe = BackendProbe::default();
    let sel = select(&probe, ForceMode::Auto);
    match realized_backend() {
        Some(k) => format!(
            "plan={} → realized={} ({})",
            sel.kind.label(),
            k.label(),
            if k.present_implemented() {
                "present 経路あり"
            } else {
                "present 未実装"
            }
        ),
        None => format!("plan={} (初期化試行は未実施)", sel.kind.label()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(dx12: ProbeState, dx11: ProbeState, vulkan: ProbeState) -> BackendProbe {
        BackendProbe { dx12, dx11, vulkan }
    }

    #[test]
    fn auto_prefers_dx12_when_unknown_or_available() {
        for s in [ProbeState::Unknown, ProbeState::Available] {
            let sel = select(&probe(s, ProbeState::Unsupported, ProbeState::Unsupported), ForceMode::Auto);
            assert_eq!(sel.kind, RenderBackendKind::Dx12, "state={s:?}");
            assert!(!sel.forced);
        }
    }

    #[test]
    fn auto_skips_dx11_for_vulkan_when_dx12_unavailable() {
        // DX11 が未実装 (Unsupported) の現状では Vulkan が次候補。
        let sel = select(
            &probe(
                ProbeState::Unavailable,
                ProbeState::Unsupported,
                ProbeState::Available,
            ),
            ForceMode::Auto,
        );
        assert_eq!(sel.kind, RenderBackendKind::VulkanWgpu);
        // …ただし present 未実装であることは API から機械検証できる。
        assert!(!sel.kind.present_implemented());
    }

    #[test]
    fn auto_uses_dx11_only_if_actually_available() {
        let sel = select(
            &probe(
                ProbeState::Unavailable,
                ProbeState::Available,
                ProbeState::Available,
            ),
            ForceMode::Auto,
        );
        assert_eq!(sel.kind, RenderBackendKind::Dx11);
    }

    #[test]
    fn auto_falls_back_to_gl_passthrough_loudly() {
        let sel = select(
            &probe(
                ProbeState::Unavailable,
                ProbeState::Unavailable,
                ProbeState::Unavailable,
            ),
            ForceMode::Auto,
        );
        assert_eq!(sel.kind, RenderBackendKind::GlPassthrough);
        assert!(sel.reason.contains("fail-loud"));
    }

    #[test]
    fn default_probe_reflects_current_implementation_facts() {
        let p = BackendProbe::default();
        assert_eq!(p.dx11, ProbeState::Unsupported);
        assert_eq!(p.vulkan, ProbeState::Unsupported);
        // 現状の自動選択は DX12 試行 → 実成否は init で確定。
        assert_eq!(select(&p, ForceMode::Auto).kind, RenderBackendKind::Dx12);
    }

    #[test]
    fn force_env_parsing() {
        assert_eq!(
            parse_force_env("DX12").0,
            ForceMode::Force(RenderBackendKind::Dx12)
        );
        assert_eq!(
            parse_force_env("vulkan").0,
            ForceMode::Force(RenderBackendKind::VulkanWgpu)
        );
        assert_eq!(
            parse_force_env("gl").0,
            ForceMode::Force(RenderBackendKind::GlPassthrough)
        );
        assert_eq!(parse_force_env("").0, ForceMode::Auto);
        assert_eq!(parse_force_env("bogus").0, ForceMode::Auto);
        // 強制選択は probe 結果を覆す (黙って落とさない)。
        let sel = select(
            &probe(
                ProbeState::Unavailable,
                ProbeState::Unsupported,
                ProbeState::Unsupported,
            ),
            ForceMode::Force(RenderBackendKind::Dx12),
        );
        assert_eq!(sel.kind, RenderBackendKind::Dx12);
        assert!(sel.forced);
    }

    #[test]
    fn realized_backend_roundtrip() {
        // テストプロセス内で 1 度だけ確定できる点に注意 (compare_exchange UNSET→v)。
        assert!(realized_backend().is_none(), "前提: 未確定");
        record_realized_backend(RenderBackendKind::GlPassthrough);
        assert_eq!(realized_backend(), Some(RenderBackendKind::GlPassthrough));
        // 既確定は上書きしない。
        record_realized_backend(RenderBackendKind::Dx12);
        assert_eq!(realized_backend(), Some(RenderBackendKind::GlPassthrough));
        assert!(status_line().contains("バニラ GL"));
    }
}
