# vendor ブランチ (自動生成・書き換え専用)
`ci/vendor.yml` (GitHub Actions) が `cargo vendor --locked` の成果物を
90MB 分割 tarball として置くブランチ。force push で常に単一コミット。
受け取り側は `ci/fetch_vendor.sh` が結合・検証・展開まで自動で行う。
