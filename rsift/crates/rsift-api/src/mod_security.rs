//! # Rsift Mod Security — Mod ハイジャック防止の capability セキュリティ層
//!
//! 第三者製 Mod がユーザーの PC を乗っ取る (プロセス起動・機密ファイル窃取・
//! 外部ホストへの情報流出・永続化・キーロギング) のを防ぐ多層防御。
//!
//! ## 脅威モデル (誠実な境界)
//!
//! Rsift のネイティブ Mod はプロセス内にロードされる DLL/so/dylib であり、
//! **ロード後のネイティブコードを OS 級に制限することは不可能** (WinAPI/libc
//! を直接叩ける)。したがって防御はロード前に集中する:
//!
//! 1. **ロード前静的検査** (`vet_bytes`): Mod バイナリの import 表 (PE 構造
//!    パース) と文字列シグネチャを独自走査し、ホスト危険プリミティブの使用を
//!    能力単位で検出する。プロセスインジェクション連鎖 (CreateRemoteThread +
//!    WriteProcessMemory 等) や既知の窃取ツール語彙は即時 **Deny**。
//! 2. **capability + 同意ストア**: 検出された危険能力は `mod_perms.json`
//!    (SHA-256 ハッシュ固定) でユーザーが明示承認したもののみ付与される。
//!    未承認ならロード自体を拒否 (サイレント乗っ取りの根絶)。
//! 3. **ランタイムゲート** (`SecurityGate::require`): rsift ブリッジ側で
//!    mod 帰属の危険操作を capability 検査する。拒否は全件監査ログ化。
//! 4. **WASM 隔離ティア**: `rsift-jvm::wasm_sandbox` はゼロ信頼 Mod 向けの
//!    最小能力ティアとして本層と併存する (未知配布元 Mod は WASM 化が安全)。
//!
//! ## 「作れる Mod の幅」を狭めない保証
//!
//! - 危険能力を一切使わない Mod (通常の worldgen / rendering / networking /
//!   registry / event / GUI 系 = Mod 生態系の大半) は **ゼロ摩擦で従来どおり
//!   ロードされる** (検出 0 → 即 Allow)。ゲーム API 表面 (SuiteModule 42
//!   モジュール・packet/render/tick/channel dispatch) は capability 不要で
//!   拘束しない (回帰 pin: `gq_breadth_lock_*`)。
//! - 危険能力が必要な Mod も **原理上禁止はしない**。必要能力を検出して
//!   ユーザーに提示し、承認されれば同一能力でロードされる。ブラウザ拡張の
//!   権限モデルと同じく「無断」だけを殺し「幅」は殺さない。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use tracing::{debug, info, warn};

// ============================================================================
// ホスト危険能力 (HostCapability) — ゲーム API ではなく「ユーザーの PC」に触れる操作
// ============================================================================

/// Mod がホスト機に対して行使しうる危険プリミティブ。既定では不許可で、
/// ユーザー同意 (consent store) でのみ付与される。ゲーム Mod API 全表面は
/// これらを必要としない (幅不変の根拠)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HostCapability {
    /// 外部プロセスの起動/制御 (CreateProcess, cmd.exe, powershell, /bin/sh)
    ProcessExec,
    /// ゲームディレクトリ外へのファイル書き込み
    FileWriteOutsideScope,
    /// 機密ファイル読取 (SSH 鍵・ブラウザ資格情報・他ランチャのセッション)
    FileReadSensitive,
    /// 任意ホストへの外向き通信 (流出経路)。ゲームのマルチプレイ通信は含まない
    NetworkEgress,
    /// 追加ネイティブモジュールの動的ロード (LoadLibrary/dlopen 連鎖)
    NativeModuleLoad,
    /// JVM 計装 (クラス再変換・注入) — Mod 同士/本体の改竄に転用可能
    JvmInstrumentation,
    /// グローバル入力/画面の捕捉 (キーロガー・スクリーンキャプチャ)
    InputCapture,
    /// 自動起動の永続化 (レジストリ Run キー・LaunchAgents・cron)
    PersistenceAutostart,
}

impl HostCapability {
    pub const ALL: &'static [HostCapability] = &[
        Self::ProcessExec,
        Self::FileWriteOutsideScope,
        Self::FileReadSensitive,
        Self::NetworkEgress,
        Self::NativeModuleLoad,
        Self::JvmInstrumentation,
        Self::InputCapture,
        Self::PersistenceAutostart,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProcessExec => "process_exec",
            Self::FileWriteOutsideScope => "file_write_outside_scope",
            Self::FileReadSensitive => "file_read_sensitive",
            Self::NetworkEgress => "network_egress",
            Self::NativeModuleLoad => "native_module_load",
            Self::JvmInstrumentation => "jvm_instrumentation",
            Self::InputCapture => "input_capture",
            Self::PersistenceAutostart => "persistence_autostart",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "process_exec" => Some(Self::ProcessExec),
            "file_write_outside_scope" => Some(Self::FileWriteOutsideScope),
            "file_read_sensitive" => Some(Self::FileReadSensitive),
            "network_egress" => Some(Self::NetworkEgress),
            "native_module_load" => Some(Self::NativeModuleLoad),
            "jvm_instrumentation" => Some(Self::JvmInstrumentation),
            "input_capture" => Some(Self::InputCapture),
            "persistence_autostart" => Some(Self::PersistenceAutostart),
            _ => None,
        }
    }

    /// ユーザー提示用の日本語ラベル (同意 UI / ログ)。
    pub fn label_ja(self) -> &'static str {
        match self {
            Self::ProcessExec => "外部プログラムの起動",
            Self::FileWriteOutsideScope => "ゲーム外フォルダへの書き込み",
            Self::FileReadSensitive => "機密ファイル (パスワード/鍵) の読み取り",
            Self::NetworkEgress => "ゲームサーバー以外への通信",
            Self::NativeModuleLoad => "追加ネイティブモジュールの読み込み",
            Self::JvmInstrumentation => "ゲームプログラム自体の書き換え",
            Self::InputCapture => "キーボード/画面の監視",
            Self::PersistenceAutostart => "PC 起動時の自動実行の登録",
        }
    }
}

// ============================================================================
// シグネチャ表 (import / 文字列 / 即時拒否)
// ============================================================================

/// import 走査シグネチャ: (dll 部分文字列, symbol 部分文字列, 能力, 根拠注記)。
/// 空文字列はワイルドカード。比較は双方小文字化の部分一致。
const IMPORT_SIGS: &[(&str, &str, HostCapability, &str)] = &[
    (
        "kernel32",
        "createprocess",
        HostCapability::ProcessExec,
        "CreateProcess",
    ),
    (
        "kernel32",
        "winexec",
        HostCapability::ProcessExec,
        "WinExec",
    ),
    (
        "shell32",
        "shellexecute",
        HostCapability::ProcessExec,
        "ShellExecute",
    ),
    (
        "advapi32",
        "createprocessasuser",
        HostCapability::ProcessExec,
        "CreateProcessAsUser",
    ),
    (
        "kernel32",
        "loadlibrary",
        HostCapability::NativeModuleLoad,
        "LoadLibrary",
    ),
    ("ws2_32", "", HostCapability::NetworkEgress, "Winsock2"),
    ("wsock32", "", HostCapability::NetworkEgress, "Winsock"),
    ("wininet", "", HostCapability::NetworkEgress, "WinINet"),
    ("winhttp", "", HostCapability::NetworkEgress, "WinHTTP"),
    (
        "urlmon",
        "urldownload",
        HostCapability::NetworkEgress,
        "URLDownload",
    ),
    (
        "advapi32",
        "regsetvalue",
        HostCapability::PersistenceAutostart,
        "registry write",
    ),
    (
        "advapi32",
        "regcreatekey",
        HostCapability::PersistenceAutostart,
        "registry key create",
    ),
    (
        "user32",
        "setwindowshookex",
        HostCapability::InputCapture,
        "global hook",
    ),
    (
        "user32",
        "getasynckeystate",
        HostCapability::InputCapture,
        "key state polling",
    ),
];

