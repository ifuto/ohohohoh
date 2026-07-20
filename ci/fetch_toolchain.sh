#!/usr/bin/env bash
# ============================================================================
# GitHub 上の `vendor` ブランチ (ci/vendor ワークフローが Rust toolchain を
# 公式 sha256 照合済みで追記した分割 tarball) から Rust toolchain を取得し、
# $HOME/rust へインストールする。cargo / rustc / clippy / rustfmt を含む。
#
# 使い方:
#   bash ci/fetch_toolchain.sh
#
# 前提: git で github.com に繋がること。static.rust-lang.org への接続は不要
#       (ランナーが公式ハッシュ照合済みのものを git 経由で中継する)。
# 配置先は既定 $HOME/rust (環境リセットで消えた場合はこのスクリプト再実行で復旧)。
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BRANCH="${VENDOR_BRANCH:-vendor}"
PREFIX_DIR="${TOOLCHAIN_DIR:-$HOME/rust}"
PART_PREFIX="toolchain.tar.xz.part-"
SUMS="TOOLCHAIN-SHA256SUMS"

echo "[1/4] git fetch origin $BRANCH"
git -C "$REPO_ROOT" fetch origin "+refs/heads/${BRANCH}:refs/remotes/origin/${BRANCH}"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "[2/4] origin/$BRANCH から toolchain パーツ取得 + SHA-256 検証"
git -C "$REPO_ROOT" archive "origin/$BRANCH" | tar -x -C "$WORK" --wildcards "${PART_PREFIX}*" "$SUMS"
if ! ls "$WORK"/${PART_PREFIX}* >/dev/null 2>&1; then
  echo "origin/$BRANCH に toolchain パーツが見つかりません。" >&2
  echo "先に toolchain 取得ステップ入りのワークフローを Actions で実行してください。" >&2
  exit 1
fi
( cd "$WORK" && sha256sum -c "$SUMS" )

echo "[3/4] 結合 → 展開 → install (--components=rustc,cargo,rust-std,clippy,rustfmt)"
cat "$WORK"/${PART_PREFIX}* > "$WORK/toolchain.tar.xz"
mkdir -p "$WORK/tc"
tar -xJf "$WORK/toolchain.tar.xz" -C "$WORK/tc"
INNER="$(find "$WORK/tc" -mindepth 1 -maxdepth 1 -type d | head -1)"
[ -x "$INNER/install.sh" ] || { echo "install.sh が見つかりません: $INNER" >&2; exit 1; }
mkdir -p "$PREFIX_DIR"
"$INNER/install.sh" \
  --prefix="$PREFIX_DIR" \
  --disable-ldconfig \
  --components=rustc,cargo,rust-std-x86_64-unknown-linux-gnu,clippy-preview,rustfmt-preview

echo "[4/4] 動作確認"
export PATH="$PREFIX_DIR/bin:$PATH"
rustc --version
cargo --version
cargo clippy --version
cargo fmt --version
echo "OK: toolchain = $PREFIX_DIR"
