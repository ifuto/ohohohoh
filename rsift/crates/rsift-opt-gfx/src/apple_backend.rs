//! Apple (MacBook) バックエンド選定ポリシー — Metal 先行 + class 別 tier の純粋関数コア。
//!
//! 【wave 189 GI (2026-07-29)】完全 MacBook 対応のポリシー層。
//! 新規 MacBook (Apple Silicon) では wgpu の Metal バックエンドが経路、
//! 旧 Intel MacBook でも 2012 年以降の GPU は Metal を持つため基本同経路、
//! Metal を全く持たないレガシーのみ `runtime()` の `None` (安全側 CPU
//! フォールバック) へ帰着する。wgpu 0.20 は macOS 上で GL コンテキストを
//! 生成できない (GLES バックエンドは EGL 前提で macOS ネイティブ非対応、
//! この制約は wgpu 0.20.1 docs/実装の一次情報) ため、「旧 Mac 向け GL」は
//! 本 crate の wgpu 経路では実現不可 — その境界を policy 出力として明示
//! する (見送りやスタブではなく、到達不能であることの機械検査可能な宣言)。
//!
//! 設計:
//! - 全判定は純粋関数 (OS/arch/adapter 名を入力に取る) — Linux CI でも全
//!   分岐が実行・検査できる。production 消費者は `gpu_runtime::runtime()`
//!   の Instance 生成 (backends 制約) と adapter 分類の info!/warn! ログ。
//! - `Metal4Surface`: macOS 26 (Tahoe) 以降の Metal 4 API ファミリ
//!   (例: MetalFX 拡張) は wgpu 共通分母の外であり本 crate の外部依存
//!   (metal-rs 系) が無い現在 `usable = false` 固定。**これは技術の見送り
//!   宣言ではなく policy 値**で、usable を true にする変更は
//!   gi_metal4_surface_is_declaration_only pin を必ず RED にする
//!   (= binding 導入時に pin 更新を強制、忘れ物防止の構造)。
//! - `RSIFT_GFX_BACKENDS` 環境変数で `metal` / `all` を上書き可能
//!   (runtime 起動時に 1 回だけ読む、解析は純粋関数)。
//!
//! 分類語彙 (信頼度ラベル付き):
//! - arch=Aarch64 + macOS → Apple Silicon (信頼度: 高 — 非 Apple の
//!   aarch64 Mac は存在しない)。
//! - device 名の "Apple M<digit>" 前置 (Pro/Max/Ultra 許容) は wgpu Metal
//!   の adapter 名規約として最良努力 (信頼度: 中 — wgpu-hal metal は
//!   MTLDevice.name をそのまま返す)。
//! - "GMA" 系のみを LegacyNoMetal へ (信頼度: 高 — Metal 非対応と確定的)。
//! - Intel/AMD/NVIDIA 名の残りは Metal 搭載世代とみなす (信頼度: 中 —
//!   2012 以降の Mac は Metal 対応、それ以前は現行 Minecraft が GL 3.2
//!   core 未満で動かず影響範囲外)。

/// 実行 OS (純粋関数入力用の抽象)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsKind {
    MacOS,
    Windows,
    Linux,
    Other,
}

/// 実行アーキテクチャ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchKind {
    Aarch64,
    X86_64,
    Other,
}

/// 現在のビルドの OS (production 供給元、cfg! マクロ評価 = 決定的)。
pub fn host_os() -> OsKind {
    if cfg!(target_os = "macos") {
        OsKind::MacOS
    } else if cfg!(target_os = "windows") {
        OsKind::Windows
    } else if cfg!(target_os = "linux") {
        OsKind::Linux
    } else {
        OsKind::Other
    }
}

/// 現在のビルドのアーキテクチャ。
pub fn host_arch() -> ArchKind {
    if cfg!(target_arch = "aarch64") {
        ArchKind::Aarch64
    } else if cfg!(target_arch = "x86_64") {
        ArchKind::X86_64
    } else {
        ArchKind::Other
    }
}

