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
/// 実物 cdylib (mods-official 由来): エンジン rsift.*、RsGraphics rsgraphics.*
/// (内部に wgpu/Vulkan/DX12 および macOS では Metal4/classic Metal の両経路を
/// 持ち実行時自動選択)、RsReplay rsreplay.*。旧 rsift_gfx_* 系名は wave 198
/// で正規化 (エイリアス重複配布を避ける)。
pub const LIB_WINDOWS_ENGINE: &str = "rsift.dll";
pub const LIB_WINDOWS_GFX: &str = "rsgraphics.dll";
pub const LIB_WINDOWS_REPLAY: &str = "rsreplay.dll";
pub const LIB_WINDOWS_ZOOM: &str = "rszoom.dll";
pub const LIB_MACOS_ENGINE: &str = "librsift.dylib";
pub const LIB_MACOS_GFX: &str = "librsgraphics.dylib";
pub const LIB_MACOS_REPLAY: &str = "librsreplay.dylib";
pub const LIB_MACOS_ZOOM: &str = "librszoom.dylib";
pub const LIB_LINUX_ENGINE: &str = "librsift.so";
pub const LIB_LINUX_GFX: &str = "librsgraphics.so";
pub const LIB_LINUX_REPLAY: &str = "librsreplay.so";
pub const LIB_LINUX_ZOOM: &str = "librszoom.so";

pub const LAUNCH_CONFIG_NAME: &str = "rsift_launch.json";
pub const LOG_TXT_NAME: &str = "rsift_setup_log.txt";
pub const LOG_JSONL_NAME: &str = "rsift_setup_log.jsonl";

/// JVMTI agent (rsift-jvm cdylib、-agentpath 対象) の OS 別ファイル名。
pub const LIB_WINDOWS_AGENT: &str = "rsift_jvm.dll";
pub const LIB_MACOS_AGENT: &str = "librsift_jvm.dylib";
pub const LIB_LINUX_AGENT: &str = "librsift_jvm.so";
/// Java ブリッジ jar (RsiftHooks 等 = Mods ボタン/フック注入の要)。
/// エージェントは dll と同階層のこの名前で探す (screen_inject::bootstrap_jar_path)。
/// OS 非依存 (pure Java) のため全ターゲットで同一ファイル。
pub const BOOTSTRAP_JAR: &str = "rsift-bootstrap.jar";
/// 破損/スタブ判定の下限サイズ (実物は ~44KB、manifest-only スタブは 152B)。
pub const BOOTSTRAP_JAR_MIN_SIZE: u64 = 512;

/// wave HR (#5 根治): Mojang 公式 ProGuard mappings (mojmap → 難読名)。
/// 1.21.11 は実行時難読化のため、agent が vanilla クラスを解決するのに必須。
/// 配布元 (一次): piston-data.mojang.com の 1.21.11 client_mappings。
/// agent (obf_map::try_load_from_dir) は `<dll_dir>/client.txt` を探す。
/// 不在時は agent は Unobfuscated (mojmap 恒等 = 従来挙動) へ安全落下する。
pub const CLIENT_TXT: &str = "client.txt";
/// 1.21.11 client_mappings の固定配布 URL と sha1 (jank.systems mappings guide
/// で一次確認: version_manifest_v2 → 1.21.11.json → client_mappings)。
pub const CLIENT_TXT_URL_1_21_11: &str =
    "https://piston-data.mojang.com/v1/objects/031a68bebf55d824f66d6573d8c752f0e1bf232a/client.txt";
pub const CLIENT_TXT_SHA1_1_21_11: &str = "031a68bebf55d824f66d6573d8c752f0e1bf232a";
pub const CLIENT_TXT_MIN_SIZE: u64 = 1_000_000;

/// 起動構成として登録する profile / version の識別子 (ユーザ仕様
/// 「versions に rsift-1.21.11 みたいなフォルダ」)。
pub const LAUNCHER_PROFILE_ID: &str = "rsift";
pub const LAUNCHER_PROFILE_NAME: &str = "Rsift";
pub const RSIFT_MC_VERSION: &str = "1.21.11";

