# RQ 言語仕様書 (rspeed 内蔵・AI 記述最優先の静的型付き小言語)

導入: 2026-07-26。実装: `tools/rspeed.rs` 内の `rq_*` 系 (`rspeed rq` サブコマンド)。
目的: 監査で繰り返し使ってきた **Python (struct + ctypes libm) による f32 IEEE
エミュレート計算を rspeed に全面移行**するための実行基盤。

設計方針: **AI が生成ミスしにくいことだけを最優先**する。人間の読みやすさ・
見た目の美しさは追求しない。構文は極小・明示的・曖昧さゼロを貫く。

---

## 1. 呼び出し

```sh
rspeed rq <file.rq>                 # ファイル実行
rspeed rq -e 'let x: f32 = 0.1; p x;'   # インライン実行
rspeed rq --check <file.rq>         # 型検査のみ (実行しない)
rspeed rq --prelude -e 'p luma601(1.0, 0.0, 0.0);'  # 標準小関数群を前置
```

終了コード: `0` 成功 / `2` 構文・型エラー / `3` 実行時エラー (assert 失敗・
ゼロ除算・ステップ上限・深度上限・ret 抜け)。

エラー形式: `rq: 行 N: メッセージ` (stderr)。行番号は 1 始まり。
`--prelude` 使用時は prelude 5 行分だけ行番号がずれる (prelude が 5 行のため)。

## 2. 字句

- コメント: `#` から行末まで。
- 空白・タブ・改行は意味を持たない (区切りは全て記号)。
- 識別子: `[A-Za-z_][A-Za-z0-9_]*`。
- 予約語 (識別子に使用不可): `let fn if else while ret p assert true false f32 i64 u32 bool str`。
- 文字列: `"..."`、エスケープは `\n` `\t` `\\` `\"` のみ (p の引数専用)。

## 3. 型 (5 種・暗黙変換なし)

| 型 | 内容 | リテラル |
|----|------|----------|
| `f32` | IEEE-754 単精度 (ハードウェア丸め) | `0.1` `1e-8` `2.` `.5` `1_000.5` (`_` 可) |
| `i64` | 64bit 符号付き (算術は wrapping) | `123` `1_000` (`.`/`e` を含まない 10 進) |
| `u32` | 32bit 符号なし (算術は wrapping) | `0x3F800000` (16 進のみ・`_` 可) |
| `bool` | 真理値 | `true` `false` |
| `str` | 文字列 (p 専用・演算不可) | `"..."` |

- 10 進整数リテラルは常に `i64`。`f32` が欲しければ `123.0` と書くか `f(123)` で変換。
- 暗黙変換は**一切存在しない**。「`0.1 + 1`」は型エラー (exit 2) で、
  「算術 + は同型の f32/i64/u32 (与 f32 と i64)。明示変換 f()/i()/u() を使う」と出る。
- f32 リテラルの丸めは **10 進 → f32 の直接正確丸め** (Rust `"..."．parse::<f32>()`
  と同一)。旧 Python 流の「f64 経由 → f32 丸め」による二重丸め差は発生しない
  (Rust コードのリテラルと常に一致する点で旧 Python シムより正確)。

## 4. 文 (8 種)

```text
let x: f32 = 0.1;            # 型付き束縛 (内側スコープで shadowing 可)
x = x * 2.0;                 # 代入 (既存変数のみ・型一致必須)
fn add(a: i64, b: i64) -> i64 { ret a + b; }   # 関数 (トップレベルのみ)
if x > 0.5 { p "big"; } else { p "small"; }    # 条件は bool 必須
while k <= 5 { k = k + 1; }  # 条件は bool 必須
ret a + b;                   # 関数内のみ・宣言戻り値と型一致必須
p x;                         # 値の出力 (全型可) 1 行 1 値
assert x == x;               # false なら exit 3 (fail-loud)
{ let y: f32 = 1.0; p y; }   # 裸ブロック (スコープ)
```

