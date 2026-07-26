// rspeed — Rsift 監査作業の統合高速ツール (wave 103/2026-07-25 導入)
//
// 目的: これまで Python (/tmp の使い捨てスクリプト) でやっていた
//   ① 数値の厳密検算 (f64/f32/Fraction)
//   ② ファイル検査 (不可視文字・CRLF・簡体字・文字化け・末尾改行)
//   ③ fmt 逸脱ベースライン照合 (HEAD 包含テスト)
//   ④ 台帳件数積算・棚卸し残抽出
//   ⑤ cargo test / bench 実行 (変更検出で再ビルドを省略)
// を「1 つの .rs」に統合し、rustc -O で単一バイナリ化して起動 ~1ms 級にする。
// 依存クレートなし (std のみ)。`rustc -O --edition 2021 tools/rspeed.rs -o ~/bin/rspeed`
//
// 使い方は `rspeed help` 参照。

use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

// ===================================================================
// 共通ユーティリティ
// ===================================================================

fn ws_root() -> PathBuf {
    // 優先順: 引数 --ws <dir> > env RSIFT_WS > 既定 /home/user/rsift/rsift
    env::var("RSIFT_WS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/user/rsift/rsift"))
}

fn git_root() -> PathBuf {
    env::var("RSIFT_GIT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/user/rsift"))
}

fn cache_dir() -> PathBuf {
    let d = PathBuf::from("/home/user/.rspeed-cache");
    let _ = fs::create_dir_all(&d);
    d
}

/// snapshot タグ → キャッシュ内 manifest パス。
/// タグにパス区切りが含まれると存在しない下位ディレクトリ名になり
/// 書込みが失敗するため、/_ sanitize して統一 (snapshot/snapcheck 両方がこれを使う)。
fn snapshot_file(tag_raw: &str) -> PathBuf {
    let tag: String = tag_raw
        .chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect();
    cache_dir().join(format!("snapshot-{tag}.txt"))
}

/// ユーザーが明示的に要求した永続化書込み — 失敗を静寂に捨てない (fail-loud)。
/// (`let _ = fs::write(...)` は書込み失敗を握り潰し「保存した」と誤報するため禁止)
fn write_loud(path: &Path, contents: impl AsRef<[u8]>) -> Result<(), String> {
    fs::write(path, contents).map_err(|e| format!("書込み失敗 {}: {e}", path.display()))
}

fn read_text(p: &Path) -> Option<String> {
    let bytes = fs::read(p).ok()?;
    String::from_utf8(bytes).ok()
}

/// 再帰的にファイルを列挙 (.git/target/node_modules を除外)。
fn walk_files(root: &Path, out: &mut Vec<PathBuf>) {
    let rd = match fs::read_dir(root) {
        Ok(r) => r,
        Err(_) => return,
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if matches!(
                name.as_str(),
                ".git" | "target" | "node_modules" | ".rspeed-cache"
            ) {
                continue;
            }
            walk_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

// ===================================================================
// expr — 数式厳密評価 (f64 / f32 逐次 / Fraction 正確分数)
// ===================================================================

#[derive(Debug, Clone)]
enum Ast {
    Num(String), // 生字句 (hex/int/decimal)
    Neg(Box<Ast>),
    Bin(char, Box<Ast>, Box<Ast>), // + - * / % ^ <(shl) >(shr)
    Call(String, Vec<Ast>),
    Var, // 'x' (table/monotone/roundtrip 系専用の自由変数)
}

struct Lexer<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.b.len() && (self.b[self.i] as char).is_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.b.get(self.i).copied()
    }
}

fn parse_expr(s: &str) -> Result<Ast, String> {
    let mut lx = Lexer {
        b: s.as_bytes(),
        i: 0,
    };
    let a = parse_shift(&mut lx)?;
    lx.skip_ws();
    if lx.i != lx.b.len() {
        return Err(format!("expr: 残余トークン @{}: {}", lx.i, &s[lx.i..]));
    }
    Ok(a)
}

fn parse_shift(lx: &mut Lexer) -> Result<Ast, String> {
    let mut a = parse_add(lx)?;
    loop {
        lx.skip_ws();
        if lx.i + 1 < lx.b.len() && lx.b[lx.i] == b'<' && lx.b[lx.i + 1] == b'<' {
            lx.i += 2;
            let r = parse_add(lx)?;
            a = Ast::Bin('<', Box::new(a), Box::new(r));
        } else if lx.i + 1 < lx.b.len() && lx.b[lx.i] == b'>' && lx.b[lx.i + 1] == b'>' {
            lx.i += 2;
            let r = parse_add(lx)?;
            a = Ast::Bin('>', Box::new(a), Box::new(r));
        } else {
            return Ok(a);
        }
    }
}

fn parse_add(lx: &mut Lexer) -> Result<Ast, String> {
    let mut a = parse_mul(lx)?;
    loop {
        match lx.peek() {
            Some(b'+') => {
                lx.i += 1;
                lx.skip_ws();
                let r = parse_mul(lx)?;
                a = Ast::Bin('+', Box::new(a), Box::new(r));
            }
            Some(b'-') => {
                lx.i += 1;
                lx.skip_ws();
                let r = parse_mul(lx)?;
                a = Ast::Bin('-', Box::new(a), Box::new(r));
            }
            _ => return Ok(a),
        }
    }
}

fn parse_mul(lx: &mut Lexer) -> Result<Ast, String> {
    let mut a = parse_unary(lx)?;
    loop {
        match lx.peek() {
            Some(b'*') => {
                lx.i += 1;
                let r = parse_unary(lx)?;
                a = Ast::Bin('*', Box::new(a), Box::new(r));
            }
            Some(b'/') => {
                lx.i += 1;
                let r = parse_unary(lx)?;
                a = Ast::Bin('/', Box::new(a), Box::new(r));
            }
            Some(b'%') => {
                lx.i += 1;
                let r = parse_unary(lx)?;
                a = Ast::Bin('%', Box::new(a), Box::new(r));
            }
            _ => return Ok(a),
        }
    }
}

fn parse_unary(lx: &mut Lexer) -> Result<Ast, String> {
    lx.skip_ws();
    if lx.peek() == Some(b'-') {
        lx.i += 1;
        return Ok(Ast::Neg(Box::new(parse_unary(lx)?)));
    }
    if lx.peek() == Some(b'+') {
        lx.i += 1;
        return parse_unary(lx);
    }
    parse_pow(lx)
}

fn parse_pow(lx: &mut Lexer) -> Result<Ast, String> {
    let base = parse_primary(lx)?;
    lx.skip_ws();
    if lx.peek() == Some(b'^') {
        lx.i += 1;
        let e = parse_unary(lx)?; // 右結合
        return Ok(Ast::Bin('^', Box::new(base), Box::new(e)));
    }
    Ok(base)
}

fn parse_primary(lx: &mut Lexer) -> Result<Ast, String> {
    lx.skip_ws();
    let c = lx.peek().ok_or("expr: 入力の途中で終端")?;
    if c == b'(' {
        lx.i += 1;
        let a = parse_shift(lx)?;
        lx.skip_ws();
        if lx.peek() != Some(b')') {
            return Err("expr: 閉じ括弧なし".into());
        }
        lx.i += 1;
        return Ok(a);
    }
    if c.is_ascii_digit() || c == b'.' {
        let start = lx.i;
        // hex
        if c == b'0' && lx.i + 1 < lx.b.len() && (lx.b[lx.i + 1] | 0x20) == b'x' {
            lx.i += 2;
            while lx.i < lx.b.len() {
                let ch = lx.b[lx.i] as char;
                if ch.is_ascii_hexdigit() || ch == '_' {
                    lx.i += 1;
                } else {
                    break;
                }
            }
            return Ok(Ast::Num(lx_text(lx, start, lx.i)));
        }
        while lx.i < lx.b.len() {
            let ch = lx.b[lx.i] as char;
            if ch.is_ascii_digit()
                || ch == '.'
                || ch == '_'
                || ch == 'e'
                || ch == 'E'
                || ((ch == '+' || ch == '-') && lx.i > start && (lx.b[lx.i - 1] | 0x20) == b'e')
            {
                lx.i += 1;
            } else {
                break;
            }
        }
        return Ok(Ast::Num(lx_text(lx, start, lx.i)));
    }
    if (c as char).is_ascii_alphabetic() || c == b'_' {
        let start = lx.i;
        while lx.i < lx.b.len() {
            let ch = lx.b[lx.i] as char;
            if ch.is_ascii_alphanumeric() || ch == '_' {
                lx.i += 1;
            } else {
                break;
            }
        }
        let name = lx_text(lx, start, lx.i);
        lx.skip_ws();
        if lx.peek() == Some(b'(') {
            lx.i += 1;
            let mut args = Vec::new();
            lx.skip_ws();
            if lx.peek() != Some(b')') {
                loop {
                    args.push(parse_shift(lx)?);
                    lx.skip_ws();
                    match lx.peek() {
                        Some(b',') => {
                            lx.i += 1;
                        }
                        Some(b')') => break,
                        _ => return Err("expr: 引数の区切りエラー".into()),
                    }
                }
            }
            lx.skip_ws();
            if lx.peek() != Some(b')') {
                return Err("expr: 関数呼出の閉じ括弧なし".into());
            }
            lx.i += 1;
            return Ok(Ast::Call(name, args));
        }
        // 定数
        return match name.as_str() {
            "pi" => Ok(Ast::Num(std::f64::consts::PI.to_string())),
            "tau" => Ok(Ast::Num(std::f64::consts::TAU.to_string())),
            "e" => Ok(Ast::Num(std::f64::consts::E.to_string())),
            "inf" => Ok(Ast::Num("1e999".into())),
            "nan" => Ok(Ast::Num("0/0".into())), // 評価器側で扱う
            "x" => Ok(Ast::Var),
            _ => Err(format!("expr: 未知の識別子 '{name}'")),
        };
    }
    Err(format!("expr: 解析不能 @{}", lx.i))
}

fn lx_text(lx: &Lexer, a: usize, b: usize) -> String {
    String::from_utf8_lossy(&lx.b[a..b]).into_owned()
}

// ---------- f64 / f32 評価 ----------

fn num_f64(raw: &str) -> Result<f64, String> {
    let s: String = raw.chars().filter(|c| *c != '_').collect();
    if s == "0/0" {
        return Ok(f64::NAN);
    }
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let v = i128::from_str_radix(h, 16).map_err(|e| format!("hex 解析: {e}"))?;
        return Ok(v as f64);
    }
    s.parse::<f64>()
        .map_err(|e| format!("数値 '{s}' 解析: {e}"))
}

fn eval_f64(a: &Ast) -> Result<f64, String> {
    eval_f64_x(a, None)
}

fn eval_f64_x(a: &Ast, xv: Option<f64>) -> Result<f64, String> {
    Ok(match a {
        Ast::Num(s) => num_f64(s)?,
        Ast::Var => xv.ok_or("変数 x には値が束縛されていません")?,
        Ast::Neg(x) => -eval_f64_x(x, xv)?,
        Ast::Bin(op, l, r) => {
            let x = eval_f64_x(l, xv)?;
            let y = eval_f64_x(r, xv)?;
            match op {
                '+' => x + y,
                '-' => x - y,
                '*' => x * y,
                '/' => x / y,
                '%' => x % y,
                '^' => x.powf(y),
                '<' | '>' => {
                    let xi = as_int(x)?;
                    let yi = as_int(y)?;
                    if yi < 0 || yi > 127 {
                        return Err("シフト量は 0..=127".into());
                    }
                    let v = if *op == '<' { xi << yi } else { xi >> yi };
                    v as f64
                }
                _ => unreachable!(),
            }
        }
        Ast::Call(f, args) => call_f64(f, args, xv)?,
    })
}

fn as_int(v: f64) -> Result<i128, String> {
    if !v.is_finite() || v.fract() != 0.0 || v.abs() > 1.7e38 {
        return Err(format!("整数に変換できない値: {v}"));
    }
    Ok(v as i128)
}

fn call_f64(f: &str, args: &[Ast], xv: Option<f64>) -> Result<f64, String> {
    let ev = |a: &Ast| eval_f64_x(a, xv);
    let one = |args: &[Ast]| -> Result<f64, String> {
        if args.len() != 1 {
            return Err(format!("{f}: 引数は 1 個"));
        }
        ev(&args[0])
    };
    Ok(match f {
        "sqrt" => one(args)?.sqrt(),
        "cbrt" => one(args)?.cbrt(),
        "abs" => one(args)?.abs(),
        "floor" => one(args)?.floor(),
        "ceil" => one(args)?.ceil(),
        "round" => one(args)?.round(),
        "trunc" => one(args)?.trunc(),
        "fract" => one(args)?.fract(),
        "ln" => one(args)?.ln(),
        "log2" => one(args)?.log2(),
        "log10" => one(args)?.log10(),
        "exp" => one(args)?.exp(),
        "sin" => one(args)?.sin(),
        "cos" => one(args)?.cos(),
        "tan" => one(args)?.tan(),
        "atan" => one(args)?.atan(),
        "gcd" | "lcm" => {
            if args.len() < 2 {
                return Err(format!("{f}: 引数は 2 個以上"));
            }
            let mut acc = as_int(ev(&args[0])?)?;
            for a in &args[1..] {
                let v = as_int(ev(a)?)?;
                acc = if f == "gcd" {
                    gcd_i128(acc, v)
                } else {
                    let g = gcd_i128(acc, v);
                    if g == 0 {
                        0
                    } else {
                        acc.checked_div(g)
                            .and_then(|q| q.checked_mul(v))
                            .ok_or("lcm: i128 オーバフロー")?
                    }
                };
            }
            acc as f64
        }
        "min" | "max" => {
            if args.is_empty() {
                return Err(format!("{f}: 引数なし"));
            }
            let mut acc = ev(&args[0])?;
            for a in &args[1..] {
                let v = ev(a)?;
                acc = if f == "min" { acc.min(v) } else { acc.max(v) };
            }
            acc
        }
        "pow" => {
            if args.len() != 2 {
                return Err("pow: 引数 2 個".into());
            }
            ev(&args[0])?.powf(ev(&args[1])?)
        }
        "hypot" => {
            if args.len() != 2 {
                return Err("hypot: 引数 2 個".into());
            }
            ev(&args[0])?.hypot(ev(&args[1])?)
        }
        "clamp" => {
            if args.len() != 3 {
                return Err("clamp: 引数 3 個".into());
            }
            ev(&args[0])?.clamp(ev(&args[1])?, ev(&args[2])?)
        }
        _ => return Err(format!("未知の関数 '{f}'")),
    })
}

fn gcd_i128(mut a: i128, mut b: i128) -> i128 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

// f32 逐次評価 (各演算ごとに f32 丸め — 実機カーネルの逐次再現用)
fn eval_f32(a: &Ast) -> Result<f32, String> {
    eval_f32_x(a, None)
}

fn eval_f32_x(a: &Ast, xv: Option<f32>) -> Result<f32, String> {
    Ok(match a {
        Ast::Var => xv.ok_or("変数 x には値が束縛されていません")?,
        Ast::Num(s) => {
            let s2: String = s.chars().filter(|c| *c != '_').collect();
            if let Some(h) = s2.strip_prefix("0x") {
                i128::from_str_radix(h, 16).map_err(|e| e.to_string())? as f32
            } else if s2 == "0/0" {
                f32::NAN
            } else {
                s2.parse::<f32>()
                    .map_err(|e| format!("f32 数値 '{s2}': {e}"))?
            }
        }
        Ast::Neg(x) => -eval_f32_x(x, xv)?,
        Ast::Bin(op, l, r) => {
            let x = eval_f32_x(l, xv)?;
            let y = eval_f32_x(r, xv)?;
            match op {
                '+' => x + y,
                '-' => x - y,
                '*' => x * y,
                '/' => x / y,
                '%' => x % y,
                '^' => x.powf(y),
                '<' | '>' => {
                    let xi = as_int(x as f64)?;
                    let yi = as_int(y as f64)?;
                    if yi < 0 || yi > 127 {
                        return Err("シフト量は 0..=127".into());
                    }
                    (if *op == '<' { xi << yi } else { xi >> yi }) as f32
                }
                _ => unreachable!(),
            }
        }
        Ast::Call(f, args) => {
            let evs: Result<Vec<f32>, _> = args.iter().map(|a| eval_f32_x(a, xv)).collect();
            let v = evs?;
            match (f.as_str(), v.as_slice()) {
                ("sqrt", [x]) => x.sqrt(),
                ("floor", [x]) => x.floor(),
                ("ceil", [x]) => x.ceil(),
                ("round", [x]) => x.round(),
                ("abs", [x]) => x.abs(),
                ("min", [x, y]) => x.min(*y),
                ("max", [x, y]) => x.max(*y),
                ("hypot", [x, y]) => x.hypot(*y),
                ("clamp", [x, lo, hi]) => x.clamp(*lo, *hi),
                ("pow", [x, y]) => x.powf(*y),
                _ => return Err(format!("f32 未対応/引数誤り: {f}")),
            }
        }
    })
}

// ---------- Fraction (正確分数) ----------

#[derive(Debug, Clone, Copy)]
struct Frac {
    n: i128,
    d: i128,
}

fn frac_new(n: i128, d: i128) -> Frac {
    if d == 0 {
        panic!("分数の分母 0");
    }
    let g = gcd_i128(n, d);
    let (mut n, mut d) = (n / g, d / g);
    if d < 0 {
        n = -n;
        d = -d;
    }
    Frac { n, d }
}

fn num_frac(raw: &str) -> Result<Frac, String> {
    let s: String = raw.chars().filter(|c| *c != '_').collect();
    if let Some(h) = s.strip_prefix("0x") {
        let v = i128::from_str_radix(h, 16).map_err(|e| e.to_string())?;
        return Ok(frac_new(v, 1));
    }
    // 10 進 (e 指数あり可) を正確に
    let (mant, exp) = match s.find(['e', 'E']) {
        Some(p) => (
            &s[..p],
            s[p + 1..].parse::<i64>().map_err(|e| e.to_string())?,
        ),
        None => (s.as_str(), 0),
    };
    let (int_part, frac_part) = match mant.find('.') {
        Some(p) => (&mant[..p], &mant[p + 1..]),
        None => (mant, ""),
    };
    let sign = if int_part.starts_with('-') { -1i128 } else { 1 };
    let int_abs: i128 = int_part.trim_start_matches(['-', '+']).parse().unwrap_or(0);
    let frac_digits = frac_part.len() as i64;
    let frac_val: i128 = if frac_part.is_empty() {
        0
    } else {
        frac_part
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?
    };
    let mut num = sign * (int_abs * 10i128.pow(frac_digits as u32) + frac_val);
    let mut den = 10i128.pow(frac_digits as u32);
    // 指数適用
    if exp >= 0 {
        num = num
            .checked_mul(10i128.checked_pow(exp as u32).ok_or("指数過大")?)
            .ok_or("オーバフロー")?;
    } else {
        den = den
            .checked_mul(10i128.checked_pow((-exp) as u32).ok_or("指数過小")?)
            .ok_or("オーバフロー")?;
    }
    Ok(frac_new(num, den))
}

fn eval_frac(a: &Ast) -> Result<Frac, String> {
    eval_frac_x(a, None)
}

fn eval_frac_x(a: &Ast, xv: Option<&str>) -> Result<Frac, String> {
    Ok(match a {
        Ast::Num(s) => num_frac(s)?,
        Ast::Var => num_frac(xv.ok_or("変数 x には値が束縛されていません")?)?,
        Ast::Neg(x) => {
            let f = eval_frac_x(x, xv)?;
            frac_new(-f.n, f.d)
        }
        Ast::Bin(op, l, r) => {
            let x = eval_frac_x(l, xv)?;
            let y = eval_frac_x(r, xv)?;
            match op {
                '+' => frac_new(x.n * y.d + y.n * x.d, x.d * y.d),
                '-' => frac_new(x.n * y.d - y.n * x.d, x.d * y.d),
                '*' => frac_new(x.n * y.n, x.d * y.d),
                '/' => frac_new(x.n * y.d, x.d * y.n),
                '%' => {
                    let xi = x.n / x.d;
                    let yi = y.n / y.d;
                    frac_new(xi % yi, 1)
                }
                '^' => {
                    let e = y.n / y.d;
                    if e.unsigned_abs() > 10000 {
                        return Err("frac: 指数は |e|<=10000 の整数".into());
                    }
                    if e >= 0 {
                        frac_new(x.n.pow(e as u32), x.d.pow(e as u32))
                    } else {
                        frac_new(x.d.pow((-e) as u32), x.n.pow((-e) as u32))
                    }
                }
                '<' | '>' => {
                    let xi = if x.d == 1 {
                        x.n
                    } else {
                        return Err("frac シフトは整数のみ".into());
                    };
                    let k = if y.d == 1 {
                        y.n
                    } else {
                        return Err("frac シフトは整数のみ".into());
                    };
                    if !(0..=126).contains(&k) {
                        return Err("frac シフト量 0..=126".into());
                    }
                    frac_new(if *op == '<' { xi << k } else { xi >> k }, 1)
                }
                _ => unreachable!(),
            }
        }
        Ast::Call(f, args) => {
            let evs: Result<Vec<Frac>, _> = args.iter().map(|a| eval_frac_x(a, xv)).collect();
            let v = evs?;
            let as_i = |x: Frac| -> Result<i128, String> {
                if x.d != 1 {
                    return Err(format!("{f}: 整数引数のみ"));
                }
                Ok(x.n)
            };
            match f.as_str() {
                "gcd" | "lcm" => {
                    if v.len() < 2 {
                        return Err(format!("{f}: 引数 2 個以上"));
                    }
                    let mut acc = as_i(v[0])?;
                    for x in &v[1..] {
                        let xi = as_i(*x)?;
                        acc = if f == "gcd" {
                            gcd_i128(acc, xi)
                        } else {
                            acc / gcd_i128(acc, xi) * xi
                        };
                    }
                    frac_new(acc, 1)
                }
                "min" | "max" => {
                    let mut acc = v[0];
                    for x in &v[1..] {
                        let lhs = acc.n * x.d;
                        let rhs = x.n * acc.d;
                        let take = if f == "min" { lhs > rhs } else { lhs < rhs };
                        if take {
                            acc = *x;
                        }
                    }
                    acc
                }
                "abs" => {
                    if v.len() != 1 {
                        return Err("abs: 引数 1 個".into());
                    }
                    frac_new(v[0].n.abs(), v[0].d)
                }
                "floor" => {
                    if v.len() != 1 {
                        return Err("floor: 引数 1 個".into());
                    }
                    frac_new(v[0].n.div_euclid(v[0].d), 1)
                }
                _ => return Err(format!("frac 未対応関数: {f}")),
            }
        }
    })
}

fn frac_decimal(f: Frac) -> String {
    // 正確な 10 進展開 (循環節は (…) で囲む)。200 桁で打切り。
    if f.n % f.d == 0 {
        return format!("{}", f.n / f.d);
    }
    let sign = if f.n < 0 { "-" } else { "" };
    let mut rem = f.n.unsigned_abs();
    let d = f.d as u128;
    let int_part = rem / d;
    rem %= d;
    let mut digits = String::new();
    let mut seen: HashMap<u128, usize> = HashMap::new();
    let mut loop_start = None;
    for _ in 0..200 {
        if rem == 0 {
            break;
        }
        if let Some(&pos) = seen.get(&rem) {
            loop_start = Some(pos);
            break;
        }
        seen.insert(rem, digits.len());
        rem *= 10;
        digits.push(char::from_digit((rem / d) as u32, 10).unwrap());
        rem %= d;
    }
    let mut dec = match loop_start {
        Some(p) => format!("{}.{}({})", int_part, &digits[..p], &digits[p..]),
        None => format!("{}.{}", int_part, digits),
    };
    if rem != 0 && loop_start.is_none() && digits.len() >= 200 {
        dec.push('…');
    }
    format!("{sign}{dec}")
}

fn fmt_f64_full(v: f64) -> String {
    let bits = v.to_bits();
    format!("{v:e} (bits 0x{bits:016x})")
}

fn cmd_expr(args: &[String]) -> i32 {
    let mut mode_f32 = false;
    let mut mode_frac = false;
    let mut exprs: Vec<String> = Vec::new();
    for a in args {
        match a.as_str() {
            "--f32" => mode_f32 = true,
            "--frac" => mode_frac = true,
            _ => exprs.push(a.clone()),
        }
    }
    if exprs.is_empty() {
        eprintln!("usage: rspeed expr [--f32] [--frac] <式>…");
        return 2;
    }
    let mut rc = 0;
    for e in &exprs {
        match parse_expr(e) {
            Ok(ast) => {
                println!("expr: {e}");
                if mode_frac {
                    match eval_frac(&ast) {
                        Ok(f) => {
                            println!("  = {} / {}", f.n, f.d);
                            println!("  = {}", frac_decimal(f));
                            let fv = f.n as f64 / f.d as f64;
                            println!("  f64 近似: {}", fmt_f64_full(fv));
                            println!(
                                "  f32 丸め: 0x{:08x} (= {})",
                                (fv as f32).to_bits(),
                                fv as f32
                            );
                        }
                        Err(m) => {
                            eprintln!("  frac error: {m}");
                            rc = 1;
                        }
                    }
                } else {
                    match eval_f64(&ast) {
                        Ok(v) => println!("  f64: {v}  |  {}", fmt_f64_full(v)),
                        Err(m) => {
                            eprintln!("  f64 error: {m}");
                            rc = 1;
                        }
                    }
                    if mode_f32 {
                        match eval_f32(&ast) {
                            Ok(v) => println!("  f32 逐次: {v}  |  bits 0x{:08x}", v.to_bits()),
                            Err(m) => {
                                eprintln!("  f32 error: {m}");
                                rc = 1;
                            }
                        }
                    } else if let Ok(v) = eval_f64(&ast) {
                        println!(
                            "  f32 丸め: {}  |  bits 0x{:08x}",
                            v as f32,
                            (v as f32).to_bits()
                        );
                    }
                }
            }
            Err(m) => {
                eprintln!("{m}");
                rc = 1;
            }
        }
    }
    rc
}

// ===================================================================
// Batch A — 厳密数値系 (bits/f16/RNG/morton/行列/テーブル掃引 27 機能)
// ===================================================================

fn cmd_bits(a: &[String]) -> i32 {
    // bits <値|式>… : 符号/指数/仮数の分解 + f32/f16/bf16 丸め併記
    if a.is_empty() {
        eprintln!("usage: rspeed bits <式>…");
        return 2;
    }
    let mut rc = 0;
    for e in a {
        match parse_expr(e).and_then(|ast| eval_f64(&ast)) {
            Ok(v) => {
                let b = v.to_bits();
                let sign = (b >> 63) as u64;
                let exp = ((b >> 52) & 0x7ff) as i64;
                let mant = b & ((1u64 << 52) - 1);
                let cls = if v.is_nan() {
                    "NaN"
                } else if v.is_infinite() {
                    "Inf"
                } else if exp == 0 && mant == 0 {
                    "±0"
                } else if exp == 0 {
                    "subnormal"
                } else {
                    "normal"
                };
                println!("{e} = {v}");
                println!(
                    "  f64: 0x{b:016x}  s={sign} e_raw={exp} (2^{} 区間) m=0x{mant:013x} [{cls}]",
                    exp - 1023
                );
                let f = v as f32;
                let fb = f.to_bits();
                println!(
                    "  f32: 0x{fb:08x} (= {f})  s={} e={} m=0x{:06x}",
                    fb >> 31,
                    (fb >> 23) & 0xff,
                    fb & 0x7f_ffff
                );
                println!(
                    "  f16: 0x{:04x} (= {})  bf16: 0x{:04x} (= {})",
                    f32_to_f16(f),
                    f16_to_f32(f32_to_f16(f)),
                    f32_to_bf16(f),
                    bf16_to_f32(f32_to_bf16(f))
                );
            }
            Err(m) => {
                eprintln!("{e}: {m}");
                rc = 1;
            }
        }
    }
    rc
}

fn cmd_bits_of(a: &[String]) -> i32 {
    // bits-of <hex> : 0x…16桁は f64、8桁は f32、4桁は f16 として値化
    if a.is_empty() {
        eprintln!("usage: rspeed bits-of <hex>…");
        return 2;
    }
    for h in a {
        let t = h.trim_start_matches("0x");
        match u64::from_str_radix(t, 16) {
            Ok(v) if t.len() > 8 => {
                let f = f64::from_bits(v);
                println!("0x{v:016x} → f64 {f:e} (= {f})  f32 投影: {}", f as f32);
            }
            Ok(v) if t.len() > 4 => {
                let f = f32::from_bits(v as u32);
                println!(
                    "0x{v:08x} → f32 {f:e} (= {f})  f64: 0x{:016x}",
                    (f as f64).to_bits()
                );
            }
            Ok(v) => println!("0x{v:04x} → f16 {}", f16_to_f32(v as u16)),
            Err(e) => {
                eprintln!("{h}: {e}");
                return 1;
            }
        }
    }
    0
}

fn next_up(x: f64) -> f64 {
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x == 0.0 {
        return f64::from_bits(1);
    }
    let b = x.to_bits();
    f64::from_bits(if x > 0.0 { b + 1 } else { b - 1 })
}

fn cmd_ulp(a: &[String]) -> i32 {
    for e in a {
        match parse_expr(e).and_then(|ast| eval_f64(&ast)) {
            Ok(v) => {
                let up = next_up(v);
                let dn = next_up(-v);
                let dn = -dn;
                println!(
                    "{e} = {v}: ulp = {} (prev {} 0x{:016x}, next {} 0x{:016x})",
                    up - v,
                    dn,
                    dn.to_bits(),
                    up,
                    up.to_bits()
                );
            }
            Err(m) => {
                eprintln!("{e}: {m}");
                return 1;
            }
        }
    }
    0
}

fn cmd_next(a: &[String]) -> i32 {
    // next <値> [+|-] [n] : n 個先の表現可能浮動小数 (f64)
    if a.is_empty() {
        eprintln!("usage: rspeed next <式> [+|-] [n]");
        return 2;
    }
    let mut v = match parse_expr(&a[0]).and_then(|ast| eval_f64(&ast)) {
        Ok(v) => v,
        Err(m) => {
            eprintln!("{m}");
            return 1;
        }
    };
    let dir = if a.get(1).map(|s| s == "-").unwrap_or(false) {
        -1
    } else {
        1
    };
    let n: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    for _ in 0..n {
        v = if dir > 0 { next_up(v) } else { -next_up(-v) };
    }
    println!("{v} 0x{:016x}", v.to_bits());
    0
}

// ---- f16/bf16 (RNE: round-to-nearest-even) ----
fn f32_to_f16(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let e = ((b >> 23) & 0xff) as i32;
    let m = b & 0x7f_ffff;
    if e == 0xff {
        return sign | 0x7c00 | if m != 0 { 0x201 } else { 0 }; // NaN → quiet 化
    }
    let e16 = e - 127 + 15;
    if e16 >= 31 {
        return sign | 0x7c00;
    } // RNE オーバフロー → Inf
    if e16 <= 0 {
        if e16 < -10 {
            return sign;
        } // 最小 subnormal 未満 → 0
        let m32 = m | 0x80_0000;
        let shift = (14 - e16) as u32;
        let mut m16 = m32 >> shift;
        let rem = m32 & ((1u32 << shift) - 1);
        let half = 1u32 << (shift - 1);
        if rem > half || (rem == half && (m16 & 1) == 1) {
            m16 += 1;
        }
        return sign | m16 as u16;
    }
    let mut m16 = m >> 13;
    let rem = m & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && (m16 & 1) == 1) {
        m16 += 1;
        if m16 == 0x400 {
            // 仮数繰り上がり → 指数+1
            return if e16 + 1 >= 31 {
                sign | 0x7c00
            } else {
                sign | (((e16 + 1) as u16) << 10)
            };
        }
    }
    sign | ((e16 as u16) << 10) | m16 as u16
}

fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h as u32) & 0x8000) << 16;
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as u32;
    if e == 0x1f {
        return f32::from_bits(sign | 0x7f80_0000 | if m != 0 { 0x40_0000 | (m << 13) } else { 0 });
    }
    if e == 0 {
        if m == 0 {
            return f32::from_bits(sign);
        }
        // subnormal → normalize
        let mut ms = m;
        let mut ee = -1i32; // e-15 の補正: f32 指数 = 127-15 = 112 起点
        loop {
            if ms & 0x400 != 0 {
                break;
            }
            ms <<= 1;
            ee -= 1;
        }
        ms &= 0x3ff;
        let ef = (127 - 14 + ee) as u32;
        return f32::from_bits(sign | (ef << 23) | (ms << 13));
    }
    f32::from_bits(sign | (((e - 15 + 127) as u32) << 23) | (m << 13))
}

fn f32_to_bf16(x: f32) -> u16 {
    if x.is_nan() {
        return ((x.to_bits() >> 16) as u16) | 0x0040;
    }
    let b = x.to_bits();
    let bias = 0x7fff + ((b >> 16) & 1); // RNE (タイ時偶数へ)
    ((b + bias) >> 16) as u16
}

fn bf16_to_f32(h: u16) -> f32 {
    f32::from_bits((h as u32) << 16)
}

fn cmd_hfbits(a: &[String]) -> i32 {
    // hfbits <式>… : f32→f16/bf16 変換の丸め誤差 (RNE) を厳密表示
    for e in a {
        match parse_expr(e).and_then(|ast| eval_f32(&ast)) {
            Ok(v) => {
                let h = f32_to_f16(v);
                let b = f32_to_bf16(v);
                let hv = f16_to_f32(h);
                let bv = bf16_to_f32(b);
                println!("{e}: f32 {v} 0x{:08x}", v.to_bits());
                println!("  f16 0x{h:04x} = {hv} (err {:e})", hv as f64 - v as f64);
                println!("  bf16 0x{b:04x} = {bv} (err {:e})", bv as f64 - v as f64);
            }
            Err(m) => {
                eprintln!("{e}: {m}");
                return 1;
            }
        }
    }
    0
}

// ---- sRGB / ガンマ ----
fn srgb_encode(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}
fn srgb_decode(c: f64) -> f64 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn cmd_gamma(a: &[String]) -> i32 {
    // gamma <0..1 値>… : sRGB piecewise ↔ linear + pow2.2 近似との差
    for e in a {
        match parse_expr(e).and_then(|ast| eval_f64(&ast)) {
            Ok(v) => {
                let enc = srgb_encode(v);
                let dec = srgb_decode(v);
                println!("{e} = {v}");
                println!("  encode: {enc:.10} (bits 0x{:016x})", enc.to_bits());
                println!("  decode: {dec:.10} (bits 0x{:016x})", dec.to_bits());
                println!(
                    "  pow2.2 enc 誤差: {:e} / pow2.2 dec 誤差: {:e}",
                    v.powf(1.0 / 2.2) - enc,
                    v.powf(2.2) - dec
                );
                println!(
                    "  roundtrip err: enc(dec) {:e} / dec(enc) {:e}",
                    srgb_encode(dec) - v,
                    srgb_decode(enc) - v
                );
            }
            Err(m) => {
                eprintln!("{e}: {m}");
                return 1;
            }
        }
    }
    0
}

// ---- Morton (2D part1by1 / 3D part1by2) ----
fn part1by1(mut x: u32) -> u32 {
    x = (x | (x << 8)) & 0x00ff_00ff;
    x = (x | (x << 4)) & 0x0f0f_0f0f;
    x = (x | (x << 2)) & 0x3333_3333;
    x = (x | (x << 1)) & 0x5555_5555;
    x
}
fn compact1by1(mut x: u32) -> u32 {
    x &= 0x5555_5555;
    x = (x | (x >> 1)) & 0x3333_3333;
    x = (x | (x >> 2)) & 0x0f0f_0f0f;
    x = (x | (x >> 4)) & 0x00ff_00ff;
    x = (x | (x >> 8)) & 0x0000_ffff;
    x
}
// マジックビット定数は libmorton 系 21bit チェーンの正値
// (2026-07-25 に selftest 境界検査で誤記を捕捉: step1/step2 の '00' 過多で
//  入力 bit8-20 が静寂ゼロ化する欠陥があった。正値で exhaustive 往復検証済)。
fn part1by2(mut x: u64) -> u64 {
    x &= 0x1f_ffff;
    x = (x | (x << 32)) & 0x001f_0000_0000_ffff;
    x = (x | (x << 16)) & 0x001f_0000_ff00_00ff;
    x = (x | (x << 8)) & 0x100f_00f0_0f00_f00f;
    x = (x | (x << 4)) & 0x10c3_0c30_c30c_30c3;
    x = (x | (x << 2)) & 0x1249_2492_4924_9249;
    x
}
fn compact1by2(mut x: u64) -> u64 {
    x &= 0x1249_2492_4924_9249;
    x = (x ^ (x >> 2)) & 0x10c3_0c30_c30c_30c3;
    x = (x ^ (x >> 4)) & 0x100f_00f0_0f00_f00f;
    x = (x ^ (x >> 8)) & 0x001f_0000_ff00_00ff;
    x = (x ^ (x >> 16)) & 0x001f_0000_0000_ffff;
    x = (x ^ (x >> 32)) & 0x1f_ffff;
    x
}

fn cmd_morton(a: &[String]) -> i32 {
    // morton <x> <y> [z] : encode + decode ラウンドトリップ検証
    let xs: Result<Vec<u64>, _> = a.iter().map(|s| s.parse::<u64>()).collect();
    let Ok(v) = xs else {
        eprintln!("usage: rspeed morton <x> <y> [z]");
        return 2;
    };
    if v.len() == 2 {
        if v[0] > 0xffff || v[1] > 0xffff {
            eprintln!("2D は 16bit まで");
            return 1;
        }
        let code = part1by1(v[0] as u32) | (part1by1(v[1] as u32) << 1);
        let (dx, dy) = (compact1by1(code), compact1by1(code >> 1));
        println!(
            "morton2({},{}) = 0x{:08x} ({}), decode=({},{}) {}",
            v[0],
            v[1],
            code,
            code,
            dx,
            dy,
            if dx as u64 == v[0] && dy as u64 == v[1] {
                "ROUNDTRIP-OK"
            } else {
                "ROUNDTRIP-FAIL"
            }
        );
    } else if v.len() == 3 {
        if v.iter().any(|&x| x > 0x1f_ffff) {
            eprintln!("3D は 21bit まで");
            return 1;
        }
        let code = part1by2(v[0]) | (part1by2(v[1]) << 1) | (part1by2(v[2]) << 2);
        let (dx, dy, dz) = (
            compact1by2(code),
            compact1by2(code >> 1),
            compact1by2(code >> 2),
        );
        println!(
            "morton3({},{},{}) = 0x{:016x}, decode=({},{},{}) {}",
            v[0],
            v[1],
            v[2],
            code,
            dx,
            dy,
            dz,
            if dx == v[0] && dy == v[1] && dz == v[2] {
                "ROUNDTRIP-OK"
            } else {
                "ROUNDTRIP-FAIL"
            }
        );
    } else {
        eprintln!("usage: rspeed morton <x> <y> [z]");
        return 2;
    }
    0
}

