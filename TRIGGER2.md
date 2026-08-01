# TRIGGER2 (setup-artifacts 実行ペイロード)

このファイルが更新されると `setup-artifacts-trigger` ワークフローが起き、
下の ```bash ブロックだけが ubuntu/windows/macos の3台で実行される。
run 番号を1つ増やして push するのが「実行の合図」(起動条件はファイル差分)。

- run: 6
- 目的: rsift-setup バイナリ + **エンジン dll + JVMTI agent (rsift_jvm) +
  2 Mod cdylib (RsGraphics=rsgraphics / RsReplay=rsreplay)** をビルドし、
  zip 展開したら全部同じフォルダに dll が並ぶ一体梱包形式で出力。
  (macOS では dylib は .app/Contents/MacOS/ 内部に同梱 — バイナリと同階層
  必須のため。外に置くと検出 0 で exit 2 になる構造欠陥が run 4 に顕在化した)
- 失敗時診断: 終了時に DIAG-<OS>.txt を Release setup-diag へ添付する。

```bash
echo "[trigger2] start os=$RUNNER_OS arch=$(uname -m) time=$(date -u +%FT%TZ)"
set -euo pipefail -x
ROOT=$(pwd)
cd rsift

# どこで死んでも診断を Release に残す (logs 経路遮断でも原因追跡できる)
diag_upload() {
  local st=$?
  set +e
  mkdir -p dist-ci
  {
    echo "== DIAG $RUNNER_OS exit=$st time=$(date -u +%FT%TZ) =="
    echo "-- 直近の cargo 出力は job log 参照。cargo 失敗時はここに error 行が来る --"
    echo "-- target/release cdylib 候補 --"
    ls -la target/release/*.dll target/release/*.dylib target/release/*.so 2>&1 | head -30
    ls -la target/aarch64-apple-darwin/release/*.dylib target/x86_64-apple-darwin/release/*.dylib 2>&1 | head -20
    echo "-- dist-ci --"
    ls -la dist-ci 2>&1 | head -20
  } > "dist-ci/DIAG-$RUNNER_OS.txt"
  if [ -n "${GH_TOKEN:-}" ] && command -v gh >/dev/null; then
    gh release create setup-diag --title "setup diag (自動診断)" --notes "trigger2 DIAG 集約先" --repo "$GITHUB_REPOSITORY" >/dev/null 2>&1
    gh release upload setup-diag "dist-ci/DIAG-$RUNNER_OS.txt" --clobber --repo "$GITHUB_REPOSITORY" >/dev/null 2>&1
  fi
  # 主要経路: git push (runner の GITHUB_TOKEN は contents: write 同意済)。
  # 3 台同時 push の race は pull --rebase のリトライで吸収する。
  cd "$ROOT" 2>/dev/null || true
  mkdir -p dist-ci-diag
  cp "rsift/dist-ci/DIAG-$RUNNER_OS.txt" "dist-ci-diag/" 2>/dev/null
  cp "rsift/dist-ci/DIAG-build-$RUNNER_OS.txt" "dist-ci-diag/" 2>/dev/null
  git config user.email "setup-bot@ifuto.local"
  git config user.name "setup-bot"
  git add dist-ci-diag/
  git commit -m "diag($RUNNER_OS): exit=$st" >/dev/null 2>&1
  for i in 1 2 3 4 5; do
    git pull --rebase origin "${GITHUB_REF_NAME}" >/dev/null 2>&1 && \
      git push origin "HEAD:${GITHUB_REF_NAME}" >/dev/null 2>&1 && break
    sleep 5
  done
  exit $st
}
trap diag_upload EXIT

rustc --version && cargo --version
mkdir -p dist-ci
HASH() { sha256sum "$@" 2>/dev/null || shasum -a 256 "$@"; }
RB() {
  echo "+ $*"
  "$@" > /tmp/rb.log 2>&1 || {
    local st=$?
    echo "FATAL: $* (exit=$st)" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
    tail -120 /tmp/rb.log >> dist-ci/DIAG-$RUNNER_OS.txt 2>/dev/null
    exit $st
  }
  tail -3 /tmp/rb.log
}

case "$RUNNER_OS" in
  Windows)
    RB cargo build -p rsift-setup --release --locked
    RB cargo build -p rsift-api -p rsift-jvm -p rsgraphics -p rsreplay --release --locked
    for F in rsift_api.dll rsift_jvm.dll rsgraphics.dll rsreplay.dll; do
      [ -f "target/release/$F" ] || { echo "FATAL: target/release/$F が無い"; exit 1; }
    done
    mkdir -p dist-ci/windows
    cp target/release/rsift-setup.exe dist-ci/windows/rsift-setup.exe
    cp target/release/rsift_api.dll    dist-ci/windows/rsift.dll
    cp target/release/rsift_jvm.dll    dist-ci/windows/rsift_jvm.dll
    cp target/release/rsgraphics.dll   dist-ci/windows/rsgraphics.dll
    cp target/release/rsreplay.dll     dist-ci/windows/rsreplay.dll
    cp docs/user/SETUP_BOOTSTRAPPER_JA.md dist-ci/windows/README_JA.md
    (cd dist-ci/windows && tar -a -c -f ../rsift-bundle-windows-x64.zip .)
    ;;
  macOS)
    rustup target add aarch64-apple-darwin x86_64-apple-darwin || true
    mkdir -p dist-ci/macos
    for T in aarch64-apple-darwin x86_64-apple-darwin; do
      RB cargo build -p rsift-setup --release --locked --target "$T"
      RB cargo build -p rsift-api -p rsift-jvm -p rsgraphics -p rsreplay --release --locked --target "$T"
      for F in librsift_api.dylib librsift_jvm.dylib librsgraphics.dylib librsreplay.dylib; do
        [ -f "target/$T/release/$F" ] || { echo "FATAL: target/$T/release/$F が無い"; exit 1; }
      done
      APP="dist-ci/stage-$T/Rsift Setup.app/Contents"
      mkdir -p "$APP/MacOS"
      cp "target/$T/release/rsift-setup" "$APP/MacOS/rsift-setup"
      chmod +x "$APP/MacOS/rsift-setup"
      # dylib はバイナリと同階層 (MacOS 内) に配置 — setup は exe の親ディレクトリを探す
      cp "target/$T/release/librsift_api.dylib"  "$APP/MacOS/librsift.dylib"
      cp "target/$T/release/librsift_jvm.dylib"  "$APP/MacOS/librsift_jvm.dylib"
      cp "target/$T/release/librsgraphics.dylib" "$APP/MacOS/librsgraphics.dylib"
      cp "target/$T/release/librsreplay.dylib"   "$APP/MacOS/librsreplay.dylib"
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
      (cd "dist-ci/stage-$T" && zip -qr "../rsift-bundle-macos-$SFX.zip" .)
    done
    ;;
  Linux)
    RB cargo build -p rsift-setup --release --locked
    RB cargo build -p rsift-api -p rsift-jvm -p rsgraphics -p rsreplay --release --locked
    for F in librsift_api.so librsift_jvm.so librsgraphics.so librsreplay.so; do
      [ -f "target/release/$F" ] || { echo "FATAL: target/release/$F が無い"; exit 1; }
    done
    mkdir -p dist-ci/linux
    cp target/release/rsift-setup dist-ci/linux/rsift-setup
    cp target/release/librsift_api.so  dist-ci/linux/librsift.so
    cp target/release/librsift_jvm.so  dist-ci/linux/librsift_jvm.so
    cp target/release/librsgraphics.so dist-ci/linux/librsgraphics.so
    cp target/release/librsreplay.so   dist-ci/linux/librsreplay.so
    cp docs/user/SETUP_BOOTSTRAPPER_JA.md dist-ci/linux/README_JA.md
    (cd dist-ci/linux && zip -qr ../rsift-bundle-linux-x64.zip .)
    ;;
esac

# セルフテスト: 同梱 dll と一緒に実行 = 本番形 (mac は app 内 MacOS 位置で)
case "$RUNNER_OS" in
  Windows)
    ST=dist-ci/selftest-win
    mkdir -p "$ST" && cp -r dist-ci/windows/. "$ST"/
    EXE=./rsift-setup.exe
    ;;
  macOS)
    ST="dist-ci/stage-aarch64-apple-darwin/Rsift Setup.app/Contents/MacOS"
    EXE=./rsift-setup
    ;;
  *)
    ST=dist-ci/selftest-linux
    mkdir -p "$ST" && cp -r dist-ci/linux/. "$ST"/
    EXE=./rsift-setup
    ;;
esac
(cd "$ST" && $EXE --self-test; echo "selftest_exit=$?" > result.txt; ls -la)

ls -la dist-ci/
for Z in dist-ci/*.zip; do
  echo "== 内容物検査: $Z =="
  if command -v unzip >/dev/null; then unzip -l "$Z"; else tar -tf "$Z" || true; fi
done
HASH dist-ci/*.zip || true

# Release へ添付 (権限 contents: write が殻 yml で付いている場合のみ)
if [ -n "${GH_TOKEN:-}" ] && command -v gh >/dev/null; then
  gh release create setup-v1 \
    --title "Rsift Setup v1 (.exe/.app + engine & agent & 2 Mod dll 同梱 + 起動構成自動登録版)" \
    --notes "zip を展開して rsift-setup(.exe) / Rsift Setup.app を実行。起動構成 (versions/rsift-1.21.11 + launcher profile) も自動登録。rsift_setup_log.txt/jsonl ができたら送ってください。" \
    --repo "$GITHUB_REPOSITORY" || true
  for Z in dist-ci/*.zip; do
    gh release upload setup-v1 "$Z" --clobber --repo "$GITHUB_REPOSITORY" || true
  done
fi
echo "[trigger2] done"
```
