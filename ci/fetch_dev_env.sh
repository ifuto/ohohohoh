#!/usr/bin/env bash
# fetch_dev_env.sh — `dev-assets` ブランチの assets/ に置かれた分割ファイルを
# 結合し、このサンドボックスで cargo check / clippy が動く状態を構築する。
# 入力形式は問わない: 巨大 ZIP / rust-*.tar.xz / vendor.tar.gz / vendor/ ディレクトリ、
# それぞれ単体でも N 分割 (.001, .002 ...) でも可。MANIFEST/README は除外。
# 環境リセット後の再実行も冪等。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE="$REPO_ROOT/rsift"
RUST_PREFIX="$HOME/rust"
ASSET_BRANCH="${ASSET_BRANCH:-dev-assets}"

echo "== [1/6] fetch $ASSET_BRANCH =="
git -C "$REPO_ROOT" fetch origin "$ASSET_BRANCH"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT

echo "== [2/6] assets/ 内を basename グループ毎に番号順結合 =="
mapfile -t files < <(git -C "$REPO_ROOT" ls-tree -r --name-only "origin/$ASSET_BRANCH" -- assets/ | sort)
if [ "${#files[@]}" -eq 0 ]; then
    echo "ERROR: origin/$ASSET_BRANCH の assets/ にファイルがありません" >&2
    exit 1
fi

declare -A groups
for f in "${files[@]}"; do
    base="$(basename "$f")"
    case "$base" in
        MANIFEST*|README*|*.md) echo "   skip (manifest/docs): $base"; continue ;;
    esac
    if [[ "$base" =~ \.[0-9]{3,}$ ]]; then
        groups["${base%.*}"]+="$f "
    else
        groups["$base"]+="$f "
    fi
done

extract_archive() { # $1=input  $2=dest-dir
    mkdir -p "$2"
    case "$1" in
        *.zip)          python3 -c "import zipfile,sys; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" "$1" "$2" ;;
        *.tar.gz|*.tgz) tar -xzf "$1" -C "$2" ;;
        *.tar.xz)       tar -xJf "$1" -C "$2" ;;
        *) echo "   (unknown archive type: $1 — 生ファイルとして配置)"; mkdir -p "$2"; cp "$1" "$2/" ;;
    esac
}

for stem in "${!groups[@]}"; do
    out="$tmp/$stem"
    # shellcheck disable=SC2086
    for f in $(echo ${groups[$stem]} | tr ' ' '\n' | sort); do
        git -C "$REPO_ROOT" show "origin/$ASSET_BRANCH:$f"
    done > "$out"
    echo "   結合: $stem (${groups[$stem]}) -> $(du -h "$out" | cut -f1)"
    # stem 名に archive 拡張子が無い場合 (e.g. big.zip.001 の stem は big.zip) でも
    # 先頭マジックで形式判定する
    magic="$(head -c4 "$out" | od -An -tx1 | tr -d ' \n')"
    case "$magic" in
        504b0304|504b0506|504b0708) mv "$out" "$out.zip"; out="$out.zip" ;;
        1f8b*)                      mv "$out" "$out.tar.gz"; out="$out.tar.gz" ;;
        fd377a585a5a00*)            mv "$out" "$out.tar.xz"; out="$out.tar.xz" ;;
    esac
    extract_archive "$out" "$tmp/stage"
done

echo "== [3/6] Rust toolchain 導入 ($RUST_PREFIX) =="
install_toolchain_dir() { # $1 = install.sh があるツールチェーンディレクトリ
    ( cd "$1" && ./install.sh --prefix="$RUST_PREFIX" )
    for d in "$1"/clippy-preview* "$1"/rustfmt-preview*; do
        [ -d "$d" ] && ( cd "$d" && ./install.sh --prefix="$RUST_PREFIX" )
    done
}
tc_tar="$(find "$tmp/stage" -name 'rust-*.tar.xz' 2>/dev/null | head -1 || true)"
tc_dir=""
if [ -n "$tc_tar" ]; then
    mkdir -p "$tmp/rust-dist"
    tar -xJf "$tc_tar" -C "$tmp/rust-dist"
    tc_dir="$(find "$tmp/rust-dist" -maxdepth 1 -mindepth 1 -type d | head -1)"
else
    tc_dir="$(dirname "$(find "$tmp/stage" -maxdepth 3 -name install.sh -path '*rust*' 2>/dev/null | head -1 || true)")"
fi
if [ -n "$tc_dir" ] && [ -f "$tc_dir/install.sh" ]; then
    install_toolchain_dir "$tc_dir"
    export PATH="$RUST_PREFIX/bin:$PATH"
    rustc --version && cargo --version && rustfmt --version
    cargo clippy --version || echo "WARN: clippy 未導入"
else
    echo "ERROR: rust toolchain が見つかりません (rust-*.tar.xz または install.sh 付きディレクトリを assets/ に配置してください)" >&2
    exit 1
fi

echo "== [4/6] cargo vendor 配置 =="
vd="$(find "$tmp/stage" -maxdepth 4 -type d -name vendor 2>/dev/null | head -1 || true)"
vg="$(find "$tmp/stage" -name 'vendor.tar.gz' 2>/dev/null | head -1 || true)"
mkdir -p "$WORKSPACE/vendor"
if [ -n "$vd" ]; then
    cp -a "$vd/." "$WORKSPACE/vendor/"
    echo "   vendor/ ディレクトリを配置: $(du -sh "$WORKSPACE/vendor" | cut -f1)"
elif [ -n "$vg" ]; then
    tar -xzf "$vg" -C "$WORKSPACE"
    echo "   vendor.tar.gz を展開: $(du -sh "$WORKSPACE/vendor" | cut -f1)"
else
    echo "   WARN: vendor が見つかりません — cargo check/clippy は依存解決 (crates.io 遮断) で失敗します" >&2
fi

mkdir -p "$WORKSPACE/.cargo"
cat > "$WORKSPACE/.cargo/config.toml" <<'EOF'
# サンドボックスは crates.io 遮断 — cargo vendor したローカルソースへ完全置換。
# (このファイルは意図的に git 管理外: vendor/ 自体は dev-assets branch で配布)
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

echo "== [5/6] cargo fmt 差分確認 (informational) =="
( cd "$WORKSPACE" && cargo fmt --all -- --check ) || echo "   (fmt 差分あり — 別途対処)"

echo "== [6/6] cargo check / clippy =="
cd "$WORKSPACE"
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
echo "== DONE =="