/// 全フォーマット横断の文字列シグネチャ (小文字化したバイト列の部分一致)。
/// ELF/Mach-O の動的シンボル名も文字列として現れるため import 解析を補完する。
const STRING_SIGS: &[(&str, HostCapability, &str)] = &[
    ("powershell", HostCapability::ProcessExec, "PowerShell 呼出"),
    ("cmd.exe /c", HostCapability::ProcessExec, "cmd /c 呼出"),
    ("/bin/sh -c", HostCapability::ProcessExec, "sh -c 呼出"),
    ("osascript", HostCapability::ProcessExec, "AppleScript 実行"),
    ("rundll32", HostCapability::ProcessExec, "rundll32 起動"),
    ("regsvr32", HostCapability::ProcessExec, "regsvr32 登録実行"),
    ("mshta", HostCapability::ProcessExec, "mshta 実行"),
    ("dlopen", HostCapability::NativeModuleLoad, "dlopen 連鎖"),
    ("/.ssh/id_", HostCapability::FileReadSensitive, "SSH 秘密鍵"),
    (
        "\\.ssh\\id_",
        HostCapability::FileReadSensitive,
        "SSH 秘密鍵 (Windows)",
    ),
    (
        ".aws/credentials",
        HostCapability::FileReadSensitive,
        "AWS 資格情報",
    ),
    (
        "launcher_accounts.json",
        HostCapability::FileReadSensitive,
        "Minecraft アカウント資格",
    ),
    (
        "launcher_profiles.json",
        HostCapability::FileReadSensitive,
        "Minecraft プロファイル資格",
    ),
    (
        "\\google\\chrome\\user data",
        HostCapability::FileReadSensitive,
        "Chrome プロファイル (資格情報)",
    ),
    (
        "/.config/google-chrome",
        HostCapability::FileReadSensitive,
        "Chrome プロファイル (Linux)",
    ),
    (
        "\\mozilla\\firefox\\profiles",
        HostCapability::FileReadSensitive,
        "Firefox プロファイル",
    ),
    (
        "currentversion\\run",
        HostCapability::PersistenceAutostart,
        "レジストリ自動起動キー",
    ),
    (
        "launchagents",
        HostCapability::PersistenceAutostart,
        "macOS 自動起動",
    ),
    (
        "systemd/user",
        HostCapability::PersistenceAutostart,
        "systemd user unit",
    ),
    ("crontab", HostCapability::PersistenceAutostart, "cron 登録"),
    (
        "getasynckeystate",
        HostCapability::InputCapture,
        "キー状態ポーリング (文字列表出)",
    ),
    (
        "setwindowshookex",
        HostCapability::InputCapture,
        "グローバルフック (文字列表出)",
    ),
];

/// 即時拒否 (協議の余地なし): 認証情報窃取ツール・マルウェア語彙。
const HARD_DENY_STRINGS: &[(&str, &str)] = &[
    ("mimikatz", "認証情報窃取ツール mimikatz"),
    ("sekurlsa::", "mimikatz sekurlsa モジュール"),
    ("lsass.exe", "LSASS 資格情報ダンプ対象"),
    ("meterpreter", "meterpreter ペイロード語彙"),
    ("reflectivedllinjection", "reflective DLL injection"),
];

/// プロセスインジェクション連鎖語彙 — 2 種以上の共起で Deny。
/// (単独 LoadLibrary/WriteFile 等は良性にも出るが、他プロセス操作系の
/// 共起はインジェクション実用連鎖として機械判定する)
const TAMPER_CHAIN_NEEDLES: &[&str] = &[
    "createremotethread",
    "writeprocessmemory",
    "virtualallocex",
    "openprocess",
    "queueuserapc",
    "ntcreatethreadex",
    "setthreadcontext",
];

/// 文字列走査の打ち切り (巨大 Mod でも解析コストを一定化)
const STRING_SCAN_LIMIT: usize = 64 * 1024 * 1024;

// ============================================================================
// SHA-256 (同意ハッシュ固定用・自前実装 / NIST ベクタ検証)
// ============================================================================

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 ダイジェスト (FIPS 180-4)? 同意ストアの Mod ファイル同一性固定に使用。
pub fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
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
                .wrapping_add(SHA256_K[i])
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
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// ダイジェストの小文字 hex 表現。
pub fn sha256_hex(data: &[u8]) -> String {
    sha256_digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ============================================================================
// PE import テーブル・パーサ (境界検査済み手書き / Windows DLL 第一層)
// ============================================================================

fn rd_u16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(o)?, *b.get(o + 1)?]))
}

fn rd_u32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(o)?,
        *b.get(o + 1)?,
        *b.get(o + 2)?,
        *b.get(o + 3)?,
    ]))
}

fn cstr_at(b: &[u8], o: usize, max: usize) -> Option<String> {
    if o >= b.len() {
        return None;
    }
    let end = (o + max).min(b.len());
    let slice = &b[o..end];
    let nul = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
    if nul == 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&slice[..nul]).into_owned())
}

struct PeSection {
    va: u32,
    raw_off: u32,
    span: u32,
}

fn rva_to_off(sections: &[PeSection], rva: u32) -> Option<usize> {
    sections
        .iter()
        .find(|s| rva >= s.va && rva < s.va.saturating_add(s.span))
        .map(|s| (rva - s.va + s.raw_off) as usize)
}

