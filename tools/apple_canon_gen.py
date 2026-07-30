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
