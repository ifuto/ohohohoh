# アセット置き場

ここに **25MB 以下に分割したファイル** を Web UI (Add file → Upload files) で
アップロードしてください。エージェントが `assets/` 配下の全ファイルを走査し、
`.001 .002 …` 番号順に結合 → 形式自動判定 → toolchain 導入 + vendor 配置まで行います。

- 分割はリポジトリの `ci/dev-assets/splitter.html` (ブラウザで開くだけ) を使用
- MANIFEST.txt / この README は結合対象から自動除外されます
- 必要な物: rust-1.94.1-x86_64-unknown-linux-gnu.tar.xz (または展開済み) + vendor(/ または tar.gz)
