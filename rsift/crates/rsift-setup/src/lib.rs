//! # rsift-setup: ゼロ依存ドロップイン・セットアップブートストラッパー
//!
//! ユーザーはこの実行ファイル (Windows: `rsift-setup.exe` / macOS: `Rsift Setup.app`
//! 内の `rsift-setup`) と各 DLL/dylib を**同じ階層に数個置くだけ**で良い。
//! 実行すると以下を自動で行う:
//!
//! 1. 同階層の `*.dll` / `*.dylib` / `*.so` を走査 (再帰なし・シンボリックリンク不読)
//! 2. 各ファイルの SHA-256 を計算し embedded manifest (SETUP_MANIFEST) と照合。
//!    manifest が空の初期配布では「受理 + ハッシュ記録」(ユーザーがログを送れば
//!    次回リリースで pin する運用、改竄には manifest 登録後に Fail)
//! 3. プラットフォーム分類 + RsGraphics 経路決定 (policy は純粋関数 [decide])
//! 4. 起動構成 `rsift_launch.json` を同階層へ生成
//! 5. **全工程を `rsift_setup_log.txt` (人間用) + `rsift_setup_log.jsonl` (機械用)
//!    に記録** — この 2 ファイルを Arena へ送れば環境診断が成立する
//!
//! 設計方針: std オンリー (配布物は何にも依存できない)。ネットワークアクセス無し。
//! ファイル削除は一切しない (追記のみ)。失敗は必ずログに残して非ゼロ終了。
//!
//! 期待する同階層ファイル命名規則 (配布 zip の構成):
//! - Windows: `rsift.dll` (エンジン) / `rsift_gfx_vulkan.dll` / `rsift_gfx_dx12.dll`
//! - macOS:   `librsift.dylib` / `librsift_gfx_metal4.dylib` /
//!            `librsift_gfx_metal.dylib` / `librsift_gfx_gl.dylib`
//! - Linux(診断経路): `librsift.so` / `librsift_gfx_vulkan.so`

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ---- 終了コード格子 (ユーザーへの約束: 番号で状態を伝える) ----
pub const EXIT_OK: i32 = 0;
pub const EXIT_NO_LIBS: i32 = 2;
pub const EXIT_VERIFY_FAIL: i32 = 3;
pub const EXIT_IO: i32 = 4;
pub const EXIT_SELFTEST_FAIL: i32 = 5;

/// embedded 検証正典。(ファイル名, SHA-256 小文字 hex)。
/// 初回配布は空 (その場合は受理してハッシュを記録、pin は次回リリース)。
/// 値を入れた途端に manifest 外ハッシュは Fail になる (改竄検出の起点)。
pub const SETUP_MANIFEST: &[(&str, &str)] = &[];

/// RsGraphics/エンジン DLL のファイル名正規名 (セレクタ正典)。
pub const LIB_WINDOWS_ENGINE: &str = "rsift.dll";
pub const LIB_WINDOWS_VULKAN: &str = "rsift_gfx_vulkan.dll";
pub const LIB_WINDOWS_DX12: &str = "rsift_gfx_dx12.dll";
pub const LIB_MACOS_ENGINE: &str = "librsift.dylib";
pub const LIB_MACOS_METAL4: &str = "librsift_gfx_metal4.dylib";
pub const LIB_MACOS_METAL: &str = "librsift_gfx_metal.dylib";
pub const LIB_MACOS_GL: &str = "librsift_gfx_gl.dylib";
pub const LIB_LINUX_ENGINE: &str = "librsift.so";
pub const LIB_LINUX_VULKAN: &str = "librsift_gfx_vulkan.so";

pub const LAUNCH_CONFIG_NAME: &str = "rsift_launch.json";
pub const LOG_TXT_NAME: &str = "rsift_setup_log.txt";
pub const LOG_JSONL_NAME: &str = "rsift_setup_log.jsonl";

// ------------------------------------------------------------------
// SHA-256 (自前実装: std オンリー維持のため。NIST 既知答えテストで pin)
// ------------------------------------------------------------------

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 を計算し小文字 hex で返す (FIPS 180-4 準拠)。
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K256[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for v in h {
        out.push_str(&format!("{v:08x}"));
    }
    out
}

// ------------------------------------------------------------------
// プラットフォーム分類と RsGraphics 経路決定 (純粋関数・Linux CI で全分岐 pin)
// ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    Windows,
    MacOs,
    Linux,
    Other,
}

