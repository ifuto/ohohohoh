#!/usr/bin/env python3
"""apple_canon.rs 機械生成 (手編集禁止)。
一次情報: vendor objc2-metal 0.2.2 + objc2-foundation 0.2.2 + objc2-quartz-core 0.2.2
(いずれも Apple SDK ヘッダの header-translator 機械転記)。
objc2 生成コードの `セレクタ末尾 :_` は error out-param の objc2 内部規約に
由来する余剰断片 (real selector はその _: 部分が実引数) のため、正典化時に
`:_` 断片を実 selector 形へ正規化する (metal-rs 0.28.0 実戦 selector との
突合で確認: 例 newLibraryWithSource:options:error:_ → ...error:)。
"""
import re, glob, os

OUT = "/home/user/rsift/rsift/crates/rsift-opt-gfx/src/apple_canon.rs"
ATTR = re.compile(
    r'#\[method(?:_id)?\((?:@__retain_semantics (\w+) )?([^\]]+)\)\]'
    r'|#\[unsafe\(method(?:_id)?\((?:@__retain_semantics (\w+) )?([^\]]+)\)\)\]')

def apple_family_owned(bare):
    """Apple ObjC 命名規約の所有族判定 (一次情報: Apple Advanced Memory
    Management Programming Guide の method family 規則 — 語頭が
    alloc/new/copy/mutableCopy/init で、直後が小文字でない場合のみ族)。
    逆例: newlineCharacterSet (new+lower → 族外)、initialize (init+lower
    → 族外)。Foundation は @__retain_semantics を持たない行が多く、
    本規則が canon の retain 精度を担保する。"""
    for p_ in ("alloc", "new", "copy", "mutableCopy", "init"):
        if bare == p_:
            return True
        if bare.startswith(p_) and len(bare) > len(p_) and not bare[len(p_)].islower():
            return True
    return False

def normalize_sel(sel):
    # "foo:error:_" の末尾 ":_" は objc2 内部規約断片 → "foo:error:" へ。
    # (実 selector は metal-rs 実戦値と一次情報突合済)
    if sel.endswith(":_"):
        return sel[:-1]
    return sel

rows = {}; order = []
PARENT_IMPLS = []
SRC_MAP = [
    ("/tmp/rsift-vendor/vendor/objc2-metal/src/generated", "MTL"),
    ("/tmp/rsift-vendor/vendor/objc2-foundation/src/generated", "NS"),
    ("/tmp/rsift-vendor/vendor/objc2-quartz-core/src/generated", "CA"),
]
for vendor_dir, _prefix in SRC_MAP:
    for f in sorted(glob.glob(os.path.join(vendor_dir, "*.rs"))):
        src = open(f, encoding="utf-8").read()
        cur = os.path.basename(f).replace(".rs", "")
        for line in src.splitlines():
            t = re.match(r'\s*pub unsafe trait (\w+)', line)
            if t: cur = t.group(1)
            # impl Proto for Type 形: メソッド帰属は trait 名側ではなく型側。
            # (Foundation は extern_methods! を bare impl Type { } 内に書く)
            i4 = re.match(r'\s*(?:unsafe )?impl (\w+) for (\w+)', line)
            i5 = re.match(r'\s*(?:unsafe )?impl (\w+)\s*\{', line)
            if i4:
                cur = i4.group(2)
                PARENT_IMPLS.append((i4.group(2), i4.group(1)))
            elif i5:
                cur = i5.group(1)
            else:
                e = re.match(r'\s*pub (?:unsafe )?(?:struct|class) (\w+)', line)
                if e: cur = e.group(1)
            m = ATTR.search(line)
            if m:
                g = m.groups()
                retain, sel = (g[0], g[1]) if g[1] is not None else (g[2], g[3])
                sel = normalize_sel(sel)
                key = (cur, sel)
                if key not in rows:
                    order.append(key)
                    rows[key] = retain or "Get"