- 式文 (副作用だけの関数呼出など) は**禁止**。出力は必ず `p` を通す。
- `fn` はトップレベルにのみ書ける (ネスト不可)。再帰は許可 (深度上限 2048)。
- `fn` は構文的に `ret` を 1 つ以上含むこと。到達不能経路があると実行時に
  「fn X が ret せずに終了」で exit 3。
- 実行ステップ上限 10,000,000 (無限ループ疑いで exit 3)。

## 5. 式・演算子優先順位 (全て左結合・Rust と同一)

```text
1. ||                      (bool)
2. &&                      (bool・短絡評価)
3. == != < <= > >=         (同型の f32/i64/u32。== != は bool も可)
4. + -                     (同型の f32/i64/u32)
5. * / %                   (同型の f32/i64/u32)
6. 単項 - !                (- は f32/i64。u32 は禁止 → `0 - x` と書く)
7. 呼出 f(...) / 括弧 / リテラル / 変数
```

- f32 比較は **IEEE 意味論そのもの**: NaN が絡むと `< <= > >= ==` は全て false、
  `!=` は true (実機の Rust/SIMD と同じ)。
- `&&`/`||` は短絡評価。
- i64/u32 の `+ - *` は wrapping (オーバーフローでエラーにしない)。
  `/` `%` はゼロ除算・`i64::MIN / -1` で実行時エラー (exit 3)。
- f32 の `%` は Rust の `%` (fmod 意味)。

## 6. 組み込み関数 (全て静的型付き)

### f32 数学 (libm FFI、ctypes libm と bit 同一を機械検証済み)

| 関数 | 型 | 備考 |
|------|-----|------|
| `sqrt(x)` | f32→f32 | libm sqrtf (正確丸め) |
| `exp(x)` | f32→f32 | libm expf |
| `ln(x)` | f32→f32 | libm logf |
| `pow(x,y)` | f32×f32→f32 | libm powf |
| `sin(x)` `cos(x)` | f32→f32 | libm sinf/cosf |
| `atan2(y,x)` | f32×f32→f32 | libm atan2f |
| `abs(x)` | f32→f32 | 厳密 (符号ビット操作) |
| `floor(x)` `ceil(x)` `trunc(x)` | f32→f32 | 厳密 |
| `round(x)` | f32→f32 | 最近接・同着は零から遠い方 (Rust round) |
| `min(x,y)` `max(x,y)` | f32×f32→f32 | Rust f32::min/max 流: 片方 NaN なら他方。両方 NaN なら NaN |
| `copysign(x,y)` | f32×f32→f32 | 厳密 |

### ビット・判定

| 関数 | 型 | 備考 |
|------|-----|------|
| `bits(x)` | f32→u32 | to_bits |
| `b(h)` | u32→f32 | from_bits |
| `is_nan(x)` `is_inf(x)` `is_fin(x)` | f32→bool | |
| `nan()` `inf()` `ninf()` | ()→f32 | NaN は正カノニカル 0x7FC00000 |

### 明示変換

| 関数 | 型 | 備考 |
|------|-----|------|
| `f(x)` | i64→f32 / u32→f32 | 最近接偶数丸め (Rust `as`) |
| `i(x)` | f32→i64 / u32→i64 | f32 は飽和 (`as`)、u32 は拡大 |
| `u(x)` | f32→u32 / i64→u32 | f32 は飽和 (`as`)、i64 は下端切捨て (`as`) |

## 7. --prelude (標準小関数群)

監査の定形計算を前置する (ソースは rspeed.rs 内 `RQ_PRELUDE` に固定):

