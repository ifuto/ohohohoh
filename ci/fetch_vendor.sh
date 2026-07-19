#!/usr/bin/env bash
# ============================================================================
# GitHub 上の `vendor` ブランチ (ci/vendor.yml が Actions 上で生成した
# cargo vendor の分割 tarball) を取得し、この sandbox でオフラインの
# cargo check / clippy が回る状態を作る。
#
# 使い方:
#   bash ci/fetch_vendor.sh              # 取得~配置まで
#   RUN_CHECK=1 bash ci/fetch_vendor.sh  # 配置後に cargo check まで実行
#
# 前提: git で github.com に繋がること。crates.io への接続は不要。
# 配置先は既定 /tmp/rsift-vendor (環境リセットで消えた場合はこのスクリプト再実行で復旧)。
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WS_ROOT="$REPO_ROOT"
# ネスト構造対応: rsift/rsift/Cargo.toml があればそちらが cargo ワークスペース
if [ -f "$REPO_ROOT/rsift/Cargo.toml" ]; then
  WS_ROOT="$REPO_ROOT/rsift"
fi

BRANCH="${VENDOR_BRANCH:-vendor}"
DEST="${VENDOR_DIR:-/tmp/rsift-vendor}"
PREFIX="vendor.tar.gz.part-"

echo "[1/5] git fetch origin $BRANCH"
if ! git -C "$REPO_ROOT" fetch origin "+refs/heads/${BRANCH}:refs/remotes/origin/${BRANCH}"; then
  if git -C "$REPO_ROOT" rev-parse --verify --quiet "refs/remotes/origin/${BRANCH}" >/dev/null; then
    echo "    (注意) fetch に失敗したため、取得済みの origin/$BRANCH を使います"
  else
    echo "origin に $BRANCH ブランチがありません。" >&2
    echo "先に ci/vendor.yml を GitHub Actions で実行してください (手順: ci/README.md)。" >&2
    exit 1
  fi
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "[2/5] origin/$BRANCH の内容を取り出し + SHA-256 検証"
git -C "$REPO_ROOT" archive "origin/$BRANCH" | tar -x -C "$WORK"
( cd "$WORK" && sha256sum -c SHA256SUMS )

echo "[3/5] 分割パーツを番号順に結合して $DEST へ展開"
# 事故防止: DEST が空/ルートの場合は中断
if [ -z "$DEST" ] || [ "$DEST" = "/" ]; then
  echo "VENDOR_DIR が不正です: '$DEST'" >&2; exit 1
fi
rm -rf "$DEST" && mkdir -p "$DEST"
# glob は part-00, part-01, ... と辞書順=番号順で展開される (-d -a 2 前提)
cat "$WORK"/${PREFIX}* | tar -xz -C "$DEST"
echo "    vendored crates: $(ls "$DEST/vendor" | wc -l)"

echo "[4/5] $WS_ROOT/.cargo/config.toml を生成 (ローカル専用・コミット対象外)"
mkdir -p "$WS_ROOT/.cargo"
cat > "$WS_ROOT/.cargo/config.toml" <<EOF
# ci/fetch_vendor.sh が生成したローカル設定。絶対パスを含むためコミットしないこと。
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "$DEST/vendor"
EOF

# 誤コミット防止: 共有される .gitignore ではなくローカルの info/exclude に登録
REL="$(realpath --relative-to="$REPO_ROOT" "$WS_ROOT/.cargo/config.toml")"
EXC="$REPO_ROOT/.git/info/exclude"
touch "$EXC"
grep -qxF "$REL" "$EXC" || echo "$REL" >> "$EXC"

echo "[5/5] 完了: vendor = $DEST/vendor"
if [ "${RUN_CHECK:-0}" = "1" ]; then
  if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/rust/bin/cargo" ]; then
    export PATH="$HOME/rust/bin:$PATH"
  fi
  echo "== cargo check --workspace --all-targets --locked =="
  ( cd "$WS_ROOT" && cargo check --workspace --all-targets --locked )
fi
echo "OK"