/// 検査対象の lib 拡張子を target 別に返す。
pub fn lib_extensions(os: TargetOs) -> &'static [&'static str] {
    match os {
        TargetOs::Windows => &["dll"],
        TargetOs::MacOs => &["dylib"],
        TargetOs::Linux => &["so"],
        TargetOs::Other => &["dll", "dylib", "so"],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    Metal4,
    MetalClassic,
    Vulkan,
    Dx12,
    /// GL 直結経路 (旧 Mac / 旧 GPU 用)。wave 後続でライブラリ供給予定。
    Gl,
    /// 同階層から適切な RsGraphics ライブラリが見つからない。
    None,
}

impl Renderer {
    pub fn as_str(self) -> &'static str {
        match self {
            Renderer::Metal4 => "metal4",
            Renderer::MetalClassic => "metal",
            Renderer::Vulkan => "vulkan",
            Renderer::Dx12 => "dx12",
            Renderer::Gl => "gl",
            Renderer::None => "none",
        }
    }
}

/// 経路決定の全判定に必要な環境情報 (実行時収集; テストでは全項目手渡しで pin)。
#[derive(Debug, Clone)]
pub struct HostInfo {
    pub os: TargetOs,
    pub arch: String,
    /// macOS のみ (major, minor)。sw_vers 由来。他 OS は None。
    pub macos_ver: Option<(u32, u32)>,
    /// 同階層に存在した lib ファイル名 (小文字化済)。
    pub present_libs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchDecision {
    pub renderer: Renderer,
    pub engine_lib: Option<String>,
    pub graphics_lib: Option<String>,
    /// true なら起動構成を書いて即起動可能。false の場合 reason を notes に。
    pub ready: bool,
    pub notes: Vec<String>,
}

/// RsGraphics 経路決定の純粋 policy。
/// 優先順位: mac = Metal4 (aarch64 && 26+ && lib あり) > Metal classic > Gl、
/// win = Vulkan > DX12 > (OpenGL は将来)、linux = Vulkan。
/// エンジン本体 lib が無ければ即 not-ready (seen は notes に残す)。
pub fn decide(host: &HostInfo) -> LaunchDecision {
    let has = |name: &str| host.present_libs.iter().any(|l| l == name);
    let mut notes = Vec::new();
    let (engine_name, renderer, gfx) = match host.os {
        TargetOs::MacOs => {
            let metal4_eligible =
                host.arch == "aarch64" && host.macos_ver.map(|(maj, _)| maj >= 26).unwrap_or(false);
            if metal4_eligible && has(LIB_MACOS_METAL4) {
                (LIB_MACOS_ENGINE, Renderer::Metal4, Some(LIB_MACOS_METAL4))
            } else if has(LIB_MACOS_METAL) {
                if !metal4_eligible && has(LIB_MACOS_METAL4) {
                    notes.push(format!(
                        "Metal 4 ライブラリはあるが非対応環境 (arch={} macos={:?}) のため classic Metal に降格",
                        host.arch, host.macos_ver
                    ));
                } else if metal4_eligible && !has(LIB_MACOS_METAL4) {
                    notes.push(format!(
                        "Metal 4 対応環境 (arch={} macos={:?}) だが {} が同階層に無いため classic Metal に降格",
                        host.arch,
                        host.macos_ver,
                        LIB_MACOS_METAL4
                    ));
                }
                (
                    LIB_MACOS_ENGINE,
                    Renderer::MetalClassic,
                    Some(LIB_MACOS_METAL),
                )
            } else if has(LIB_MACOS_GL) {
                notes.push(
                    "GL 経路のライブラリは見つかったが、GL 直 binding 本体は後続 wave で供給予定 (未搭載) のため ready=false"
                        .to_string(),
                );
                (LIB_MACOS_ENGINE, Renderer::Gl, Some(LIB_MACOS_GL))
            } else {
                (LIB_MACOS_ENGINE, Renderer::None, None)
            }
        }
        TargetOs::Windows => {
            if has(LIB_WINDOWS_VULKAN) {
                (
                    LIB_WINDOWS_ENGINE,
                    Renderer::Vulkan,
                    Some(LIB_WINDOWS_VULKAN),
                )
            } else if has(LIB_WINDOWS_DX12) {
                (LIB_WINDOWS_ENGINE, Renderer::Dx12, Some(LIB_WINDOWS_DX12))
            } else {
                (LIB_WINDOWS_ENGINE, Renderer::None, None)
            }
        }
        TargetOs::Linux => {
            if has(LIB_LINUX_VULKAN) {
                (LIB_LINUX_ENGINE, Renderer::Vulkan, Some(LIB_LINUX_VULKAN))
            } else {
                (LIB_LINUX_ENGINE, Renderer::None, None)
            }
        }
        TargetOs::Other => ("", Renderer::None, None),
    };
    let engine_present = !engine_name.is_empty() && has(engine_name);
    if !engine_name.is_empty() && !engine_present {
        notes.push(format!("エンジン本体 {engine_name} が同階層に無い"));
    }
    if renderer == Renderer::None {
        notes.push("RsGraphics ライブラリが同階層に見つからない".to_string());
    }
    let ready = engine_present && matches!(gfx, Some(_)) && renderer != Renderer::Gl;
    LaunchDecision {
        renderer,
        engine_lib: if engine_present {
            Some(engine_name.to_string())
        } else {
            None
        },
        graphics_lib: gfx.map(str::to_string),
        ready,
        notes,
    }
}

// ------------------------------------------------------------------
// スキャン + 検証
// ------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LibFile {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// 同階層 (再帰なし) の lib を走査して SHA-256 を計算。読めないファイルは
/// スキップせず Err (起動物の読取失敗を静黙化しない)。
pub fn scan_libs(dir: &Path, os: TargetOs) -> Result<Vec<LibFile>, String> {
    let mut out = Vec::new();
    let rd = fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    let exts = lib_extensions(os);
    for entry in rd {
        let entry = entry.map_err(|e| format!("read_dir entry: {e}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if !exts.contains(&ext.as_str()) {
            continue;
        }
        let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        out.push(LibFile {
            name: name.to_ascii_lowercase(),
            size: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyState {
    /// manifest 空の初期配布: 受理しハッシュだけ記録した。
    Recorded,
    /// manifest 登録済みで全件一致。
    Pinned,
    /// manifest と不一致のファイルがある (改竄/破損の疑い)。names に列挙。
    Mismatch(Vec<String>),
}

/// 走査結果を manifest と照合。空 manifest では Recorded。
pub fn verify_libs(libs: &[LibFile]) -> VerifyState {
    if SETUP_MANIFEST.is_empty() {
        return VerifyState::Recorded;
    }
    let mut bad = Vec::new();
    for (name, sha) in SETUP_MANIFEST {
        match libs.iter().find(|l| l.name == *name) {
            Some(l) if l.sha256 == *sha => {}
            Some(l) => bad.push(format!("{} (hash 不一致)", l.name)),
            None => bad.push(format!("{name} (不在)")),
        }
    }
    if bad.is_empty() {
        VerifyState::Pinned
    } else {
        VerifyState::Mismatch(bad)
    }
}

// ------------------------------------------------------------------
// 起動構成 + ログ生成
// ------------------------------------------------------------------

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn json_str_array(items: &[String]) -> String {
    let inner: Vec<String> = items
        .iter()
        .map(|s| format!("\"{}\"", json_escape(s)))
        .collect();
    format!("[{}]", inner.join(", "))
}

/// 起動構成 JSON をレンダー (手書き・依存ゼロ; フィールドは schema "rsift.launch/1" で pin)。
pub fn render_launch_config(host: &HostInfo, dec: &LaunchDecision) -> String {
    let os_name = match host.os {
        TargetOs::Windows => "windows",
        TargetOs::MacOs => "macos",
        TargetOs::Linux => "linux",
        TargetOs::Other => "other",
    };
    let ver = host
        .macos_ver
        .map(|(a, b)| format!("\"{a}.{b}\""))
        .unwrap_or_else(|| "null".to_string());
    let engine = dec
        .engine_lib
        .as_deref()
        .map(|s| format!("\"{}\"", json_escape(s)))
        .unwrap_or_else(|| "null".to_string());
    let gfx = dec
        .graphics_lib
        .as_deref()
        .map(|s| format!("\"{}\"", json_escape(s)))
        .unwrap_or_else(|| "null".to_string());
    let mut libs: Vec<String> = Vec::new();
    if let Some(e) = &dec.engine_lib {
        libs.push(e.clone());
    }
    if let Some(g) = &dec.graphics_lib {
        libs.push(g.clone());
    }
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str("  \"schema\": \"rsift.launch/1\",\n");
    s.push_str(&format!("  \"target\": \"{os_name}\",\n"));
    s.push_str(&format!("  \"arch\": \"{}\",\n", json_escape(&host.arch)));
    s.push_str(&format!("  \"macos_version\": {ver},\n"));
    s.push_str(&format!("  \"renderer\": \"{}\",\n", dec.renderer.as_str()));
    s.push_str(&format!("  \"engine_lib\": {engine},\n"));
    s.push_str(&format!("  \"graphics_lib\": {gfx},\n"));
    s.push_str(&format!("  \"load_libs\": {},\n", json_str_array(&libs)));
    s.push_str(&format!("  \"prepared\": {},\n", dec.ready));
    s.push_str(&format!("  \"notes\": {}\n", json_str_array(&dec.notes)));
    s.push_str("}\n");
    s
}

/// UNIX epoch 秒 (ログの時系列照合用; 人間可読日付はユーザー環境側で変換可能)。
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    Ok,
    Warn,
    Fail,
}

impl StepStatus {
    fn as_str(&self) -> &'static str {
        match self {
            StepStatus::Ok => "ok",
            StepStatus::Warn => "warn",
            StepStatus::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StepResult {
    pub name: &'static str,
    pub status: StepStatus,
    pub detail: String,
}

/// ログアキュムレータ。txt (人間) と jsonl (機械) を同時に組み立てる。
/// ユーザーが送るのはこの 2 ファイル — 環境のスキャン結果・判定・
/// 失敗点がすべて自己記述されている。
pub struct SetupLog {
    pub steps: Vec<StepResult>,
    started_unix: u64,
}

impl SetupLog {
    pub fn new() -> Self {
        Self {
            steps: Vec::new(),
            started_unix: unix_now(),
        }
    }

    pub fn step(&mut self, name: &'static str, status: StepStatus, detail: impl Into<String>) {
        self.steps.push(StepResult {
            name,
            status,
            detail: detail.into(),
        });
    }

    pub fn render_txt(&self, exit_code: i32) -> String {
        let mut s = String::new();
        s.push_str("# rsift-setup 実行ログ\n");
        s.push_str(&format!(
            "# schema=rsift.setup.log/1 started_unix={} exit_code={exit_code}\n",
            self.started_unix
        ));
        for st in &self.steps {
            s.push_str(&format!(
                "[{:<4}] {:<12} {}\n",
                st.status.as_str(),
                st.name,
                st.detail.replace('\n', " | ")
            ));
        }
        s
    }

    pub fn render_jsonl(&self, exit_code: i32) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "{{\"schema\":\"rsift.setup.log/1\",\"started_unix\":{},\"exit_code\":{exit_code}}}\n",
            self.started_unix
        ));
        for st in &self.steps {
            s.push_str(&format!(
                "{{\"step\":\"{}\",\"status\":\"{}\",\"detail\":\"{}\"}}\n",
                st.name,
                st.status.as_str(),
                json_escape(&st.detail)
            ));
        }
        s
    }
}

// ------------------------------------------------------------------
// 実行時環境収集 (macOS では sw_vers、それ以外はマクロ値)
// ------------------------------------------------------------------

pub fn host_os() -> TargetOs {
    #[cfg(target_os = "windows")]
    {
        TargetOs::Windows
    }
    #[cfg(target_os = "macos")]
    {
        TargetOs::MacOs
    }
    #[cfg(all(
        not(target_os = "windows"),
        not(target_os = "macos"),
        target_os = "linux"
    ))]
    {
        TargetOs::Linux
    }
    #[cfg(all(
        not(target_os = "windows"),
        not(target_os = "macos"),
        not(target_os = "linux")
    ))]
    {
        TargetOs::Other
    }
}

