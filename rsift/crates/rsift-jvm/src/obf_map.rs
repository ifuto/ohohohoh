//! wave 219 HP: 実行時難読化 (obfuscated runtime) の名変換層。
//!
//! 一次ソースで確定した事実:
//! - Minecraft Java は **1.21.11 まで難読化** (最終難読化リリース)。非難読化は
//!   26.1 (2025-12) から (Mojang 公式発表「removing-obfuscation-in-java-edition」
//!   2025-10-30、Fabric 公式 FAQ「1.21.11 is still obfuscated. The next version
//!   26.1 will be the first unobfuscated」)。実機 #5 の `net.minecraft.client.
//!   Minecraft` 永続 CNFE はこの構造的事実と完全一致 (エントリ
//!   `net.minecraft.client.main.Main` のみランチャー契約で原型名を残す)。
//! - したがって難読化バージョンの実行時には mojmap 名は **存在しない**。
//!   エージェントの固定対象 (Minecraft/Screen/DebugScreenEntryList 等) を触るには
//!   公式 client_mappings.txt (ProGuard 形式) で mojmap → 難読名へ変換する必要がある。
//!
//! ファイル形式 (Mojang 配布 client.txt / ProGuard。方向検証済: 左=mojmap、右=難読):
//! ```text
//! net.minecraft.client.Minecraft -> gfj:
//!     net.minecraft.client.Minecraft instance -> A
//!     187:187:void init() -> a
//! ```
//! クラス行は行頭空白なし・末尾 `:`。メンバ行は行頭空白あり。
//! メソッド行は括弧を含み、任意で `行番号:行番号:` 前置を持つ。フィールド行は括弧無し。
//!
//! 規律: 推測・当ては一切しない。mode=Obfuscated で map に無い名前は None を返し、
//! 呼出側は fail-loud ログのうえ対象機能をスキップする (適当な代替名で当たりに
//! 行くことは恒久的に禁止)。

use std::collections::HashMap;
use std::sync::OnceLock;

/// 実行時の命名モード (起動時に 1 度だけプローブで確定する)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeNaming {
    /// 26.1 以降: mojmap 名がそのまま実行時名。変換は恒等。
    Unobfuscated,
    /// 1.21.11 以前 (実機の現行対象): 難読名。公式 mappings が必須。
    Obfuscated,
}

/// 変換表 (mojmap → 難読名)。
#[derive(Default, Debug)]
pub struct ObfMap {
    /// "net.minecraft.client.Minecraft" -> "gfj"
    class_m2o: HashMap<String, String>,
    /// (class, mojmap method name) -> [(正規化 decl, obf name)] (overload 保持)
    method_m2o: HashMap<(String, String), Vec<(String, String)>>,
    /// (class, mojmap field name) -> obf name (衝突時は後勝ちせず Err 方向へ)
    field_m2o: HashMap<(String, String), String>,
}

impl ObfMap {
    pub fn class_count(&self) -> usize {
        self.class_m2o.len()
    }
    pub fn method_count(&self) -> usize {
        self.method_m2o.values().map(|v| v.len()).sum()
    }
    pub fn field_count(&self) -> usize {
        self.field_m2o.len()
    }

    /// クラス名 (dotted mojmap) → 難読 dotted。Unobfuscated は恒等。
    /// Obfuscated で未収録は None (= 呼出側 fail-loud。推測しない)。
    pub fn resolve_class(&self, mode: RuntimeNaming, mojmap_dotted: &str) -> Option<String> {
        match mode {
            RuntimeNaming::Unobfuscated => Some(mojmap_dotted.to_string()),
            RuntimeNaming::Obfuscated => self.class_m2o.get(mojmap_dotted).cloned(),
        }
    }

    /// クラス名 → 難読 internal 名 (slash は caller 側で変換: dotted を `.`→`/`)。
    pub fn resolve_class_internal(&self, mode: RuntimeNaming, mojmap_dotted: &str) -> Option<String> {
        self.resolve_class(mode, mojmap_dotted)
            .map(|d| d.replace('.', "/"))
    }

    /// メソッド一意解決 (class, mojmap 名)。overload が 2 以上なら None
    /// (曖昧解消は `resolve_method_decl` を使う — 当て推測を構造的に排除)。
    pub fn resolve_method_by_name(
        &self,
        mode: RuntimeNaming,
        class_mojmap_dotted: &str,
        mojmap_name: &str,
    ) -> Option<String> {
        match mode {
            RuntimeNaming::Unobfuscated => Some(mojmap_name.to_string()),
            RuntimeNaming::Obfuscated => {
                let v = self
                    .method_m2o
                    .get(&(class_mojmap_dotted.to_string(), mojmap_name.to_string()))?;
                if v.len() == 1 {
                    Some(v[0].1.clone())
                } else {
                    None
                }
            }
        }
    }