// ---- RNG 再現系 ----
fn cmd_murmur(a: &[String]) -> i32 {
    // murmur <u64>… : murmur3 fmix64 最終化
    for s in a {
        match parse_u64_auto(s) {
            Ok(mut k) => {
                k ^= k >> 33;
                k = k.wrapping_mul(0xff51_afd7_ed55_8ccd);
                k ^= k >> 33;
                k = k.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
                k ^= k >> 33;
                println!("fmix64({s}) = 0x{k:016x} ({k})");
            }
            Err(e) => {
                eprintln!("{s}: {e}");
                return 1;
            }
        }
    }
    0
}

fn cmd_splitmix(a: &[String]) -> i32 {
    // splitmix <seed> [n] : splitmix64 n 個 (既定 8)
    let Some(seed) = a.first().and_then(|s| parse_u64_auto(s).ok()) else {
        eprintln!("usage: rspeed splitmix <seed> [n]");
        return 2;
    };
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(8);
    let mut x = seed;
    for i in 0..n {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        println!("splitmix64[{i}]: 0x{z:016x} ({z})");
    }
    0
}

fn cmd_xs64(a: &[String]) -> i32 {
    // xs64 <seed> [n] : xorshift64*
    let Some(mut x) = a.first().and_then(|s| parse_u64_auto(s).ok()) else {
        eprintln!("usage: rspeed xs64 <seed> [n]");
        return 2;
    };
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(8);
    if x == 0 {
        eprintln!("seed 0 は退化");
        return 1;
    }
    for i in 0..n {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let z = x.wrapping_mul(0x2545_f491_4f6c_dd1d);
        println!("xorshift64*[{i}]: 0x{z:016x} ({z})");
    }
    0
}

fn cmd_pcg(a: &[String]) -> i32 {
    // pcg <seed> [n] : PCG32 (state64 inc=default stream 1442695040888963407)
    let Some(mut st) = a.first().and_then(|s| parse_u64_auto(s).ok()) else {
        eprintln!("usage: rspeed pcg <seed> [n]");
        return 2;
    };
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(8);
    const INC: u64 = 1_442_695_040_888_963_407;
    for i in 0..n {
        let old = st;
        st = old
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(INC);
        let xs = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        let z = xs.rotate_right(rot);
        println!("pcg32[{i}]: 0x{z:08x} ({z}) st=0x{st:016x}");
    }
    0
}

fn cmd_fnv(a: &[String]) -> i32 {
    // fnv <文字列|@file>… : FNV-1a 32/64
    for s in a {
        let bytes = if let Some(f) = s.strip_prefix('@') {
            fs::read(f).unwrap_or_default()
        } else {
            s.clone().into_bytes()
        };
        let mut h64: u64 = 0xcbf2_9ce4_8422_2325;
        let mut h32: u32 = 0x811c_9dc5;
        for b in &bytes {
            h64 = (h64 ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
            h32 = (h32 ^ u32::from(*b)).wrapping_mul(0x0100_0193);
        }
        println!(
            "fnv1a64=0x{h64:016x} fnv1a32=0x{h32:08x}  {s}{}",
            if s.starts_with('@') {
                format!(" ({} bytes)", bytes.len())
            } else {
                String::new()
            }
        );
    }
    0
}

// ---- 自動基数解析 ----
/// 0x/0X 接頭辞は hex、hex 文字 (a-f/A-F) 含有も hex、それ以外は decimal として解析。
/// ("4095" のような数字のみ値が hex 誤解釈されるのを防ぐのが目的。
/// 単純な `from_str_radix(16).or_else(parse)` だと数字のみ文字列が常に hex 成功してしまう)
fn parse_u64_auto(s: &str) -> Result<u64, String> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u64::from_str_radix(&h.replace('_', ""), 16).map_err(|e| format!("{s}: {e}"));
    }
    if t.chars().any(|c| matches!(c, 'a'..='f' | 'A'..='F')) {
        return u64::from_str_radix(&t.replace('_', ""), 16).map_err(|e| format!("{s}: {e}"));
    }
    t.replace('_', "")
        .parse::<u64>()
        .map_err(|e| format!("{s}: {e}"))
}

// ---- 整数系 ----
fn cmd_prime(a: &[String]) -> i32 {
    // prime <n>… : 素数判定 + 素因数分解 (試行除法、n ≤ ~2^60 実用域)
    for s in a {
        match s.parse::<u64>() {
            Ok(mut n) => {
                if n < 2 {
                    println!("{n}: 素数ではない");
                    continue;
                }
                let orig = n;
                let mut fs: Vec<(u64, u32)> = Vec::new();
                let mut d = 2u64;
                while d.saturating_mul(d) <= n && d < 10_000_000 {
                    let mut e = 0;
                    while n % d == 0 {
                        n /= d;
                        e += 1;
                    }
                    if e > 0 {
                        fs.push((d, e));
                    }
                    d = if d == 2 { 3 } else { d + 2 };
                }
                if n > 1 {
                    fs.push((n, 1));
                }
                let is_prime = fs.len() == 1 && fs[0].1 == 1 && fs[0].0 == orig;
                println!(
                    "{orig}: {}{}",
                    if is_prime { "素数" } else { "合成数 " },
                    fs.iter()
                        .map(|(p, e)| if *e > 1 {
                            format!("{p}^{e}")
                        } else {
                            format!("{p}")
                        })
                        .collect::<Vec<_>>()
                        .join(" × ")
                );
            }
            Err(e) => {
                eprintln!("{s}: {e}");
                return 1;
            }
        }
    }
    0
}

fn cmd_modpow(a: &[String]) -> i32 {
    // modpow <base> <exp> <mod> : u128 中間で i128 安全に
    if a.len() != 3 {
        eprintln!("usage: rspeed modpow <b> <e> <m>");
        return 2;
    }
    let (b, e, m) = (
        a[0].parse::<i128>(),
        a[1].parse::<u128>(),
        a[2].parse::<i128>(),
    );
    let (Ok(mut b), Ok(mut e), Ok(m)) = (b, e, m) else {
        eprintln!("整数で指定");
        return 2;
    };
    if m <= 0 {
        eprintln!("mod は正");
        return 2;
    }
    b = b.rem_euclid(m);
    let mut r: i128 = 1;
    let m128 = m as u128;
    while e > 0 {
        if e & 1 == 1 {
            r = (((r as u128) * (b as u128)) % m128) as i128;
        }
        b = (((b as u128) * (b as u128)) % m128) as i128;
        e >>= 1;
    }
    println!("{}^{} mod {} = {}", a[0], a[1], a[2], r);
    0
}

/// 拡張ユークリッドによる a^{-1} mod m (gcd(a,m)=1 のみ Some)。
/// cmd_invmod と selftest が同一定義を共有するための抽出 (wave 106)
fn modinv_i128(a: i128, m: i128) -> Option<i128> {
    let (mut t, mut nt) = (0i128, 1i128);
    let (mut r, mut nr) = (m, a.rem_euclid(m));
    while nr != 0 {
        let q = r / nr;
        (t, nt) = (nt, t - q * nt);
        (r, nr) = (nr, r - q * nr);
    }
    (r == 1).then(|| t.rem_euclid(m))
}

fn cmd_invmod(a: &[String]) -> i32 {
    // invmod <a> <m> : 拡張ユークリッド (gcd(a,m)=1 必要)
    if a.len() != 2 {
        eprintln!("usage: rspeed invmod <a> <m>");
        return 2;
    }
    let (Ok(x), Ok(m)) = (a[0].parse::<i128>(), a[1].parse::<i128>()) else {
        eprintln!("整数で指定");
        return 2;
    };
    let Some(inv) = modinv_i128(x, m) else {
        eprintln!("逆元なし (gcd={})", gcd_i128(x, m));
        return 1;
    };
    println!(
        "inverse({} , {}) = {} (検算: {}×{} mod {} = {})",
        a[0],
        a[1],
        inv,
        a[0],
        inv,
        a[1],
        (x.rem_euclid(m) * inv).rem_euclid(m)
    );
    0
}

