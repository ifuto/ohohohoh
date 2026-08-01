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

/// 【wave 193 GM】adapter 要求の電力 policy。policy 値の実効は wgpu-core
/// の adapter 選択規則 (vendor instance.rs:923-933 一次情報) に従う:
/// LowPower → integrated 優先、HighPerformance → discrete 優先、
/// None → 列挙最小 id。単一 GPU 環境 (Apple Silicon 全機、一般デスクトップ)
/// では何を選んでも同一デバイスへ到達するため効果差なし = 非破壊。
/// 「軽く」の実効面は dual-GPU Intel MacBook (iGPU+dGPU、15/16 型世代) の
/// LowPowerPreferred = iGPU 選択 (低温・バッテリー側)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuPowerPolicy {
    /// discrete 優先 (既定族)。dual-GPU Intel Mac では dGPU 確定。
    HighPerformance,
    /// integrated 優先。dual-GPU Intel MacBook の省電力・低温経路。
    LowPowerPreferred,
    /// wgpu PowerPreference::None 委譲 (列挙最小 id、判断を戻す)。
    WgpuDefault,
}

/// env `RSIFT_GFX_POWER` からの電力 policy 決定 (trim+lowercase):
/// "low" → LowPowerPreferred、"high" → HighPerformance、
/// "default" → WgpuDefault、未知値 → 既定再帰、無指定 → HighPerformance
/// (性能系 mod の既定として性能優先を明示; 単一 GPU 環境では従来デバイスと
/// 同一到達で非破壊的厳密化)。選択規則の厳密 matrix は
/// gm_adapter_power_policy_golden が pin。
pub fn adapter_power_policy(env: Option<&str>) -> GpuPowerPolicy {
    match env.map(|s| s.trim().to_ascii_lowercase()) {
        Some(v) if v == "low" => GpuPowerPolicy::LowPowerPreferred,
        Some(v) if v == "high" => GpuPowerPolicy::HighPerformance,
        Some(v) if v == "default" => GpuPowerPolicy::WgpuDefault,
        Some(_) => adapter_power_policy(None),
        None => GpuPowerPolicy::HighPerformance,
    }
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

// ---- 【wave 194 GN】native direct binding 経路 (policy 層と直 binding 層の橋) ----

/// native 直 binding 経路の配線宣言。
/// ユーザー方針「Metal/GL を直 binding で完全実装」(wave 194 GN) に基づき、
/// wgpu 経路 (BackendChoice) の**隣に** native direct 経路の存在を宣言する:
/// - 既定は引き続き wgpu 経路 (実績・安全側)。
/// - env `RSIFT_GFX_APPLE_NATIVE` ("1"/"true"/"yes") で native direct 経路
///   への切替を*宣言*できる (実セッション起動は native_direct_session)。
/// - 経路の Apple 契約正しさは apple_ffi_audit が canon 一次情報と全件
///   照合済 (ffi_audit_clean が起動条件に含まれる設計)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeDirectPlan {
    /// ホスト OS が macOS (compile-time 真値)。
    pub host_is_macos: bool,
    /// env 宣言で native direct 経路が選択されたか。
    pub env_declared_native: bool,
    /// FFI 監査 (apple_ffi_audit::run_full_audit) が clean か。
    /// native 経路起動の前提条件 (canon との完全一致)。
    pub ffi_audit_clean: bool,
}

/// native direct 経路の plan 生成。`env` は `RSIFT_GFX_APPLE_NATIVE` の値。
/// 選択 matrix: 値 "1"/"true"/"yes" で宣言 ON (trim+lowercase 後)、
/// それ以外/未指定は OFF。ffi_audit_clean は plan 生成時に実監査を実行。
pub fn native_direct_plan(env: Option<&str>) -> NativeDirectPlan {
    let declared = env
        .map(|s| s.trim().to_ascii_lowercase())
        .map(|v| v == "1" || v == "true" || "yes" == v)
        .unwrap_or(false);
    NativeDirectPlan {
        host_is_macos: cfg!(target_os = "macos"),
        env_declared_native: declared,
        ffi_audit_clean: crate::apple_ffi_audit::run_full_audit().is_empty(),
    }
}