    /// メソッド decl 前方一致解決: ProGuard 宣言 (除去済み行番号の
    /// `ret name(args)` 文字列) が `decl_contains` を含む候補のみに絞る。
    /// 0 または 2 以上なら None (これも曖昧/不存在の明確化)。
    pub fn resolve_method_decl(
        &self,
        mode: RuntimeNaming,
        class_mojmap_dotted: &str,
        mojmap_name: &str,
        decl_contains: &str,
    ) -> Option<String> {
        match mode {
            RuntimeNaming::Unobfuscated => Some(mojmap_name.to_string()),
            RuntimeNaming::Obfuscated => {
                let v = self
                    .method_m2o
                    .get(&(class_mojmap_dotted.to_string(), mojmap_name.to_string()))?;
                let hits: Vec<&String> = v
                    .iter()
                    .filter(|(decl, _)| decl.contains(decl_contains))
                    .map(|(_, obf)| obf)
                    .collect();
                if hits.len() == 1 {
                    Some(hits[0].clone())
                } else {
                    None
                }
            }
        }
    }

    /// フィールド解決。
    pub fn resolve_field(
        &self,
        mode: RuntimeNaming,
        class_mojmap_dotted: &str,
        mojmap_name: &str,
    ) -> Option<String> {
        match mode {
            RuntimeNaming::Unobfuscated => Some(mojmap_name.to_string()),
            RuntimeNaming::Obfuscated => self
                .field_m2o
                .get(&(class_mojmap_dotted.to_string(), mojmap_name.to_string()))
                .cloned(),
        }
    }

    /// ProGuard mapping テキストの解析。方向検証は `require_anchor` で
    /// 呼出側が行う (本関数自体は文法のみ検査)。
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut map = ObfMap::default();
        let mut cur_class: Option<String> = None;
        for (lineno0, raw) in text.lines().enumerate() {
            let lineno = lineno0 + 1;
            if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
                continue;
            }
            let indented = raw.starts_with(' ') || raw.starts_with('\t');
            if !indented {
                // クラス行: "mojmap.Name -> obf.name:"
                let body = raw
                    .strip_suffix(':')
                    .ok_or_else(|| format!("line {lineno}: class line missing ':' suffix"))?;
                let (mojmap, obf) = body
                    .split_once(" -> ")
                    .ok_or_else(|| format!("line {lineno}: class line missing ' -> '"))?;
                if mojmap.is_empty() || obf.is_empty() {
                    return Err(format!("line {lineno}: empty class name side"));
                }
                map.class_m2o
                    .insert(mojmap.to_string(), obf.to_string());
                cur_class = Some(mojmap.to_string());
                continue;
            }
            // メンバ行: 前のクラス行必須
            let class = match &cur_class {
                Some(c) => c.clone(),
                None => {
                    return Err(format!(
                        "line {lineno}: member line before any class line"
                    ))
                }
            };
            let body = raw.trim();
            let (decl, obf) = body
                .split_once(" -> ")
                .ok_or_else(|| format!("line {lineno}: member line missing ' -> '"))?;
            if obf.contains(char::is_whitespace) || obf.is_empty() {
                return Err(format!("line {lineno}: malformed obf member name"));
            }
            if decl.contains('(') {
                // メソッド: "N:N:retType name(args)" (行番号は任意前置)
                let paren = decl
                    .find('(')
                    .ok_or_else(|| format!("line {lineno}: malformed method decl"))?;
                let args_end = decl
                    .find(')')
                    .ok_or_else(|| format!("line {lineno}: malformed method decl"))?;
                if args_end <= paren {
                    return Err(format!("line {lineno}: malformed method parens"));
                }
                let before = strip_line_numbers(decl[..paren].trim_end());
                let name = before
                    .rsplit_once(' ')
                    .map(|(_, n)| n)
                    .ok_or_else(|| {
                        format!("line {lineno}: method decl missing return type")
                    })?;
                // 正規化 decl: "retType name(args)" (行番号除去済・前後空白除去)
                let norm = format!(
                    "{}({})",
                    before.trim(),
                    decl[paren + 1..args_end].split(',').map(|a| a.trim()).collect::<Vec<_>>().join(",")
                );
                map.method_m2o
                    .entry((class, name.to_string()))
                    .or_default()
                    .push((norm, obf.to_string()));
            } else {
                // フィールド: "type name"
                let name = decl
                    .rsplit_once(' ')
                    .map(|(_, n)| n)
                    .ok_or_else(|| format!("line {lineno}: field decl missing type"))?;
                map.field_m2o
                    .insert((class, name.to_string()), obf.to_string());
            }
        }
        Ok(map)
    }

    /// 方向アンカー検証: mojmap 側キーとして既知アンカークラスが含まれること。
    /// (変換方向が逆、あるいは別形式のファイルを誤って食わせた場合の fail-loud。
    /// アンカー `net.minecraft.client.Minecraft` は mojmap 1.21.11 に実在することを
    /// mappings.dev で一次確認済 — 本検証は推測ではなく実ファイルへの照合。)
    pub fn require_anchor(&self) -> Result<(), String> {
        const ANCHOR: &str = "net.minecraft.client.Minecraft";
        if self.class_m2o.contains_key(ANCHOR) {
            Ok(())
        } else {
            Err(format!(
                "anchor class {ANCHOR} not found as mojmap key — mapping direction or version mismatch"
            ))
        }
    }
}