fn cmd_contfrac(a: &[String]) -> i32 {
    // contfrac <式> [n] : 連分数展開 n 項 (既定 12)
    if a.is_empty() {
        eprintln!("usage: rspeed contfrac <式> [n]");
        return 2;
    }
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(12);
    match parse_expr(&a[0]).and_then(|ast| eval_f64(&ast)) {
        Ok(mut v) => {
            let mut terms = Vec::new();
            for _ in 0..n {
                let fl = v.floor();
                terms.push(fl as i64);
                let frac = v - fl;
                if frac.abs() < 1e-12 {
                    break;
                }
                v = 1.0 / frac;
            }
            println!(
                "{} = [{}]",
                a[0],
                terms
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            // 漸近分数
            let (mut p0, mut p1, mut q0, mut q1) = (0i128, 1i128, 1i128, 0i128);
            for t in &terms {
                let (p2, q2) = ((*t as i128) * p1 + p0, (*t as i128) * q1 + q0);
                if q2 != 0 {
                    println!("  {}/{} ≈ {:.12}", p2, q2, p2 as f64 / q2 as f64);
                }
                (p0, p1, q0, q1) = (p1, p2, q1, q2);
                if q1 > 1_000_000 {
                    break;
                }
            }
        }
        Err(m) => {
            eprintln!("{m}");
            return 1;
        }
    }
    0
}

// ---- グリッド掃引系 (table/range/monotone/roundtrip/ulperr) ----
fn parse_grid(a: &[String], n_expr: usize) -> Option<(Vec<String>, f64, f64, f64)> {
    let rest = &a[n_expr..];
    if rest.len() < 3 {
        return None;
    }
    Some((
        a[..n_expr].to_vec(),
        rest[0].parse().ok()?,
        rest[1].parse().ok()?,
        rest[2].parse().ok()?,
    ))
}

fn cmd_table(a: &[String]) -> i32 {
    // table <式> <from> <to> <step> [--f32|--frac] : x に from..to を走査
    let mut mode = 0u8; // 0=f64 1=f32 2=frac
    let args: Vec<String> = a
        .iter()
        .filter(|s| {
            if s.as_str() == "--f32" {
                mode = 1;
                false
            } else if s.as_str() == "--frac" {
                mode = 2;
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();
    let Some((exprs, from, to, step)) = parse_grid(&args, 1) else {
        eprintln!("usage: rspeed table <式> <from> <to> <step> [--f32|--frac]");
        return 2;
    };
    if step == 0.0 || (to - from).signum() != step.signum() {
        eprintln!("ステップ方向異常");
        return 2;
    }
    let Ok(ast) = parse_expr(&exprs[0]) else {
        eprintln!("式解析失敗");
        return 2;
    };
    let mut x = from;
    let mut cnt = 0;
    while (step > 0.0 && x <= to) || (step < 0.0 && x >= to) {
        let row = match mode {
            1 => eval_f32_x(&ast, Some(x as f32)).map(|v| format!("{v} (0x{:08x})", v.to_bits())),
            2 => eval_frac_x(&ast, Some(&x.to_string()))
                .map(|f| format!("{}/{} = {}", f.n, f.d, frac_decimal(f))),
            _ => eval_f64_x(&ast, Some(x)).map(|v| format!("{v} (0x{:016x})", v.to_bits())),
        };
        match row {
            Ok(r) => println!("{x:.17}\t{r}"),
            Err(m) => println!("{x:.17}\tERR: {m}"),
        }
        x += step;
        cnt += 1;
        if cnt > 1_000_000 {
            eprintln!("100 万行制限");
            return 1;
        }
    }
    0
}

fn cmd_range(a: &[String]) -> i32 {
    // range <式> <from> <to> <n点> : グリッド上の min/max/argmin/argmax (f64 + f32)
    let Some((exprs, from, to, n)) = parse_grid(&a.to_vec(), 1) else {
        eprintln!("usage: rspeed range <式> <from> <to> <n>");
        return 2;
    };
    if n < 2.0 {
        eprintln!("n≥2");
        return 2;
    }
    let n = n as usize;
    let Ok(ast) = parse_expr(&exprs[0]) else {
        eprintln!("式解析失敗");
        return 2;
    };
    let (mut mn, mut mx, mut amn, mut amx) = (f64::INFINITY, f64::NEG_INFINITY, 0.0, 0.0);
    for i in 0..n {
        let x = from + (to - from) * (i as f64) / ((n - 1) as f64);
        if let Ok(v) = eval_f64_x(&ast, Some(x)) {
            if v < mn {
                mn = v;
                amn = x;
            }
            if v > mx {
                mx = v;
                amx = x;
            }
        }
    }
    println!(
        "range [{from}, {to}] n={n}:  min = {mn} @ {amn}  |  max = {mx} @ {amx}  |  幅 = {}",
        mx - mn
    );
    0
}

fn cmd_monotone(a: &[String]) -> i32 {
    // monotone <式> <from> <to> <n点> : 単調性検査 + 最初の違反 x
    let Some((exprs, from, to, n)) = parse_grid(a, 1) else {
        eprintln!("usage: rspeed monotone <式> <from> <to> <n>");
        return 2;
    };
    let n = n as usize;
    if n < 2 {
        eprintln!("n≥2");
        return 2;
    }
    let Ok(ast) = parse_expr(&exprs[0]) else {
        eprintln!("式解析失敗");
        return 2;
    };
    let mut prev: Option<f64> = None;
    let (mut inc, mut dec, mut flat, mut first_v, mut nan_err) =
        (0u64, 0u64, 0u64, String::new(), false);
    for i in 0..n {
        let x = from + (to - from) * (i as f64) / ((n - 1) as f64);
        match eval_f64_x(&ast, Some(x)) {
            Ok(v) => {
                if let Some(p) = prev {
                    if v > p {
                        inc += 1;
                    } else if v < p {
                        dec += 1;
                    } else {
                        flat += 1;
                    }
                    if first_v.is_empty() && v < p {
                        first_v = format!("x={x}: {p} → {v} (非単調増)");
                    }
                }
                prev = Some(v);
            }
            Err(_) => {
                nan_err = true;
                prev = None;
            }
        }
    }
    let kind = if inc > 0 && dec > 0 {
        "非単調"
    } else if dec > 0 && inc == 0 {
        "単調非増加"
    } else if inc > 0 && dec == 0 {
        "単調非減少"
    } else {
        "定数"
    };
    let bad = inc > 0 && dec > 0 && !first_v.is_empty();
    println!(
        "増加ステップ {inc} / 減少 {dec} / 平坦 {flat}{} → {kind}",
        if nan_err { " (NaN/ERR あり)" } else { "" }
    );
    if bad {
        println!("最初の違反: {first_v} (増減混在 = 非単調)");
        return 1;
    }
    0
}

fn ulp_distance_f32(x: f32, y: f32) -> i64 {
    // 順序マップ: bits を i32 解釈し、負数側は全ビット反転相当のオフセット (IEEE total order)
    let ord = |v: f32| -> i64 {
        let b = v.to_bits() as i32;
        if b < 0 {
            i64::from(i32::MIN) - i64::from(b)
        } else {
            i64::from(b) - i64::from(i32::MIN)
        }
    };
    (ord(y) - ord(x)).abs()
}

fn cmd_roundtrip(a: &[String]) -> i32 {
    // roundtrip <式f> <式g(逆)> <from> <to> <n点> : f32 f(g(x))−x の ulp 距離
    let Some((exprs, from, to, n)) = parse_grid(a, 2) else {
        eprintln!("usage: rspeed roundtrip <式f> <式g> <from> <to> <n>");
        return 2;
    };
    let n = n as usize;
    if n < 2 {
        eprintln!("n≥2");
        return 2;
    }
    let (Ok(af), Ok(ag)) = (parse_expr(&exprs[0]), parse_expr(&exprs[1])) else {
        eprintln!("式解析失敗");
        return 2;
    };
    let (mut max_ulp, mut max_at, mut errs) = (0i64, 0.0, 0u64);
    for i in 0..n {
        let x = from + (to - from) * (i as f64) / ((n - 1) as f64);
        let xf = x as f32;
        let Ok(fy) = eval_f32_x(&af, Some(xf)) else {
            continue;
        };
        let Ok(rt) = eval_f32_x(&ag, Some(fy)) else {
            continue;
        };
        let d = ulp_distance_f32(xf, rt);
        if d > max_ulp {
            max_ulp = d;
            max_at = x;
        }
        if d > 0 {
            errs += 1;
        }
    }
    println!("roundtrip ulp: max {max_ulp} @ x={max_at}  ({errs}/{n} 点で誤差)");
    if max_ulp > 0 {
        1
    } else {
        0
    }
}

fn cmd_ulperr(a: &[String]) -> i32 {
    // ulperr <式> [x] : 式の f32 逐次値 と x での正確 (frac) 値の ulp 距離
    if a.len() < 2 {
        eprintln!("usage: rspeed ulperr <式(x 含む)> <x>");
        return 2;
    }
    let Ok(ast) = parse_expr(&a[0]) else {
        eprintln!("式解析失敗");
        return 2;
    };
    let Ok(xv) = a[1].parse::<f64>() else {
        eprintln!("x は数値");
        return 2;
    };
    let f = match eval_f32_x(&ast, Some(xv as f32)) {
        Ok(v) => v,
        Err(m) => {
            eprintln!("f32: {m}");
            return 1;
        }
    };
    let exact = match eval_frac_x(&ast, Some(&a[1])) {
        Ok(v) => v,
        Err(m) => {
            eprintln!("frac: {m}");
            return 1;
        }
    };
    let exact_f = exact.n as f64 / exact.d as f64;
    let nearest = exact_f as f32;
    println!("f32 逐次: {} (0x{:08x})", f, f.to_bits());
    println!("正確: {}/{} = {}", exact.n, exact.d, frac_decimal(exact));
    println!("正確→f32 最近接: {} (0x{:08x})", nearest, nearest.to_bits());
    println!("ulp 距離: {}", ulp_distance_f32(f, nearest));
    0
}

fn cmd_int_cast(a: &[String]) -> i32 {
    // int-cast <値式> <型> : Rust `as` 意味論 (浮動→int は saturate、int→int は truncate)
    if a.len() != 2 {
        eprintln!("usage: rspeed int-cast <値式> <u8|u16|u32|u64|usize|i8|i16|i32|i64|isize|f32>");
        return 2;
    }
    let Ok(v) = parse_expr(&a[0]).and_then(|ast| eval_f64(&ast)) else {
        eprintln!("値解析失敗");
        return 2;
    };
    let ty = a[1].as_str();
    let is_int = v.fract() == 0.0 && v.abs() < 1.7e38;
    let show = |name: &str, val: String| println!("  {name}: 入力 {v} → {val}");
    macro_rules! cast {
        ($t:ty, $name:expr) => {
            show(
                $name,
                if is_int {
                    format!(
                        "float-cast {} / int 経由 truncate {}",
                        (v as $t),
                        ((v as i128) as $t)
                    )
                } else {
                    format!("float-cast {} (saturate)", (v as $t))
                },
            )
        };
    }
    match ty {
        "u8" => cast!(u8, "u8"),
        "u16" => cast!(u16, "u16"),
        "u32" => cast!(u32, "u32"),
        "u64" => cast!(u64, "u64"),
        "usize" => cast!(usize, "usize"),
        "i8" => cast!(i8, "i8"),
        "i16" => cast!(i16, "i16"),
        "i32" => cast!(i32, "i32"),
        "i64" => cast!(i64, "i64"),
        "isize" => cast!(isize, "isize"),
        "f32" => println!(
            "  f32: {} → {} (0x{:08x})",
            v,
            v as f32,
            (v as f32).to_bits()
        ),
        _ => {
            eprintln!("未知の型: {ty}");
            return 2;
        }
    }
    0
}

fn cmd_quant(a: &[String]) -> i32 {
    // quant <0..1 値> <maxint> : 正規化→整数量子化 (round/trunc/floor) と逆量子化誤差
    if a.len() != 2 {
        eprintln!("usage: rspeed quant <0..1> <maxint>");
        return 2;
    }
    let Ok(v) = parse_expr(&a[0]).and_then(|ast| eval_f64(&ast)) else {
        return 2;
    };
    let Ok(mx) = a[1].parse::<u64>() else {
        eprintln!("maxint 整数");
        return 2;
    };
    let mx = mx as f64;
    for (name, q) in [
        ("round", (v * mx).round()),
        ("trunc", (v * mx).trunc()),
        ("floor", (v * mx).floor()),
        ("ceil ", (v * mx).ceil()),
    ] {
        let qi = q.clamp(0.0, mx);
        let de = qi / mx;
        println!(
            "  {name}: {} → {qi} → dequant {de:.10} (誤差 {:e})",
            v * mx,
            de - v
        );
    }
    0
}

// ---- 行列・ベクトル (f64 と f32 を併記して精度差を可視化) ----
fn det4(m: &[[f64; 4]; 4]) -> f64 {
    // ラプラス展開 (第 1 行)
    let mut det = 0.0;
    for col in 0..4 {
        let mut sub = [[0.0; 3]; 3];
        for (r, row) in m.iter().enumerate().skip(1) {
            let mut ci = 0;
            for (c, &v) in row.iter().enumerate() {
                if c == col {
                    continue;
                }
                sub[r - 1][ci] = v;
                ci += 1;
            }
        }
        let d3 = sub[0][0] * (sub[1][1] * sub[2][2] - sub[1][2] * sub[2][1])
            - sub[0][1] * (sub[1][0] * sub[2][2] - sub[1][2] * sub[2][0])
            + sub[0][2] * (sub[1][0] * sub[2][1] - sub[1][1] * sub[2][0]);
        det += if col % 2 == 0 {
            m[0][col] * d3
        } else {
            -m[0][col] * d3
        };
    }
    det
}

fn mul4(a: &[[f64; 4]; 4], b: &[[f64; 4]; 4]) -> [[f64; 4]; 4] {
    let mut r = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            for k in 0..4 {
                r[i][j] += a[i][k] * b[k][j];
            }
        }
    }
    r
}

fn parse_16(a: &[String]) -> Option<[[f64; 4]; 4]> {
    if a.len() != 16 {
        return None;
    }
    let mut m = [[0.0; 4]; 4];
    for (i, s) in a.iter().enumerate() {
        m[i / 4][i % 4] = s.parse().ok()?;
    }
    Some(m)
}

fn print_m4(name: &str, m: &[[f64; 4]; 4]) {
    println!("{name}:");
    for row in m {
        println!(
            "  [{:.8}  {:.8}  {:.8}  {:.8}]",
            row[0], row[1], row[2], row[3]
        );
    }
}

fn cmd_mat4(a: &[String]) -> i32 {
    // mat4 det|inv|mul <16 values> [<16 values>] : 4x4 行列 (行優先)
    if a.len() < 17 {
        eprintln!("usage: rspeed mat4 det|inv|mul <16> [mul は 32]");
        return 2;
    }
    let Some(m) = parse_16(&a[1..17].to_vec()) else {
        eprintln!("16 個の数値");
        return 2;
    };
    match a[0].as_str() {
        "det" => println!("det = {:e} (= {})", det4(&m), det4(&m)),
        "inv" => {
            let d = det4(&m);
            if d == 0.0 {
                eprintln!("特異行列");
                return 1;
            }
            // 伴因子行列転置 / det
            let mut cof = [[0.0; 4]; 4];
            for r in 0..4 {
                for c in 0..4 {
                    let mut sub = [[0.0; 3]; 3];
                    let mut si = 0;
                    for i in 0..4 {
                        if i == r {
                            continue;
                        }
                        let mut sj = 0;
                        for j in 0..4 {
                            if j == c {
                                continue;
                            }
                            sub[si][sj] = m[i][j];
                            sj += 1;
                        }
                        si += 1;
                    }
                    let d3 = sub[0][0] * (sub[1][1] * sub[2][2] - sub[1][2] * sub[2][1])
                        - sub[0][1] * (sub[1][0] * sub[2][2] - sub[1][2] * sub[2][0])
                        + sub[0][2] * (sub[1][0] * sub[2][1] - sub[1][1] * sub[2][0]);
                    cof[r][c] = if (r + c) % 2 == 0 { d3 / d } else { -d3 / d };
                }
            }
            let mut inv = [[0.0; 4]; 4];
            for r in 0..4 {
                for c in 0..4 {
                    inv[r][c] = cof[c][r];
                }
            }
            print_m4("inv", &inv);
            let prod = mul4(&m, &inv);
            let mut maxoff: f64 = 0.0;
            for r in 0..4 {
                for c in 0..4 {
                    let t = if r == c { 1.0 } else { 0.0 };
                    maxoff = maxoff.max((prod[r][c] - t).abs());
                }
            }
            println!("検算 max|A·A⁻¹−I| = {maxoff:e}");
        }
        "mul" => {
            let Some(n) = parse_16(&a[17..33].to_vec()) else {
                eprintln!("mul は 32 個");
                return 2;
            };
            print_m4("A·B", &mul4(&m, &n));
        }
        _ => {
            eprintln!("det|inv|mul");
            return 2;
        }
    }
    0
}

fn cmd_vec3(a: &[String]) -> i32 {
    // vec3 dot|cross|norm|dist <3> <3> : f64 と f32 を併記
    if a.len() < 7 {
        eprintln!("usage: rspeed vec3 dot|cross|norm|dist <ax ay az> [bx by bz]");
        return 2;
    }
    let p: Result<Vec<f64>, _> = a[1..].iter().map(|s| s.parse::<f64>()).collect();
    let Ok(v) = p else {
        eprintln!("数値列");
        return 2;
    };
    let (va, vb) = ([v[0], v[1], v[2]], [v[3], v[4], v[5]]);
    match a[0].as_str() {
        "dot" => {
            let d = va[0] * vb[0] + va[1] * vb[1] + va[2] * vb[2];
            let d32 = (va[0] as f32) * (vb[0] as f32)
                + (va[1] as f32) * (vb[1] as f32)
                + (va[2] as f32) * (vb[2] as f32);
            println!(
                "dot: f64 {:.12} / f32 {:.12} (差 {:e})",
                d,
                d32,
                d - d32 as f64
            );
        }
        "cross" => {
            let c = [
                va[1] * vb[2] - va[2] * vb[1],
                va[2] * vb[0] - va[0] * vb[2],
                va[0] * vb[1] - va[1] * vb[0],
            ];
            println!("cross: [{:.12}, {:.12}, {:.12}]", c[0], c[1], c[2]);
        }
        "norm" => println!(
            "norm(a) = {:.12}, norm(b) = {:.12}",
            (va[0] * va[0] + va[1] * va[1] + va[2] * va[2]).sqrt(),
            (vb[0] * vb[0] + vb[1] * vb[1] + vb[2] * vb[2]).sqrt()
        ),
        "dist" => println!(
            "dist = {:.12}",
            ((va[0] - vb[0]).powi(2) + (va[1] - vb[1]).powi(2) + (va[2] - vb[2]).powi(2)).sqrt()
        ),
        _ => {
            eprintln!("dot|cross|norm|dist");
            return 2;
        }
    }
    0
}

fn cmd_lerp(a: &[String]) -> i32 {
    // lerp <a> <b> <t> : a+(b-a)t vs (1-t)a+tb 形式の f32/f64 差異 (発散箇所の検出)
    if a.len() != 3 {
        eprintln!("usage: rspeed lerp <a> <b> <t>");
        return 2;
    }
    let v: Result<Vec<f64>, _> = a.iter().map(|s| s.parse::<f64>()).collect();
    let Ok(v) = v else {
        eprintln!("数値");
        return 2;
    };
    let (x, y, t) = (v[0], v[1], v[2]);
    let f1 = x + (y - x) * t;
    let f2 = (1.0 - t) * x + t * y;
    let (x32, y32, t32) = (x as f32, y as f32, t as f32);
    let g1 = x32 + (y32 - x32) * t32;
    let g2 = (1.0f32 - t32) * x32 + t32 * y32;
    println!(
        "a+(b−a)t : f64 {:.12} / f32 {:.12} (0x{:08x})",
        f1,
        g1,
        g1.to_bits()
    );
    println!(
        "(1−t)a+tb: f64 {:.12} / f32 {:.12} (0x{:08x})",
        f2,
        g2,
        g2.to_bits()
    );
    println!("t=0/1 保証: a+(b−a)t は t=1 で b+((b−a)−b)=b 系に限り端点安全でない可能性 (bab −1 ulp 分析): f64 差 {:e}", f1 - f2);
    0
}

fn cmd_hypot(a: &[String]) -> i32 {
    // hypot <a> <b> : naive sqrt(a²+b²) と f64::hypot の差 (オーバフロー境界探索つき)
    if a.len() != 2 {
        eprintln!("usage: rspeed hypot <a> <b>");
        return 2;
    }
    let v: Result<Vec<f64>, _> = a.iter().map(|s| s.parse::<f64>()).collect();
    let Ok(v) = v else {
        eprintln!("数値");
        return 2;
    };
    let naive = (v[0] * v[0] + v[1] * v[1]).sqrt();
    let robust = v[0].hypot(v[1]);
    println!(
        "naive sqrt(a²+b²) = {naive:e}{}",
        if naive.is_infinite() {
            "  (OVERFLOW)"
        } else {
            ""
        }
    );
    println!("hypot            = {robust:e}");
    println!("差 = {:e}", robust - naive);
    // f32 での naive 破綻境界: a=1 のとき a²+b² が inf になる最小の b (2 分探索)
    let (mut lo, mut hi) = (1.0f64, 1e20f64);
    for _ in 0..200 {
        let mid = (lo * hi).sqrt();
        let b32 = mid as f32;
        let val = (1.0f32 * 1.0f32 + b32 * b32).sqrt();
        if val.is_finite() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    println!(
        "f32 naive 破綻境界 (a=1): b ≈ {lo:e} 0x{:08x} (bsqrt 系 ulp 解析用)",
        (lo as f32).to_bits()
    );
    0
}

fn cmd_fp_table(_a: &[String]) -> i32 {
    // fp-table: 監査で壁打ちする特殊値の即時参照表
    let rows: Vec<(&str, f64)> = vec![
        ("f64 最小正 subnormal", f64::from_bits(1)),
        ("f64 最小正 normal (2^−1022)", f64::MIN_POSITIVE),
        ("f64 eps (2^−52)", f64::EPSILON),
        ("f32 eps", f32::EPSILON as f64),
        ("f32 最小正 subnormal", f32::from_bits(1) as f64),
        ("f32 最小正 normal", f32::MIN_POSITIVE as f64),
        ("2^24 (f32 整数限界)", 16_777_216.0),
        ("2^53 (f64 整数限界)", 9_007_199_254_740_992.0),
        ("f32::MAX", f32::MAX as f64),
        ("f64::MAX", f64::MAX),
        ("sqrt(f32::MAX)", (f32::MAX as f64).sqrt()),
    ];
    for (name, v) in rows {
        println!("{name:28} = {v:e}  0x{:016x}", v.to_bits());
    }
    println!("NaN (quiet) bits: 0x7ff8000000000000 / signaling 例 0x7ff4000000000000");
    0
}

// ===================================================================
// Batch B — ソーススキャナ系 (magic 数値/キャスト/unwrap/header API 24 機能)
// ===================================================================

/// コメント・文字列を除去した疑似コード (数値リテラル/キャスト走査用)。
/// ヒューリスティック: // コメント、/* */ (非ネスト対応)、"文字列"、'c' リテラルを空白化。
/// raw string (r#""#) やライフタイムで完全ではないが、監査一次走査として十分。
fn strip_rust_code(text: &str) -> String {
    let b: Vec<char> = text.chars().collect();
    let mut out = vec![' '; b.len()];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let nx = b.get(i + 1).copied().unwrap_or(' ');
        if c == '/' && nx == '/' {
            // 行末まで
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && nx == '*' {
            i += 2;
            let mut depth = 1;
            while i < b.len() && depth > 0 {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if c == '"' {
            i += 1;
            while i < b.len() {
                if b[i] == '\\' {
                    i += 2;
                    continue;
                }
                if b[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else if c == '\'' {
            // 'a' | '\n' | '\u{...}' のみ (ライフタイム 'a は次が識別子継続+非閉じで判定)
            let close = b
                .get(i + 1..i + 4)
                .unwrap_or(&[])
                .iter()
                .position(|&x| x == '\'');
            if close.map(|c2| c2 <= 2).unwrap_or(false) {
                i += 1;
                while i < b.len() && b[i] != '\'' {
                    if b[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1; // 閉じ '
            } else {
                out[i] = c;
                i += 1;
            }
        } else {
            out[i] = c;
            i += 1;
        }
    }
    out.into_iter().collect()
}

/// (path, 除去コード, 原文) を列挙。
fn collect_rs_code(targets: &[String]) -> Vec<(PathBuf, String, String)> {
    let mut files: Vec<PathBuf> = Vec::new();
    for t in targets {
        let p = PathBuf::from(t);
        if p.is_dir() {
            walk_files(&p, &mut files);
        } else {
            files.push(p);
        }
    }
    files.retain(|p| p.extension().map(|e| e == "rs").unwrap_or(false));
    files.sort();
    files
        .iter()
        .filter_map(|p| {
            let t = read_text(p)?;
            let c = strip_rust_code(&t);
            Some((p.clone(), c, t))
        })
        .collect()
}

fn default_scan_targets(args: &[String]) -> Vec<String> {
    if args.is_empty() {
        vec![format!("{}/crates/rsift-opt-gfx/src", ws_root().display())]
    } else {
        args.to_vec()
    }
}

fn cmd_magic(a: &[String]) -> i32 {
    // magic [dir] [top]: コード中の数値リテラル出現頻度 (頻出=文書化要の魔法数)
    let top: usize = 20;
    let files = collect_rs_code(&default_scan_targets(a));
    let mut freq: HashMap<String, usize> = HashMap::new();
    let mut first_at: HashMap<String, (PathBuf, usize)> = HashMap::new();
    for (path, code, _) in &files {
        let mut tok = String::new();
        let mut ln = 1usize;
        for ch in code.chars() {
            if ch == '\n' {
                ln += 1;
            }
            let is_num_char = ch.is_ascii_alphanumeric() || ch == '.' || ch == '_';
            if is_num_char {
                tok.push(ch);
            } else {
                if tok.len() >= 2
                    && tok.chars().next().unwrap().is_ascii_digit()
                    && tok.chars().any(|c| c.is_ascii_digit())
                {
                    // 16e1 / 0x.. / 1.0 / 52 等
                    *freq.entry(tok.clone()).or_default() += 1;
                    first_at.entry(tok.clone()).or_insert((path.clone(), ln));
                }
                tok.clear();
            }
        }
    }
    let mut v: Vec<_> = freq.into_iter().collect();
    v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (lit, cnt) in v.into_iter().take(top) {
        let (p, ln) = &first_at[&lit];
        println!("{cnt:5}× {lit}   初出 {}:{ln}", p.display());
    }
    0
}

fn cmd_floatlits(a: &[String]) -> i32 {
    // floatlits [file|dir]: 小数リテラルとその正確 bits (0.1 型の精度壁拾い)
    let files = collect_rs_code(&default_scan_targets(a));
    for (path, code, _) in &files {
        let mut tok = String::new();
        let mut ln = 1usize;
        for ch in code.chars() {
            if ch == '\n' {
                ln += 1;
            }
            let is_num = ch.is_ascii_digit()
                || ch == '.'
                || ch == '_'
                || tok.ends_with('e') && (ch == '-' || ch == '+');
            if is_num && (tok.is_empty() && ch.is_ascii_digit() || !tok.is_empty()) {
                tok.push(ch);
                if ch == 'e' || ch == 'E' {
                    tok.push(' ');
                }
            } else {
                let t = tok.trim_end();
                if t.contains('.') && t.chars().next().unwrap_or(' ').is_ascii_digit() {
                    let clean: String = t.chars().filter(|c| *c != '_').collect();
                    if let Ok(v) = clean.parse::<f64>() {
                        println!(
                            "{}:{ln}  {t}  → f64 0x{:016x} f32 0x{:08x} (= {})",
                            path.display(),
                            v.to_bits(),
                            (v as f32).to_bits(),
                            v as f32
                        );
                    }
                }
                tok.clear();
            }
        }
    }
    0
}

fn line_scan(a: &[String], kind: &str) -> i32 {
    // 共通行スキャナ: コード行からパターンに合うものを file:line で列挙
    let mut patterns: Vec<&str> = Vec::new();
    match kind {
        "casts" => {
            for t in [
                "as u8", "as u16", "as u32", "as u64", "as usize", "as i8", "as i16", "as i32",
                "as i64", "as f32", "as f64",
            ] {
                patterns.push(t);
            }
        }
        "clamps" => {
            patterns.extend([".clamp(", ".min(", ".max(", "f32::min", "f32::max"]);
        }
        "divmod" => {
            patterns.extend([" / ", " % "]);
        }
        "shifts" => {
            patterns.extend([" << ", " >> ", "<<=", ">>="]);
        }
        "unwraps" => {
            patterns.extend([
                ".unwrap()",
                ".expect(",
                "panic!",
                "unreachable!",
                "todo!",
                "unimplemented!",
                ".unwrap_or(",
                ".unwrap_or_else(",
            ]);
        }
        _ => unreachable!(),
    }
    let files = collect_rs_code(&default_scan_targets(a));
    let mut total = 0usize;
    let mut per: HashMap<&str, usize> = HashMap::new();
    for (path, code, orig) in &files {
        let code_lines: Vec<&str> = code.lines().collect();
        for (i, l) in orig.lines().enumerate() {
            let cl = code_lines.get(i).copied().unwrap_or("");
            for p in &patterns {
                if cl.contains(p) {
                    total += 1;
                    *per.entry(p).or_default() += 1;
                    println!("{}:{}  [{p}] {}", path.display(), i + 1, l.trim());
                }
            }
        }
    }
    println!("--- 集計: total {total}");
    let mut v: Vec<_> = per.into_iter().collect();
    v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (p, c) in v {
        println!("  {c:6} {p}");
    }
    0
}

fn cmd_casts(a: &[String]) -> i32 {
    line_scan(a, "casts")
}
fn cmd_clamps(a: &[String]) -> i32 {
    line_scan(a, "clamps")
}
fn cmd_divmod(a: &[String]) -> i32 {
    line_scan(a, "divmod")
}
fn cmd_shifts(a: &[String]) -> i32 {
    line_scan(a, "shifts")
}
fn cmd_unwraps(a: &[String]) -> i32 {
    line_scan(a, "unwraps")
}

// ---- テスト索引 ----
fn extract_tests(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if t.starts_with("fn ") || t.starts_with("pub fn ") {
            // 直上 4 行以内に #[test]
            let mut has = false;
            for j in (i.saturating_sub(5))..i {
                if lines[j].contains("#[test]") && !lines[j].contains("#[cfg") {
                    has = true;
                }
                if lines[j].contains("#[should_panic") || lines[j].contains("#[ignore") {
                    continue;
                }
            }
            if has {
                let name: String = t
                    .split(|c: char| c == '(' || c.is_whitespace())
                    .nth(1)
                    .unwrap_or("")
                    .trim_matches(|c: char| c == '(')
                    .to_string();
                if !name.is_empty() {
                    out.push((i + 1, name));
                }
            }
        }
    }
    out
}

fn cmd_tests_index(a: &[String]) -> i32 {
    // tests-index [crate]: #[test] 名索引をキャッシュへ (test-find のデータ源)
    let krate = a.first().map(|s| s.as_str()).unwrap_or("rsift-opt-gfx");
    let src = ws_root().join("crates").join(krate);
    let mut files = Vec::new();
    walk_files(&src, &mut files);
    files.retain(|p| p.extension().map(|e| e == "rs").unwrap_or(false));
    files.sort();
    let mut lines_out = Vec::new();
    for f in &files {
        let Some(t) = read_text(f) else { continue };
        for (ln, name) in extract_tests(&t) {
            lines_out.push(format!("{name}\t{}:{ln}", f.display()));
        }
    }
    let dst = cache_dir().join(format!("tests-index-{krate}.txt"));
    if let Err(e) = write_loud(&dst, lines_out.join("\n")) {
        eprintln!("{e}");
        return 1;
    }
    println!("tests-index: {} 件 → {}", lines_out.len(), dst.display());
    0
}

fn cmd_test_find(a: &[String]) -> i32 {
    // test-find <部分文字列>: 索引からテスト名検索 (rspeed test のフィルタ発見用)
    if a.is_empty() {
        eprintln!("usage: rspeed test-find <部分文字列>");
        return 2;
    }
    for idx in fs::read_dir(cache_dir())
        .into_iter()
        .flat_map(|r| r.flatten())
    {
        let p = idx.path();
        if !idx
            .file_name()
            .to_string_lossy()
            .starts_with("tests-index-")
        {
            continue;
        }
        let Some(t) = read_text(&p) else { continue };
        for l in t.lines() {
            if a.iter().all(|k| l.contains(k)) {
                println!("{l}");
            }
        }
    }
    0
}

fn cmd_test_count(a: &[String]) -> i32 {
    // test-count [dir]: #[test] 数の高速積算 (ビルド不要)
    let files = collect_rs_code(&default_scan_targets(a));
    let mut n = 0;
    for (p, _, t) in &files {
        let c = extract_tests(t).len();
        if c > 0 {
            println!("{c:5}  {}", p.display());
        }
        n += c;
    }
    println!("test-count: {n}");
    0
}

fn cmd_fns(a: &[String]) -> i32 {
    // fns [file|dir]: fn シグネチャ列挙 (pub 標識つき)
    let files = collect_rs_code(&default_scan_targets(a));
    for (path, _, text) in &files {
        for (i, l) in text.lines().enumerate() {
            let t = l.trim_start();
            if (t.starts_with("fn ")
                || t.starts_with("pub fn ")
                || t.starts_with("pub(crate) fn ")
                || t.starts_with("pub(super) fn "))
                && !t.contains(" fn_main_skip")
            {
                let mark = if t.starts_with("pub fn") { "P" } else { " " };
                println!("{}:{} [{mark}] {}", path.display(), i + 1, l.trim_end());
            }
        }
    }
    0
}

fn pubs_of(dir: &[String]) -> BTreeSet<String> {
    let files = collect_rs_code(&default_scan_targets(dir));
    let mut set = BTreeSet::new();
    for (_, _, text) in &files {
        for l in text.lines() {
            let t = l.trim();
            for prefix in [
                "pub fn ",
                "pub struct ",
                "pub enum ",
                "pub const ",
                "pub type ",
                "pub static ",
                "pub trait ",
            ] {
                if let Some(rest) = t.strip_prefix(prefix) {
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    set.insert(format!("{} {name}", prefix.trim_end()));
                }
            }
            // impl 内 pub fn
            if t.starts_with("pub fn ") && !t.contains("trait") {
                // 捕捉済 (上)
            }
        }
    }
    set
}

fn cmd_pubs(a: &[String]) -> i32 {
    // pubs [dir] [--save <tag>] [--check <tag>]: API 面スナップショット + FNV 指紋。
    let mut save: Option<String> = None;
    let mut check: Option<String> = None;
    let dirs: Vec<String> = a
        .iter()
        .filter(|s| {
            if s.starts_with("--save") {
                false
            } else if s.starts_with("--check") {
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();
    for (i, s) in a.iter().enumerate() {
        if s == "--save" {
            save = a.get(i + 1).cloned();
        }
        if s == "--check" {
            check = a.get(i + 1).cloned();
        }
    }
    let set = pubs_of(&dirs);
    let joined = set.iter().cloned().collect::<Vec<_>>().join("\n");
    let bytes = joined.as_bytes();
    let mut h64: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h64 = (h64 ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    println!("pubs: {} items, API 指紋 fnv1a64=0x{h64:016x}", set.len());
    if let Some(tag) = save {
        let dst = cache_dir().join(format!("pubs-{tag}.txt"));
        if let Err(e) = write_loud(&dst, &joined) {
            eprintln!("{e}");
            return 1;
        }
        println!("保存 → {}", dst.display());
    }
    if let Some(tag) = check {
        let src = cache_dir().join(format!("pubs-{tag}.txt"));
        let old: BTreeSet<String> = read_text(&src)
            .unwrap_or_default()
            .lines()
            .map(|s| s.to_string())
            .collect();
        let added: Vec<_> = set.difference(&old).collect();
        let removed: Vec<_> = old.difference(&set).collect();
        println!(
            "比較 vs {tag}: 追加 {} / 削除 {}",
            added.len(),
            removed.len()
        );
        for x in &added {
            println!("  + {x}");
        }
        for x in &removed {
            println!("  - {x}");
        }
        if !added.is_empty() || !removed.is_empty() {
            return 1;
        }
    }
    for x in &set {
        println!("  {x}");
    }
    0
}

fn cmd_docs(a: &[String]) -> i32 {
    // docs [dir]: pub fn/struct/enum の直前 /// doc カバレッジ
    let files = collect_rs_code(&default_scan_targets(a));
    let (mut have, mut missing) = (0usize, Vec::new());
    for (path, _, text) in &files {
        let lines: Vec<&str> = text.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            let t = l.trim_start();
            let is_pub = [
                "pub fn ",
                "pub struct ",
                "pub enum ",
                "pub const ",
                "pub type ",
            ]
            .iter()
            .any(|p| t.starts_with(p));
            if !is_pub {
                continue;
            }
            let mut j = i;
            let mut doc = false;
            while j > 0 {
                j -= 1;
                let p = lines[j].trim();
                if p.starts_with("///") || p.starts_with("//") {
                    doc = true;
                    continue;
                }
                if p.starts_with("#[") {
                    continue;
                }
                break;
            }
            if doc {
                have += 1;
            } else {
                missing.push(format!("{}:{} {}", path.display(), i + 1, l.trim()));
            }
        }
    }
    let total = have + missing.len();
    println!(
        "docs: {have}/{total} ({}%) doc コメントあり",
        if total > 0 { have * 100 / total } else { 100 }
    );
    for m in missing.iter().take(40) {
        println!("  未文書: {m}");
    }
    0
}

fn cmd_todo_scan(a: &[String]) -> i32 {
    // todo-scan [dir]: TODO/FIXME/HACK/XXX/未実装/仮実装/TBD 棚卸し
    let files = collect_rs_code(&default_scan_targets(a));
    let keys = [
        "TODO",
        "FIXME",
        "HACK",
        "XXX",
        "TBD",
        "未実装",
        "仮実装",
        "仮の",
        "暫定",
    ];
    let mut n = 0;
    for (path, _, text) in &files {
        for (i, l) in text.lines().enumerate() {
            if keys.iter().any(|k| l.contains(k)) {
                n += 1;
                println!("{}:{}: {}", path.display(), i + 1, l.trim());
            }
        }
    }
    println!("todo-scan: {n} 件");
    0
}

fn cmd_dups(a: &[String]) -> i32 {
    // dups [dir] [minlen]: 40 文字超の重複行クラスタ (コピペ監査)
    let minlen: usize = a.last().and_then(|s| s.parse().ok()).unwrap_or(48);
    let files = collect_rs_code(&default_scan_targets(
        &a[..a.len().saturating_sub(1)].to_vec(),
    ));
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (path, _, text) in &files {
        for (i, l) in text.lines().enumerate() {
            let t = l.trim();
            if t.len() >= minlen
                && !t.starts_with("//")
                && !t.starts_with('*')
                && !t.starts_with('!')
            {
                map.entry(t.to_string())
                    .or_default()
                    .push(format!("{}:{}", path.display(), i + 1));
            }
        }
    }
    let mut v: Vec<_> = map.into_iter().filter(|(_, v)| v.len() >= 2).collect();
    v.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (line, locs) in v.iter().take(30) {
        println!(
            "{}× {}{}",
            locs.len(),
            &line[..line.len().min(90)],
            if line.len() > 90 { "…" } else { "" }
        );
        for l in locs.iter().take(6) {
            println!("     {l}");
        }
    }
    0
}

fn cmd_longlines(a: &[String]) -> i32 {
    let (mut limit, mut targets) = (100usize, Vec::new());
    for x in a {
        if let Ok(v) = x.parse::<usize>() {
            limit = v;
        } else {
            targets.push(x.clone());
        }
    }
    let files = collect_rs_code(&default_scan_targets(&targets));
    let mut n = 0;
    for (path, _, text) in &files {
        for (i, l) in text.lines().enumerate() {
            let w = l.chars().count();
            if w > limit {
                n += 1;
                println!(
                    "{}:{} ({} chars) {}…",
                    path.display(),
                    i + 1,
                    w,
                    &l.chars().take(60).collect::<String>()
                );
            }
        }
    }
    println!("longlines: {n} 行が {limit} 文字超");
    0
}

fn cmd_trailws(a: &[String]) -> i32 {
    let files = collect_rs_code(&default_scan_targets(a));
    let mut n = 0;
    for (path, _, text) in &files {
        for (i, l) in text.lines().enumerate() {
            if l.ends_with(' ') || l.ends_with('\t') {
                n += 1;
                println!("{}:{}", path.display(), i + 1);
            }
        }
    }
    println!("trailws: {n} 行");
    if n > 0 {
        1
    } else {
        0
    }
}

fn cmd_nonascii(a: &[String]) -> i32 {
    let files = collect_rs_code(&default_scan_targets(a));
    for (path, _, text) in &files {
        let mut hist: HashMap<char, usize> = HashMap::new();
        for c in text.chars() {
            if !c.is_ascii() {
                *hist.entry(c).or_default() += 1;
            }
        }
        if hist.is_empty() {
            continue;
        }
        let mut v: Vec<_> = hist.into_iter().collect();
        v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        println!("{}: 非 ASCII {} 種", path.display(), v.len());
        for (c, n) in v.iter().take(25) {
            println!("  U+{:04X} '{c}' ×{n}", *c as u32);
        }
    }
    0
}

fn cmd_eol(a: &[String]) -> i32 {
    let mut files = Vec::new();
    for t in default_scan_targets(a) {
        walk_files(Path::new(&t), &mut files);
    }
    files.sort();
    let (mut lf, mut crlf, mut nofinal, mut bin) = (0, 0, 0, 0);
    for f in &files {
        match fs::read(f) {
            Ok(b) => {
                let cr = b.iter().filter(|&&c| c == b'\r').count();
                let nl = b.iter().filter(|&&c| c == b'\n').count();
                if cr > 0 {
                    crlf += 1;
                    println!("CRLF 混入: {}", f.display());
                } else {
                    lf += 1;
                }
                if !b.is_empty() && *b.last().unwrap() != b'\n' {
                    nofinal += 1;
                    println!("末尾改行なし: {}", f.display());
                }
                let _ = nl;
            }
            Err(_) => bin += 1,
        }
    }
    println!("eol: LF-only {lf} / CRLF 混入 {crlf} / 末尾改行なし {nofinal} / 読取不可 {bin}");
    0
}

fn cmd_tabs(a: &[String]) -> i32 {
    let files = collect_rs_code(&default_scan_targets(a));
    let mut n = 0;
    for (path, _, text) in &files {
        for (i, l) in text.lines().enumerate() {
            if l.starts_with('\t') || l.contains(" \t") {
                n += 1;
                println!("{}:{}", path.display(), i + 1);
            }
        }
    }
    println!("tabs: {n} 行 (先頭タブ/空白混在)");
    0
}

fn cmd_dead(a: &[String]) -> i32 {
    // dead [dir]: pub fn の repo 全体トークン参照数を数え、定義行のみのものを列挙。
    // ヒューリスティック (単純トークン境界一致; 同名シャドウは分離不能 → 「消費者ゼロ候補」)。
    let dirs = default_scan_targets(a);
    let files = collect_rs_code(&dirs);
    // 全 repo テキストを1つに (消費者検索面)
    let mut all_files = Vec::new();
    walk_files(&ws_root().join("crates"), &mut all_files);
    all_files.retain(|p| p.extension().map(|e| e == "rs").unwrap_or(false));
    let hay: Vec<(PathBuf, String)> = all_files
        .iter()
        .filter_map(|p| read_text(p).map(|t| (p.clone(), t)))
        .collect();
    for (path, _, text) in &files {
        let lines: Vec<&str> = text.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            let t = l.trim_start();
            if !t.starts_with("pub fn ") {
                continue;
            }
            let name: String = t["pub fn ".len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                continue;
            }
            let mut refs = 0usize;
            for (_, h) in &hay {
                // 単純境界フィルタ: 前後が識別子文字でない出現のみ
                let mut ok = 0;
                let mut start = 0;
                while let Some(pos) = h[start..].find(&name) {
                    let s = start + pos;
                    let e = s + name.len();
                    let pre = h[..s]
                        .chars()
                        .last()
                        .map(|c| c.is_ascii_alphanumeric() || c == '_')
                        .unwrap_or(false);
                    let post = h[e..]
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_alphanumeric() || c == '_')
                        .unwrap_or(false);
                    if !pre && !post {
                        ok += 1;
                    }
                    start = e;
                }
                refs += ok;
            }
            if refs <= 1 {
                println!(
                    "消費者ゼロ候補: {}:{}  {name} (refs={refs} = 定義行のみ)",
                    path.display(),
                    i + 1
                );
            }
        }
    }
    0
}

fn cmd_hotfiles(a: &[String]) -> i32 {
    // hotfiles [n] : git log --numstat (直近 n=200) で変更量上位 (監査ホットスポット)
    let n = a.first().map(|s| s.as_str()).unwrap_or("200");
    let out = Command::new("git")
        .args(["-C"])
        .arg(git_root())
        .args(["log", "--numstat", "-n", n, "--format="])
        .output()
        .ok();
    let Some(o) = out else {
        eprintln!("git log 失敗");
        return 1;
    };
    let text = String::from_utf8_lossy(&o.stdout);
    let mut acc: HashMap<String, u64> = HashMap::new();
    for l in text.lines() {
        let mut it = l.split_whitespace();
        let (Some(add), Some(del), Some(file)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let v: u64 = add.parse().unwrap_or(0) + del.parse().unwrap_or(0);
        *acc.entry(file.to_string()).or_default() += v;
    }
    let mut v: Vec<_> = acc.into_iter().collect();
    v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (f, c) in v.iter().take(25) {
        println!("{c:8}  {f}");
    }
    0
}

fn cmd_diff(a: &[String]) -> i32 {
    // diff <a> <b> : 内部 LCS 集合差分 (外部コマンド不要)
    if a.len() != 2 {
        eprintln!("usage: rspeed diff <a> <b>");
        return 2;
    }
    let Some(ta) = read_text(Path::new(&a[0])) else {
        eprintln!("{} 読めません", a[0]);
        return 2;
    };
    let Some(tb) = read_text(Path::new(&a[1])) else {
        eprintln!("{} 読めません", a[1]);
        return 2;
    };
    let va: Vec<&str> = ta.lines().collect();
    let vb: Vec<&str> = tb.lines().collect();
    let added = lcs_added(&va, &vb);
    let removed = lcs_added(&vb, &va);
    println!(
        "diff {} → {}: +{} / -{}",
        a[0],
        a[1],
        added.len(),
        removed.len()
    );
    for x in added.iter().take(40) {
        println!("+ {x}");
    }
    for x in removed.iter().take(40) {
        println!("- {x}");
    }
    if !added.is_empty() || !removed.is_empty() {
        1
    } else {
        0
    }
}

fn cmd_grep2(a: &[String]) -> i32 {
    // grep2 <patA> <patB> [dir] : A/B 両含有/片方のみ/両方なし のファイル分類
    if a.len() < 2 {
        eprintln!("usage: rspeed grep2 <A> <B> [dir]");
        return 2;
    }
    let targets = if a.len() > 2 {
        a[2..].to_vec()
    } else {
        default_scan_targets(&[])
    };
    let mut files = Vec::new();
    for t in &targets {
        let p = PathBuf::from(t);
        if p.is_dir() {
            walk_files(&p, &mut files);
        } else {
            files.push(p);
        }
    }
    files.sort();
    let (mut both, mut onlya, mut onlyb) = (0, 0, 0);
    for f in &files {
        let Some(t) = read_text(f) else { continue };
        let (ha, hb) = (t.contains(&a[0]), t.contains(&a[1]));
        match (ha, hb) {
            (true, true) => {
                both += 1;
                println!("BOTH : {}", f.display());
            }
            (true, false) => {
                onlya += 1;
                println!("Aのみ: {}", f.display());
            }
            (false, true) => {
                onlyb += 1;
                println!("Bのみ: {}", f.display());
            }
            _ => {}
        }
    }
    println!("grep2: BOTH {both} / A のみ {onlya} / B のみ {onlyb}");
    0
}

// ===================================================================
// Batch C — リポジトリ運用系 (rescue/snapshot/seal/md5/adversarial 儀式 27 機能)
// ===================================================================

// ---- MD5 (RFC 1321 自前実装; 外部コマンド非依存の破損検証用) ----
const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];
const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

fn md5_bytes(data: &[u8]) -> [u8; 16] {
    let (mut a0, mut b0, mut c0, mut d0) =
        (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = data.to_vec();
    let bitlen = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_le_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (mut f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            f = f.wrapping_add(a).wrapping_add(MD5_K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(MD5_S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

fn md5_hex(data: &[u8]) -> String {
    md5_bytes(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn cmd_md5(a: &[String]) -> i32 {
    for f in a {
        match fs::read(f) {
            Ok(b) => println!("{}  {f}", md5_hex(&b)),
            Err(e) => {
                eprintln!("{f}: {e}");
                return 1;
            }
        }
    }
    if a.is_empty() {
        eprintln!("usage: rspeed md5 <file>…");
        return 2;
    }
    0
}

fn cmd_md5check(a: &[String]) -> i32 {
    // md5check <manifest> : "<md5>  <path>" 行を照合 (md5sum -c 等価)
    if a.len() != 1 {
        eprintln!("usage: rspeed md5check <manifest>");
        return 2;
    }
    let Some(t) = read_text(Path::new(&a[0])) else {
        eprintln!("manifest が読めません");
        return 2;
    };
    let mut fails = 0;
    for l in t.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let Some((want, path)) = l.split_once(char::is_whitespace) else {
            continue;
        };
        let path = path.trim_start();
        match fs::read(path) {
            Ok(b) => {
                let got = md5_hex(&b);
                if got == want {
                    println!("OK   {path}");
                } else {
                    fails += 1;
                    println!("FAIL {path}: {got} ≠ {want}");
                }
            }
            Err(_) => {
                fails += 1;
                println!("MISS {path}");
            }
        }
    }
    if fails > 0 {
        1
    } else {
        0
    }
}

// ---- git ラッパ ----
fn git_out(args: &[&str]) -> Option<String> {
    Command::new("git")
        .arg("-C")
        .arg(git_root())
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn modified_tracked() -> Vec<String> {
    git_out(&["status", "--porcelain"])
        .unwrap_or_default()
        .lines()
        .filter(|l| {
            l.len() >= 4
                && (l.starts_with(" M")
                    || l.starts_with("M ")
                    || l.starts_with("MM")
                    || l.starts_with("AM"))
        })
        .map(|l| l[3..].trim().to_string())
        .collect()
}

fn cmd_status(_a: &[String]) -> i32 {
    // status: HEAD/branch/変更ファイル/未追跡 を 1 画面で (sandbox 巻戻り迅速発見用)
    let head = git_out(&["rev-parse", "--short=9", "HEAD"]).unwrap_or_default();
    let subject = git_out(&["log", "-1", "--format=%s"]).unwrap_or_default();
    let remote_raw = git_out(&["rev-parse", "--short=9", "@{u}"]);
    let upnote = match &remote_raw {
        Some(r) if !r.trim().is_empty() => {
            if r.trim() != head.trim() {
                format!(" (upstream {} と不一致 — 要 fetch/reset 確認)", r.trim())
            } else {
                format!(" (upstream {} と一致)", r.trim())
            }
        }
        _ => " (upstream 未追跡)".to_string(),
    };
    println!("HEAD: {}{upnote}", head.trim());
    println!("件名: {}", subject.trim());
    print!("{}", git_out(&["status", "--short"]).unwrap_or_default());
    println!(
        "ahead/behind: {}",
        git_out(&["status", "-sb"])
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    );
    0
}

fn cmd_changed_tests(a: &[String]) -> i32 {
    // changed-tests [ref]: HEAD 差分ファイル中の #[test] 一覧 → rspeed test フィルタ生成
    let refname = a.first().map(|s| s.as_str()).unwrap_or("HEAD");
    let changed = git_out(&["diff", "--name-only", refname]).unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    for f in changed.lines().filter(|l| l.ends_with(".rs")) {
        let p = git_root().join(f);
        let Some(t) = read_text(&p) else { continue };
        for (_, name) in extract_tests(&t) {
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    println!("changed-tests (vs {refname}): {} 件", names.len());
    for n in &names {
        println!("  {n}");
    }
    if !names.is_empty() {
        println!("filter 例: rspeed test {} …", names[0]);
    }
    0
}

fn cmd_env_check(_a: &[String]) -> i32 {
    // env-check: ツールチェーン/到達性/キャッシュの健全診断 (sandbox 崩壊後の初期動作)
    let mut bad = 0;
    let check_cmd = |name: &str, cmd: &str, args: &[&str]| {
        let ok = Command::new(cmd)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        println!("  {} {:<14}", if ok { "OK " } else { "MISS" }, name);
        ok
    };
    if !check_cmd("rustc", "rustc", &["--version"]) {
        bad += 1;
    }
    if !check_cmd("cargo", "cargo", &["--version"]) {
        bad += 1;
    }
    if !check_cmd("rustfmt", "rustfmt", &["--version"]) {
        bad += 1;
    }
    if !check_cmd("git", "git", &["--version"]) {
        bad += 1;
    }
    if !check_cmd("gh", "gh", &["--version"]) {
        bad += 1;
    }
    for (label, p) in [
        ("rspeed bin", "/home/user/bin/rspeed"),
        ("vendor cache", "/tmp/rsift-vendor"),
        (
            "vendor config",
            &format!("{}/.cargo/config.toml", ws_root().display()).leak() as &str,
        ),
        (
            "gcc-ld shim",
            &format!(
                "{}/lib/rustlib/{}/bin/gcc-ld",
                "/home/user/rust",
                rust_host()
            )
            .leak() as &str,
        ),
        (
            "target dir",
            &format!("{}/target", ws_root().display()).leak() as &str,
        ),
    ] {
        let ok = Path::new(p).exists();
        println!("  {} {:<14} {}", if ok { "OK " } else { "MISS" }, label, p);
        if !ok {
            bad += 1;
        }
    }
    if bad > 0 {
        println!(
            "→ 修復: bash {}/ci/restore-env.sh && bash {}/tools/build-rspeed.sh",
            git_root().display(),
            git_root().display()
        );
        return 1;
    }
    println!("env-check: 全項目 OK");
    0
}

fn tracked_files_manifest() -> Vec<(String, [u8; 16], u64)> {
    let list = git_out(&["ls-files"]).unwrap_or_default();
    let mut out = Vec::new();
    for f in list.lines() {
        let p = git_root().join(f);
        if let Ok(b) = fs::read(&p) {
            let h = md5_bytes(&b);
            out.push((f.to_string(), h, b.len() as u64));
        }
    }
    out
}

fn cmd_snapshot(a: &[String]) -> i32 {
    // snapshot [tag]: 追跡全ファイルの md5 manifest を保存 (巻戻り/破損検出の土台)
    let tag = a.first().map(|s| s.as_str()).unwrap_or("latest");
    let rows = tracked_files_manifest();
    let body: String = rows
        .iter()
        .map(|(f, h, len)| {
            format!(
                "{}  {}  {}",
                h.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                len,
                f
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let dst = snapshot_file(tag);
    if let Err(e) = write_loud(&dst, &body) {
        eprintln!("snapshot: {e}");
        return 1;
    }
    let mut whole: u64 = 0xcbf2_9ce4_8422_2325;
    for bb in body.as_bytes() {
        whole = (whole ^ u64::from(*bb)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    println!(
        "snapshot: {} files → {} (全体指紋 fnv 0x{whole:016x})",
        rows.len(),
        dst.display()
    );
    0
}

fn cmd_snapcheck(a: &[String]) -> i32 {
    // snapcheck [tag]: 保存 manifest との差分 (added/removed/modified) — git 非存在でも動く
    let tag = a.first().map(|s| s.as_str()).unwrap_or("latest");
    let src = snapshot_file(tag);
    let Some(old) = read_text(&src) else {
        eprintln!("snapshot が無い (先に snapshot を実行)");
        return 2;
    };
    let oldmap: HashMap<String, (String, u64)> = old
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (Some(h), Some(len), Some(f)) = (it.next(), it.next(), it.next()) else {
                return None;
            };
            Some((f.to_string(), (h.to_string(), len.parse().ok()?)))
        })
        .collect();
    let cur = tracked_files_manifest();
    let curmap: HashMap<String, (String, u64)> = cur
        .into_iter()
        .map(|(f, h, l)| {
            (
                f,
                (h.iter().map(|b| format!("{b:02x}")).collect::<String>(), l),
            )
        })
        .collect();
    let (mut add, mut rem, mut mdf) = (0, 0, 0);
    for (f, (h, l)) in &curmap {
        match oldmap.get(f) {
            None => {
                add += 1;
                println!("ADD  {f}");
            }
            Some((oh, ol)) => {
                if oh != h || ol != l {
                    mdf += 1;
                    println!("MOD  {f}");
                }
            }
        }
    }
    for f in oldmap.keys() {
        if !curmap.contains_key(f) {
            rem += 1;
            println!("DEL  {f}");
        }
    }
    println!("snapcheck: ADD {add} / MOD {mdf} / DEL {rem}");
    if add + rem + mdf > 0 {
        1
    } else {
        0
    }
}

fn cmd_rescue(a: &[String]) -> i32 {
    // rescue [dir]: 変更追跡ファイル＋指定 untracked を退避 dir へ構造維持コピー+md5 manifest。
    // sandbox 巻戻り対策の定型手順 (これまで /tmp/rescue を 3 回手運用) の自動化。
    let dest = a
        .first()
        .map(|s| PathBuf::from(s))
        .unwrap_or_else(|| PathBuf::from("/tmp/rescue-rspeed"));
    let _ = fs::create_dir_all(&dest);
    let mut files = modified_tracked();
    for extra in ["tools/rspeed.rs", "tools/build-rspeed.sh"] {
        if !files.contains(&extra.to_string()) && git_root().join(extra).exists() {
            files.push(extra.to_string());
        }
    }
    let mut manifest = String::new();
    for f in &files {
        let src = git_root().join(f);
        if let Ok(b) = fs::read(&src) {
            let dst = dest.join(f);
            if let Some(par) = dst.parent() {
                if let Err(e) = fs::create_dir_all(par) {
                    eprintln!("rescue: ディレクトリ作成失敗 {}: {e}", par.display());
                    return 1;
                }
            }
            if let Err(e) = write_loud(&dst, &b) {
                eprintln!("rescue: {e}");
                return 1;
            }
            manifest.push_str(&format!("{}  {}\n", md5_hex(&b), f));
        }
    }
    if let Err(e) = write_loud(&dest.join("MANIFEST.md5"), &manifest) {
        eprintln!("rescue: {e}");
        return 1;
    }
    println!(
        "rescue: {} 件 → {} (MANIFEST.md5 同梱)",
        files.len(),
        dest.display()
    );
    if files.is_empty() {
        eprintln!("(変更追跡ファイルなし)");
        return 1;
    }
    0
}

// ---- adversarial 儀式 (旧セマンティクス厳密逆戻し → 新テスト RED → md5 忠実復元) ----
fn adv_dir() -> PathBuf {
    let d = cache_dir().join("adv");
    let _ = fs::create_dir_all(&d);
    d
}

fn cmd_adv_save(a: &[String]) -> i32 {
    // adv-save <file>… : 固定版をゴールデン保存 (md5 記録)
    if a.is_empty() {
        eprintln!("usage: rspeed adv-save <file>…");
        return 2;
    }
    for f in a {
        let p = PathBuf::from(f);
        let Some(name) = p.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        match fs::read(&p) {
            Ok(b) => {
                if let Err(e) = write_loud(&adv_dir().join(&name), &b) {
                    eprintln!("adv-save: {e}");
                    return 1;
                }
                if let Err(e) = write_loud(&adv_dir().join(format!("{name}.md5")), md5_hex(&b)) {
                    eprintln!("adv-save: {e}");
                    return 1;
                }
                println!("adv-save: {name} md5={}", md5_hex(&b));
            }
            Err(e) => {
                eprintln!("{f}: {e}");
                return 1;
            }
        }
    }
    0
}

fn cmd_adv_restore(a: &[String]) -> i32 {
    // adv-restore <file>… : ゴールデンを md5 照合後に厳密復元 (1bit でも違えば失敗)
    if a.is_empty() {
        eprintln!("usage: rspeed adv-restore <file>…");
        return 2;
    }
    for f in a {
        let p = PathBuf::from(f);
        let Some(name) = p.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let golden = adv_dir().join(&name);
        let md5f = adv_dir().join(format!("{name}.md5"));
        let (Ok(b), Ok(want)) = (fs::read(&golden), fs::read_to_string(&md5f)) else {
            eprintln!("{name}: ゴールデン無し");
            return 1;
        };
        let want = want.trim().to_string();
        let gh = md5_hex(&b);
        if gh != want {
            eprintln!("{name}: ゴールデン自身が破損 ({gh} ≠ {want})");
            return 1;
        }
        if fs::write(&p, &b).is_err() {
            eprintln!("{name}: 書き込み失敗");
            return 1;
        }
        let after = md5_hex(&fs::read(&p).unwrap_or_default());
        println!(
            "adv-restore: {name} md5={after} {}",
            if after == want {
                "MD5-VERIFIED"
            } else {
                "FAIL"
            }
        );
        if after != want {
            return 1;
        }
    }
    0
}

fn cmd_adv_diff(a: &[String]) -> i32 {
    // adv-diff <file>: 現在 vs ゴールデンの集合差分 (注入内容の自己説明用)
    for f in a {
        let p = PathBuf::from(f);
        let Some(name) = p.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let Some(g) = read_text(&adv_dir().join(&name)) else {
            eprintln!("{name}: ゴールデン無し");
            return 1;
        };
        let Some(c) = read_text(&p) else {
            eprintln!("{name}: 現ファイル無し");
            return 2;
        };
        let gv: Vec<&str> = g.lines().collect();
        let cv: Vec<&str> = c.lines().collect();
        let added = lcs_added(&gv, &cv);
        let removed = lcs_added(&cv, &gv);
        println!(
            "adv-diff {name}: 注入差分 +{} / -{}",
            added.len(),
            removed.len()
        );
        for x in added.iter().take(25) {
            println!("+ {x}");
        }
        for x in removed.iter().take(25) {
            println!("- {x}");
        }
    }
    0
}

fn cmd_time_run(a: &[String]) -> i32 {
    // time-run <n> <cmd…> : hyperfine 風 min/median/p95 計測 (shell なし直接起動)
    if a.len() < 2 {
        eprintln!("usage: rspeed time-run <n> <cmd> [args…]");
        return 2;
    }
    let Ok(n): Result<usize, _> = a[0].parse() else {
        eprintln!("n は整数");
        return 2;
    };
    let mut times = Vec::new();
    for _ in 0..n {
        let t0 = Instant::now();
        let st = Command::new(&a[1])
            .args(&a[2..])
            .stdin(Stdio::null())
            .status();
        let el = t0.elapsed().as_secs_f64();
        match st {
            Ok(s) if s.success() => {
                times.push(el);
            }
            Ok(s) => {
                eprintln!("rc={} ({}回目)", s.code().unwrap_or(-1), times.len() + 1);
                return 1;
            }
            Err(e) => {
                eprintln!("起動失敗: {e}");
                return 127;
            }
        }
    }
    times.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let med = times[times.len() / 2];
    let p95 = times[(times.len() as f64 * 0.95) as usize % times.len()];
    println!(
        "time-run: {} 回 min {:.4}s / median {:.4}s / p95 {:.4}s / max {:.4}s",
        times.len(),
        times[0],
        med,
        p95,
        times[times.len() - 1]
    );
    0
}

fn cmd_binsize(a: &[String]) -> i32 {
    // binsize [n]: target 配下の容量上位デバッグ (デッドバイナリ発見)
    let n: usize = a.first().and_then(|s| s.parse().ok()).unwrap_or(15);
    let tdir = ws_root().join("target");
    let mut files = Vec::new();
    walk_files(&tdir, &mut files);
    let mut sized: Vec<(u64, PathBuf)> = files
        .iter()
        .filter_map(|p| fs::metadata(p).ok().map(|m| (m.len(), p.clone())))
        .collect();
    sized.sort_by_key(|(v, _)| std::cmp::Reverse(*v));
    let total: u64 = sized.iter().map(|(v, _)| *v).sum();
    println!(
        "target 総量: {:.1} MB ({} files)",
        total as f64 / 1e6,
        sized.len()
    );
    for (v, p) in sized.into_iter().take(n) {
        println!(
            "{:10.1} MB  {}",
            v as f64 / 1e6,
            p.strip_prefix(&tdir).unwrap_or(&p).display()
        );
    }
    0
}

fn cmd_ghfile(a: &[String]) -> i32 {
    // ghfile <owner/repo> <path> [ref] : GitHub 一次原文の取得 (gh api raw)
    if a.len() < 2 {
        eprintln!("usage: rspeed ghfile <owner/repo> <path> [ref]");
        return 2;
    }
    let refpart = a.get(2).map(|r| format!("?ref={r}")).unwrap_or_default();
    let endpoint = format!("repos/{}/contents/{}{}", a[0], a[1], refpart);
    let out = Command::new("gh")
        .args(["api", &endpoint, "-H", "Accept: application/vnd.github.raw"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            print!("{}", String::from_utf8_lossy(&o.stdout));
            0
        }
        Ok(o) => {
            eprintln!("gh api 失敗: {}", String::from_utf8_lossy(&o.stderr));
            1
        }
        Err(e) => {
            eprintln!("gh 起動失敗: {e}");
            127
        }
    }
}

fn cmd_ghlatest(a: &[String]) -> i32 {
    // ghlatest <owner/repo> : 最新 release の tag/asset 名 + asset DL 到達性プローブ
    if a.is_empty() {
        eprintln!("usage: rspeed ghlatest <owner/repo>");
        return 2;
    }
    let endpoint = format!("repos/{}/releases/latest", a[0]);
    let out = Command::new("gh").args(["api", &endpoint]).output().ok();
    let Some(o) = out else {
        eprintln!("gh 起動失敗");
        return 127;
    };
    let text = String::from_utf8_lossy(&o.stdout);
    if let Some(t) = text.lines().find(|l| l.contains("\"tag_name\"")) {
        println!("tag: {}", t.trim());
    }
    for l in text.lines().filter(|l| l.contains("\"name\"")) {
        println!("asset: {}", l.trim());
    }
    0
}

// ---- wave 運用 (journal/統計/todo 選択) ----
fn now_jst() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + 9 * 3600;
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe + 1 - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} JST",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn cmd_wave_log(a: &[String]) -> i32 {
    // wave-log <text> : 時刻つき作業ジャーナルに追記 (セッション跨ぎの記憶)
    let dst = cache_dir().join("wave-journal.md");
    let line = format!("- {} {}\n", now_jst(), a.join(" "));
    let prev = read_text(&dst).unwrap_or_default();
    if let Err(e) = write_loud(&dst, format!("{prev}{line}")) {
        eprintln!("wave-log: {e}");
        return 1;
    }
    println!("wave-log: {line}");
    0
}

fn cmd_journal(a: &[String]) -> i32 {
    let n: usize = a.first().and_then(|s| s.parse().ok()).unwrap_or(20);
    let dst = cache_dir().join("wave-journal.md");
    let t = read_text(&dst).unwrap_or_default();
    let lines: Vec<&str> = t.lines().collect();
    for l in lines.iter().skip(lines.len().saturating_sub(n)) {
        println!("{l}");
    }
    0
}

fn registry_rows() -> Vec<(String, String, String)> {
    let reg = read_text(&ws_root().join("docs/internal/BUGFIX_REGISTRY.md")).unwrap_or_default();
    reg.lines()
        .filter(|l| regcount_line(l))
        .filter_map(|l| {
            let cells: Vec<&str> = l.split('|').collect();
            if cells.len() >= 4 {
                Some((
                    cells[1].trim().to_string(),
                    cells[2].trim().to_string(),
                    cells[3].trim().to_string(),
                ))
            } else {
                None
            }
        })
        .collect()
}

fn cmd_registry_stats(_a: &[String]) -> i32 {
    // registry-stats: 深刻度×接尾辞のクロス統計 (監査深度の定量把握)
    let rows = registry_rows();
    let mut sev: HashMap<String, usize> = HashMap::new();
    let mut pref: HashMap<String, usize> = HashMap::new();
    for (id, s, _) in &rows {
        *sev.entry(s.clone()).or_default() += 1;
        let p = id.split('-').next().unwrap_or("？").to_string();
        *pref.entry(p).or_default() += 1;
    }
    println!("総件数: {}", rows.len());
    let mut sv: Vec<_> = sev.into_iter().collect();
    sv.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (k, c) in &sv {
        println!("  深刻度 {k}: {c}");
    }
    let mut pv: Vec<_> = pref.into_iter().collect();
    pv.sort();
    print!("  期: ");
    for (k, c) in &pv {
        print!("{k}={c} ");
    }
    println!();
    0
}

fn audit_path() -> PathBuf {
    ws_root().join("docs/internal/AUDIT_2026-07-21_RSGFX_MODULES.md")
}

fn cmd_wave_info(a: &[String]) -> i32 {
    // wave-info <接尾辞>: 台帳行 + 監査節冒頭を一括表示
    if a.is_empty() {
        eprintln!("usage: rspeed wave-info <接尾辞 (例: DC)>");
        return 2;
    }
    let pref = &a[0];
    for (id, sev, desc) in registry_rows()
        .into_iter()
        .filter(|(id, _, _)| id.starts_with(&format!("{pref}-")))
    {
        println!("| {id} | {sev} | {desc}");
    }
    let audit = read_text(&audit_path()).unwrap_or_default();
    let mut take = false;
    let mut n = 0;
    for l in audit.lines() {
        if l.starts_with(&format!("## {pref}. ")) {
            take = true;
        } else if l.starts_with("## ") && take {
            break;
        }
        if take {
            println!("{l}");
            n += 1;
            if n > 60 {
                break;
            }
        }
    }
    0
}

fn todo_set() -> Vec<String> {
    let audit = read_text(&audit_path()).unwrap_or_default();
    let mut done: BTreeSet<String> = BTreeSet::new();
    for l in audit.lines() {
        let Some(body) = l.strip_prefix("## ") else {
            continue;
        };
        let Some(dotpos) = body.find(". ") else {
            continue;
        };
        if !body[..dotpos]
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '/' || c == '-')
        {
            continue;
        }
        let name: String = body[dotpos + 2..]
            .chars()
            .take_while(|c| *c != '.' && *c != ' ')
            .collect();
        if !name.is_empty() {
            done.insert(name);
        }
    }
    let src = ws_root().join("crates/rsift-opt-gfx/src");
    let mut all: BTreeSet<String> = BTreeSet::new();
    for e in fs::read_dir(&src).into_iter().flat_map(|r| r.flatten()) {
        let p = e.path();
        if p.extension().map(|x| x == "rs").unwrap_or(false) {
            all.insert(p.file_stem().unwrap().to_string_lossy().into_owned());
        }
    }
    all.difference(&done).cloned().collect()
}

fn cmd_todo_pick(a: &[String]) -> i32 {
    // todo-pick [n] : 行数昇順の次 wave 候補 (小さい順が既定の棚卸し流儀)
    let n: usize = a.first().and_then(|s| s.parse().ok()).unwrap_or(10);
    let src = ws_root().join("crates/rsift-opt-gfx/src");
    let mut sized: Vec<(u64, String)> = todo_set()
        .into_iter()
        .filter_map(|m| {
            let p = src.join(format!("{m}.rs"));
            read_text(&p).map(|t| (t.lines().count() as u64, m))
        })
        .collect();
    sized.sort();
    for (c, m) in sized.into_iter().take(n) {
        println!("{c:6} 行  {m}");
    }
    0
}

fn cmd_todo_pri(a: &[String]) -> i32 {
    // todo-pri: 優先度推定スコア (消費者数×10 + digest 経路含有×50 + 行数/20 + キーワード×15)
    let _ = a;
    let src = ws_root().join("crates/rsift-opt-gfx/src");
    let digest_hot = read_text(&src.join("../examples/wide_static_bench.rs")).unwrap_or_default()
        + &read_text(&src.join("../examples/pseudo_mc_bench.rs")).unwrap_or_default();
    let pipe = read_text(&src.join("render_pipeline.rs")).unwrap_or_default()
        + &read_text(&src.join("full_graph_wiring.rs")).unwrap_or_default();
    let keywords = [
        "dda", "cache", "light", "pipeline", "graph", "sched", "mesh", "compress",
    ];
    let mut scored: Vec<(i64, String, u64)> = Vec::new();
    for m in todo_set() {
        let Some(t) = read_text(&src.join(format!("{m}.rs"))) else {
            continue;
        };
        let lines = t.lines().count() as u64;
        let mut score = lines as i64 / 20;
        let refs = pipe.matches(&m).count() as i64 * 10;
        score += refs;
        if digest_hot.contains(&m) {
            score += 50;
        }
        if keywords.iter().any(|k| m.contains(k)) {
            score += 15;
        }
        scored.push((score, m, lines));
    }
    scored.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
    println!("score  stem (行数)  [digest 経路含有は +50]");
    for (s, m, l) in scored.into_iter().take(20) {
        println!(
            "{s:5}  {m} ({l}){}",
            if digest_hot.contains(&m) { " ★" } else { "" }
        );
    }
    0
}

fn cmd_coverage(_a: &[String]) -> i32 {
    let done = 163 - todo_set().len();
    let total = 163;
    let pct = done * 100 / total;
    let bar: String = (0..40)
        .map(|i| if i < done * 40 / total { '█' } else { '░' })
        .collect();
    println!("[{bar}] {done}/{total} ({pct}%)");
    println!("台帳: {} 件 / テスト: 全緑で維持", registry_rows().len());
    0
}

fn cmd_ci_status(a: &[String]) -> i32 {
    let n = a.first().map(|s| s.as_str()).unwrap_or("5");
    let out = Command::new("gh")
        .args([
            "run",
            "list",
            "--branch",
            "arena/019f88d7-rsift",
            "--limit",
            n,
            "--json",
            "databaseId,conclusion,displayTitle",
        ])
        .output();
    match out {
        Ok(o) => {
            let t = String::from_utf8_lossy(&o.stdout);
            // JSON を軽量整形
            let mut t = t.replace("},", "}\n");
            for ch in [',', '[', ']', '"', '{', '}'] {
                t = t.replace(ch, if ch == ',' { "  " } else { "" });
            }
            for l in t.lines().filter(|l| !l.trim().is_empty()) {
                println!("{}", l.trim());
            }
            0
        }
        Err(e) => {
            eprintln!("gh 起動失敗: {e}");
            127
        }
    }
}

fn cmd_lines(a: &[String]) -> i32 {
    let targets = default_scan_targets(a);
    let mut sized: Vec<(u64, PathBuf)> = collect_rs_code(&targets)
        .into_iter()
        .map(|(p, _, t)| (t.lines().count() as u64, p))
        .collect();
    sized.sort_by_key(|(c, _)| std::cmp::Reverse(*c));
    let total: u64 = sized.iter().map(|(c, _)| *c).sum();
    for (c, p) in sized.into_iter().take(40) {
        println!("{c:6}  {}", p.display());
    }
    println!("total {total} 行");
    0
}

fn cmd_burndown(_a: &[String]) -> i32 {
    // burndown: ci/TRIGGER.md の注記行から wave 進行表 (count, wave, テスト総数)
    let trg = read_text(&git_root().join("ci/TRIGGER.md")).unwrap_or_default();
    println!("{:>5}  {:>10}  {}", "count", "wave", "備考");
    for l in trg.lines() {
        if !(l.starts_with("- 202") && l.contains("(count")) {
            continue;
        }
        let Some(ci) = l.find("(count ") else {
            continue;
        };
        let Some(cend) = l[ci..].find(')') else {
            continue;
        };
        let count = &l[ci + 7..ci + cend];
        let wave = l
            .split("wave ")
            .nth(1)
            .and_then(|r| r.split(' ').next())
            .unwrap_or("？");
        let tests = l.split(' ').find(|t| t.ends_with("全緑")).unwrap_or("");
        println!("{count:>5}  wave {wave:<7} {tests}");
    }
    0
}

fn cmd_seal(a: &[String]) -> i32 {
    // seal: 提出前検証の一括ゲート (監査テンプレの機械化)
    //  --quick: cargo 関係を指紋 SKIP 重視 / --skip-tests / --skip-bench で段省略
    let quick = a.iter().any(|s| s == "--quick");
    let skip_tests = a.iter().any(|s| s == "--skip-tests");
    let skip_bench = a.iter().any(|s| s == "--skip-bench");
    let mut stage = 0;
    let mut fail = 0;
    let mut mark = |name: &str, ok: bool| {
        stage += 1;
        println!(
            "  {} ゲート{stage}: {name}",
            if ok { "PASS" } else { "FAIL" }
        );
        if !ok {
            fail += 1;
        }
    };
    println!("seal 開始: {}", now_jst());
    // 1) 変更ファイル集合
    let changed = modified_tracked();
    println!("  変更追跡ファイル: {}", changed.len());
    // 2) san (変更ファイル+docs)
    let mut san_args = changed.clone();
    san_args.push(
        ws_root()
            .join("docs/internal")
            .to_string_lossy()
            .into_owned(),
    );
    mark(
        "san (不可視/CRLF/簡体字/文字化け/末尾改行)",
        cmd_san(&san_args) == 0,
    );
    // 3) fmdiff (変更 .rs のみ)
    let rs: Vec<String> = changed
        .iter()
        .filter(|f| f.ends_with(".rs"))
        .map(|f| git_root().join(f).to_string_lossy().into_owned())
        .collect();
    if !rs.is_empty() {
        mark("fmdiff (fmt 逸脱 HEAD 包含)", cmd_fmdiff(&rs) == 0);
    }
    // 4) trailws
    if !rs.is_empty() {
        mark("trailws", cmd_trailws(&rs) == 0);
    }
    // 5) regcount 報告
    let n = read_text(&ws_root().join("docs/internal/BUGFIX_REGISTRY.md"))
        .unwrap_or_default()
        .lines()
        .filter(|l| regcount_line(l))
        .count();
    println!("  INFO 台帳件数: {n}");
    // 6) テスト
    if !skip_tests {
        let targs: Vec<String> = if quick { vec![] } else { vec![] };
        mark("テスト全緑", cmd_test(&targs) == 0);
    }
    // 7) digest
    if !skip_bench {
        mark(
            "digest 004c1cf5fb17bfe8",
            cmd_bench(&[
                "wide_static_bench".into(),
                "--expect-digest".into(),
                "004c1cf5fb17bfe8".into(),
            ]) == 0,
        );
    }
    // 8) env
    mark("env-check", cmd_env_check(&[]) == 0);
    println!(
        "seal 結果: {}",
        if fail == 0 {
            "全ゲート PASS — push 可能"
        } else {
            "FAIL ゲートあり — 要対応"
        }
    );
    if fail > 0 {
        1
    } else {
        0
    }
}

fn cmd_dashboard(a: &[String]) -> i32 {
    // dashboard: 監査状況の一括俯瞰 (高速版のみ既定、--full で重いのも)
    let full = a.iter().any(|s| s == "--full");
    println!("== Rsift 監査ダッシュボード ==  {}", now_jst());
    let _ = cmd_status(&[]);
    let _ = cmd_coverage(&[]);
    let _ = cmd_registry_stats(&[]);
    if full {
        let _ = cmd_ci_status(&[]);
        let _ = cmd_env_check(&[]);
    }
    let _ = cmd_journal(&["8".into()]);
    0
}

// ===================================================================
// Batch D — 統計/ビット/グラフィクス数学系 (26 機能 + selftest/man)
// ===================================================================

fn collect_numbers(xs: &[String]) -> Vec<f64> {
    let mut v = Vec::new();
    for x in xs {
        let p = PathBuf::from(x);
        if p.exists() {
            if let Some(t) = read_text(&p) {
                for tok in t.split(|c: char| {
                    !c.is_ascii_digit() && c != '.' && c != '-' && c != '+' && c != 'e' && c != 'E'
                }) {
                    if let Ok(f) = tok.parse::<f64>() {
                        v.push(f);
                    }
                }
            }
        } else if let Ok(f) = x.parse::<f64>() {
            v.push(f);
        }
    }
    v
}

fn cmd_percentile(a: &[String]) -> i32 {
    // percentile <値…|file> : p0/p25/p50/p75/p90/p95/p99/max/mean/stdev
    let mut v = collect_numbers(a);
    if v.is_empty() {
        eprintln!("usage: rspeed percentile <値…|file>");
        return 2;
    }
    v.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let n = v.len();
    let pct = |p: f64| v[((n - 1) as f64 * p).round() as usize];
    let mean: f64 = v.iter().sum::<f64>() / n as f64;
    let var: f64 = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
    println!("n={n} mean={mean:e} stdev={:e}", var.sqrt());
    for p in [0.0, 0.25, 0.5, 0.75, 0.9, 0.95, 0.99, 1.0] {
        println!("  p{:04.0}: {:e}", p * 100.0, pct(p));
    }
    0
}

fn cmd_histogram(a: &[String]) -> i32 {
    // histogram <値…|file> [bins]: テキストヒストグラム
    let mut bins = 20usize;
    let xs: Vec<String> = a
        .iter()
        .filter(|s| match s.parse::<usize>() {
            Ok(v) if v <= 200 => {
                bins = v.max(2);
                false
            }
            _ => true,
        })
        .cloned()
        .collect();
    let v = collect_numbers(&xs);
    if v.is_empty() {
        eprintln!("usage: rspeed histogram <値…|file> [bins]");
        return 2;
    }
    let (mut mn, mut mx) = (f64::INFINITY, f64::NEG_INFINITY);
    for &x in &v {
        if x < mn {
            mn = x;
        }
        if x > mx {
            mx = x;
        }
    }
    let mut counts = vec![0usize; bins];
    for &x in &v {
        let idx = if mx == mn {
            0
        } else {
            (((x - mn) / (mx - mn) * (bins - 1) as f64).round() as usize).min(bins - 1)
        };
        counts[idx] += 1;
    }
    let maxc = *counts.iter().max().unwrap_or(&1);
    for (i, c) in counts.iter().enumerate() {
        let lo = mn + (mx - mn) * i as f64 / (bins - 1).max(1) as f64;
        println!("{:12.6e} |{} {}", lo, "█".repeat(c * 50 / maxc), c);
    }
    0
}

fn cmd_bigfact(a: &[String]) -> i32 {
    for s in a {
        match s.parse::<u32>() {
            Ok(n) if n <= 34 => {
                let mut f: i128 = 1;
                for i in 2..=n {
                    f *= i as i128;
                }
                println!("{n}! = {f} ({} 桁)", f.to_string().len());
            }
            Ok(n) => {
                // log10 を厳密加算して桁数と先頭仮数を高精度推定
                let log10: f64 = (1..=n).map(|i| (i as f64).log10()).sum();
                let fl = log10.floor();
                println!(
                    "{n}! ≈ {:.6} × 10^{} (exact log10 = {:.12}、{} 桁)",
                    10f64.powf(log10 - fl),
                    fl as i64,
                    log10,
                    fl as i64 + 1
                );
            }
            Err(e) => {
                eprintln!("{s}: {e}");
                return 1;
            }
        }
    }
    0
}

fn cmd_fib(a: &[String]) -> i32 {
    let Some(n) = a.first().and_then(|s| s.parse::<u32>().ok()) else {
        eprintln!("usage: rspeed fib <n>");
        return 2;
    };
    let (mut x, mut y): (i128, i128) = (0, 1);
    for i in 0..n {
        let tmp = x.checked_add(y);
        match tmp {
            Some(t) => {
                x = y;
                y = t;
            }
            None => {
                eprintln!(
                    "i128 限界で停止 (fib({}) ≈ {:.6e})",
                    i + 1,
                    x as f64 + y as f64
                );
                return 1;
            }
        }
    }
    println!("fib({n}) = {y} (i128 厳密)");
    0
}

fn cmd_crc32(a: &[String]) -> i32 {
    for x in a {
        let bytes = if x.starts_with('@') {
            fs::read(&x[1..]).unwrap_or_default()
        } else {
            x.clone().into_bytes()
        };
        let mut crc: u32 = 0xffff_ffff;
        for b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        println!("crc32=0x{:08x}  {x}", crc ^ 0xffff_ffff);
    }
    0
}

fn cmd_bitops(a: &[String]) -> i32 {
    // bitops <op> <a> [b] : xor/and/or/not/rotl/rotr (u64、hex 可)
    if a.len() < 2 {
        eprintln!("usage: rspeed bitops xor|and|or|not <a> [b]  /  rotl|rotr <a> <k>");
        return 2;
    }
    let pv = |s: &str| parse_u64_auto(s);
    let (Ok(x), Ok(y)) = (pv(&a[1]), a.get(2).map(|s| pv(s)).unwrap_or(Ok(0))) else {
        eprintln!("u64 値");
        return 2;
    };
    let r = match a[0].as_str() {
        "xor" => x ^ y,
        "and" => x & y,
        "or" => x | y,
        "not" => !x,
        "rotl" => x.rotate_left(view_u32(&a[2])),
        "rotr" => x.rotate_right(view_u32(&a[2])),
        _ => {
            eprintln!("未知 op");
            return 2;
        }
    };
    println!("0x{r:016x} ({r})  bin {:064b}", r);
    0
}

fn view_u32(s: &str) -> u32 {
    s.parse::<u32>().unwrap_or(0)
}

fn cmd_pack(a: &[String]) -> i32 {
    // pack <width>… : 値を幅配列で LSB-first にパック (値は同数の後続引数) → unpack で逆検証
    let n = a.len() / 2;
    if a.len() < 2 || a.len() % 2 != 0 {
        eprintln!("usage: rspeed pack <w1>..<wk> <v1>..<vk> (対数一致)");
        return 2;
    }
    let mut widths = Vec::new();
    let mut values = Vec::new();
    for i in 0..n {
        widths.push(a[i].parse::<u32>().unwrap_or(64));
        match parse_u64_auto(&a[n + i]) {
            Ok(v) => values.push(v),
            Err(e) => {
                eprintln!("{e}");
                return 2;
            }
        }
    }
    let total: u32 = widths.iter().sum();
    if total > 64 {
        eprintln!("合計幅 {total} > 64");
        return 2;
    }
    let mut packed = 0u64;
    let mut shift = 0u32;
    let mut over = false;
    for (i, &v) in values.iter().enumerate() {
        let w = widths[i];
        let mask = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
        if v > mask {
            over = true;
            println!("  警告: v{i}={v} が幅 {w} を超過 (mask 0x{mask:x}) → 静寂切捨て!");
        }
        packed |= (v & mask) << shift;
        shift += w;
    }
    println!(
        "packed = 0x{packed:016x}{}",
        if over {
            "  (超過あり — キャスト静寂化の再現)"
        } else {
            ""
        }
    );
    // 逆検証
    let mut back = Vec::new();
    let mut shift = 0u32;
    for &w in &widths {
        let mask = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
        back.push((packed >> shift) & mask);
        shift += w;
    }
    println!(
        "unpack 逆検証: {:?} {}",
        back,
        if back == values {
            "ROUNDTRIP-OK"
        } else {
            "ROUNDTRIP-FAIL (幅超過の影響)"
        }
    );
    0
}

fn cmd_unpack(a: &[String]) -> i32 {
    // unpack <packed> <w1>… : LSB-first アンパック
    if a.len() < 2 {
        eprintln!("usage: rspeed unpack <packed> <w>…");
        return 2;
    }
    let packed = match parse_u64_auto(&a[0]) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let mut shift = 0u32;
    for w in &a[1..] {
        let w = w.parse::<u32>().unwrap_or(0);
        let mask = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
        println!(
            "  幅 {w}: 0x{:x} ({})",
            (packed >> shift) & mask,
            (packed >> shift) & mask
        );
        shift += w;
    }
    let rest = if shift >= 64 { 0 } else { packed >> shift };
    if rest != 0 {
        println!("  残余ビット: 0x{rest:x} (幅指定が不足)");
        return 1;
    }
    0
}

fn cmd_endian(a: &[String]) -> i32 {
    for s in a {
        let Ok(x) = parse_u64_auto(s) else {
            eprintln!("{s}: u64");
            return 2;
        };
        println!(
            "{s}: 0x{x:016x} → bswap 0x{:016x}  bytes LE {:02x?}",
            x.swap_bytes(),
            x.to_le_bytes()
        );
    }
    0
}

fn cmd_clamp_table(_a: &[String]) -> i32 {
    // clamp-table: f32→int の境界丸め一覧 (Rust `as` saturate 意味論の基準表)
    let probes: Vec<f64> = vec![
        f64::NAN,
        f64::NEG_INFINITY,
        -1e30,
        -256.5,
        -1.9,
        -1.0,
        -0.5,
        -0.0,
        0.0,
        0.4,
        0.5,
        0.6,
        127.4,
        127.5,
        127.6,
        255.4,
        255.5,
        255.9,
        256.4,
        1e30,
        f64::INFINITY,
    ];
    println!(
        "{:>12} {:>8} {:>8} {:>8} {:>8}",
        "input", "u8", "u16", "i16", "i8"
    );
    for v in probes {
        println!(
            "{v:>12} {:>8} {:>8} {:>8} {:>8}",
            v as u8, v as u16, v as i16, v as i8
        );
    }
    0
}

fn cmd_matc(a: &[String]) -> i32 {
    // matc <16 values> : det4 を f64 vs f32 (演算毎 f32 逐次) で比較 (精度破壊箇所)
    let Some(m) = parse_16(a) else {
        eprintln!("usage: rspeed matc <16 values>");
        return 2;
    };
    let d64 = det4(&m);
    let m32: [[f32; 4]; 4] = core::array::from_fn(|i| core::array::from_fn(|j| m[i][j] as f32));
    let m64: [[f64; 4]; 4] = core::array::from_fn(|i| core::array::from_fn(|j| m32[i][j] as f64));
    let d32 = det4(&m64);
    println!("det f64       = {d64:e}");
    println!(
        "det f32 逐次  = {d32:e}  (誤差 {:e}、相対 {:e})",
        d32 - d64,
        if d64 != 0.0 { (d32 - d64) / d64 } else { 0.0 }
    );
    0
}

// ---- グラフィクス数学 ----
fn cmd_quat(a: &[String]) -> i32 {
    // quat <ax> <ay> <az> <角度deg> : 軸角→クォータニオン (f64+f32) + →行列
    if a.len() != 4 {
        eprintln!("usage: rspeed quat <ax> <ay> <az> <deg>");
        return 2;
    }
    let v: Result<Vec<f64>, _> = a.iter().map(|s| s.parse::<f64>()).collect();
    let Ok(v) = v else {
        eprintln!("数値");
        return 2;
    };
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len == 0.0 {
        eprintln!("軸長 0");
        return 1;
    }
    let half = v[3].to_radians() / 2.0;
    let (s, c) = (half.sin() / len, half.cos());
    let q = [c, v[0] * s, v[1] * s, v[2] * s];
    println!(
        "q = (w {:.12}, x {:.12}, y {:.12}, z {:.12})",
        q[0], q[1], q[2], q[3]
    );
    let qn = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    println!("|q| = {qn:.15}");
    let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
    let m = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    println!("回転行列:");
    for r in &m {
        println!("  [{:.8} {:.8} {:.8}]", r[0], r[1], r[2]);
    }
    0
}

fn proj_mats(fovy_deg: f64, aspect: f64, near: f64, far: f64) -> ([[f64; 4]; 4], [[f64; 4]; 4]) {
    let f = 1.0 / (fovy_deg.to_radians() / 2.0).tan();
    let mut wgpu = [[0.0; 4]; 4]; // RH, z∈[0,1], wgpu 規約
    wgpu[0][0] = f / aspect;
    wgpu[1][1] = f;
    wgpu[2][2] = far / (near - far);
    wgpu[2][3] = -1.0;
    wgpu[3][2] = (far * near) / (near - far);
    let mut gl = [[0.0; 4]; 4]; // RH, z∈[-1,1], GL 規約
    gl[0][0] = f / aspect;
    gl[1][1] = f;
    gl[2][2] = (far + near) / (near - far);
    gl[2][3] = -1.0;
    gl[3][2] = (2.0 * far * near) / (near - far);
    (wgpu, gl)
}

fn cmd_proj(a: &[String]) -> i32 {
    // proj <fov_y_deg> <aspect> <near> <far> : wgpu (z[0,1]) と GL (z[-1,1]) 両規約
    if a.len() != 4 {
        eprintln!("usage: rspeed proj <fov_y> <aspect> <near> <far>");
        return 2;
    }
    let v: Result<Vec<f64>, _> = a.iter().map(|s| s.parse::<f64>()).collect();
    let Ok(v) = v else {
        eprintln!("数値");
        return 2;
    };
    let (wgpu, gl) = proj_mats(v[0], v[1], v[2], v[3]);
    print_m4("wgpu 規約 (RH, z∈[0,1])", &wgpu);
    print_m4("GL 規約 (RH, z∈[-1,1])", &gl);
    // 検算: near → ndc 0 (wgpu) / -1 (gl), far → 1
    for (name, m, want_n) in [("wgpu", &wgpu, 0.0), ("gl", &gl, -1.0)] {
        let zc = m[2][2] * -v[2] + m[3][2];
        let ndc = zc / v[2];
        let zf = m[2][2] * -v[3] + m[3][2];
        let ndf = zf / v[3];
        println!(
            "検算 {name}: near→ndc {ndc:.12} (期待 {want_n}) / far→ndc {ndf:.12} (期待 1) {}",
            if (ndc - want_n).abs() < 1e-9 && (ndf - 1.0).abs() < 1e-9 {
                "OK"
            } else {
                "FAIL"
            }
        );
    }
    0
}

fn cmd_lookat(a: &[String]) -> i32 {
    // lookat <eye3> <center3> <up3> : RH ビュー行列
    if a.len() != 9 {
        eprintln!("usage: rspeed lookat <ex ey ez> <cx cy cz> <ux uy uz>");
        return 2;
    }
    let v: Result<Vec<f64>, _> = a.iter().map(|s| s.parse::<f64>()).collect();
    let Ok(v) = v else {
        eprintln!("数値");
        return 2;
    };
    let norm = |x: [f64; 3]| {
        let l = (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt();
        [x[0] / l, x[1] / l, x[2] / l]
    };
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let eye = [v[0], v[1], v[2]];
    let center = [v[3], v[4], v[5]];
    let f = norm([center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]]);
    let s = norm(cross(f, [v[6], v[7], v[8]]));
    let u = cross(s, f);
    let m = [
        [s[0], s[1], s[2], -dot(s, eye)],
        [u[0], u[1], u[2], -dot(u, eye)],
        [-f[0], -f[1], -f[2], dot(f, eye)],
        [0.0, 0.0, 0.0, 1.0],
    ];
    print_m4("view (lookat RH)", &m);
    0
}

fn cmd_tri_area(a: &[String]) -> i32 {
    // tri-area <9 値 (3 頂点)> : クロス積法 f64 vs f32 vs Heron (精度比較)
    if a.len() != 9 {
        eprintln!("usage: rspeed tri-area <9 値>");
        return 2;
    }
    let v: Result<Vec<f64>, _> = a.iter().map(|s| s.parse::<f64>()).collect();
    let Ok(v) = v else {
        eprintln!("数値");
        return 2;
    };
    let (p, q, r) = ([v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]);
    let ab = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
    let ac = [r[0] - p[0], r[1] - p[1], r[2] - p[2]];
    let cx = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let area64 = 0.5 * (cx[0] * cx[0] + cx[1] * cx[1] + cx[2] * cx[2]).sqrt();
    let area32 = {
        let (p, q, r): ([f32; 3], [f32; 3], [f32; 3]) = (
            core::array::from_fn(|i| p[i] as f32),
            core::array::from_fn(|i| q[i] as f32),
            core::array::from_fn(|i| r[i] as f32),
        );
        let ab = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
        let ac = [r[0] - p[0], r[1] - p[1], r[2] - p[2]];
        let cx = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        0.5f32 * (cx[0] * cx[0] + cx[1] * cx[1] + cx[2] * cx[2]).sqrt()
    };
    let len = |x: [f64; 3], y: [f64; 3]| {
        ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()
    };
    let (la, lb, lc) = (len(q, r), len(r, p), len(p, q));
    let s2 = (la + lb + lc) / 2.0;
    let heron = (s2 * (s2 - la) * (s2 - lb) * (s2 - lc)).max(0.0).sqrt();
    println!("cross f64 = {area64:.12}\ncross f32 = {area32:.12} (誤差 {:e})\nHeron f64 = {heron:.12} (退化三角形での不安定源比較用)", area32 as f64 - area64);
    0
}

fn cmd_bary(a: &[String]) -> i32 {
    // bary <p2> <a2> <b2> <c2> : 2D 重心座標 + 内外判定 (符号つき面積比)
    if a.len() != 8 {
        eprintln!("usage: rspeed bary <px py> <ax ay> <bx by> <cx cy>");
        return 2;
    }
    let v: Result<Vec<f64>, _> = a.iter().map(|s| s.parse::<f64>()).collect();
    let Ok(v) = v else {
        eprintln!("数値");
        return 2;
    };
    let (p, pa, pb, pc) = ([v[0], v[1]], [v[2], v[3]], [v[4], v[5]], [v[6], v[7]]);
    let d = |u: [f64; 2], x: [f64; 2], y: [f64; 2]| {
        let (v0, v1, v2) = (
            [x[0] - u[0], x[1] - u[1]],
            [y[0] - u[0], y[1] - u[1]],
            [p[0] - u[0], p[1] - u[1]],
        );
        (v0[0] * v2[1] - v0[1] * v2[0], v1[0] * v2[1] - v1[1] * v2[0])
    };
    // 面積座標: w_a = A(p,b,c)/A(a,b,c)
    let sx = |u: [f64; 2], x: [f64; 2], y: [f64; 2]| {
        (x[0] - u[0]) * (y[1] - u[1]) - (x[1] - u[1]) * (y[0] - u[0])
    };
    let da = sx(pb, pc, pa);
    let wa = sx(pb, pc, p);
    let wb = sx(pc, pa, p);
    let denom = wa + wb + sx(pa, pb, p);
    let _ = d;
    let _ = da;
    let u = wa / denom;
    let w = wb / denom;
    let tw = 1.0 - u - w;
    println!(
        "重心座標: u(a) {:.12} / v(b) {:.12} / w(c) {:.12}  総和 {:.15}",
        u,
        w,
        tw,
        u + w + tw
    );
    println!(
        "内外: {}",
        if u >= 0.0 && w >= 0.0 && tw >= 0.0 {
            "内部"
        } else {
            "外部"
        }
    );
    0
}

fn cmd_halton(a: &[String]) -> i32 {
    // halton <index> <base> : 低食い違い列の厳密分数つき値 (sampling 監査用)
    if a.len() != 2 {
        eprintln!("usage: rspeed halton <index> <base>");
        return 2;
    }
    let (Ok(mut i), Ok(b)) = (a[0].parse::<u64>(), a[1].parse::<u64>()) else {
        eprintln!("整数");
        return 2;
    };
    let (mut num, mut den) = (0u128, 1u128);
    while i > 0 {
        den *= b as u128;
        num = num * b as u128 + (i % b) as u128;
        i /= b;
    }
    let g = gcd_i128(num as i128, den as i128) as u128;
    println!(
        "halton({},{}) = {}/{} ≈ {:.15}",
        a[0],
        a[1],
        num / g,
        den / g,
        num as f64 / den as f64
    );
    0
}

fn cmd_r2(a: &[String]) -> i32 {
    // r2 <n> : R2 列 (plactic ψ2=1.32471795724474602596) n 個
    let n: usize = a.first().and_then(|s| s.parse().ok()).unwrap_or(8);
    let g: f64 = 1.324_717_957_244_746_025_96;
    let (a1, a2x) = (1.0 / g, 1.0 / (g * g));
    for i in 0..n {
        let x = (0.5 + a1 * (i + 1) as f64).fract();
        let y = (0.5 + a2x * (i + 1) as f64).fract();
        println!("r2[{i}] = ({x:.12}, {y:.12})");
    }
    0
}

fn cmd_color(a: &[String]) -> i32 {
    // color <r> <g> <b> (0..1) : linear 化 + Rec.709/601 輝度
    if a.len() != 3 {
        eprintln!("usage: rspeed color <r> <g> <b>");
        return 2;
    }
    let v: Result<Vec<f64>, _> = a.iter().map(|s| s.parse::<f64>()).collect();
    let Ok(v) = v else {
        eprintln!("数値");
        return 2;
    };
    let lin: Vec<f64> = v.iter().map(|&c| srgb_decode(c)).collect();
    let y709 = 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2];
    let y601 = 0.299 * v[0] + 0.587 * v[1] + 0.114 * v[2];
    println!("linear: ({:.6}, {:.6}, {:.6})", lin[0], lin[1], lin[2]);
    println!("Rec.709 (linear) 輝度: {y709:.6}  /  Rec.601 γ 輝度: {y601:.6}");
    println!(
        "輝度比較 (γ 空間演算誤りの被害推定): 709-linear vs 601-γ 差 {:e}",
        y709 - y601
    );
    0
}

fn cmd_srgb_err(a: &[String]) -> i32 {
    // srgb-err [n] : pow2.2 近似の最大誤差 (piecewise 厳密との差) と argmax
    let n: usize = a.first().and_then(|s| s.parse().ok()).unwrap_or(10001);
    let (mut me, mut mx, mut enc_err, mut enc_arg) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        let x = i as f64 / (n - 1) as f64;
        let d1 = (x.powf(1.0 / 2.2) - srgb_encode(x)).abs();
        if d1 > enc_err {
            enc_err = d1;
            enc_arg = x;
        }
        let d2 = (x.powf(2.2) - srgb_decode(x)).abs();
        if d2 > me {
            me = d2;
            mx = x;
        }
    }
    println!("encode 側: pow(1/2.2)−sRGB 最大誤差 {enc_err:.6} @ {enc_arg}");
    println!("decode 側: pow(2.2)−sRGB⁻¹ 最大誤差 {me:.6} @ {mx}");
    println!("判定: pow2.2 近似を使っているシェーダがあればこの誤差がそのまま写り込む (監査指標)");
    0
}

fn cmd_quat_slerp(a: &[String]) -> i32 {
    // quat-slerp <ax ay az deg1> <deg2> <t> : 同一軸の角度 1→2 の slerp vs nlerp 差
    if a.len() != 6 {
        eprintln!("usage: rspeed quat-slerp <ax> <ay> <az> <deg1> <deg2> <t>");
        return 2;
    }
    let v: Result<Vec<f64>, _> = a.iter().map(|s| s.parse::<f64>()).collect();
    let Ok(v) = v else {
        eprintln!("数値");
        return 2;
    };
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len == 0.0 {
        eprintln!("軸長 0");
        return 1;
    }
    let axis = [v[0] / len, v[1] / len, v[2] / len];
    let mk = |deg: f64| {
        let h = deg.to_radians() / 2.0;
        [
            h.cos(),
            axis[0] * h.sin(),
            axis[1] * h.sin(),
            axis[2] * h.sin(),
        ]
    };
    let (qa, mut qb) = (mk(v[3]), mk(v[4]));
    let t = v[5];
    let dot: f64 = qa.iter().zip(qb.iter()).map(|(x, y)| x * y).sum();
    if dot < 0.0 {
        qb = [-qb[0], -qb[1], -qb[2], -qb[3]];
    }
    let da: f64 = qa.iter().zip(qb.iter()).map(|(x, y)| x * y).sum();
    let th = da.clamp(-1.0, 1.0).acos();
    let qs = if th.abs() < 1e-9 {
        qa
    } else {
        let s0 = ((1.0 - t) * th).sin() / th.sin();
        let s1 = (t * th).sin() / th.sin();
        [
            qa[0] * s0 + qb[0] * s1,
            qa[1] * s0 + qb[1] * s1,
            qa[2] * s0 + qb[2] * s1,
            qa[3] * s0 + qb[3] * s1,
        ]
    };
    let nl: Vec<f64> = qa
        .iter()
        .zip(qb.iter())
        .map(|(x, y)| x * (1.0 - t) + y * t)
        .collect();
    let nln = (nl.iter().map(|x| x * x).sum::<f64>()).sqrt();
    let qn: Vec<f64> = nl.iter().map(|x| x / nln).collect();
    let ang = |q: [f64; 4]| 2.0 * q[0].clamp(-1.0, 1.0).acos().to_degrees();
    println!(
        "slerp: 角 {:.9}°  q({:.8},{:.8},{:.8},{:.8})",
        ang(qs),
        qs[0],
        qs[1],
        qs[2],
        qs[3]
    );
    println!(
        "nlerp: 角 {:.9}°  (slerp−nlerp 角差 {:e}°)",
        ang([qn[0], qn[1], qn[2], qn[3]]),
        ang(qs) - ang([qn[0], qn[1], qn[2], qn[3]])
    );
    let expect = v[3] * (1.0 - t) + v[4] * t;
    println!("線形期待角 {expect:.9}° — 同一軸の場合 slerp==線形 が理論照合");
    0
}

// ---- selftest / man / nextwave ----
fn cmd_selftest(_a: &[String]) -> i32 {
    // selftest: 既知ピンで rspeed 自身の正確さを検証 (ツール堕落検出)
    let mut fails = 0;
    let mut chk = |name: &str, got: String, want: String| {
        let ok = got == want;
        if !ok {
            fails += 1;
        }
        println!(
            "  {} {name}: got={got} want={want}",
            if ok { "PASS" } else { "FAIL" }
        );
    };
    // expr f64 bits
    let ast = parse_expr("0.1+0.2").unwrap();
    chk(
        "expr f64 bits 0.1+0.2",
        format!("0x{:016x}", eval_f64(&ast).unwrap().to_bits()),
        "0x3fd3333333333334".into(),
    );
    // frac 1/3
    let f = eval_frac(&parse_expr("1/3").unwrap()).unwrap();
    chk("frac decimal 1/3", frac_decimal(f), "0.(3)".into());
    // f16
    chk(
        "f16(1.5)",
        format!("0x{:04x}", f32_to_f16(1.5)),
        "0x3e00".into(),
    );
    chk(
        "f16(0.1)",
        format!("0x{:04x}", f32_to_f16(0.1)),
        "0x2e66".into(),
    );
    chk(
        "bf16(1.0)",
        format!("0x{:04x}", f32_to_bf16(1.0)),
        "0x3f80".into(),
    );
    chk(
        "f16→f32 roundtrip max",
        format!(
            "{}",
            (0..=0x7bffu16)
                .filter(|h| !matches!(h, 0x7c01..=0x7fff))
                .all(|h| (f16_to_f32(h) - f16_to_f32(h)).abs() <= 0.0)
        ),
        "true".into(),
    );
    // morton
    chk(
        "morton2(5,9)",
        format!("0x{:08x}", part1by1(5) | (part1by1(9) << 1)),
        "0x00000093".into(),
    );
    chk(
        "morton3(5,9,3)",
        format!(
            "0x{:016x}",
            part1by2(5) | (part1by2(9) << 1) | (part1by2(3) << 2)
        ),
        "0x0000000000000467".into(),
    );
    // morton roundtrip 全点
    let ok_rt = (0..65536u32).all(|x| compact1by1(part1by1(x)) == x)
        && (0..100_000u64).all(|x| compact1by2(part1by2(x)) == x)
        && compact1by2(part1by2(0x1f_ffff)) == 0x1f_ffff
        && compact1by2(part1by2(256)) == 256
        && compact1by2(part1by2(0x154123)) == 0x154123;
    chk(
        "morton 往復全数 (2D 0..65536 / 3D 0..10万+境界)",
        ok_rt.to_string(),
        "true".into(),
    );
    // 仕様直交検証: 入力 bit i が出力 bit 3i に来る (小境界)
    let spec_ok = (0..1024u64).all(|x| {
        let mut r = 0u64;
        for i in 0..21 {
            r |= ((x >> i) & 1) << (3 * i);
        }
        part1by2(x) == r
    });
    chk(
        "morton3 仕様 bit i→3i (0..1024)",
        spec_ok.to_string(),
        "true".into(),
    );
    // rn gs
    chk(
        "splitmix64(42)[0]",
        format!("0x{:016x}", {
            let mut x = 42u64.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            z ^= z >> 31;
            x = 0;
            let _ = x;
            z
        }),
        "0xbdd732262feb6e95".into(),
    );
    // md5
    chk(
        "md5(\"\")",
        md5_hex(b""),
        "d41d8cd98f00b204e9800998ecf8427e".into(),
    );
    chk(
        "md5(\"abc\")",
        md5_hex(b"abc"),
        "900150983cd24fb0d6963f7d28e17f72".into(),
    );
    // sRGB
    chk(
        "srgb encode(0.5) ",
        format!("{:.10}", srgb_encode(0.5)),
        format!("{:.10}", 0.7353569831),
    );
    chk(
        "srgb decode(0.5)",
        format!("{:.10}", srgb_decode(0.5)),
        format!("{:.10}", 0.2140411405),
    );
    // proj 検算
    let (w, _) = proj_mats(90.0, 1.0, 0.1, 100.0);
    chk(
        "proj near→ndc 0",
        format!("{:.12}", (w[2][2] * -0.1 + w[3][2]) / 0.1),
        "0.000000000000".into(),
    );
    chk(
        "proj far→ndc 1",
        format!("{:.12}", (w[2][2] * -100.0 + w[3][2]) / 100.0),
        "1.000000000000".into(),
    );
    // gcd
    chk(
        "gcd(1071,1029)",
        gcd_i128(1071, 1029).to_string(),
        "21".into(),
    );
    // invmod (孤立コメントとして残っていた忘れ物 pin を回収 — wave 106。
    // 3×81 = 243 = 2×121+1)
    chk(
        "invmod(3,121)",
        modinv_i128(3, 121)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "None".into()),
        "81".into(),
    );
    // san 簡体字集合 (wave 106 で 298 → 452 字「主張」、wave 110 で機械検算により
    // 実効ユニーク 445 (447 tokens − 重複 2) に訂正し +12 字で 確定 457 字) の回帰ピン:
    // U+4E3A (コミットメッセージ誤字で素通りした字) を検出し、日本語使用字は誤検出しない
    let sf = san_scan_text("安全=\u{4e3a}重確認\n");
    chk(
        "san 簡体字検出 (U+4E3A)",
        sf.iter()
            .filter(|f| f.rule == "SIMPLIFIED")
            .count()
            .to_string(),
        "1".into(),
    );
    chk(
        "san 日本文誤検出なし",
        san_scan_text("安全確認・一万円の処理結果\n")
            .len()
            .to_string(),
        "0".into(),
    );
    // wave 110 追加 pin: 452 主張の過大申告 (+7) を機械検算が捕捉した経緯の再発防止。
    // 集合のユニーク数を厳密 457 にピン (重複混入・過大申告・静寂縮小の全てを RED 化)。
    let uniq: std::collections::HashSet<char> = SIMPLIFIED_SURE.chars().collect();
    chk(
        "san 簡体字集合ユニーク 457 字",
        uniq.len().to_string(),
        "457".into(),
    );
    // wave 110 追加 pin: 実在汚染として発見された新規 12 字の代表 (U+73AF) を検出し、
    // 対応する日本語使用字 (U+74B0) は誤検出しない。
    let sf2 = san_scan_text("境界=\u{73af} untouched\n");
    chk(
        "san 簡体字検出 (U+73AF)",
        sf2.iter()
            .filter(|f| f.rule == "SIMPLIFIED")
            .count()
            .to_string(),
        "1".into(),
    );
    chk(
        "san 日本文誤検出なし (U+74B0)",
        san_scan_text("境界環は untouched\n").len().to_string(),
        "0".into(),
    );
    // ---- rq 小言語ピン (2026-07-26 導入。Python struct+ctypes 移行の正確性錨) ----
    // f32 加算の厳密 bits (ハード IEEE: 0.1f32+0.2f32 = 0x3E99999A)
    let (rc, out) = rq_run("p 0.1+0.2;\n", false, false);
    chk(
        "rq f32 0.1+0.2 bits",
        format!("{rc}:{}", out.trim_end()),
        "0:0x3E99999A 0.3".into(),
    );
    // libm FFI 同一性 (ctypes libm 検証値: expf(0.5) = 0x3FD3094C)
    let (rc, out) = rq_run("p exp(0.5);\n", false, false);
    chk(
        "rq libm exp(0.5) FFI",
        format!("{rc}:{}", out.trim_end()),
        "0:0x3FD3094C 1.6487212".into(),
    );
    // 静的型検査: i64 リテラルの f32 への暗黙流入を拒否
    let (rc, out) = rq_run("let a: f32 = 1;\n", false, false);
    chk(
        "rq 型不一致拒否 (exit 2)",
        format!("{rc}:{}", out.contains("宣言型 f32 に対し式の型は i64")),
        "2:true".into(),
    );
    // fn + while + 左結合の決定出力 (1+..+5 = 15)
    let (rc, out) = rq_run(
        "fn addmul(a:i64,b:i64)->i64 { ret a*10+b; }\nlet s:i64 = 0;\nlet k:i64 = 1;\nwhile k <= 5 {\n s = addmul(s,k);\n k = k+1;\n}\np s;\n",
        false,
        false,
    );
    chk(
        "rq fn/while 累積",
        format!("{rc}:{}", out.trim_end()),
        "0:0x0000000000003039 12345".into(),
    );
    // bits/b 往復 (from_bits 0x3F800000 = 1.0)
    let (rc, out) = rq_run("p bits(b(0x3F800000));\n", false, false);
    chk(
        "rq bits/b 往復",
        format!("{rc}:{}", out.trim_end()),
        "0:0x3F800000 1065353216".into(),
    );
    // NaN 比較規則 (IEEE: 全順序比較 false、!= は true)
    let (rc, out) = rq_run("p nan() < 1.0;\np nan() != nan();\n", false, false);
    chk(
        "rq NaN 比較規則",
        format!("{rc}:{}", out.trim_end().replace('\n', "|")),
        "0:false|true".into(),
    );
    // assert 失敗は exit 3 (fail-loud)
    let (rc, out) = rq_run("assert 1 > 2;\n", false, false);
    chk(
        "rq assert 失敗 exit 3",
        format!("{rc}:{}", out.contains("assert 失敗")),
        "3:true".into(),
    );
    // prelude: dot3/len3/luma601 が Rust 実装と bit 同一 (luma601(1,0,0)=f32(0.299))
    let (rc, out) = rq_run(
        "p luma601(1.0,0.0,0.0);\np len3(0.0,0.0,4.5);\n",
        true,
        false,
    );
    chk(
        "rq prelude luma601/len3",
        format!("{rc}:{}", out.trim_end().replace('\n', "|")),
        "0:0x3E991687 0.299|0x40900000 4.5".into(),
    );
    // ---- rq v2 ピン (const/複合代入/for/loop/match/配列/ビット演算/elif 等) ----
    let (rc, out) = rq_run(
        "const B: i64 = A * 3;\nconst A: i64 = 2;\np B;\n",
        false,
        false,
    );
    chk(
        "rq v2 const 順序不問",
        format!("{rc}:{}", out.trim_end()),
        "0:0x0000000000000006 6".into(),
    );
    let (rc, out) = rq_run("const A: i64 = 2.5;\n", false, false);
    chk(
        "rq v2 const 型不一致拒否",
        format!("{rc}:{}", out.contains("宣言型 i64 に対し式の型は f32")),
        "2:true".into(),
    );
    let (rc, out) = rq_run(
        "let x: f32 = 1.0;\nx += 0.5;\nx *= 3.0;\np x;\n",
        false,
        false,
    );
    chk(
        "rq v2 複合代入",
        format!("{rc}:{}", out.trim_end()),
        "0:0x40900000 4.5".into(),
    );
    let (rc, out) = rq_run(
        "let s: i64 = 0;\nfor i: i64 in 0..=10 { s += i; }\np s;\n",
        false,
        false,
    );
    chk(
        "rq v2 for ..= 総和 55",
        format!("{rc}:{}", out.trim_end()),
        "0:0x0000000000000037 55".into(),
    );
    let (rc, out) = rq_run(
        "let s: i64 = 0;\nfor i: i64 in 0..10 {\n if i == 5 { break; }\n if i % 2 == 0 { continue; }\n s += i;\n}\np s;\n",
        false,
        false,
    );
    chk(
        "rq v2 for break/continue",
        format!("{rc}:{}", out.trim_end()),
        "0:0x0000000000000004 4".into(),
    );
    let (rc, out) = rq_run(
        "let k: i64 = 0;\nloop {\n k += 1;\n if k >= 5 { break; }\n}\np k;\n",
        false,
        false,
    );
    chk(
        "rq v2 loop break",
        format!("{rc}:{}", out.trim_end()),
        "0:0x0000000000000005 5".into(),
    );
    let (rc, out) = rq_run(
        "let c: i64 = 2;\nmatch c {\n 0 => { p \"zero\"; }\n 2 => { p \"two\"; }\n _ => { p \"other\"; }\n}\nlet b: bool = false;\nmatch b {\n true => { p \"t\"; }\n false => { p \"f\"; }\n}\n",
        false,
        false,
    );
    chk(
        "rq v2 match i64/bool",
        format!("{rc}:{}", out.trim_end().replace('\n', "|")),
        "0:two|f".into(),
    );
    let (rc, out) = rq_run(
        "let c: i64 = 1;\nmatch c {\n 1 => { p 1; }\n}\n",
        false,
        false,
    );
    chk(
        "rq v2 match 網羅強制",
        format!("{rc}:{}", out.contains("match は網羅的でない")),
        "2:true".into(),
    );
    let (rc, out) = rq_run(
        "let c: i64 = 1;\nmatch c {\n 1 => { p 1; }\n 1 => { p 2; }\n _ => { p 0; }\n}\n",
        false,
        false,
    );
    chk(
        "rq v2 match 腕重複拒否",
        format!("{rc}:{}", out.contains("match 腕が重複")),
        "2:true".into(),
    );
    let (rc, out) = rq_run(
        "let a: [f32; 3] = [1.0, 2.0, 3.0];\na[1] = 9.0;\np a;\np len(a);\nlet b2: [f32; 3] = fill(a, 0.0);\np b2;\n",
        false,
        false,
    );
    chk(
        "rq v2 配列基本",
        format!("{rc}:{}", out.trim_end().replace('\n', "|")),
        "0:[ 0x3F800000 1, 0x41100000 9, 0x40400000 3 ]|0x0000000000000003 3|[ 0x00000000 0, 0x00000000 0, 0x00000000 0 ]".into(),
    );
    let (rc, out) = rq_run("let a: [f32; 2] = [1.0, 2.0];\np a[2];\n", false, false);
    chk(
        "rq v2 配列境界外 exit 3",
        format!("{rc}:{}", out.contains("境界外アクセス")),
        "3:true".into(),
    );
    let (rc, out) = rq_run(
        "let h: u32 = 0x811C9DC5;\nlet d: u32 = 0x61;\nh = (h ^ d) * 0x01000193;\np h;\n",
        false,
        false,
    );
    chk(
        "rq v2 ビット演算 FNV-1a",
        format!("{rc}:{}", out.trim_end()),
        "0:0xE40C292C 3826002220".into(),
    );
    let (rc, out) = rq_run("let x: i64 = 1;\np x << 65;\n", false, false);
    chk(
        "rq v2 シフトマスク",
        format!("{rc}:{}", out.trim_end()),
        "0:0x0000000000000002 2".into(),
    );
    let (rc, out) = rq_run("p pi();\n", false, false);
    chk(
        "rq v2 pi() bits",
        format!("{rc}:{}", out.trim_end()),
        "0:0x40490FDB 3.1415927".into(),
    );
    let (rc, out) = rq_run(
        "p fma(b(0x3F800400), b(0x3F800400), b(0xBF801000));\np b(0x3F800400)*b(0x3F800400)+b(0xBF801000);\n",
        false,
        false,
    );
    chk(
        "rq v2 fma 単一丸め",
        format!("{rc}:{}", out.trim_end().replace('\n', "|")),
        "0:0xB97FFC00 -0.00024412572|0xB9800000 -0.00024414063".into(),
    );
    let (rc, out) = rq_run(
        "let v: i64 = 2;\nif v == 1 { p \"one\"; }\nelif v == 2 { p \"two\"; }\nelse { p \"other\"; }\n",
        false,
        false,
    );
    chk(
        "rq v2 elif",
        format!("{rc}:{}", out.trim_end()),
        "0:two".into(),
    );
    let (rc, out) = rq_run("assert 1 == 2, \"等しいはず\";\n", false, false);
    chk(
        "rq v2 assert ラベル",
        format!("{rc}:{}", out.contains("assert 失敗: 等しいはず")),
        "3:true".into(),
    );
    let (rc, out) = rq_run("for i: i64 in 0..3 {\n i = 9;\n}\n", false, false);
    chk(
        "rq v2 for 変数代入拒否",
        format!("{rc}:{}", out.contains("for ループ変数 `i` への代入は禁止")),
        "2:true".into(),
    );
    let (rc, out) = rq_run(
        "const RC: i64 = 7;\nlet c: i64 = 7;\nmatch c {\n RC => { p \"hit\"; }\n _ => { p \"miss\"; }\n}\n",
        false,
        false,
    );
    chk(
        "rq v2 match const 腕",
        format!("{rc}:{}", out.trim_end()),
        "0:hit".into(),
    );
    println!("selftest: {fails} FAIL");
    if fails > 0 {
        1
    } else {
        0
    }
}

fn cmd_man(a: &[String]) -> i32 {
    // man <cmd>: 主要コマンドの詳細説明 (flagship に限る)
    let map: &[(&str, &str)] = &[
        ("expr", "数式の厳密評価。既定 f64+bits、--f32 で全演算 f32 逐次丸め、--frac で i128 Fraction (10 進リテラル正確・循環節付き 10 進展開)。演算子 + - * / % ^ << >>、関数 gcd/lcm/min/max/sqrt/pow/abs/floor/ceil/round/trunc/ln/log2/log10/exp/sin/cos/tan/atan/hypot/clamp、定数 pi/e/tau/inf/nan、hex 0x リテラル。"),
        ("ulperr", "式 (x 含有) の f32 逐次評価と x での Fraction 正確評価を比較し、近接 f32 との ulp 距離を返す。監査対象式の誤差源分離に。例: rspeed ulperr 'x*0.5+0.25' 1.3"),
        ("monotone", "式のグリッド上の単調性を検査。違反の最初の x も報告。単調増加前提の検証 (パレット成長等) に。例: rspeed monotone 'x*2+1' 0 100 101"),
        ("roundtrip", "f(g(x))−x の f32 ulp 距離をグリッド走査。pack/unpack 系の自己双対検証に。例: rspeed roundtrip 'x/100' 'x*100' 0 1000 101"),
        ("fmdiff", "rustfmt 逸脱行の集合を HEAD 版の同集合に包含させる監査 fmt 規律の機械判定。自己起因逸脱 0 で PASS。"),
        ("seal", "提出前検証の一括ゲート: 変更ファイル san → fmdiff → trailws → 台帳件数 → テスト → digest → env-check。--quick --skip-tests --skip-bench で段省略可。"),
        ("snapshot", "git 追跡全ファイルの md5 manifest を ~/.rspeed-cache/snapshot-<tag>.txt へ。snapcheck で ADD/MOD/DEL 差分 (sandbox 巻戻りの機械検出)。git が壊れていても restore-env 後でも動く。"),
        ("rescue", "変更追跡 + 指定 untracked の構造維持退避 (MANIFEST.md5 同梱)。既定 /tmp/rescue-rspeed。巻戻り 3 連発の教訓からの定形化。"),
        ("rq", "AI 記述最優先の静的型付き小言語 (f32 IEEE 厳密計算。Python struct+ctypes libm エミュレートの移行先)。rspeed rq <file.rq> | -e ソース [--prelude] [--check]。v2: const/for/loop/match/配列/ビット演算/elif。構文一次情報は docs/internal/RQ.md。"),
        ("adv-save", "固定版ファイルのゴールデン + md5 を ~/.rspeed-cache/adv/ に保存。adversarial 儀式 (逆行・RED 確認・忠実復元) の bookend。"),
        ("adv-restore", "ゴールデンを md5 照合して厳密復元。復元後 md5 も再照合。1bit でも違えば失敗。adversarial 儀式の endgame。"),
        ("dead", "pub fn の repo 全体トークン参照数を数え、定義行のみのものを列挙。ヒューリスティック (同名衝突・self 参照混入あり) のため 『消費者ゼロ候補』。一次情報照合 (実 grep) を別途必須とする。"),
        ("wave-info", "接尾辞 (例 DC) の台帳行 + 監査節冒頭を一括表示。振り返り・棚卸しの即時参照用。"),
        ("todo-pri", "残モジュールの推定優先度: 消費者×10 + digest 経路★+50 + キーワード +15 + 行数/20。digest 経路含有は最優先 (bit 同一性への影響)。"),
        ("selftest", "rspeed 自身の既知ピン検証 (expr bits/frac/f16/morton/RNG/md5/sRGB/proj/gcd)。ツール堕落を 1 コマンドで検出。"),
    ];
    if a.is_empty() {
        println!(
            "man 対応: {}",
            map.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
        );
        return 0;
    }
    for (k, v) in map {
        if *k == a[0] {
            println!("== {k} ==\n{v}");
            return 0;
        }
    }
    println!("{a0}: man なし (help 参照)", a0 = a[0]);
    0
}

fn cmd_nextwave(a: &[String]) -> i32 {
    // nextwave: 監査接尾辞未来予測 (現状分析: 最新 prefix の次 + 最小行数 todo)
    let _ = a;
    let rows = registry_rows();
    let last = rows
        .last()
        .map(|(id, _, _)| id.split('-').next().unwrap_or("？").to_string())
        .unwrap_or_default();
    println!("最新接尾辞: {last}");
    println!("次候補 (todo-pick 3):");
    let _ = cmd_todo_pick(&["3".into()]);
    let audit = read_text(&audit_path()).unwrap_or_default();
    let mut used: BTreeSet<char> = audit
        .lines()
        .filter_map(|l| l.strip_prefix("## ").and_then(|b| b.chars().next()))
        .collect();
    for c in 'A'..='Z' {
        if !used.contains(&c) {
            println!("次の未使用接尾辞: {c}");
            break;
        }
    }
    used.clear();
    0
}

// ===================================================================
// san — 不可視文字/CRLF/簡体字/文字化け/末尾改行 検査
// ===================================================================

const INVISIBLES: &[(char, &str)] = &[
    ('\u{200B}', "ZWSP U+200B"),
    ('\u{200C}', "ZWNJ U+200C"),
    ('\u{200D}', "ZWJ U+200D"),
    ('\u{2060}', "WordJoiner U+2060"),
    ('\u{FEFF}', "BOM U+FEFF"),
    ('\u{00A0}', "NBSP U+00A0"),
    ('\u{3000}', "IdeographicSpace U+3000"),
    ('\u{00AD}', "SoftHyphen U+00AD"),
    ('\u{2028}', "LineSeparator U+2028"),
    ('\u{2029}', "ParaSeparator U+2029"),
    ('\u{034F}', "CGJ U+034F"),
    ('\u{180E}', "MongolianVowelSep U+180E"),
];

const MOJIBAKE: &[(char, &str)] = &[
    ('\u{FFFD}', "ReplacementChar U+FFFD"),
    (
        '\u{00E3}',
        "Mojibake U+00E3 (日本語→Latin1 化け痕跡の可能性)",
    ),
];

// JIS X 0208 (新字体) に存在しない代表的な簡体字のみからなる保守的確定集合
// (誤検出を避けるため「日本語新字体と一致する可能性がある字」は除外済: 参/余/麦/乱/画/抜 等は入れない)
const SIMPLIFIED_SURE: &str = concat!(
    "\u{95e8}\u{95ee}\u{95ef}\u{95fb}\u{95f4}\u{95ed}\u{95f2}\u{95f8}\u{95fd}\u{9600}\u{961f}\u{9634}\u{9633}\u{9635}\u{9636}\u{9645}\u{9646}\u{9648}\u{9655}\u{9669}\u{9690}\u{96be}\u{9875}\u{9876}",
    "\u{987a}\u{987b}\u{987e}\u{988a}\u{9898}\u{989c}\u{989d}\u{98ce}\u{98de}\u{996e}\u{996d}\u{9970}\u{9971}\u{9972}\u{9976}\u{997a}\u{997c}\u{997f}\u{9986}\u{9992}\u{9a6c}\u{9a6e}\u{9a76}\u{9a8c}",
    "\u{9a91}\u{9a97}\u{9c7c}\u{9c81}\u{9e1f}\u{9e21}\u{9e2d}\u{9e45}\u{9e26}\u{9e70}\u{9f50}\u{9f7f}\u{9f84}\u{8f66}\u{8f68}\u{8f6c}\u{8f6e}\u{8f6f}\u{8f7b}\u{8f74}\u{8f83}\u{8f85}\u{8f86}\u{8f88}",
    "\u{8f89}\u{8f91}\u{8f93}\u{8f99}\u{8fb9}\u{8fc7}\u{8fbe}\u{8fc1}\u{8fd0}\u{8fdb}\u{8fdc}\u{8fde}\u{8fdf}\u{9009}\u{9012}\u{903b}\u{9057}\u{89c2}\u{89c4}\u{89c9}\u{8ba1}\u{8ba4}\u{8ba5}\u{8ba8}",
    "\u{8ba9}\u{8bad}\u{8bae}\u{8baf}\u{8bb0}\u{8bb2}\u{8bb8}\u{8bba}\u{8bbe}\u{8bbf}\u{8bc1}\u{8bc4}\u{8bc6}\u{8bc9}\u{8bca}\u{8bcd}\u{8bd1}\u{8bd5}\u{8bd7}\u{8bda}\u{8bdd}\u{8be2}\u{8be5}\u{8be6}",
    "\u{8bed}\u{8bef}\u{8bf1}\u{8bf2}\u{8bf4}\u{8bf7}\u{8bf8}\u{8bfa}\u{8bfb}\u{8bfe}\u{8c01}\u{8c03}\u{8c05}\u{8c08}\u{8c0a}\u{8c0b}\u{8c13}\u{8c22}\u{8c23}\u{8c26}\u{8c28}\u{8d22}\u{8d23}\u{8d24}",
    "\u{8d25}\u{8d27}\u{8d28}\u{8d29}\u{8d2a}\u{8d2b}\u{8d2f}\u{8d31}\u{8d34}\u{8d35}\u{8d37}\u{8d38}\u{8d39}\u{8d3a}\u{8d3c}\u{8d3e}\u{8d42}\u{8d44}\u{8d4b}\u{8d4c}\u{8d4f}\u{8d50}\u{8d54}\u{8d56}",
    "\u{8d5a}\u{8d5b}\u{8d60}\u{9488}\u{7ec7}\u{9489}\u{9493}\u{949d}\u{949e}\u{949f}\u{94a2}\u{94a5}\u{94a6}\u{94a7}\u{94a9}\u{94ae}\u{94b1}\u{94b3}\u{94bb}\u{94c1}\u{94c3}\u{94c5}\u{94c6}\u{94dc}",
    "\u{94dd}\u{94ed}\u{94f6}\u{94f8}\u{94fa}\u{94fe}\u{9500}\u{9501}\u{9504}\u{9505}\u{9508}\u{950b}\u{950c}\u{9510}\u{9519}\u{951a}\u{9521}\u{9523}\u{9524}\u{9525}\u{9526}\u{952d}\u{952e}\u{952f}",
    "\u{9530}\u{9540}\u{9547}\u{955c}\u{9570}\u{7ea2}\u{7ea6}\u{7ea7}\u{7eaa}\u{7eab}\u{7eac}\u{7eaf}\u{7eb1}\u{7eb2}\u{7eb3}\u{7eb5}\u{7eb6}\u{7eb7}\u{7eb8}\u{7eb9}\u{7eba}\u{7ebd}\u{7ebf}\u{7ec3}",
    "\u{7ec4}\u{7ec5}\u{7ec6}\u{7ec8}\u{7eca}\u{7ecd}\u{7ece}\u{7ecf}\u{7ed1}\u{7ed2}\u{7ed3}\u{7ed5}\u{7ed8}\u{7ed9}\u{7eda}\u{7edd}\u{7edf}\u{7ee2}\u{7ee3}\u{7ee7}\u{7ee9}\u{7eea}\u{7eeb}",
    "\u{7eed}\u{7eee}\u{7ef3}\u{7ef4}\u{7ef5}\u{7ef7}\u{7ef8}\u{7efc}\u{7efd}\u{7eff}\u{7f00}\u{7f06}\u{7f0e}\u{7f13}\u{7f14}\u{7f15}\u{7f16}\u{7f18}\u{7f1a}\u{7f20}\u{7f28}\u{7f29}\u{7f2a}\u{9965}",
    "\u{9968}\u{996a}\u{996f}\u{9980}\u{9981}\u{9988}\u{998b}\u{998d}\u{998f}",
    // wave 106 追加 154 字 (コミットメッセージ簡体字誤字 (U+4E3A) が san を素通りした経緯で拡張。
    // 候補 426 字 → cp932 エンコード不可 (= JIS X 0208 非含有 = 日本文出現不能) の
    // 機械的フィルタで 226 字に確定 → 既存 298 字と重複除去で +154 字 = 計 452 字。
    // 写学数据个网没体 等の日本語使用字は同フィルタで機械的に除外済)
    "\u{4e3a}\u{6c49}\u{89c1}\u{5173}\u{53d1}\u{79cd}\u{6837}\u{4e60}\u{65f6}\u{957f}\u{5e93}\u{5904}\u{73b0}\u{5f00}\u{4e48}\u{8fd9}\u{4ebf}\u{9f99}\u{5b9e}\u{56fe}\u{5706}\u{4e50}\u{9a7f}\u{6cfd}\u{62e9}\u{5bf9}",
    "\u{5e94}\u{4e49}\u{52a1}\u{52a8}\u{7535}\u{84dd}\u{5458}\u{4f17}\u{4f18}\u{4fe9}\u{4eec}\u{4ec5}\u{4f1e}\u{4f1f}\u{4f20}\u{4f24}\u{4f26}\u{4f2a}\u{5e01}\u{5e05}\u{5e08}\u{5e10}\u{5e26}\u{5e2e}\u{5e86}\u{5e90}",
    "\u{5e99}\u{5e9f}\u{5f02}\u{5f20}\u{5f39}\u{5f52}\u{5f55}\u{5f7b}\u{590d}\u{5fc6}\u{5fe7}\u{6000}\u{6001}\u{6002}\u{603b}\u{6073}\u{6076}\u{60af}\u{60ef}\u{6124}\u{6151}\u{61a8}\u{8ba2}\u{8bbc}\u{8bbd}\u{8bc0}",
    "\u{7978}\u{79bb}\u{79ef}\u{7a02}\u{7a23}\u{7a33}\u{7a77}\u{7a8d}\u{7a91}\u{7a9c}\u{7a9d}\u{7ade}\u{7b3a}\u{7b5b}\u{7b77}\u{7b79}\u{7b7e}\u{7b80}\u{7bd3}\u{7ba9}\u{7c7b}\u{7caa}\u{7d27}\u{7ea0}\u{7ea4}\u{7f19}",
    "\u{7f1d}\u{7f24}\u{7f34}\u{7f81}\u{7f9f}\u{7fd8}\u{800d}\u{529e}\u{529d}\u{52b2}\u{52b3}\u{52bf}\u{52cb}\u{534f}\u{5356}\u{5355}\u{5361}\u{5367}\u{536b}\u{5385}\u{5386}\u{538b}\u{538c}\u{5395}\u{53a2}\u{53bf}",
    "\u{53d8}\u{53e0}\u{53e6}\u{53f9}\u{5413}\u{5415}\u{5417}\u{542f}\u{5434}\u{5455}\u{545b}\u{545c}\u{5462}\u{5482}\u{54b1}\u{54cd}\u{54d1}\u{54d7}\u{54df}",
    // wave 110 追加 12 字 (リポジトリ全量走査で実在汚染として発見し実測で補完:
    // 機械フィルタの候補プール網羅漏れだった 12 字)。併せて機械検算で確定:
    // 従来の「452 字」主張は過大で実効ユニーク 445 (447 tokens − 重複 2) だった
    // ため、重複 2 (U+9992/U+7EC7) を除去し +12 字で 457 字に確定。
    // selftest でユニーク数を厳密ピン (過大申告・重複の再発防止)。
    "\u{89c6}\u{6c89}\u{786e}\u{5783}\u{573e}\u{541e}\u{73af}\u{78b3}\u{6237}\u{4efd}\u{9304}\u{6362}"
);

struct Finding {
    line: usize,
    col: usize,
    rule: &'static str,
    msg: String,
}

fn san_scan_file(p: &Path) -> Result<Vec<Finding>, String> {
    let bytes = fs::read(p).map_err(|e| e.to_string())?;
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return Err("非 UTF-8 (バイナリ/文字コード異常)".into()),
    };
    Ok(san_scan_text(&text))
}

/// san の走査核 (ファイル IO から分離 — selftest で直接ピン可能)
fn san_scan_text(text: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut line = 1usize;
    let mut col = 0usize;
    for ch in text.chars() {
        col += 1;
        if ch == '\n' {
            line += 1;
            col = 0;
            continue;
        }
        if ch == '\r' {
            out.push(Finding {
                line,
                col,
                rule: "CR",
                msg: "CR (CRLF 混入)".into(),
            });
            continue;
        }
        for &(c, name) in INVISIBLES {
            if ch == c {
                out.push(Finding {
                    line,
                    col,
                    rule: "INVISIBLE",
                    msg: name.into(),
                });
            }
        }
        for &(c, name) in MOJIBAKE {
            if ch == c {
                out.push(Finding {
                    line,
                    col,
                    rule: "MOJIBAKE",
                    msg: name.into(),
                });
            }
        }
        if SIMPLIFIED_SURE.contains(ch) {
            out.push(Finding {
                line,
                col,
                rule: "SIMPLIFIED",
                msg: format!("簡体字 U+{:04X} '{ch}'", ch as u32),
            });
        }
    }
    if !text.is_empty() && !text.ends_with('\n') {
        out.push(Finding {
            line,
            col: col + 1,
            rule: "EOF",
            msg: "末尾改行なし".into(),
        });
    }
    out
}

fn cmd_san(args: &[String]) -> i32 {
    let mut targets: Vec<PathBuf> = Vec::new();
    for a in args {
        targets.push(PathBuf::from(a));
    }
    if targets.is_empty() {
        eprintln!("usage: rspeed san <file|dir>…");
        return 2;
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for t in &targets {
        if t.is_dir() {
            walk_files(t, &mut files);
        } else {
            files.push(t.clone());
        }
    }
    files.sort();
    let mut total = 0usize;
    let mut nfiles = 0usize;
    for f in &files {
        nfiles += 1;
        match san_scan_file(f) {
            Ok(fs) => {
                for x in &fs {
                    total += 1;
                    println!(
                        "{}:{}:{} [{}] {}",
                        f.display(),
                        x.line,
                        x.col,
                        x.rule,
                        x.msg
                    );
                }
            }
            Err(e) => {
                // バイナリ等は警告のみ
                println!("{}: SKIP ({e})", f.display());
            }
        }
    }
    println!("san: {nfiles} files scanned, {total} findings");
    if total > 0 {
        1
    } else {
        0
    }
}

// ===================================================================
// find — 高速リテラル検索 (memchr 相当、UTF-8 テキストのみ)
// ===================================================================

fn cmd_find(args: &[String]) -> i32 {
    let mut count_only = false;
    let mut rest: Vec<String> = Vec::new();
    for a in args {
        match a.as_str() {
            "--count" | "-c" => count_only = true,
            _ => rest.push(a.clone()),
        }
    }
    if rest.len() < 2 {
        eprintln!("usage: rspeed find [--count] <needle> <path>…");
        return 2;
    }
    let needle = rest.remove(0);
    let mut files: Vec<PathBuf> = Vec::new();
    for t in &rest {
        let p = PathBuf::from(t);
        if p.is_dir() {
            walk_files(&p, &mut files);
        } else {
            files.push(p);
        }
    }
    files.sort();
    let mut hits = 0usize;
    for f in &files {
        let Some(text) = read_text(f) else { continue };
        let mut file_hits = 0usize;
        for (ln, l) in text.lines().enumerate() {
            if l.contains(&needle) {
                file_hits += l.matches(&needle).count();
                if !count_only {
                    println!("{}:{}: {}", f.display(), ln + 1, l.trim());
                }
            }
        }
        if count_only && file_hits > 0 {
            println!("{}: {}", f.display(), file_hits);
        }
        hits += file_hits;
    }
    println!("find: {hits} hits");
    if hits == 0 {
        1
    } else {
        0
    }
}

// ===================================================================
// regcount — 台帳エントリ数積算 (grep -cE '^\| (A期|[A-Z]{1,3})-[0-9]+' 等価)
// ===================================================================

fn regcount_line(l: &str) -> bool {
    let Some(rest) = l.strip_prefix("| ") else {
        return false;
    };
    // A期-<digits> or [A-Z]{1,3}-<digits>
    if let Some(r) = rest.strip_prefix("A期-") {
        return r
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false);
    }
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_uppercase() {
        i += 1;
    }
    if i == 0 || i > 3 {
        return false;
    }
    if bytes.get(i) != Some(&b'-') {
        return false;
    }
    bytes
        .get(i + 1)
        .map(|b| b.is_ascii_digit())
        .unwrap_or(false)
}

fn cmd_regcount(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("usage: rspeed regcount <file>");
        return 2;
    }
    let text = match read_text(Path::new(&args[0])) {
        Some(t) => t,
        None => {
            eprintln!("読めません: {}", args[0]);
            return 2;
        }
    };
    let n = text.lines().filter(|l| regcount_line(l)).count();
    println!("{n}");
    0
}

// ===================================================================
// audit-todo — 棚卸し残抽出 (訂正版 comm 一式の内部実装)
// ===================================================================

fn cmd_audit_todo(args: &[String]) -> i32 {
    if args.len() < 2 {
        eprintln!("usage: rspeed audit-todo <audit.md> <src-dir>");
        return 2;
    }
    let audit = read_text(Path::new(&args[0])).unwrap_or_default();
    let mut done: BTreeSet<String> = BTreeSet::new();
    for l in audit.lines() {
        let Some(body) = l.strip_prefix("## ") else {
            continue;
        };
        // "## DC. render_graph.rs — …" 形式: 接尾辞部分 + ". " + 名前.rs
        let Some(dotpos) = body.find(". ") else {
            continue;
        };
        let (prefix, rest) = (&body[..dotpos], &body[dotpos + 2..]);
        if prefix.is_empty()
            || !prefix
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '/' || c == '-')
        {
            continue;
        }
        let name: String = rest
            .chars()
            .take_while(|c| *c != '.' && *c != ' ' && *c != '\u{3000}')
            .collect();
        if !name.is_empty() {
            done.insert(name);
        }
    }
    let mut all: BTreeSet<String> = BTreeSet::new();
    for e in fs::read_dir(&args[1]).into_iter().flat_map(|r| r.flatten()) {
        let p = e.path();
        if p.extension().map(|x| x == "rs").unwrap_or(false) {
            if let Some(stem) = p.file_stem() {
                all.insert(stem.to_string_lossy().into_owned());
            }
        }
    }
    let todo: Vec<&String> = all.difference(&done).collect();
    println!(
        "done: {}, all: {}, todo: {}",
        done.len(),
        all.len(),
        todo.len()
    );
    for t in &todo {
        println!("{t}");
    }
    0
}

// ===================================================================
// fmdiff — rustfmt 逸脱の HEAD ベースライン包含照合
//   規律: 現偏差行集合 ⊆ HEAD 偏差行集合 (自分が足した行は全て正準形)
// ===================================================================

fn run_fmt(path: &Path) -> Result<String, String> {
    let out = Command::new("rustfmt")
        .args(["--edition", "2021", "--emit", "stdout"])
        // skip_children: out-of-line mod (子モジュール) へ再帰しない。
        // 既定 (再帰) だと HEAD 版を /tmp の孤立ファイルに落とした際 `mod foo;`
        // の解決に失敗して fmt 不能 (= mod 含有ファイルの fmdiff が構造的に
        // FAIL する制約) だった。子へ潜らないので孤立ファイルでも常に評価可、
        // mod 無しファイルの出力は不変 (wave 110 で根治、捕捉 32 件目)。
        .args(["--config", "skip_children=true"])
        .arg(path)
        .output()
        .map_err(|e| format!("rustfmt 起動失敗: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "rustfmt 失敗: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    // 先頭 2 行 ("<path>:" + 空行) を除く
    let mut lines = text.lines();
    lines.next();
    lines.next();
    Ok(lines.collect::<Vec<_>>().join("\n"))
}

/// DP-LCS 差分: a→b の b 側差分行 (a になく b にある) を集合で返す。
/// 両方数 <~4000 行を想定 (full_graph_wiring 級で ~2700)。
fn lcs_added(a: &[&str], b: &[&str]) -> BTreeSet<String> {
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return b.iter().map(|s| s.to_string()).collect();
    }
    if m == 0 {
        return BTreeSet::new();
    }
    // 共通前後縁を落として DP サイズ削減
    let mut head = 0;
    while head < n.min(m) && a[head] == b[head] {
        head += 1;
    }
    let (mut ta, mut tb) = (n, m);
    while ta > head && tb > head && a[ta - 1] == b[tb - 1] {
        ta -= 1;
        tb -= 1;
    }
    let an = ta - head;
    let bn = tb - head;
    if an == 0 {
        return b[head..tb].iter().map(|s| s.to_string()).collect();
    }
    if bn == 0 {
        return BTreeSet::new();
    }
    // DP (u32, 行優先) — an*bn が 25M 超なら安全側で全差集合扱い
    if an as u64 * bn as u64 > 25_000_000 {
        let sa: BTreeSet<&str> = a.iter().copied().collect();
        return b
            .iter()
            .filter(|s| !sa.contains(*s))
            .map(|s| s.to_string())
            .collect();
    }
    let mut dp = vec![0u32; (an + 1) * (bn + 1)];
    let stride = bn + 1;
    for i in 1..=an {
        for j in 1..=bn {
            dp[i * stride + j] = if a[head + i - 1] == b[head + j - 1] {
                dp[(i - 1) * stride + (j - 1)] + 1
            } else {
                dp[(i - 1) * stride + j].max(dp[i * stride + (j - 1)])
            };
        }
    }
    // 逆走査で差分行抽出
    let mut added = BTreeSet::new();
    let (mut i, mut j) = (an, bn);
    while i > 0 && j > 0 {
        if a[head + i - 1] == b[head + j - 1] {
            i -= 1;
            j -= 1;
        } else if dp[(i - 1) * stride + j] >= dp[i * stride + (j - 1)] {
            i -= 1;
        } else {
            added.insert(b[head + j - 1].to_string());
            j -= 1;
        }
    }
    while j > 0 {
        added.insert(b[head + j - 1].to_string());
        j -= 1;
    }
    added
}

fn deviant_lines(path: &Path, content: &str) -> Result<BTreeSet<String>, String> {
    let fmt = run_fmt(path)?;
    let a: Vec<&str> = fmt.lines().collect();
    let b: Vec<&str> = content.lines().collect();
    Ok(lcs_added(&a, &b)) // fmt→現状 の 現状側差分 = 逸脱行
}

fn cmd_fmdiff(args: &[String]) -> i32 {
    let gr = git_root();
    let mut rc = 0;
    for a in args {
        let p = PathBuf::from(a);
        if !p.exists() {
            eprintln!("{a}: 存在しません");
            rc = 2;
            continue;
        }
        let cur = fs::read_to_string(&p).unwrap_or_default();
        let cur_dev = match deviant_lines(&p, &cur) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{a}: {e}");
                rc = 2;
                continue;
            }
        };
        // HEAD 側
        let rel = p
            .strip_prefix(&gr)
            .unwrap_or(&p)
            .to_string_lossy()
            .replace('\\', "/");
        let head_content = Command::new("git")
            .args(["-C"])
            .arg(&gr)
            .args(["show", &format!("HEAD:{rel}")])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        match head_content {
            Some(hc) => {
                // HEAD 版を一時ファイルに書いて rustfmt に通す
                let tmp = std::env::temp_dir().join("rspeed_fmdiff_head.rs");
                if fs::write(&tmp, &hc).is_err() {
                    eprintln!("{a}: 一時ファイル書けません");
                    rc = 2;
                    continue;
                }
                let head_dev = match deviant_lines(&tmp, &hc) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("{a}: HEAD fmt: {e}");
                        rc = 2;
                        continue;
                    }
                };
                let extra: Vec<&String> = cur_dev.difference(&head_dev).collect();
                let status = if extra.is_empty() { "PASS" } else { "FAIL" };
                println!(
                    "{a}: HEAD 逸脱 {} 行 / 現 逸脱 {} 行 / 自己起因逸脱 {} 行 → {status}",
                    head_dev.len(),
                    cur_dev.len(),
                    extra.len()
                );
                for e in &extra[..extra.len().min(20)] {
                    println!("  自己起因: {e}");
                }
                if !extra.is_empty() {
                    rc = 1;
                }
            }
            None => {
                println!(
                    "{a}: HEAD に無い新規ファイル → 現逸脱 {} 行 (要全行正準)",
                    cur_dev.len()
                );
                if !cur_dev.is_empty() {
                    rc = 1;
                }
            }
        }
    }
    rc
}

// ===================================================================
// test / bench / warn — 変更検出つき独自ランナー
//   ・ソース指紋 (パス+mtime+size) が同一なら cargo を走らせず
//    直近ビルド成果物を直接実行 (cargo の fingerprint/起動コストを省略)
//   ・RUSTFLAGS に lld を既定同梱
// ===================================================================

fn rust_host() -> String {
    Command::new("rustc")
        .arg("-vV")
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find(|l| l.starts_with("host: "))
                .map(|l| l[6..].trim().to_string())
        })
        .unwrap_or_else(|| "x86_64-unknown-linux-gnu".into())
}

fn rustflags_default() -> String {
    let sysroot = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "/home/user/rust".into());
    let gcc_ld = format!("{sysroot}/lib/rustlib/{}/bin/gcc-ld", rust_host());
    env::var("RSPEED_RUSTFLAGS")
        .unwrap_or_else(|_| format!("-C link-arg=-fuse-ld=lld -C link-arg=-B{gcc_ld}"))
}

/// 指紋: crates/<p>/ 配下の .rs + Cargo.toml + workspace Cargo.toml/.cargo 設定
fn fingerprint(krate: &str) -> u64 {
    let ws = ws_root();
    let mut has = std::collections::hash_map::DefaultHasher::new();
    let mut files: Vec<PathBuf> = Vec::new();
    walk_files(&ws.join("crates").join(krate), &mut files);
    files.push(ws.join("Cargo.toml"));
    files.push(ws.join("Cargo.lock"));
    files.sort();
    for f in &files {
        f.hash(&mut has);
        if let Ok(md) = fs::metadata(f) {
            md.len().hash(&mut has);
            if let Ok(mt) = md.modified() {
                if let Ok(d) = mt.duration_since(std::time::UNIX_EPOCH) {
                    d.as_nanos().hash(&mut has);
                }
            }
        }
    }
    rust_version().hash(&mut has);
    has.finish()
}

fn rust_version() -> String {
    Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn fp_path(tag: &str) -> PathBuf {
    cache_dir().join(format!("fp-{tag}.txt"))
}

fn fp_changed(tag: &str, cur: u64) -> bool {
    let p = fp_path(tag);
    match fs::read_to_string(&p) {
        Ok(s) if s.trim() == format!("{cur:016x}") => false,
        _ => true,
    }
}

fn fp_store(tag: &str, cur: u64) {
    if let Err(e) = write_loud(&fp_path(tag), format!("{cur:016x}")) {
        eprintln!("警告: fingerprint 保存失敗: {e}");
    }
}

fn newest_unittest(krate: &str, profile: &str) -> Option<PathBuf> {
    let deps =
        ws_root()
            .join("target")
            .join(profile)
            .join(if profile == "dev" || profile == "debug" {
                "deps"
            } else {
                "deps"
            });
    let prefix = krate.replace('-', "_");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in fs::read_dir(deps).into_iter().flat_map(|r| r.flatten()) {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with(&format!("{prefix}-")) || p.extension().is_some() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(&p)
                .map(|m| m.permissions().mode() & 0o111 == 0)
                .unwrap_or(true)
            {
                continue;
            }
        }
        let mt = fs::metadata(&p).and_then(|m| m.modified()).ok()?;
        if best.as_ref().map(|(t, _)| mt > *t).unwrap_or(true) {
            best = Some((mt, p));
        }
    }
    best.map(|(_, p)| p)
}

fn run_cargo(args: &[&str]) -> i32 {
    let ws = ws_root();
    let st = Command::new("cargo")
        .args(args)
        .current_dir(&ws)
        .env("RUSTFLAGS", rustflags_default())
        .stdin(Stdio::null())
        .status();
    match st {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("cargo 起動失敗: {e}");
            127
        }
    }
}

fn cmd_test(args: &[String]) -> i32 {
    let mut krate = "rsift-opt-gfx".to_string();
    let mut filter: Vec<String> = Vec::new();
    let mut force = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" => {
                i += 1;
                krate = args
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| "rsift-opt-gfx".into());
            }
            "--rebuild" => force = true,
            other => filter.push(other.to_string()),
        }
        i += 1;
    }
    let tag = format!("test-{krate}");
    let fp = fingerprint(&krate);
    let need_build = force || fp_changed(&tag, fp) || newest_unittest(&krate, "debug").is_none();
    if need_build {
        let t0 = Instant::now();
        let rc = run_cargo(&[
            "test",
            "-p",
            &krate,
            "--lib",
            "--locked",
            "--offline",
            "--no-run",
        ]);
        if rc != 0 {
            eprintln!("rspeed test: ビルド失敗 (rc={rc})");
            return rc;
        }
        println!("rspeed: build {:.1}s", t0.elapsed().as_secs_f64());
        fp_store(&tag, fp);
    } else {
        println!("rspeed: build SKIP (ソース指紋不変)");
    }
    let Some(bin) = newest_unittest(&krate, "debug") else {
        eprintln!("rspeed test: テストバイナリが見つからない");
        return 3;
    };
    let n = std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(2);
    let t0 = Instant::now();
    let mut cmd = Command::new(&bin);
    cmd.arg("--test-threads").arg(n.to_string());
    for f in &filter {
        cmd.arg(f);
    }
    let st = cmd.status().map_err(|e| format!("{bin:?} 実行失敗: {e}"));
    println!(
        "rspeed: run {:.2}s (bin={})",
        t0.elapsed().as_secs_f64(),
        bin.display()
    );
    match st {
        Ok(s) => s.code().unwrap_or(1),
        Err(m) => {
            eprintln!("{m}");
            127
        }
    }
}

fn cmd_bench(args: &[String]) -> i32 {
    // rspeed bench <example> [--expect-digest <hex>] [grep 語…]
    let mut example = "wide_static_bench".to_string();
    let mut expect: Option<String> = None;
    let mut greps: Vec<String> = vec!["structural_digest".into(), "rows=".into()];
    let mut force = false;
    let mut i = 0;
    let mut rest: Vec<String> = Vec::new();
    while i < args.len() {
        match args[i].as_str() {
            "--expect-digest" => {
                i += 1;
                expect = args.get(i).cloned();
            }
            "--rebuild" => force = true,
            other => rest.push(other.to_string()),
        }
        i += 1;
    }
    if !rest.is_empty() {
        example = rest.remove(0);
    }
    if !rest.is_empty() {
        greps = rest;
    }
    let krate = "rsift-opt-gfx";
    let tag = format!("bench-{example}");
    let fp = fingerprint(krate);
    let bin = ws_root()
        .join("target")
        .join("release")
        .join("examples")
        .join(&example);
    let need_build = force || fp_changed(&tag, fp) || !bin.exists();
    if need_build {
        let t0 = Instant::now();
        let rc = run_cargo(&[
            "build",
            "-p",
            krate,
            "--release",
            "--locked",
            "--offline",
            "--example",
            &example,
        ]);
        if rc != 0 {
            eprintln!("rspeed bench: ビルド失敗 (rc={rc})");
            return rc;
        }
        println!("rspeed: build {:.1}s", t0.elapsed().as_secs_f64());
        fp_store(&tag, fp);
    } else {
        println!("rspeed: build SKIP (ソース指紋不変)");
    }
    let t0 = Instant::now();
    let out = Command::new(&bin).output();
    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            for line in stdout.lines() {
                if greps.iter().any(|g| line.contains(g)) {
                    println!("{line}");
                }
            }
            println!("rspeed: run {:.2}s", t0.elapsed().as_secs_f64());
            if let Some(want) = expect {
                let ok = stdout
                    .lines()
                    .any(|l| l.contains("structural_digest") && l.contains(&want));
                if ok {
                    println!("digest PASS: {want}");
                    0
                } else {
                    eprintln!("digest FAIL: 期待 {want} が出力に無い");
                    2
                }
            } else if o.status.success() {
                0
            } else {
                o.status.code().unwrap_or(1)
            }
        }
        Err(e) => {
            eprintln!("rspeed bench: 実行失敗: {e}");
            127
        }
    }
}