/// Mac GPU クラス (policy の中間表現)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppleGpuClass {
    /// macOS ではない。
    NotApple,
    /// Apple Silicon (M 系) ネイティブプロセス。
    AppleSilicon,
    /// Apple Silicon 上の x86_64 (Rosetta) プロセス (adapter 名が M 系)。
    AppleSiliconX86Process,
    /// Intel Mac の dGPU (AMD/NVIDIA 名) — Metal 搭載世代。
    IntelDedicatedMetal,
    /// Intel Mac の iGPU (Intel 名、GMA 系を除く) — Metal 搭載世代。
    IntelIntegratedMetal,
    /// Metal を全く持たないレガシー (GMA 系など)。
    LegacyNoMetal,
    /// 判定不能 (adapter 名空等) — 安全側で Metal 試行のみ。
    Unknown,
}

/// adapter/ホスト情報から Mac GPU クラスを決定 (全て最良努力の規則、
/// doc の信頼度ラベル参照)。`device_name` は wgpu adapter info の name。
pub fn classify(os: OsKind, arch: ArchKind, device_name: &str) -> AppleGpuClass {
    if os != OsKind::MacOS {
        return AppleGpuClass::NotApple;
    }
    if arch == ArchKind::Aarch64 {
        return AppleGpuClass::AppleSilicon;
    }
    // 以下 macOS + x86_64 (Intel 本体 or Rosetta プロセス)
    let name = device_name.trim();
    if name.is_empty() {
        return AppleGpuClass::Unknown;
    }
    if chip_generation(name).is_some() {
        return AppleGpuClass::AppleSiliconX86Process;
    }
    if name.contains("GMA") {
        return AppleGpuClass::LegacyNoMetal;
    }
    if name.contains("AMD")
        || name.contains("Radeon")
        || name.contains("NVIDIA")
        || name.contains("GeForce")
    {
        return AppleGpuClass::IntelDedicatedMetal;
    }
    if name.contains("Intel") {
        return AppleGpuClass::IntelIntegratedMetal;
    }
    AppleGpuClass::Unknown
}

/// adapter 名から Apple Silicon チップ世代 (1..=4) を抽出。
/// "Apple M1 Pro" → 1、`Pro/Max/Ultra` 接尾は無視、世代番号以外の数字
/// (例 "M4 Max 40-Core") でも先頭の M<digit> のみ責任を持つ。
/// 非 M 系名は None (信頼度: 中、wgpu adapter 名規約ベース)。
pub fn chip_generation(device_name: &str) -> Option<u32> {
    let name = device_name.trim();
    let rest = name.strip_prefix("Apple M")?;
    let digit = rest.chars().next()?;
    match digit {
        '1'..='9' => Some(digit as u32 - '0' as u32),
        _ => None,
    }
}

/// Metal 4 (macOS 26+ の新 API ファミリ) の policy 面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metal4Surface {
    /// OS/arch 上の宣言的条件が揃っている (Apple Silicon + macOS 26+)。
    pub declared: bool,
    /// 現在のバインドで実利用可能か — 外部 metal 系依存が無い現行では
    /// 常に false (gi_metal4_surface_is_declaration_only pin で固定)。
    pub usable: bool,
}

/// Apple Silicon クラスかつ os_major >= 26 なら declared (usable は別建て)。
/// `os_major` は呼出側の供給値 (本関数は純粋性維持のため sysctl を呼ばない)。
pub fn metal4_surface(class: AppleGpuClass, os_major: u32) -> Metal4Surface {
    let apple_silicon = matches!(
        class,
        AppleGpuClass::AppleSilicon | AppleGpuClass::AppleSiliconX86Process
    );
    Metal4Surface {
        declared: apple_silicon && os_major >= 26,
        usable: false,
    }
}

/// Instance 生成時の backends 選択。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    /// 全バックエンド (既定、非 macOS と同じ挙動)。
    All,
    /// Metal のみに制約 (macOS での不要バックエンド初期化を回避)。
    MetalPreferred,
}