ENUMHDR = re.compile(r'pub struct (\w+)\(pub (?:NSInteger|NSUInteger)\);')
ENUMVAL = re.compile(r'pub const (\w+): Self = Self\((-?\d+)\);')
# bitflags 形式: impl MTLTextureUsage: NSUInteger { const RenderTarget = 0x0004; }
ENUMTYPE = re.compile(r'impl (MTL\w+): (?:NS)?U?Integer \{')
ENUMBIT = re.compile(r'const (\w+) = (0x[0-9A-Fa-f]+|\d+);')
enums = {}
for f in sorted(glob.glob("/tmp/rsift-vendor/vendor/objc2-metal/src/generated/MTL*.rs")):
    src = open(f, encoding="utf-8").read()
    cur = None
    for line in src.splitlines():
        h = ENUMHDR.search(line)
        if h:
            cur = h.group(1); enums.setdefault(cur, [])
        t = ENUMTYPE.search(line)
        if t:
            cur = t.group(1); enums.setdefault(cur, [])
        v = ENUMVAL.search(line)
        if v and cur:
            enums[cur].append((v.group(1), int(v.group(2))))
        b = ENUMBIT.search(line)
        if b and cur:
            enums[cur].append((b.group(1), int(b.group(2), 0)))

CFUN = re.compile(r'pub fn (MTL\w+)\(([^)]*)\)\s*(?:->\s*([^;]+?))?;', re.S)
cfuns = []
for f in sorted(glob.glob("/tmp/rsift-vendor/vendor/objc2-metal/src/generated/MTL*.rs")):
    src = open(f, encoding="utf-8").read()
    for blk in re.findall(r'extern "C" \{(.*?)\}', src, re.S):
        for m in CFUN.finditer(blk):
            name, args, ret = m.groups()
            argc = 0 if args.strip() == "" else len([a for a in args.split(",") if a.strip()])
            cfuns.append((name, argc, (ret or "()").strip()))
cfuns = sorted(set(cfuns))

# NSObject 普遍 selector (全 ObjC オブジェクトが応答、一次情報:
# objc2-0.5.2 top_level_traits.rs retain/release/autorelease 契約記述 +
# Apple Objective-C NSObject プロトコル文書)。CanonSel でない別枠ではなく
# CANON_SELS 末尾へ統合 (監査は同一表で完結する設計)。
NSOBJCORE = [
    ("NSObject", "retain", "Get"), ("NSObject", "release", "Get"),
    ("NSObject", "autorelease", "Get"), ("NSObject", "class", "Get"),
    ("NSObject", "description", "Get"), ("NSObject", "isKindOfClass:", "Get"),
    ("NSObject", "respondsToSelector:", "Get"),
    # クラスオブジェクト側 (alloc/new 系、Apple NSObject 文書の普遍契約)
    ("NSObject", "alloc", "New"), ("NSObject", "new", "New"),
    ("NSObject", "allocWithZone:", "New"), ("NSObject", "init", "New"),
]
for c_, s_, r_ in NSOBJCORE:
    if (c_, s_) not in rows:
        order.append((c_, s_)); rows[(c_, s_)] = r_

# ---- 継承 (プロトコル/クラス) 正典表の機械抽出 ----
# pub unsafe trait Child: Parent + P2 → (Child, Parent)
# extern_class ClassType: type Super = Parent → (Child, Parent)
PARENTS = set()
TPAT = re.compile(r'pub unsafe trait (\w+)\s*:\s*([\w\s,+]+)')
for vendor_dir, _p in SRC_MAP:
    for f in sorted(glob.glob(os.path.join(vendor_dir, "*.rs"))):
        src = open(f, encoding="utf-8").read()
        for m in TPAT.finditer(src):
            child, parents = m.groups()
            for par in re.split(r'[+,]', parents):
                par = par.strip()
                if par and par[0].isupper():
                    PARENTS.add((child, par))
        # ClassType Super
        for m in re.finditer(r'unsafe impl ClassType for (\w+) \{[^}]*?type Super = (\w+)', src, re.S):
            PARENTS.add((m.group(1), m.group(2)))
        # objc2 0.6 extern_class! の superclass 宣言:
        # #[unsafe(super(NSObject))] 直後の pub struct X; から (X, super) 辺。
        for m in re.finditer(r'#\[unsafe\(super\((\w+)\)\)\].{0,400}?pub struct (\w+);', src, re.S):
            PARENTS.add((m.group(2), m.group(1)))
# impl Proto for Type 形の適合宣言も継承解決に統合
# (例: unsafe impl NSObjectProtocol for NSString → (NSString, NSObjectProtocol))
for _ty, _proto in PARENT_IMPLS:
    if _ty[0].isupper() and _proto[0].isupper():
        PARENTS.add((_ty, _proto))
PARENTS = sorted(PARENTS)
print(f"selectors: {len(rows)}, enums: {len(enums)}, c fns: {len(cfuns)}, parents: {len(PARENTS)}")