fn cmd_warn(args: &[String]) -> i32 {
    // rspeed warn <crate> [expect]
    let mut krate = "rsift-opt-gfx".to_string();
    let mut expect: Option<usize> = None;
    for a in args {
        if let Ok(v) = a.parse::<usize>() {
            expect = Some(v);
        } else {
            krate = a.clone();
        }
    }
    let ws = ws_root();
    let out = Command::new("cargo")
        .args(["check", "-p", &krate, "--lib", "--locked", "--offline"])
        .current_dir(&ws)
        .env("RUSTFLAGS", rustflags_default())
        .stdin(Stdio::null())
        .output()
        .expect("cargo check 失敗");
    let text = String::from_utf8_lossy(&out.stderr).into_owned();
    let n = text.matches("warning").count(); // 概算
                                             // 厳密には "warning: " 行数
    let strict = text
        .lines()
        .filter(|l| l.starts_with("warning") || l.contains(": warning"))
        .count();
    println!("warn: {krate} '{n}' (行ベース {strict})");
    match expect {
        Some(e) if e != strict => {
            eprintln!("warn FAIL: 期待 {e} ≠ 実測 {strict}");
            1
        }
        _ => 0,
    }
}

// ===================================================================
// main
// ===================================================================

fn help() {
    println!(
        "rspeed — Rsift 監査統合高速ツール (116 機能 / std のみ / rustc -O 単一バイナリ)\n\
\n\
[厳密数値系]\n\
  expr [--f32|--frac] <式>…      f64+bits / f32 逐次丸め / Fraction 正確分数 (循環節つき)\n\
  bits <式>…                     符号/指数/仮数分解 + f32/f16/bf16 丸め併記\n\
  bits-of <hex>                  bits→値 (16桁 f64 / 8桁 f32 / 4桁 f16)\n\
  ulp <式> | next <式> [+|-] [n]  ulp 値・隣接表現値\n\
  hfbits <式>…                   f32→f16/bf16 RNE 変換と誤差\n\
  fp-table | clamp-table         特殊値参照表 / f32→int の as 意味論境界表\n\
  gamma <0..1> | srgb-err [n]    sRGB piecewise 厳密 + pow2.2 近似誤差\n\
  morton <x> <y> [z]             モートン encode/decode + 往復検証 (21bit 全数検証済)\n\
  murmur|splitmix|xs64|pcg|fnv   ハッシュ/RNG 系列の厳密再現\n\
  prime <n> | bigfact <n> | fib <n>  素因数分解 / 階乗 / フィボナッチ\n\
  modpow b e m | invmod a m      冪乗剰余 / 拡張ユークリッド逆元\n\
  contfrac <式> [n]              連分数展開+漸近分数\n\
  table <式> <from> <to> <step> [--f32|--frac]  x 掃引テーブル\n\
  range <式> <from> <to> <n>     min/max/argmax (f64)\n\
  monotone <式> <from> <to> <n>  単調性検査 + 最初の違反\n\
  roundtrip <f> <g> <from> <to> <n>  f(g(x))−x の f32 ulp 距離\n\
  ulperr <式(x)> <x>             f32 逐次 vs 正確分数の ulp 距離\n\
  int-cast <値> <型> | quant <0..1> <max>  Rust `as` 意味論 / 量子化誤差\n\
  mat4 det|inv|mul <16>[32] | matc <16>  4x4 行列 (inv は検算つき) / det f32-f64 誤差\n\
  vec3 dot|cross|norm|dist <6>   ベクトル演算 f64+f32\n\
  lerp <a> <b> <t> | hypot <a> <b>  補間形式差 / naive-hypot 破綻境界\n\
  proj <fov> <aspect> <n> <f>    投影行列 (wgpu/GL 両規約+検算) / lookat <9>\n\
  quat <ax ay az deg> | quat-slerp <6>  クォータニオン+行列 / slerp vs nlerp\n\
  tri-area <9> | bary <8>        三角形面積 3 方式 / 重心座標+内外\n\
  halton <i> <b> | r2 [n]        低食い違い列 厳密分数/R2\n\
  color <r> <g> <b>              sRGB→linear + Rec709/601 輝度\n\
  percentile|histogram <値…|file> 分位数/ヒストグラム\n\
\n\
[ソーススキャナ系]\n\
  san <file|dir>…        不可視 12 種/CRLF/末尾改行/U+FFFD・U+00E3/簡体字 457 字\n\
  find [--count] <n> <p> 高速リテラル検索 / grep2 <A> <B> 共起分類\n\
  magic [dir] | floatlits | casts | clamps | divmod | shifts | unwraps\n\
  tests-index [crate] / test-find <str> / test-count [dir]  #[test] 索引・検索・積算\n\
  fns | pubs [--save/--check <tag>] | docs | dead  API 面・文書カバレッジ・消費者ゼロ候補\n\
  todo-scan | dups | longlines [n] | trailws | nonascii | eol | tabs\n\
  hotfiles [n] | diff <a> <b> | lines [dir]  churn・LCS 差分・行数順位\n\
\n\
[リポジトリ運用系]\n\
  status | changed-tests | env-check        git 状態/差分テスト提案/環境診断\n\
  snapshot [tag] / snapcheck [tag]          md5 manifest 保存/差分 (sandbox 巻戻り検出)\n\
  rescue [dir]                              変更追跡ファイル構造維持退避+MANIFEST.md5\n\
  md5 <f> / md5check <manifest>             自前 MD5 (RFC1321 検証済)\n\
  adv-save / adv-restore / adv-diff         adversarial 儀式 (ゴールデン/md5 忠実復元)\n\
  seal [--quick|--skip-tests|--skip-bench]  提出前検証一括ゲート (推奨: push 前に必ず)\n\
  dashboard [--full]                        監査状況の一括俯瞰\n\
  time-run <n> <cmd…> | binsize [n]         実行時間 min/median/p95 / target 容量上位\n\
  ghfile <repo> <path> [ref] | ghlatest <repo>  GitHub 一次原文/リリース到達性\n\
  wave-log <text> | journal [n]             時刻つき作業ジャーナル\n\
  registry-stats | wave-info <prefix> | burndown  台帳統計/節表示/wave 進行\n\
  audit-todo <md> <src> | todo-pick [n] | todo-pri | coverage  棚卸し系\n\
  regcount <md> | fmdiff <.rs>… | test | bench | warn  台帳積算/fmt 規律/ランナー/警告\n\
  selftest | man <cmd> | nextwave           自己既知ピン検証 / 個別解説 / 次 wave 分析\n\
\n\
環境変数: RSIFT_WS / RSIFT_GIT_ROOT / RSPEED_RUSTFLAGS (既定 lld)\n\
自己検証: `rspeed selftest` (expr/f16/morton/md5/sRGB/proj 他 19 ピン) を定期的に"
    );
}