/// PE (Windows DLL) の import 記述子を走査し (dll, symbol) 一覧を返す。
/// ordinal import は `<ordinal:N>` 形式で記録。壊れたバイナリは Err で
/// 上位の fail-closed 判定に委ねる (panic しないこと)。
pub fn pe_imports(b: &[u8]) -> Result<Vec<(String, String)>, String> {
    if b.len() < 0x40 || &b[0..2] != b"MZ" {
        return Err("not a PE (MZ header missing)".into());
    }
    let pe_off = rd_u32(b, 0x3C).ok_or("e_lfanew truncated")? as usize;
    if b.get(pe_off..pe_off + 4) != Some(b"PE\0\0") {
        return Err("PE signature missing".into());
    }
    let coff = pe_off + 4;
    let nsec = rd_u16(b, coff + 2).ok_or("COFF truncated")? as usize;
    let optsz = rd_u16(b, coff + 16).ok_or("COFF opt size truncated")? as usize;
    let opt = coff + 20;
    if opt + optsz > b.len() {
        return Err("optional header truncated".into());
    }
    let magic = rd_u16(b, opt).ok_or("magic truncated")?;
    let (dd_base, is_pe64) = match magic {
        0x10B => (opt + 96, false),
        0x20B => (opt + 112, true),
        other => return Err(format!("unknown PE magic {other:#x}")),
    };
    let n_dirs = rd_u32(b, dd_base - 4).unwrap_or(0);
    if n_dirs < 2 {
        return Ok(Vec::new()); // import directory なし (dllmain のみ等)
    }
    let imp_rva = rd_u32(b, dd_base + 8).ok_or("import dir truncated")?;
    if imp_rva == 0 {
        return Ok(Vec::new());
    }
    let mut sections = Vec::with_capacity(nsec.min(96));
    for i in 0..nsec.min(96) {
        let s = opt + optsz + i * 40;
        let va = rd_u32(b, s + 12).ok_or("section va truncated")?;
        let vsize = rd_u32(b, s + 8).ok_or("section vsize truncated")?;
        let raw_size = rd_u32(b, s + 16).ok_or("section raw truncated")?;
        let raw_off = rd_u32(b, s + 20).ok_or("section raw off truncated")?;
        sections.push(PeSection {
            va,
            raw_off,
            span: vsize.max(raw_size),
        });
    }
    let thunk_size = if is_pe64 { 8usize } else { 4 };
    let ordinal_mask: u64 = if is_pe64 { 1u64 << 63 } else { 1u64 << 31 };
    let mut out = Vec::new();
    let mut d_off = rva_to_off(&sections, imp_rva).ok_or("import dir RVA unmappable")?;
    for _ in 0..4096 {
        let orig_thunk = rd_u32(b, d_off).ok_or("import descriptor truncated")?;
        let name_rva = rd_u32(b, d_off + 12).ok_or("import descriptor truncated")?;
        let first_thunk = rd_u32(b, d_off + 16).ok_or("import descriptor truncated")?;
        if orig_thunk == 0 && name_rva == 0 && first_thunk == 0 {
            break; // null descriptor = 終端
        }
        d_off += 20;
        let Some(name_off) = rva_to_off(&sections, name_rva) else {
            continue;
        };
        let Some(dll) = cstr_at(b, name_off, 256) else {
            continue;
        };
        let thunk_rva = if orig_thunk != 0 {
            orig_thunk
        } else {
            first_thunk
        };
        let Some(mut t_off) = rva_to_off(&sections, thunk_rva) else {
            continue;
        };
        for _ in 0..65536 {
            let val = if is_pe64 {
                let lo = rd_u32(b, t_off).ok_or("thunk truncated")? as u64;
                let hi = rd_u32(b, t_off + 4).ok_or("thunk truncated")? as u64;
                (hi << 32) | lo
            } else {
                rd_u32(b, t_off).ok_or("thunk truncated")? as u64
            };
            t_off += thunk_size;
            if val == 0 {
                break;
            }
            if val & ordinal_mask != 0 {
                out.push((dll.clone(), format!("<ordinal:{}>", val & 0xFFFF)));
                continue;
            }
            let Some(hn_off) = rva_to_off(&sections, val as u32) else {
                continue;
            };
            if let Some(sym) = cstr_at(b, hn_off + 2, 256) {
                out.push((dll.clone(), sym));
            }
        }
    }
    Ok(out)
}

// ============================================================================
// 検査レポート (VetReport) と判定 (LoadVerdict)
// ============================================================================

/// バイナリフォーマット検出 (走査戦略の分岐に使用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryFormat {
    Pe,
    Elf,
    MachO,
    Unknown,
}

/// `vet_bytes` の機械レポート。判定の根拠 (evidence) を全件保持する。
#[derive(Debug, Clone, Default)]
pub struct VetReport {
    pub format: Option<BinaryFormat>,
    pub imports_found: usize,
    pub capabilities_detected: BTreeSet<HostCapability>,
    pub evidence: Vec<String>,
    pub hard_deny_reasons: Vec<String>,
}

fn detect_format(b: &[u8]) -> BinaryFormat {
    if b.len() >= 2 && &b[0..2] == b"MZ" {
        BinaryFormat::Pe
    } else if b.len() >= 4 && &b[0..4] == b"\x7fELF" {
        BinaryFormat::Elf
    } else if b.len() >= 4 {
        match u32::from_le_bytes([b[0], b[1], b[2], b[3]]) {
            0xFEEDFACE | 0xFEEDFACF | 0xCAFEBABE | 0xCEFAEDFE | 0xCFFAEDFE | 0xBEBAFECA => {
                BinaryFormat::MachO
            }
            _ => BinaryFormat::Unknown,
        }
    } else {
        BinaryFormat::Unknown
    }
}

/// Mod バイナリの静的検査 — ホスト危険能力の使用を証拠つきで検出する。
/// ファイル読み込みは行わない (純粋関数 / 並行テスト安全)。
pub fn vet_bytes(b: &[u8]) -> VetReport {
    let mut r = VetReport {
        format: Some(detect_format(b)),
        ..VetReport::default()
    };
    // 第 1 層: PE import 構造走査 (Windows DLL の高精度分類)
    if r.format == Some(BinaryFormat::Pe) {
        match pe_imports(b) {
            Ok(imports) => {
                r.imports_found = imports.len();
                for (dll, sym) in &imports {
                    let dl = dll.to_ascii_lowercase();
                    let sl = sym.to_ascii_lowercase();
                    for (dpat, spat, cap, note) in IMPORT_SIGS {
                        if (dpat.is_empty() || dl.contains(dpat))
                            && (spat.is_empty() || sl.contains(spat))
                        {
                            r.capabilities_detected.insert(*cap);
                            r.evidence.push(format!("PE import {dll}!{sym} ≈ {note}"));
                        }
                    }
                }
            }
            Err(e) => {
                // PE なのに import 解析不能 = 不正/難読化の可能性。fail-closed:
                // 拒否ではなく「構造異常」証拠としてユーザー協議に回す。
                r.evidence.push(format!("PE import parse failed: {e}"));
                r.capabilities_detected
                    .insert(HostCapability::NativeModuleLoad);
            }
        }
    }
    // 第 2 層: 全フォーマット横断の文字列シグネチャ走査
    let scan = &b[..b.len().min(STRING_SCAN_LIMIT)];
    let lower = String::from_utf8_lossy(scan).to_lowercase();
    for (needle, cap, note) in STRING_SIGS {
        if lower.contains(needle) {
            r.capabilities_detected.insert(*cap);
            r.evidence.push(format!("string \"{needle}\" ≈ {note}"));
        }
    }
    for (needle, reason) in HARD_DENY_STRINGS {
        if lower.contains(needle) {
            r.hard_deny_reasons.push(format!("{reason} (\"{needle}\")"));
        }
    }
    let chain_hits: Vec<&str> = TAMPER_CHAIN_NEEDLES
        .iter()
        .copied()
        .filter(|n| lower.contains(n))
        .collect();
    if chain_hits.len() >= 2 {
        r.hard_deny_reasons.push(format!(
            "process-injection chain: {} 語彙共起 [{}]",
            chain_hits.len(),
            chain_hits.join(", ")
        ));
    }
    r
}

/// ロード判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadVerdict {
    /// ロード許可 — 付与された危険能力 (least-privilege: 検出分のみ)。
    Allow(BTreeSet<HostCapability>),
    /// ユーザー同意が必要 — 不足能力の一覧。対処: mod_perms.json で承認。
    RequireConsent(Vec<HostCapability>),
    /// 即時拒否 — 理由一覧 (rootkit/窃取ツール語彙)。
    Deny(Vec<String>),
}