with open(OUT, "w", encoding="utf-8") as w:
    w.write("//! 【機械生成・手編集禁止】Apple Metal/Foundation/QuartzCore 正典 API 表 (canon)。\n")
    w.write("//! 生成: tools/apple_canon_gen.py / 一次情報: vendor objc2-metal 0.2.2 +\n")
    w.write("//! objc2-foundation 0.2.2 + objc2-quartz-core 0.2.2 (全て Apple SDK ヘッダの\n")
    w.write("//! header-translator 機械転記)。apple_ffi_audit が binding 層の selector/\n")
    w.write("//! enum/extern 宣言と照合する静的解析の正典。\n\n")
    w.write("/// retain 規則 (ObjC メモリ管理正典)。\n")
    w.write("#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n")
    w.write("pub enum RetainRule {\n")
    w.write("    /// new/copy/mutableCopy/alloc/init 系: 呼出側所有 (+1)、Drop で release。\n")
    w.write("    Owned,\n")
    w.write("    /// それ以外: 所有なし借用、release 禁止。\n")
    w.write("    Borrowed,\n")
    w.write("}\n\n")
    w.write("/// 正典 selector 1 件。\n")
    w.write("#[derive(Debug, Clone, Copy)]\n")
    w.write("pub struct CanonSel {\n")
    w.write("    /// 所属クラス/プロトコル名。\n")
    w.write("    pub class_name: &'static str,\n")
    w.write("    /// selector 本体 (実 ObjC 形、`:_` 断片正規化済)。\n")
    w.write("    pub selector: &'static str,\n")
    w.write("    /// 戻り値の所有規則 (正典)。\n")
    w.write("    pub retain: RetainRule,\n")
    w.write("}\n\n")
    w.write(f"/// 正典 selector 表 ({len(rows)} 件、重複除去+正規化済、生成値)。\n")
    w.write("#[rustfmt::skip]\npub static CANON_SELS: &[CanonSel] = &[\n")
    for (cls, sel) in order:
        tag = rows[(cls, sel)]
        bare = sel.split(":")[0]
        r = "Owned" if (tag in ("New", "Copy", "Init") or apple_family_owned(bare)) else "Borrowed"
        w.write(f'    CanonSel {{ class_name: "{cls}", selector: "{sel}", retain: RetainRule::{r} }},\n')
    w.write("];\n\n")
    w.write("/// 正典 enum 値 1 件。\n")
    w.write("#[derive(Debug, Clone, Copy)]\n")
    w.write("pub struct CanonEnumVal {\n")
    w.write("    /// enum 型名。\n")
    w.write("    pub enum_name: &'static str,\n")
    w.write("    /// バリアント名。\n")
    w.write("    pub variant: &'static str,\n")
    w.write("    /// 値 (正典)。\n")
    w.write("    pub value: i64,\n")
    w.write("}\n\n")
    total_enum = sum(len(v) for v in enums.values())
    w.write(f"/// 正典 Metal enum 値表 ({total_enum} 件、生成値)。\n")
    w.write("#[rustfmt::skip]\npub static CANON_METAL_ENUMS: &[CanonEnumVal] = &[\n")
    for en in sorted(enums):
        for (name, val) in enums[en]:
            w.write(f'    CanonEnumVal {{ enum_name: "{en}", variant: "{name}", value: {val} }},\n')
    w.write("];\n\n")
    w.write("/// 正典 C エントリ関数 1 件。\n")
    w.write("#[derive(Debug, Clone, Copy)]\n")
    w.write("pub struct CanonCFn {\n")
    w.write("    /// 関数名。\n")
    w.write("    pub name: &'static str,\n")
    w.write("    /// 引数個数 (正典)。\n")
    w.write("    pub argc: usize,\n")
    w.write("}\n\n")
    w.write("/// 継承関係 1 件 (子 class/protocol → 親、監査の selector 帰属解決用)。\n")
    w.write("#[derive(Debug, Clone, Copy)]\n")
    w.write("pub struct CanonParent {\n")
    w.write("    /// 子。\n")
    w.write("    pub child: &\'static str,\n")
    w.write("    /// 親。\n")
    w.write("    pub parent: &\'static str,\n")
    w.write("}\n\n")
    w.write(f"/// 正典継承表 ({len(PARENTS)} 件、生成値)。\n")
    w.write("#[rustfmt::skip]\npub static CANON_PARENTS: &[CanonParent] = &[\n")
    for (c_, p_) in PARENTS:
        w.write(f'    CanonParent {{ child: "{c_}", parent: "{p_}" }},\n')
    w.write("];\n\n")
    w.write(f"/// 正典 Metal C エントリ関数表 ({len(cfuns)} 件、生成値)。\n")
    w.write("#[rustfmt::skip]\npub static CANON_METAL_CFNS: &[CanonCFn] = &[\n")
    for (name, argc, _ret) in cfuns:
        w.write(f'    CanonCFn {{ name: "{name}", argc: {argc} }},\n')
    w.write("];\n")