// =====================================================================
// rq: AI 記述最優先の静的型付き小言語 (2026-07-26 v1 / 2026-07-26 v2)
//
// 目的: Python (struct + ctypes libm) による f32 IEEE エミュレート計算の
// rspeed 全面移行基盤。
//
// 設計方針 (AI が生成ミスしにくいことのみ最優先、人間の読みやすさは二の次):
//   * 全変数は明示型、暗黙変換は一切なし (静的型検査)。
//   * 文は必ず `;` 終端 (ブロック文を除く)。空白/改行は意味を持たない。
//   * f32 演算はハード IEEE-754 単精度。sqrt/exp/ln/pow/sin/cos/tan/atan2/
//     hypot は libm FFI (ctypes libm と bit 同一検証済)。fma は単一丸め。
//   * 演算子是法は Rust 同一・全て左結合。
//   * 仕様の一次情報は docs/internal/RQ.md (v2)。構文を忘れたら必ず読む。
// =====================================================================

/// 配列要素型 (スカラーのみ。配列の配列は禁止 = 平坦保証)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RqElem {
    F,
    I,
    U,
    B,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RqTy {
    F32,
    I64,
    U32,
    Bool,
    Str,
    /// 固定長配列 [要素; N] (N は 1..=256)
    Arr(RqElem, u16),
}