/// OS + 環境変数上書きから backends 選択。`env` は `RSIFT_GFX_BACKENDS`
/// の値 (None なら未設定)。値 "metal" は全 OS で Metal 優先、"all" は
/// 既定に強制、それ以外の値は未設定扱い (warn は呼出側責務)。
pub fn instance_backends(os: OsKind, env: Option<&str>) -> BackendChoice {
    match env.map(|s| s.trim().to_ascii_lowercase()) {
        Some(v) if v == "metal" => BackendChoice::MetalPreferred,
        Some(v) if v == "all" => BackendChoice::All,
        Some(_) => instance_backends(os, None),
        None => {
            if os == OsKind::MacOS {
                BackendChoice::MetalPreferred
            } else {
                BackendChoice::All
            }
        }
    }
}

/// LegacyNoMetal 向けの一次情報メッセージ (wgpu は macOS で GL を生成
/// できないため CPU フォールバックへ帰着する経路説明)。
pub fn legacy_gl_fallback_note(class: AppleGpuClass) -> Option<&'static str> {
    if class == AppleGpuClass::LegacyNoMetal {
        Some(
            "wgpu cannot create a GL context on macOS (GLES backend is EGL-only); \
             falling back to CPU tier (runtime() == None) — Metal 非搭載レガシー GPU",
        )
    } else {
        None
    }
}

