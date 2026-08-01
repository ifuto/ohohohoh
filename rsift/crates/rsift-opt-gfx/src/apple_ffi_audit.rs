//! Apple FFI 監査機 — binding 層ソースと一次情報 canon の機械照合。
//!
//! 【wave 194 GN (2026-07-30)】ユーザー要求「Mac 実機なしで METAL API
//! ドキュメントを正典とする完璧な独自の静的解析マシーンを Rust で作成し、
//! 実際に動作するかある程度検証して動くことは保証して」に対する中核機構。
//!
//! ## 保証の体系 (何をどこまで保証するか、誠実な境界)
//! 本 audit が保証するのは「binding 層に書かれた Apple 契約が一次情報
//! canon (apple_canon.rs = Apple SDK ヘッダ機械転記 + DocC) と**完全に
//! 一致する**」こと。具体的には:
//! - R1: 全 SEL_* 宣言が (class, selector) として canon に存在
//!   (canon 継承表 CANON_PARENTS で親帰属も解決: 例 endEncoding は
//!   MTLRenderCommandEncoder 上の呼出だが MTLCommandEncoder 帰属を正とする)。
//! - R2: 全 MTLV_* enum 値が canon CANON_METAL_ENUMS と厳密一致。
//! - R3: dispatcher 呼出 80 件全てで「selector コロン数 == trait 宣言の
//!   データ引数個数 == 呼出側の実引数個数」の三者一致 (引数過不足の
//!   ABI 破壊を全件機械検査)。
//! - R4: extern "C" link_name が libobjc 既知集合 ∪ CANON_METAL_CFNS
//!   と一致し、宣言引数個数も正典と一致。
//! - R5: テスト fixture の on_make/on_borrow 所有規則登録が canon の
//!   retain 規則と一致 (on_make=Owned 必須、on_borrow=Borrowed 必須)。
//!   mock 検証自体の正しさを機械保証する自己照査。
//! - R6: SEL_/MTLV_/CLASS_ 定数に消費者ゼロ (宣言のみで未使用) がない
//!   (指令 §7 の機械化)。
//! - R7: CLASS_* 名が canon 既知クラスに解決できる。
//!
//! ## 保証の境界 (誠実な明記)
//! - 実機 GPU ドライバの応答・実描画結果は sandbox 計測外 (Mac 実機非所持)。
//!   その代替として MockObjcRt による「本番コード全行の Linux 動的実行
//!   検証」(呼出列・所有規則・リーク・物理 memcpy) をテストで実施済。
//! - extern 宣言の ABI が実際に macOS で一致することは、一次情報
//!   (objc2 header-translator 転記 / Apple DocC) との照合で静的保証。

use crate::apple_canon::{
    RetainRule, CANON_METAL_CFNS, CANON_METAL_ENUMS, CANON_PARENTS, CANON_SDK26_CLASSES,
    CANON_SDK26_ENUMS, CANON_SDK26_PARENTS, CANON_SDK26_SELS, CANON_SELS,
};

/// 監査違反 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// 規則 ID (R1..R7)。
    pub rule: &'static str,
    /// 詳細 (人間可読、どの宣言/呼出が何に違反したか)。
    pub detail: String,
}

impl core::fmt::Display for Violation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "[{}] {}", self.rule, self.detail)
    }
}

// ---------------------------------------------------------------------------
// 構文スキャナ (外部 regex 非依存の決定的ミニパーサ)
// ---------------------------------------------------------------------------

/// 論理フラグメント列: 行コメント (doc 含む) を除去し、`;` `{` `}` を
/// 区切りとして切り出したソース片 (開始行番号付き)。これにより監査は
/// 物理行レイアウト (rustfmt の折返し) や入れ子深度に依存しない
/// 完全構文駆動となる。対象文法は const 宣言とメソッド呼出列で、
/// いずれも `;` 区切りで完結する。文字列/文字 literal 内の区切り文字は
/// 無視する (raw string 中の `;` も安全側で吸収)。
pub fn logical_statements(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start = 0usize;
    let mut in_str = false;
    let mut in_char = false;
    let mut prev = '\0';
    let flush = |cur: &mut String, start: usize, out: &mut Vec<(usize, String)>| {
        let t = cur.trim();
        if !t.is_empty() {
            out.push((start, t.to_string()));
        }
        cur.clear();
    };
    for (ln, line) in src.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        if cur.is_empty() {
            start = ln + 1;
        } else {
            cur.push(' ');
        }
        cur.push_str(line.trim());
        // char リテラルと lifetime 引用符の区別 (wave 196 GO 改善):
        // `'` 直後が「任意 1 文字 + `'`」or `\` + 1 文字 + `'` の形のみ
        // char リテラルとみなす (配列内部 index 参照で決定的判定)。
        // それ以外 (`'static` 等の lifetime 引用符、1 行に奇数個出現し得る)
        // は状態非遷移 — 旧実装は lifetime `'` で in_char に誤入し、
        // 次の `'` までの全 `;`/`{}` 分割を喪失して文を丸呑みする
        // 構造バグがあった (metal4_direct.rs で go- 系の消費者検査が
        // 陰性化する形で発覚・ironclad に根治)。
        let chars: Vec<char> = line.chars().collect();
        let mut ci = 0usize;
        while ci < chars.len() {
            let ch = chars[ci];
            if in_str {
                if ch == '"' && prev != '\\' {
                    in_str = false;
                }
            } else if in_char {
                if ch == '\'' && prev != '\\' {
                    in_char = false;
                }
            } else {
                match ch {
                    '"' => in_str = true,
                    '\'' => {
                        let char_lit = (ci + 2 < chars.len() && chars[ci + 2] == '\'')
                            || (ci + 3 < chars.len()
                                && chars[ci + 1] == '\\'
                                && chars[ci + 3] == '\'');
                        if char_lit {
                            in_char = true;
                        }
                    }
                    ';' | '{' | '}' => {
                        flush(&mut cur, start, &mut out);
                        start = ln + 1;
                    }
                    _ => {}
                }
            }
            prev = ch;
            ci += 1;
        }
    }
    flush(&mut cur, start, &mut out);
    out
}

