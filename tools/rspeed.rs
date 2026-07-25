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
            if matches!(name.as_str(), ".git" | "target" | "node_modules" | ".rspeed-cache") {
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
    Num(String),               // 生字句 (hex/int/decimal)
    Neg(Box<Ast>),
    Bin(char, Box<Ast>, Box<Ast>), // + - * / % ^ <(shl) >(shr)
    Call(String, Vec<Ast>),
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
    let mut lx = Lexer { b: s.as_bytes(), i: 0 };
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
                || ((ch == '+' || ch == '-')
                    && lx.i > start
                    && (lx.b[lx.i - 1] | 0x20) == b'e')
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
    s.parse::<f64>().map_err(|e| format!("数値 '{s}' 解析: {e}"))
}

fn eval_f64(a: &Ast) -> Result<f64, String> {
    Ok(match a {
        Ast::Num(s) => num_f64(s)?,
        Ast::Neg(x) => -eval_f64(x)?,
        Ast::Bin(op, l, r) => {
            let x = eval_f64(l)?;
            let y = eval_f64(r)?;
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
        Ast::Call(f, args) => call_f64(f, args)?,
    })
}

fn as_int(v: f64) -> Result<i128, String> {
    if !v.is_finite() || v.fract() != 0.0 || v.abs() > 1.7e38 {
        return Err(format!("整数に変換できない値: {v}"));
    }
    Ok(v as i128)
}

fn call_f64(f: &str, args: &[Ast]) -> Result<f64, String> {
    let ev = |a: &Ast| eval_f64(a);
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
    Ok(match a {
        Ast::Num(s) => {
            let s2: String = s.chars().filter(|c| *c != '_').collect();
            if let Some(h) = s2.strip_prefix("0x") {
                i128::from_str_radix(h, 16).map_err(|e| e.to_string())? as f32
            } else if s2 == "0/0" {
                f32::NAN
            } else {
                s2.parse::<f32>().map_err(|e| format!("f32 数値 '{s2}': {e}"))?
            }
        }
        Ast::Neg(x) => -eval_f32(x)?,
        Ast::Bin(op, l, r) => {
            let x = eval_f32(l)?;
            let y = eval_f32(r)?;
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
            let evs: Result<Vec<f32>, _> = args.iter().map(eval_f32).collect();
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
        Some(p) => (&s[..p], s[p + 1..].parse::<i64>().map_err(|e| e.to_string())?),
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
        frac_part.parse().map_err(|e: std::num::ParseIntError| e.to_string())?
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
    Ok(match a {
        Ast::Num(s) => num_frac(s)?,
        Ast::Neg(x) => {
            let f = eval_frac(x)?;
            frac_new(-f.n, f.d)
        }
        Ast::Bin(op, l, r) => {
            let x = eval_frac(l)?;
            let y = eval_frac(r)?;
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
                    let xi = if x.d == 1 { x.n } else { return Err("frac シフトは整数のみ".into()) };
                    let k = if y.d == 1 { y.n } else { return Err("frac シフトは整数のみ".into()) };
                    if !(0..=126).contains(&k) {
                        return Err("frac シフト量 0..=126".into());
                    }
                    frac_new(if *op == '<' { xi << k } else { xi >> k }, 1)
                }
                _ => unreachable!(),
            }
        }
        Ast::Call(f, args) => {
            let evs: Result<Vec<Frac>, _> = args.iter().map(eval_frac).collect();
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
                        acc = if f == "gcd" { gcd_i128(acc, xi) } else { acc / gcd_i128(acc, xi) * xi };
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
                            println!("  f32 丸め: 0x{:08x} (= {})", (fv as f32).to_bits(), fv as f32);
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
                        println!("  f32 丸め: {}  |  bits 0x{:08x}", v as f32, (v as f32).to_bits());
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
    ('\u{00E3}', "Mojibake U+00E3 (日本語→Latin1 化け痕跡の可能性)"),
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
    "\u{7ec4}\u{7ec5}\u{7ec6}\u{7ec7}\u{7ec8}\u{7eca}\u{7ecd}\u{7ece}\u{7ecf}\u{7ed1}\u{7ed2}\u{7ed3}\u{7ed5}\u{7ed8}\u{7ed9}\u{7eda}\u{7edd}\u{7edf}\u{7ee2}\u{7ee3}\u{7ee7}\u{7ee9}\u{7eea}\u{7eeb}",
    "\u{7eed}\u{7eee}\u{7ef3}\u{7ef4}\u{7ef5}\u{7ef7}\u{7ef8}\u{7efc}\u{7efd}\u{7eff}\u{7f00}\u{7f06}\u{7f0e}\u{7f13}\u{7f14}\u{7f15}\u{7f16}\u{7f18}\u{7f1a}\u{7f20}\u{7f28}\u{7f29}\u{7f2a}\u{9965}",
    "\u{9968}\u{996a}\u{996f}\u{9980}\u{9981}\u{9988}\u{998b}\u{998d}\u{998f}\u{9992}"
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
            out.push(Finding { line, col, rule: "CR", msg: "CR (CRLF 混入)".into() });
            continue;
        }
        for &(c, name) in INVISIBLES {
            if ch == c {
                out.push(Finding { line, col, rule: "INVISIBLE", msg: name.into() });
            }
        }
        for &(c, name) in MOJIBAKE {
            if ch == c {
                out.push(Finding { line, col, rule: "MOJIBAKE", msg: name.into() });
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
        out.push(Finding { line, col: col + 1, rule: "EOF", msg: "末尾改行なし".into() });
    }
    Ok(out)
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
                    println!("{}:{}:{} [{}] {}", f.display(), x.line, x.col, x.rule, x.msg);
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
    let Some(rest) = l.strip_prefix("| ") else { return false };
    // A期-<digits> or [A-Z]{1,3}-<digits>
    if let Some(r) = rest.strip_prefix("A期-") {
        return r.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false);
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
    bytes.get(i + 1).map(|b| b.is_ascii_digit()).unwrap_or(false)
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
        let Some(body) = l.strip_prefix("## ") else { continue };
        // "## DC. render_graph.rs — …" 形式: 接尾辞部分 + ". " + 名前.rs
        let Some(dotpos) = body.find(". ") else { continue };
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
    println!("done: {}, all: {}, todo: {}", done.len(), all.len(), todo.len());
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
        .arg(path)
        .output()
        .map_err(|e| format!("rustfmt 起動失敗: {e}"))?;
    if !out.status.success() {
        return Err(format!("rustfmt 失敗: {}", String::from_utf8_lossy(&out.stderr)));
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
        return b.iter().filter(|s| !sa.contains(*s)).map(|s| s.to_string()).collect();
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
        let rel = p.strip_prefix(&gr).unwrap_or(&p).to_string_lossy().replace('\\', "/");
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
                println!("{a}: HEAD に無い新規ファイル → 現逸脱 {} 行 (要全行正準)", cur_dev.len());
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
    env::var("RSPEED_RUSTFLAGS").unwrap_or_else(|_| {
        format!("-C link-arg=-fuse-ld=lld -C link-arg=-B{gcc_ld}")
    })
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
    let _ = fs::write(fp_path(tag), format!("{cur:016x}"));
}

fn newest_unittest(krate: &str, profile: &str) -> Option<PathBuf> {
    let deps = ws_root()
        .join("target")
        .join(profile)
        .join(if profile == "dev" || profile == "debug" { "deps" } else { "deps" });
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
            if fs::metadata(&p).map(|m| m.permissions().mode() & 0o111 == 0).unwrap_or(true) {
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
                krate = args.get(i).cloned().unwrap_or_else(|| "rsift-opt-gfx".into());
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
        let rc = run_cargo(&["test", "-p", &krate, "--lib", "--locked", "--offline", "--no-run"]);
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
    let n = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(2);
    let t0 = Instant::now();
    let mut cmd = Command::new(&bin);
    cmd.arg("--test-threads").arg(n.to_string());
    for f in &filter {
        cmd.arg(f);
    }
    let st = cmd.status().map_err(|e| format!("{bin:?} 実行失敗: {e}"));
    println!("rspeed: run {:.2}s (bin={})", t0.elapsed().as_secs_f64(), bin.display());
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
    let bin = ws_root().join("target").join("release").join("examples").join(&example);
    let need_build = force || fp_changed(&tag, fp) || !bin.exists();
    if need_build {
        let t0 = Instant::now();
        let rc = run_cargo(&[
            "build", "-p", krate, "--release", "--locked", "--offline", "--example", &example,
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
                let ok = stdout.lines().any(|l| l.contains("structural_digest") && l.contains(&want));
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
    let strict = text.lines().filter(|l| l.starts_with("warning") || l.contains(": warning")).count();
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
        "rspeed — Rsift 監査統合高速ツール (std のみ / rustc -O 単一バイナリ)\n\
         \n\
         数値厳密検算:\n\
         \x20 rspeed expr [--f32] [--frac] <式>…   f64 評価+bits / f32 逐次 / Fraction 正確 (gcd,lcm,pow,<< 等)\n\
         \n\
         ファイル検査/検索:\n\
         \x20 rspeed san <file|dir>…              不可視文字・CRLF・末尾改行・文字化け・簡体字検査\n\
         \x20 rspeed find [--count] <needle> <path>… 高速リテラル検索\n\
         \x20 rspeed regcount <registry.md>       台帳エントリ数 (grep -cE 等価)\n\
         \x20 rspeed audit-todo <audit.md> <src>  棚卸し残モジュール抽出\n\
         \x20 rspeed fmdiff <.rs>…                rustfmt 逸脱の HEAD 包含照合 (監査 fmt 規律)\n\
         \n\
         ビルド/テスト (指紋キャッシュで再ビルド省略):\n\
         \x20 rspeed test [-p <crate>] [--rebuild] [filter]…   cargo test 代替ランナー\n\
         \x20 rspeed bench [example] [--expect-digest HEX] [grep 語…]  wide_static_bench 等の実行+digest 照合\n\
         \x20 rspeed warn [crate] [期待数]        cargo check 警告数照合\n\
         \n\
         環境変数: RSIFT_WS=/home/user/rsift/rsift (workspace), RSIFT_GIT_ROOT=/home/user/rsift,\n\
         \x20 RSPEED_RUSTFLAGS (既定 lld)"
    );
}

fn main() {
    // PATH 保険 (bash ツール毎回 export する手間を詰める)
    if let Ok(path) = env::var("PATH") {
        if !path.contains("/home/user/rust/bin") {
            env::set_var("PATH", format!("/home/user/rust/bin:{path}"));
        }
    }
    let args: Vec<String> = env::args().skip(1).collect();
    let rc = match args.first().map(|s| s.as_str()) {
        Some("expr") => cmd_expr(&args[1..]),
        Some("san") => cmd_san(&args[1..]),
        Some("find") | Some("grep") => cmd_find(&args[1..]),
        Some("regcount") => cmd_regcount(&args[1..]),
        Some("audit-todo") => cmd_audit_todo(&args[1..]),
        Some("fmdiff") => cmd_fmdiff(&args[1..]),
        Some("test") => cmd_test(&args[1..]),
        Some("bench") => cmd_bench(&args[1..]),
        Some("warn") => cmd_warn(&args[1..]),
        Some("help") | Some("--help") | Some("-h") | None => {
            help();
            0
        }
        Some(other) => {
            eprintln!("未知のサブコマンド: {other} (help 参照)");
            2
        }
    };
    std::process::exit(rc);
}