impl LoadVerdict {
    pub fn is_allow(&self) -> bool {
        matches!(self, LoadVerdict::Allow(_))
    }
}

// ============================================================================
// capability マニフェスト (sidecar) と同意ストア (consent store)
// ============================================================================

/// Mod 開発者同梱の希望 capability 宣言 (`<mod-id>.rsift-perms.json`)。
/// 判定の承認根拠にはならず (ユーザー同意のみが根拠)、未承認時の
/// scaffold 提示に使われる。
#[derive(Debug, Clone, Default)]
pub struct CapabilityManifest {
    pub declared: BTreeSet<HostCapability>,
}

impl CapabilityManifest {
    pub fn from_json_str(s: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(s).map_err(|e| format!("manifest JSON parse: {e}"))?;
        let arr = v
            .get("capabilities")
            .and_then(|c| c.as_array())
            .ok_or("manifest must contain \"capabilities\" array")?;
        let mut declared = BTreeSet::new();
        for item in arr {
            let name = item.as_str().ok_or("capability entries must be strings")?;
            let cap = HostCapability::from_str(name).ok_or_else(|| {
                format!("unknown capability \"{name}\" (typo では黙って無力化するため明示拒否)")
            })?;
            declared.insert(cap);
        }
        Ok(Self { declared })
    }

    /// sidecar の規約パス: `<stem>.dll` → `<stem>.rsift-perms.json`
    pub fn sidecar_path_for(dll_path: &Path) -> PathBuf {
        let mut p = dll_path.to_path_buf();
        p.set_extension("rsift-perms.json");
        p
    }

    pub fn load_sidecar(dll_path: &Path) -> (Option<Self>, Option<String>) {
        let p = Self::sidecar_path_for(dll_path);
        match std::fs::read_to_string(&p) {
            Ok(s) => match Self::from_json_str(&s) {
                Ok(m) => (Some(m), None),
                Err(e) => (None, Some(format!("{e} (sidecar: {p:?})"))),
            },
            Err(_) => (None, None), // 同梱なしは正常 (宣言不要)
        }
    }
}

/// `mod_perms.json` の 1 Mod 分の同意レコード。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsentRecord {
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub granted: Vec<String>,
    /// 検査で要求されたが未承認の能力 (UI が提示するための scaffold)。
    #[serde(default)]
    pub requested: Vec<String>,
    #[serde(default)]
    pub denied: Vec<String>,
}

/// ユーザー同意ストア — `<mod_dir>/.rsift_mod_perms.json`。
/// Mod ファイルの SHA-256 と能力付与を対に保存し、ロード時にハッシュ照合する
/// (Mod の差替え = ハッシュ変化 = 同意失効、で再バイナリ化のすり替えを防ぐ)。
#[derive(Debug, Clone, Default)]
pub struct ConsentStore {
    pub mods: BTreeMap<String, ConsentRecord>,
}

/// 同意ストアの規約ファイル名 (Mod ディレクトリ直下の隠しファイル)。
pub const CONSENT_FILE_NAME: &str = ".rsift_mod_perms.json";

impl ConsentStore {
    pub fn consent_path_for(dll_path: &Path) -> PathBuf {
        dll_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(CONSENT_FILE_NAME)
    }

    /// 読み込み。ファイル不在は正常 (空)。JSON 破損は fail-closed で空に
    /// フォールバックし、理由を第 2 戻り値で報告する。
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(s) => match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(v) => {
                    let mut mods = BTreeMap::new();
                    if let Some(obj) = v.get("mods").and_then(|m| m.as_object()) {
                        for (id, rec) in obj {
                            match serde_json::from_value::<ConsentRecord>(rec.clone()) {
                                Ok(r) => {
                                    mods.insert(id.clone(), r);
                                }
                                Err(e) => {
                                    return (
                                        Self::default(),
                                        Some(format!("consent record for \"{id}\" corrupt: {e}")),
                                    );
                                }
                            }
                        }
                    }
                    (Self { mods }, None)
                }
                Err(e) => (Self::default(), Some(format!("consent JSON parse: {e}"))),
            },
            Err(_) => (Self::default(), None),
        }
    }

    /// 保存 (pretty JSON、ユーザー直接編集を前提)。
    pub fn write_json(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct Root<'a> {
            mods: &'a BTreeMap<String, ConsentRecord>,
        }
        serde_json::to_string_pretty(&Root { mods: &self.mods }).map_err(|e| e.to_string())
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let s = self.write_json()?;
        std::fs::write(path, s + "\n").map_err(|e| e.to_string())
    }

    /// ハッシュ照合つきの同意済み能力集合。レコード不在・ハッシュ不一致
    /// (Mod 差替え) は None = 同意なし。未知能力名・denied 指定は除外する。
    pub fn granted_caps_for(
        &self,
        mod_id: &str,
        file_sha256_hex: &str,
    ) -> BTreeSet<HostCapability> {
        let Some(rec) = self.mods.get(mod_id) else {
            return BTreeSet::new();
        };
        if rec.sha256.is_empty() || !rec.sha256.eq_ignore_ascii_case(file_sha256_hex) {
            return BTreeSet::new(); // 差替え検出 = 同意失効
        }
        let denied: BTreeSet<HostCapability> = rec
            .denied
            .iter()
            .filter_map(|s| HostCapability::from_str(s))
            .collect();
        rec.granted
            .iter()
            .filter_map(|s| HostCapability::from_str(s))
            .filter(|c| !denied.contains(c))
            .collect()
    }
}

// ============================================================================
// vet_mod_binary — ローダー統合入口 (検査→同意照合→判定)
// ============================================================================

/// 判定結果一式 (ローダーがエラーメッセージと scaffold に使う)。
#[derive(Debug, Clone)]
pub struct VetOutcome {
    pub report: VetReport,
    pub verdict: LoadVerdict,
    pub file_sha256_hex: String,
    pub consent_path: PathBuf,
    /// sidecar 宣言 (未承認時 scaffold の提示用)
    pub declared_by_manifest: BTreeSet<HostCapability>,
    /// 同意 JSON 破損などの fail-closed 注記
    pub notes: Vec<String>,
}

/// Mod バイナリを読み、検査して同意ストアと照合し判定を返す。
/// ロード可否の最終決定は呼び出し側 (native_loader) が行う。
pub fn vet_mod_binary(mod_id: &str, dll_path: &Path) -> Result<VetOutcome, String> {
    let bytes = std::fs::read(dll_path).map_err(|e| format!("[modsec] read {dll_path:?}: {e}"))?;
    let hex = sha256_hex(&bytes);
    let report = vet_bytes(&bytes);
    let consent_path = ConsentStore::consent_path_for(dll_path);
    let (store, consent_note) = ConsentStore::load(&consent_path);
    let (manifest, manifest_note) = CapabilityManifest::load_sidecar(dll_path);
    let declared = manifest.map(|m| m.declared).unwrap_or_default();
    let mut notes = Vec::new();
    if let Some(n) = consent_note {
        notes.push(n);
    }
    if let Some(n) = manifest_note {
        notes.push(n);
    }

    let verdict = decide(&report, &store, mod_id, &hex);
    debug!(
        "[modsec] vet {mod_id}: fmt={:?} imports={} caps={:?} hard={} verdict={}",
        report.format,
        report.imports_found,
        report.capabilities_detected,
        report.hard_deny_reasons.len(),
        if verdict.is_allow() {
            "allow"
        } else {
            "blocked"
        }
    );
    Ok(VetOutcome {
        report,
        verdict,
        file_sha256_hex: hex,
        consent_path,
        declared_by_manifest: declared,
        notes,
    })
}