print("wrote", OUT)


# ===========================================================================
# SDK26 supplement — Apple SDK 26.5 (Metal 4) 一次情報ヘッダ canon
#   一次情報: alexey-lysiuk/macos-sdk (GitHub) MacOSX26.5.sdk
#   System/Library/Frameworks/Metal.framework/Versions/A/Headers/*.h
#   (Apple 公式 SDK の verbatim ミラー) からの機械転記。
#   32 個の MTL4* 新規 API + macOS 26 時点の classic 全 enum を正典化し、
#   CANON_SDK26_* (marker 区画、冪等再生成) として apple_canon.rs へ追記する。
# ===========================================================================

SDK26_CAND = [
    os.environ.get("RSIFT_APPLE_SDK26_DIR"),
    "/home/user/rsift-scratch/sdk26",
    "/tmp/sdk26",
    "/home/user/rsift/sdk26",
]

SDK26_BEGIN = "// ---- CANON_SDK26_BEGIN (machine generated, do not hand-edit) ----"
SDK26_END = "// ---- CANON_SDK26_END ----"


def _sdk26_dir():
    for d in SDK26_CAND:
        if d and os.path.isdir(d) and glob.glob(os.path.join(d, "*.h")):
            return d
    return None


def _sdk_strip_comments(text):
    out = []
    i, n = 0, len(text)
    state = "code"
    while i < n:
        c = text[i]
        nx = text[i + 1] if i + 1 < n else ""
        if state == "code":
            if c == '"':
                out.append(c); state = "str"; i += 1
            elif c == "'":
                out.append(c); state = "char"; i += 1
            elif c == "/" and nx == "/":
                state = "line"; i += 2
            elif c == "/" and nx == "*":
                state = "block"; i += 2
            else:
                out.append(c); i += 1
        elif state == "str":
            out.append(c)
            if c == "\\" and i + 1 < n:
                out.append(text[i + 1]); i += 2
            elif c == '"':
                state = "code"; i += 1
            else:
                i += 1
        elif state == "char":
            out.append(c)
            if c == "\\" and i + 1 < n:
                out.append(text[i + 1]); i += 2
            elif c == "'":
                state = "code"; i += 1
            else:
                i += 1
        elif state == "line":
            if c == "\n":
                out.append("\n"); state = "code"
            i += 1
        else:  # block comment
            if c == "*" and nx == "/":
                state = "code"; i += 2
            else:
                if c == "\n":
                    out.append("\n")
                i += 1
    return "".join(out)


# 関数的マクロ (一次情報 inventory 閉鎖確認済、wave 196 GO):
# API_AVAILABLE 432 / API_UNAVAILABLE 63 / API_DEPRECATED_WITH_REPLACEMENT 13 /
# API_DEPRECATED 3 / NS_SWIFT_UNAVAILABLE_FROM_ASYNC 1 / NS_SWIFT_UNAVAILABLE 1
_SDK26_CALL_MACROS = (
    "API_AVAILABLE", "API_UNAVAILABLE", "API_DEPRECATED_WITH_REPLACEMENT",
    "API_DEPRECATED", "API_INTRODUCED", "NS_SWIFT_UNAVAILABLE_FROM_ASYNC",
    "NS_SWIFT_UNAVAILABLE", "NS_SWIFT_NAME", "NS_REFINED_FOR_SWIFT",
    "NS_HEADER_AUDIT_BEGIN",
)

_SDK26_BARE_TOKENS = (
    "MTL_EXPORT", "NS_DESIGNATED_INITIALIZER", "__autoreleasing",
    "_Nonnull", "_Nullable", "_Nullable_result", "__nullable",
    "NS_ASSUME_NONNULL_BEGIN", "NS_ASSUME_NONNULL_END", "nullable",
)


