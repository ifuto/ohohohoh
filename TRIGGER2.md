# TRIGGER2 (setup-artifacts 実行ペイロード)

このファイルが更新されると `setup-artifacts-trigger` ワークフローが起き、
下の ```bash ブロックだけが ubuntu/windows/macos の3台で実行される。
run 番号を1つ増やして push するのが「実行の合図」(起動条件はファイル差分)。

- run: 28  (リリース候補スナップショット — commit f98d400 現状態同梱: wave 216 HM (RD24 2,401 チャンク判定版) + wave 217 HN (実体 40,016/編集 7,980 面積比例スケーリング + diff-slot 27→6 縮約 (OOM 根治) + edit_sim_diff 機械配分配線) + BENCH/AUDIT doc 整合修正。bench-ci 緑 (lib 1,492/1,492・36c 決定性 diff 一致) 確認済の上でのリリース化。前回分 = run 27 完全同梱の上に積層)
- run: 27  (wave 217 HN: RD24 機械スケーリング版を同梱 — 実体 40,016/編集 7,980 + diff-slot 27→6 縮約 (OOM 根治) + edit_sim_diff 機械配分。lib 本体 diff_mesh.rs は slot 内部表現のみ (公開 API・意味論不変)。36c 出力 bit 一致・lib 1492/1492 完走確認。前回分 = wave 216 HM (2,401 チャンク判定版) 完全同梱の上に積層)
- run: 26  (wave 216 HM: RD24 判定版を同梱 — 2,401 チャンク同一内容ワークロードで計測可能全項目 C ≥ B (メッシュ 12.80 s vs B 13.93 s 逆転・頂点 bytes −76.3%・ACMR 1.030・I-O 5.6x・ソート時間+精度全勝・可視性 graph bit 同一・編集差分 3.94 ms/3.8 KB = B 比 68x・合成 1.48x vs A)。HM-1 メッシュ (encode-on-miss+slot 世代スタンプ)・HM-2 ソート (u64 単一キー直列化)・HM-3 可視性 (graph 採用) の 3 根治を pseudo_mc ハーネスに適用。bench ハーネス側の変更のみ = rsgraphics/opt-gfx 本体動作は不変。前回分 = wave 215 HL (編集差分メッシュ 9.9x) 完全同梱の上に積層)
- run: 25  (wave 215 HL: 編集ワークロード差分メッシュ版を同梱 — per-block DiffSectionMesh で編集時再メッシュ 840 → 84.7 ms (9.9x、A 系と同速度・面集合 FULL rescan と bit 一致を parity 停止ガードで機械保証)。前回分 = wave 214 HK (実体カリング V2 統一 2.57→0.53 ms) 完全同梱の上に積層)
- run: 24  (wave 214 HK: 実体カリング V2 統一版を同梱 — FastEntityCuller V2 を bench C 行・本番 wiring の双方に本採用 (2.572→0.529 ms、4.9x、rays −80%) + ChunkBucketGate 全撤去 (0 レイ永久可視の虚偽可視バグ + 純損デッドコード) + boot banner "FastEntityCuller V2" が初めて実装と一致 + CL-4 実カメラ FOV 真値配線解消。dense125 真値計測器での密度膝点スイープで 5 レイ採定 (pops 4/3/2 vs 27 レイの 3.9x コスト)。opt-gfx 1486/1486・変異 RED×4+MD5-VERIFIED・合成 A 同値圏 1.00-1.05x。前回分 = wave 212 HI + 213 HJ 完全同梱の上に積層)
- run: 23  (wave 212 HI + 213 HJ: ベンチ実走で機械確定した 2 大律速の根治版を同梱 — Tipsify 同一意味論線形化 507→189 ms (2.7x, rsgraphics のメッシュ再構築に直結) + 半透明ソート評価実装の全損 topo 試行門番化 (3.6 s→25 ms, 143x・誤順も改善)。合成フレームはバニラ系と互角 (0.97x) に到達。前回分 = wave 211 HG (自動復旧複数源フォールバック) 完全同梱の上に積層)
- 目的: rsift-setup バイナリ + **エンジン dll + JVMTI agent (rsift_jvm =
  keybind_bridge 内蔵 = バニラ KeyMapping 登録/同期機) +
  **3 Mod cdylib (RsGraphics=rsgraphics / RsReplay=rsreplay /
  RsZoom=rszoom = バニラキーバインド式イーズアウトズーム Mod: 既定 C キー、
  ゲーム内「設定→コントロール」で再割当可・全 OS 共通)** +
  **rsift-bootstrap.jar (Java ブリッジ: RsiftHooks / ScreenInitPatcher 等 =
  Mods ボタンとフック注入の要。run 11 まで未同梱で実機「Mods ボタン無し」
  不具合の直接原因だった構造的欠陥 = wave 204 根治)** をビルドし、
  rsift_jvm は wave 205 版 (早期 CFLH install + 捕捉 loader 直接利用 +
  GetLoadedClasses 掃引 + RetransformClasses 追撃 + F3 マーカー
  「RsGraphics Render (Rsift)」+ ウィンドウタイトルマーカー内蔵、
  JVMTI index 公式 jvmti.xml 準拠へ全書換) + mod_dir 誤解決根治
  (rsift-natives ではなくゲーム cwd の mods を解決)、
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
    echo "-- target/release cdylib 候補 --"
    ls -la target/release/*.dll target/release/*.dylib target/release/*.so 2>&1 | head -30
    ls -la target/aarch64-apple-darwin/release/*.dylib target/x86_64-apple-darwin/release/*.dylib 2>&1 | head -20
    echo "-- dist-ci --"
    ls -la dist-ci 2>&1 | head -20
    echo "-- selftest result --"
    cat dist-ci/selftest-*/result.txt 2>&1 | head -5
    find dist-ci/selftest-win -type f 2>/dev/null | head -10
  } >> "dist-ci/DIAG-$RUNNER_OS.txt"
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
    RB cargo build -p rsift-api -p rsift-jvm -p rsgraphics -p rsreplay -p rszoom --release --locked
    for F in rsift_api.dll rsift_jvm.dll rsgraphics.dll rsreplay.dll rszoom.dll; do
      [ -f "target/release/$F" ] || { echo "FATAL: target/release/$F が無い"; exit 1; }
    done
    mkdir -p dist-ci/windows
    cp target/release/rsift-setup.exe dist-ci/windows/rsift-setup.exe
    cp target/release/rsift_api.dll    dist-ci/windows/rsift.dll
    cp target/release/rsift_jvm.dll    dist-ci/windows/rsift_jvm.dll
    cp target/release/rsgraphics.dll   dist-ci/windows/rsgraphics.dll
    cp target/release/rsreplay.dll     dist-ci/windows/rsreplay.dll
    cp target/release/rszoom.dll       dist-ci/windows/rszoom.dll
    cp bootstrap/prebuilt/rsift-bootstrap.jar dist-ci/windows/rsift-bootstrap.jar
    cp docs/user/SETUP_BOOTSTRAPPER_JA.md dist-ci/windows/README_JA.md
    (cd dist-ci/windows && tar -a -c -f ../rsift-bundle-windows-x64.zip .)
    ;;
  macOS)
    rustup target add aarch64-apple-darwin x86_64-apple-darwin || true
    mkdir -p dist-ci/macos
    for T in aarch64-apple-darwin x86_64-apple-darwin; do
      RB cargo build -p rsift-setup --release --locked --target "$T"
      RB cargo build -p rsift-api -p rsift-jvm -p rsgraphics -p rsreplay -p rszoom --release --locked --target "$T"
      for F in librsift_api.dylib librsift_jvm.dylib librsgraphics.dylib librsreplay.dylib librszoom.dylib; do
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
      cp "target/$T/release/librszoom.dylib"     "$APP/MacOS/librszoom.dylib"
      cp "bootstrap/prebuilt/rsift-bootstrap.jar" "$APP/MacOS/rsift-bootstrap.jar"
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
    RB cargo build -p rsift-api -p rsift-jvm -p rsgraphics -p rsreplay -p rszoom --release --locked
    for F in librsift_api.so librsift_jvm.so librsgraphics.so librsreplay.so librszoom.so; do
      [ -f "target/release/$F" ] || { echo "FATAL: target/release/$F が無い"; exit 1; }
    done
    mkdir -p dist-ci/linux
    cp target/release/rsift-setup dist-ci/linux/rsift-setup
    cp target/release/librsift_api.so  dist-ci/linux/librsift.so
    cp target/release/librsift_jvm.so  dist-ci/linux/librsift_jvm.so
    cp target/release/librsgraphics.so dist-ci/linux/librsgraphics.so
    cp target/release/librsreplay.so   dist-ci/linux/librsreplay.so
    cp target/release/librszoom.so     dist-ci/linux/librszoom.so
    cp bootstrap/prebuilt/rsift-bootstrap.jar dist-ci/linux/rsift-bootstrap.jar
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
  tar -tf "$Z" 2>&1 | head -40 || echo "(list 不可だが継続)"
done
HASH dist-ci/*.zip || true

# Release へ添付 (権限 contents: write が殻 yml で付いている場合のみ)
if [ -n "${GH_TOKEN:-}" ] && command -v gh >/dev/null; then
  gh release create setup-v1 \
    --title "Rsift Setup v1 (.exe/.app + engine & agent & 3 Mod dll 同梱 + 起動構成 & PrismLauncher インスタンス自動登録版)" \
    --notes "zip を展開して rsift-setup(.exe) / Rsift Setup.app を実行。起動構成 (versions/rsift-1.21.11 + launcher profile) と PrismLauncher インスタンス (instances/rsift) も自動登録 (検出時のみ・外部製 rsift 名インスタンスは絶対に上書きしない)。RsZoom: ズームキー (既定 C) 押下中ズーム — キーはゲーム内「設定→コントロール」で変更可・倍率はタイトル→Mods→RsZoom→Config。Mods ボタンが出ない場合は .minecraft/versions/rsift-1.21.11/ または .minecraft/ 直下の rsift-bootstrap.log も併せて送ってください。rsift_setup_log.txt/jsonl ができたら送ってください。" \
    --repo "$GITHUB_REPOSITORY" || true
  for Z in dist-ci/*.zip; do
    gh release upload setup-v1 "$Z" --clobber --repo "$GITHUB_REPOSITORY" || true
  done
fi
echo "[trigger2] done"
```