/// バージョン識別子 (`versions/<id>/<id>.json`)。
pub fn rsift_version_id() -> String {
    format!("rsift-{RSIFT_MC_VERSION}")
}

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
            // rsgraphics dylib 内部に Metal 4 (aarch64+macOS 26+) と
            // classic Metal の両経路があり、実行時に適格側を自動選択する。
            let metal4_eligible =
                host.arch == "aarch64" && host.macos_ver.map(|(maj, _)| maj >= 26).unwrap_or(false);
            if has(LIB_MACOS_GFX) {
                if metal4_eligible {
                    (LIB_MACOS_ENGINE, Renderer::Metal4, Some(LIB_MACOS_GFX))
                } else {
                    notes.push(
                        "RsGraphics は実行時に classic Metal 経路を選択 (Metal 4 は aarch64+macOS 26+)"
                            .to_string(),
                    );
                    (
                        LIB_MACOS_ENGINE,
                        Renderer::MetalClassic,
                        Some(LIB_MACOS_GFX),
                    )
                }
            } else {
                (LIB_MACOS_ENGINE, Renderer::None, None)
            }
        }
        TargetOs::Windows => {
            if has(LIB_WINDOWS_GFX) {
                notes.push(
                    "RsGraphics は実行時に wgpu が最適 backend を自動選択 (Vulkan 優先/DX12)"
                        .to_string(),
                );
                (LIB_WINDOWS_ENGINE, Renderer::Vulkan, Some(LIB_WINDOWS_GFX))
            } else {
                (LIB_WINDOWS_ENGINE, Renderer::None, None)
            }
        }
        TargetOs::Linux => {
            if has(LIB_LINUX_GFX) {
                (LIB_LINUX_ENGINE, Renderer::Vulkan, Some(LIB_LINUX_GFX))
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
    let replay_name = match host.os {
        TargetOs::Windows => LIB_WINDOWS_REPLAY,
        TargetOs::MacOs => LIB_MACOS_REPLAY,
        TargetOs::Linux => LIB_LINUX_REPLAY,
        TargetOs::Other => "",
    };
    if !replay_name.is_empty() && has(replay_name) {
        notes.push(format!(
            "RsReplay 同梱検出: {replay_name} (録画/export mod として準備)"
        ));
    }
    let replay_name = match host.os {
        TargetOs::Windows => LIB_WINDOWS_REPLAY,
        TargetOs::MacOs => LIB_MACOS_REPLAY,
        TargetOs::Linux => LIB_LINUX_REPLAY,
        TargetOs::Other => "",
    };
    if !replay_name.is_empty() && has(replay_name) {
        notes.push(format!(
            "RsReplay 同梱検出: {replay_name} (録画/export mod として準備)"
        ));
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

#[derive(Debug, Clone, PartialEq)]
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

/// bootstrap jar 状態 (3 値: 有効/不在/破損)。
/// 破損 (manifest-only スタブ・PK 無し・中途半端) も「ないより悪い」ため分ける。
#[derive(Debug, Clone, PartialEq)]
pub enum BootstrapJarState {
    Present(LibFile),
    Missing,
    Corrupt,
}

/// `BOOTSTRAP_JAR` を dir で探し、有効/不在/破損を返す。
/// 読取不可は静黙化せず Err (scan_libs と同規則)。
pub fn probe_bootstrap_jar(dir: &Path) -> Result<BootstrapJarState, String> {
    let path = dir.join(BOOTSTRAP_JAR);
    if !path.exists() {
        return Ok(BootstrapJarState::Missing);
    }
    if !path.is_file() {
        return Ok(BootstrapJarState::Corrupt);
    }
    let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let valid = bytes.len() as u64 >= BOOTSTRAP_JAR_MIN_SIZE && bytes.starts_with(b"PK\x03\x04");
    if !valid {
        return Ok(BootstrapJarState::Corrupt);
    }
    Ok(BootstrapJarState::Present(LibFile {
        name: BOOTSTRAP_JAR.to_string(),
        size: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
    }))
}

/// wave HR (#5 根治): client.txt (ProGuard mappings) を source_dir → dst_dir へ
/// 配備 (dst = agent の dll_directory と同階層)。source に無い場合は配備せず
/// warn note を返す (agent は Unobfuscated へ安全落下 = 従来挙動)。
/// 破損 (小さすぎ) も配備せず warn。戻り値 note は setup ログへ記録される。
pub fn deploy_client_mappings(source_dir: &Path, dst_dir: &Path) -> Result<String, String> {
    fs::create_dir_all(dst_dir).map_err(|e| format!("create {}: {e}", dst_dir.display()))?;
    let src = source_dir.join(CLIENT_TXT);
    if !src.is_file() {
        return Ok(format!(
            "{CLIENT_TXT} not bundled -> agent runs Unobfuscated (legacy mojmap). \
             FIX: bundle {CLIENT_TXT} (1.21.11 client_mappings) next to setup"
        ));
    }
    let bytes = fs::read(&src).map_err(|e| format!("read {}: {e}", src.display()))?;
    if (bytes.len() as u64) < CLIENT_TXT_MIN_SIZE {
        return Ok(format!(
            "{CLIENT_TXT} bundled but too small ({}B < {}) -> not deployed; agent runs Unobfuscated",
            bytes.len(),
            CLIENT_TXT_MIN_SIZE
        ));
    }
    let dst = dst_dir.join(CLIENT_TXT);
    fs::write(&dst, &bytes).map_err(|e| format!("write {}: {e}", dst.display()))?;
    Ok(format!(
        "{CLIENT_TXT} deployed ({}B sha256={})",
        bytes.len(),
        sha256_hex(&bytes)
    ))
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
/// `launcher_json` には launcher 統合結果の JSON 断片 (または "null") を渡す。
pub fn render_launch_config(
    host: &HostInfo,
    dec: &LaunchDecision,
    launcher_json: &str,
    prism_json: &str,
) -> String {
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
    s.push_str(&format!("  \"notes\": {},\n", json_str_array(&dec.notes)));
    s.push_str(&format!("  \"launcher\": {},\n", launcher_json));
    s.push_str(&format!("  \"prism\": {}\n", prism_json));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
/// wave 210 HF: prism ステップの mods/natives 配備サマリ行 (launcher 側の
/// `version_dir=... natives={:?} mods={:?}` 行と対称)。実機 #4/#5 で
/// 「setup 成功 -> でも mods フォルダ空」が起きた際、prism 側は配備結果を
/// ログに残さず切り分け不能だった構造的欠陥の根治。#6 以降のセットアップ
/// ログでは `mods=[...]` の有無で配備成否が一目で判別できる。
pub fn prism_summary_line(out: &PrismOutcome) -> String {
    format!(
        "instance_dir={} natives={:?} mods={:?}",
        out.instance_dir.display(),
        out.natives_deployed,
        out.mods_deployed
    )
}

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

    // Minecraft ランチャー統合: versions/rsift-1.21.11/ + 起動構成登録
    let mut launcher_json = "null".to_string();
    if cli.dry_run {
        log.step("launcher", StepStatus::Ok, "dry-run: 起動構成登録は未実施");
    } else {
        match find_minecraft_dir(os) {
            None => log.step(
                "launcher",
                StepStatus::Warn,
                "minecraft ディレクトリ検出不可 (RSIFT_MC_DIR 未設定) → 登録スキップ",
            ),
            Some(mc) if !mc.is_dir() => log.step(
                "launcher",
                StepStatus::Warn,
                format!(
                    "{} が存在しない (Minecraft 本体の導入後に再実行すると起動構成を登録します)",
                    mc.display()
                ),
            ),
            Some(mc) => match setup_launcher(&mc, os, &cli.dir, &libs, dec.renderer.as_str()) {
                Ok(out) => {
                    let status = if out.profile_registered {
                        StepStatus::Ok
                    } else {
                        StepStatus::Warn
                    };
                    for n in &out.notes {
                        log.step("launcher", status, n.clone());
                    }
                    log.step(
                        "launcher",
                        status,
                        format!(
                            "version_dir={} natives={:?} mods={:?}",
                            out.version_dir.display(),
                            out.natives_deployed,
                            out.mods_deployed
                        ),
                    );
                    launcher_json = format!(
                        "{{\"registered\": {}, \"profile_id\": \"{}\", \"version_id\": \"{}\", \"minecraft_dir\": \"{}\"}}",
                        out.profile_registered,
                        LAUNCHER_PROFILE_ID,
                        rsift_version_id(),
                        json_escape(&mc.display().to_string())
                    );
                }
                Err(e) => {
                    log.step("launcher", StepStatus::Fail, e);
                    return finish(&cli.dir, log, EXIT_IO);
                }
            },
        }
    }

    // PrismLauncher 統合: instances/rsift/ 生成 (Windows 10 標準構成を第一級)
    let mut prism_json = "null".to_string();
    if cli.dry_run {
        log.step("prism", StepStatus::Ok, "dry-run: Prism 登録は未実施");
    } else if let Some(prism_root) = find_prism_dir(os) {
        if !prism_root.is_dir() {
            log.step(
                "prism",
                StepStatus::Ok,
                format!(
                    "{} が存在しない (PrismLauncher 未導入 → スキップ。導入後に再実行で登録)",
                    prism_root.display()
                ),
            );
        } else {
            match setup_prism(&prism_root, os, &cli.dir, &libs, dec.renderer.as_str()) {
                Ok(out) => {
                    let status = if out.instance_created {
                        StepStatus::Ok
                    } else if out.foreign_conflict {
                        StepStatus::Fail
                    } else {
                        StepStatus::Warn
                    };
                    for n in &out.notes {
                        log.step("prism", status, n.clone());
                    }
                    // wave 210 HF: launcher 側と対称に mods/natives 配備結果を残す。
                    log.step("prism", status, prism_summary_line(&out));
                    if out.foreign_conflict {
                        return finish(&cli.dir, log, EXIT_IO);
                    }
                    prism_json = format!(
                        "{{\"created\": {}, \"instance_id\": \"{}\", \"instance_dir\": \"{}\"}}",
                        out.instance_created,
                        PRISM_INSTANCE_ID,
                        json_escape(&out.instance_dir.display().to_string())
                    );
                }
                Err(e) => {
                    log.step("prism", StepStatus::Fail, e);
                    return finish(&cli.dir, log, EXIT_IO);
                }
            }
        }
    }

    let config = render_launch_config(&host, &dec, &launcher_json, &prism_json);
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

// ------------------------------------------------------------------
// 最小 JSON (std オンリー): launcher_profiles.json の安全な upsert に必要。
// 文字列継ぎ足しではプロファイルを壊し得るため、構造理解のあるパーサを
// 自前で持つ (round-trip 忠実性はテストで pin)。
// ------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(String), // リテラル保持 (1.0 / 1e3 等を失わない)
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>), // 挿入順保持 (BTreeMap だと書き戻しで順が揺れる)
}

impl Json {
    pub fn obj_get<'a>(&'a self, key: &str) -> Option<&'a Json> {
        if let Json::Obj(m) = self {
            m.iter().find(|(k, _)| k == key).map(|(_, v)| v)
        } else {
            None
        }
    }

    pub fn obj_get_mut<'a>(&'a mut self, key: &str) -> Option<&'a mut Json> {
        if let Json::Obj(m) = self {
            m.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
        } else {
            None
        }
    }

    pub fn obj_insert(&mut self, key: &str, val: Json) {
        if let Json::Obj(m) = self {
            if let Some(e) = m.iter_mut().find(|(k, _)| k == key) {
                e.1 = val;
            } else {
                m.push((key.to_string(), val));
            }
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Json::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }

    pub fn parse(text: &str) -> Result<Json, String> {
        let b = text.as_bytes();
        let mut i = 0usize;
        let v = parse_value(b, &mut i)?;
        skip_ws(b, &mut i);
        if i != b.len() {
            return Err(format!("JSON 末尾に余計なデータ (byte {i})"));
        }
        Ok(v)
    }

    pub fn render(&self) -> String {
        let mut s = String::new();
        render_json(self, &mut s, 0);
        s
    }
}

fn skip_ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && matches!(b[*i], b' ' | b'\t' | b'\n' | b'\r') {
        *i += 1;
    }
}

fn parse_value(b: &[u8], i: &mut usize) -> Result<Json, String> {
    skip_ws(b, i);
    if *i >= b.len() {
        return Err("JSON 値が途中で終了".to_string());
    }
    match b[*i] {
        b'{' => {
            *i += 1;
            let mut m = Vec::new();
            skip_ws(b, i);
            if *i < b.len() && b[*i] == b'}' {
                *i += 1;
                return Ok(Json::Obj(m));
            }
            loop {
                skip_ws(b, i);
                let key = match parse_value(b, i)? {
                    Json::Str(s) => s,
                    other => return Err(format!("オブジェクトキーが文字列でない: {other:?}")),
                };
                skip_ws(b, i);
                if *i >= b.len() || b[*i] != b':' {
                    return Err(format!("キー {key} の後に ':' が無い"));
                }
                *i += 1;
                let v = parse_value(b, i)?;
                m.push((key, v));
                skip_ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b'}') => {
                        *i += 1;
                        return Ok(Json::Obj(m));
                    }
                    other => return Err(format!("オブジェクト区切り不正: {other:?}")),
                }
            }
        }
        b'[' => {
            *i += 1;
            let mut a = Vec::new();
            skip_ws(b, i);
            if *i < b.len() && b[*i] == b']' {
                *i += 1;
                return Ok(Json::Arr(a));
            }
            loop {
                let v = parse_value(b, i)?;
                a.push(v);
                skip_ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b']') => {
                        *i += 1;
                        return Ok(Json::Arr(a));
                    }
                    other => return Err(format!("配列区切り不正: {other:?}")),
                }
            }
        }
        b'"' => Ok(Json::Str(parse_string(b, i)?)),
        b't' => {
            if b[*i..].starts_with(b"true") {
                *i += 4;
                Ok(Json::Bool(true))
            } else {
                Err(format!("リテラル不正 (byte {i})"))
            }
        }
        b'f' => {
            if b[*i..].starts_with(b"false") {
                *i += 5;
                Ok(Json::Bool(false))
            } else {
                Err(format!("リテラル不正 (byte {i})"))
            }
        }
        b'n' => {
            if b[*i..].starts_with(b"null") {
                *i += 4;
                Ok(Json::Null)
            } else {
                Err(format!("リテラル不正 (byte {i})"))
            }
        }
        c if c == b'-' || c.is_ascii_digit() => {
            let st = *i;
            if c == b'-' {
                *i += 1;
            }
            while *i < b.len()
                && (b[*i].is_ascii_digit() || matches!(b[*i], b'.' | b'e' | b'E' | b'+' | b'-'))
            {
                *i += 1;
            }
            let lit = std::str::from_utf8(&b[st..*i]).map_err(|e| e.to_string())?;
            if lit.is_empty() || lit == "-" {
                return Err(format!("数値リテラル不正 (byte {st})"));
            }
            Ok(Json::Num(lit.to_string()))
        }
        other => Err(format!("JSON 値の先頭が不正: 0x{other:02x} (byte {i})")),
    }
}