/// `pub const SEL_<NAME>: (&str, &CStr) = ("CLASS", c"SEL");` 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelDecl {
    /// 定数名 (例 SEL_COMMIT)。
    pub name: String,
    /// class 帰属 (canon 照合キー)。
    pub class_name: String,
    /// selector 本体。
    pub selector: String,
}

/// ミニカーソル (ws 吸収 + リテラル1個読み)。
struct Cur<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Cur<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            s: s.as_bytes(),
            i: 0,
        }
    }
    fn ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] as char).is_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, b: u8) -> bool {
        self.ws();
        if self.s.get(self.i) == Some(&b) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    /// 識別子 1 個 ([A-Za-z0-9_]+)。
    fn ident(&mut self) -> Option<String> {
        self.ws();
        let start = self.i;
        while self.i < self.s.len() {
            let c = self.s[self.i] as char;
            if !(c.is_ascii_alphanumeric() || c == '_') {
                break;
            }
            self.i += 1;
        }
        if self.i == start {
            return None;
        }
        Some(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }
    /// 文字列 literal 1 個 ("..."、escape は対象外: selector/class/enum 名に
    /// 引用符は出現しない一次情報特性)。
    fn str_lit(&mut self) -> Option<String> {
        self.ws();
        if self.s.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let start = self.i;
        while self.i < self.s.len() && self.s[self.i] != b'"' {
            self.i += 1;
        }
        if self.i >= self.s.len() {
            return None;
        }
        let out = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
        self.i += 1;
        Some(out)
    }
    /// 整数 (符号任意)。
    fn int_lit(&mut self) -> Option<i64> {
        self.ws();
        let start = self.i;
        if self.s.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        while self.i < self.s.len() && (self.s[self.i] as char).is_ascii_digit() {
            self.i += 1;
        }
        if self.i == start || (self.i == start + 1 && self.s[start] == b'-') {
            return None;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).parse().ok()
    }
}

/// ソースから SEL_ 宣言を全件抽出 (論理ステートメント駆動、
/// rustfmt 等の折返しに非依存。パース失敗は行番号付きで返す)。
pub fn extract_sel_decls(src: &str) -> (Vec<SelDecl>, Vec<String>) {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for (ln, stmt) in logical_statements(src) {
        if !stmt.starts_with("pub const SEL_") {
            continue;
        }
        let mut c = Cur::new(&stmt);
        let parsed = (|| {
            c.ident()?; // pub
            c.ident()?; // const
            let name = c.ident()?;
            if !c.eat(b':') {
                return None;
            }
            // 型 (&str, &CStr) は `=` まで読み飛ばす。
            while !c.eat(b'=') {
                c.i += 1;
                if c.i >= c.s.len() {
                    return None;
                }
            }
            if !c.eat(b'(') {
                return None;
            }
            let class_name = c.str_lit()?;
            if !c.eat(b',') {
                return None;
            }
            // c"..." 前置子
            c.ws();
            if c.s.get(c.i) != Some(&b'c') {
                return None;
            }
            c.i += 1;
            let selector = c.str_lit()?;
            Some(SelDecl {
                name,
                class_name,
                selector,
            })
        })();
        match parsed {
            Some(d) => out.push(d),
            None => errs.push(format!(
                "line {ln}: SEL 宣言が監査形式に合致しない ({stmt})"
            )),
        }
    }
    (out, errs)
}

/// `pub const MTLV_<NAME>: (&str, &str, i64) = ("ENUM", "VARIANT", -?N);` 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumConst {
    /// 定数名。
    pub name: String,
    /// enum 型名。
    pub enum_name: String,
    /// バリアント名。
    pub variant: String,
    /// 値。
    pub value: i64,
}

/// MTLV_ enum 値宣言の全件抽出 (論理ステートメント駆動)。
pub fn extract_enum_consts(src: &str) -> (Vec<EnumConst>, Vec<String>) {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for (ln, stmt) in logical_statements(src) {
        if !stmt.starts_with("pub const MTLV_") {
            continue;
        }
        let mut c = Cur::new(&stmt);
        let parsed = (|| {
            c.ident()?; // pub
            c.ident()?; // const
            let name = c.ident()?;
            if !c.eat(b':') {
                return None;
            }
            while !c.eat(b'=') {
                c.i += 1;
                if c.i >= c.s.len() {
                    return None;
                }
            }
            if !c.eat(b'(') {
                return None;
            }
            let enum_name = c.str_lit()?;
            if !c.eat(b',') {
                return None;
            }
            let variant = c.str_lit()?;
            if !c.eat(b',') {
                return None;
            }
            let value = c.int_lit()?;
            Some(EnumConst {
                name,
                enum_name,
                variant,
                value,
            })
        })();
        match parsed {
            Some(d) => out.push(d),
            None => errs.push(format!(
                "line {ln}: MTLV 宣言が監査形式に合致しない ({stmt})"
            )),
        }
    }
    (out, errs)
}