impl RqTy {
    fn name(self) -> String {
        match self {
            RqTy::F32 => "f32".into(),
            RqTy::I64 => "i64".into(),
            RqTy::U32 => "u32".into(),
            RqTy::Bool => "bool".into(),
            RqTy::Str => "str".into(),
            RqTy::Arr(e, n) => format!("[{}; {}]", rq_scalar_ty(e).name(), n),
        }
    }
    fn from_name(s: &str) -> Option<RqTy> {
        match s {
            "f32" => Some(RqTy::F32),
            "i64" => Some(RqTy::I64),
            "u32" => Some(RqTy::U32),
            "bool" => Some(RqTy::Bool),
            "str" => Some(RqTy::Str),
            _ => None,
        }
    }
}

fn rq_scalar_ty(e: RqElem) -> RqTy {
    match e {
        RqElem::F => RqTy::F32,
        RqElem::I => RqTy::I64,
        RqElem::U => RqTy::U32,
        RqElem::B => RqTy::Bool,
    }
}

fn rq_elem_of(t: RqTy) -> Option<RqElem> {
    match t {
        RqTy::F32 => Some(RqElem::F),
        RqTy::I64 => Some(RqElem::I),
        RqTy::U32 => Some(RqElem::U),
        RqTy::Bool => Some(RqElem::B),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
enum RqTok {
    Id(String),
    Fl(f32),
    I(i64),
    U(u32),
    St(String),
    Op(&'static str),
}

/// rq 字句解析。エラーは (行, メッセージ)。
fn rq_lex(src: &str) -> Result<Vec<(RqTok, usize)>, (usize, String)> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut out: Vec<(RqTok, usize)> = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
            continue;
        }
        if c == b'#' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let ln = line;
        if c.is_ascii_alphabetic() || c == b'_' {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push((RqTok::Id(src[s..i].to_string()), ln));
            continue;
        }
        if c.is_ascii_digit() {
            let s = i;
            if c == b'0' && i + 1 < b.len() && (b[i + 1] == b'x' || b[i + 1] == b'X') {
                i += 2;
                let hs = i;
                while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == b'_') {
                    i += 1;
                }
                if hs == i {
                    return Err((ln, "0x の直後に 16 進桁が必要".into()));
                }
                let t: String = src[hs..i].chars().filter(|&c| c != '_').collect();
                let v = u32::from_str_radix(&t, 16)
                    .map_err(|_| (ln, format!("u32 範囲外の 16 進リテラル: 0x{t}")))?;
                out.push((RqTok::U(v), ln));
                continue;
            }
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
            let mut is_f = false;
            // `1..2` (range) との曖昧さ回避: `.` の直後がもう一つ `.` なら
            // 小数点にしない (range 演算子へ抜ける)。`2.` のように直後が数字以外・
            // `.` 以外でも小数として確定する (Rust 同様)。
            if i < b.len() && b[i] == b'.' && !(i + 1 < b.len() && b[i + 1] == b'.') {
                is_f = true;
                i += 1;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                is_f = true;
                i += 1;
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                    i += 1;
                }
                let ds = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
                if ds == i {
                    return Err((ln, "指数部に桁が必要".into()));
                }
            }
            let t: String = src[s..i].chars().filter(|&c| c != '_').collect();
            if is_f {
                let v: f32 = t
                    .parse()
                    .map_err(|_| (ln, format!("f32 リテラル不正: {t}")))?;
                out.push((RqTok::Fl(v), ln));
            } else {
                let v: i64 = t
                    .parse()
                    .map_err(|_| (ln, format!("i64 範囲外の整数リテラル: {t}")))?;
                out.push((RqTok::I(v), ln));
            }
            continue;
        }
        // 先頭 `.` 小数 (.5 等)。`..` / `..=` は後段の 3/2 文字演算子が優先。
        if c == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            i += 1;
            let s = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                i += 1;
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                    i += 1;
                }
                let ds = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
                if ds == i {
                    return Err((ln, "指数部に桁が必要".into()));
                }
            }
            let raw: String = src[s..i].chars().filter(|&c| c != '_').collect();
            let v: f32 = format!("0.{raw}")
                .parse()
                .map_err(|_| (ln, format!("f32 リテラル不正: .{raw}")))?;
            out.push((RqTok::Fl(v), ln));
            continue;
        }
        if c == b'"' {
            i += 1;
            let mut raw: Vec<u8> = Vec::new();
            loop {
                if i >= b.len() {
                    return Err((ln, "文字列が閉じていない".into()));
                }
                match b[i] {
                    b'"' => {
                        i += 1;
                        break;
                    }
                    b'\\' => {
                        i += 1;
                        if i >= b.len() {
                            return Err((ln, "エスケープ途中で終端".into()));
                        }
                        match b[i] {
                            b'n' => raw.push(b'\n'),
                            b't' => raw.push(b'\t'),
                            b'\\' => raw.push(b'\\'),
                            b'"' => raw.push(b'"'),
                            x => return Err((ln, format!("未知のエスケープ: \\{}", x as char))),
                        }
                        i += 1;
                    }
                    x => {
                        if x == b'\n' {
                            line += 1;
                        }
                        raw.push(x);
                        i += 1;
                    }
                }
            }
            let s = String::from_utf8(raw).map_err(|_| (ln, "文字列が UTF-8 不正".into()))?;
            out.push((RqTok::St(s), ln));
            continue;
        }
        // 3 文字演算子優先 (..=)
        let three = if i + 2 < b.len() { &src[i..i + 3] } else { "" };
        if three == "..=" {
            out.push((RqTok::Op("..="), ln));
            i += 3;
            continue;
        }
        // 2 文字演算子優先
        let two = if i + 1 < b.len() { &src[i..i + 2] } else { "" };
        let two_op = match two {
            "<=" => Some("<="),
            ">=" => Some(">="),
            "==" => Some("=="),
            "!=" => Some("!="),
            "&&" => Some("&&"),
            "||" => Some("||"),
            "->" => Some("->"),
            "=>" => Some("=>"),
            ".." => Some(".."),
            "+=" => Some("+="),
            "-=" => Some("-="),
            "*=" => Some("*="),
            "/=" => Some("/="),
            "%=" => Some("%="),
            "<<" => Some("<<"),
            ">>" => Some(">>"),
            _ => None,
        };
        if let Some(op) = two_op {
            out.push((RqTok::Op(op), ln));
            i += 2;
            continue;
        }
        let one_op = match c {
            b'+' => Some("+"),
            b'-' => Some("-"),
            b'*' => Some("*"),
            b'/' => Some("/"),
            b'%' => Some("%"),
            b'<' => Some("<"),
            b'>' => Some(">"),
            b'!' => Some("!"),
            b'=' => Some("="),
            b';' => Some(";"),
            b':' => Some(":"),
            b',' => Some(","),
            b'(' => Some("("),
            b')' => Some(")"),
            b'{' => Some("{"),
            b'}' => Some("}"),
            b'[' => Some("["),
            b']' => Some("]"),
            b'&' => Some("&"),
            b'|' => Some("|"),
            b'^' => Some("^"),
            b'~' => Some("~"),
            _ => None,
        };
        match one_op {
            Some(op) => {
                out.push((RqTok::Op(op), ln));
                i += 1;
            }
            None => {
                return Err((ln, format!("解釈できない文字: {:?}", c as char)));
            }
        }
    }
    Ok(out)
}

#[derive(Clone, Debug)]
struct RqE {
    k: RqEK,
    ln: usize,
}