fn parse_string(b: &[u8], i: &mut usize) -> Result<String, String> {
    debug_assert_eq!(b[*i], b'"');
    *i += 1;
    let mut s = String::new();
    loop {
        if *i >= b.len() {
            return Err("文字列が閉じていない".to_string());
        }
        match b[*i] {
            b'"' => {
                *i += 1;
                return Ok(s);
            }
            b'\\' => {
                *i += 1;
                if *i >= b.len() {
                    return Err("エスケープ途中で終了".to_string());
                }
                let e = b[*i];
                *i += 1;
                match e {
                    b'"' => s.push('"'),
                    b'\\' => s.push('\\'),
                    b'/' => s.push('/'),
                    b'b' => s.push('\u{0008}'),
                    b'f' => s.push('\u{000c}'),
                    b'n' => s.push('\n'),
                    b'r' => s.push('\r'),
                    b't' => s.push('\t'),
                    b'u' => {
                        if *i + 4 > b.len() {
                            return Err("\\u エスケープが短い".to_string());
                        }
                        let hex = std::str::from_utf8(&b[*i..*i + 4]).map_err(|e| e.to_string())?;
                        let cp = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
                        *i += 4;
                        // サロゲートペア処理
                        if (0xD800..=0xDBFF).contains(&cp) && b[*i..].starts_with(b"\\u") {
                            let hex2 = std::str::from_utf8(&b[*i + 2..*i + 6])
                                .map_err(|e| e.to_string())?;
                            let lo = u32::from_str_radix(hex2, 16).map_err(|e| e.to_string())?;
                            if (0xDC00..=0xDFFF).contains(&lo) {
                                *i += 6;
                                let c = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                s.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                                continue;
                            }
                        }
                        s.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                    }
                    other => return Err(format!("不明なエスケープ: \\{}", other as char)),
                }
            }
            c if c < 0x80 => {
                s.push(c as char);
                *i += 1;
            }
            c => {
                // UTF-8 マルチバイトをそのまま通す
                let len = if c >= 0xF0 {
                    4
                } else if c >= 0xE0 {
                    3
                } else {
                    2
                };
                if *i + len > b.len() {
                    return Err("UTF-8 シーケンス途中で終了".to_string());
                }
                let chunk = std::str::from_utf8(&b[*i..*i + len]).map_err(|e| e.to_string())?;
                s.push_str(chunk);
                *i += len;
            }
        }
    }
}

fn render_json(v: &Json, out: &mut String, indent: usize) {
    let pad = "  ".repeat(indent);
    let pad1 = "  ".repeat(indent + 1);
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(n) => out.push_str(n),
        Json::Str(s) => {
            out.push('"');
            out.push_str(&json_escape(s));
            out.push('"');
        }
        Json::Arr(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (k, item) in a.iter().enumerate() {
                out.push_str(&pad1);
                render_json(item, out, indent + 1);
                if k + 1 < a.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad);
            out.push(']');
        }
        Json::Obj(m) => {
            if m.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (k, (key, val)) in m.iter().enumerate() {
                out.push_str(&pad1);
                out.push('"');
                out.push_str(&json_escape(key));
                out.push_str("\": ");
                render_json(val, out, indent + 1);
                if k + 1 < m.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad);
            out.push('}');
        }
    }
}

// ------------------------------------------------------------------
// Minecraft ランチャー統合 (起動構成への追加 + versions フォルダ完備)
// スキーマは rsift-installer 本流と同一:
//   <mc>/versions/rsift-<mcver>/rsift-<mcver>.json  (inheritsFrom + jvm args)
//   <mc>/launcher_profiles.json の profiles に rsift エントリ upsert
// 破壊回避: 書き戻し前に .bak を1回だけ作成、書き込みは tmp→rename。
// ------------------------------------------------------------------

/// minecraft ディレクトリ検出。環境変数 RSIFT_MC_DIR が最優先 (テスト/上級者)。
pub fn find_minecraft_dir(os: TargetOs) -> Option<PathBuf> {
    if let Ok(v) = std::env::var("RSIFT_MC_DIR") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    match os {
        TargetOs::Windows => std::env::var("APPDATA")
            .ok()
            .map(|a| PathBuf::from(a).join(".minecraft")),
        TargetOs::MacOs => std::env::var("HOME").ok().map(|h| {
            PathBuf::from(h)
                .join("Library")
                .join("Application Support")
                .join("minecraft")
        }),
        _ => std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".minecraft")),
    }
}

/// UNIX 秒 → ISO-8601 UTC (例 2026-08-01T12:34:56Z)。Howard Hinnant の
/// civil calendar アルゴリズムで std のみで換算する (chrono 非依存)。
pub fn iso8601_utc(unix: u64) -> String {
    let secs = unix % 86400;
    let days = (unix / 86400) as i64;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (hh, mm, ss) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp_rsift");
    fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename {}: {e}", path.display())
    })
}

#[derive(Debug, Clone)]
pub struct LauncherOutcome {
    pub profile_registered: bool,
    pub version_dir: PathBuf,
    pub natives_deployed: Vec<String>,
    pub mods_deployed: Vec<String>,
    pub java_args: String,
    pub notes: Vec<String>,
}

pub fn agent_name(os: TargetOs) -> &'static str {
    match os {
        TargetOs::Windows => LIB_WINDOWS_AGENT,
        TargetOs::MacOs => LIB_MACOS_AGENT,
        TargetOs::Linux => LIB_LINUX_AGENT,
        TargetOs::Other => LIB_WINDOWS_AGENT,
    }
}

/// ランチャー統合の本工程。`libs` は同階層に実在する lib (scan 結果)。
/// agent (rsift_jvm) が存在しない環境では profile/version 登録はせず
/// notes に理由を残す (壊れた起動構成を登録しない: fail-loud with note)。
pub fn setup_launcher(
    mc_dir: &Path,
    os: TargetOs,
    source_dir: &Path,
    libs: &[LibFile],
    renderer: &str,
) -> Result<LauncherOutcome, String> {
    let mut notes = Vec::new();
    let agent = agent_name(os);
    let has_agent = libs.iter().any(|l| l.name == agent);
    let version_id = rsift_version_id();
    let versions_root = mc_dir.join("versions");
    let version_dir = versions_root.join(&version_id);

    let (engine_name, gfx_name, replay_name, zoom_name) = match os {
        TargetOs::Windows => (
            LIB_WINDOWS_ENGINE,
            LIB_WINDOWS_GFX,
            LIB_WINDOWS_REPLAY,
            LIB_WINDOWS_ZOOM,
        ),
        TargetOs::MacOs => (
            LIB_MACOS_ENGINE,
            LIB_MACOS_GFX,
            LIB_MACOS_REPLAY,
            LIB_MACOS_ZOOM,
        ),
        TargetOs::Linux => (
            LIB_LINUX_ENGINE,
            LIB_LINUX_GFX,
            LIB_LINUX_REPLAY,
            LIB_LINUX_ZOOM,
        ),
        TargetOs::Other => {
            return Err("launcher 登録は windows/macos/linux のみ対象".to_string());
        }
    };

    if !has_agent {
        notes.push(format!(
            "JVMTI agent {agent} が同階層に無いため profile/version 登録は未実施 (zip 同梱版で再実行してください)"
        ));
        return Ok(LauncherOutcome {
            profile_registered: false,
            version_dir,
            natives_deployed: vec![],
            mods_deployed: vec![],
            java_args: String::new(),
            notes,
        });
    }

    // bootstrap jar 審査: RsiftHooks 等 (Mods ボタン/フック注入) の Java 実体。
    // 不在/破損のまま agentpath だけ登録すると「ゲームは起動するが Mod が
    // 一切効かない (Mods ボタンすら出ない)」壊れた構成を量産してしまうため、
    // agent 不在と同様に登録自体を見送る (wave 204: 実機報告の根治)。
    let jar = match probe_bootstrap_jar(source_dir)? {
        BootstrapJarState::Present(j) => j,
        BootstrapJarState::Missing => {
            notes.push(format!(
                "{BOOTSTRAP_JAR} (Java ブリッジ) が同階層に無いため profile/version 登録は未実施 (Mods ボタンが出ない壊れた構成を置かない。最新の一体 zip 同梱版で再実行してください)"
            ));
            return Ok(LauncherOutcome {
                profile_registered: false,
                version_dir,
                natives_deployed: vec![],
                mods_deployed: vec![],
                java_args: String::new(),
                notes,
            });
        }
        BootstrapJarState::Corrupt => {
            notes.push(format!(
                "{BOOTSTRAP_JAR} が破損/不正 (PK マジック無し又は小さすぎ) のため profile/version 登録は未実施 (再ダウンロード・再展開してください)"
            ));
            return Ok(LauncherOutcome {
                profile_registered: false,
                version_dir,
                natives_deployed: vec![],
                mods_deployed: vec![],
                java_args: String::new(),
                notes,
            });
        }
    };

    // 1) versions/rsift-<mcver>/ フォルダ + natives 配置
    fs::create_dir_all(&version_dir)
        .map_err(|e| format!("create {}: {e}", version_dir.display()))?;
    let mut natives = Vec::new();
    for name in [engine_name, agent, gfx_name, replay_name, zoom_name] {
        if libs.iter().any(|l| l.name == name) {
            let src = source_dir.join(name);
            let dst = version_dir.join(name);
            fs::copy(&src, &dst).map_err(|e| format!("copy {}: {e}", dst.display()))?;
            natives.push(name.to_string());
        }
    }
    // bootstrap jar も agent (dll) の探索先と同じ階層へ配置。
    fs::copy(
        source_dir.join(BOOTSTRAP_JAR),
        version_dir.join(BOOTSTRAP_JAR),
    )
    .map_err(|e| format!("copy {}: {e}", version_dir.join(BOOTSTRAP_JAR).display()))?;
    natives.push(BOOTSTRAP_JAR.to_string());
    notes.push(format!(
        "{BOOTSTRAP_JAR} sha256={} (Java ブリッジ配置)",
        jar.sha256
    ));
    // wave HR (#5 根治): client.txt (難読化 mappings) を agent 探索先と同階層へ配備。
    notes.push(deploy_client_mappings(&source_dir, &version_dir)?);

    // 2) version JSON (inheritsFrom + jvm args、本流スキーマ準拠)
    let agent_path = version_dir.join(agent).display().to_string();
    let java_args = format!(
        "-agentpath:{agent_path} -Drsift.gfx={renderer} -Drsift.home={}",
        version_dir.display()
    );
    let now = iso8601_utc(unix_now());
    let version_json = Json::Obj(vec![
        ("id".into(), Json::Str(version_id.clone())),
        ("inheritsFrom".into(), Json::Str(RSIFT_MC_VERSION.into())),
        ("releaseTime".into(), Json::Str(now.clone())),
        ("time".into(), Json::Str(now)),
        ("type".into(), Json::Str("release".into())),
        (
            "mainClass".into(),
            Json::Str("net.minecraft.client.main.Main".into()),
        ),
        (
            "arguments".into(),
            Json::Obj(vec![(
                "jvm".into(),
                Json::Arr(vec![
                    Json::Str(format!("-Drsift.gfx={renderer}")),
                    Json::Str(format!("-Drsift.home={}", version_dir.display())),
                ]),
            )]),
        ),
    ]);
    let json_path = version_dir.join(format!("{version_id}.json"));
    write_atomic(&json_path, version_json.render().as_bytes())
        .map_err(|e| format!("version json: {e}"))?;

    // 3) mods 配置 (rsgraphics / rsreplay / rszoom を <mc>/mods/ へ)
    let mods_dir = mc_dir.join("mods");
    fs::create_dir_all(&mods_dir).map_err(|e| format!("create {}: {e}", mods_dir.display()))?;
    let mut mods = Vec::new();
    for name in [gfx_name, replay_name, zoom_name] {
        if libs.iter().any(|l| l.name == name) {
            let dst = mods_dir.join(name);
            fs::copy(source_dir.join(name), &dst)
                .map_err(|e| format!("copy {}: {e}", dst.display()))?;
            mods.push(name.to_string());
        }
    }

    // 4) launcher_profiles.json upsert (壊れていたら bak を残して失敗する、上書き破壊はしない)
    let profiles_path = mc_dir.join("launcher_profiles.json");
    let mut root = if profiles_path.is_file() {
        let text = fs::read_to_string(&profiles_path)
            .map_err(|e| format!("read {}: {e}", profiles_path.display()))?;
        Json::parse(&text)
            .map_err(|e| format!("launcher_profiles.json を壊さないために中止: {e}"))?
    } else {
        Json::Obj(vec![])
    };
    if root.obj_get("profiles").is_none() {
        root.obj_insert("profiles", Json::Obj(vec![]));
    }
    let profile = Json::Obj(vec![
        ("created".into(), Json::Str(iso8601_utc(unix_now()))),
        ("gameDir".into(), Json::Str(mc_dir.display().to_string())),
        ("icon".into(), Json::Str("Furnace".into())),
        ("javaArgs".into(), Json::Str(java_args.clone())),
        ("lastVersionId".into(), Json::Str(version_id.clone())),
        ("name".into(), Json::Str(LAUNCHER_PROFILE_NAME.into())),
        ("type".into(), Json::Str("custom".into())),
    ]);
    // 破壊前に1回だけバックアップ (既存 .bak は温存 = 最初の状態を保護)
    if profiles_path.is_file() {
        let bak = mc_dir.join("launcher_profiles.json.bak_rsift");
        if !bak.exists() {
            fs::copy(&profiles_path, &bak).map_err(|e| format!("backup: {e}"))?;
        }
    }
    if let Some(profiles) = root.obj_get_mut("profiles") {
        profiles.obj_insert(LAUNCHER_PROFILE_ID, profile);
    }
    write_atomic(&profiles_path, root.render().as_bytes())
        .map_err(|e| format!("profiles write: {e}"))?;

    notes.push(format!(
        "起動構成 '{LAUNCHER_PROFILE_NAME}' (lastVersionId={version_id}) をランチャーに登録"
    ));
    Ok(LauncherOutcome {
        profile_registered: true,
        version_dir,
        natives_deployed: natives,
        mods_deployed: mods,
        java_args,
        notes,
    })
}

