#!/usr/bin/env bash
# 配布用セットアップ成果物 (Windows .exe zip / macOS .app zip) ビルドスクリプト。
# 使い方:
#   bash ci/build-setup-artifacts.sh              # 全 4 target (toolchain があれば)
#   bash ci/build-setup-artifacts.sh --local      # ホスト target のみ (検証用)
#
# 必要 toolchain (この sandbox には無いもの含む — 一度だけ):
#   rustup target add x86_64-pc-windows-msvc aarch64-apple-darwin x86_64-apple-darwin
# (windows-msvc にはさらに MSVC link.exe 経路、あるいは windows-gnu なら mingw 不要の
#  rust-lld 経路が必要。apple target は Xcode SDK 必須)
#
# 出力: rsift/dist/setup/ に下記 zip + SHA256SUMS.txt
#   rsift-setup-windows-x64.zip    ... rsift-setup.exe + README_JA.txt
#   Rsift-Setup-macos-arm64.zip    ... Rsift Setup.app (aarch64)
#   Rsift-Setup-macos-x64.zip      ... Rsift Setup.app (x86_64)
# ※ dll/dylib 本体 (rsift.dll 等) はエンジン配布 wave で同梱される。
#   setup は「同階層に置くだけ」で動く自己完結バイナリ (std のみ)。
set -euo pipefail
cd "$(dirname "$0")/../rsift"

out=dist/setup
rm -rf "$out"; mkdir -p "$out"

cat > "$out/README_JA.txt" <<'EOF'
Rsift セットアップ (使い方)
1. この zip を解凍
2. rsift-setup.exe (Windows) / Rsift Setup.app (Mac) と、
   同封の dll / dylib を全部 同じフォルダ に置く
3. rsift-setup を実行
4. rsift_launch.json ができたら準備完了
失敗しても大丈夫: rsift_setup_log.txt をそのまま送ってください
(終了コード: 0=OK / 2=lib無し / 3=不一致 / 4=IO失敗 / 5=自己診断失敗)
EOF

build_one () { # $1=target $2=outdir $3=app(0/1)
  local tgt="$1" dst="$2" is_app="$3"
  cargo build -p rsift-setup --release --target "$tgt" --locked
  mkdir -p "$dst"
  if [ "$is_app" = 1 ]; then
    local app="$dst/Rsift Setup.app/Contents"
    mkdir -p "$app/MacOS"
    cp "target/$tgt/release/rsift-setup" "$app/MacOS/rsift-setup"
    cat > "$app/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
 <key>CFBundleName</key><string>Rsift Setup</string>
 <key>CFBundleDisplayName</key><string>Rsift Setup</string>
 <key>CFBundleIdentifier</key><string>dev.ifuto.rsift.setup</string>
 <key>CFBundleVersion</key><string>1.0</string>
 <key>CFBundleExecutable</key><string>rsift-setup</string>
 <key>CFBundlePackageType</key><string>APPL</string>
 <key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
    chmod +x "$app/MacOS/rsift-setup"
  else
    cp "target/$tgt/release/rsift-setup.exe" "$dst/rsift-setup.exe"
  fi
  cp README_JA.txt "$dst/" 2>/dev/null || true
}

if [ "${1:-}" = "--local" ]; then
  echo "[local] ホスト検証ビルドのみ"
  cargo build -p rsift-setup --release --locked
  echo "OK: rsift/target/release/rsift-setup"
  exit 0
fi

cp "$out/README_JA.txt" README_JA.txt
for spec in "x86_64-pc-windows-msvc windows-x64 0" \
            "aarch64-apple-darwin macos-arm64 1" \
            "x86_64-apple-darwin macos-x64 1"; do
  set -- $spec
  echo "== build $1 =="
  build_one "$1" "$out/pack-$2" "$3"
done

cd "$out"
cp ../README_JA.txt pack-windows-x64/ 2>/dev/null || cp ../../README_JA.txt pack-windows-x64/ 2>/dev/null || true
(cd pack-windows-x64 && zip -qr ../rsift-setup-windows-x64.zip .)
(cd pack-macos-arm64  && zip -qr ../Rsift-Setup-macos-arm64.zip .)
(cd pack-macos-x64    && zip -qr ../Rsift-Setup-macos-x64.zip .)
sha256sum *.zip > SHA256SUMS.txt
rm -rf pack-*
echo "成果物:"; ls -la
echo "※ .app は未署名 (ローカル配布運用)。Gatekeeper は初回右クリック→開くで通す"