def _sdk_remove_call_macros(text):
    """NAME(...) を NAME の ident 境界 + paren 任意深度平衡で除去。"""
    out = []
    i, n = 0, len(text)
    while i < n:
        m = re.match(r"[A-Za-z_]\w*", text[i:])
        if m and m.group(0) in _SDK26_CALL_MACROS:
            j = i + len(m.group(0))
            while j < n and text[j] in " \t":
                j += 1
            if j < n and text[j] == "(":
                depth, k = 1, j + 1
                while k < n and depth > 0:
                    if text[k] == "(":
                        depth += 1
                    elif text[k] == ")":
                        depth -= 1
                    k += 1
                out.append(" ")
                i = k
                continue
        out.append(text[i])
        i += 1
    return "".join(out)


def _sdk_remove_preprocessor(text):
    lines = text.split("\n")
    out = []
    cont = False
    for ln in lines:
        if cont:
            if ln.rstrip().endswith("\\"):
                continue
            cont = False
            continue
        if ln.strip().startswith("#"):
            if ln.rstrip().endswith("\\"):
                cont = True
            continue
        out.append(ln)
    return "\n".join(out)


def _sdk_clean(text):
    text = _sdk_strip_comments(text)
    text = _sdk_remove_call_macros(text)
    text = _sdk_remove_preprocessor(text)
    for tok in _SDK26_BARE_TOKENS:
        text = re.sub(r"\b" + re.escape(tok) + r"\b", " ", text)
    return text


def _split_statements(body):
    """';' 区切り (深さ 0・文字列吸収)。括弧類は任意深度追跡。"""
    stmts = []
    depth = 0
    cur = []
    i, n = 0, len(body)
    in_str = False
    while i < n:
        c = body[i]
        if in_str:
            cur.append(c)
            if c == "\\" and i + 1 < n:
                cur.append(body[i + 1]); i += 2
                continue
            if c == '"':
                in_str = False
            i += 1
            continue
        if c == '"':
            in_str = True; cur.append(c); i += 1; continue
        if c in "([{<":
            depth += 1
        elif c in ")]}>":
            depth -= 1
        if c == ";" and depth == 0:
            stmts.append("".join(cur))
            cur = []
        else:
            cur.append(c)
        i += 1
    if "".join(cur).strip():
        stmts.append("".join(cur))
    return stmts


_SDK26_NUM_IDS = {
    "NSIntegerMax": 2**63 - 1,
    "NSUIntegerMax": 2**64 - 1,
    "NSIntegerMin": -(2**63),
    "INT64_MAX": 2**63 - 1,
    "UINT64_MAX": 2**64 - 1,
}


def _sdk_eval_const(expr, known):
    """enum 値式の再帰下降 eval。一次情報語彙 (閉鎖): 数値 (0x, U/L 接尾)、
    <<, >>, |, &, +, -, ~, (, ) と NSIntegerMax/NSUIntegerMax、同 enum 内の
    alias 参照 (vals dict) のみ。"""
    toks = re.findall(
        r"0[xX][0-9A-Fa-f]+[uUlL]*|[0-9]+[uUlL]*|[A-Za-z_]\w*|<<|>>|[|&+\-~()]",
        expr,
    )
    pos = [0]

    def peek():
        return toks[pos[0]] if pos[0] < len(toks) else None

    def take():
        t = toks[pos[0]]; pos[0] += 1; return t

    def primary():
        t = take()
        if t == "(":
            v = or_expr()
            assert peek() == ")", f"missing ) in {expr!r}"
            take()
            return v
        if t == "-":
            return -primary()
        if t == "~":
            return ~primary()
        if t.startswith(("0x", "0X")):
            return int(re.sub(r"[uUlL]+$", "", t), 16)
        if re.match(r"^[0-9]", t):
            return int(re.sub(r"[uUlL]+$", "", t))
        if t in known:
            return known[t]
        if t in _SDK26_NUM_IDS:
            return _SDK26_NUM_IDS[t]
        raise ValueError(f"unknown ident {t!r} in enum expr {expr!r}")

    def shift():
        v = primary()
        while peek() in ("<<", ">>"):
            op = take()
            r = primary()
            v = v << r if op == "<<" else v >> r
        return v

    def term():
        v = shift()
        while peek() in ("|", "&", "+"):
            op = take()
            r = shift()
            if op == "|":
                v = v | r
            elif op == "&":
                v = v & r
            else:
                v = v + r
        return v

    def or_expr():
        return term()

    v = or_expr()
    if pos[0] != len(toks):
        raise ValueError(f"trailing tokens in enum expr {expr!r}: {toks[pos[0]:]}")
    return v


