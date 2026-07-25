# DEV_ACCEL — 監査/dev ループ高速化ツールの一次情報調査と切替基準

2026-07-25 (wave 98 前工程)。ユーザー指示: 「毎回 cargo 叩いてると遅いから、
状況に応じて ast-grep / typos-cli / Kani Rust Verifier / Miri / cargo-nextest /
Mold・Sold リンカを切り替えて高速化。それぞれのツールについては検索」。

## サンドボックスの外向き到達性 (2026-07-25 実測)

| 到達先 | 可否 | 根拠実測 |
| :--- | :--- | :--- |
| api.github.com (gh api) | 可 | wgsl/index.bs 一次取得・release asset 一覧取得成功 |
| github.com git プロトコル | 可 | fetch/push/git clone (vendor ブランチ復元) |
| release-assets.githubusercontent.com | **不可** | typos/nextest の asset 取得が EOF/0 バイト |
| index.crates.io / static.crates.io | **不可** | `cargo add` が SSL unexpected EOF |
| files.pythonhosted.org (PyPI) | **可** | ast-grep-cli 0.45.0 wheel 取得成功 |
| registry.npmjs.org | 可 (問合せのみ確認) | `npm view` 応答あり |
| archive.ubuntu.com (apt) | 部分不可 | `mold` パッケージ索引なし |
| static.rust-lang.org (rustup) | **不可** | /home/user/rust に rustup 非含有・取得不可 |

## 6 ツールの一次情報調査 (検索結果要約)

| ツール | 目的 | 導入形 | 本環境での可否 |
| :--- | :--- | :--- | :--- |
| **ast-grep (sg)** | AST 構造検索/ lint / codemod。`sg -p '$X.unwrap()' -l rust`、YAML ルールで `sg scan` | cargo/pip/npm/GitHub releases | **導入済 0.45.0** (PyPI 公式 wheel = 作者 Herrington Darkholme、Project-URL = github.com/ast-grep/ast-grep、SBOM 同梱を検証。`~/tools/bin` へ展開) |
| **typos-cli** | ソース/ doc の typo 検査 (`typos` / `-w` 書換 / `_typos.toml` 語彙拡張) | cargo/GitHub releases | **不可** (crates 遮断・GitHub asset 遮断・PyPI に公式なし・npm は 2023-11-29 に Unpublished)。代替: 既存の Python 自家スキャン (不可視文字/簡体字/CRLF) を継続 |
| **Kani Rust Verifier** | bit 精密モデル検査。`#[kani::proof]` + `kani::any()` で全入力性質証明 (overflow/パニック/UB) | `cargo install --locked kani-verifier` + `cargo kani setup` (GitHub bundle) | **不可** (crates 遮断 + bundle = GitHub asset 遮断)。再有効化手順は末尾 |
| **Miri** | MIR インタプリタによる UB 検出 (OOB/UAF/データ競合/provenance)。`cargo +nightly miri test` | nightly + miri component | **不可** (static.rust-lang.org 遮断で nightly/miri 取得不可) |
| **cargo-nextest** | プロセス単位並列テストランナー。2-5x 高速・リトライ・分割・JUnit | cargo/GitHub releases/get.nexte.st | **不可** (crates 遮断・GitHub asset 遮断・PyPI/npm に公式なし)。現在の 967-971 本 lib suite は単一バイナリ内スレッド並列のため nextest 効果は中程度と評価 |
| **mold / sold** | 高速リンカ (ld/lld の drop-in、debug/incremental で大きな効果。sold は macOS 版だが作者 Rui Ueyama が「Apple 純正リンカ高速化で差分消失」として継続意義を消極評価) | apt/GitHub releases | **mold 不可** (apt 索引なし・GitHub asset 遮断)。**代替 lld 導入済**: Rust 1.94.1 同梱 rust-lld 21.1.8 を `-C link-arg=-fuse-ld=lld -B<toolchain>/bin/gcc-ld` で使用 (Linux ELF の現実的な最速代替。readelf .comment = "Linker: LLD 21.1.8" で検証済) |

## 切替基準 (状況別)

- **テスト実行**: 通常の `cargo test`(スレッド並列)。単発フィルタ (`<module>::tests::<name>`) で
  微小再実行 (<1s)。nextest が将来入ったら `cargo nextest run -E 'test(name)'` に切替。
- **リンク**: 全 cargo 呼出に lld RUSTFLAGS を付与 (本環境での統一フラグ:
  `RUSTFLAGS="-C link-arg=-fuse-ld=lld -C link-arg=-B/home/user/rust/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld"`)。