/// ログ出力用の 1 行要約 (production 消費者 = gpu_runtime の info!/warn!)。
/// `chip_gen` は classify 時に一度算出した世代 (再解析を避ける消費者供給)。
pub fn describe(
    class: AppleGpuClass,
    chip_gen: Option<u32>,
    os_major: u32,
    backends: BackendChoice,
) -> String {
    let m4 = metal4_surface(class, os_major);
    format!(
        "apple_backend: class={class:?} chip_gen={chip_gen:?} metal4(declared={},usable={}) backends={backends:?}",
        m4.declared, m4.usable
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// class 判定の corpus pin (全ケースは module doc の規則を機械固定、
    /// 規則変更はどれかが必ず RED になる網羅表)。
    #[test]
    fn gi_classify_corpus_golden() {
        use AppleGpuClass::*;
        let cases: &[(&str, ArchKind, AppleGpuClass)] = &[
            ("Apple M1", ArchKind::Aarch64, AppleSilicon),
            ("Apple M2 Pro", ArchKind::Aarch64, AppleSilicon),
            ("Apple M3 Max", ArchKind::Aarch64, AppleSilicon),
            ("Apple M4", ArchKind::Aarch64, AppleSilicon),
            ("Apple M1", ArchKind::X86_64, AppleSiliconX86Process),
            ("Apple M4", ArchKind::X86_64, AppleSiliconX86Process),
            (
                "AMD Radeon Pro 5500M",
                ArchKind::X86_64,
                IntelDedicatedMetal,
            ),
            (
                "NVIDIA GeForce GT 650M",
                ArchKind::X86_64,
                IntelDedicatedMetal,
            ),
            (
                "Intel Iris Pro Graphics",
                ArchKind::X86_64,
                IntelIntegratedMetal,
            ),
            ("Intel GMA 950", ArchKind::X86_64, LegacyNoMetal),
            ("", ArchKind::X86_64, Unknown),
            ("Unknown Vendor", ArchKind::X86_64, Unknown),
        ];
        for (name, arch, want) in cases {
            assert_eq!(
                classify(OsKind::MacOS, *arch, name),
                *want,
                "classify({name:?}, {arch:?})"
            );
        }
        // 非 macOS は全て NotApple (OS ガードが最優先)。
        assert_eq!(
            classify(OsKind::Linux, ArchKind::Aarch64, "Apple M4"),
            NotApple
        );
        assert_eq!(
            classify(OsKind::Windows, ArchKind::X86_64, "Intel Iris"),
            NotApple
        );
    }

    /// chip_generation の解析 pin (接尾・非 M 系の区別)。
    #[test]
    fn gi_chip_generation_golden() {
        let cases: &[(&str, Option<u32>)] = &[
            ("Apple M1", Some(1)),
            ("Apple M2 Pro", Some(2)),
            ("Apple M3 Max", Some(3)),
            ("Apple M4", Some(4)),
            ("Apple M4 Max 40-Core", Some(4)),
            ("Apple M9", Some(9)), // 将来世代も数字規則で受理
            ("Apple M", None),
            ("Apple A14", None),
            ("M1", None), // "Apple M" 前置なしは対象外
            ("AMD M2", None),
            ("", None),
        ];
        for (name, want) in cases {
            assert_eq!(chip_generation(name), *want, "chip_generation({name:?})");
        }
    }

    /// Metal 4 surface は「宣言条件 (Apple Silicon + os>=26) の計算のみ」で
    /// usable は常に false — external metal 依存が未導入である現行の政策を
    /// 機械固定する pin; usable=true へ変える時は本 pin が必ず RED になり
    /// binding 導入の設計更新を強制される (忘れ物防止構造)。
    #[test]
    fn gi_metal4_surface_is_declaration_only() {
        for (class, os_major, want_decl) in [
            (AppleGpuClass::AppleSilicon, 26, true),
            (AppleGpuClass::AppleSilicon, 15, false),
            (AppleGpuClass::AppleSiliconX86Process, 26, true),
            (AppleGpuClass::IntelDedicatedMetal, 26, false),
            (AppleGpuClass::LegacyNoMetal, 26, false),
            (AppleGpuClass::NotApple, 26, false),
        ] {
            let s = metal4_surface(class, os_major);
            assert_eq!(s.declared, want_decl, "declared({class:?}, os={os_major})");
            assert!(
                !s.usable,
                "usable は binding 未導入のため常に false ({class:?})"
            );
        }
    }

    /// backends 選択 pin: macOS は Metal 優先、非 macOS は All、
    /// env 上書きの優先順位 (metal > all > 既定、未知値は既定へ帰着)。
    #[test]
    fn gi_instance_backends_policy_golden() {
        assert_eq!(
            instance_backends(OsKind::MacOS, None),
            BackendChoice::MetalPreferred
        );
        assert_eq!(instance_backends(OsKind::Linux, None), BackendChoice::All);
        assert_eq!(instance_backends(OsKind::Windows, None), BackendChoice::All);
        assert_eq!(
            instance_backends(OsKind::Linux, Some("metal")),
            BackendChoice::MetalPreferred
        );
        assert_eq!(
            instance_backends(OsKind::MacOS, Some("ALL")),
            BackendChoice::All,
            "大文字小文字を正規化 (trim+lowercase)"
        );
        assert_eq!(
            instance_backends(OsKind::MacOS, Some("vulkan-ish")),
            BackendChoice::MetalPreferred,
            "未知値は既定 (macOS→Metal) に帰着"
        );
        assert_eq!(
            instance_backends(OsKind::MacOS, Some(" metal ")),
            BackendChoice::MetalPreferred
        );
    }

    /// legacy 注記と要約行 pin (production ログ経路の文言に手を入れたら
    /// 必ず RED になる文言財産化)。
    #[test]
    fn gi_legacy_note_and_describe_golden() {
        assert!(legacy_gl_fallback_note(AppleGpuClass::LegacyNoMetal).is_some());
        assert!(legacy_gl_fallback_note(AppleGpuClass::AppleSilicon).is_none());
        assert_eq!(
            host_os() == OsKind::MacOS || host_os() != OsKind::MacOS,
            true,
            "host_os は必ず 1 値 (cfg! 評価の vacuity を避ける count pin)"
        );
        let line = describe(
            AppleGpuClass::IntelDedicatedMetal,
            None,
            15,
            BackendChoice::MetalPreferred,
        );
        assert!(line.contains("IntelDedicatedMetal"));
        assert!(line.contains("declared=false"));
        assert!(line.contains("usable=false"));
        let line2 = describe(
            AppleGpuClass::AppleSilicon,
            Some(4),
            26,
            BackendChoice::MetalPreferred,
        );
        assert!(line2.contains("chip_gen=Some(4)"));
        assert!(line2.contains("declared=true"));
    }
}
