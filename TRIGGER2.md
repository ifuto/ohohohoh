# TRIGGER2 (setup-artifacts 実行ペイロード)

このファイルが更新されると `setup-artifacts-trigger` ワークフローが起き、
下の ```bash ブロックだけが ubuntu/windows/macos の3台で実行される。
run 番号を1つ増やして push するのが「実行の合図」(起動条件はファイル差分)。

- run: 3
- 目的: rsift-setup バイナリ + **エンジン dll (rsift_api cdylib) + 2 Mod cdylib
  (RsGraphics=rsgraphics, RsReplay=rsreplay)** をビルドし、zip 展開したら
  全部同じフォルダに dll が並ぶ 1 梱包形式で `rsift/dist-ci/` へ出力。
  ワークフローが Artifacts + Release `setup-v1` へ添付する。
- 収録物 (Windows 例): rsift-setup.exe / rsift.dll / rsgraphics.dll / rsreplay.dll

```bash
echo "[trigger2] start os=$RUNNER_OS arch=$(uname -m) time=$(date -u +%FT%TZ)"
set -x
cd rsift
rustc --version && cargo --version
mkdir -p dist-ci
HASH() { sha256sum "$@" 2>/dev/null || shasum -a 256 "$@"; }

case "$RUNNER_OS" in
  Windows)
    cargo build -p rsift-setup --release --locked
    cargo build -p rsift-api -p rsgraphics -p rsreplay --release --locked
    mkdir -p dist-ci/windows
    cp target/release/rsift-setup.exe dist-ci/windows/rsift-setup.exe
    cp target/release/rsift_api.dll    dist-ci/windows/rsift.dll
    cp target/release/rsgraphics.dll   dist-ci/windows/rsgraphics.dll
    cp target/release/rsreplay.dll     dist-ci/windows/rsreplay.dll
    cp docs/user/SETUP_BOOTSTRAPPER_JA.md dist-ci/windows/README_JA.md
    (cd dist-ci/windows && tar -a -c -f ../rsift-bundle-windows-x64.zip .)
    ;;
  macOS)
    rustup target add aarch64-apple-darwin x86_64-apple-darwin || true
    mkdir -p dist-ci/macos
    for T in aarch64-apple-darwin x86_64-apple-darwin; do
      cargo build -p rsift-setup --release --locked --target "$T"
      cargo build -p rsift-api -p rsgraphics -p rsreplay --release --locked --target "$T"
      APP="dist-ci/stage-$T/Rsift Setup.app/Contents"
      mkdir -p "$APP/MacOS"
      cp "target/$T/release/rsift-setup" "$APP/MacOS/rsift-setup"
      chmod +x "$APP/MacOS/rsift-setup"
      cat > "$APP/Info.plist" <<'PLIST'
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
PLIST
      cp "target/$T/release/librsift_api.dylib"  "dist-ci/stage-$T/librsift.dylib"
      cp "target/$T/release/librsgraphics.dylib" "dist-ci/stage-$T/librsgraphics.dylib"
      cp "target/$T/release/librsreplay.dylib"   "dist-ci/stage-$T/librsreplay.dylib"
      cp docs/user/SETUP_BOOTSTRAPPER_JA.md "dist-ci/stage-$T/README_JA.md"
      SFX=$(echo "$T" | sed 's/aarch64/arm64/;s/-apple-darwin//')
      (cd "dist-ci/stage-$T" && zip -qr "../rsift-bundle-macos-$SFX.zip" .)
    done
    ;;
  Linux)
    cargo build -p rsift-setup --release --locked
    cargo build -p rsift-api -p rsgraphics -p rsreplay --release --locked
    mkdir -p dist-ci/linux
    cp target/release/rsift-setup dist-ci/linux/rsift-setup
    cp target/release/librsift_api.so  dist-ci/linux/librsift.so
    cp target/release/librsgraphics.so dist-ci/linux/librsgraphics.so
    cp target/release/librsreplay.so   dist-ci/linux/librsreplay.so
    cp docs/user/SETUP_BOOTSTRAPPER_JA.md dist-ci/linux/README_JA.md
    (cd dist-ci/linux && zip -qr ../rsift-bundle-linux-x64.zip .)
    ;;
esac

# セルフテスト + 動作ログを成果物として残す (zip 同梱 dll と一緒に実行 = 本番形)
case "$RUNNER_OS" in
  Windows) D=dist-ci/windows ;;
  macOS)   D=dist-ci/stage-aarch64-apple-darwin ;;
  *)       D=dist-ci/linux ;;
esac
mkdir -p dist-ci/$RUNNER_OS-selftest
cp -r "$D"/. dist-ci/$RUNNER_OS-selftest/ \
  && (cd dist-ci/$RUNNER_OS-selftest && \
      if [ -f "Rsift Setup.app/Contents/MacOS/rsift-setup" ]; then \
        cd "Rsift Setup.app/Contents/MacOS" && ./rsift-setup --self-test ; \
      elif [ -f rsift-setup.exe ]; then ./rsift-setup.exe --self-test ; \
      else ./rsift-setup --self-test ; fi ; \
      echo "selftest_exit=$?" > result.txt)

ls -la dist-ci/ && HASH dist-ci/*.zip || true

# Release へ添付 (権限 contents: write が殻 yml で付いている場合のみ)
if [ -n "${GH_TOKEN:-}" ] && command -v gh >/dev/null; then
  gh release create setup-v1 \
    --title "Rsift Setup v1 (.exe / .app + engine & 2 Mod dll 同梱)" \
    --notes "zip を展開して rsift-setup(.exe) を実行。rsift_setup_log.txt/jsonl ができます。使い方は zip 内 README_JA.md。" \
    --repo "$GITHUB_REPOSITORY" || true
  for Z in dist-ci/*.zip; do
    gh release upload setup-v1 "$Z" --clobber --repo "$GITHUB_REPOSITORY" || true
  done
fi
echo "[trigger2] done"
```