- **構造スイープ (監査の横断検査)**: ast-grep を第一選択 (unwrap/索引/キャスト/
  静寂破棄パターンの構造抽出)。テキスト grep は予備。
- **typo**: Python 自家スキャン (docs/internal/*.{md} と変更 .rs に毎 wave 適用)。
  typos-cli が入り次第 `_typos.toml` での語彙管理へ移行。
- **形式検証**: 純粋関数のオーバーフロー/不変条件は現状「Python f64/i128 での
  機械検算 + bit ピン」。Kani が入り次第 `#[kani::proof]` での全入力証明へ格上げ
  (候補: gpu_arena アライン繰上げ、mesh_cache wire デコード境界、
  quality_governor の saturating 契約)。
- **UB 検出**: Miri 不在のため現状は channel (weak-UB: release-wrap 静寂縮退) を
  監査で潰す方針。Miri 導入後は unsafe 含有モジュール (zerocopy_cast,
  vertex_compression_r10g10, bc7_ktx2 他) の test subset を `cargo +nightly miri test` で走査。

## 再有効化手順 (エグレス解放時)

```bash
cargo install --locked cargo-nextest typos-cli kani-verifier
cargo kani setup                               # Kani bundle
rustup toolchain install nightly --component miri  # rustup 導入後
sudo apt install mold   # or GitHub releases (rui314/mold)
# typos: `_typos.toml` で RSift 固有用語 (aokana 等) を語彙登録してから `typos` 走査
```

## 検証 (導入時実測)

- ast-grep: `unwrap()` 構造マッチ 263 件、索引式 1,548 件を opt-gfx 全域で抽出
  (wave 98 CW で `let _ = write_all` 静寂破棄 0 件・decode_all 残存特定
  (region_zstd.rs:107/170) の横断確認に実使用)。
- lld: hello world リンクで .comment = LLD 21.1.8、cargo テストビルド実使用。
- 全可否判定は計測値 (推測なし)。失敗経路は本ファイルに記録済。

## wave 103 (2026-07-25) 速度革命 — rspeed 単一バイナリ統合

ユーザー指示: 「数学の構文を超高速で解くもの / ファイルパスと文字列を
送ると超高速で検査するもの / その他遅いもの全部を 1 つの .rs にまとめて
高速化。独自の cargo test 的なものも作って高速化して」。

### 実装: `tools/rspeed.rs` (std のみ・`rustc -O` 直コンパイル 10 秒・`tools/build-rspeed.sh`)

従来 /tmp の使い捨て Python に分散していた検証系を 1 バイナリ
(`/home/user/bin/rspeed`) に統合 = **起動 ~2-3ms (Python 30-50ms+ の 1/20 級)
かつ sandbox クラッシュで消失しない**。

| サブコマンド | 機能 | 代替対象 (旧) | 実測 |
| :--- | :--- | :--- | :--- |
| `expr [--f32] [--frac]` | 再帰下降パーサで f64 評価+bits / f32 逐次再現 / Fraction 正確分数 (循環節表示) | Python Fraction/Decimal 検算 | 全ピン計算専用化 (例: `0.1+0.2` → 3.0000000000000004 bits 0x4008000000000001、`1/3` → 0.(3)、gcd) |
| `san` | 不可視文字 (U+200B/FEFF/NBSP/U+3000 等 12 種)・CRLF・末尾改行・文字化け (U+FFFD/U+00E3)・簡体字 298 字確定集合 (source 自身も 0 findings になるよう \u{...} エスケープ表記化) | /tmp/san_check.py | 5 files 一括 ~3ms |
| `find [--count]` | リテラル高速検索 (消費者照合) | grep -rn | repo 規模で即時 |
| `fmdiff` | rustfmt 逸脱行の HEAD ベースライン包含照合 (DP-LCS 内蔵、逸脱行集合⊆HEAD 集合の機械判定) | /tmp/fmt_verify2.py + 手作業 4 コマンド | render_graph.rs で 0.2s (HEAD 5 行/自己 0 行 PASS) |
| `test [-p crate] [filter]` | **独自 cargo test ランナー**: ソース指紋 (path+mtime+size+rustc ver) 不変なら cargo を起動せず target/debug/deps のテストバイナリを直接実行、変更時のみ `cargo test --no-run` 後に直接実行 | cargo test 毎回 | 指紋不変時 build SKIP 表示+実行尺のみ (full_graph 単体 161s、strict 系 0.00s) |
| `bench [example] [--expect-digest HEX]` | wide_static_bench 等の release 指紋ゲートビルド+直接実行+digest fail-loud 照合 | cargo run --example + 手 grep | release ビルド 100.7s→不変時 SKIP、`digest PASS: 004c1cf5fb17bfe8` |
| `regcount` | 台帳件数積算 (grep -cE '^\| (A期|[A-Z]{1,3})-[0-9]+' 意味論等価を手書きパーサで) | grep -cE | 346→353 積算に使用 |
| `audit-todo` | 棚卸し残抽出 (訂正版 comm 一式の内部実装) | grep+sed+comm 3 連 | 3ms (done=101/todo=79 一致) |
| `warn` | cargo check 警告数照合 | 手 grep | 14/17/13 据え置き確認済 |

### dev プロファイル実測 A/B (nproc=2 sandbox、2026-07-25)

| 構成 | フルビルド | lib テスト全実行 | 判定 |
| :--- | :--- | :--- | :--- |
| opt1 + debug=true (旧) | (基準) rsift-api test バイナリ 13.3MB / 10.56s | full_graph_periodic 系が長大 (CI も時間支配) | — |
| opt0 + debug=0 | **44.4s** | 312.4s (うち full_graph_wiring 1 テスト群 310s、単体 90s timeout 超過) | 却下 (実行支配で逆効果) |
| opt0 + debug=0 + `[profile.dev.package.rsift-opt-gfx] opt-level=1` (採用) | **37.8s** (deps opt0 再利用) | **149.8s** | **採用** |

根拠: 全テスト実行 312s の 99% 超が `tick_world_periodic_rebuild_cross_instance
_deterministic` 1 本系 (自重カーネル sim) で、opt-gfx **クレート自身** の
hot loop のみ opt1 にすれば実行速度を回復しつつ、registry deps (wgpu/zstd)
のビルド高速化 (opt0) とバイナリ縮小 (debug=0: 6.6x) を両立できる。
パニック Location・debug-assertions・overflow-checks は全て不変を実測確認
(lib 警告 14 / lib-test 17 / api 13 も据え置き)。

### 追加データポイント (本波の実測)

- mold 2.41.0-x86_64-linux.tar.gz: `gh release download` / `gh api … assets/<id>`
  両経路とも release-assets.githubusercontent.com で EOF (到達性表と一致) →
  **導入不可・lld 21.1.8 継続**。一試行 0.8s で判断可能になった。
- cargo-nextest: 同上 asset 不可 + nproc=2・単プロセススレッド並列で既に
  並列飽和しているため見送り継続 (asset 経路が開けば再評価)。

## rspeed v2 大拡張 (2026-07-25、115 機能・単一バイナリ 4,927,944 B)

ユーザー指示「rspeed に正確に行いたい機能を 100 個くらい自由に追加して」に対応。
`tools/rspeed.rs` 一本 (std のみ、`rustc -O --edition 2021` で ~9s) を 9 → **115 機能**
(dispatch エントリ 116 − エイリアス 1 (`grep`→`find`)) へ拡張。
ビルドは `bash tools/build-rspeed.sh` → `/home/user/bin/rspeed` (git 追跡外)。

### 追加バッチ構成 (全機能 `rspeed help` / `rspeed man <機能名>`)

| バッチ | 内訳 | 代表機能 |
| :--- | :--- | :--- |
| A 厳密数値 (27) | f64/f32/f16/bf16 bit 解析・RNG 再現・整数論 | bits/bits-of/ulp/next/hfbits/gamma/morton/murmur/splitmix/xs64/pcg/fnv/prime/modpow/invmod/contfrac/table/range/monotone/roundtrip/ulperr/int-cast/quant/mat4/vec3/lerp/hypot/fp-table |
| B スキャナ (24) | 監査用ソース走査 (strip_rust_code でコメント/文字列近似除去) | magic/floatlits/casts/clamps/divmod/shifts/unwraps/tests-index/test-find/test-count/fns/pubs (--save/--check API 指紋)/docs/todo-scan/dups/longlines/trailws/nonascii/eol/tabs/dead/hotfiles/diff (内蔵 LCS)/grep2 |
| C リポジトリ (28) | 巻戻り対策・adversarial 儀式・提出ゲートの機械化 | md5/md5check (自前 MD5、RFC1321 照合)/status/changed-tests/env-check/snapshot/snapcheck (全追跡ファイル manifest)/rescue/adv-save/adv-restore/adv-diff/time-run/binsize/ghfile/ghlatest/wave-log/journal/registry-stats/wave-info/todo-pick/todo-pri/coverage/ci-status/lines/burndown/**seal** (提出前一括ゲート: san→fmdiff→trailws→台帳→test→digest→env-check)/dashboard |
| D 統計/グラフィクス (26+1) | 数学ユーティリティと自己検証 | percentile/histogram/bigfact/fib/crc32/bitops/pack/unpack/endian/clamp-table/matc/quat/proj (wgpu/GL 両規約)/lookat/tri-area/bary/halton/r2/color/srgb-err/quat-slerp + **selftest (18 ピン)・man・nextwave** |

### 精度保証 (全数値 Python/hashlib/仕様値の独立検算で一致確認済)

- MD5 自前実装: `md5("")=d41d8cd98f00b204e9800998ecf8427e`、`md5("abc")=900150983cd24fb0d6963f7d28e17f72` (md5sum・hashlib 両照合)
- splitmix64(0x42)=[0x2c1c719d2c17b759,0xa211b519d9a09a1c]、fmix64(4095)=0x29a9375b70e7db5a、fmix64(0xdeadbeef)=0xd24bd59f862a1dac (Python 再実装と完全一致)
- f16 変換 RNE (0x2e66/0x3e00)、morton2(14,5)=0x93、fnv1a64("hello")=0xa430d84680aabd0b、pcg32[1]=0x75830bbd、invmod(3,121)=81、gamma(0.5) sRGB enc/dec、halton(5,2)=5/8、crc32("hello")=0x3610a686、gcd(1071,1029)=21
- `seal` 全ゲート PASS を以って提出可能の定義とする (本コミット前に全ゲート実測 PASS)

### 開発中の自己捕捉 6 件 (ツール作成時のテスト赤/実害を根治)

1. **selftest ピン誤記**: 仮ピン 0x4008000000000001 (= (0.1+0.2)*10) → 正値 0x3fd3333333333334 (0.1+0.2) に訂正 (selftest 即 RED で捕捉)
2. **monotone 判定 rc 設計訂正**: 単調非増加を rc=1 扱いしていた誤り → 増減混在のみ rc=1 (非増加/非減少は共に単調として rc=0) + bits の指数部ラベル誤植 (e→e_raw) 訂正
3. **morton part1by2/compact1by2 定数誤記** [selftest 捕捉最大事例]: マジック定数を `0x1f0000000000ffff`/`0x1f0000ffff000000ff` と転記誤り (正: `0x001f_0000_0000_ffff`/`0x001f_0000_ff00_00ff`)。誤値では入力 bit8-20 が**静寂ゼロ化** (= 256 以上の座標で interleave 往復が壊れる潜伏実害、LUT 不要の morton 展開に直結) → 定数直値化 + exhaustive 往復検証 (2D 0..65536、3D 0..100,000+境界、仕様直交 bit i→3i 0..1024) 全 PASS
4. **parse_u64_auto 追加**: 「数字のみ文字列を hex 優先解釈」する従来式 `from_str_radix(16).or_else(parse)` は `"4095"` を 0x4095=16533 に誤読 (pack 4 12 9 4095 で ROUNDTRIP-FAIL として顕在化) → 0x/0X 接頭辞=a-f 含有のみ hex、それ以外 decimal の自動基数判別ヘルパーへ統一 (murmur/bitops/pack/unpack/endian/splitmix/xs64/pcg 適用、pack/unpack の `unwrap_or(0)` 静寂ゼロ化も fail-loud 化)
5. **snapshot タグ未サニタイズ + 書込み静寂スロー**: `snapshot /tmp/x` でタグ文字列をそのままファイル名化 → 存在しない下位パスで `let _ = fs::write(...)` が失敗を握り潰し「保存した」と誤報 → `snapshot_file()` で `/`,`\\` を `_` 化して統一 + **`write_loud()` ヘルパーを新設しユーザー要求の永続化書込み 9 サイト (tests-index/pubs --save/rescue×2/MANIFEST/adv-save×2/wave-log/fp_store) を全て fail-loud 化**
6. **コンパイラ警告 4 件根治**: extract_tests 未使用 `path` 引数は削除側で正直化 (呼出 3 箇所追従) / `cnt` 死に代入は構造削除 (`refs += ok`) / 未使用 `nf` 行削除 / `qa` の不要 `mut` 除去 → `rustc -O` 警告 0

### 不変量 (本拡張後に再実測)

- `rustfmt --check tools/rspeed.rs` PASS (全行正準形)・`san` 0 findings 維持
- selftest 18/18 PASS・警告 0 (`rustc -O`)
- クレート側は無変更のまま 1000 テスト全緑 (161.17s)・digest `004c1cf5fb17bfe8` rows=357 不変
- trailws 走査で HEAD 既存 3 件を発見 (compute_light_prop.rs:48/56, entity_culling.rs:419) — 棚卸し候補として記録 (本波では触らず)
