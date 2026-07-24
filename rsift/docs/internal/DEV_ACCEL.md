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
