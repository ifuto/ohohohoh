#!/usr/bin/env bash
# fetch_dev_env.sh — orphan branch `dev-assets` から toolchain + vendor を取得し、
# このサンドボックスで cargo check / clippy が動く状態を構築する。
# 前提: git fetch できること (github.com は疎通確認済み)。環境リセット後の再実行も冪等。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE="$REPO_ROOT/rsift"
RUST_PREFIX="$HOME/rust"
ASSET_BRANCH="${ASSET_BRANCH:-dev-assets}"
ASSET_REMOTE_REPO="$(git -C "$REPO_ROOT" remote get-url origin)"

echo "== [1/5] fetch $ASSET_BRANCH =="
git -C "$REPO_ROOT" fetch origin "$ASSET_BRANCH"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "== [2/5] 分割アセットを git show で結合 =="
concat_assets() {
    local pattern="$1" out="$2"
    git -C "$REPO_ROOT" ls-tree -r --name-only "origin/$ASSET_BRANCH" \
      | grep -E "$pattern" | sort | while read -r f; do
          git -C "$REPO_ROOT" show "origin/$ASSET_BRANCH:$f"
        done > "$out"
    echo "   -> $out ($(du -h "$out" | cut -f1))"
}
concat_assets 'rust.*\.tar\.xz\.[0-9]+$' "$tmp/rust.tar.xz"
concat_assets 'vendor\.tar\.gz\.[0-9]+$|vendor\.tar\.gz$' "$tmp/vendor.tar.gz"

echo "== [3/5] Rust toolchain 導入 ($RUST_PREFIX) =="
mkdir -p "$tmp/dist" && tar -xJf "$tmp/rust.tar.xz" -C "$tmp/dist"
dist_dir="$(find "$tmp/dist" -maxdepth 1 -mindepth 1 -type d | head -1)"
( cd "$dist_dir" && ./install.sh --prefix="$RUST_PREFIX" )
# clippy / rustfmt はデフォルト導入されないため個別に追加 (ディレクトリ名差異を許容)
for d in "$dist_dir"/clippy-preview* "$dist_dir"/rustfmt-preview*; do
    [ -d "$d" ] && ( cd "$d" && ./install.sh --prefix="$RUST_PREFIX" )
done
export PATH="$RUST_PREFIX/bin:$PATH"
rustc --version && cargo --version && rustfmt --version && cargo clippy --version

echo "== [4/5] vendor 展開 + cargo 設定 =="
mkdir -p "$WORKSPACE/vendor"
tar -xzf "$tmp/vendor.tar.gz" -C "$WORKSPACE"
mkdir -p "$WORKSPACE/.cargo"
cat > "$WORKSPACE/.cargo/config.toml" <<'EOF'
# サンドボックスは crates.io 遮断 — cargo vendor したローカルソースへ完全置換。
# (このファイルは意図的に git 管理外: vendor/ 自体は dev-assets branch で配布)
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

echo "== [5/5] 検証実行 =="
cd "$WORKSPACE"
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
echo "== DONE =="