fn decide(
    report: &VetReport,
    store: &ConsentStore,
    mod_id: &str,
    file_sha256_hex: &str,
) -> LoadVerdict {
    if !report.hard_deny_reasons.is_empty() {
        return LoadVerdict::Deny(report.hard_deny_reasons.clone());
    }
    let detected = &report.capabilities_detected;
    if detected.is_empty() {
        return LoadVerdict::Allow(BTreeSet::new());
    }
    let consented = store.granted_caps_for(mod_id, file_sha256_hex);
    let missing: Vec<HostCapability> = detected.difference(&consented).copied().collect();
    if missing.is_empty() {
        // least-privilege: 検出分のみ付与 (過剰付与を構造排除)
        LoadVerdict::Allow(detected.clone())
    } else {
        LoadVerdict::RequireConsent(missing)
    }
}

/// 未承認判定時に consent store へ scaffold (hash + requested) を書き込み、
/// ユーザーが `granted` へ移すだけで承認できる状態にする。
pub fn ensure_pending_entry(outcome: &VetOutcome, mod_id: &str) {
    let (mut store, _) = ConsentStore::load(&outcome.consent_path);
    let rec = store.mods.entry(mod_id.to_string()).or_default();
    rec.sha256 = outcome.file_sha256_hex.clone();
    let mut requested: BTreeSet<String> = rec.requested.iter().cloned().collect();
    for cap in &outcome.report.capabilities_detected {
        requested.insert(cap.as_str().to_string());
    }
    for cap in &outcome.declared_by_manifest {
        requested.insert(cap.as_str().to_string());
    }
    if let LoadVerdict::RequireConsent(missing) = &outcome.verdict {
        // 未承認分を先頭へ (ユーザーが見るべき集合)
        let mut v: Vec<String> = missing.iter().map(|c| c.as_str().to_string()).collect();
        for s in requested {
            if !v.contains(&s) {
                v.push(s);
            }
        }
        rec.requested = v;
    } else {
        rec.requested = requested.into_iter().collect();
    }
    if let Err(e) = store.save(&outcome.consent_path) {
        warn!(
            "[modsec] consent scaffold write failed {:?}: {e}",
            outcome.consent_path
        );
    }
}

// ============================================================================
// ランタイムゲート (SecurityGate) — mod 帰属の危険操作の最終チェック + 監査
// ============================================================================

/// ゲート拒否エラー (mod 帰属の危険操作が capability 不足で止められた)。
#[derive(Debug, Clone)]
pub struct SecurityDeny {
    pub mod_id: String,
    pub capability: HostCapability,
    pub reason: String,
}

impl std::fmt::Display for SecurityDeny {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[modsec] mod \"{}\" lacks capability {} ({}): {}",
            self.mod_id,
            self.capability.as_str(),
            self.capability.label_ja(),
            self.reason
        )
    }
}

impl std::error::Error for SecurityDeny {}

#[derive(Debug, Clone)]
struct AuditEvent {
    seq: u64,
    mod_id: String,
    capability: String,
    allowed: bool,
}

#[derive(Debug, Default)]
struct GateInner {
    profiles: BTreeMap<String, BTreeSet<HostCapability>>,
    events: VecDeque<AuditEvent>,
    next_seq: u64,
    checks: u64,
    allowed: u64,
    denied: u64,
    jni_host_refusals: u64,
}

const AUDIT_RING_CAPACITY: usize = 64;

static GATE: OnceLock<RwLock<GateInner>> = OnceLock::new();

fn gate() -> &'static RwLock<GateInner> {
    GATE.get_or_init(|| RwLock::new(GateInner::default()))
}

/// ゲート集計値 (観測用スナップショット)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateSnapshot {
    pub profiles: usize,
    pub checks: u64,
    pub allowed: u64,
    pub denied: u64,
    pub jni_host_refusals: u64,
}

/// グローバルセキュリティゲート。ローダーが Mod の許可能力を登録し、
/// rsift ブリッジが mod 帰属の危険操作のたびに `require` で検査する。
pub struct SecurityGate;

impl SecurityGate {
    /// Mod ロード成功時の能力登録 (least-privilege、上書き可)。
    pub fn register(mod_id: &str, caps: BTreeSet<HostCapability>) {
        if let Ok(mut g) = gate().write() {
            debug!("[modsec] register {mod_id}: {} capabilities", caps.len());
            g.profiles.insert(mod_id.to_string(), caps);
        }
    }

    /// Mod の登録済み能力 (再ロード/アンロード時の除去)。
    pub fn unregister(mod_id: &str) {
        if let Ok(mut g) = gate().write() {
            g.profiles.remove(mod_id);
        }
    }

    /// 登録能力の参照 (ローダーの検証 / 同意 UI)。
    pub fn granted_set(mod_id: &str) -> Option<BTreeSet<HostCapability>> {
        gate().read().ok()?.profiles.get(mod_id).cloned()
    }

    /// mod 帰属の危険操作の能力検査。拒否は監査リングに刻む。
    pub fn require(mod_id: &str, cap: HostCapability) -> Result<(), SecurityDeny> {
        let mut g = gate().write().map_err(|_| SecurityDeny {
            mod_id: mod_id.to_string(),
            capability: cap,
            reason: "gate lock poisoned".into(),
        })?;
        g.checks += 1;
        let ok = g
            .profiles
            .get(mod_id)
            .map(|caps| caps.contains(&cap))
            .unwrap_or(false);
        if ok {
            g.allowed += 1;
        } else {
            g.denied += 1;
        }
        g.next_seq += 1;
        let seq = g.next_seq;
        if g.events.len() >= AUDIT_RING_CAPACITY {
            g.events.pop_front();
        }
        g.events.push_back(AuditEvent {
            seq,
            mod_id: mod_id.to_string(),
            capability: cap.as_str().to_string(),
            allowed: ok,
        });
        if ok {
            Ok(())
        } else {
            let reason = if g.profiles.contains_key(mod_id) {
                "capability not granted"
            } else {
                "mod not registered (vetted before bridge use)"
            };
            warn!(
                "[modsec] DENY mod \"{mod_id}\" -> {} ({})",
                cap.as_str(),
                cap.label_ja()
            );
            Err(SecurityDeny {
                mod_id: mod_id.to_string(),
                capability: cap,
                reason: reason.into(),
            })
        }
    }

    /// JNI ブリッジからの `host.` 系 op 拒否を計数 (観測可能性)。
    pub fn note_jni_host_op_refusal(op: &str) {
        if let Ok(mut g) = gate().write() {
            g.jni_host_refusals += 1;
            g.next_seq += 1;
            let seq = g.next_seq;
            if g.events.len() >= AUDIT_RING_CAPACITY {
                g.events.pop_front();
            }
            g.events.push_back(AuditEvent {
                seq,
                mod_id: "<jni-op>".into(),
                capability: op.to_string(),
                allowed: false,
            });
        }
    }