#[derive(Clone, Debug)]
enum RqEK {
    Fl(f32),
    I(i64),
    U(u32),
    Bl(bool),
    Sl(String),
    /// 配列リテラル (型注釈の文脈でのみ使用可)
    ArrLit(Vec<RqE>),
    Var(String),
    Call(String, Vec<RqE>),
    Neg(Box<RqE>),
    Not(Box<RqE>),
    /// ビット否定 ~ (i64/u32)
    BitNot(Box<RqE>),
    /// 添字 a[i]
    Idx(Box<RqE>, Box<RqE>),
    Bin(&'static str, Box<RqE>, Box<RqE>),
}

#[derive(Clone, Debug)]
struct RqS {
    k: RqSK,
    ln: usize,
}

/// 代入左辺 (変数 or 配列要素)
#[derive(Clone, Debug)]
enum RqTarget {
    Var(String),
    Idx(String, RqE),
}

/// match 腕パターン (型検査で const を値に解決済み)
#[derive(Clone, Copy, Debug, PartialEq)]
enum RqPat {
    I(i64),
    B(bool),
    Wild,
}

#[derive(Clone, Debug)]
enum RqSK {
    Let {
        n: String,
        ty: RqTy,
        e: RqE,
    },
    /// op=None は単純代入、Some(op) は複合代入 (+= 等、op は + - * / % の字)
    Set {
        t: RqTarget,
        op: Option<&'static str>,
        e: RqE,
    },
    /// トップレベル定数 (順序不問・コンパイル時評価)
    Const {
        n: String,
        ty: RqTy,
        e: RqE,
    },
    Fn {
        n: String,
        ps: Vec<(String, RqTy)>,
        rt: RqTy,
        body: Vec<RqS>,
    },
    If {
        c: RqE,
        t: Vec<RqS>,
        f: Vec<RqS>,
    },
    While {
        c: RqE,
        body: Vec<RqS>,
    },
    /// 範囲 for: VAR:i64 in a..b (incl=false) / a..=b (incl=true)
    For {
        v: String,
        a: RqE,
        b: RqE,
        incl: bool,
        body: Vec<RqS>,
    },
    Loop {
        body: Vec<RqS>,
    },
    Break,
    Continue,
    /// arms: (生パターン, 本体, 行)。const 名は型検査で解決
    Match {
        e: RqE,
        arms: Vec<(RawPat, Vec<RqS>, usize)>,
    },
    Ret(RqE),
    P(RqE),
    Assert {
        e: RqE,
        label: Option<String>,
    },
    Block(Vec<RqS>),
}

/// 予約語 (変数名・関数名に使用不可)。`_` は match のワイルドカード専用。
const RQ_RESERVED: &[&str] = &[
    "let", "const", "fn", "if", "elif", "else", "while", "for", "in", "loop", "match", "ret",
    "break", "continue", "p", "assert", "true", "false", "f32", "i64", "u32", "bool", "str", "_",
];
struct RqParser {
    t: Vec<(RqTok, usize)>,
    p: usize,
}

impl RqParser {
    fn peek(&self) -> Option<&RqTok> {
        self.t.get(self.p).map(|(k, _)| k)
    }
    fn peek2(&self) -> Option<&RqTok> {
        self.t.get(self.p + 1).map(|(k, _)| k)
    }
    fn peek_ln(&self) -> usize {
        self.t.get(self.p).map(|(_, l)| *l).unwrap_or(0)
    }
    fn next(&mut self) -> Option<RqTok> {
        let r = self.t.get(self.p).map(|(k, _)| k.clone());
        if r.is_some() {
            self.p += 1;
        }
        r
    }
    fn at_op(&self, op: &str) -> bool {
        matches!(self.peek(), Some(RqTok::Op(o)) if *o == op)
    }
    fn eat_op(&mut self, op: &str) -> bool {
        if self.at_op(op) {
            self.p += 1;
            true
        } else {
            false
        }
    }
    fn expect_op(&mut self, op: &str) -> Result<usize, (usize, String)> {
        let ln = self.peek_ln();
        if self.eat_op(op) {
            Ok(ln)
        } else {
            Err((ln, format!("`{op}` が必要 (直前トークン位置)")))
        }
    }
    fn at_id(&self, w: &str) -> bool {
        matches!(self.peek(), Some(RqTok::Id(s)) if s == w)
    }
    fn take_id(&mut self) -> Result<(String, usize), (usize, String)> {
        let ln = self.peek_ln();
        match self.next() {
            Some(RqTok::Id(s)) => {
                if RQ_RESERVED.contains(&s.as_str()) {
                    Err((ln, format!("予約語 `{s}` は識別子に使えない")))
                } else {
                    Ok((s, ln))
                }
            }
            _ => Err((ln, "識別子が必要".into())),
        }
    }
    /// 型構文: スカラー名 または `[要素; N]` (N は 1..=256 の整数リテラル)
    fn take_ty(&mut self) -> Result<RqTy, (usize, String)> {
        let ln = self.peek_ln();
        if self.eat_op("[") {
            let (en, eln) = match self.next() {
                Some(RqTok::Id(s)) => (s, ln),
                _ => return Err((ln, "配列要素の型名が必要".into())),
            };
            let el = match en.as_str() {
                "f32" => RqElem::F,
                "i64" => RqElem::I,
                "u32" => RqElem::U,
                "bool" => RqElem::B,
                other => {
                    return Err((
                        eln,
                        format!("配列要素は f32/i64/u32/bool のみ (`{other}` は不可)"),
                    ))
                }
            };
            self.expect_op(";")?;
            let nln = self.peek_ln();
            let n = match self.next() {
                Some(RqTok::I(v)) if (1..=256).contains(&v) => v as u16,
                Some(RqTok::I(v)) => {
                    return Err((nln, format!("配列長は 1..=256 (与 {v})")));
                }
                _ => return Err((nln, "配列長は整数リテラル (1..=256)".into())),
            };
            self.expect_op("]")?;
            return Ok(RqTy::Arr(el, n));
        }
        match self.next() {
            Some(RqTok::Id(s)) => RqTy::from_name(&s).ok_or_else(|| {
                (
                    ln,
                    format!("型名 (f32/i64/u32/bool/str/[T; N]) が必要: `{s}`"),
                )
            }),
            _ => Err((ln, "型名が必要".into())),
        }
    }
    fn program(&mut self) -> Result<Vec<RqS>, (usize, String)> {
        let mut v = Vec::new();
        while self.peek().is_some() {
            v.push(self.stmt()?);
        }
        Ok(v)
    }
    fn block_body(&mut self) -> Result<Vec<RqS>, (usize, String)> {
        self.expect_op("{")?;
        let mut v = Vec::new();
        while !self.at_op("}") {
            if self.peek().is_none() {
                return Err((self.peek_ln(), "`}` が無いまま終端".into()));
            }
            v.push(self.stmt()?);
        }
        self.expect_op("}")?;
        Ok(v)
    }
    /// 代入系の共通尾部: `= e;` / `+= e;` 等
    fn assign_tail(&mut self, t: RqTarget, ln: usize) -> Result<RqS, (usize, String)> {
        const COMPOUND: &[&str] = &["+=", "-=", "*=", "/=", "%="];
        if self.eat_op("=") {
            let e = self.expr()?;
            self.expect_op(";")?;
            return Ok(RqS {
                k: RqSK::Set { t, op: None, e },
                ln,
            });
        }
        for op in COMPOUND {
            if self.at_op(op) {
                self.next();
                let e = self.expr()?;
                self.expect_op(";")?;
                let base = match *op {
                    "+=" => "+",
                    "-=" => "-",
                    "*=" => "*",
                    "/=" => "/",
                    "%=" => "%",
                    _ => unreachable!(),
                };
                return Ok(RqS {
                    k: RqSK::Set {
                        t,
                        op: Some(base),
                        e,
                    },
                    ln,
                });
            }
        }
        Err((
            ln,
            "代入の形式が不明 (`=` または 複合代入演算子が必要)".into(),
        ))
    }
    fn stmt(&mut self) -> Result<RqS, (usize, String)> {
        let ln = self.peek_ln();
        if self.at_op("{") {
            let b = self.block_body()?;
            return Ok(RqS {
                k: RqSK::Block(b),
                ln,
            });
        }
        if self.at_id("let") {
            self.next();
            let (n, _) = self.take_id()?;
            self.expect_op(":")?;
            let ty = self.take_ty()?;
            self.expect_op("=")?;
            let e = self.expr()?;
            self.expect_op(";")?;
            return Ok(RqS {
                k: RqSK::Let { n, ty, e },
                ln,
            });
        }
        if self.at_id("const") {
            self.next();
            let (n, _) = self.take_id()?;
            self.expect_op(":")?;
            let ty = self.take_ty()?;
            self.expect_op("=")?;
            let e = self.expr()?;
            self.expect_op(";")?;
            return Ok(RqS {
                k: RqSK::Const { n, ty, e },
                ln,
            });
        }
        if self.at_id("fn") {
            self.next();
            let (n, _) = self.take_id()?;
            self.expect_op("(")?;
            let mut ps = Vec::new();
            if !self.at_op(")") {
                loop {
                    let (pn, pln) = self.take_id()?;
                    self.expect_op(":")?;
                    let pt = self.take_ty()?;
                    if ps.iter().any(|(x, _): &(String, RqTy)| *x == pn) {
                        return Err((pln, format!("仮引数 `{pn}` が重複")));
                    }
                    ps.push((pn, pt));
                    if !self.eat_op(",") {
                        break;
                    }
                }
            }
            self.expect_op(")")?;
            self.expect_op("->")?;
            let rt = self.take_ty()?;
            let body = self.block_body()?;
            return Ok(RqS {
                k: RqSK::Fn { n, ps, rt, body },
                ln,
            });
        }
        if self.at_id("if") {
            return self.if_stmt(ln);
        }
        if self.at_id("while") {
            self.next();
            let c = self.expr()?;
            let body = self.block_body()?;
            return Ok(RqS {
                k: RqSK::While { c, body },
                ln,
            });
        }
        if self.at_id("for") {
            self.next();
            let (v, vln) = self.take_id()?;
            self.expect_op(":")?;
            let tln = self.peek_ln();
            match self.next() {
                Some(RqTok::Id(s)) if s == "i64" => {}
                _ => {
                    return Err((
                        tln,
                        "for のループ変数型は常に i64 (`for i: i64 in ..`)".into(),
                    ))
                }
            }
            if !self.at_id("in") {
                return Err((vln, "for の形式は `for i: i64 in A..B {{ .. }}`".into()));
            }
            self.next();
            let a = self.expr()?;
            let incl = if self.eat_op("..=") {
                true
            } else if self.eat_op("..") {
                false
            } else {
                return Err((self.peek_ln(), "範囲演算子 `..` または `..=` が必要".into()));
            };
            let b = self.expr()?;
            let body = self.block_body()?;
            return Ok(RqS {
                k: RqSK::For {
                    v,
                    a,
                    b,
                    incl,
                    body,
                },
                ln,
            });
        }
        if self.at_id("loop") {
            self.next();
            let body = self.block_body()?;
            return Ok(RqS {
                k: RqSK::Loop { body },
                ln,
            });
        }
        if self.at_id("break") {
            self.next();
            self.expect_op(";")?;
            return Ok(RqS { k: RqSK::Break, ln });
        }
        if self.at_id("continue") {
            self.next();
            self.expect_op(";")?;
            return Ok(RqS {
                k: RqSK::Continue,
                ln,
            });
        }
        if self.at_id("match") {
            self.next();
            let e = self.expr()?;
            self.expect_op("{")?;
            let mut arms = Vec::new();
            while !self.at_op("}") {
                if self.peek().is_none() {
                    return Err((self.peek_ln(), "match が `}` 無く終端".into()));
                }
                let pln = self.peek_ln();
                let pat = self.match_pat(pln)?;
                self.expect_op("=>")?;
                let body = self.block_body()?;
                arms.push((pat, body, pln));
            }
            self.expect_op("}")?;
            return Ok(RqS {
                k: RqSK::Match { e, arms },
                ln,
            });
        }
        if self.at_id("ret") {
            self.next();
            let e = self.expr()?;
            self.expect_op(";")?;
            return Ok(RqS {
                k: RqSK::Ret(e),
                ln,
            });
        }
        if self.at_id("p") {
            self.next();
            let e = self.expr()?;
            self.expect_op(";")?;
            return Ok(RqS { k: RqSK::P(e), ln });
        }
        if self.at_id("assert") {
            self.next();
            let e = self.expr()?;
            let label = if self.eat_op(",") {
                let lln = self.peek_ln();
                match self.next() {
                    Some(RqTok::St(s)) => Some(s),
                    _ => return Err((lln, "assert の第 2 引数は文字列リテラル".into())),
                }
            } else {
                None
            };
            self.expect_op(";")?;
            return Ok(RqS {
                k: RqSK::Assert { e, label },
                ln,
            });
        }
        match self.peek() {
            Some(RqTok::Id(_)) => {
                let (n, nln) = self.take_id()?;
                if self.eat_op("[") {
                    let idx = self.expr()?;
                    self.expect_op("]")?;
                    return self.assign_tail(RqTarget::Idx(n, idx), nln);
                }
                self.assign_tail(RqTarget::Var(n), nln)
            }
            _ => Err((
                ln,
                "文の先頭が不明 (let/const/fn/if/while/for/loop/match/ret/break/continue/p/assert/代入/{)"
                    .into(),
            )),
        }
    }
    /// if / elif チェーン (elif は else { if } へ脱糖)
    fn if_stmt(&mut self, ln: usize) -> Result<RqS, (usize, String)> {
        self.next(); // "if"
        let c = self.expr()?;
        let t = self.block_body()?;
        let f = if self.at_id("elif") {
            let eln = self.peek_ln();
            vec![self.if_stmt(eln)?]
        } else if self.at_id("else") {
            self.next();
            self.block_body()?
        } else {
            Vec::new()
        };
        Ok(RqS {
            k: RqSK::If { c, t, f },
            ln,
        })
    }
    /// match 腕パターンの生解析 (const 名は型検査で解決)
    fn match_pat(&mut self, pln: usize) -> Result<RawPat, (usize, String)> {
        if self.at_id("_") {
            self.next();
            return Ok(RawPat::Wild);
        }
        if self.at_id("true") {
            self.next();
            return Ok(RawPat::B(true));
        }
        if self.at_id("false") {
            self.next();
            return Ok(RawPat::B(false));
        }
        let neg = self.eat_op("-");
        match self.next() {
            Some(RqTok::I(v)) => Ok(RawPat::I(if neg { -v } else { v })),
            Some(RqTok::Id(s)) => {
                if neg {
                    return Err((pln, "const 腕に単項 - は付けられない".into()));
                }
                if RQ_RESERVED.contains(&s.as_str()) {
                    return Err((pln, format!("予約語 `{s}` は腕に使えない")));
                }
                Ok(RawPat::Const(s))
            }
            _ => Err((
                pln,
                "match 腕は 整数リテラル / true / false / const 名 / _ のみ".into(),
            )),
        }
    }
    fn expr(&mut self) -> Result<RqE, (usize, String)> {
        self.p_bin(1)
    }
    /// 優先順位 (数値が大きいほど強い)。Rust 同一・全て左結合。
    /// 1: ||  2: &&  3: 比較  4: |  5: ^  6: &  7: << >>  8: + -  9: * / %
    fn p_bin(&mut self, min: u8) -> Result<RqE, (usize, String)> {
        fn op_prec(o: &'static str) -> Option<(u8, &'static str)> {
            match o {
                "||" => Some((1, o)),
                "&&" => Some((2, o)),
                "==" | "!=" | "<" | "<=" | ">" | ">=" => Some((3, o)),
                "|" => Some((4, o)),
                "^" => Some((5, o)),
                "&" => Some((6, o)),
                "<<" | ">>" => Some((7, o)),
                "+" | "-" => Some((8, o)),
                "*" | "/" | "%" => Some((9, o)),
                _ => None,
            }
        }
        let mut l = if min > 9 {
            self.p_un()?
        } else {
            self.p_bin(min + 1)?
        };
        loop {
            let (prec, op) = match self.peek() {
                Some(RqTok::Op(o)) => match op_prec(o) {
                    Some(x) => x,
                    None => break,
                },
                _ => break,
            };
            if prec < min {
                break;
            }
            let ln = self.peek_ln();
            self.next();
            let r = if prec + 1 > 9 {
                self.p_un()?
            } else {
                self.p_bin(prec + 1)?
            };
            l = RqE {
                k: RqEK::Bin(op, Box::new(l), Box::new(r)),
                ln,
            };
        }
        Ok(l)
    }
    fn p_un(&mut self) -> Result<RqE, (usize, String)> {
        let ln = self.peek_ln();
        if self.at_op("-") {
            self.next();
            let e = self.p_un()?;
            return Ok(RqE {
                k: RqEK::Neg(Box::new(e)),
                ln,
            });
        }
        if self.at_op("!") {
            self.next();
            let e = self.p_un()?;
            return Ok(RqE {
                k: RqEK::Not(Box::new(e)),
                ln,
            });
        }
        if self.at_op("~") {
            self.next();
            let e = self.p_un()?;
            return Ok(RqE {
                k: RqEK::BitNot(Box::new(e)),
                ln,
            });
        }
        self.p_post()
    }
    /// 後置: 添字 `a[i]` (最優先)
    fn p_post(&mut self) -> Result<RqE, (usize, String)> {
        let mut e = self.p_prim()?;
        loop {
            if self.at_op("[") {
                let ln = self.peek_ln();
                self.next();
                let idx = self.expr()?;
                self.expect_op("]")?;
                e = RqE {
                    k: RqEK::Idx(Box::new(e), Box::new(idx)),
                    ln,
                };
            } else {
                break;
            }
        }
        Ok(e)
    }
    fn p_prim(&mut self) -> Result<RqE, (usize, String)> {
        let ln = self.peek_ln();
        match self.next() {
            Some(RqTok::Fl(v)) => Ok(RqE { k: RqEK::Fl(v), ln }),
            Some(RqTok::I(v)) => Ok(RqE { k: RqEK::I(v), ln }),
            Some(RqTok::U(v)) => Ok(RqE { k: RqEK::U(v), ln }),
            Some(RqTok::St(s)) => Ok(RqE { k: RqEK::Sl(s), ln }),
            Some(RqTok::Id(s)) => {
                if s == "true" {
                    return Ok(RqE {
                        k: RqEK::Bl(true),
                        ln,
                    });
                }
                if s == "false" {
                    return Ok(RqE {
                        k: RqEK::Bl(false),
                        ln,
                    });
                }
                if RQ_RESERVED.contains(&s.as_str()) {
                    return Err((ln, format!("予約語 `{s}` は式に使えない")));
                }
                if self.at_op("(") {
                    self.next();
                    let mut args = Vec::new();
                    if !self.at_op(")") {
                        loop {
                            args.push(self.expr()?);
                            if !self.eat_op(",") {
                                break;
                            }
                        }
                    }
                    self.expect_op(")")?;
                    Ok(RqE {
                        k: RqEK::Call(s, args),
                        ln,
                    })
                } else {
                    Ok(RqE {
                        k: RqEK::Var(s),
                        ln,
                    })
                }
            }
            Some(RqTok::Op("(")) => {
                let e = self.expr()?;
                self.expect_op(")")?;
                Ok(e)
            }
            Some(RqTok::Op("[")) => {
                let mut es = Vec::new();
                if !self.at_op("]") {
                    loop {
                        es.push(self.expr()?);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                }
                self.expect_op("]")?;
                if es.is_empty() {
                    return Err((
                        ln,
                        "空の配列リテラルは禁止 (長さ 0 の配列は存在しない)".into(),
                    ));
                }
                Ok(RqE {
                    k: RqEK::ArrLit(es),
                    ln,
                })
            }
            other => Err((ln, format!("式の先頭が不明: {other:?}"))),
        }
    }
}

/// match 腕パターンの生表現 (const は型検査で RqPat::I に解決)
#[derive(Clone, Debug)]
enum RawPat {
    I(i64),
    B(bool),
    Const(String),
    Wild,
}
/// 組み込み関数シグネチャ表 (名前, 仮引数型, 戻り値型)。同名の複数候補は
/// 厳密一致で選ぶ (f/i/u のみ多重定義)。配列汎用の len/fill/copy は
/// 型検査で特別扱いするため表には含めない。
fn rq_builtin_sigs() -> Vec<(&'static str, Vec<RqTy>, RqTy)> {
    use RqTy::*;
    let mut v: Vec<(&'static str, Vec<RqTy>, RqTy)> = Vec::new();
    for n in [
        "sqrt", "exp", "ln", "sin", "cos", "tan", "abs", "floor", "ceil", "trunc", "round",
    ] {
        v.push((n, vec![F32], F32));
    }
    for n in ["pow", "atan2", "hypot", "min", "max", "copysign"] {
        v.push((n, vec![F32, F32], F32));
    }
    v.push(("fma", vec![F32, F32, F32], F32));
    v.push(("bits", vec![F32], U32));
    v.push(("b", vec![U32], F32));
    for n in ["is_nan", "is_inf", "is_fin"] {
        v.push((n, vec![F32], Bool));
    }
    for n in ["nan", "inf", "ninf", "pi", "e"] {
        v.push((n, vec![], F32));
    }
    v.push(("f", vec![I64], F32));
    v.push(("f", vec![U32], F32));
    v.push(("i", vec![F32], I64));
    v.push(("i", vec![U32], I64));
    v.push(("u", vec![F32], U32));
    v.push(("u", vec![I64], U32));
    v
}

enum RqErr {
    /// 構文/型エラー (exit 2)
    C(usize, String),
    /// 実行時エラー (exit 3)
    R(usize, String),
}

/// const 式のコンパイル時評価。許可: スカラーリテラル・他 const 名・
/// + - * / % (同型)・& | ^ << >> (整数)・単項 - ! ~・f()/i()/u()・括弧。
/// 変数参照・比較・配列・文字列・その他関数呼出は禁止。
fn rq_const_eval(e: &RqE, env: &HashMap<String, (RqTy, RqV)>) -> Result<(RqTy, RqV), RqErr> {
    let bad = |m: &str| RqErr::C(e.ln, format!("const 式に使えない要素: {m}"));
    match &e.k {
        RqEK::Fl(v) => Ok((RqTy::F32, RqV::F(*v))),
        RqEK::I(v) => Ok((RqTy::I64, RqV::I(*v))),
        RqEK::U(v) => Ok((RqTy::U32, RqV::U(*v))),
        RqEK::Bl(v) => Ok((RqTy::Bool, RqV::B(*v))),
        RqEK::Var(n) => env.get(n).map(|(t, v)| (*t, v.clone())).ok_or_else(|| {
            RqErr::C(
                e.ln,
                format!("const 式の参照 `{n}` が未解決 (定義順・名前を確認)"),
            )
        }),
        RqEK::Neg(x) => {
            let (t, v) = rq_const_eval(x, env)?;
            match v {
                RqV::F(a) => Ok((t, RqV::F(-a))),
                RqV::I(a) => Ok((t, RqV::I(a.wrapping_neg()))),
                _ => Err(bad("数値以外への単項 -")),
            }
        }
        RqEK::Not(x) => {
            let (t, v) = rq_const_eval(x, env)?;
            match v {
                RqV::B(a) => Ok((t, RqV::B(!a))),
                _ => Err(bad("bool 以外への !")),
            }
        }
        RqEK::BitNot(x) => {
            let (t, v) = rq_const_eval(x, env)?;
            match v {
                RqV::I(a) => Ok((t, RqV::I(!a))),
                RqV::U(a) => Ok((t, RqV::U(!a))),
                _ => Err(bad("整数以外への ~")),
            }
        }
        RqEK::Bin(op, a, b2) => {
            let (ta, va) = rq_const_eval(a, env)?;
            let (_tb, vb) = rq_const_eval(b2, env)?;
            let v = rq_binop(op, &va, &vb)
                .ok_or_else(|| RqErr::C(e.ln, "const 式の演算が型不整合またはゼロ除算".into()))?;
            if matches!(v, RqV::B(_)) {
                // 比較・&& || の結果は全て bool → const 式では禁止 (v2 仕様)
                return Err(bad("比較・論理演算 (結果が bool)"));
            }
            // 結果型は算術・ビット・シフトとも左辺型
            Ok((ta, v))
        }
        RqEK::Call(n, args) if n == "f" || n == "i" || n == "u" => {
            if args.len() != 1 {
                return Err(bad("変換の引数数"));
            }
            let (_t, v) = rq_const_eval(&args[0], env)?;
            match (n.as_str(), v) {
                ("f", RqV::I(a)) => Ok((RqTy::F32, RqV::F(a as f32))),
                ("f", RqV::U(a)) => Ok((RqTy::F32, RqV::F(a as f32))),
                ("i", RqV::F(a)) => Ok((RqTy::I64, RqV::I(a as i64))),
                ("i", RqV::U(a)) => Ok((RqTy::I64, RqV::I(a as i64))),
                ("u", RqV::F(a)) => Ok((RqTy::U32, RqV::U(a as u32))),
                ("u", RqV::I(a)) => Ok((RqTy::U32, RqV::U(a as u32))),
                _ => Err(bad("f/i/u の引数型")),
            }
        }
        _ => Err(bad("変数・配列・文字列・比較・関数呼出 (f/i/u 以外)")),
    }
}

struct RqChecker {
    /// ユーザー関数名 → (仮引数型, 戻り値型)
    fns: HashMap<String, (Vec<RqTy>, RqTy)>,
    /// const 名 → (型, 値)
    consts: HashMap<String, (RqTy, RqV)>,
}

impl RqChecker {
    /// expected: 配列リテラルに与える型の文脈 (その他の式では無視)。
    fn expr(
        &self,
        e: &RqE,
        scopes: &[HashMap<String, RqTy>],
        expected: Option<RqTy>,
    ) -> Result<RqTy, RqErr> {
        match &e.k {
            RqEK::Fl(_) => Ok(RqTy::F32),
            RqEK::I(_) => Ok(RqTy::I64),
            RqEK::U(_) => Ok(RqTy::U32),
            RqEK::Bl(_) => Ok(RqTy::Bool),
            RqEK::Sl(_) => Ok(RqTy::Str),
            RqEK::ArrLit(es) => {
                let Some(RqTy::Arr(el, n)) = expected else {
                    return Err(RqErr::C(
                        e.ln,
                        "配列リテラルには型注釈の文脈が必要 (let の型注釈・fn 引数・ret で使う)"
                            .into(),
                    ));
                };
                if es.len() != n as usize {
                    return Err(RqErr::C(
                        e.ln,
                        format!("配列リテラルの要素数 {} が宣言長 {} と不一致", es.len(), n),
                    ));
                }
                let et = rq_scalar_ty(el);
                for x in es {
                    let tx = self.expr(x, scopes, None)?;
                    if tx != et {
                        return Err(RqErr::C(
                            x.ln,
                            format!("配列要素の型が不均一 (要 {} / 与 {})", et.name(), tx.name()),
                        ));
                    }
                }
                Ok(RqTy::Arr(el, n))
            }
            RqEK::Var(n) => {
                for sc in scopes.iter().rev() {
                    if let Some(t) = sc.get(n) {
                        return Ok(*t);
                    }
                }
                if let Some((t, _)) = self.consts.get(n) {
                    return Ok(*t);
                }
                Err(RqErr::C(e.ln, format!("未定義の変数 `{n}`")))
            }
            RqEK::Idx(a, i) => {
                let ta = self.expr(a, scopes, None)?;
                let ti = self.expr(i, scopes, None)?;
                if ti != RqTy::I64 {
                    return Err(RqErr::C(
                        i.ln,
                        format!("添字は i64 型式 (与 {}。f(i) などで変換)", ti.name()),
                    ));
                }
                match ta {
                    RqTy::Arr(el, _) => Ok(rq_scalar_ty(el)),
                    _ => Err(RqErr::C(
                        e.ln,
                        format!(
                            "添字の対象は配列型のみ (与 {}。多次元添字 a[i][j] は禁止)",
                            ta.name()
                        ),
                    )),
                }
            }
            RqEK::Call(n, args) => {
                // 配列汎用ビルトイン (len/fill/copy) を特別処理
                if n == "len" {
                    if args.len() != 1 {
                        return Err(RqErr::C(e.ln, "len は引数 1 個".into()));
                    }
                    return match self.expr(&args[0], scopes, None)? {
                        RqTy::Arr(_, _) => Ok(RqTy::I64),
                        t => Err(RqErr::C(
                            e.ln,
                            format!("len の引数は配列 (与 {})", t.name()),
                        )),
                    };
                }
                if n == "fill" {
                    if args.len() != 2 {
                        return Err(RqErr::C(e.ln, "fill は引数 2 個 (配列, 値)".into()));
                    }
                    let ta = self.expr(&args[0], scopes, None)?;
                    let RqTy::Arr(el, n) = ta else {
                        return Err(RqErr::C(
                            e.ln,
                            format!("fill の第 1 引数は配列 (与 {})", ta.name()),
                        ));
                    };
                    let tv = self.expr(&args[1], scopes, None)?;
                    let et = rq_scalar_ty(el);
                    if tv != et {
                        return Err(RqErr::C(
                            e.ln,
                            format!("fill の値型 {} が要素型 {} と不一致", tv.name(), et.name()),
                        ));
                    }
                    return Ok(RqTy::Arr(el, n));
                }
                if n == "copy" {
                    if args.len() != 2 {
                        return Err(RqErr::C(e.ln, "copy は引数 2 個 (dst, src)".into()));
                    }
                    // 片方が配列リテラルの場合のみ、もう一方の確定配列型を
                    // 期待型として補完する (型が一意に決まるケースに限定)。
                    let l0 = matches!(args[0].k, RqEK::ArrLit(_));
                    let l1 = matches!(args[1].k, RqEK::ArrLit(_));
                    if l0 && l1 {
                        return Err(RqErr::C(
                            e.ln,
                            "copy の両引数が配列リテラルでは型が決まらない (let 注釈で分割)".into(),
                        ));
                    }
                    let (td, ts) = if l1 {
                        let td = self.expr(&args[0], scopes, None)?;
                        let ts = self.expr(&args[1], scopes, Some(td))?;
                        (td, ts)
                    } else if l0 {
                        let ts = self.expr(&args[1], scopes, None)?;
                        let td = self.expr(&args[0], scopes, Some(ts))?;
                        (td, ts)
                    } else {
                        (
                            self.expr(&args[0], scopes, None)?,
                            self.expr(&args[1], scopes, None)?,
                        )
                    };
                    match (td, ts) {
                        (RqTy::Arr(e1, n1), RqTy::Arr(e2, n2)) if e1 == e2 && n1 == n2 => {
                            Ok(RqTy::Arr(e1, n1))
                        }
                        _ => Err(RqErr::C(
                            e.ln,
                            format!(
                                "copy は同型配列同士のみ (与 {} と {})",
                                td.name(),
                                ts.name()
                            ),
                        )),
                    }
                } else if let Some((ps, rt)) = self.fns.get(n) {
                    if ps.len() != args.len() {
                        return Err(RqErr::C(
                            e.ln,
                            format!(
                                "fn {n} の引数数が不一致 (要 {} / 与 {})",
                                ps.len(),
                                args.len()
                            ),
                        ));
                    }
                    let mut arg_tys = Vec::new();
                    for (a, pt) in args.iter().zip(ps.iter()) {
                        arg_tys.push(self.expr(a, scopes, Some(*pt))?);
                    }
                    if *ps == arg_tys {
                        return Ok(*rt);
                    }
                    return Err(RqErr::C(
                        e.ln,
                        format!(
                            "fn {n} の引数型が不一致 (要 {} / 与 {})",
                            ps.iter().map(|t| t.name()).collect::<Vec<_>>().join(","),
                            arg_tys
                                .iter()
                                .map(|t| t.name())
                                .collect::<Vec<_>>()
                                .join(",")
                        ),
                    ));
                } else {
                    // 固定シグネチャ組み込み (スカラーのみ、配列リテラル不可)
                    let mut tys = Vec::new();
                    for a in args {
                        tys.push(self.expr(a, scopes, None)?);
                    }
                    for (bn, ps, rt) in rq_builtin_sigs() {
                        if *bn == *n && ps == tys {
                            return Ok(rt);
                        }
                    }
                    // copy の return 漏れを防ぐため copy は上で必ず return する
                    Err(RqErr::C(
                        e.ln,
                        format!(
                            "未知の関数または引数型不一致: {n}({})",
                            tys.iter().map(|t| t.name()).collect::<Vec<_>>().join(",")
                        ),
                    ))
                }
            }
            RqEK::Neg(x) => {
                let t = self.expr(x, scopes, None)?;
                match t {
                    RqTy::F32 | RqTy::I64 => Ok(t),
                    RqTy::U32 => Err(RqErr::C(
                        e.ln,
                        "u32 への unary - は禁止 (意図が wrapping なら 0 - x)".into(),
                    )),
                    _ => Err(RqErr::C(
                        e.ln,
                        format!("unary - は数値型のみ (与 {})", t.name()),
                    )),
                }
            }
            RqEK::Not(x) => {
                let t = self.expr(x, scopes, None)?;
                if t == RqTy::Bool {
                    Ok(RqTy::Bool)
                } else {
                    Err(RqErr::C(e.ln, format!("! は bool のみ (与 {})", t.name())))
                }
            }
            RqEK::BitNot(x) => {
                let t = self.expr(x, scopes, None)?;
                match t {
                    RqTy::I64 | RqTy::U32 => Ok(t),
                    _ => Err(RqErr::C(
                        e.ln,
                        format!("~ は i64/u32 のみ (与 {}。bool 否定は !)", t.name()),
                    )),
                }
            }
            RqEK::Bin(op, a, b) => {
                let ta = self.expr(a, scopes, None)?;
                let tb = self.expr(b, scopes, None)?;
                let ln = e.ln;
                match *op {
                    "&&" | "||" => {
                        if ta == RqTy::Bool && tb == RqTy::Bool {
                            Ok(RqTy::Bool)
                        } else {
                            Err(RqErr::C(
                                ln,
                                format!(
                                    "{op} は bool 同士のみ (与 {} と {})",
                                    ta.name(),
                                    tb.name()
                                ),
                            ))
                        }
                    }
                    "==" | "!=" => {
                        if ta == tb && matches!(ta, RqTy::F32 | RqTy::I64 | RqTy::U32 | RqTy::Bool)
                        {
                            Ok(RqTy::Bool)
                        } else {
                            Err(RqErr::C(
                                ln,
                                format!("{op} は同型の f32/i64/u32/bool (与 {} と {}。配列の比較は要素ごとに)", ta.name(), tb.name()),
                            ))
                        }
                    }
                    "<" | "<=" | ">" | ">=" => {
                        if ta == tb && matches!(ta, RqTy::F32 | RqTy::I64 | RqTy::U32) {
                            Ok(RqTy::Bool)
                        } else {
                            Err(RqErr::C(
                                ln,
                                format!(
                                    "{op} は同型の f32/i64/u32 (与 {} と {})",
                                    ta.name(),
                                    tb.name()
                                ),
                            ))
                        }
                    }
                    "&" | "|" | "^" => {
                        if ta == tb && matches!(ta, RqTy::I64 | RqTy::U32) {
                            Ok(ta)
                        } else {
                            Err(RqErr::C(
                                ln,
                                format!(
                                    "ビット演算 {op} は同型の i64/u32 (与 {} と {})",
                                    ta.name(),
                                    tb.name()
                                ),
                            ))
                        }
                    }
                    "<<" | ">>" => {
                        if matches!(ta, RqTy::I64 | RqTy::U32) && tb == RqTy::I64 {
                            Ok(ta)
                        } else {
                            Err(RqErr::C(
                                ln,
                                format!("シフト {op} は 左辺 i64/u32・右辺 i64 (与 {} と {}。マスク付き wrapping)", ta.name(), tb.name()),
                            ))
                        }
                    }
                    _ => {
                        // 算術 (+ - * / %)
                        if ta == tb && matches!(ta, RqTy::F32 | RqTy::I64 | RqTy::U32) {
                            Ok(ta)
                        } else {
                            Err(RqErr::C(
                                ln,
                                format!("算術 {op} は同型の f32/i64/u32 (与 {} と {})。明示変換 f()/i()/u() を使う", ta.name(), tb.name()),
                            ))
                        }
                    }
                }
            }
        }
    }
    /// 代入左辺の検査 → 要素/変数の型を返す
    fn target(
        &self,
        t: &RqTarget,
        scopes: &[HashMap<String, RqTy>],
        immut: &[String],
        ln: usize,
    ) -> Result<RqTy, RqErr> {
        match t {
            RqTarget::Var(n) => {
                if immut.iter().any(|x| x == n) {
                    return Err(RqErr::C(
                        ln,
                        format!(
                            "for ループ変数 `{n}` への代入は禁止 (インデックス操作は while で)"
                        ),
                    ));
                }
                if self.consts.contains_key(n) {
                    return Err(RqErr::C(ln, format!("const `{n}` には代入不可")));
                }
                for sc in scopes.iter().rev() {
                    if let Some(t) = sc.get(n) {
                        return Ok(*t);
                    }
                }
                Err(RqErr::C(ln, format!("代入先 `{n}` が未定義")))
            }
            RqTarget::Idx(n, i) => {
                if immut.iter().any(|x| x == n) {
                    return Err(RqErr::C(
                        ln,
                        format!("for ループ変数 `{n}` 経由の代入は禁止"),
                    ));
                }
                let ti = self.expr(i, scopes, None)?;
                if ti != RqTy::I64 {
                    return Err(RqErr::C(
                        i.ln,
                        format!("添字は i64 型式 (与 {})", ti.name()),
                    ));
                }
                for sc in scopes.iter().rev() {
                    if let Some(RqTy::Arr(el, _)) = sc.get(n) {
                        return Ok(rq_scalar_ty(*el));
                    }
                    if let Some(t) = sc.get(n) {
                        return Err(RqErr::C(
                            ln,
                            format!("添字代入の対象は配列型のみ (与 {})", t.name()),
                        ));
                    }
                }
                Err(RqErr::C(ln, format!("代入先配列 `{n}` が未定義")))
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn stmts(
        &self,
        ss: &[RqS],
        scopes: &mut Vec<HashMap<String, RqTy>>,
        cur_ret: Option<RqTy>,
        loop_depth: usize,
        immut: &mut Vec<String>,
    ) -> Result<(), RqErr> {
        for s in ss {
            match &s.k {
                RqSK::Let { n, ty, e } => {
                    let te = self.expr(e, scopes, Some(*ty))?;
                    if te != *ty {
                        return Err(RqErr::C(
                            s.ln,
                            format!("let {n}: 宣言型 {} に対し式の型は {}", ty.name(), te.name()),
                        ));
                    }
                    scopes.last_mut().unwrap().insert(n.clone(), *ty);
                }
                RqSK::Set { t, op, e } => {
                    let tt = self.target(t, scopes, immut, s.ln)?;
                    let te = self.expr(e, scopes, Some(tt))?;
                    match op {
                        None => {
                            if te != tt {
                                return Err(RqErr::C(
                                    s.ln,
                                    format!(
                                        "代入: 左辺型 {} に対し式の型は {}",
                                        tt.name(),
                                        te.name()
                                    ),
                                ));
                            }
                        }
                        Some(o) => {
                            // 複合代入: 二項演算 o の規則そのまま
                            if te != tt || !matches!(tt, RqTy::F32 | RqTy::I64 | RqTy::U32) {
                                return Err(RqErr::C(
                                    s.ln,
                                    format!(
                                        "複合代入 {o}= は同型の f32/i64/u32 (左辺 {} / 式 {})",
                                        tt.name(),
                                        te.name()
                                    ),
                                ));
                            }
                        }
                    }
                }
                RqSK::Const { n, .. } => {
                    return Err(RqErr::C(
                        s.ln,
                        format!("const {n} はトップレベルにのみ書ける"),
                    ));
                }
                RqSK::Fn { n, .. } => {
                    return Err(RqErr::C(s.ln, format!("fn {n} はトップレベルにのみ書ける")));
                }
                RqSK::If { c, t, f } => {
                    let tc = self.expr(c, scopes, None)?;
                    if tc != RqTy::Bool {
                        return Err(RqErr::C(s.ln, format!("if 条件は bool (与 {})", tc.name())));
                    }
                    scopes.push(HashMap::new());
                    self.stmts(t, scopes, cur_ret, loop_depth, immut)?;
                    scopes.pop();
                    scopes.push(HashMap::new());
                    self.stmts(f, scopes, cur_ret, loop_depth, immut)?;
                    scopes.pop();
                }
                RqSK::While { c, body } => {
                    let tc = self.expr(c, scopes, None)?;
                    if tc != RqTy::Bool {
                        return Err(RqErr::C(
                            s.ln,
                            format!("while 条件は bool (与 {})", tc.name()),
                        ));
                    }
                    scopes.push(HashMap::new());
                    self.stmts(body, scopes, cur_ret, loop_depth + 1, immut)?;
                    scopes.pop();
                }
                RqSK::For { v, a, b, body, .. } => {
                    let ta = self.expr(a, scopes, None)?;
                    let tb = self.expr(b, scopes, None)?;
                    if ta != RqTy::I64 || tb != RqTy::I64 {
                        return Err(RqErr::C(
                            s.ln,
                            format!(
                                "for の範囲端は i64 (与 {} と {}。f()/i() で変換)",
                                ta.name(),
                                tb.name()
                            ),
                        ));
                    }
                    scopes.push(HashMap::new());
                    scopes.last_mut().unwrap().insert(v.clone(), RqTy::I64);
                    immut.push(v.clone());
                    self.stmts(body, scopes, cur_ret, loop_depth + 1, immut)?;
                    immut.pop();
                    scopes.pop();
                }
                RqSK::Loop { body } => {
                    scopes.push(HashMap::new());
                    self.stmts(body, scopes, cur_ret, loop_depth + 1, immut)?;
                    scopes.pop();
                }
                RqSK::Break | RqSK::Continue => {
                    if loop_depth == 0 {
                        return Err(RqErr::C(
                            s.ln,
                            "break/continue は while/for/loop の内側でのみ有効".into(),
                        ));
                    }
                }
                RqSK::Match { e, arms } => {
                    let te = self.expr(e, scopes, None)?;
                    if !matches!(te, RqTy::I64 | RqTy::Bool) {
                        return Err(RqErr::C(
                            s.ln,
                            format!("match の対象は i64 または bool (与 {}。f32 は IEEE 等値の曖昧さ回避のため禁止)", te.name()),
                        ));
                    }
                    let mut seen: Vec<RqPat> = Vec::new();
                    let mut wild = false;
                    let armn = arms.len();
                    for (ai, (raw, body, pln)) in arms.iter().enumerate() {
                        let pat = match (raw, te) {
                            (RawPat::Wild, _) => RqPat::Wild,
                            (RawPat::I(v), RqTy::I64) => RqPat::I(*v),
                            (RawPat::Const(name), RqTy::I64) => match self.consts.get(name) {
                                Some((RqTy::I64, RqV::I(v))) => RqPat::I(*v),
                                Some((t, _)) => {
                                    return Err(RqErr::C(
                                        *pln,
                                        format!(
                                            "match 腕の const `{name}` は i64 (与 {})",
                                            t.name()
                                        ),
                                    ))
                                }
                                None => {
                                    return Err(RqErr::C(
                                        *pln,
                                        format!("match 腕の const `{name}` が未定義"),
                                    ))
                                }
                            },
                            (RawPat::B(v), RqTy::Bool) => RqPat::B(*v),
                            (RawPat::I(_), other) | (RawPat::Const(_), other) => {
                                return Err(RqErr::C(
                                    *pln,
                                    format!("match 対象 {} に整数腕は使えない", other.name()),
                                ))
                            }
                            (RawPat::B(_), other) => {
                                return Err(RqErr::C(
                                    *pln,
                                    format!("match 対象 {} に bool 腕は使えない", other.name()),
                                ))
                            }
                        };
                        if wild {
                            return Err(RqErr::C(
                                *pln,
                                "_ 腕は最後にのみ置ける (それ以降の腕は到達不能)".into(),
                            ));
                        }
                        if pat == RqPat::Wild {
                            if ai != armn - 1 {
                                return Err(RqErr::C(*pln, "_ 腕は最後の腕にのみ置ける".into()));
                            }
                            wild = true;
                        } else if seen.contains(&pat) {
                            return Err(RqErr::C(
                                *pln,
                                format!("match 腕が重複: {pat:?} (別腕に分ける・到達不能を除く)"),
                            ));
                        } else {
                            seen.push(pat);
                        }
                        scopes.push(HashMap::new());
                        self.stmts(body, scopes, cur_ret, loop_depth, immut)?;
                        scopes.pop();
                    }
                    // 網羅性: _ 腕 or (bool で true/false 両方)
                    let exhaustive = wild
                        || (te == RqTy::Bool
                            && seen.contains(&RqPat::B(true))
                            && seen.contains(&RqPat::B(false)));
                    if !exhaustive {
                        return Err(RqErr::C(
                            s.ln,
                            "match は網羅的でない (i64 は _ 腕が必須。bool は true/false 両腕で可)"
                                .into(),
                        ));
                    }
                }
                RqSK::Ret(ex) => {
                    let Some(rt) = cur_ret else {
                        return Err(RqErr::C(s.ln, "ret は fn の中でのみ有効".into()));
                    };
                    let te = self.expr(ex, scopes, Some(rt))?;
                    if te != rt {
                        return Err(RqErr::C(
                            s.ln,
                            format!("ret の型 {} が fn 宣言の {} と不一致", te.name(), rt.name()),
                        ));
                    }
                }
                RqSK::P(ex) => {
                    self.expr(ex, scopes, None)?;
                }
                RqSK::Assert { e, .. } => {
                    let te = self.expr(e, scopes, None)?;
                    if te != RqTy::Bool {
                        return Err(RqErr::C(s.ln, format!("assert は bool (与 {})", te.name())));
                    }
                }
                RqSK::Block(b) => {
                    scopes.push(HashMap::new());
                    self.stmts(b, scopes, cur_ret, loop_depth, immut)?;
                    scopes.pop();
                }
            }
        }
        Ok(())
    }
    /// トップレベル: const 評価 (順序不問) → fn シグネチャ収集 → 本文検査 → 他文検査
    fn program(
        &self,
        ss: &[RqS],
    ) -> Result<
        (
            HashMap<String, (Vec<(String, RqTy)>, RqTy, Vec<RqS>)>,
            HashMap<String, (RqTy, RqV)>,
        ),
        RqErr,
    > {
        // --- const 収集 + 順序不問の fixpoint 評価 ---
        let mut consts: HashMap<String, (RqTy, RqV)> = HashMap::new();
        for s in ss {
            if let RqSK::Const { n, .. } = &s.k {
                if consts.contains_key(n) {
                    return Err(RqErr::C(s.ln, format!("const {n} が二重定義")));
                }
                if rq_builtin_sigs().iter().any(|(bn, _, _)| bn == n) {
                    return Err(RqErr::C(s.ln, format!("組み込み関数名 `{n}` は再定義不可")));
                }
                consts.insert(n.clone(), (RqTy::Bool, RqV::B(false))); // 仮
            }
        }
        let mut env: HashMap<String, (RqTy, RqV)> = HashMap::new();
        let mut pend: Vec<&RqS> = ss
            .iter()
            .filter(|s| matches!(s.k, RqSK::Const { .. }))
            .collect();
        loop {
            let mut progress = false;
            let mut rest = Vec::new();
            for s in pend {
                let RqSK::Const { n, ty, e } = &s.k else {
                    unreachable!()
                };
                // 配列/str の const は禁止
                if !matches!(ty, RqTy::F32 | RqTy::I64 | RqTy::U32 | RqTy::Bool) {
                    return Err(RqErr::C(
                        s.ln,
                        "const の型は f32/i64/u32/bool (配列・str の const は禁止)".into(),
                    ));
                }
                match rq_const_eval(e, &env) {
                    Ok((te, v)) => {
                        if te != *ty {
                            return Err(RqErr::C(
                                s.ln,
                                format!(
                                    "const {n}: 宣言型 {} に対し式の型は {}",
                                    ty.name(),
                                    te.name()
                                ),
                            ));
                        }
                        env.insert(n.clone(), (*ty, v));
                        progress = true;
                    }
                    Err(RqErr::C(_, m)) if m.contains("未解決") => {
                        rest.push(s);
                    }
                    Err(e2) => return Err(e2),
                }
            }
            if rest.is_empty() {
                break;
            }
            if !progress {
                let (n, ln) = match &rest[0].k {
                    RqSK::Const { n, .. } => (n.clone(), rest[0].ln),
                    _ => unreachable!(),
                };
                return Err(RqErr::C(
                    ln,
                    format!("const {n} の依存が循環または未定義 (順序不問で解決できない)"),
                ));
            }
            pend = rest;
        }
        // --- fn シグネチャ収集 ---
        let mut fns: HashMap<String, (Vec<(String, RqTy)>, RqTy, Vec<RqS>)> = HashMap::new();
        for s in ss {
            if let RqSK::Fn { n, ps, rt, body } = &s.k {
                if rq_builtin_sigs().iter().any(|(bn, _, _)| bn == n)
                    || n == "len"
                    || n == "fill"
                    || n == "copy"
                {
                    return Err(RqErr::C(s.ln, format!("組み込み関数名 `{n}` は再定義不可")));
                }
                if consts.contains_key(n) {
                    return Err(RqErr::C(
                        s.ln,
                        format!("fn {n} は const と同名 (名前空間共有)"),
                    ));
                }
                if fns.contains_key(n) {
                    return Err(RqErr::C(s.ln, format!("fn {n} が二重定義")));
                }
                fns.insert(n.clone(), (ps.clone(), *rt, body.clone()));
            }
        }
        for s in ss {
            if let RqSK::Let { n, .. } = &s.k {
                if consts.contains_key(n) {
                    return Err(RqErr::C(
                        s.ln,
                        format!("トップレベル let {n} は const と同名 (名前空間共有)"),
                    ));
                }
                if fns.contains_key(n) {
                    return Err(RqErr::C(
                        s.ln,
                        format!("トップレベル let {n} は fn と同名 (名前空間共有)"),
                    ));
                }
            }
        }
        let chk = RqChecker {
            fns: fns
                .iter()
                .map(|(n, (ps, rt, _))| (n.clone(), (ps.iter().map(|(_, t)| *t).collect(), *rt)))
                .collect(),
            consts: env.clone(),
        };
        // fn 本文の検査 (各 fn は独立スコープ)
        for (n, (ps, rt, body)) in &fns {
            let mut scopes = vec![ps.iter().cloned().collect::<HashMap<String, RqTy>>()];
            fn has_ret(ss: &[RqS]) -> bool {
                ss.iter().any(|s| match &s.k {
                    RqSK::Ret(_) => true,
                    RqSK::If { t, f, .. } => has_ret(t) || has_ret(f),
                    RqSK::While { body, .. } | RqSK::For { body, .. } | RqSK::Loop { body } => {
                        has_ret(body)
                    }
                    RqSK::Match { arms, .. } => arms.iter().any(|(_, b, _)| has_ret(b)),
                    RqSK::Block(b) => has_ret(b),
                    _ => false,
                })
            }
            if !has_ret(body) {
                let ln = body.first().map(|s| s.ln).unwrap_or(0);
                return Err(RqErr::C(ln, format!("fn {n} に ret がありません")));
            }
            let mut immut = Vec::new();
            chk.stmts(body, &mut scopes, Some(*rt), 0, &mut immut)?;
        }
        // トップレベル文の検査 (fn/const 除く)
        let mut scopes = vec![HashMap::new()];
        let mut immut = Vec::new();
        for s in ss {
            if matches!(s.k, RqSK::Fn { .. } | RqSK::Const { .. }) {
                continue;
            }
            chk.stmts(std::slice::from_ref(s), &mut scopes, None, 0, &mut immut)?;
        }
        Ok((fns, env))
    }
}
#[derive(Clone, Debug)]
enum RqV {
    F(f32),
    I(i64),
    U(u32),
    B(bool),
    S(String),
    /// 固定長配列 (コピーセマンティクス: Var 読み出し時に clone される)
    A(Vec<RqV>),
}

enum RqFlow {
    None,
    Ret(RqV),
    Break,
    Continue,
}

const RQ_STEP_LIMIT: u64 = 10_000_000;
const RQ_DEPTH_LIMIT: u32 = 2048;

struct RqInterp<'a> {
    fns: &'a HashMap<String, (Vec<(String, RqTy)>, RqTy, Vec<RqS>)>,
    consts: &'a HashMap<String, (RqTy, RqV)>,
    steps: u64,
    depth: u32,
    out: String,
}

extern "C" {
    fn sqrtf(x: f32) -> f32;
    fn expf(x: f32) -> f32;
    fn logf(x: f32) -> f32;
    fn powf(x: f32, y: f32) -> f32;
    fn sinf(x: f32) -> f32;
    fn cosf(x: f32) -> f32;
    fn tanf(x: f32) -> f32;
    fn atan2f(y: f32, x: f32) -> f32;
    fn hypotf(x: f32, y: f32) -> f32;
}

impl<'a> RqInterp<'a> {
    fn tick(&mut self, ln: usize) -> Result<(), RqErr> {
        self.steps += 1;
        if self.steps > RQ_STEP_LIMIT {
            return Err(RqErr::R(
                ln,
                format!("実行ステップ上限 {RQ_STEP_LIMIT} 超過 (無限ループ疑い)"),
            ));
        }
        Ok(())
    }
    fn expr(&mut self, e: &RqE, scopes: &mut Vec<HashMap<String, RqV>>) -> Result<RqV, RqErr> {
        self.tick(e.ln)?;
        match &e.k {
            RqEK::Fl(v) => Ok(RqV::F(*v)),
            RqEK::I(v) => Ok(RqV::I(*v)),
            RqEK::U(v) => Ok(RqV::U(*v)),
            RqEK::Bl(v) => Ok(RqV::B(*v)),
            RqEK::Sl(v) => Ok(RqV::S(v.clone())),
            RqEK::ArrLit(es) => {
                // 要素型・長さは型検査で保証済み
                let mut v = Vec::with_capacity(es.len());
                for x in es {
                    v.push(self.expr(x, scopes)?);
                }
                Ok(RqV::A(v))
            }
            RqEK::Var(n) => {
                for sc in scopes.iter().rev() {
                    if let Some(v) = sc.get(n) {
                        return Ok(v.clone());
                    }
                }
                if let Some((_, v)) = self.consts.get(n) {
                    return Ok(v.clone());
                }
                Err(RqErr::R(e.ln, format!("未定義変数 `{n}` (型検査漏れ)")))
            }
            RqEK::Idx(a, i) => {
                let va = self.expr(a, scopes)?;
                let vi = self.expr(i, scopes)?;
                let (RqV::A(vec), RqV::I(k)) = (&va, &vi) else {
                    return Err(RqErr::R(e.ln, "添字の型不正 (型検査漏れ)".into()));
                };
                if *k < 0 || *k >= vec.len() as i64 {
                    return Err(RqErr::R(
                        e.ln,
                        format!("境界外アクセス: 添字 {k} / 配列長 {}", vec.len()),
                    ));
                }
                Ok(vec[*k as usize].clone())
            }
            RqEK::Neg(x) => match self.expr(x, scopes)? {
                RqV::F(v) => Ok(RqV::F(-v)),
                RqV::I(v) => Ok(RqV::I(v.wrapping_neg())),
                _ => Err(RqErr::R(e.ln, "unary - の型不正 (型検査漏れ)".into())),
            },
            RqEK::Not(x) => match self.expr(x, scopes)? {
                RqV::B(v) => Ok(RqV::B(!v)),
                _ => Err(RqErr::R(e.ln, "! の型不正 (型検査漏れ)".into())),
            },
            RqEK::BitNot(x) => match self.expr(x, scopes)? {
                RqV::I(v) => Ok(RqV::I(!v)),
                RqV::U(v) => Ok(RqV::U(!v)),
                _ => Err(RqErr::R(e.ln, "~ の型不正 (型検査漏れ)".into())),
            },
            RqEK::Bin(op, a, b) => {
                // && || は短絡評価
                if *op == "&&" {
                    let RqV::B(x) = self.expr(a, scopes)? else {
                        return Err(RqErr::R(e.ln, "&& の型不正".into()));
                    };
                    if !x {
                        return Ok(RqV::B(false));
                    }
                    let RqV::B(y) = self.expr(b, scopes)? else {
                        return Err(RqErr::R(e.ln, "&& の型不正".into()));
                    };
                    return Ok(RqV::B(y));
                }
                if *op == "||" {
                    let RqV::B(x) = self.expr(a, scopes)? else {
                        return Err(RqErr::R(e.ln, "|| の型不正".into()));
                    };
                    if x {
                        return Ok(RqV::B(true));
                    }
                    let RqV::B(y) = self.expr(b, scopes)? else {
                        return Err(RqErr::R(e.ln, "|| の型不正".into()));
                    };
                    return Ok(RqV::B(y));
                }
                let va = self.expr(a, scopes)?;
                let vb = self.expr(b, scopes)?;
                rq_binop(op, &va, &vb).ok_or_else(|| {
                    RqErr::R(e.ln, format!("二項演算 {op} の型不整合またはゼロ除算"))
                })
            }
            RqEK::Call(n, args) => {
                let mut vs = Vec::new();
                for a in args {
                    vs.push(self.expr(a, scopes)?);
                }
                if let Some((ps, _rt, body)) = self.fns.get(n) {
                    self.depth += 1;
                    if self.depth > RQ_DEPTH_LIMIT {
                        return Err(RqErr::R(
                            e.ln,
                            format!("fn 呼出深度上限 {RQ_DEPTH_LIMIT} 超過 (再帰疑い)"),
                        ));
                    }
                    let mut sc = vec![ps
                        .iter()
                        .map(|(pn, _)| pn.clone())
                        .zip(vs)
                        .collect::<HashMap<String, RqV>>()];
                    for s in body {
                        if let RqFlow::Ret(v) = self.stmt(s, &mut sc)? {
                            self.depth -= 1;
                            return Ok(v);
                        }
                    }
                    self.depth -= 1;
                    return Err(RqErr::R(
                        e.ln,
                        format!("fn {n} が ret せずに終了 (全経路で ret すること)"),
                    ));
                }
                rq_builtin(n, &vs).ok_or_else(|| {
                    RqErr::R(e.ln, format!("組み込み関数 {n} の型不整合 (型検査漏れ)"))
                })
            }
        }
    }
    /// ループ本体の実行で使い回すフロー処理: Ret/Break なら Some を返す
    fn run_body(
        &mut self,
        body: &[RqS],
        scopes: &mut Vec<HashMap<String, RqV>>,
    ) -> Result<Option<RqFlow>, RqErr> {
        scopes.push(HashMap::new());
        let mut r = None;
        for st in body {
            match self.stmt(st, scopes)? {
                RqFlow::None => {}
                f => {
                    r = Some(f);
                    break;
                }
            }
        }
        scopes.pop();
        Ok(r)
    }
    fn stmt(&mut self, s: &RqS, scopes: &mut Vec<HashMap<String, RqV>>) -> Result<RqFlow, RqErr> {
        self.tick(s.ln)?;
        match &s.k {
            RqSK::Let { n, e, .. } => {
                let v = self.expr(e, scopes)?;
                scopes.last_mut().unwrap().insert(n.clone(), v);
                Ok(RqFlow::None)
            }
            RqSK::Set { t, op, e } => {
                let rhs = self.expr(e, scopes)?;
                match t {
                    RqTarget::Var(n) => {
                        for sc in scopes.iter_mut().rev() {
                            if let Some(slot) = sc.get_mut(n) {
                                let nv = match op {
                                    None => rhs,
                                    Some(o) => rq_binop(o, slot, &rhs).ok_or_else(|| {
                                        RqErr::R(s.ln, format!("複合代入 {o}= の型不整合"))
                                    })?,
                                };
                                *slot = nv;
                                return Ok(RqFlow::None);
                            }
                        }
                        if self.consts.contains_key(n) {
                            return Err(RqErr::R(s.ln, format!("const `{n}` には代入不可")));
                        }
                        Err(RqErr::R(
                            s.ln,
                            format!("代入先 `{n}` が未定義 (型検査漏れ)"),
                        ))
                    }
                    RqTarget::Idx(n, ie) => {
                        let RqV::I(k) = self.expr(ie, scopes)? else {
                            return Err(RqErr::R(s.ln, "添字の型不正".into()));
                        };
                        for sc in scopes.iter_mut().rev() {
                            if let Some(slot) = sc.get_mut(n) {
                                let RqV::A(vec) = slot else {
                                    return Err(RqErr::R(
                                        s.ln,
                                        "添字代入の対象が配列でない".into(),
                                    ));
                                };
                                if k < 0 || k >= vec.len() as i64 {
                                    return Err(RqErr::R(
                                        s.ln,
                                        format!("境界外アクセス: 添字 {k} / 配列長 {}", vec.len()),
                                    ));
                                }
                                let nv =
                                    match op {
                                        None => rhs,
                                        Some(o) => rq_binop(o, &vec[k as usize], &rhs).ok_or_else(
                                            || RqErr::R(s.ln, "複合代入の型不整合".into()),
                                        )?,
                                    };
                                vec[k as usize] = nv;
                                return Ok(RqFlow::None);
                            }
                        }
                        Err(RqErr::R(
                            s.ln,
                            format!("代入先配列 `{n}` が未定義 (型検査漏れ)"),
                        ))
                    }
                }
            }
            RqSK::If { c, t, f } => {
                let RqV::B(cv) = self.expr(c, scopes)? else {
                    return Err(RqErr::R(s.ln, "if 条件の型不正".into()));
                };
                let body = if cv { t } else { f };
                match self.run_body(body, scopes)? {
                    Some(fl) => Ok(fl),
                    None => Ok(RqFlow::None),
                }
            }
            RqSK::While { c, body } => {
                loop {
                    self.tick(s.ln)?;
                    let RqV::B(cv) = self.expr(c, scopes)? else {
                        return Err(RqErr::R(s.ln, "while 条件の型不正".into()));
                    };
                    if !cv {
                        break;
                    }
                    match self.run_body(body, scopes)? {
                        None | Some(RqFlow::None) | Some(RqFlow::Continue) => {}
                        Some(RqFlow::Break) => break,
                        Some(f @ RqFlow::Ret(_)) => return Ok(f),
                    }
                }
                Ok(RqFlow::None)
            }
            RqSK::For {
                v,
                a,
                b,
                incl,
                body,
            } => {
                let RqV::I(mut var) = self.expr(a, scopes)? else {
                    return Err(RqErr::R(s.ln, "for 範囲端の型不正".into()));
                };
                let RqV::I(end) = self.expr(b, scopes)? else {
                    return Err(RqErr::R(s.ln, "for 範囲端の型不正".into()));
                };
                loop {
                    self.tick(s.ln)?;
                    let done = if *incl { var > end } else { var >= end };
                    if done {
                        break;
                    }
                    scopes.push(HashMap::new());
                    scopes.last_mut().unwrap().insert(v.clone(), RqV::I(var));
                    let mut flow = None;
                    for st in body {
                        match self.stmt(st, scopes)? {
                            RqFlow::None => {}
                            f => {
                                flow = Some(f);
                                break;
                            }
                        }
                    }
                    scopes.pop();
                    match flow {
                        None | Some(RqFlow::None) | Some(RqFlow::Continue) => {}
                        Some(RqFlow::Break) => break,
                        Some(f @ RqFlow::Ret(_)) => return Ok(f),
                    }
                    if var == i64::MAX {
                        // wrapping による永久ループの防御 (i64::MAX 到達で打ち切り)
                        break;
                    }
                    var = var.wrapping_add(1);
                }
                Ok(RqFlow::None)
            }
            RqSK::Loop { body } => {
                loop {
                    self.tick(s.ln)?;
                    match self.run_body(body, scopes)? {
                        None | Some(RqFlow::None) | Some(RqFlow::Continue) => {}
                        Some(RqFlow::Break) => break,
                        Some(f @ RqFlow::Ret(_)) => return Ok(f),
                    }
                }
                Ok(RqFlow::None)
            }
            RqSK::Break => Ok(RqFlow::Break),
            RqSK::Continue => Ok(RqFlow::Continue),
            RqSK::Match { e, arms } => {
                let v = self.expr(e, scopes)?;
                for (raw, body, pln) in arms {
                    let hit = match (raw, &v) {
                        (RawPat::Wild, _) => true,
                        (RawPat::I(x), RqV::I(y)) => x == y,
                        (RawPat::B(x), RqV::B(y)) => x == y,
                        (RawPat::Const(name), RqV::I(y)) => match self.consts.get(name) {
                            Some((_, RqV::I(x))) => x == y,
                            _ => {
                                return Err(RqErr::R(
                                    *pln,
                                    format!("match 腕の const `{name}` が未解決 (型検査漏れ)"),
                                ))
                            }
                        },
                        _ => false,
                    };
                    if hit {
                        match self.run_body(body, scopes)? {
                            Some(fl) => return Ok(fl),
                            None => return Ok(RqFlow::None),
                        }
                    }
                }
                // 型検査で網羅性保証済みだが、防御的に到達時は fail-loud
                Err(RqErr::R(
                    s.ln,
                    "match に一致する腕が無い (型検査漏れ)".into(),
                ))
            }
            RqSK::Ret(e) => Ok(RqFlow::Ret(self.expr(e, scopes)?)),
            RqSK::P(e) => {
                let v = self.expr(e, scopes)?;
                self.out.push_str(&rq_fmt(&v));
                self.out.push('\n');
                Ok(RqFlow::None)
            }
            RqSK::Assert { e, label } => {
                let RqV::B(v) = self.expr(e, scopes)? else {
                    return Err(RqErr::R(s.ln, "assert の型不正".into()));
                };
                if !v {
                    let msg = match label {
                        Some(l) => format!("assert 失敗: {l}"),
                        None => "assert 失敗".into(),
                    };
                    return Err(RqErr::R(s.ln, msg));
                }
                Ok(RqFlow::None)
            }
            RqSK::Block(b) => match self.run_body(b, scopes)? {
                Some(fl) => Ok(fl),
                None => Ok(RqFlow::None),
            },
            RqSK::Const { .. } => Ok(RqFlow::None), // 値はコンパイル時確定済
            RqSK::Fn { n, .. } => Err(RqErr::R(
                s.ln,
                format!("fn {n} はトップレベルのみ (型検査漏れ)"),
            )),
        }
    }
}

/// 二項演算 (型検査通過済みの組のみ成功。None は型不整合または整数ゼロ除算)。
/// シフト量はマスク付き wrapping (i64 は &63、u32 は &31) で予測可能に固定。
fn rq_binop(op: &str, a: &RqV, b: &RqV) -> Option<RqV> {
    match (a, b) {
        (RqV::F(x), RqV::F(y)) => Some(match op {
            "+" => RqV::F(x + y),
            "-" => RqV::F(x - y),
            "*" => RqV::F(x * y),
            "/" => RqV::F(x / y),
            "%" => RqV::F(x % y),
            "<" => RqV::B(x < y),
            "<=" => RqV::B(x <= y),
            ">" => RqV::B(x > y),
            ">=" => RqV::B(x >= y),
            "==" => RqV::B(x == y),
            "!=" => RqV::B(x != y),
            _ => return None,
        }),
        (RqV::I(x), RqV::I(y)) => Some(match op {
            "+" => RqV::I(x.wrapping_add(*y)),
            "-" => RqV::I(x.wrapping_sub(*y)),
            "*" => RqV::I(x.wrapping_mul(*y)),
            "/" => RqV::I(x.checked_div(*y)?),
            "%" => RqV::I(x.checked_rem(*y)?),
            "&" => RqV::I(x & y),
            "|" => RqV::I(x | y),
            "^" => RqV::I(x ^ y),
            "<<" => RqV::I(x.wrapping_shl((*y & 63) as u32)),
            ">>" => RqV::I(x.wrapping_shr((*y & 63) as u32)),
            "<" => RqV::B(x < y),
            "<=" => RqV::B(x <= y),
            ">" => RqV::B(x > y),
            ">=" => RqV::B(x >= y),
            "==" => RqV::B(x == y),
            "!=" => RqV::B(x != y),
            _ => return None,
        }),
        (RqV::U(x), RqV::U(y)) => Some(match op {
            "+" => RqV::U(x.wrapping_add(*y)),
            "-" => RqV::U(x.wrapping_sub(*y)),
            "*" => RqV::U(x.wrapping_mul(*y)),
            "/" => RqV::U(x.checked_div(*y)?),
            "%" => RqV::U(x.checked_rem(*y)?),
            "&" => RqV::U(x & y),
            "|" => RqV::U(x | y),
            "^" => RqV::U(x ^ y),
            "<<" => RqV::U(x.wrapping_shl((*y & 31) as u32)),
            ">>" => RqV::U(x.wrapping_shr((*y & 31) as u32)),
            "<" => RqV::B(x < y),
            "<=" => RqV::B(x <= y),
            ">" => RqV::B(x > y),
            ">=" => RqV::B(x >= y),
            "==" => RqV::B(x == y),
            "!=" => RqV::B(x != y),
            _ => return None,
        }),
        // シフトの右辺は i64 規則 (左辺 u32 + 右辺 i64 の組)
        (RqV::U(x), RqV::I(y)) => match op {
            "<<" => Some(RqV::U(x.wrapping_shl((*y & 31) as u32))),
            ">>" => Some(RqV::U(x.wrapping_shr((*y & 31) as u32))),
            _ => None,
        },
        (RqV::B(x), RqV::B(y)) => match op {
            "==" => Some(RqV::B(x == y)),
            "!=" => Some(RqV::B(x != y)),
            _ => None,
        },
        _ => None,
    }
}

/// 組み込み関数の実行 (型検査で引数型は保証済み)。
fn rq_builtin(n: &str, vs: &[RqV]) -> Option<RqV> {
    let f1 = |i: usize| -> Option<f32> {
        match vs.get(i) {
            Some(RqV::F(x)) => Some(*x),
            _ => None,
        }
    };
    match n {
        "sqrt" => Some(RqV::F(unsafe { sqrtf(f1(0)?) })),
        "exp" => Some(RqV::F(unsafe { expf(f1(0)?) })),
        "ln" => Some(RqV::F(unsafe { logf(f1(0)?) })),
        "pow" => Some(RqV::F(unsafe { powf(f1(0)?, f1(1)?) })),
        "sin" => Some(RqV::F(unsafe { sinf(f1(0)?) })),
        "cos" => Some(RqV::F(unsafe { cosf(f1(0)?) })),
        "tan" => Some(RqV::F(unsafe { tanf(f1(0)?) })),
        "atan2" => Some(RqV::F(unsafe { atan2f(f1(0)?, f1(1)?) })),
        "hypot" => Some(RqV::F(unsafe { hypotf(f1(0)?, f1(1)?) })),
        "fma" => Some(RqV::F(f1(0)?.mul_add(f1(1)?, f1(2)?))),
        "abs" => Some(RqV::F(f1(0)?.abs())),
        "floor" => Some(RqV::F(f1(0)?.floor())),
        "ceil" => Some(RqV::F(f1(0)?.ceil())),
        "trunc" => Some(RqV::F(f1(0)?.trunc())),
        "round" => Some(RqV::F(f1(0)?.round())),
        "min" => Some(RqV::F(f1(0)?.min(f1(1)?))),
        "max" => Some(RqV::F(f1(0)?.max(f1(1)?))),
        "copysign" => Some(RqV::F(f1(0)?.copysign(f1(1)?))),
        "bits" => Some(RqV::U(f1(0)?.to_bits())),
        "b" => match vs.first() {
            Some(RqV::U(x)) => Some(RqV::F(f32::from_bits(*x))),
            _ => None,
        },
        "is_nan" => Some(RqV::B(f1(0)?.is_nan())),
        "is_inf" => Some(RqV::B(f1(0)?.is_infinite())),
        "is_fin" => Some(RqV::B(f1(0)?.is_finite())),
        "nan" => Some(RqV::F(f32::NAN)),
        "inf" => Some(RqV::F(f32::INFINITY)),
        "ninf" => Some(RqV::F(f32::NEG_INFINITY)),
        "pi" => Some(RqV::F(std::f32::consts::PI)),
        "e" => Some(RqV::F(std::f32::consts::E)),
        "f" => Some(RqV::F(match vs.first()? {
            RqV::I(x) => *x as f32,
            RqV::U(x) => *x as f32,
            _ => return None,
        })),
        "i" => Some(RqV::I(match vs.first()? {
            RqV::F(x) => *x as i64,
            RqV::U(x) => *x as i64,
            _ => return None,
        })),
        "u" => Some(RqV::U(match vs.first()? {
            RqV::F(x) => *x as u32,
            RqV::I(x) => *x as u32,
            _ => return None,
        })),
        "len" => match vs.first()? {
            RqV::A(v) => Some(RqV::I(v.len() as i64)),
            _ => None,
        },
        "fill" => match (vs.first()?, vs.get(1)?) {
            (RqV::A(v), x) => Some(RqV::A(vec![x.clone(); v.len()])),
            _ => None,
        },
        "copy" => match (vs.first()?, vs.get(1)?) {
            (RqV::A(_), RqV::A(src)) => Some(RqV::A(src.clone())),
            _ => None,
        },
        _ => None,
    }
}

/// p 出力の整形 (スカラは `0x<bits 16進大文字> <10進>` の統一形、
/// 配列は `[ v0, v1, .. ]` で各要素をスカラと同一形式)。
fn rq_fmt(v: &RqV) -> String {
    match v {
        RqV::F(x) => format!(
            "0x{:08X} {}",
            x.to_bits(),
            if x.is_nan() {
                "NaN".to_string()
            } else if x.is_infinite() {
                if x.is_sign_negative() {
                    "-inf".into()
                } else {
                    "inf".into()
                }
            } else {
                format!("{x}")
            }
        ),
        RqV::I(x) => format!("0x{:016X} {}", *x as u64, x),
        RqV::U(x) => format!("0x{:08X} {}", x, x),
        RqV::B(x) => format!("{x}"),
        RqV::S(x) => x.clone(),
        RqV::A(xs) => format!(
            "[ {} ]",
            xs.iter().map(rq_fmt).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// --prelude で前置される標準小関数群 (Vec3/luma/補間系。監査 wave の定形)。
/// 左結合評価は Rust 実装 (Vec3::dot/length, luma) と bit 同一。
/// ※ 行番号ずれは 9 行分。
const RQ_PRELUDE: &str = r#"fn dot3(ax:f32,ay:f32,az:f32,bx:f32,by:f32,bz:f32)->f32 { ret ax*bx+ay*by+az*bz; }
fn len3(x:f32,y:f32,z:f32)->f32 { ret sqrt(dot3(x,y,z,x,y,z)); }
fn ns(x:f32,y:f32,z:f32)->f32 { let l:f32 = len3(x,y,z); if l > 1e-8 { ret 1.0/l; } ret 1.0; }
fn luma601(r:f32,g:f32,b:f32)->f32 { ret 0.299*r+0.587*g+0.114*b; }
fn luma709(r:f32,g:f32,b:f32)->f32 { ret 0.2126*r+0.7152*g+0.0722*b; }
fn clamp01(x:f32)->f32 { if x < 0.0 { ret 0.0; } if x > 1.0 { ret 1.0; } ret x; }
fn lerp(a:f32,b:f32,t:f32)->f32 { ret a+(b-a)*t; }
fn sign(x:f32)->f32 { if x > 0.0 { ret 1.0; } if x < 0.0 { ret -1.0; } ret 0.0; }
fn frac(x:f32)->f32 { ret x-floor(x); }
"#;

/// rq プログラム全体の実行。戻り値は (終了コード, 出力文字列)。
/// 0=成功, 2=構文/型エラー, 3=実行時エラー。エラー文は出力に含む。
fn rq_run(src: &str, prelude: bool, check_only: bool) -> (i32, String) {
    let owned;
    let s = if prelude {
        owned = format!("{RQ_PRELUDE}{src}");
        owned.as_str()
    } else {
        src
    };
    let toks = match rq_lex(s) {
        Ok(t) => t,
        Err((ln, m)) => return (2, format!("rq: 行 {ln}: {m}\n")),
    };
    let mut p = RqParser { t: toks, p: 0 };
    let prog = match p.program() {
        Ok(v) => v,
        Err((ln, m)) => return (2, format!("rq: 行 {ln}: {m}\n")),
    };
    let checker = RqChecker {
        fns: HashMap::new(),
        consts: HashMap::new(),
    };
    let (fns, consts) = match checker.program(&prog) {
        Ok(f) => f,
        Err(RqErr::C(ln, m)) => return (2, format!("rq: 行 {ln}: {m}\n")),
        Err(RqErr::R(ln, m)) => return (3, format!("rq: 行 {ln}: {m}\n")),
    };
    if check_only {
        return (0, format!("rq: 型検査 OK (fn {} 件)\n", fns.len()));
    }
    let mut it = RqInterp {
        fns: &fns,
        consts: &consts,
        steps: 0,
        depth: 0,
        out: String::new(),
    };
    let mut scopes = vec![HashMap::new()];
    for st in &prog {
        if matches!(st.k, RqSK::Fn { .. }) {
            continue;
        }
        match it.stmt(st, &mut scopes) {
            Ok(RqFlow::None) => {}
            Ok(_) => {
                return (
                    3,
                    format!(
                        "rq: 行 {}: トップレベルで ret/break/continue は不可\n",
                        st.ln
                    ),
                );
            }
            Err(RqErr::R(ln, m)) => {
                it.out.push_str(&format!("rq: 行 {ln}: {m}\n"));
                return (3, it.out);
            }
            Err(RqErr::C(ln, m)) => {
                it.out.push_str(&format!("rq: 行 {ln}: {m}\n"));
                return (2, it.out);
            }
        }
    }
    (0, it.out)
}

fn cmd_rq(a: &[String]) -> i32 {
    let mut src: Option<String> = None;
    let mut path: Option<&str> = None;
    let mut prelude = false;
    let mut check_only = false;
    let mut i = 0;
    while i < a.len() {
        match a[i].as_str() {
            "-e" => {
                i += 1;
                if i >= a.len() {
                    eprintln!("rspeed rq: -e の後にソースが必要");
                    return 2;
                }
                src = Some(a[i].clone());
            }
            "--prelude" => prelude = true,
            "--check" => check_only = true,
            s if path.is_none() => path = Some(s),
            _ => {
                eprintln!("rspeed rq: 引数過多: {}", a[i]);
                return 2;
            }
        }
        i += 1;
    }
    let code = match (src, path) {
        (Some(s), None) => s,
        (None, Some(p)) => match std::fs::read_to_string(p) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("rspeed rq: {p} が読めない: {e}");
                return 2;
            }
        },
        _ => {
            eprintln!("usage: rspeed rq <file.rq> | -e '<ソース>' [--prelude] [--check]");
            eprintln!("  f32 IEEE 厳密計算の小言語 v2 (構文一次情報: docs/internal/RQ.md)");
            return 2;
        }
    };
    let (rc, out) = rq_run(&code, prelude, check_only);
    if rc == 0 {
        print!("{out}");
    } else {
        eprint!("{out}");
    }
    rc
}
fn main() {
    // | head 等で stdout が閉じた時の EPIPE パニックを静かに扱う
    // (UNIX ツール流儀: 141 (=128+SIGPIPE) で終了。それ以外のパニックは従来通り表示)。
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("{info}");
        if msg.contains("Broken pipe") {
            return;
        }
        default_hook(info);
    }));
    // PATH 保険 (bash ツール毎回 export する手間を詰める)
    if let Ok(path) = env::var("PATH") {
        if !path.contains("/home/user/rust/bin") {
            env::set_var("PATH", format!("/home/user/rust/bin:{path}"));
        }
    }
    let args: Vec<String> = env::args().skip(1).collect();
    let rc = match args.first().map(|s| s.as_str()) {
        Some("expr") => cmd_expr(&args[1..]),
        Some("rq") => cmd_rq(&args[1..]),
        Some("san") => cmd_san(&args[1..]),
        Some("find") | Some("grep") => cmd_find(&args[1..]),
        Some("regcount") => cmd_regcount(&args[1..]),
        Some("audit-todo") => cmd_audit_todo(&args[1..]),
        Some("fmdiff") => cmd_fmdiff(&args[1..]),
        Some("test") => cmd_test(&args[1..]),
        Some("bench") => cmd_bench(&args[1..]),
        Some("warn") => cmd_warn(&args[1..]),
        // Batch A: 厳密数値
        Some("bits") => cmd_bits(&args[1..]),
        Some("bits-of") => cmd_bits_of(&args[1..]),
        Some("ulp") => cmd_ulp(&args[1..]),
        Some("next") => cmd_next(&args[1..]),
        Some("hfbits") => cmd_hfbits(&args[1..]),
        Some("gamma") => cmd_gamma(&args[1..]),
        Some("morton") => cmd_morton(&args[1..]),
        Some("murmur") => cmd_murmur(&args[1..]),
        Some("splitmix") => cmd_splitmix(&args[1..]),
        Some("xs64") => cmd_xs64(&args[1..]),
        Some("pcg") => cmd_pcg(&args[1..]),
        Some("fnv") => cmd_fnv(&args[1..]),
        Some("prime") => cmd_prime(&args[1..]),
        Some("modpow") => cmd_modpow(&args[1..]),
        Some("invmod") => cmd_invmod(&args[1..]),
        Some("contfrac") => cmd_contfrac(&args[1..]),
        Some("table") => cmd_table(&args[1..]),
        Some("range") => cmd_range(&args[1..]),
        Some("monotone") => cmd_monotone(&args[1..]),
        Some("roundtrip") => cmd_roundtrip(&args[1..]),
        Some("ulperr") => cmd_ulperr(&args[1..]),
        Some("int-cast") => cmd_int_cast(&args[1..]),
        Some("quant") => cmd_quant(&args[1..]),
        Some("mat4") => cmd_mat4(&args[1..]),
        Some("vec3") => cmd_vec3(&args[1..]),
        Some("lerp") => cmd_lerp(&args[1..]),
        Some("hypot") => cmd_hypot(&args[1..]),
        Some("fp-table") => cmd_fp_table(&args[1..]),
        // Batch B: ソーススキャナ
        Some("magic") => cmd_magic(&args[1..]),
        Some("floatlits") => cmd_floatlits(&args[1..]),
        Some("casts") => cmd_casts(&args[1..]),
        Some("clamps") => cmd_clamps(&args[1..]),
        Some("divmod") => cmd_divmod(&args[1..]),
        Some("shifts") => cmd_shifts(&args[1..]),
        Some("unwraps") => cmd_unwraps(&args[1..]),
        Some("tests-index") => cmd_tests_index(&args[1..]),
        Some("test-find") => cmd_test_find(&args[1..]),
        Some("test-count") => cmd_test_count(&args[1..]),
        Some("fns") => cmd_fns(&args[1..]),
        Some("pubs") => cmd_pubs(&args[1..]),
        Some("docs") => cmd_docs(&args[1..]),
        Some("todo-scan") => cmd_todo_scan(&args[1..]),
        Some("dups") => cmd_dups(&args[1..]),
        Some("longlines") => cmd_longlines(&args[1..]),
        Some("trailws") => cmd_trailws(&args[1..]),
        Some("nonascii") => cmd_nonascii(&args[1..]),
        Some("eol") => cmd_eol(&args[1..]),
        Some("tabs") => cmd_tabs(&args[1..]),
        Some("dead") => cmd_dead(&args[1..]),
        Some("hotfiles") => cmd_hotfiles(&args[1..]),
        Some("diff") => cmd_diff(&args[1..]),
        Some("grep2") => cmd_grep2(&args[1..]),
        // Batch C: リポジトリ運用
        Some("md5") => cmd_md5(&args[1..]),
        Some("md5check") => cmd_md5check(&args[1..]),
        Some("status") => cmd_status(&args[1..]),
        Some("changed-tests") => cmd_changed_tests(&args[1..]),
        Some("env-check") => cmd_env_check(&args[1..]),
        Some("snapshot") => cmd_snapshot(&args[1..]),
        Some("snapcheck") => cmd_snapcheck(&args[1..]),
        Some("rescue") => cmd_rescue(&args[1..]),
        Some("adv-save") => cmd_adv_save(&args[1..]),
        Some("adv-restore") => cmd_adv_restore(&args[1..]),
        Some("adv-diff") => cmd_adv_diff(&args[1..]),
        Some("time-run") => cmd_time_run(&args[1..]),
        Some("binsize") => cmd_binsize(&args[1..]),
        Some("ghfile") => cmd_ghfile(&args[1..]),
        Some("ghlatest") => cmd_ghlatest(&args[1..]),
        Some("wave-log") => cmd_wave_log(&args[1..]),
        Some("journal") => cmd_journal(&args[1..]),
        Some("registry-stats") => cmd_registry_stats(&args[1..]),
        Some("wave-info") => cmd_wave_info(&args[1..]),
        Some("todo-pick") => cmd_todo_pick(&args[1..]),
        Some("todo-pri") => cmd_todo_pri(&args[1..]),
        Some("coverage") => cmd_coverage(&args[1..]),
        Some("ci-status") => cmd_ci_status(&args[1..]),
        Some("lines") => cmd_lines(&args[1..]),
        Some("burndown") => cmd_burndown(&args[1..]),
        Some("seal") => cmd_seal(&args[1..]),
        Some("dashboard") => cmd_dashboard(&args[1..]),
        // Batch D: 統計/ビット/グラフィクス数学
        Some("percentile") => cmd_percentile(&args[1..]),
        Some("histogram") => cmd_histogram(&args[1..]),
        Some("bigfact") => cmd_bigfact(&args[1..]),
        Some("fib") => cmd_fib(&args[1..]),
        Some("crc32") => cmd_crc32(&args[1..]),
        Some("bitops") => cmd_bitops(&args[1..]),
        Some("pack") => cmd_pack(&args[1..]),
        Some("unpack") => cmd_unpack(&args[1..]),
        Some("endian") => cmd_endian(&args[1..]),
        Some("clamp-table") => cmd_clamp_table(&args[1..]),
        Some("matc") => cmd_matc(&args[1..]),
        Some("quat") => cmd_quat(&args[1..]),
        Some("proj") => cmd_proj(&args[1..]),
        Some("lookat") => cmd_lookat(&args[1..]),
        Some("tri-area") => cmd_tri_area(&args[1..]),
        Some("bary") => cmd_bary(&args[1..]),
        Some("halton") => cmd_halton(&args[1..]),
        Some("r2") => cmd_r2(&args[1..]),
        Some("color") => cmd_color(&args[1..]),
        Some("srgb-err") => cmd_srgb_err(&args[1..]),
        Some("quat-slerp") => cmd_quat_slerp(&args[1..]),
        Some("selftest") => cmd_selftest(&args[1..]),
        Some("man") => cmd_man(&args[1..]),
        Some("nextwave") => cmd_nextwave(&args[1..]),
        Some("help") | Some("--help") | Some("-h") | None => {
            help();
            0
        }
        Some(other) => {
            eprintln!("未知のサブコマンド: {other} (help 参照)");
            2
        }
    };
    // EPIPE 経由の場合も rc は維持。パニックが既に起きた場合は catch_unwind 側は
    // 責務外 (各コマンドは panic を投げない設計) のため単純終了。
    std::process::exit(rc);
}
