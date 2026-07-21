#!/usr/bin/env bash
# restore-env.sh — vendor ブランチから Rust ツールチェーンと crates vendor
# キャッシュを復元する。sandbox リセットで ~/rust と /tmp が消えたとき、
# このスクリプト 1 本でローカルのビルド/テスト/ベンチ環境が復活する。
#
# 信頼チェーン (サプライチェーン規律):
#   toolchain tarball は GitHub Actions (.github/workflows/main.yml on main)
#   が static.rust-lang.org 公式配布物を公式 .sha256 と照合済みのものを
#   90MB 分割で vendor ブランチへ push したもの。本スクリプトは受け取り側でも
#   分割パーツ単位の sha256 (workflow が同梱した TOOLCHAIN-SHA256SUMS /
#   SHA256SUMS) を照合してから結合・展開する。一致しなければ即中断する。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK=/tmp/vendor-br
VENDOR_DIR=/tmp/rsift-vendor
PREFIX="$HOME/rust"

echo "== vendor ブランチ取得 =="
rm -rf "$WORK"
git clone --depth 1 --branch vendor https://github.com/ifuto/rsift.git "$WORK"
cd "$WORK"
sha256sum -c TOOLCHAIN-SHA256SUMS
sha256sum -c SHA256SUMS

echo "== Rust toolchain 展開 → $PREFIX =="
cat toolchain.tar.xz.part-* > toolchain.tar.xz
EX=/tmp/rust-ex
rm -rf "$EX" && mkdir -p "$EX"
tar xf toolchain.tar.xz -C "$EX"
"$EX"/rust-*-x86_64-unknown-linux-gnu/install.sh --prefix="$PREFIX" \
  --components=rustc,cargo,rust-std-x86_64-unknown-linux-gnu,rustfmt-preview,clippy-preview

echo "== crates vendor キャッシュ展開 → $VENDOR_DIR =="
mkdir -p "$VENDOR_DIR"
cat vendor.tar.gz.part-* | tar xz -C "$VENDOR_DIR"

echo "== cargo ソース置換設定 (ローカル専用・git 追跡外) =="
mkdir -p "$REPO_ROOT/rsift/.cargo"
cat > "$REPO_ROOT/rsift/.cargo/config.toml" <<EOF
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "$VENDOR_DIR/vendor"
EOF
grep -qx '/rsift/.cargo/' "$REPO_ROOT/.git/info/exclude" 2>/dev/null || \
  echo '/rsift/.cargo/' >> "$REPO_ROOT/.git/info/exclude"

export PATH="$PREFIX/bin:$PATH"
rustc -V && cargo -V
echo
echo "OK: 以降は次でビルド/テスト/ベンチが回ります:"
echo "  export PATH=\"$PREFIX/bin:\$PATH\""
echo "  cd $REPO_ROOT/rsift"
echo "  cargo build --release -p rsift-opt-gfx --example pseudo_mc_bench --locked --offline"
echo "  cargo test  -p rsift-opt-gfx --lib --locked --offline"