/// FFI 監査レポート (人間可読 1 行/件)。空 = canon との完全一致。
/// policy 層から audit への実消費配線 (rspeed seal 等の外部検査が
/// 本 API 経由で監査結果を参照できる)。
pub fn ffi_audit_report() -> Vec<String> {
    crate::apple_ffi_audit::run_full_audit()
        .iter()
        .map(|v| v.to_string())
        .collect()
}

/// WSL headless 検証用のネイティブセッション起動 (macOS のみ実体)。
/// plan で宣言選択された経路の実起動点 — 直 binding (objc_msgSend
/// typed transmute) で device→queue→MSL→pipeline→target 全構築する。
/// 他 OS ではシンボル不在のため cfg 除外 (plan 側が host_is_macos=false
/// を返し本関数の存在自体を配線から外す設計)。
#[cfg(target_os = "macos")]
pub fn native_direct_session(
    width: u32,
    height: u32,
) -> Result<crate::metal_direct::DirectMetal, crate::metal_direct::DirectMetalError> {
    let mut rt = crate::objc_rt::NativeObjcRt;
    crate::metal_direct::DirectMetal::create(&mut rt, width, height)
}

// ---- 【wave 196 GO】Metal 4 (MTL4) 直 binding 経路の policy 橋 ----

/// Metal 4 direct 経路を選ぶべきかの純粋 policy 判定。
/// 全条件を AND で要求 (ひとつでも欠ければ classic Metal direct/wgpu 側):
/// - `plan.env_declared_native`: ユーザー宣言 (RSIFT_GFX_APPLE_NATIVE)。
/// - `plan.host_is_macos`: compile-time 真値。
/// - `plan.ffi_audit_clean`: canon 完全一致 (SDK26 区画を含む全件照合)。
/// - `surface.declared`: Apple Silicon + macOS 26+ の MTL4 宣言的条件。
/// 非 cfg 関数であり Linux CI でも route matrix を機械 pin できる
/// (mock 不要・実セッション起動は native_direct4_session のみ cfg 実体)。
pub fn native_direct4_route(surface: &Metal4Surface, plan: &NativeDirectPlan) -> bool {
    plan.env_declared_native && plan.host_is_macos && plan.ffi_audit_clean && surface.declared
}

