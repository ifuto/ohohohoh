# 開発環境アセット手動配布ガイド (Web UI 25MB 分割版 — CI 不要)

## これは何

サンドボックス側のネットワーク制限 (公式ミラー・CDN 全遮断、git プロトコルのみ疎通 — 全経路実測済み)
のため、Rust toolchain と cargo vendor を **git 経由** でエージェントへ受け渡す手順です。

> Releases 添付は「上げられるが sandbox から取り出せない」(`gh release download` も CDN で EOF)
> ことも実測済みのため、本ブランチ (`dev-assets`) 方式を使います。

## 手順 (ブラウザ操作のみで完結・git CLI 不要)

### 1. 巨大 ZIP を 25MB 分割する — `splitter.html`

リポジトリ内の **`ci/dev-assets/splitter.html`** をローカルに保存してブラウザで開き、
巨大 ZIP をドラッグ&ドロップ → 「✂️ 分割する」。

- ブラウザ内オフライン完結 (ファイルは外部送信されません)
- 既定 25,000,000 bytes/chunk (GitHub web UI の 25MB 上限に安全に収まる)
- **SHA-256 つき MANIFEST.txt** も生成 (増分ストリーム計算、既知ベクトル&Node crypto で検証済み)
- パート名は `big.zip.001, .002 …` (エージェント側が番号順に自動結合)

### 2. `dev-assets` ブランチの `assets/` に Web UI でアップロード

ブランチはエージェント側で作成済み:

1. <https://github.com/ifuto/rsift/tree/dev-assets> を開く
2. `assets/` フォルダへ移動 → **Add file → Upload files**
3. 分割ファイル (各 ≤25MB) をドラッグ → **Commit changes** (バッチ毎に数回に分けて OK、
   1ファイル1コミットでも OK — エージェント側は全ファイルを走査します)

### 3. エージェントに「上げた」と伝える

`ci/fetch_dev_env.sh` が自動で: fetch → 番号順結合 → マジックバイト判定で形式自動検出
(ZIP/tar.gz/tar.xz) → toolchain install → vendor 配置 → `cargo fmt --check` /
`cargo check --workspace --all-targets --locked` / `cargo clippy` まで実行します。

## 入れてほしい内容 (どちらも必須)

| 物 | 取得方法 | 目安 |
|---|---|---|
| Rust toolchain (Linux x86_64!!) | <https://static.rust-lang.org/dist/rust-1.94.1-x86_64-unknown-linux-gnu.tar.xz> (公式一次情報。Windows 版不可) | ~190MB |
| cargo vendor (全依存ソース) | `rsift/` で `cargo vendor --locked vendor` | ~100–200MB |

巨大 ZIP の中身のレイアウト例 (どれでも可 — スクリプトが自動検出):

```text
big.zip
├── rust-1.94.1-x86_64-unknown-linux-gnu.tar.xz   (tarball そのまま)
└── rsift/vendor/ または vendor/                  (vendor ディレクトリ)

または
├── rust-1.94.1-x86_64-unknown-linux-gnu/         (tarball 展開済み + install.sh)
└── vendor.tar.gz
```

## 代替: git CLI 派 (90MB 分割 → 直接 push)

```powershell
# 90MB 分割 (PowerShell 関数)
function Split-BigFile($in) {
  $fs = [IO.File]::OpenRead($in); $buf = New-Object byte[] 90MB; $i = 0
  while (($n = $fs.Read($buf, 0, $buf.Length)) -gt 0) {
    $o = "{0}.{1:D3}" -f $in, ++$i
    $w = [IO.File]::Create($o); $w.Write($buf, 0, $n); $w.Close()
  }; $fs.Close()
}
```

```cmd
git switch dev-assets
:: 分割ファイルを assets\ に配置
git add assets && git commit -m "chore: toolchain + vendor"
git push origin dev-assets
git switch -
```