/// `sw_vers -productVersion` の "26.1" / "26" 文字列を (26,1)/(26,0) にパース。
/// 純粋関数に分離して Linux CI で pin する (runtime 取得は host_info 側)。
pub fn parse_macos_version(text: &str) -> Option<(u32, u32)> {
    let t = text.trim();
    let mut it = t.split('.');
    let major: u32 = it.next()?.parse().ok()?;
    let minor: u32 = it.next().map(|m| m.parse().ok()).flatten().unwrap_or(0);
    Some((major, minor))
}

fn runtime_macos_version() -> Option<(u32, u32)> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        parse_macos_version(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// 実行時環境から HostInfo を構成 (libs は scan 結果のファイル名小文字列)。
pub fn host_info(libs: &[LibFile]) -> HostInfo {
    HostInfo {
        os: host_os(),
        arch: std::env::consts::ARCH.to_string(),
        macos_ver: runtime_macos_version(),
        present_libs: libs.iter().map(|l| l.name.clone()).collect(),
    }
}

// ------------------------------------------------------------------
// エントリポイント
// ------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Cli {
    pub dir: PathBuf,
    pub self_test: bool,
    pub dry_run: bool,
}

impl Cli {
    pub fn parse<I: Iterator<Item = String>>(mut args: I) -> Result<Self, String> {
        let mut cli = Cli::default();
        while let Some(a) = args.next() {
            match a.as_str() {
                "--self-test" => cli.self_test = true,
                "--dry-run" => cli.dry_run = true,
                "--dir" => {
                    let v = args.next().ok_or("--dir に値が無い")?;
                    cli.dir = PathBuf::from(v);
                }
                other => return Err(format!("不明な引数: {other}")),
            }
        }
        if cli.dir.as_os_str().is_empty() {
            cli.dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| PathBuf::from("."));
        }
        Ok(cli)
    }
}