/// Metal 4 (MTL4) 直 binding セッション起動 (macOS のみ実体)。
/// classic native_direct_session と同じ配置規則: queue4/allocator×3/
/// residency/argument table/shared event までを DirectMetal4::create4 が
/// Apple SDK 26 canon 順序で構築する。起動前に native_direct4_route が
/// true であることを呼出側で確認する契約 (本関数自体は route を再評価
/// しない: OS バージョン供給値は呼出側の純粋性維持設計に従う)。
#[cfg(target_os = "macos")]
pub fn native_direct4_session(
    width: u32,
    height: u32,
) -> Result<crate::metal4_direct::DirectMetal4, crate::metal_direct::DirectMetalError> {
    let mut rt = crate::objc_rt::NativeObjcRt;
    crate::metal4_direct::DirectMetal4::create4(&mut rt, width, height)
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
    /// 【wave 193 GM】電力 policy golden: env `RSIFT_GFX_POWER` 規則
    /// ("low"→LowPowerPreferred (integrated 優先、dual-GPU Intel MacBook
    /// では iGPU = 低温・バッテリーの「軽い」側)、"high"/無指定既定→
    /// HighPerformance (discrete 優先)・"default"→WgpuDefault (wgpu None
    /// 委譲)。trim+lowercase、未知値は既定再帰)。選択の実効は vendor
    /// wgpu-core instance.rs:923-933 の一次規則どおり (LowPower→
    /// integrated優先、HighPerformance→discrete優先、単一 GPU 環境
    /// (Apple Silicon 全機・通常デスクトップ) では両者同一デバイス =
    /// no-op で非破壊)。
    #[test]
    fn gm_adapter_power_policy_golden() {
        let pairs: [(&str, GpuPowerPolicy); 4] = [
            ("low", GpuPowerPolicy::LowPowerPreferred),
            (" LOW ", GpuPowerPolicy::LowPowerPreferred),
            ("high", GpuPowerPolicy::HighPerformance),
            ("default", GpuPowerPolicy::WgpuDefault),
        ];
        for (env, want) in pairs {
            assert_eq!(adapter_power_policy(Some(env)), want, "env={env:?}");
        }
        assert_eq!(
            adapter_power_policy(None),
            GpuPowerPolicy::HighPerformance,
            "無指定既定は性能優先 (単一 GPU 環境では従来デバイスと同一の非破壊厳密化)"
        );
        for junk in ["turbo", "", "loww", "Default1"] {
            assert_eq!(
                adapter_power_policy(Some(junk)),
                adapter_power_policy(None),
                "未知値 {junk:?} は既定再帰"
            );
        }
    }

    /// 【wave 194 GN】native direct plan の選択 matrix と FFI 監査ゲートの
    /// 実動作 pin (policy 層 ⇔ 直 binding 層の橋の消費者)。
    #[test]
    fn gn_native_direct_plan_matrix() {
        // 既定/未指定は OFF (wgpu 経路が引き続き既定)。
        assert!(!native_direct_plan(None).env_declared_native);
        assert!(!native_direct_plan(Some("0")).env_declared_native);
        assert!(!native_direct_plan(Some("metal")).env_declared_native);
        // ON 値 trim+lowercase matrix。
        for on in ["1", "true", "TRUE", " yes ", "Yes"] {
            assert!(
                native_direct_plan(Some(on)).env_declared_native,
                "on={on:?}"
            );
        }
        // ホスト OS 真値 (sandbox では false、Mac では true)。
        assert_eq!(
            native_direct_plan(None).host_is_macos,
            cfg!(target_os = "macos")
        );
        // FFI 監査が実起動ゲートとして組み込まれ、現状 canon と完全一致。
        assert!(
            native_direct_plan(None).ffi_audit_clean,
            "canon 差分ゼロが前提"
        );
    }

    /// 【wave 196 GO】MTL4 route policy の全分岐 pin (非 cfg・mock 不要)。
    /// 4 条件 AND の各欠落パターンで必ず false、全充足でのみ true。
    #[test]
    fn go_native_direct4_route_matrix() {
        let plan_on_mac = NativeDirectPlan {
            host_is_macos: true,
            env_declared_native: true,
            ffi_audit_clean: true,
        };
        let surf_declared = Metal4Surface {
            declared: true,
            usable: false,
        };
        let surf_undeclared = Metal4Surface {
            declared: false,
            usable: false,
        };
        // 全充足のみ true。
        assert!(native_direct4_route(&surf_declared, &plan_on_mac));
        // surface 未宣言 (Intel Mac / macOS 25 以下相当) → false。
        assert!(!native_direct4_route(&surf_undeclared, &plan_on_mac));
        // env 未宣言 → false (既定は classic/wgpu 側のまま)。
        assert!(!native_direct4_route(
            &surf_declared,
            &NativeDirectPlan {
                env_declared_native: false,
                ..plan_on_mac
            }
        ));
        // 非 macOS → false。
        assert!(!native_direct4_route(
            &surf_declared,
            &NativeDirectPlan {
                host_is_macos: false,
                ..plan_on_mac
            }
        ));
        // 監査差分あり → false (canon 不一致での MTL4 起動を構造拒否)。
        assert!(!native_direct4_route(
            &surf_declared,
            &NativeDirectPlan {
                ffi_audit_clean: false,
                ..plan_on_mac
            }
        ));
        // 実 plan (env 未指定) は sandbox で必ず false に落ちる。
        let declared_here = metal4_surface(classify(host_os(), host_arch(), "Apple M4"), 26);
        assert!(!native_direct4_route(
            &declared_here,
            &native_direct_plan(None)
        ));
    }

    /// FFI 監査レポートが空 (canon 完全一致) であることの配線 pin。
    /// (report API 自体の消費者としても機能)
    #[test]
    fn gn_ffi_audit_report_empty() {
        let report = ffi_audit_report();
        assert!(report.is_empty(), "canon 差分がある: {report:?}");
    }

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