    /// 集計スナップショット (テスト / ops 観測)。
    pub fn snapshot() -> GateSnapshot {
        let g = gate().read().map(|g| GateSnapshot {
            profiles: g.profiles.len(),
            checks: g.checks,
            allowed: g.allowed,
            denied: g.denied,
            jni_host_refusals: g.jni_host_refusals,
        });
        g.unwrap_or(GateSnapshot {
            profiles: 0,
            checks: 0,
            allowed: 0,
            denied: 0,
            jni_host_refusals: 0,
        })
    }

    /// 人間可読レポート (ロード完了時の要約ログ / ops 画面用)。
    pub fn report_string() -> String {
        let Ok(g) = gate().read() else {
            return "[modsec] gate unreadable".to_string();
        };
        let mut s = format!(
            "[modsec] profiles={} checks={} allowed={} denied={} jni_host_refusals={}",
            g.profiles.len(),
            g.checks,
            g.allowed,
            g.denied,
            g.jni_host_refusals
        );
        for e in g.events.iter().rev().take(8) {
            s.push_str(&format!(
                "\n  #{} {} {} {}",
                e.seq,
                if e.allowed { "allow" } else { "DENY " },
                e.mod_id,
                e.capability
            ));
        }
        s
    }
}

// ============================================================================
// JNI ブリッジの op 接頭辞ガード
// ============================================================================

/// JNI (`RsiftModBridge.nativeDispatch`) 到来の op のうち、ホスト特権系に
/// 予約する接頭辞。Java ブリッジは呼出 Mod を帰属できない (単一ネイティブ
/// エントリ) ため、ホスト特権 op はブリッジ経由では **構造的に不許可** とし、
/// 特権操作は capability 検査済みの Rust 側導線に限定する。既存 op
/// (packet/render/client_tick/channel) はゲーム API 表面であり無制限を維持。
pub const HOST_OP_PREFIX: &str = "host.";

pub fn jni_op_allowed(op: &str) -> bool {
    !op.starts_with(HOST_OP_PREFIX)
}

/// ロード完了時のセキュリティ要約ログ (native_loader 消費)。
pub fn log_load_summary() {
    info!("{}", SecurityGate::report_string());
}