/// CLASS_* 宣言 `(name, class 実名)` の全件抽出 (論理ステートメント駆動)。
pub fn extract_class_consts(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (_ln, stmt) in logical_statements(src) {
        if !stmt.starts_with("pub const CLASS_") {
            continue;
        }
        let mut c = Cur::new(&stmt);
        let parsed = (|| {
            c.ident()?; // pub
            c.ident()?; // const
            let name = c.ident()?;
            if !c.eat(b':') {
                return None;
            }
            while !c.eat(b'=') {
                c.i += 1;
                if c.i >= c.s.len() {
                    return None;
                }
            }
            c.ws();
            if c.s.get(c.i) != Some(&b'c') {
                return None;
            }
            c.i += 1;
            let cls = c.str_lit()?;
            Some((name, cls))
        })();
        if let Some(d) = parsed {
            out.push(d);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// canon 解決 (継承は BFS、CANON_PARENTS 正典表を巡回)
// ---------------------------------------------------------------------------

/// (class, sel) を canon から解決。親帰属も OK。
/// 戻り値: (実際に宣言のある class 名, retain 規則)。
pub fn resolve_selector(class_name: &str, selector: &str) -> Option<(&'static str, RetainRule)> {
    let mut todo: Vec<String> = vec![class_name.to_string()];
    let mut seen: Vec<String> = Vec::new();
    while let Some(c) = todo.pop() {
        if seen.iter().any(|x| *x == c) {
            continue;
        }
        seen.push(c.clone());
        for cs in CANON_SELS.iter().chain(CANON_SDK26_SELS) {
            if cs.class_name == c && cs.selector == selector {
                return Some((cs.class_name, cs.retain));
            }
        }
        for p in CANON_PARENTS.iter().chain(CANON_SDK26_PARENTS) {
            if p.child == c && !seen.iter().any(|x| x == p.parent) {
                todo.push(p.parent.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// R1 / R2 / R7: 宣言の canon 存在検査
// ---------------------------------------------------------------------------

/// R1: 全 SEL_* 宣言が canon (継承解決込み) に存在する。
pub fn audit_sel_decls(src: &str) -> Vec<Violation> {
    let mut v = Vec::new();
    let (decls, errs) = extract_sel_decls(src);
    for e in errs {
        v.push(Violation {
            rule: "R1-parse",
            detail: e,
        });
    }
    for d in &decls {
        if resolve_selector(&d.class_name, &d.selector).is_none() {
            v.push(Violation {
                rule: "R1",
                detail: format!(
                    "{}: ({}, \"{}\") は canon に存在しない (継承解決でも帰属なし)",
                    d.name, d.class_name, d.selector
                ),
            });
        }
    }
    v
}

/// R2: 全 MTLV_* enum 値が canon と厳密一致 (値まで) する。
pub fn audit_enum_consts(src: &str) -> Vec<Violation> {
    let mut v = Vec::new();
    let (consts, errs) = extract_enum_consts(src);
    for e in errs {
        v.push(Violation {
            rule: "R2-parse",
            detail: e,
        });
    }
    for c in &consts {
        let hit = CANON_METAL_ENUMS
            .iter()
            .chain(CANON_SDK26_ENUMS)
            .any(|e| e.enum_name == c.enum_name && e.variant == c.variant);
        if !hit {
            v.push(Violation {
                rule: "R2",
                detail: format!(
                    "{}: ({}, {}) が canon enum に存在しない",
                    c.name, c.enum_name, c.variant
                ),
            });
            continue;
        }
        let canon_val = CANON_METAL_ENUMS
            .iter()
            .chain(CANON_SDK26_ENUMS)
            .filter(|e| e.enum_name == c.enum_name && e.variant == c.variant)
            .map(|e| e.value)
            .next()
            .unwrap_or(i64::MIN);
        if canon_val != c.value {
            v.push(Violation {
                rule: "R2",
                detail: format!(
                    "{}: {}.{} の値が canon {canon_val} に対し {} と食い違う",
                    c.name, c.enum_name, c.variant, c.value
                ),
            });
        }
    }
    v
}

/// R7: CLASS_* の実名が canon 既知クラスに解決できる。
pub fn audit_class_consts(src: &str) -> Vec<Violation> {
    let mut v = Vec::new();
    for (name, cls) in extract_class_consts(src) {
        let known = CANON_SELS.iter().any(|s| s.class_name == cls)
            || CANON_SDK26_SELS.iter().any(|s| s.class_name == cls)
            || CANON_SDK26_CLASSES.iter().any(|c| *c == cls)
            || CANON_PARENTS
                .iter()
                .chain(CANON_SDK26_PARENTS)
                .any(|p| p.child == cls || p.parent == cls);
        if !known {
            v.push(Violation {
                rule: "R7",
                detail: format!("{name}: クラス {cls} は canon 未収録"),
            });
        }
    }
    v
}

// ---------------------------------------------------------------------------
// R3: selector コロン数 == trait 宣言データ引数個数 == 呼出実引数個数
// ---------------------------------------------------------------------------

/// src 内で whitespace を潰した正規形にする (シグネチャ形状照合用)。
fn squish(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// rt_src (objc_rt.rs) から ObjcRt dispatcher 表を抽出:
/// メソッド名 → sel 以降のデータ引数個数。
/// 対象は `fn <name>(&mut self, obj: ObjcId, sel: &'static core::ffi::CStr[, ...])` 形のみ。
pub fn extract_dispatch_table(rt_src: &str) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    let mut i = 0usize;
    while let Some(found) = rt_src[i..].find("fn ") {
        let start = i + found + 3;
        let name_end = rt_src[start..].find('(').map(|p| start + p);
        let Some(open) = name_end else { break };
        let name = rt_src[start..open].trim().to_string();
        // 対応する閉じカッコまでバランス走査。
        let mut depth = 0i32;
        let mut close = open;
        for (k, ch) in rt_src[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = open + k;
                        break;
                    }
                }
                _ => {}
            }
        }
        let params = squish(&rt_src[open + 1..close]);
        if params.starts_with("&mut self, obj: ObjcId, sel: &'static core::ffi::CStr") {
            let rest =
                params["&mut self, obj: ObjcId, sel: &'static core::ffi::CStr".len()..].trim();
            let data_args = if rest.is_empty() {
                0
            } else {
                rest.trim_start_matches(',')
                    .split(',')
                    .filter(|s| !s.trim().is_empty())
                    .count()
            };
            if !out.iter().any(|(n, _)| *n == name) {
                out.push((name, data_args));
            }
        }
        i = close.max(open) + 1;
    }
    out
}

/// 呼出サイト 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchCall {
    /// 行番号 (1 起算)。
    pub line: usize,
    /// dispatcher メソッド名。
    pub method: String,
    /// SEL_ 定数名 (存在すれば)。
    pub sel_const: Option<String>,
    /// sel 引数以降の実引数個数。
    pub data_args: usize,
}

/// use_src (metal_direct.rs) から `rt.<method>(...)` 呼出を全件抽出。
/// `(`)》のバランスは ()/{} 双方、文字列 literal 内の記号は無視する。
pub fn extract_dispatch_calls(use_src: &str) -> Vec<DispatchCall> {
    let mut out = Vec::new();
    let needle = "rt.";
    let mut pos = 0usize;
    // 行番号は事前に newline 位置テーブルで引く。
    let nl: Vec<usize> = use_src.match_indices('\n').map(|(i, _)| i).collect();
    let line_of = |p: usize| nl.partition_point(|&x| x < p) + 1;
    while let Some(f) = use_src[pos..].find(needle) {
        let s = pos + f + needle.len();
        let name_end = use_src[s..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map(|p| s + p)
            .unwrap_or(use_src.len());
        let name = use_src[s..name_end].to_string();
        if name.is_empty() || !use_src[name_end..].starts_with('(') {
            pos = name_end.max(s) + 1;
            continue;
        }
        let open = name_end;
        // 終了条件は () のバランスのみで確定 (文字列内記号は無視)。
        // ({} 内のカンマは終了に関与しないためここでは追跡不要、
        //  args 分割側で別途深度管理する)
        let mut pd = 0i32;
        let mut in_str = false;
        let mut close = open;
        for (k, ch) in use_src[open..].char_indices() {
            if in_str {
                if ch == '"' {
                    in_str = false;
                }
                continue;
            }
            match ch {
                '"' => in_str = true,
                '(' => pd += 1,
                ')' => {
                    pd -= 1;
                    if pd == 0 {
                        close = open + k;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &use_src[open + 1..close];
        // top level でカンマ分割 (pd==bd==0 のカンマのみ)。
        let mut args: Vec<String> = Vec::new();
        let mut cur = String::new();
        let (mut pd2, mut bd2, mut in_str2) = (0i32, 0i32, false);
        for ch in body.chars() {
            if in_str2 {
                if ch == '"' {
                    in_str2 = false;
                }
                cur.push(ch);
                continue;
            }
            match ch {
                '"' => {
                    in_str2 = true;
                    cur.push(ch);
                }
                '(' => {
                    pd2 += 1;
                    cur.push(ch);
                }
                ')' => {
                    pd2 -= 1;
                    cur.push(ch);
                }
                '{' => {
                    bd2 += 1;
                    cur.push(ch);
                }
                '}' => {
                    bd2 -= 1;
                    cur.push(ch);
                }
                ',' if pd2 == 0 && bd2 == 0 => {
                    args.push(cur.trim().to_string());
                    cur.clear();
                }
                _ => cur.push(ch),
            }
        }
        if !cur.trim().is_empty() {
            args.push(cur.trim().to_string());
        }
        // SEL_ トークンを持つ引数位置 (recv 直後が契約)。
        let sel_const = args.iter().find_map(|a| {
            let a = a.trim();
            if a.starts_with("SEL_") && a.ends_with(".1") {
                Some(a.trim_end_matches(".1").to_string())
            } else {
                None
            }
        });
        let data_args = if sel_const.is_some() {
            args.len().saturating_sub(2)
        } else {
            args.len()
        };
        out.push(DispatchCall {
            line: line_of(s),
            method: name,
            sel_const,
            data_args,
        });
        pos = close.max(open) + 1;
    }
    out
}

/// R3: three-way 一致の全件照合。
pub fn audit_dispatch_shapes(rt_src: &str, use_src: &str) -> Vec<Violation> {
    audit_dispatch_shapes_with_decls(rt_src, use_src, use_src)
}

/// R3 の宣言解決元を別指定する亜種 (wave 196 GO)。
/// metal_direct.rs / metal4_direct.rs は単一の binding 層として相互の SEL
/// 宣言を参照し合うため、呼出抽出は走査対象ファイル、宣言解決は層全体の
/// 連結ソースで行う必要がある。`decl_src` には宣言が集約されたソースを渡す。
pub fn audit_dispatch_shapes_with_decls(
    rt_src: &str,
    use_src: &str,
    decl_src: &str,
) -> Vec<Violation> {
    let mut v = Vec::new();
    let table = extract_dispatch_table(rt_src);
    let (sel_decls, _) = extract_sel_decls(decl_src);
    let calls = extract_dispatch_calls(use_src);
    for c in &calls {
        // インフラメソッド (dispatcher 非該当) は照合対象外。
        let is_infra = c.method.starts_with("on_")
            || matches!(
                c.method.as_str(),
                "mk" | "pool_push"
                    | "pool_pop"
                    | "c_mtl_default_device"
                    | "get_class"
                    | "reg_sel"
                    | "arena_bytes"
            );
        if is_infra {
            continue;
        }
        let Some(sel_name) = &c.sel_const else {
            v.push(Violation {
                rule: "R3",
                detail: format!(
                    "line {}: dispatcher 呼出 {} に SEL_*.1 トークンがない",
                    c.line, c.method
                ),
            });
            continue;
        };
        let Some((_, trait_args)) = table.iter().find(|(n, _)| n == &c.method) else {
            v.push(Violation {
                rule: "R3",
                detail: format!(
                    "line {}: dispatcher {} は trait 宣言に存在しない",
                    c.line, c.method
                ),
            });
            continue;
        };
        if c.data_args != *trait_args {
            v.push(Violation {
                rule: "R3",
                detail: format!(
                    "line {}: {} の実引数 {} 個に対し trait 宣言は {} 個",
                    c.line, c.method, c.data_args, trait_args
                ),
            });
        }
        let Some(decl) = sel_decls.iter().find(|d| &d.name == sel_name) else {
            v.push(Violation {
                rule: "R3",
                detail: format!("line {}: {sel_name} の SEL 宣言が見つからない", c.line),
            });
            continue;
        };
        let colons = decl.selector.matches(':').count();
        if colons != *trait_args {
            v.push(Violation {
                rule: "R3",
                detail: format!(
                    "line {}: {} (\"{}\") のコロン {} 個に対し dispatcher {} は {} 引数",
                    c.line, sel_name, decl.selector, colons, c.method, trait_args
                ),
            });
        }
    }
    v
}

// ---------------------------------------------------------------------------
// R4: extern "C" link_name の完全性
// ---------------------------------------------------------------------------

/// libobjc 既知 C 関数 (正典外のため厳選リスト、一次情報: Apple
/// Objective-C Runtime Reference)。objc_msgSend は variadic 本体のため
/// 宣言は 0 引数 (typed transmute 前提) とする規約。
pub const KNOWN_OBJC_CFNS: &[(&str, usize)] = &[
    ("objc_msgSend", 0),
    ("objc_getClass", 1),
    ("sel_registerName", 1),
    ("objc_autoreleasePoolPush", 0),
    ("objc_autoreleasePoolPop", 1),
];

/// (link_name, 宣言引数個数) を rt_src から抽出。
pub fn extract_link_cfns(rt_src: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut rest = rt_src;
    while let Some(p) = rest.find("#[link_name = \"") {
        let after = &rest[p + "#[link_name = \"".len()..];
        let Some(q) = after.find('"') else { break };
        let name = after[..q].to_string();
        // 後続の最初の `fn <local>(` の引数個数を数える。
        let Some(fp) = after.find("fn ") else { break };
        let fafter = &after[fp + 3..];
        let Some(o) = fafter.find('(') else { break };
        let params = &fafter[o + 1..];
        let Some(c) = params.find(')') else { break };
        let plist = params[..c].trim();
        let argc = if plist.is_empty() {
            0
        } else {
            plist.split(',').count()
        };
        out.push((name, argc));
        // after は rest[p+15..] 起点のため、走査継続も after 基準で進める。
        rest = &after[fp + 3 + o + c..];
    }
    out
}

/// R4: extern 宣言名と引数個数を正典 (libobjc 既知集合 ∪ CANON_METAL_CFNS) と照合。
pub fn audit_extern_cfns(rt_src: &str) -> Vec<Violation> {
    let mut v = Vec::new();
    for (name, argc) in extract_link_cfns(rt_src) {
        if let Some((_, spec_argc)) = KNOWN_OBJC_CFNS.iter().find(|(n, _)| *n == name) {
            if argc != *spec_argc && name != "objc_msgSend" {
                v.push(Violation {
                    rule: "R4",
                    detail: format!("{name}: 宣言引数 {argc} に対し正典は {spec_argc}"),
                });
            }
            continue;
        }
        match CANON_METAL_CFNS.iter().find(|f| f.name == name) {
            Some(f) => {
                if argc != f.argc {
                    v.push(Violation {
                        rule: "R4",
                        detail: format!("{name}: 宣言引数 {argc} に対し canon は {}", f.argc),
                    });
                }
            }
            None => v.push(Violation {
                rule: "R4",
                detail: format!(
                    "{name}: libobjc 既知集合にも CANON_METAL_CFNS にも存在しない extern 宣言"
                ),
            }),
        }
    }
    v
}

// ---------------------------------------------------------------------------
// R5: fixture 所有規則登録の canon 整合
// ---------------------------------------------------------------------------

/// on_make / on_borrow 登録 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureReg {
    /// "on_make" | "on_borrow"。
    pub kind: String,
    /// 受け側 class。
    pub recv: String,
    /// selector。
    pub sel: String,
    /// 行番号。
    pub line: usize,
}

/// fixture 登録の全件抽出 (論理フラグメント駆動、折返し非依存)。
pub fn extract_fixture_regs(src: &str) -> Vec<FixtureReg> {
    let mut out = Vec::new();
    for (ln, frag) in logical_statements(src) {
        for kind in ["on_make", "on_borrow"] {
            let needle = format!("rt.{kind}(");
            let Some(p) = frag.find(&needle) else {
                continue;
            };
            let mut c = Cur::new(&frag[p + needle.len()..]);
            let Some(recv) = c.str_lit() else { continue };
            if !c.eat(b',') {
                continue;
            }
            let Some(sel) = c.str_lit() else { continue };
            out.push(FixtureReg {
                kind: kind.to_string(),
                recv,
                sel,
                line: ln,
            });
        }
    }
    out
}

/// R5: on_make 登録は canon Owned、on_borrow は canon Borrowed に一致必須。
/// (fixture が ObjC メモリ規則を正しく模倣していることを機械保証する自己照査)
pub fn audit_fixture_retain(src: &str) -> Vec<Violation> {
    let mut v = Vec::new();
    for r in extract_fixture_regs(src) {
        let Some((resolved_on, retain)) = resolve_selector(&r.recv, &r.sel) else {
            v.push(Violation {
                rule: "R5",
                detail: format!(
                    "line {}: ({}, \"{}\") は canon に存在しない (fixture 不正)",
                    r.line, r.recv, r.sel
                ),
            });
            continue;
        };
        let expect_owned = r.kind == "on_make";
        let canon_owned = retain == RetainRule::Owned;
        if expect_owned != canon_owned {
            v.push(Violation {
                rule: "R5",
                detail: format!(
                    "line {}: {}(\"{}\", \"{}\") — canon ({}) は {:?} 規則 (所有規則と登録が不一致)",
                    r.line, r.kind, r.recv, r.sel, resolved_on, retain
                ),
            });
        }
    }
    v
}

/// R6: SEL_/MTLV_/CLASS_ 定数に消費者ゼロ宣言がない (§7 の機械化)。
pub fn audit_const_consumers(src: &str) -> Vec<Violation> {
    let mut v = Vec::new();
    let (sels, _) = extract_sel_decls(src);
    let (enums, _) = extract_enum_consts(src);
    let classes = extract_class_consts(src);
    let names: Vec<&str> = sels
        .iter()
        .map(|d| d.name.as_str())
        .chain(enums.iter().map(|d| d.name.as_str()))
        .chain(classes.iter().map(|(n, _)| n.as_str()))
        .collect();
    for n in names {
        // 単語境界一致で数える (SEL_NEW が SEL_NEW_COMMAND_QUEUE に
        // 部分一致する等の偽陰性を防ぐ)。後続が識別子文字なら別名扱い。
        let count = src
            .match_indices(n)
            .filter(|(i, _)| {
                src[i + n.len()..]
                    .chars()
                    .next()
                    .map(|c| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(true)
            })
            .count();
        if count < 2 {
            v.push(Violation {
                rule: "R6",
                detail: format!("{n}: 宣言のみで実使用なし (消費者ゼロ、§7 違反)"),
            });
        }
    }
    v
}

// ---------------------------------------------------------------------------
// 総合実行 (本 crate 内の実ソースに include_str! で固定配線)
// ---------------------------------------------------------------------------

/// 全規則を本 crate の実ソースに適用した総合監査。
/// 空 Vec = Apple 契約と binding 層の完全一致 (canon 真値との差分ゼロ)。
pub fn run_full_audit() -> Vec<Violation> {
    let use_src = include_str!("metal_direct.rs");
    let use_src4 = include_str!("metal4_direct.rs");
    let rt_src = include_str!("objc_rt.rs");
    let mut v = Vec::new();
    for u in [use_src, use_src4] {
        v.extend(audit_sel_decls(u));
        v.extend(audit_enum_consts(u));
        v.extend(audit_class_consts(u));
        v.extend(audit_fixture_retain(u));
        v.extend(audit_const_consumers(u));
    }
    // R3 三件照合: metal_direct / metal4_direct は単一 binding 層として
    // 相互の SEL 宣言を共有消費する (例: metal4_direct が SEL_CONTENTS を
    // 参照)。呼出抽出は各ファイル単位、宣言解決は層全体の連結ソースで行う。
    let decl_merged = format!("{use_src}\n{use_src4}");
    for u in [use_src, use_src4] {
        v.extend(audit_dispatch_shapes_with_decls(rt_src, u, &decl_merged));
    }
    v.extend(audit_extern_cfns(rt_src));
    // R6 相当の相互消費補完: metal4_direct.rs が classic の共有 SEL/MTLV/CLASS
    // 名前を追加消費することを許容する (metal_direct.rs 側の消費数は不変)。
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 変異サンプル: 元ソースへ差し替えを施して各規則の検出能力を証明する。
    /// (adversarial TDD: 監査機自体が機能していることを機械証明する)
    fn use_src() -> &'static str {
        include_str!("metal_direct.rs")
    }
    fn rt_src() -> &'static str {
        include_str!("objc_rt.rs")
    }

    #[test]
    fn gn_audit_full_clean() {
        let v = run_full_audit();
        assert!(v.is_empty(), "実装と canon の完全一致が前提: {:#?}", v);
    }

    #[test]
    fn gn_audit_detects_selector_typo() {
        let src = use_src().replace("c\"commit\"", "c\"comnit\"");
        let v = audit_sel_decls(&src);
        assert!(v.iter().any(|x| x.rule == "R1"), "typo を R1 が検出: {v:?}");
    }

    #[test]
    fn gn_audit_detects_enum_value_tamper() {
        let src = use_src().replace(
            "(\"MTLPixelFormat\", \"BGRA8Unorm\", 80)",
            "(\"MTLPixelFormat\", \"BGRA8Unorm\", 81)",
        );
        let v = audit_enum_consts(&src);
        assert!(
            v.iter().any(|x| x.rule == "R2"),
            "値改竄を R2 が検出: {v:?}"
        );
    }

    #[test]
    fn gn_audit_detects_arg_shape_mismatch() {
        let src = use_src().replacen(
            "SEL_SET_PIPELINE_STATE.1, spec.pipeline)",
            "SEL_SET_PIPELINE_STATE.1, spec.pipeline, 99)",
            1,
        );
        let v = audit_dispatch_shapes(rt_src(), &src);
        assert!(
            v.iter().any(|x| x.rule == "R3"),
            "引数過剰を R3 が検出: {v:?}"
        );
    }

    #[test]
    fn gn_audit_detects_wrong_dispatcher_name() {
        // 正しい selector に対し引数個数の違う dispatcher を使う改竄。
        let src = use_src().replacen(
            "rt.void_1p(enc, SEL_SET_PIPELINE_STATE.1, spec.pipeline);",
            "rt.void_2pu(enc, SEL_SET_PIPELINE_STATE.1, spec.pipeline);",
            1,
        );
        let v = audit_dispatch_shapes(rt_src(), &src);
        assert!(
            v.iter().any(|x| x.rule == "R3"),
            "dispatcher 名差し替えを R3 が検出: {v:?}"
        );
    }

    #[test]
    fn gn_audit_detects_unknown_extern() {
        let src = format!(
            "{}\nextern \"C\" {{\n    #[cfg(target_os = \"macos\")]\n    #[link_name = \"objc_tamperedSymbol\"]\n    fn tampered_link(x: ObjcId);\n}}\n",
            rt_src()
        );
        let v = audit_extern_cfns(&src);
        assert!(
            v.iter().any(|x| x.rule == "R4"),
            "未知 extern を R4 が検出: {v:?}"
        );
    }

    #[test]
    fn gn_audit_detects_extern_argc_drift() {
        // MTLCreateSystemDefaultDevice は canon argc=0 → 1 に改竄。
        let src = rt_src().replace(
            "fn mtl_create_system_default_device_link() -> ObjcId;",
            "fn mtl_create_system_default_device_link(x: ObjcId) -> ObjcId;",
        );
        let v = audit_extern_cfns(&src);
        assert!(
            v.iter().any(|x| x.rule == "R4"),
            "extern 引数改竄を R4 が検出: {v:?}"
        );
    }

    #[test]
    fn gn_audit_detects_retain_rule_swap() {
        // GN 自己照査で実際に起きた fixture 誤登録の再現:
        // colorAttachments (canon Borrowed) を on_make (Owned 期待) にする。
        // 変異はレイアウト非依存 (rustfmt 折返しに耐える anchor 逆引き)。
        let src = use_src();
        let anchor = "\"MTLRenderPipelineColorAttachmentDescriptorArray\"";
        let pos = match src.rfind(anchor) {
            Some(p) => p,
            None => panic!("anchor 不在"),
        };
        let bp = match src[..pos].rfind("rt.on_borrow(") {
            Some(p) => p,
            None => panic!("borrow 基点不在"),
        };
        let mutated = format!(
            "{}rt.on_make({}",
            &src[..bp],
            &src[bp + "rt.on_borrow(".len()..]
        );
        let v = audit_fixture_retain(&mutated);
        assert!(
            v.iter().any(|x| x.rule == "R5"),
            "所有規則 swap を R5 が検出: {v:?}"
        );
    }

    #[test]
    fn gn_audit_detects_dead_const() {
        let src = format!(
            "{}\npub const SEL_AUDIT_DEAD: (&str, &CStr) = (\"MTLDevice\", c\"name\");\n",
            use_src()
        );
        let v = audit_const_consumers(&src);
        assert!(
            v.iter().any(|x| x.rule == "R6"),
            "消費者ゼロ宣言を R6 が検出: {v:?}"
        );
    }

    #[test]
    fn gn_audit_detects_unknown_class() {
        let src = use_src().replace(
            "pub const CLASS_NSSTRING: &CStr = c\"NSString\";",
            "pub const CLASS_NSSTRING: &CStr = c\"NSstriong\";",
        );
        let v = audit_class_consts(&src);
        assert!(
            v.iter().any(|x| x.rule == "R7"),
            "未知クラス名を R7 が検出: {v:?}"
        );
    }

    #[test]
    fn gn_audit_inheritance_resolution_end_encoding() {
        // endEncoding は MTLRenderCommandEncoder ではなく親
        // MTLCommandEncoder 帰属: 継承解決が正しく効くことを直接 pin。
        let (on, retain) = match resolve_selector("MTLRenderCommandEncoder", "endEncoding") {
            Some(v) => v,
            None => panic!("endEncoding が canon 解決できない"),
        };
        assert_eq!(on, "MTLCommandEncoder");
        assert_eq!(retain, RetainRule::Borrowed);
    }

    fn use_src4() -> &'static str {
        include_str!("metal4_direct.rs")
    }

    /// SDK26 canon 表の存在と機械量 pin (生成値の不変条件)。
    #[test]
    fn go_canon_sdk26_table_shape_pins() {
        // 生成量 (tools/apple_canon_gen.py 機械値、98 Metal ヘッダから転記)。
        assert_eq!(CANON_SDK26_SELS.len(), 2476, "SDK26 SEL 件数 pin");
        assert_eq!(CANON_SDK26_ENUMS.len(), 902, "SDK26 enum 件数 pin");
        assert_eq!(CANON_SDK26_CLASSES.len(), 231, "SDK26 class 件数 pin");
        assert_eq!(CANON_SDK26_PARENTS.len(), 316, "SDK26 parent 件数 pin");
        // 重複ゼロ (機械生成契約)。
        let mut seen = std::collections::BTreeSet::new();
        for s in CANON_SDK26_SELS {
            assert!(
                seen.insert((s.class_name, s.selector)),
                "duplicate SDK26 sel: {:?}",
                (s.class_name, s.selector)
            );
        }
        let mut seen_e = std::collections::BTreeSet::new();
        for e in CANON_SDK26_ENUMS {
            assert!(
                seen_e.insert((e.enum_name, e.variant)),
                "duplicate SDK26 enum: {:?}",
                (e.enum_name, e.variant)
            );
        }
    }

    /// Metal 4 硬値の canon 存在 (MTLGPUFamilyMetal4=5002 は MTLDevice.h:255
    /// の逐語、MTLStages/MTLRenderStages は bit 位置厳密)。
    #[test]
    fn go_canon_sdk26_metal4_enum_exact() {
        let has = |e: &str, v: &str, val: i64| {
            CANON_SDK26_ENUMS
                .iter()
                .any(|x| x.enum_name == e && x.variant == v && x.value == val)
        };
        assert!(
            has("MTLGPUFamily", "Metal4", 5002),
            "Metal4=5002 canon 必須"
        );
        assert!(has("MTLStages", "Vertex", 1));
        assert!(has("MTLStages", "Fragment", 2));
        assert!(has("MTLStages", "Dispatch", 1 << 27));
        assert!(has("MTLRenderStages", "Vertex", 1));
        assert!(has("MTLRenderStages", "Fragment", 2));
        assert!(has("MTL4CommandQueueError", "Timeout", 1));
        assert!(has("MTL4VisibilityOptions", "Device", 1));
    }

    /// 消費する全 MTL4 SEL が canon union で継承解決込み解決できる
    /// (metal4_direct.rs の実 selector 集合との対応を直接 pin)。
    #[test]
    fn go_canon_sdk26_mtl4_consumed_sels_resolve() {
        let need: &[(&str, &str)] = &[
            ("MTLDevice", "newMTL4CommandQueue"),
            ("MTLDevice", "newCommandBuffer"),
            ("MTLDevice", "newCommandAllocatorWithDescriptor:error:"),
            ("MTLDevice", "newArgumentTableWithDescriptor:error:"),
            ("MTLDevice", "newCompilerWithDescriptor:error:"),
            ("MTLDevice", "newResidencySetWithDescriptor:error:"),
            ("MTLDevice", "newSharedEvent"),
            (
                "MTL4Compiler",
                "newRenderPipelineStateWithDescriptor:compilerTaskOptions:error:",
            ),
            ("MTL4CommandQueue", "addResidencySet:"),
            ("MTL4CommandQueue", "commit:count:"),
            ("MTL4CommandQueue", "signalEvent:value:"),
            ("MTL4CommandQueue", "signalDrawable:"),
            ("MTL4CommandQueue", "waitForDrawable:"),
            ("MTL4CommandAllocator", "reset"),
            ("MTL4CommandBuffer", "beginCommandBufferWithAllocator:"),
            ("MTL4CommandBuffer", "endCommandBuffer"),
            ("MTL4CommandBuffer", "renderCommandEncoderWithDescriptor:"),
            ("MTL4CommandBuffer", "useResidencySet:"),
            ("MTL4ArgumentTable", "setAddress:atIndex:"),
            ("MTL4ArgumentTable", "setTexture:atIndex:"),
            ("MTL4RenderCommandEncoder", "setArgumentTable:atStages:"),
            ("MTL4RenderCommandEncoder", "setRenderPipelineState:"),
            ("MTL4RenderCommandEncoder", "setViewport:"),
            (
                "MTL4RenderCommandEncoder",
                "drawPrimitives:vertexStart:vertexCount:",
            ),
            ("MTLSharedEvent", "waitUntilSignaledValue:timeoutMS:"),
            ("MTLBuffer", "gpuAddress"),
            ("MTLTexture", "gpuResourceID"),
            ("MTLResidencySet", "addAllocation:"),
            ("MTLResidencySet", "commit"),
            ("MTLDrawable", "present"),
        ];
        for (c, s) in need {
            assert!(
                resolve_selector(c, s).is_some(),
                "consumed sel ({c}, \"{s}\") が canon union に必要"
            );
        }
        // endEncoding は親 MTL4CommandEncoder 帰属の継承解決を直接 pin。
        let (on, retain) = match resolve_selector("MTL4RenderCommandEncoder", "endEncoding") {
            Some(v) => v,
            None => panic!("MTL4 endEncoding 継承解決失敗"),
        };
        assert_eq!(on, "MTL4CommandEncoder");
        assert_eq!(retain, RetainRule::Borrowed);
    }

    /// 変異: SDK26 帰属 selector の typo を R1 が検出する。
    #[test]
    fn go_audit_detects_sdk26_selector_typo() {
        let src = use_src4().replace("c\"commit:count:\"", "c\"conmit:count:\"");
        assert!(src.contains("conmit"), "変異適用確認");
        let v = audit_sel_decls(&src);
        assert!(
            v.iter().any(|x| x.rule == "R1"),
            "SDK26 sel typo を R1 が検出: {v:?}"
        );
    }

    /// 変異: Metal4 family 値の改竄 (5002→5003) を R2 が検出する。
    #[test]
    fn go_audit_detects_sdk26_enum_tamper() {
        let src = use_src4().replace(
            "(\"MTLGPUFamily\", \"Metal4\", 5002)",
            "(\"MTLGPUFamily\", \"Metal4\", 5003)",
        );
        assert!(src.contains("5003"), "変異適用確認");
        let v = audit_enum_consts(&src);
        assert!(
            v.iter().any(|x| x.rule == "R2"),
            "Metal4=5003 改竄を R2 が検出: {v:?}"
        );
    }

    /// 変異: metal4_direct の dispatcher 名差替 (arg 個数不整合) を R3 が検出。
    #[test]
    fn go_audit_detects_sdk26_arg_shape_mismatch() {
        let src = use_src4().replacen(
            "SEL_MTL4_DRAW_PRIMITIVES.1,\n            MTLV_PRIM_TRIANGLE.2 as u64,",
            "SEL_MTL4_DRAW_PRIMITIVES.1,\n            MTLV_PRIM_TRIANGLE.2 as u64,\n            0,",
            1,
        );
        // replacen 失効時は源不一致 → 代替 marker 変異
        let src = if src.contains(",\n            0,\n            verts.len()") {
            src
        } else {
            use_src4().replacen("rt.void_3uuu(", "rt.void_2pu(", 1)
        };
        let v = audit_dispatch_shapes(rt_src(), &src);
        assert!(
            v.iter().any(|x| x.rule == "R3"),
            "MTL4 呼出引数形状改竄を R3 が検出: {v:?}"
        );
    }

    /// 変異: SDK26 SEL 宣言の消費者ゼロ死蔵を R6 が検出する。
    #[test]
    fn go_audit_detects_dead_sdk26_const() {
        let src = format!(
            "{}\npub const SEL_MTL4_DEAD: (&str, &CStr) = (\"MTL4CommandQueue\", c\"commit:count:\");\n",
            use_src4()
        );
        let v = audit_const_consumers(&src);
        assert!(
            v.iter().any(|x| x.rule == "R6"),
            "SDK26 消費者ゼロ宣言を R6 が検出: {v:?}"
        );
    }
}
