# TRIGGER2 (setup-artifacts 実行ペイロード)

このファイルが更新されると `setup-artifacts-trigger` ワークフローが起き、
下の ```bash ブロックだけが ubuntu/windows/macos の3台で実行される。
run 番号を1つ増やして push するのが「実行の合図」(起動条件はファイル差分)。

- run: 2
- 目的: rsift-setup の Windows .exe / macOS .app / Linux バイナリをビルドし、
  `rsift/dist-ci/` に zip として出力 → ワークフローが Artifacts + Release
  `setup-v1` へ添付する。

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
    mkdir -p dist-ci/windows
    cp target/release/rsift-setup.exe dist-ci/windows/rsift-setup.exe
    cp docs/user/SETUP_BOOTSTRAPPER_JA.md dist-ci/windows/README_JA.md
    (cd dist-ci/windows && tar -a -c -f ../rsift-setup-windows-x64.zip .)
    ;;
  macOS)
    rustup target add aarch64-apple-darwin x86_64-apple-darwin || true
    mkdir -p dist-ci/macos
    for T in aarch64-apple-darwin x86_64-apple-darwin; do
      cargo build -p rsift-setup --release --locked --target "$T"
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
      cp docs/user/SETUP_BOOTSTRAPPER_JA.md "dist-ci/stage-$T/README_JA.md"
      SFX=$(echo "$T" | sed 's/aarch64/arm64/;s/-apple-darwin//')
      (cd "dist-ci/stage-$T" && zip -qr "../Rsift-Setup-macos-$SFX.zip" .)
    done
    ;;
  Linux)
    cargo build -p rsift-setup --release --locked
    mkdir -p dist-ci/linux
    cp target/release/rsift-setup dist-ci/linux/rsift-setup
    cp docs/user/SETUP_BOOTSTRAPPER_JA.md dist-ci/linux/README_JA.md
    (cd dist-ci/linux && zip -qr ../rsift-setup-linux-x64.zip .)
    ;;
esac

# セルフテスト + 動作ログを成果物として残す (ユーザーに送ってもらう形式)
case "$RUNNER_OS" in
  Windows) BIN=dist-ci/windows/rsift-setup.exe ;;
  macOS)   BIN="dist-ci/stage-aarch64-apple-darwin/Rsift Setup.app/Contents/MacOS/rsift-setup" ;;
  *)       BIN=dist-ci/linux/rsift-setup ;;
esac
mkdir -p dist-ci/$RUNNER_OS-selftest
cp "$BIN" dist-ci/$RUNNER_OS-selftest/rsift-setup \
  && (cd dist-ci/$RUNNER_OS-selftest && ./rsift-setup --self-test --dry-run; echo "selftest_exit=$?" > result.txt)

ls -la dist-ci/ && HASH dist-ci/*.zip || true

# Release へ添付 (権限 contents: write が殻 yml で付いている場合のみ)
if [ -n "${GH_TOKEN:-}" ] && command -v gh >/dev/null; then
  gh release create setup-v1 \
    --title "Rsift Setup v1 (.exe / .app / Linux)" \
    --notes "使い方は zip 内 README_JA.md。実行すると rsift_setup_log.txt/jsonl ができます。その2枚を送ってください。" \
    --repo "$GITHUB_REPOSITORY" || true
  for Z in dist-ci/*.zip; do
    gh release upload setup-v1 "$Z" --clobber --repo "$GITHUB_REPOSITORY" || true
  done
fi
echo "[trigger2] done"
```