/// ProGuard メソッド宣言前置の行番号列 "N:N:" (0 以上繰返し) を除去。
fn strip_line_numbers(mut s: &str) -> &str {
    loop {
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i > 0 && i < b.len() && b[i] == b':' {
            let rest = &s[i + 1..];
            let rb = rest.as_bytes();
            let mut j = 0;
            while j < rb.len() && rb[j].is_ascii_digit() {
                j += 1;
            }
            if j > 0 && j < rb.len() && rb[j] == b':' {
                s = &rest[j + 1..];
                continue;
            }
        }
        return s;
    }
}

// ------------------------------------------------------------------------
// グローバル (init は起動プローブ後に 1 度だけ)
// ------------------------------------------------------------------------

static MAP: OnceLock<std::sync::RwLock<Option<ObfMap>>> = OnceLock::new();
static MODE: OnceLock<RuntimeNaming> = OnceLock::new();

fn map_slot() -> &'static std::sync::RwLock<Option<ObfMap>> {
    MAP.get_or_init(|| std::sync::RwLock::new(None))
}

/// mode + map の確定 (冪等ではなく 1 度のみ。2 回目は false)。
pub fn install(mode: RuntimeNaming, map: Option<ObfMap>) -> Result<(), String> {
    MODE.set(mode)
        .map_err(|_| "obf_map: mode already installed".to_string())?;
    if let Some(m) = map {
        *map_slot().write().map_err(|e| e.to_string())? = Some(m);
    }
    Ok(())
}

pub fn current_mode() -> Option<RuntimeNaming> {
    MODE.get().copied()
}

/// クラス名解決 (グローバル版)。mode 未確定は None (= 呼出側はプローブ後に呼ぶ)。
pub fn resolve_class(mojmap_dotted: &str) -> Option<String> {
    let mode = current_mode()?;
    let guard = map_slot().read().ok()?;
    match mode {
        RuntimeNaming::Unobfuscated => Some(mojmap_dotted.to_string()),
        RuntimeNaming::Obfuscated => guard.as_ref()?.resolve_class(mode, mojmap_dotted),
    }
}

/// クラス名 → internal (slash) 名解決。
pub fn resolve_class_internal(mojmap_dotted: &str) -> Option<String> {
    resolve_class(mojmap_dotted).map(|d| d.replace('.', "/"))
}

pub fn resolve_method_by_name(class_dotted: &str, mojmap_name: &str) -> Option<String> {
    let mode = current_mode()?;
    let guard = map_slot().read().ok()?;
    match mode {
        RuntimeNaming::Unobfuscated => Some(mojmap_name.to_string()),
        RuntimeNaming::Obfuscated => {
            guard
                .as_ref()?
                .resolve_method_by_name(mode, class_dotted, mojmap_name)
        }
    }
}

pub fn resolve_method_decl(class_dotted: &str, mojmap_name: &str, decl_contains: &str) -> Option<String> {
    let mode = current_mode()?;
    let guard = map_slot().read().ok()?;
    match mode {
        RuntimeNaming::Unobfuscated => Some(mojmap_name.to_string()),
        RuntimeNaming::Obfuscated => {
            guard
                .as_ref()?
                .resolve_method_decl(mode, class_dotted, mojmap_name, decl_contains)
        }
    }
}