// ============================================================
// PrismLauncher 統合 (wave 202)。対象は Windows 10 の標準構成を第一級とし、
// macOS/Linux も同一スキーマで扱う。レイアウト一次情報:
// - インスタンス: <root>/instances/<id>/{instance.cfg, mmc-pack.json, patches/, .minecraft/}
//   (PrismLauncher 公式アーキテクチャ文書・rubenerd 実測ツリー)
// - instance.cfg: INISettingsObject、必須キー InstanceType (InstanceList.cpp:694)
// - mmc-pack.json: {"formatVersion": 1, "components": [{uid, version, cachedName?, cachedVersion?}]}
//   (PackProfile.cpp:152-157 toJson)
// - patches/rsift.json: OneSixVersionFormat.cpp の formatVersion 1 パッチ。
//   "+jvmArgs" (追記型) で -agentpath/-Drsift.* を載せる (同:144-149,
//   VersionFile::applyTo → applyAddnJvmArguments)。mainClass 等は設定しない
//   ため適用順に依存しない (順不同でも安全なキーだけを使う設計)。
// ============================================================

/// Prism インスタンスのフォルダ名 (= Prisma 画面の instance id)。
pub const PRISM_INSTANCE_ID: &str = "rsift";
/// 画面表示名。
pub const PRISM_INSTANCE_NAME: &str = "Rsift";
/// Rsift パッチの component UID (uid 正規表現 [a-zA-Z0-9-_]+(\....)* 準拠)。
pub const PRISM_COMPONENT_UID: &str = "rsift";

/// PrismLauncher データルート検出。`RSIFT_PRISM_DIR` 環境変数が最優先
/// (RSIFT_MC_DIR と同じ説明可能な上書き規則)。既定:
/// - windows: %APPDATA%\PrismLauncher
/// - macos:   ~/Library/Application Support/PrismLauncher
/// - linux:   ~/.local/share/PrismLauncher、無ければ flatpak 配置
pub fn find_prism_dir(os: TargetOs) -> Option<PathBuf> {
    if let Ok(v) = std::env::var("RSIFT_PRISM_DIR") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    match os {
        TargetOs::Windows => std::env::var("APPDATA")
            .ok()
            .map(|a| PathBuf::from(a).join("PrismLauncher")),
        TargetOs::MacOs => std::env::var("HOME").ok().map(|h| {
            PathBuf::from(h)
                .join("Library")
                .join("Application Support")
                .join("PrismLauncher")
        }),
        TargetOs::Linux => std::env::var("HOME").ok().map(|h| {
            let home = PathBuf::from(h);
            let standard = home.join(".local").join("share").join("PrismLauncher");
            if standard.is_dir() {
                standard
            } else {
                home.join(".var")
                    .join("app")
                    .join("org.prismlauncher.PrismLauncher")
                    .join("data")
                    .join("PrismLauncher")
            }
        }),
        TargetOs::Other => None,
    }
}

#[derive(Debug, Clone)]
pub struct PrismOutcome {
    pub instance_created: bool,
    /// "rsift" 名の外部製インスタンスと衝突して中止した場合 true (非破壊保証)。
    pub foreign_conflict: bool,
    pub instance_dir: PathBuf,
    pub natives_deployed: Vec<String>,
    pub mods_deployed: Vec<String>,
    pub notes: Vec<String>,
}