def _sdk_variant_names(enum_name, variants):
    """MTLStages 系の共通接頭辞を除去して objc2 命名規則と整合させる。
    一次情報規則: ObjC enum バリアントは
    `<Enum名><Variant名>` (または `s` 揺れ `MTLStages`→`MTLStageVertex`)
    で、接頭辞は全バリアントの LCP。LCP 除字で空になる例外は原名保持。"""
    names = [v for (v, _val) in variants]
    pref = os.path.commonprefix(names) if names else ""
    out = []
    for (v, val) in variants:
        stripped = v[len(pref):] if len(pref) >= 3 and v[len(pref):] else v
        out.append((stripped, val))
    return out


_HDR_RE = re.compile(r"@(interface|protocol)\s+([A-Za-z_]\w*)\s*"
                     r"(?:\(([^)]*)\))?\s*(?::\s*([A-Za-z_]\w*))?\s*"
                     r"(?:<([^>]*)>)?", re.S)
_PROP_RE = re.compile(r"^@property\s*\(([^)]*)\)\s*(.+?)$", re.S)
_METH_RE = re.compile(r"^[-+]\s*\((.*?)\)\s*(.*)$", re.S)
_PARAM_RE = re.compile(r"([A-Za-z_]\w*)\s*:\s*\([^()]*\)")


def _sdk_collect_defines(raw_text):
    """object-like `#define NAME <const-expr>` を (name, expr) で収集。
    関数型 (NAME(...) 直後空白なし) は対象外。一次情報: MTLResourceOptions
    が参照する MTLResource{CPUCacheMode,StorageMode,HazardTrackingMode}Shift
    定数群 (MTLResource.h) 等。"""
    out = []
    for ln in raw_text.splitlines():
        st = ln.strip()
        m = re.match(r"^#define\s+([A-Za-z_]\w*)[ \t]+(.+?)\s*$", st)
        if not m:
            continue
        name, expr = m.group(1), m.group(2)
        # 引数列誤認防止: NAME 直後が ( の行は呼出側 regex が拾わない
        # (\s+ 必須)。文字列リテラル定義は enum 非参照のため除外。
        if expr.startswith('"'):
            continue
        out.append((name, expr))
    return out