// ============================================================================
// テスト (TDD: gq_ prefix / wave 195)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 小道具: 最小 PE (import descriptor 付き) の手組バイナリ ----
    // Layout: DOS(0x80) / PE hdr / COFF / optional(PE32, 0xE0) / 1 section (.idata) /
    // import descriptors / thunk / names — 0x1000 raw に全て置く簡略版。
    fn build_pe(dll: &str, syms: &[&str]) -> Vec<u8> {
        let mut b = vec![0u8; 0x1000];
        b[0] = b'M';
        b[1] = b'Z';
        b[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        // COFF: nsec=1, optsz=0xE0
        b[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        b[0x94..0x96].copy_from_slice(&0xE0u16.to_le_bytes());
        let opt = 0x98usize;
        b[opt..opt + 2].copy_from_slice(&0x10Bu16.to_le_bytes()); // PE32
        b[opt + 92..opt + 96].copy_from_slice(&16u32.to_le_bytes()); // n dirs
                                                                     // section .idata: RVA 0x1000 raw_off 0x400 span 0x600
        let sec = opt + 0xE0;
        b[sec..sec + 8].copy_from_slice(b".idata\0\0");
        b[sec + 8..sec + 12].copy_from_slice(&0x600u32.to_le_bytes()); // vsize
        b[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // va
        b[sec + 16..sec + 20].copy_from_slice(&0x600u32.to_le_bytes()); // raw size
        b[sec + 20..sec + 24].copy_from_slice(&0x400u32.to_le_bytes()); // raw off
                                                                        // import dir (data directory #1) → RVA 0x1000
        b[opt + 96 + 8..opt + 96 + 12].copy_from_slice(&0x1000u32.to_le_bytes());
        b[opt + 96 + 12..opt + 96 + 16].copy_from_slice(&0x100u32.to_le_bytes());
        // descriptor @ file 0x400 (RVA 0x1000): OriginalFirstThunk=0x1010, Name=0x1040, FirstThunk=0x1010
        let desc = 0x400usize;
        b[desc..desc + 4].copy_from_slice(&0x1010u32.to_le_bytes());
        b[desc + 12..desc + 16].copy_from_slice(&0x1040u32.to_le_bytes());
        b[desc + 16..desc + 20].copy_from_slice(&0x1010u32.to_le_bytes());
        // thunks @ RVA 0x1010 (file 0x410): syms 分の HintName RVA + 終端 0
        let mut name_rva = 0x1100u32;
        let mut name_file = 0x500usize;
        for (i, s) in syms.iter().enumerate() {
            let t = 0x410 + i * 4;
            b[t..t + 4].copy_from_slice(&name_rva.to_le_bytes());
            b[name_file..name_file + 2].copy_from_slice(&0u16.to_le_bytes()); // hint
            let bytes = s.as_bytes();
            b[name_file + 2..name_file + 2 + bytes.len()].copy_from_slice(bytes);
            b[name_file + 2 + bytes.len()] = 0;
            let adv = (2 + bytes.len() + 1 + 3) & !3;
            name_rva += adv as u32;
            name_file += adv;
        }
        // dll name @ RVA 0x1040 (file 0x440)
        let dn = dll.as_bytes();
        b[0x440..0x440 + dn.len()].copy_from_slice(dn);
        b[0x440 + dn.len()] = 0;
        b
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "rsift_modsec_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|t| t.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    struct DirGuard(PathBuf);
    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // ---- SHA-256 (NIST FIPS 180-4 例) ----
    #[test]
    fn gq_sha256_nist_vectors() {
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
        // 反復ベクタ (100万 'a') — 56 バイト境界・長さ拡張の網羅
        let big = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256_hex(&big),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    // ---- PE import 解析 + 能力分類 ----
    #[test]
    fn gq_pe_imports_parse_and_classify() {
        let pe = build_pe(
            "KERNEL32.dll",
            &["CreateProcessW", "LoadLibraryA", "GetVersion"],
        );
        let imports = pe_imports(&pe).expect("valid PE parses");
        assert_eq!(imports.len(), 3);
        assert!(imports
            .iter()
            .any(|(d, s)| d == "KERNEL32.dll" && s == "CreateProcessW"));
        let report = vet_bytes(&pe);
        assert!(report
            .capabilities_detected
            .contains(&HostCapability::ProcessExec));
        assert!(report
            .capabilities_detected
            .contains(&HostCapability::NativeModuleLoad));
        assert!(report.hard_deny_reasons.is_empty());
        assert!(report
            .evidence
            .iter()
            .any(|e| e.contains("ProcessExec") || e.contains("CreateProcess")));
    }

    // ---- 良性 PE → 検出ゼロ (幅不変の機械証明: 通常 Mod に摩擦なし) ----
    #[test]
    fn gq_pe_benign_imports_clean() {
        let pe = build_pe("USER32.dll", &["MessageBoxW", "GetDlgItem"]);
        let report = vet_bytes(&pe);
        assert_eq!(report.imports_found, 2);
        assert!(report.capabilities_detected.is_empty());
        assert!(report.hard_deny_reasons.is_empty());
    }

    // ---- 破損 PE の全境界で panic せず Err ----
    #[test]
    fn gq_pe_malformed_no_panic_all_truncations() {
        let pe = build_pe("KERNEL32.dll", &["CreateProcessW"]);
        for cut in [
            0usize, 1, 0x3B, 0x40, 0x84, 0x90, 0x98, 0x100, 0x180, 0x200, 0x300, 0x400, 0x410,
            0x440, 0x500, 0x600,
        ] {
            let t = &pe[..cut.min(pe.len())];
            let _ = pe_imports(t); // Err でもよい — panic しないことが契約
            let _ = vet_bytes(t);
        }
        // e_lfanew 暴走値
        let mut evil = pe.clone();
        evil[0x3C..0x40].copy_from_slice(&0xF000_0000u32.to_le_bytes());
        assert!(pe_imports(&evil).is_err());
    }

    // ---- インジェクション連鎖 (2 語彙共起) → 即時 Deny ----
    #[test]
    fn gq_hard_deny_injection_chain() {
        let mut blob = b"\x7fELF fake module ".to_vec();
        blob.extend_from_slice(b"CreateRemoteThread helper, WriteProcessMemory usage");
        let report = vet_bytes(&blob);
        assert!(!report.hard_deny_reasons.is_empty());
        assert!(report.hard_deny_reasons[0].contains("process-injection"));
        // 単独 1 語彙は Deny にならない (良性 CreateToolhelp 系 API ログ文字列等を誤爆しない)
        let mut one = b"\x7fELF fake ".to_vec();
        one.extend_from_slice(b"mention OpenProcess in a doc string only");
        let r2 = vet_bytes(&one);
        assert!(r2.hard_deny_reasons.is_empty());
    }

    // ---- 窃取ツール語彙 → 即時 Deny ----
    #[test]
    fn gq_hard_deny_credential_theft_lexicon() {
        for payload in [
            &b"mimikatz"[..],
            b"sekurlsa::logonpasswords",
            b"dump lsass.exe now",
        ] {
            let report = vet_bytes(payload);
            assert!(
                !report.hard_deny_reasons.is_empty(),
                "payload {:?} must hard-deny",
                String::from_utf8_lossy(payload)
            );
        }
    }

    // ---- 文字列走査: shell / 機密パス / 永続化 / キーログ ----
    #[test]
    fn gq_string_scan_capabilities() {
        let blob = b"\x7fELF /bin/sh -c powershell -enc AAAA /.ssh/id_rsa currentversion\\run getasynckeystate";
        let report = vet_bytes(blob);
        let caps = &report.capabilities_detected;
        assert!(caps.contains(&HostCapability::ProcessExec));
        assert!(caps.contains(&HostCapability::FileReadSensitive));
        assert!(caps.contains(&HostCapability::PersistenceAutostart));
        assert!(caps.contains(&HostCapability::InputCapture));
    }

    // ---- マニフェスト: roundtrip + 未知能力名の明示拒否 ----
    #[test]
    fn gq_manifest_parse_roundtrip_unknown_rejected() {
        let m = CapabilityManifest::from_json_str(
            r#"{"capabilities": ["process_exec", "network_egress"]}"#,
        )
        .unwrap();
        assert!(m.declared.contains(&HostCapability::ProcessExec));
        assert!(m.declared.contains(&HostCapability::NetworkEgress));
        assert_eq!(m.declared.len(), 2);
        assert!(
            CapabilityManifest::from_json_str(r#"{"capabilities": ["process_execx"]}"#).is_err()
        );
        assert!(CapabilityManifest::from_json_str(r#"{"caps": []}"#).is_err());
        assert!(CapabilityManifest::from_json_str("not json").is_err());
    }

    // ---- 判定ラティス 4 腕: clean→Allow / 同意済→Allow(least) / 未同意→RequireConsent / tamper→失効 ----
    #[test]
    fn gq_verdict_lattice_with_consent_store() {
        let dir = tmpdir("lattice");
        let _g = DirGuard(dir.clone());
        let dll = dir.join("libevil.so");
        let bytes = b"\x7fELF mod uses powershell payload";
        std::fs::write(&dll, bytes).unwrap();
        let hex = sha256_hex(bytes);

        // 1) 未同意 → RequireConsent(process_exec)
        let out = vet_mod_binary("evil", &dll).unwrap();
        match &out.verdict {
            LoadVerdict::RequireConsent(missing) => {
                assert_eq!(missing, &vec![HostCapability::ProcessExec])
            }
            other => panic!("expected RequireConsent, got {other:?}"),
        }

        // 2) scaffold 生成 (requested に能力名)
        ensure_pending_entry(&out, "evil");
        let raw = std::fs::read_to_string(&out.consent_path).unwrap();
        assert!(raw.contains("\"requested\""));
        assert!(raw.contains("process_exec"));
        assert!(raw.contains(&hex));

        // 3) ユーザー承認 (granted 追加) → Allow(least-privilege)
        let (mut store, _) = ConsentStore::load(&out.consent_path);
        store.mods.get_mut("evil").unwrap().granted = vec!["process_exec".into()];
        store.save(&out.consent_path).unwrap();
        let out2 = vet_mod_binary("evil", &dll).unwrap();
        match &out2.verdict {
            LoadVerdict::Allow(granted) => {
                assert!(granted.contains(&HostCapability::ProcessExec));
                assert_eq!(granted.len(), 1);
            }
            other => panic!("expected Allow, got {other:?}"),
        }

        // 4) Mod 差替え (ハッシュ変化) → 同意失効 → RequireConsent
        std::fs::write(&dll, b"\x7fELF mod uses powershell payload v2").unwrap();
        let out3 = vet_mod_binary("evil", &dll).unwrap();
        assert!(matches!(out3.verdict, LoadVerdict::RequireConsent(_)));

        // 5) ハード Deny は同意があっても覆らない
        std::fs::write(&dll, b"mimikatz now").unwrap();
        let out4 = vet_mod_binary("evil", &dll).unwrap();
        assert!(matches!(out4.verdict, LoadVerdict::Deny(_)));
    }

    // ---- ランタイムゲート: 拒否監査・付与・audit ring 有界 ----
    #[test]
    fn gq_gate_require_audit_and_ring_bound() {
        let id = "gqmod_gate_test";
        let before = SecurityGate::snapshot();
        // 未登録 → 拒否 (mod not registered)
        let e = SecurityGate::require(id, HostCapability::NetworkEgress).unwrap_err();
        assert!(e.reason.contains("not registered"));
        // 登録 (空) → 拒否 (not granted)
        SecurityGate::register(id, BTreeSet::new());
        assert!(SecurityGate::require(id, HostCapability::ProcessExec).is_err());
        // 付与 → 許可
        let mut caps = BTreeSet::new();
        caps.insert(HostCapability::ProcessExec);
        SecurityGate::register(id, caps.clone());
        assert!(SecurityGate::require(id, HostCapability::ProcessExec).is_ok());
        assert!(SecurityGate::require(id, HostCapability::InputCapture).is_err());
        assert_eq!(SecurityGate::granted_set(id), Some(caps));
        let after = SecurityGate::snapshot();
        assert_eq!(after.checks, before.checks + 4);
        assert_eq!(after.allowed, before.allowed + 1);
        assert_eq!(after.denied, before.denied + 3);
        let report = SecurityGate::report_string();
        assert!(report.contains("process_exec"));
        // ring 有界: 128 回叩いてもイベント 64 件で頭打ち (OOM なし)
        for _ in 0..128 {
            let _ = SecurityGate::require(id, HostCapability::InputCapture);
        }
        assert!(SecurityGate::report_string().matches('\n').count() <= 8);
        SecurityGate::unregister(id);
        assert_eq!(SecurityGate::granted_set(id), None);
    }

    // ---- 同意ストア単体: 往復・denied 除外・hash 不一致 ----
    #[test]
    fn gq_consent_store_roundtrip_and_tamper() {
        let dir = tmpdir("consent");
        let _g = DirGuard(dir.clone());
        let path = dir.join(CONSENT_FILE_NAME);
        let mut store = ConsentStore::default();
        let rec = store.mods.entry("m".into()).or_default();
        rec.sha256 = "ABCD".into();
        rec.granted = vec![
            "network_egress".into(),
            "process_exec".into(),
            "junk".into(),
        ];
        rec.denied = vec!["process_exec".into()];
        store.save(&path).unwrap();
        let (loaded, note) = ConsentStore::load(&path);
        assert!(note.is_none());
        let caps = loaded.granted_caps_for("m", "abcd"); // case-insensitive hash
        assert!(caps.contains(&HostCapability::NetworkEgress));
        assert!(!caps.contains(&HostCapability::ProcessExec)); // denied 優先
        assert_eq!(caps.len(), 1); // "junk" は無視
        assert!(loaded.granted_caps_for("m", "0000").is_empty()); // tamper
        assert!(loaded.granted_caps_for("other", "abcd").is_empty());
        // 破損 JSON → fail-closed 空 + note
        std::fs::write(&path, "{broken").unwrap();
        let (s2, note2) = ConsentStore::load(&path);
        assert!(note2.is_some());
        assert!(s2.mods.is_empty());
    }

    // ---- JNI op 接頭辞ガード: 既存 op 無制限維持 (幅不変) / host.* は拒否 ----
    #[test]
    fn gq_jni_op_prefix_guard_breadth_lock() {
        for op in ["packet", "render", "client_tick", "channel"] {
            assert!(jni_op_allowed(op), "existing op {op} must stay allowed");
        }
        for op in ["host.exec", "host.fs.write", "host.net.send"] {
            assert!(!jni_op_allowed(op));
        }
        assert!(jni_op_allowed("hostname_check")); // 「host.」接頭辞のみ (誤爆なし)
    }

    // ---- 幅不変の回帰 lock: ゲーム Mod 表面は capability 不要 ----
    #[test]
    fn gq_breadth_lock_game_surface_requires_no_capability() {
        // SuiteModule 42 全稼働は capability と直交 (Mod 生態系の大半は危険能力不要)
        assert_eq!(crate::mod_suite::SuiteModuleId::ALL.len(), 42);
        // 危険能力なし Mod の判定は常に即 Allow (摩擦ゼロ)
        let report = vet_bytes(b"\x7fELF a perfectly ordinary decoration mod");
        assert!(report.capabilities_detected.is_empty());
        let verdict = decide(&report, &ConsentStore::default(), "deco", "00");
        assert!(verdict.is_allow());
    }

    // ---- dispatch_op 統合: host.* op はゲート計数つきで遮断 ----
    #[test]
    fn gq_dispatch_op_host_prefix_blocked_and_counted() {
        // runtime 非初期化でも拒否は計数される (拒否は dispatch 最前段)
        let before = SecurityGate::snapshot().jni_host_refusals;
        crate::mod_dispatch::dispatch_op("host.exec", 0, 0, 0);
        crate::mod_dispatch::dispatch_op("host.fs.write", 0, 0, 0);
        let after = SecurityGate::snapshot().jni_host_refusals;
        assert_eq!(after, before + 2);
    }

    // ---- sidecar パス規約 + sidecar 宣言が scaffold に反映 ----
    #[test]
    fn gq_sidecar_path_and_declared_scaffold() {
        let dir = tmpdir("sidecar");
        let _g = DirGuard(dir.clone());
        let dll = dir.join("libnetty.so");
        assert_eq!(
            CapabilityManifest::sidecar_path_for(&dll),
            dir.join("libnetty.rsift-perms.json")
        );
        std::fs::write(&dll, b"\x7fELF uses dlopen for plugin").unwrap();
        std::fs::write(
            dir.join("libnetty.rsift-perms.json"),
            r#"{"capabilities": ["native_module_load"]}"#,
        )
        .unwrap();
        let out = vet_mod_binary("netty", &dll).unwrap();
        assert!(out
            .declared_by_manifest
            .contains(&HostCapability::NativeModuleLoad));
        assert!(matches!(out.verdict, LoadVerdict::RequireConsent(_)));
        ensure_pending_entry(&out, "netty");
        let raw = std::fs::read_to_string(&out.consent_path).unwrap();
        assert!(raw.contains("native_module_load"));
    }

    // ---- フォーマット検出 ----
    #[test]
    fn gq_format_detection() {
        assert_eq!(detect_format(b"MZ...."), BinaryFormat::Pe);
        assert_eq!(detect_format(b"\x7fELF.."), BinaryFormat::Elf);
        assert_eq!(
            detect_format(&0xFEEDFACFu32.to_le_bytes()),
            BinaryFormat::MachO
        );
        assert_eq!(
            detect_format(&0xCAFEBABEu32.to_be_bytes()),
            BinaryFormat::MachO
        );
        assert_eq!(detect_format(b"garbage"), BinaryFormat::Unknown);
        assert_eq!(detect_format(b""), BinaryFormat::Unknown);
    }

    // ---- ローダー統合: Deny は動的リンク以前に fail / 良性は vet を通過 ----
    #[test]
    fn gq_loader_integration_deny_precedes_linker() {
        let dir = tmpdir("loader");
        let _g = DirGuard(dir.clone());
        let rt = crate::runtime::RsiftRuntime::new(dir.clone());
        // wave HR: プラットフォーム拡張子 (.dll/.dylib/.so) を使う — .so 固定だと
        // Windows(.dll)/macOS(.dylib) のローダがファイルを拾わず gqok 未登録で
        // assert が失敗していた (run-33 で cargo test の 3OS 化により顕在化)。
        let ext = crate::native_loader::platform_extension();
        // Deny: mimikatz 語彙 — 実行形式を名乗るがリンク以前に止まる
        std::fs::write(dir.join(format!("libgqevil.{ext}")), b"mimikatz payload").unwrap();
        // 良性 (検出ゼロ): 動的リンクには進む (不正バイナリなのでそこで失敗 = vet 非阻止の証明)
        std::fs::write(dir.join(format!("libgqok.{ext}")), b"\x7fELF gentle decor").unwrap();
        let result =
            crate::native_loader::load_mods_filtered(&dir, &rt, true, &["gqevil", "gqok"]).unwrap();
        assert!(result.loaded.is_empty());
        // vet 通過 (Allow) した gqok はゲートに登録済み、Deny の gqevil は未登録
        assert!(SecurityGate::granted_set("gqok").is_some());
        assert!(SecurityGate::granted_set("gqevil").is_none());
        SecurityGate::unregister("gqok");
    }
}