```text
fn dot3(ax:f32,ay:f32,az:f32,bx:f32,by:f32,bz:f32)->f32 { ret ax*bx+ay*by+az*bz; }
fn len3(x:f32,y:f32,z:f32)->f32 { ret sqrt(dot3(x,y,z,x,y,z)); }
fn ns(x:f32,y:f32,z:f32)->f32 { let l:f32 = len3(x,y,z); if l > 1e-8 { ret 1.0/l; } ret 1.0; }
fn luma601(r:f32,g:f32,b:f32)->f32 { ret 0.299*r+0.587*g+0.114*b; }
fn luma709(r:f32,g:f32,b:f32)->f32 { ret 0.2126*r+0.7152*g+0.0722*b; }
```

- `dot3`/`len3` は opt-gfx `Vec3::dot`/`length` と bit 同一 (左結合 + sqrtf)。
- `ns` は `Vec3::normalize` の逆数係数: `v * ns(x,y,z)` が normalize と bit 同一
  (l<=1e-8 のとき Rust が `self` を返すのに対し `x*1.0` は x と厳密一致するため同等)。
- `luma601`/`luma709` も左結合でコードベースの luma と bit 同一。
- prelude 使用時はエラー行番号が **5 行前倒し**で表示される点に注意。

## 8. 出力形式 (p)

```text
f32 : 0x3F800000 1          # 0x + bits 8 桁 16 進大文字 + 空白 + 最短往復 10 進
u32 : 0x3F800000 1065353216 # 0x + 8 桁 16 進大文字 + 空白 + 10 進
i64 : 0x0000000000003039 12345
bool: true / false
str : 内容そのまま
特殊: NaN / inf / -inf / -0
```

## 9. Python からの移行対応表

| 旧 Python (struct+ctypes) | rq |
|---|---|
| `struct.pack('<f', x)` で逐次丸め | 不要 (全演算が最初から f32) |
| `ctypes.CDLL("libm.so.6").expf` | `exp(x)` (同じ libm を FFI 直叩き) |
| `'0x%08X' % bits(x)` | `p bits(x);` または `p x;` |
| `libm.sqrtf` | `sqrt(x)` |
| `f32(0.299)` の係数丸め確認 | `p 0.299;` → `0x3E991687 0.299` |
| 左結合の手動展開 `f32(f32(a+b)+c)` | `a+b+c` (言語が左結合で評価) |
| 正規化シム (len+条件+逆数) | `--prelude` の `ns` (§7) |

## 10. 実用例 (wave 120 SSR 監査の再導出)

```sh
rspeed rq --prelude -e '
# reflect_dir: i=(0.6,-0.8,0), n=(0,1,0) の厳密 bits
let s: f32 = ns(0.6, -0.8, 0.0);
let ix: f32 = 0.6*s; let iy: f32 = -0.8*s; let iz: f32 = 0.0*s;
let d: f32 = dot3(ix, iy, iz, 0.0, 1.0, 0.0);
let t: f32 = 2.0*d;
let px: f32 = ix - 0.0*t; let py: f32 = iy - 1.0*t; let pz: f32 = iz - 0.0*t;
let s2: f32 = ns(px, py, pz);
p bits(px*s2);   # 0x3F19999A
p bits(py*s2);   # 0x3F4CCCCD
p bits(pz*s2);   # 0x00000000
# march の hit 窓 inclusive 境界 (surf=5.0, th=0.5, step=0.5)
let surf: f32 = 5.0;
let k: i64 = 1;
while k <= 10 {
  let tv: f32 = len3(0.0, 0.0, f(k)*0.5);
  let df: f32 = surf - tv;
  if df >= 0.0 && df <= 0.5 { p tv; p bits(tv); k = 99; }
  k = k + 1;
}  # 4.5 (0x40900000) で hit = 上端 inclusive
'
```

## 11. 設計の非目標 (明示的に持たないもの)

配列・構造体・タプル・浮動小数 suffix・文字列演算・import・メモリ確保・
副作用 (時刻/乱数/ファイル)。必要になれば最小単位で追加し、
追加のたびに本書を更新する (構文の一次情報は常に本書)。