def parse_sdk26(sdk_dir):
    sels = {}
    sel_order = []
    parents = set()
    classes = set()
    enums = {}

    def emit_sel(cls, sel):
        key = (cls, sel)
        if key not in sels:
            sel_order.append(key)
            sels[key] = "Owned" if apple_family_owned(sel.split(":")[0]) else "Borrowed"

    # ---- Phase 1: 全ファイルの raw 収集 (enum expr は未評価) ----
    raw_enums = []  # (name, variants[(variant, expr|None)], file_order)
    raw_defs = []
    for f in sorted(glob.glob(os.path.join(sdk_dir, "*.h"))):
        raw_text = open(f, encoding="utf-8").read()
        raw_defs.extend(_sdk_collect_defines(raw_text))
        src = _sdk_clean(raw_text)
        for m in re.finditer(
            r"typedef\s+NS_(?:ENUM|OPTIONS)\s*\(\s*([A-Za-z_]\w*)\s*,"
            r"\s*([A-Za-z_]\w*)\s*\)\s*\{(.*?)\}\s*\w*\s*;?",
            src, re.S):
            _basetype, ename, body = m.groups()
            items = []
            depth = 0
            cur = []
            for ch in body:
                if ch in "([":
                    depth += 1
                elif ch in ")]":
                    depth -= 1
                if ch == "," and depth == 0:
                    items.append("".join(cur))
                    cur = []
                else:
                    cur.append(ch)
            items.append("".join(cur))
            variants = []
            for it in items:
                mm = re.match(r"^\s*([A-Za-z_]\w*)\s*(?:=\s*(.*?))?\s*$", it, re.S)
                if not mm:
                    continue
                variants.append((mm.group(1), (mm.group(2) or "").strip() or None))
            if variants:
                raw_enums.append((ename, variants))

    # ---- Phase 2: define + enum の fixpoint 評価 (前方参照許容) ----
    known = dict(_SDK26_NUM_IDS)
    pend_defs = list(raw_defs)
    for _round in range(16):
        progress = False
        still = []
        for (nm, ex) in pend_defs:
            try:
                known[nm] = _sdk_eval_const(ex, known)
                progress = True
            except Exception:
                still.append((nm, ex))
        pend_defs = still
        if not still or not progress:
            break
    # enum 状態機械 (同 enum 内順序で暗黙 +1 を保持、全体 fixpoint)
    enum_states = [
        {"name": ename, "vals": {}, "rem": list(variants), "done": [], "curv": -1}
        for (ename, variants) in raw_enums
    ]
    for _round in range(16):
        progress = False
        for st in enum_states:
            while st["rem"]:
                vn, ex = st["rem"][0]
                if ex is None:
                    val = st["curv"] + 1
                else:
                    merged = dict(known)
                    merged.update(st["vals"])
                    try:
                        val = _sdk_eval_const(ex, merged)
                    except ValueError:
                        break
                st["vals"][vn] = val
                st["done"].append((vn, val))
                st["curv"] = val
                known.setdefault(vn, val)
                st["rem"].pop(0)
                progress = True
        if all(not st["rem"] for st in enum_states) or not progress:
            break
    unresolved = [(st["name"], len(st["rem"])) for st in enum_states if st["rem"]]
    # ---- Phase 3: 命名規則 strip して enums 完成形へ (未解決 enum は
    # 一次情報忠実のため全体除外 + 表示) ----
    for st in enum_states:
        if not st["rem"]:
            enums.setdefault(st["name"], []).extend(
                _sdk_variant_names(st["name"], st["done"]))
    if unresolved:
        print("sdk26 enum unresolved (excluded):", unresolved)

    for f in sorted(glob.glob(os.path.join(sdk_dir, "*.h"))):
        raw_text = open(f, encoding="utf-8").read()
        raw_defs.extend(_sdk_collect_defines(raw_text))
        src = _sdk_clean(raw_text)
        for m in re.finditer(
            r"typedef\s+NS_(?:ENUM|OPTIONS)\s*\(\s*([A-Za-z_]\w*)\s*,"
            r"\s*([A-Za-z_]\w*)\s*\)\s*\{(.*?)\}\s*\w*\s*;?",
            src, re.S):
            _basetype, ename, body = m.groups()
            items = []
            depth = 0
            cur = []
            for ch in body:
                if ch in "([":
                    depth += 1
                elif ch in ")]":
                    depth -= 1
                if ch == "," and depth == 0:
                    items.append("".join(cur))
                    cur = []
                else:
                    cur.append(ch)
            items.append("".join(cur))
            variants = []
            for it in items:
                mm = re.match(r"^\s*([A-Za-z_]\w*)\s*(?:=\s*(.*?))?\s*$", it, re.S)
                if not mm:
                    continue
                variants.append((mm.group(1), (mm.group(2) or "").strip() or None))
            if variants:
                raw_enums.append((ename, variants))

    for f in sorted(glob.glob(os.path.join(sdk_dir, "*.h"))):
        src = _sdk_clean(open(f, encoding="utf-8").read())
        # ---- @class 前方宣言 ----
        for m in re.finditer(r"@class\s+([A-Za-z_\w,\s]*);", src):
            for nm in m.group(1).split(","):
                nm = nm.strip()
                if nm:
                    classes.add(nm)
        # ---- @interface/@protocol (+ @protocol X; 前方宣言) ----
        for m in _HDR_RE.finditer(src):
            kind, name, cat, sup, protos = m.groups()
            # 直後が ';' なら @protocol X; 前方宣言 (本体なし)
            rest = src[m.end():].lstrip()
            classes.add(name)
            if rest.startswith(";"):
                continue
            if cat:
                continue
            if sup:
                parents.add((name, sup.strip()))
            if protos:
                for p in protos.split(","):
                    p = p.strip()
                    if p and p[0].isupper():
                        parents.add((name, p))
            end = src.find("@end", m.end())
            body = src[m.end(): end if end >= 0 else len(src)]
            for st in _split_statements(body):
                st = " ".join(st.split())
                if not st:
                    continue
                mm = _METH_RE.match(st)
                if mm:
                    rest2 = mm.group(2).strip()
                    parts = _PARAM_RE.findall(rest2)
                    if parts:
                        emit_sel(name, "".join(p + ":" for p in parts))
                    else:
                        b = re.match(r"^([A-Za-z_]\w*)\s*$", rest2)
                        if b:
                            emit_sel(name, b.group(1))
                    continue
                pm = _PROP_RE.match(st)
                if pm:
                    attrs, decl = pm.group(1), pm.group(2)
                    ids = re.findall(r"[A-Za-z_]\w*", decl)
                    if not ids:
                        continue
                    pname = ids[-1]
                    gm = re.search(r"getter\s*=\s*([A-Za-z_]\w*)", attrs)
                    getter = gm.group(1) if gm else pname
                    emit_sel(name, getter)
                    attr_set = {a.strip() for a in attrs.split(",")}
                    if "readonly" not in attr_set:
                        emit_sel(name, "set" + pname[0].upper() + pname[1:] + ":")
    return sels, sel_order, parents, classes, enums


