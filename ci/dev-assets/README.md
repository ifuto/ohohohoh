# 開発環境アセット手動配布ガイド (CI 不要版)

サンドボックス側のネットワーク制限のため、公式ミラー経由の自動ダウンロードは全滅でした
(実測 2026-07-20: `static.rust-lang.org`, `sh.rustup.rs`, TUNA/USTC/上交/阿里/華為/騰訊/npmmirror,
Docker Hub, GHCR, conda-forge, HuggingFace **全て遮断**。疎通するのは github.com / api.github.com /
codeload.github.com / docs.rs / npm / PyPI のみ)。
**GitHub リリースの asset CDN (release-assets.githubusercontent.com) も遮断** のため、
リリース添付ではなく **git プロトコル経由 (専用 orphan branch)** で配布します。

## 必要なもの (2 点)

### 1. Rust toolchain — Linux x86_64 用 tarball (**Windows 版ではない**)

- ダウンロード先 (公式一次情報): <https://static.rust-lang.org/dist/rust-1.94.1-x86_64-unknown-linux-gnu.tar.xz> (~250MB)
- 検証用ハッシュ: <https://static.rust-lang.org/dist/rust-1.94.1-x86_64-unknown-linux-gnu.tar.xz.sha256>
- 404 の場合は <https://static.rust-lang.org/dist/index.html> で正確なファイル名を確認
- ※ サンドボックスは Linux x86_64。`rustup-init.exe` や MSVC 版を上げても動きません

### 2. cargo vendor (全依存クレートのソース, ~100–200MB)

リポジトリの `rsift/` ディレクトリで (cargo が動く PC で):

```cmd
cd rsift
cargo vendor --locked vendor
tar -czf ..\vendor.tar.gz vendor
```

## 分割 (GitHub の 100MB/ファイル上限対策)

Windows の PowerShell で 90MB ずつ分割:

```powershell
function Split-BigFile($in) {
  $fs = [IO.File]::OpenRead($in); $buf = New-Object byte[] 90MB; $i = 0
  while (($n = $fs.Read($buf, 0, $buf.Length)) -gt 0) {
    $o = "{0}.{1:D3}" -f $in, ++$i
    $w = [IO.File]::Create($o); $w.Write($buf, 0, $n); $w.Close()
  }
  $fs.Close()
}
Split-BigFile "rust-1.94.1-x86_64-unknown-linux-gnu.tar.xz"
Split-BigFile "vendor.tar.gz"
```

## アップロード (orphan branch — main 系の履歴を汚染しない)

```cmd
git checkout --orphan dev-assets
git rm -rf .
mkdir assets
:: 分割ファイル (rust-*.tar.xz.001, vendor.tar.gz.001 等) を assets\ にコピー
git add assets
git commit -m "chore: sandbox dev toolchain (Linux rust 1.94.1 + cargo vendor)"
git push origin dev-assets
git switch -
```

(代替: `ifuto/rsift-dev-assets` のような専用リポジトリを作って push してもらっても OK)

## 完了後

エージェントに「dev-assets に上げた」と伝えてください。
`ci/fetch_dev_env.sh` が自動で取得・結合・展開し、`cargo check` / `cargo clippy` まで実行します。