pub fn resolve_field(class_dotted: &str, mojmap_name: &str) -> Option<String> {
    let mode = current_mode()?;
    let guard = map_slot().read().ok()?;
    match mode {
        RuntimeNaming::Unobfuscated => Some(mojmap_name.to_string()),
        RuntimeNaming::Obfuscated => guard.as_ref()?.resolve_field(mode, class_dotted, mojmap_name),
    }
}

/// (class, method, field) の収録数サマリ (起動ログ用)。
pub fn stats() -> Option<(usize, usize, usize)> {
    let guard = map_slot().read().ok()?;
    guard
        .as_ref()
        .map(|m| (m.class_count(), m.method_count(), m.field_count()))
}

/// `<dir>/client.txt` (Mojang 公式 ProGuard mappings) を読み込み解析し、方向アンカー
/// 検証済みの ObfMap を返す (wave HR #5 根治基盤)。
///
/// - ファイル不在 → `Ok(None)` (呼出側は Unobfuscated mode へ落下 = 従来挙動完全保存)。
/// - 読取/解析/アンカー不適合 → `Err` (呼出側がログ化のうえ None 扱いへ)。
///
/// データ調達は rsift-setup が 1.21.11 client.txt (sha1 `031a68be…`, 11.8MB) を
/// ユーザー機で取得して本ディレクトリへ配置する (agent 側はネットワーク非依存)。
pub fn try_load_from_dir(dir: &std::path::Path) -> Result<Option<ObfMap>, String> {
    let path = dir.join("client.txt");
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("obf_map read {:?}: {}", path, e))?;
    let map = ObfMap::parse(&text).map_err(|e| format!("obf_map parse {:?}: {}", path, e))?;
    map.require_anchor()
        .map_err(|e| format!("obf_map anchor {:?}: {}", path, e))?;
    Ok(Some(map))
}

/// 起動プローブ: dll_dir の client.txt を読み、モードを確定して install する
/// (冪等ではなく起動時に 1 度だけ呼ぶ)。戻り値は採用モード (ログ用)。
/// - client.txt 在 + 解析成功 → Obfuscated モード (map 付き install)
/// - 不在/失敗 → Unobfuscated モード (map 無し = 従来挙動。エラーはログ)
///
/// `install` は MODE.set を 1 度しか許さないため、本関数も 1 度のみ呼出可能
/// (2 度目は Err — 起動点での単一呼出を前提)。
pub fn install_from_dir(dir: &std::path::Path) -> Result<RuntimeNaming, String> {
    match try_load_from_dir(dir) {
        Ok(Some(map)) => {
            let (c, mth, f) = (map.class_count(), map.method_count(), map.field_count());
            install(RuntimeNaming::Obfuscated, Some(map))?;
            agent_log_obf("obf_map", &format!(
                "client.txt loaded → Obfuscated mode (classes={c} methods={mth} fields={f})"
            ));
            Ok(RuntimeNaming::Obfuscated)
        }
        Ok(None) => {
            install(RuntimeNaming::Unobfuscated, None)?;
            agent_log_obf("obf_map", &format!(
                "client.txt absent in {:?} → Unobfuscated mode (mojmap identity, legacy behavior)",
                dir
            ));
            Ok(RuntimeNaming::Unobfuscated)
        }
        Err(e) => {
            // 解析/アンカー失敗は安全側へ落下 (従来挙動) するが理由は必ず残す。
            install(RuntimeNaming::Unobfuscated, None)?;
            agent_log_obf("obf_map", &format!(
                "client.txt load failed ({e}) → falling back to Unobfuscated mode (legacy). FIX: re-run setup to fetch client.txt"
            ));
            Ok(RuntimeNaming::Unobfuscated)
        }
    }
}