def _sdk26_section_text(sels, sel_order, parents, classes, enums):
    out = []
    out.append("")
    out.append(SDK26_BEGIN)
    out.append("/// 正典表 — Apple SDK 26.5 (Xcode 26 世代 / Metal 4 対応) 一次情報")
    out.append("/// (alexey-lysiuk/macos-sdk MacOSX26.5.sdk、Metal.framework Headers の")
    out.append("/// 機械転記。tools/apple_canon_gen.py SDK26 parser 生成・手編集禁止)。")
    out.append("/// 監査機は classic 表との union で照合する (重複 SEL/enum は同一真値)。")
    out.append("#[rustfmt::skip]")
    out.append("pub static CANON_SDK26_SELS: &[CanonSel] = &[")
    sels_sorted = sorted(sel_order)
    for cls, sel in sels_sorted:
        r = sels[(cls, sel)]
        out.append(
            f'    CanonSel {{ class_name: "{cls}", selector: "{sel}", '
            f"retain: RetainRule::{r} }},"
        )
    out.append("];")
    out.append("")
    total_enum = sum(len(v) for v in enums.values())
    out.append(f"/// 正典 enum 値表 SDK26 ({total_enum} 件、生成値)。")
    out.append("#[rustfmt::skip]")
    out.append("pub static CANON_SDK26_ENUMS: &[CanonEnumVal] = &[")
    for en in sorted(enums):
        for (name, val) in enums[en]:
            # NSUInteger 系の巨大値 (NSUIntegerMax=2^64-1 等) は C 規格の
            # two's complement で i64 ビット再現 (signed 縮小は UB 回避の
            # 標準表現: 0xFFFF...FFFF → -1。canon 値はビット正確を維持)。
            v = val
            if v > 2**63 - 1:
                v = val - 2**64
            if v < -(2**63):
                v = val + 2**64
            out.append(
                f'    CanonEnumVal {{ enum_name: "{en}", variant: "{name}", value: {v} }},'
            )
    out.append("];")
    out.append("")
    out.append(f"/// 正典継承表 SDK26 ({len(parents)} 件、生成値)。")
    out.append("#[rustfmt::skip]")
    out.append("pub static CANON_SDK26_PARENTS: &[CanonParent] = &[")
    for (c_, p_) in sorted(parents):
        out.append(f'    CanonParent {{ child: "{c_}", parent: "{p_}" }},')
    out.append("];")
    out.append("")
    out.append(f"/// SDK26 定義 class/protocol 名一覧 ({len(classes)} 件、生成値)。")
    out.append("#[rustfmt::skip]")
    out.append("pub static CANON_SDK26_CLASSES: &[&str] = &[")
    for c_ in sorted(classes):
        out.append(f'    "{c_}",')
    out.append("];")
    out.append(SDK26_END)
    out.append("")
    return "\n".join(out)


def write_sdk26_section(out_path):
    """OUT の CANON_SDK26 marker 区画を冪等再生成。sdk26 不在時は既存区画を
    温存する (vendor 消失ガードと同思想 — 破壊的上書き禁止)。"""
    sdk = _sdk26_dir()
    cur = open(out_path, encoding="utf-8").read()
    if sdk is None:
        print("sdk26 headers not found: CANON_SDK26 section preserved/absent")
        return False
    sels, sel_order, parents, classes, enums = parse_sdk26(sdk)
    section = _sdk26_section_text(sels, sel_order, parents, classes, enums)
    if SDK26_BEGIN in cur and SDK26_END in cur:
        pre = cur[: cur.index(SDK26_BEGIN)].rstrip("\n")
        post = cur[cur.index(SDK26_END) + len(SDK26_END):]
        new = pre + "\n" + section + post
    else:
        new = cur.rstrip("\n") + "\n" + section
    with open(out_path, "w", encoding="utf-8") as w:
        w.write(new)
    n_sel = len(sel_order)
    n_enum = sum(len(v) for v in enums.values())
    print(
        f"sdk26 selectors: {n_sel}, enums: {n_enum}, "
        f"classes: {len(classes)}, parents: {len(parents)}  (from {sdk})"
    )
    return True


write_sdk26_section(OUT)