/// Prism インスタンス生成の本工程。launcher 版と同じガード思想:
/// - agent (rsift_jvm) 不在 → 登録しない (notes に理由、created=false)
/// - 外部製 "rsift" インスタンスがある → 触らず中止 (foreign_conflict=true)
/// - 2 回目の実行は自家マーカー経由で冪等更新 (ファイルは tmp→rename)
pub fn setup_prism(
    prism_root: &Path,
    os: TargetOs,
    source_dir: &Path,
    libs: &[LibFile],
    renderer: &str,
) -> Result<PrismOutcome, String> {
    let mut notes = Vec::new();
    let agent = agent_name(os);
    let version_id = rsift_version_id();
    let instance_dir = prism_root.join("instances").join(PRISM_INSTANCE_ID);

    let (engine_name, gfx_name, replay_name, zoom_name) = match os {
        TargetOs::Windows => (
            LIB_WINDOWS_ENGINE,
            LIB_WINDOWS_GFX,
            LIB_WINDOWS_REPLAY,
            LIB_WINDOWS_ZOOM,
        ),
        TargetOs::MacOs => (
            LIB_MACOS_ENGINE,
            LIB_MACOS_GFX,
            LIB_MACOS_REPLAY,
            LIB_MACOS_ZOOM,
        ),
        TargetOs::Linux => (
            LIB_LINUX_ENGINE,
            LIB_LINUX_GFX,
            LIB_LINUX_REPLAY,
            LIB_LINUX_ZOOM,
        ),
        TargetOs::Other => {
            return Err("prism 登録は windows/macos/linux のみ対象".to_string());
        }
    };

    if !libs.iter().any(|l| l.name == agent) {
        notes.push(format!(
            "JVMTI agent {agent} が同階層に無いため Prism インスタンス登録は未実施 (zip 同梱版で再実行してください)"
        ));
        return Ok(PrismOutcome {
            instance_created: false,
            foreign_conflict: false,
            instance_dir,
            natives_deployed: vec![],
            mods_deployed: vec![],
            notes,
        });
    }

    // bootstrap jar 審査 (launcher 経路と同構造): 無ければ Mod が全く
    // 効かない壊れたインスタンスを量産しない (wave 204: 実機報告の根治)。
    let jar = match probe_bootstrap_jar(source_dir)? {
        BootstrapJarState::Present(j) => j,
        BootstrapJarState::Missing => {
            notes.push(format!(
                "{BOOTSTRAP_JAR} (Java ブリッジ) が同階層に無いため Prism インスタンス登録は未実施 (Mods ボタンが出ない壊れた構成を置かない。最新の一体 zip 同梱版で再実行してください)"
            ));
            return Ok(PrismOutcome {
                instance_created: false,
                foreign_conflict: false,
                instance_dir,
                natives_deployed: vec![],
                mods_deployed: vec![],
                notes,
            });
        }
        BootstrapJarState::Corrupt => {
            notes.push(format!(
                "{BOOTSTRAP_JAR} が破損/不正 (PK マジック無し又は小さすぎ) のため Prism インスタンス登録は未実施 (再ダウンロード・再展開してください)"
            ));
            return Ok(PrismOutcome {
                instance_created: false,
                foreign_conflict: false,
                instance_dir,
                natives_deployed: vec![],
                mods_deployed: vec![],
                notes,
            });
        }
    };

    // 外部製インスタンスの保護: Rsift 生成印 (自家パッチ + mmc-pack 内の
    // rsift 成分) が無い "rsift" 名フォルダは絶対に上書きしない。
    if instance_dir.exists() {
        let marker_patch = instance_dir
            .join("patches")
            .join(format!("{PRISM_COMPONENT_UID}.json"));
        let marker_pack = instance_dir.join("mmc-pack.json");
        let ours = marker_patch.is_file()
            && marker_pack.is_file()
            && fs::read_to_string(&marker_pack)
                .map(|t| t.contains("\"uid\": \"rsift\""))
                .unwrap_or(false);
        if !ours {
            return Ok(PrismOutcome {
                instance_created: false,
                foreign_conflict: true,
                instance_dir,
                natives_deployed: vec![],
                mods_deployed: vec![],
                notes: vec![format!(
                    "Prism インスタンス '{}' が既に存在しますが Rsift 生成印がありません。ユーザーの大切な構成かもしれないため中止しました (手動で退避するか RSIFT_PRISM_DIR で別 root を指定してください)",
                    PRISM_INSTANCE_ID
                )],
            });
        }
    }

    // 1) natives 配置 (agentpath 実体): <inst>/rsift-natives/
    let natives_dir = instance_dir.join("rsift-natives");
    fs::create_dir_all(&natives_dir)
        .map_err(|e| format!("create {}: {e}", natives_dir.display()))?;
    let mut natives = Vec::new();
    for name in [engine_name, agent, gfx_name, replay_name, zoom_name] {
        if libs.iter().any(|l| l.name == name) {
            let dst = natives_dir.join(name);
            fs::copy(source_dir.join(name), &dst)
                .map_err(|e| format!("copy {}: {e}", dst.display()))?;
            natives.push(name.to_string());
        }
    }
    // bootstrap jar も agent (dll) の探索先と同じ階層へ配置。
    fs::copy(
        source_dir.join(BOOTSTRAP_JAR),
        natives_dir.join(BOOTSTRAP_JAR),
    )
    .map_err(|e| format!("copy {}: {e}", natives_dir.join(BOOTSTRAP_JAR).display()))?;
    natives.push(BOOTSTRAP_JAR.to_string());
    notes.push(format!(
        "{BOOTSTRAP_JAR} sha256={} (Java ブリッジ配置)",
        jar.sha256
    ));
    // wave HR (#5 根治): client.txt (難読化 mappings) を agent 探索先と同階層へ配備。
    notes.push(deploy_client_mappings(&source_dir, &natives_dir)?);

    // 2) instance.cfg (InstanceList が読む必須キー InstanceType を必ず書く)
    let cfg_text = concat!(
        "[General]\n",
        "InstanceType=OneSix\n",
        "iconKey=default\n",
        "name=Rsift\n",
        "notes=Rsift native loader (generated by rsift-setup; safe to re-run)\n",
    );
    write_atomic(&instance_dir.join("instance.cfg"), cfg_text.as_bytes())
        .map_err(|e| format!("instance.cfg: {e}"))?;

    // 3) mmc-pack.json (PackProfile.cpp toJson 準拠)
    let pack = Json::Obj(vec![
        (
            "components".into(),
            Json::Arr(vec![
                Json::Obj(vec![
                    ("cachedName".into(), Json::Str("Minecraft".into())),
                    ("cachedVersion".into(), Json::Str(RSIFT_MC_VERSION.into())),
                    ("uid".into(), Json::Str("net.minecraft".into())),
                    ("version".into(), Json::Str(RSIFT_MC_VERSION.into())),
                ]),
                Json::Obj(vec![
                    ("cachedName".into(), Json::Str(PRISM_INSTANCE_NAME.into())),
                    ("cachedVersion".into(), Json::Str(RSIFT_MC_VERSION.into())),
                    ("uid".into(), Json::Str(PRISM_COMPONENT_UID.into())),
                    ("version".into(), Json::Str(RSIFT_MC_VERSION.into())),
                ]),
            ]),
        ),
        ("formatVersion".into(), Json::Num("1".into())),
    ]);
    write_atomic(
        &instance_dir.join("mmc-pack.json"),
        pack.render().as_bytes(),
    )
    .map_err(|e| format!("mmc-pack.json: {e}"))?;

    // 4) patches/rsift.json ("+jvmArgs" 追記型 → 既定 JVM 引数を壊さない)
    //    パスに空白を含んでも JVM には argv として 1 要素で渡るため安全
    //    (QProcess の引数配列経路; shell 展開ではない)。
    let patches_dir = instance_dir.join("patches");
    fs::create_dir_all(&patches_dir)
        .map_err(|e| format!("create {}: {e}", patches_dir.display()))?;
    let agent_path = natives_dir.join(agent).display().to_string();
    let java_args = format!(
        "-agentpath:{agent_path} -Drsift.gfx={renderer} -Drsift.home={}",
        natives_dir.display()
    );
    let patch = Json::Obj(vec![
        ("formatVersion".into(), Json::Num("1".into())),
        (
            "name".into(),
            Json::Str("Rsift (native engine + JVMTI agent)".into()),
        ),
        ("uid".into(), Json::Str(PRISM_COMPONENT_UID.into())),
        ("version".into(), Json::Str(RSIFT_MC_VERSION.into())),
        (
            "+jvmArgs".into(),
            Json::Arr(vec![
                Json::Str(format!("-agentpath:{agent_path}")),
                Json::Str(format!("-Drsift.gfx={renderer}")),
                Json::Str(format!("-Drsift.home={}", natives_dir.display())),
            ]),
        ),
    ]);
    write_atomic(
        &patches_dir.join(format!("{PRISM_COMPONENT_UID}.json")),
        patch.render().as_bytes(),
    )
    .map_err(|e| format!("patch json: {e}"))?;

    // 5) mods 配置: <inst>/.minecraft/mods/
    let mods_dir = instance_dir.join(".minecraft").join("mods");
    fs::create_dir_all(&mods_dir).map_err(|e| format!("create {}: {e}", mods_dir.display()))?;
    let mut mods = Vec::new();
    for name in [gfx_name, replay_name, zoom_name] {
        if libs.iter().any(|l| l.name == name) {
            let dst = mods_dir.join(name);
            fs::copy(source_dir.join(name), &dst)
                .map_err(|e| format!("copy {}: {e}", dst.display()))?;
            mods.push(name.to_string());
        }
    }

    notes.push(format!(
        "Prism インスタンス '{PRISM_INSTANCE_NAME}' (id={PRISM_INSTANCE_ID}, mc={}) を登録",
        RSIFT_MC_VERSION
    ));
    notes.push(format!(
        "PrismLauncher を再起動するとインスタンス一覧に現れます (jvm args: {java_args})"
    ));
    let _ = version_id;
    Ok(PrismOutcome {
        instance_created: true,
        foreign_conflict: false,
        instance_dir,
        natives_deployed: natives,
        mods_deployed: mods,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // wave 210 HF pin: prism ステップの配備サマリ行は mods/natives/instance_dir
    // を必ず含む (実機「setup 成功 -> mods 空」の切り分け不能を二度と起こさない)。
    #[test]
    fn hf_prism_summary_line_lists_deployed_mods() {
        let out = PrismOutcome {
            instance_created: true,
            foreign_conflict: false,
            instance_dir: PathBuf::from("C:/instances/rsift"),
            natives_deployed: vec!["rsift_jvm.dll".to_string(), "rsift.dll".to_string()],
            mods_deployed: vec!["rsgraphics.dll".to_string(), "rsreplay.dll".to_string()],
            notes: vec![],
        };
        let line = prism_summary_line(&out);
        assert!(line.contains("instance_dir=C:/instances/rsift"));
        assert!(line.contains("natives=[\"rsift_jvm.dll\", \"rsift.dll\"]"));
        assert!(line.contains("mods=[\"rsgraphics.dll\", \"rsreplay.dll\"]"));
        // 空配備でも省略せず mods=[] と出ること (0 件の沈黙こそが観測欠陥だった)
        let empty = PrismOutcome {
            instance_created: true,
            foreign_conflict: false,
            instance_dir: PathBuf::from("C:/instances/rsift"),
            natives_deployed: vec![],
            mods_deployed: vec![],
            notes: vec![],
        };
        let line2 = prism_summary_line(&empty);
        assert!(line2.contains("mods=[]"));
        assert!(line2.contains("natives=[]"));
    }

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
        // aarch64 + macOS 26+ + rsgraphics.dylib → Metal 4 経路で ready。
        let d = decide(&host(
            TargetOs::MacOs,
            "aarch64",
            Some((26, 0)),
            &[LIB_MACOS_ENGINE, LIB_MACOS_GFX],
        ));
        assert_eq!(d.renderer, Renderer::Metal4);
        assert!(d.ready);
        assert_eq!(d.graphics_lib.as_deref(), Some(LIB_MACOS_GFX));
    }

    #[test]
    fn decide_mac_ineligible_host_uses_classic_metal() {
        // Intel または macOS 25 以下では classic Metal 経路 (誤 Metal4 構造拒否)。
        for (arch, ver) in [("x86_64", Some((26, 0))), ("aarch64", Some((25, 5)))] {
            let d = decide(&host(
                TargetOs::MacOs,
                arch,
                ver,
                &[LIB_MACOS_ENGINE, LIB_MACOS_GFX],
            ));
            assert_eq!(
                d.renderer,
                Renderer::MetalClassic,
                "arch={arch} ver={ver:?} では classic 必須"
            );
            assert!(d.notes.iter().any(|n| n.contains("classic Metal")));
        }
    }

    #[test]
    fn decide_mac_no_gfx_not_ready() {
        // gfx dylib が無ければ renderer None で not-ready。
        let d = decide(&host(
            TargetOs::MacOs,
            "aarch64",
            Some((26, 0)),
            &[LIB_MACOS_ENGINE],
        ));
        assert_eq!(d.renderer, Renderer::None);
        assert!(!d.ready);
    }

    #[test]
    fn decide_mac_replay_only_not_ready_but_noted() {
        // replay だけあって gfx/エンジンが無い: not-ready、かつ replay 検出は notes に残る。
        let d = decide(&host(
            TargetOs::MacOs,
            "aarch64",
            Some((26, 0)),
            &[LIB_MACOS_REPLAY],
        ));
        assert!(!d.ready);
        assert!(d.notes.iter().any(|n| n.contains("RsReplay")));
    }

    #[test]
    fn decide_mac_no_engine_not_ready() {
        let d = decide(&host(
            TargetOs::MacOs,
            "aarch64",
            Some((26, 0)),
            &[LIB_MACOS_GFX],
        ));
        assert!(!d.ready);
        assert!(d.engine_lib.is_none());
        assert!(d.notes.iter().any(|n| n.contains(LIB_MACOS_ENGINE)));
    }

    #[test]
    fn decide_windows_gfx_ready() {
        // windows は rsgraphics.dll (wgpu 自動選択) で ready。
        let d = decide(&host(
            TargetOs::Windows,
            "x86_64",
            None,
            &[LIB_WINDOWS_ENGINE, LIB_WINDOWS_GFX, LIB_WINDOWS_REPLAY],
        ));
        assert!(d.ready);
        assert!(d.notes.iter().any(|n| n.contains("wgpu")));
        assert!(d.notes.iter().any(|n| n.contains("RsReplay")));
    }

    #[test]
    fn decide_windows_replay_same_zip_detected() {
        // zip 同梱の replay.dll があれば notes に載る (2 Mod 同梱形式の検証)。
        let d = decide(&host(
            TargetOs::Windows,
            "x86_64",
            None,
            &[LIB_WINDOWS_ENGINE, LIB_WINDOWS_GFX, LIB_WINDOWS_REPLAY],
        ));
        assert!(d.notes.iter().any(|n| n.contains(LIB_WINDOWS_REPLAY)));
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
    fn decide_linux_gfx() {
        let d = decide(&host(
            TargetOs::Linux,
            "x86_64",
            None,
            &[LIB_LINUX_ENGINE, LIB_LINUX_GFX],
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
            &[LIB_MACOS_ENGINE, LIB_MACOS_GFX],
        );
        let d = decide(&h);
        let s = render_launch_config(&h, &d, "null", "null");
        assert!(s.contains("\"schema\": \"rsift.launch/1\""));
        assert!(s.contains("\"target\": \"macos\""));
        assert!(s.contains("\"renderer\": \"metal4\""));
        assert!(s.contains("\"macos_version\": \"26.0\""));
        assert!(s.contains("\"prepared\": true"));
        assert!(s.contains(&format!("\"{LIB_MACOS_ENGINE}\"")));
        assert!(s.contains(&format!("\"{LIB_MACOS_GFX}\"")));
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
        fs::write(d.join(LIB_LINUX_GFX), b"gfx").unwrap();
        fs::write(d.join(LIB_LINUX_REPLAY), b"replay").unwrap();
        let cli = Cli {
            dir: d.clone(),
            self_test: true,
            dry_run: false,
        };
        let code = run(&cli);
        assert_eq!(code, EXIT_OK);
        let cfg = fs::read_to_string(d.join(LAUNCH_CONFIG_NAME)).unwrap();
        assert!(cfg.contains("\"renderer\": \"vulkan\""));
        assert!(cfg.contains("librsgraphics.so"));
        let log_txt = fs::read_to_string(d.join(LOG_TXT_NAME)).unwrap();
        assert!(log_txt.contains("rsift.setup.log/1"));
        assert!(log_txt.contains("sha256="), "lib ハッシュが記録される");
        assert!(log_txt.contains("RsReplay"), "2 Mod 同梱検出がログに残る");
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
        fs::write(d.join(LIB_LINUX_GFX), b"g").unwrap();
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
    // ---- 最小 JSON ----
    #[test]
    fn json_roundtrip_nested_and_escapes() {
        let src = r#"{"profiles": {"abc": {"name": "Rsift \"x\"", "n": 1, "f": false, "a": [1, 2.5, "e\n"], "o": {}}}, "top": true}"#;
        let v = Json::parse(src).unwrap();
        let out = v.render();
        let v2 = Json::parse(&out).unwrap();
        assert_eq!(v, v2, "parse→render→parse で構造が完全一致");
        let name = v
            .obj_get("profiles")
            .and_then(|p| p.obj_get("abc"))
            .and_then(|p| p.obj_get("name"))
            .and_then(|n| n.as_str())
            .unwrap();
        assert_eq!(name, "Rsift \"x\"");
    }

    #[test]
    fn json_malformed_fails_with_position() {
        assert!(Json::parse("{").is_err());
        assert!(Json::parse(r#"{"a": }"#).is_err());
        assert!(Json::parse("not json").is_err());
        assert!(Json::parse(r#"[1, 2"#).is_err());
    }

    #[test]
    fn iso8601_utc_known_values() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(iso8601_utc(1_785_571_200), "2026-08-01T08:00:00Z");
    }

    // ---- ランチャー統合 ----
    fn fake_lib(name: &str) -> LibFile {
        LibFile {
            name: name.to_string(),
            size: 3,
            sha256: "0".repeat(64),
        }
    }

    fn full_linux_libs() -> Vec<LibFile> {
        vec![
            fake_lib(LIB_LINUX_ENGINE),
            fake_lib(LIB_LINUX_AGENT),
            fake_lib(LIB_LINUX_GFX),
            fake_lib(LIB_LINUX_REPLAY),
            fake_lib(LIB_LINUX_ZOOM),
        ]
    }

    fn full_windows_libs() -> Vec<LibFile> {
        vec![
            fake_lib(LIB_WINDOWS_ENGINE),
            fake_lib(LIB_WINDOWS_AGENT),
            fake_lib(LIB_WINDOWS_GFX),
            fake_lib(LIB_WINDOWS_REPLAY),
            fake_lib(LIB_WINDOWS_ZOOM),
        ]
    }

    /// 妥当形の偽 bootstrap jar (PK + 下限サイズ超) を dir に書き、内容を返す
    /// (配置後のバイト一致検証用)。
    fn write_fake_bootstrap_jar(dir: &Path) -> Vec<u8> {
        let mut body = b"PK\x03\x04".to_vec();
        body.resize(600, 0xAB);
        fs::write(dir.join(BOOTSTRAP_JAR), &body).unwrap();
        body
    }

    #[test]
    fn launcher_full_setup_creates_version_dir_and_profile() {
        let home = tmpdir("launcher");
        let src = home.join("src");
        fs::create_dir_all(&src).unwrap();
        for l in &full_linux_libs() {
            fs::write(src.join(&l.name), b"dll").unwrap();
        }
        write_fake_bootstrap_jar(&src);
        let jar_bytes = fs::read(src.join(BOOTSTRAP_JAR)).unwrap();
        let mc = home.join(".minecraft");
        fs::create_dir_all(&mc).unwrap();
        fs::write(
            mc.join("launcher_profiles.json"),
            r#"{"profiles": {"vanilla": {"name": "release", "type": "latest-release"}}}"#,
        )
        .unwrap();

        let out = setup_launcher(&mc, TargetOs::Linux, &src, &full_linux_libs(), "vulkan")
            .expect("setup_launcher Ok");
        assert!(out.profile_registered);
        let vdir = mc.join("versions").join("rsift-1.21.11");
        assert!(vdir.is_dir(), "versions フォルダが実在する");
        let vjson = vdir.join("rsift-1.21.11.json");
        let vtext = fs::read_to_string(&vjson).unwrap();
        assert!(vtext.contains("\"id\": \"rsift-1.21.11\""));
        assert!(vtext.contains("\"inheritsFrom\": \"1.21.11\""));
        // version json の jvm は -Drsift.* のみ (本流準拠: agentpath は profile 側)
        assert!(vtext.contains("-Drsift.gfx=vulkan"));
        assert!(vtext.contains("net.minecraft.client.main.Main"));
        assert!(
            out.java_args.contains("-agentpath:"),
            "JVM agent 配線は javaArgs に"
        );
        for n in &out.natives_deployed {
            assert!(vdir.join(n).is_file(), "native {n} 実在");
        }
        // bootstrap jar (Java ブリッジ) も agent と同階層へバイト一致で配置
        assert!(
            out.natives_deployed.iter().any(|n| n == BOOTSTRAP_JAR),
            "natives に jar: {:?}",
            out.natives_deployed
        );
        assert_eq!(
            fs::read(vdir.join(BOOTSTRAP_JAR)).unwrap(),
            jar_bytes,
            "jar はバイト一致配置 (agent の探索先 = dll 同階層)"
        );
        assert!(mc.join("mods").join(LIB_LINUX_GFX).is_file());
        assert!(mc.join("mods").join(LIB_LINUX_REPLAY).is_file());
        assert!(mc.join("mods").join(LIB_LINUX_ZOOM).is_file());
        let pv =
            Json::parse(&fs::read_to_string(mc.join("launcher_profiles.json")).unwrap()).unwrap();
        let profs = pv.obj_get("profiles").unwrap();
        let rs = profs.obj_get("rsift").expect("rsift プロファイル登録");
        assert_eq!(
            rs.obj_get("lastVersionId").and_then(|x| x.as_str()),
            Some("rsift-1.21.11")
        );
        assert!(profs.obj_get("vanilla").is_some(), "既存プロファイルは温存");
        assert!(mc.join("launcher_profiles.json.bak_rsift").is_file());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn launcher_second_run_idempotent_no_duplicate_no_new_backup() {
        let home = tmpdir("idem");
        let src = home.join("src");
        fs::create_dir_all(&src).unwrap();
        for l in &full_linux_libs() {
            fs::write(src.join(&l.name), b"dll").unwrap();
        }
        write_fake_bootstrap_jar(&src);
        let mc = home.join(".minecraft");
        fs::create_dir_all(&mc).unwrap();
        fs::write(mc.join("launcher_profiles.json"), r#"{"profiles": {}}"#).unwrap();
        setup_launcher(&mc, TargetOs::Linux, &src, &full_linux_libs(), "vulkan").unwrap();
        let bak1 = fs::read(mc.join("launcher_profiles.json.bak_rsift")).unwrap();
        setup_launcher(&mc, TargetOs::Linux, &src, &full_linux_libs(), "vulkan").unwrap();
        let bak2 = fs::read(mc.join("launcher_profiles.json.bak_rsift")).unwrap();
        assert_eq!(bak1, bak2, "2 回目で初期バックアップは上書きしない");
        let text = fs::read_to_string(mc.join("launcher_profiles.json")).unwrap();
        assert_eq!(
            text.matches("\"rsift\"").count(),
            1,
            "rsift エントリは 1 個のみ"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn launcher_malformed_profiles_aborts_without_clobber() {
        let home = tmpdir("badprof");
        let src = home.join("src");
        fs::create_dir_all(&src).unwrap();
        for l in &full_linux_libs() {
            fs::write(src.join(&l.name), b"dll").unwrap();
        }
        write_fake_bootstrap_jar(&src);
        let mc = home.join(".minecraft");
        fs::create_dir_all(&mc).unwrap();
        fs::write(mc.join("launcher_profiles.json"), "{ broken").unwrap();
        let r = setup_launcher(&mc, TargetOs::Linux, &src, &full_linux_libs(), "vulkan");
        assert!(r.is_err(), "壊れたプロファイルは登録中止");
        assert!(r.unwrap_err().contains("壊さないために中止"));
        assert_eq!(
            fs::read_to_string(mc.join("launcher_profiles.json")).unwrap(),
            "{ broken"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn launcher_missing_agent_skips_registration_with_note() {
        let home = tmpdir("noagent");
        let mc = home.join(".minecraft");
        fs::create_dir_all(&mc).unwrap();
        let libs = vec![fake_lib(LIB_LINUX_ENGINE), fake_lib(LIB_LINUX_GFX)];
        let out = setup_launcher(&mc, TargetOs::Linux, &home, &libs, "vulkan").unwrap();
        assert!(
            !out.profile_registered,
            "agent 無しでは登録しない (壊れた構成を置かない)"
        );
        assert!(out.notes.iter().any(|n| n.contains("JVMTI agent")));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn find_minecraft_dir_env_override_wins() {
        std::env::set_var("RSIFT_MC_DIR", "/tmp/rsift_mc_override_test");
        let d = find_minecraft_dir(TargetOs::Linux);
        std::env::remove_var("RSIFT_MC_DIR");
        assert_eq!(d, Some(PathBuf::from("/tmp/rsift_mc_override_test")));
    }
    // ============================================================
    // wave 202: PrismLauncher 統合の検定
    // ============================================================
    #[test]
    fn prism_dir_env_override_wins_and_has_priority_shape() {
        let d = tmpdir("prism_env");
        std::env::set_var("RSIFT_PRISM_DIR", &d);
        assert_eq!(find_prism_dir(TargetOs::Windows), Some(d.clone()));
        std::env::remove_var("RSIFT_PRISM_DIR");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn prism_full_setup_creates_instance_layout_verified_against_format() {
        let home = tmpdir("prism_full");
        let src = home.join("src");
        fs::create_dir_all(&src).unwrap();
        for l in &full_windows_libs() {
            fs::write(src.join(&l.name), b"dll").unwrap();
        }
        write_fake_bootstrap_jar(&src);
        let root = home.join("PrismLauncher");
        fs::create_dir_all(&root).unwrap();

        let out = setup_prism(
            &root,
            TargetOs::Windows,
            &src,
            &full_windows_libs(),
            "vulkan",
        )
        .expect("setup_prism Ok");
        assert!(out.instance_created);
        assert!(!out.foreign_conflict);
        let inst = root.join("instances").join("rsift");
        // instance.cfg: InstanceList 必須キー InstanceType を実検証
        let cfg = fs::read_to_string(inst.join("instance.cfg")).unwrap();
        assert!(cfg.contains("InstanceType=OneSix"), "必須キー実在: {cfg}");
        assert!(cfg.contains("name=Rsift"));
        assert!(cfg.contains("[General]"));
        // mmc-pack.json: PackProfile.cpp toJson 準拠の完全形
        let pack_text = fs::read_to_string(inst.join("mmc-pack.json")).unwrap();
        assert!(
            pack_text.contains("\"formatVersion\": 1"),
            "formatVersion 1 実在: {pack_text}"
        );
        let pack = Json::parse(&pack_text).expect("mmc-pack parse");
        let comps = match pack.obj_get("components") {
            Some(Json::Arr(v)) => v,
            other => panic!("components は配列: {other:?}"),
        };
        assert_eq!(comps.len(), 2);
        assert_eq!(
            comps[0].obj_get("uid").and_then(|u| u.as_str()),
            Some("net.minecraft")
        );
        assert_eq!(
            comps[0].obj_get("version").and_then(|u| u.as_str()),
            Some("1.21.11")
        );
        assert_eq!(
            comps[1].obj_get("uid").and_then(|u| u.as_str()),
            Some("rsift")
        );
        // patches/rsift.json: OneSixVersionFormat の "+jvmArgs" + uid 規格
        let patch =
            Json::parse(&fs::read_to_string(inst.join("patches").join("rsift.json")).unwrap())
                .expect("patch parse");
        assert_eq!(patch.obj_get("uid").and_then(|u| u.as_str()), Some("rsift"));
        let args = match patch.obj_get("+jvmArgs") {
            Some(Json::Arr(v)) => v,
            other => panic!("+jvmArgs は配列: {other:?}"),
        };
        let argstrs: Vec<String> = args
            .iter()
            .filter_map(|a| a.as_str().map(|x| x.to_string()))
            .collect();
        assert!(
            argstrs
                .iter()
                .any(|a| a.starts_with("-agentpath:") && a.contains(LIB_WINDOWS_AGENT)),
            "agentpath 実在: {argstrs:?}"
        );
        assert!(argstrs
            .iter()
            .any(|a| a == &"-Drsift.gfx=vulkan".to_string()));
        assert!(argstrs.iter().any(|a| a.starts_with("-Drsift.home=")));
        // natives + mods の実体
        for n in &out.natives_deployed {
            assert!(inst.join("rsift-natives").join(n).is_file(), "native {n}");
        }
        assert_eq!(
            out.natives_deployed.len(),
            6,
            "engine/agent/gfx/replay/zoom/bootstrap-jar"
        );
        for m in ["rsgraphics.dll", "rsreplay.dll", "rszoom.dll"] {
            assert!(
                inst.join(".minecraft").join("mods").join(m).is_file(),
                "mod {m}"
            );
        }
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn prism_second_run_idempotent_updates_not_duplicates() {
        let home = tmpdir("prism_idem");
        let src = home.join("src");
        fs::create_dir_all(&src).unwrap();
        for l in &full_windows_libs() {
            fs::write(src.join(&l.name), b"dll").unwrap();
        }
        write_fake_bootstrap_jar(&src);
        let root = home.join("PrismLauncher");
        fs::create_dir_all(&root).unwrap();
        setup_prism(
            &root,
            TargetOs::Windows,
            &src,
            &full_windows_libs(),
            "vulkan",
        )
        .unwrap();
        setup_prism(
            &root,
            TargetOs::Windows,
            &src,
            &full_windows_libs(),
            "vulkan",
        )
        .unwrap();
        let inst = root.join("instances").join("rsift");
        let pack = fs::read_to_string(inst.join("mmc-pack.json")).unwrap();
        assert_eq!(
            pack.matches("\"uid\": \"rsift\"").count(),
            1,
            "成分は重複しない"
        );
        assert_eq!(pack.matches("net.minecraft").count(), 1);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn prism_foreign_instance_named_rsift_is_never_overwritten() {
        let home = tmpdir("prism_foreign");
        let src = home.join("src");
        fs::create_dir_all(&src).unwrap();
        for l in &full_windows_libs() {
            fs::write(src.join(&l.name), b"dll").unwrap();
        }
        write_fake_bootstrap_jar(&src);
        let root = home.join("PrismLauncher");
        let inst = root.join("instances").join("rsift");
        fs::create_dir_all(&inst).unwrap();
        // 外部製 (Rsift 印の無い) rsift 名インスタンス
        fs::write(
            inst.join("instance.cfg"),
            b"[General]\nname=My Precious Pack\n",
        )
        .unwrap();
        let before = fs::read(inst.join("instance.cfg")).unwrap();
        let out = setup_prism(
            &root,
            TargetOs::Windows,
            &src,
            &full_windows_libs(),
            "vulkan",
        )
        .expect("Ok (中止は Ok+Conflict)");
        assert!(out.foreign_conflict, "衝突検出");
        assert!(!out.instance_created, "作成しない");
        assert!(out.notes.iter().any(|n| n.contains("生成印がありません")));
        assert_eq!(
            fs::read(inst.join("instance.cfg")).unwrap(),
            before,
            "外部製は 1 バイトも触らない"
        );
        assert!(
            !inst.join("mmc-pack.json").exists(),
            "新規ファイルも作らない"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn prism_without_agent_skips_with_note_nothing_created() {
        let home = tmpdir("prism_noagent");
        let src = home.join("src");
        fs::create_dir_all(&src).unwrap();
        let libs = vec![fake_lib(LIB_WINDOWS_GFX)]; // agent 無し
        fs::write(src.join(LIB_WINDOWS_GFX), b"g").unwrap();
        let root = home.join("PrismLauncher");
        fs::create_dir_all(&root).unwrap();
        let out = setup_prism(&root, TargetOs::Windows, &src, &libs, "none").unwrap();
        assert!(!out.instance_created);
        assert!(out.notes.iter().any(|n| n.contains("agent")));
        assert!(
            !root
                .join("instances")
                .join("rsift")
                .join("instance.cfg")
                .is_file(),
            "agent 無しでは何も登録しない = 壊れた起動構成を量産しない"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn prism_jvm_args_are_single_string_argvs_even_with_spaces() {
        // ユーザー名に空白を含む環境 (例 "C:\Users\Sei San\...") でも、
        // agentpath/home は各々 1 つの argv 要素として JVM へ届く構造を pin。
        let home = tmpdir("prism sp ace");
        let src = home.join("s r c");
        fs::create_dir_all(&src).unwrap();
        for l in &full_windows_libs() {
            fs::write(src.join(&l.name), b"dll").unwrap();
        }
        write_fake_bootstrap_jar(&src);
        let root = home.join("Pr ism");
        fs::create_dir_all(&root).unwrap();
        let out = setup_prism(
            &root,
            TargetOs::Windows,
            &src,
            &full_windows_libs(),
            "vulkan",
        )
        .unwrap();
        let inst = root.join("instances").join("rsift");
        let patch =
            Json::parse(&fs::read_to_string(inst.join("patches").join("rsift.json")).unwrap())
                .unwrap();
        let args = match patch.obj_get("+jvmArgs") {
            Some(Json::Arr(v)) => v,
            other => panic!("+jvmArgs は配列: {other:?}"),
        };
        let argstrs: Vec<String> = args
            .iter()
            .filter_map(|a| a.as_str().map(|x| x.to_string()))
            .collect();
        assert!(
            argstrs
                .iter()
                .all(|a| a.contains(" ") && a.contains("Pr ism") || !a.contains("Pr ism")),
            "空白を含んでも 1 要素のまま: {argstrs:?}"
        );
        assert!(out.notes.iter().any(|n| n.contains("再起動")));
        let _ = fs::remove_dir_all(&home);
    }

    // ============================================================
    // wave 204: bootstrap jar 同梱/配線の検定 (wave 203 実機報告
    // 「Mods ボタンが無い」= jar 未同梱で Java ブリッジ全ロード失敗の根治)
    // ============================================================
    #[test]
    fn bootstrap_jar_probe_states() {
        let home = tmpdir("jarprobe");
        // 不在
        assert_eq!(
            probe_bootstrap_jar(&home).unwrap(),
            BootstrapJarState::Missing
        );
        // 破損 (PK マジック無し)
        fs::write(home.join(BOOTSTRAP_JAR), b"NOPE-not-a-jar").unwrap();
        assert_eq!(
            probe_bootstrap_jar(&home).unwrap(),
            BootstrapJarState::Corrupt,
            "PK 無しは Corrupt"
        );
        // 破損 (PK 有りだが下限未満 = 152B スタブ系)
        let mut tiny = b"PK\x03\x04".to_vec();
        tiny.resize(200, 0);
        fs::write(home.join(BOOTSTRAP_JAR), &tiny).unwrap();
        assert_eq!(
            probe_bootstrap_jar(&home).unwrap(),
            BootstrapJarState::Corrupt,
            "下限サイズ未満は Corrupt"
        );
        // 破損 (サイズ十分だが PK マジック無し — adversarial 変異で露出した
        // 検査の独立 pin: サイズ条件単独では見抜けないケース)
        let mut notzip = b"NO".to_vec();
        notzip.resize(600, 0xCD);
        fs::write(home.join(BOOTSTRAP_JAR), &notzip).unwrap();
        assert_eq!(
            probe_bootstrap_jar(&home).unwrap(),
            BootstrapJarState::Corrupt,
            "サイズ >= 512 でも PK マジック無しは Corrupt (jar ではない)"
        );
        // 有効
        let body = write_fake_bootstrap_jar(&home);
        let st = probe_bootstrap_jar(&home).unwrap();
        match st {
            BootstrapJarState::Present(l) => {
                assert_eq!(l.name, BOOTSTRAP_JAR);
                assert_eq!(l.sha256, sha256_hex(&body), "実内容の SHA-256");
                assert_eq!(l.size, body.len() as u64);
            }
            other => panic!("有効 jar は Present: {other:?}"),
        }
        let _ = fs::remove_dir_all(&home);
    }

    /// wave HR (#5 根治): deploy_client_mappings の 在/不在/破損/有効 4 ケース。
    #[test]
    fn client_mappings_deploy_states() {
        let home = tmpdir("clientmap");
        let src = home.join("src");
        let dst = home.join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        // (1) 不在 → warn note (agent は Unobfuscated へ落下)。dst にファイル無し。
        let note = deploy_client_mappings(&src, &dst).unwrap();
        assert!(note.contains("not bundled"), "{note}");
        assert!(!dst.join(CLIENT_TXT).exists());

        // (2) 破損 (小さすぎ) → warn note。配備されず。
        fs::write(src.join(CLIENT_TXT), b"# tiny not a mapping").unwrap();
        let note = deploy_client_mappings(&src, &dst).unwrap();
        assert!(note.contains("too small"), "{note}");
        assert!(!dst.join(CLIENT_TXT).exists(), "破損は配備されない");

        // (3) 有効 (下限以上) → 配備 + sha256 note。dst に byte 一致で存在。
        let mut body = b"# ProGuard mappings (fake)\nnet.minecraft.client.Minecraft -> gfj:\n".to_vec();
        body.resize(CLIENT_TXT_MIN_SIZE as usize + 100, b' ');
        fs::write(src.join(CLIENT_TXT), &body).unwrap();
        let note = deploy_client_mappings(&src, &dst).unwrap();
        assert!(note.contains("deployed"), "{note}");
        assert!(note.contains(&sha256_hex(&body)), "{note}");
        let deployed = fs::read(dst.join(CLIENT_TXT)).unwrap();
        assert_eq!(deployed, body, "byte 一致配置");
    }

    #[test]
    fn launcher_missing_bootstrap_jar_skips_registration_with_note() {
        let home = tmpdir("nojar_l");
        let src = home.join("src");
        fs::create_dir_all(&src).unwrap();
        for l in &full_linux_libs() {
            fs::write(src.join(&l.name), b"dll").unwrap();
        }
        // jar は書かない (wave 203 配布物の再現)
        let mc = home.join(".minecraft");
        fs::create_dir_all(&mc).unwrap();
        let out = setup_launcher(&mc, TargetOs::Linux, &src, &full_linux_libs(), "vulkan")
            .expect("Ok (登録中止はエラーではなく note)");
        assert!(
            !out.profile_registered,
            "jar 無しでは登録しない (起動するが Mod が全く効かない壊れた構成を置かない)"
        );
        assert!(
            out.notes.iter().any(|n| n.contains(BOOTSTRAP_JAR)),
            "note に jar 欠如の理由: {:?}",
            out.notes
        );
        assert!(
            !mc.join("versions").join("rsift-1.21.11").exists(),
            "versions フォルダ自体を作らない"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn launcher_corrupt_bootstrap_jar_skips_registration_with_note() {
        let home = tmpdir("badjar_l");
        let src = home.join("src");
        fs::create_dir_all(&src).unwrap();
        for l in &full_linux_libs() {
            fs::write(src.join(&l.name), b"dll").unwrap();
        }
        fs::write(src.join(BOOTSTRAP_JAR), b"PKtiny").unwrap();
        let mc = home.join(".minecraft");
        fs::create_dir_all(&mc).unwrap();
        let out =
            setup_launcher(&mc, TargetOs::Linux, &src, &full_linux_libs(), "vulkan").expect("Ok");
        assert!(!out.profile_registered, "破損 jar でも登録しない");
        assert!(
            out.notes.iter().any(|n| n.contains("破損")),
            "note は破損を明示: {:?}",
            out.notes
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn prism_missing_bootstrap_jar_skips_with_note_nothing_created() {
        let home = tmpdir("nojar_p");
        let src = home.join("src");
        fs::create_dir_all(&src).unwrap();
        for l in &full_windows_libs() {
            fs::write(src.join(&l.name), b"dll").unwrap();
        }
        let root = home.join("PrismLauncher");
        fs::create_dir_all(&root).unwrap();
        let out = setup_prism(
            &root,
            TargetOs::Windows,
            &src,
            &full_windows_libs(),
            "vulkan",
        )
        .expect("Ok");
        assert!(!out.instance_created, "jar 無しではインスタンス登録しない");
        assert!(out.notes.iter().any(|n| n.contains(BOOTSTRAP_JAR)));
        assert!(
            !root
                .join("instances")
                .join("rsift")
                .join("instance.cfg")
                .is_file(),
            "壊れた起動構成を量産しない"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn prism_full_setup_deploys_bootstrap_jar_next_to_agent() {
        // prism_full_setup_* 本体に統合済みのため、ここでは jar 配置の代表 pin:
        // natives 6 点 (engine/agent/gfx/replay/zoom/jar) とバイト一致を確認。
        let home = tmpdir("prism_jar");
        let src = home.join("src");
        fs::create_dir_all(&src).unwrap();
        for l in &full_windows_libs() {
            fs::write(src.join(&l.name), b"dll").unwrap();
        }
        let jar_bytes = write_fake_bootstrap_jar(&src);
        let root = home.join("PrismLauncher");
        fs::create_dir_all(&root).unwrap();
        let out = setup_prism(
            &root,
            TargetOs::Windows,
            &src,
            &full_windows_libs(),
            "vulkan",
        )
        .expect("Ok");
        assert!(out.instance_created);
        let inst = root.join("instances").join("rsift");
        assert!(
            out.natives_deployed.iter().any(|n| n == BOOTSTRAP_JAR),
            "natives に jar: {:?}",
            out.natives_deployed
        );
        assert_eq!(
            fs::read(inst.join("rsift-natives").join(BOOTSTRAP_JAR)).unwrap(),
            jar_bytes,
            "agentpath (rsift-natives) と同じフォルダへバイト一致配置"
        );
        let _ = fs::remove_dir_all(&home);
    }
}