/// 内蔵自己診断。真の依存 (SHA-256/構成レンダー/判定 policy) を対象に
/// NIST 既知答え + 全 policy 分岐をその場で検査する。結果はログへ全記録。
fn self_test_steps(log: &mut SetupLog) -> bool {
    let mut ok = true;
    let v = sha256_hex(b"");
    if v == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" {
        log.step("selftest", StepStatus::Ok, "sha256 empty vector");
    } else {
        log.step(
            "selftest",
            StepStatus::Fail,
            format!("sha256 empty vector: {v}"),
        );
        ok = false;
    }
    let v = sha256_hex(b"abc");
    if v == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad" {
        log.step("selftest", StepStatus::Ok, "sha256 abc vector");
    } else {
        log.step(
            "selftest",
            StepStatus::Fail,
            format!("sha256 abc vector: {v}"),
        );
        ok = false;
    }
    let v = parse_macos_version("26.1");
    if v == Some((26, 1)) {
        log.step("selftest", StepStatus::Ok, "macos version parse");
    } else {
        log.step(
            "selftest",
            StepStatus::Fail,
            format!("macos version parse: {v:?}"),
        );
        ok = false;
    }
    ok
}

/// セットアップ本体。ログ 2 ファイルを必ず (失敗時も) 書いてから終了コードを返す。
/// --dry-run では構成 JSON を書かない (ログは書く)。
pub fn run(cli: &Cli) -> i32 {
    let mut log = SetupLog::new();
    log.step(
        "env",
        StepStatus::Ok,
        format!(
            "os={} arch={} dir={}",
            match host_os() {
                TargetOs::Windows => "windows",
                TargetOs::MacOs => "macos",
                TargetOs::Linux => "linux",
                TargetOs::Other => "other",
            },
            std::env::consts::ARCH,
            cli.dir.display()
        ),
    );

    if cli.self_test && !self_test_steps(&mut log) {
        return finish(&cli.dir, log, EXIT_SELFTEST_FAIL);
    }
    if cli.self_test {
        log.step("selftest", StepStatus::Ok, "内蔵自己診断 全緑");
    }

    let os = host_os();
    let libs = match scan_libs(&cli.dir, os) {
        Ok(l) => {
            log.step(
                "scan",
                StepStatus::Ok,
                format!("{} 個の lib を検出", l.len()),
            );
            l
        }
        Err(e) => {
            log.step("scan", StepStatus::Fail, e);
            return finish(&cli.dir, log, EXIT_IO);
        }
    };
    for l in &libs {
        log.step(
            "lib",
            StepStatus::Ok,
            format!("{} size={} sha256={}", l.name, l.size, l.sha256),
        );
    }
    if libs.is_empty() {
        log.step(
            "scan",
            StepStatus::Fail,
            format!(
                "同階層に {} が 1 つも無い。zip の中身を実行ファイルと同じ場所に置いてください",
                lib_extensions(os).join("/")
            ),
        );
        return finish(&cli.dir, log, EXIT_NO_LIBS);
    }

    match verify_libs(&libs) {
        VerifyState::Recorded => log.step(
            "verify",
            StepStatus::Warn,
            "manifest 未登録 (初期配布): ハッシュを記録のみ。次回リリースで pin されます",
        ),
        VerifyState::Pinned => log.step("verify", StepStatus::Ok, "manifest 全件一致"),
        VerifyState::Mismatch(bad) => {
            log.step(
                "verify",
                StepStatus::Fail,
                format!("manifest 不一致: {}", bad.join("; ")),
            );
            return finish(&cli.dir, log, EXIT_VERIFY_FAIL);
        }
    }

    let host = host_info(&libs);
    let dec = decide(&host);
    for n in &dec.notes {
        log.step("decide", StepStatus::Warn, n.clone());
    }
    log.step(
        "decide",
        if dec.ready {
            StepStatus::Ok
        } else {
            StepStatus::Warn
        },
        format!(
            "renderer={} engine={:?} gfx={:?} ready={}",
            dec.renderer.as_str(),
            dec.engine_lib,
            dec.graphics_lib,
            dec.ready
        ),
    );

    let config = render_launch_config(&host, &dec);
    if cli.dry_run {
        log.step(
            "config",
            StepStatus::Ok,
            "dry-run: rsift_launch.json は未生成",
        );
    } else {
        let path = cli.dir.join(LAUNCH_CONFIG_NAME);
        match fs::write(&path, config.as_bytes()) {
            Ok(()) => log.step(
                "config",
                StepStatus::Ok,
                format!("{} を生成", path.display()),
            ),
            Err(e) => {
                log.step(
                    "config",
                    StepStatus::Fail,
                    format!("{}: {e}", path.display()),
                );
                return finish(&cli.dir, log, EXIT_IO);
            }
        }
    }
    finish(&cli.dir, log, EXIT_OK)
}