/// obf_map モジュール内ロガー (agent_log への thin wrapper。循環依存回避のため
/// 直接 crate::agent_log を呼ぶ — rsift-jvm 内なので安全)。
fn agent_log_obf(tag: &str, msg: &str) {
    crate::agent_log::agent_log(&format!("[Rsift] [{}] {}", tag, msg));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一次確認済形式 (Mojang client.txt: 左=mojmap、右=難読) の縮小 fixture。
    const FIXTURE: &str = r#"
# ProGuard mapping (縮小 fixture — 構造は実ファイルと同一文法)
net.minecraft.client.Minecraft -> gfj:
    net.minecraft.client.Minecraft instance -> A
    net.minecraft.client.Options options -> B
    187:187:void init() -> a
    44:44:void run() -> b
    4:12:void overload(int) -> c
    40:52:void overload(int,int) -> d
net.minecraft.client.gui.screens.Screen -> xyz:
    90:90:void init(net.minecraft.client.Minecraft,int,int) -> a
net.minecraft.client.gui.components.debug.DebugScreenEntryList -> abc:
    java.util.List getLines() -> a
"#;

    #[test]
    fn parse_and_resolve_class_method_field() {
        let m = ObfMap::parse(FIXTURE).expect("parse");
        assert_eq!(m.class_count(), 3);
        m.require_anchor().expect("anchor ok");
        let ob = RuntimeNaming::Obfuscated;
        assert_eq!(
            m.resolve_class(ob, "net.minecraft.client.Minecraft").as_deref(),
            Some("gfj")
        );
        assert_eq!(
            m.resolve_class_internal(ob, "net.minecraft.client.gui.screens.Screen")
                .as_deref(),
            Some("xyz")
        );
        assert_eq!(
            m.resolve_field(ob, "net.minecraft.client.Minecraft", "options")
                .as_deref(),
            Some("B")
        );
        assert_eq!(
            m.resolve_method_by_name(ob, "net.minecraft.client.Minecraft", "init")
                .as_deref(),
            Some("a")
        );
        assert_eq!(
            m.resolve_method_by_name(ob, "net.minecraft.client.gui.components.debug.DebugScreenEntryList", "getLines")
                .as_deref(),
            Some("a")
        );
        // overload は名前一意解決を拒否 → decl 絞込で解決
        assert!(m
            .resolve_method_by_name(ob, "net.minecraft.client.Minecraft", "overload")
            .is_none());
        assert_eq!(
            m.resolve_method_decl(ob, "net.minecraft.client.Minecraft", "overload", "int,int")
                .as_deref(),
            Some("d")
        );
        // 未収録は None (当て推測なし)
        assert!(m
            .resolve_class(ob, "net.minecraft.NoSuchClass")
            .is_none());
        assert!(m
            .resolve_method_by_name(ob, "net.minecraft.client.Minecraft", "noSuch")
            .is_none());
    }

    #[test]
    fn unobfuscated_is_identity() {
        let m = ObfMap::default();
        let un = RuntimeNaming::Unobfuscated;
        assert_eq!(
            m.resolve_class(un, "net.minecraft.client.Minecraft").as_deref(),
            Some("net.minecraft.client.Minecraft")
        );
        assert_eq!(
            m.resolve_method_by_name(un, "anything", "getLines").as_deref(),
            Some("getLines")
        );
    }

    #[test]
    fn anchor_validation_rejects_wrong_direction() {
        // 方向が逆のファイル (難読名が左) はアンカー検証で拒否される。
        let bad = "gfj -> net.minecraft.client.Minecraft:\n";
        let m = ObfMap::parse(bad).expect("parse itself ok");
        assert!(m.require_anchor().is_err());
    }

    #[test]
    fn member_before_class_is_rejected() {
        let bad = "    int x -> a\n";
        assert!(ObfMap::parse(bad).is_err());
    }

    #[test]
    fn strip_line_numbers_works() {
        assert_eq!(strip_line_numbers("187:187:void init"), "void init");
        assert_eq!(strip_line_numbers("void init"), "void init");
        assert_eq!(strip_line_numbers("4:12:int f"), "int f");
    }

    /// wave HR: try_load_from_dir — 在/不在/方向不適合の 3 ケース。
    #[test]
    fn try_load_from_dir_present_absent_bad() {
        let dir = std::env::temp_dir().join(format!(
            "rsift_obf_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // (1) 不在 → Ok(None)
        assert!(try_load_from_dir(&dir).unwrap().is_none());

        // (2) 在 + 正方向 → Ok(Some) でアンカー検証済
        std::fs::write(dir.join("client.txt"), FIXTURE).unwrap();
        let m = try_load_from_dir(&dir).unwrap().expect("Some(map)");
        assert_eq!(m.class_count(), 3);
        assert_eq!(
            m.resolve_class(RuntimeNaming::Obfuscated, "net.minecraft.client.Minecraft")
                .as_deref(),
            Some("gfj")
        );

        // (3) 逆方向ファイル (難読名が左) → Err (アンカー検証拒否)
        std::fs::write(dir.join("client.txt"), "gfj -> net.minecraft.client.Minecraft:\n").unwrap();
        assert!(try_load_from_dir(&dir).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