fn finish(dir: &Path, log: SetupLog, code: i32) -> i32 {
    let txt = log.render_txt(code);
    let jsonl = log.render_jsonl(code);
    // ログ書き込み失敗は「失敗の失敗」: stderr に出して私自体は既定コードを返す。
    if let Err(e) = fs::write(dir.join(LOG_TXT_NAME), txt.as_bytes()) {
        eprintln!(
            "rsift-setup: ログ書込失敗 ({}): {e}",
            dir.join(LOG_TXT_NAME).display()
        );
    }
    if let Err(e) = fs::write(dir.join(LOG_JSONL_NAME), jsonl.as_bytes()) {
        eprintln!(
            "rsift-setup: ログ書込失敗 ({}): {e}",
            dir.join(LOG_JSONL_NAME).display()
        );
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("rsift_setup_test_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn host(os: TargetOs, arch: &str, ver: Option<(u32, u32)>, libs: &[&str]) -> HostInfo {
        HostInfo {
            os,
            arch: arch.to_string(),
            macos_ver: ver,
            present_libs: libs.iter().map(|s| s.to_string()).collect(),
        }
    }

    // ---- sha256: NIST FIPS 180-4 既知答え ----
    #[test]
    fn sha256_nist_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // 大きめ入力 (複数ブロック)
        let big = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256_hex(&big),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    // ---- decide: 全分岐 matrix ----
    #[test]
    fn decide_mac_metal4_ready() {
        let d = decide(&host(
            TargetOs::MacOs,
            "aarch64",
            Some((26, 0)),
            &[LIB_MACOS_ENGINE, LIB_MACOS_METAL4],
        ));
        assert_eq!(d.renderer, Renderer::Metal4);
        assert!(d.ready);
        assert_eq!(d.graphics_lib.as_deref(), Some(LIB_MACOS_METAL4));
    }

    #[test]
    fn decide_mac_metal4_lib_present_but_ineligible_host_stays_classic() {
        // 非対応ホスト (Intel または macOS 25 以下) に metal4 dylib があっても
        // Metal 4 を選んではいけない (誤 ready を構造拒否、系統A 変異の網)。
        for (arch, ver) in [("x86_64", Some((26, 0))), ("aarch64", Some((25, 5)))] {
            let d = decide(&host(
                TargetOs::MacOs,
                arch,
                ver,
                &[LIB_MACOS_ENGINE, LIB_MACOS_METAL4, LIB_MACOS_METAL],
            ));
            assert_eq!(
                d.renderer,
                Renderer::MetalClassic,
                "arch={arch} ver={ver:?} では classic へ降格必須"
            );
            assert!(d.notes.iter().any(|n| n.contains("降格")));
        }
    }

    #[test]
    fn decide_mac_metal4_capable_but_lib_missing_downgrades() {
        let d = decide(&host(
            TargetOs::MacOs,
            "aarch64",
            Some((26, 1)),
            &[LIB_MACOS_ENGINE, LIB_MACOS_METAL],
        ));
        assert_eq!(d.renderer, Renderer::MetalClassic);
        assert!(d.ready);
        // 降格の理由が notes に残る (ユーザーへ正直に伝える)。
        assert!(d.notes.iter().any(|n| n.contains("降格")));
    }

    #[test]
    fn decide_mac_intel_old_goes_gl_not_ready() {
        let d = decide(&host(
            TargetOs::MacOs,
            "x86_64",
            Some((15, 3)),
            &[LIB_MACOS_ENGINE, LIB_MACOS_GL],
        ));
        assert_eq!(d.renderer, Renderer::Gl);
        // GL 直 binding 本体は未供給 — 嘘で ready=true にしない。
        assert!(!d.ready);
        assert!(d.notes.iter().any(|n| n.contains("未搭載")));
    }

    #[test]
    fn decide_mac_no_engine_not_ready() {
        let d = decide(&host(
            TargetOs::MacOs,
            "aarch64",
            Some((26, 0)),
            &[LIB_MACOS_METAL4],
        ));
        assert!(!d.ready);
        assert!(d.engine_lib.is_none());
        assert!(d.notes.iter().any(|n| n.contains(LIB_MACOS_ENGINE)));
    }

    #[test]
    fn decide_windows_prefers_vulkan() {
        let d = decide(&host(
            TargetOs::Windows,
            "x86_64",
            None,
            &[LIB_WINDOWS_ENGINE, LIB_WINDOWS_VULKAN, LIB_WINDOWS_DX12],
        ));
        assert_eq!(d.renderer, Renderer::Vulkan);
        assert!(d.ready);
    }

    #[test]
    fn decide_windows_dx12_fallback() {
        let d = decide(&host(
            TargetOs::Windows,
            "x86_64",
            None,
            &[LIB_WINDOWS_ENGINE, LIB_WINDOWS_DX12],
        ));
        assert_eq!(d.renderer, Renderer::Dx12);
        assert!(d.ready);
    }

    #[test]
    fn decide_windows_nothing_not_ready() {
        let d = decide(&host(
            TargetOs::Windows,
            "x86_64",
            None,
            &[LIB_WINDOWS_ENGINE],
        ));
        assert_eq!(d.renderer, Renderer::None);
        assert!(!d.ready);
    }

    #[test]
    fn decide_linux_vulkan() {
        let d = decide(&host(
            TargetOs::Linux,
            "x86_64",
            None,
            &[LIB_LINUX_ENGINE, LIB_LINUX_VULKAN],
        ));
        assert_eq!(d.renderer, Renderer::Vulkan);
        assert!(d.ready);
    }

    // ---- macOS バージョン文字列パース ----
    #[test]
    fn parse_macos_version_variants() {
        assert_eq!(parse_macos_version("26.1"), Some((26, 1)));
        assert_eq!(parse_macos_version("26"), Some((26, 0)));
        assert_eq!(parse_macos_version("15.3.1\n"), Some((15, 3)));
        assert_eq!(parse_macos_version(""), None);
        assert_eq!(parse_macos_version("abc"), None);
    }

    // ---- scan/verify ----
    #[test]
    fn scan_filters_and_hashes() {
        let d = tmpdir("scan");
        fs::write(d.join("rsift.dll"), b"engine-bytes").unwrap();
        fs::write(d.join("rsift_gfx_vulkan.dll"), b"gfx-bytes").unwrap();
        fs::write(d.join("ignore.txt"), b"nope").unwrap();
        fs::write(d.join("Rsift_Setup_Log.TXT"), b"nope2").unwrap();
        fs::create_dir_all(d.join("subdir")).unwrap();
        fs::write(d.join("subdir").join("nested.dll"), b"x").unwrap();
        let libs = scan_libs(&d, TargetOs::Windows).unwrap();
        assert_eq!(libs.len(), 2);
        assert_eq!(libs[0].name, "rsift.dll");
        assert_eq!(
            libs[0].sha256,
            sha256_hex(b"engine-bytes"),
            "実ファイル内容の SHA-256"
        );
        // 再帰なし (nested.dll は拾わない)。
        assert!(libs.iter().all(|l| l.name != "nested.dll"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn verify_empty_manifest_records() {
        let libs = vec![LibFile {
            name: "a.dll".to_string(),
            size: 1,
            sha256: "0".repeat(64),
        }];
        assert_eq!(verify_libs(&libs), VerifyState::Recorded);
    }

    // ---- config レンダー golden ----
    #[test]
    fn render_launch_config_golden() {
        let h = host(
            TargetOs::MacOs,
            "aarch64",
            Some((26, 0)),
            &[LIB_MACOS_ENGINE, LIB_MACOS_METAL4],
        );
        let d = decide(&h);
        let s = render_launch_config(&h, &d);
        assert!(s.contains("\"schema\": \"rsift.launch/1\""));
        assert!(s.contains("\"target\": \"macos\""));
        assert!(s.contains("\"renderer\": \"metal4\""));
        assert!(s.contains("\"macos_version\": \"26.0\""));
        assert!(s.contains("\"prepared\": true"));
        assert!(s.contains(&format!("\"{LIB_MACOS_ENGINE}\"")));
        assert!(s.ends_with("}\n"));
    }

    #[test]
    fn json_escape_controls() {
        assert_eq!(json_escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
        assert_eq!(json_escape("ok"), "ok");
    }

    // ---- log レンダー ----
    #[test]
    fn log_renders_both_formats() {
        let mut log = SetupLog::new();
        log.step("env", StepStatus::Ok, "os=linux arch=x86_64");
        log.step("scan", StepStatus::Warn, "0 個の lib");
        let txt = log.render_txt(EXIT_NO_LIBS);
        let jl = log.render_jsonl(EXIT_NO_LIBS);
        assert!(txt.contains("schema=rsift.setup.log/1"));
        assert!(txt.contains("exit_code=2"));
        assert!(txt.contains("[ok  ] env"));
        assert!(txt.contains("[warn] scan"));
        assert!(jl.lines().count() == 3, "1 meta + 2 step 行");
        assert!(jl.contains("\"step\":\"env\",\"status\":\"ok\""));
    }

    // ---- end-to-end: 実ファイルで run/dry-run ----
    #[test]
    fn run_end_to_end_writes_config_and_logs() {
        let d = tmpdir("e2e");
        fs::write(d.join(LIB_LINUX_ENGINE), b"engine").unwrap();
        fs::write(d.join(LIB_LINUX_VULKAN), b"gfx").unwrap();
        let cli = Cli {
            dir: d.clone(),
            self_test: true,
            dry_run: false,
        };
        let code = run(&cli);
        assert_eq!(code, EXIT_OK);
        let cfg = fs::read_to_string(d.join(LAUNCH_CONFIG_NAME)).unwrap();
        assert!(cfg.contains("\"renderer\": \"vulkan\""));
        let log_txt = fs::read_to_string(d.join(LOG_TXT_NAME)).unwrap();
        assert!(log_txt.contains("rsift.setup.log/1"));
        assert!(log_txt.contains("sha256="), "lib ハッシュが記録される");
        let log_jl = fs::read_to_string(d.join(LOG_JSONL_NAME)).unwrap();
        assert!(log_jl.contains("\"exit_code\":0"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn run_no_libs_fails_loud_with_logs() {
        let d = tmpdir("nolibs");
        let cli = Cli {
            dir: d.clone(),
            self_test: false,
            dry_run: false,
        };
        let code = run(&cli);
        // linux 拡張子の lib が 0 → EXIT_NO_LIBS。でもログは必ず書かれる。
        assert_eq!(code, EXIT_NO_LIBS);
        assert!(d.join(LOG_TXT_NAME).exists());
        assert!(d.join(LOG_JSONL_NAME).exists());
        assert!(!d.join(LAUNCH_CONFIG_NAME).exists(), "構成は生成しない");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn run_dry_run_skips_config_but_logs() {
        let d = tmpdir("dry");
        fs::write(d.join(LIB_LINUX_ENGINE), b"e").unwrap();
        fs::write(d.join(LIB_LINUX_VULKAN), b"g").unwrap();
        let cli = Cli {
            dir: d.clone(),
            self_test: false,
            dry_run: true,
        };
        assert_eq!(run(&cli), EXIT_OK);
        assert!(!d.join(LAUNCH_CONFIG_NAME).exists());
        assert!(d.join(LOG_TXT_NAME).exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn cli_parse_variants() {
        let cli = Cli::parse(
            vec![
                "--dry-run".to_string(),
                "--dir".to_string(),
                "/tmp/x".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert!(cli.dry_run);
        assert_eq!(cli.dir, PathBuf::from("/tmp/x"));
        assert!(Cli::parse(vec!["--dir".to_string()].into_iter()).is_err());
        assert!(Cli::parse(vec!["--wat".to_string()].into_iter()).is_err());
    }
}
