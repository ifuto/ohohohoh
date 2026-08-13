# TRIGGER2 (setup-artifacts 実行ペイロード)

このファイルが更新されると `setup-artifacts-trigger` ワークフローが起き、
下の ```bash ブロックだけが ubuntu/windows/macos の3台で実行される。
run 番号を1つ増やして push するのが「実行の合図」(起動条件はファイル差分)。

- run: 61  (ensure()もfind_class統一 → RegisterNatives+syncFromMinecraft完全一致。前回分=run 60)
- run: 60  (ChunkBridge sync: loadClass→find_class でClassLoader統一。前回分=run 59)
- run: 59  (ChunkBridge sync エラー可視化 + yaw/pitch field access修正。前回分=run 58)
- run: 58  (ChunkBridge sync markNativesReady再実行でnativesReady確保。前回分=run 57)
- run: 57  (screen getter → field access追加 + ChunkBridge sync診断。前回分=run 56)
- run: 56  (根本治療: patch_if_needed の this_class検証削除 — CFLHがmojmap名を渡すがbytecodeのthis_classは難読名で常時不一致→全net.minecraft系が却下されていた。is_target_classがgateなので安全。前回分=run 55)
- run: 55  (M1: ChunkBridge obf解決全面リファクタ + resolveField bridge。前回分=run 54)
- run: 54  (ログ #13 解析: 白画面フリーズは解消(MC稼働・render_flip 2400回安定・titleセット)。残課題の精確な特定のため診断強化: (1) #13でDX12 glfwHookが「rel32 out of range」でdetour失敗→DX12未稼働(GL描画に退化) (2) Modsボタン未注入=client_tickのscreen getterがNone返却(どのメソッド名か不明) (3) メソッド難読化5/11のみ解決(init/tick等のambiguity)。対策: obf_method_aliases を各メソッド単位で解決結果(obf名 or FAIL理由)をログ出力、client_tick のscreen getter失敗時に試行メソッド名を1回ログ。→ #14で screen_m/getscreen_m の実値と ambiguity 状況が確定し decl-based 解決等の正確な修正が可能に。前回分=run 53完全同梱)
- run: 53  (ログ #12 白画面の正確な根治＋DX12 FPSルート維持。run-52のGLパススルー化は「FPS上がらない=NG」で却下されたため撤回。#12白画面の真因はDX12デバイス生成成功直後にglobal_proxy(dx12_active)を有効化してGL描画/スワップを抑制したが、スワップチェーン未生成・present未成功だったためバニラ描画も消えて白になったこと。修正: GL抑制(dx12_active)をDX12 device生成時ではなく present_frame が実際に成功した時(presented=true)のみに移動。→ DX12が実際フレームを出した時だけGL抑制(FPS向上)、失敗/未生成時はGL描画継続(白画面なし)。DX12成功時1回ログ追加。前回分=run 52完全同梱)
- run: 52  (ログ #12 白画面フリーズ根治。#12 はクラッシュ無し(catch_unwind効いた)・render_flip発火・title set まで到達したが**画面が白くなってフリーズ**。真因: DX12が「アクティブ」になり glfwSwapBuffers をフックでスキップして DXGI present に差し替えたが、(a) DX12コンテンツパイプラインが未完成で描画内容が無く白画面 (b) MCのOpenGL と DX12 が同一HWNDで競合してフリーズ。DX12成功パスは [RsiftRender] ログを出さないため #12 にログが無かった。修正: **デフォルト backend を GLパススルーに変更** (ensure_engine の force 既定を Auto→GlPassthrough)。これでバニラMC描画が正常動作(白画面/フリーズ解消)し Rsift フック/mods/title は稼働。DX12差替えは `rsift.render.backend=dx12` env 明示時のみ(実験中・同一HWND競合とコンテンツ完成度の課題あり)。前回分=run 51完全同梱)
- run: 51  (総合デバッグwave — クラッシュ保護 + 配線(メソッド名難読化)の2本柱。#9でnet.minecraft系(Minecraft/Screen/Connection/Mob/Entity等)が全て「CFLH matched but NOT modified」だった真因は、パッチャがmojmapメソッド名("tick"/"init"/"channelRead0"/"aiStep"等)で難読bytecodeを検索してmissしていたこと。修正: (1) patcherにメソッドエイリアスレジストリ追加(register_runtime_method/runtime_method_name) — agentがobf_map install直後に各ターゲットのmojmap→難読メソッド名を解決して登録、patcherは難読名で注入(未登録はmojmap名へ安全落下=非難読RenderSystem.flipFrame等)。descriptorも全てワイルドカード""化(難読クラス参照を含むため厳密一致せず)。 → tick/init/network/mobAI/redstone/chunk等のCFLH HEAD注入が有効化。(2) render_bridge nativeOnFlip(on_render_frame→opt-gfx)にcatch_unwind — render_flipのdispatch_render保護(run-50)だけではnativeOnFlipが裸でopt-gfx panic→JVM abortする穴があった。検証: parser19/jvm27テスト緑・cargo check緑。前回分=run 50完全同梱)
- run: 50  (ログ #10 クラッシュ根治。#10 は ClassFormatError 解決・bridges ロード・[CFLH] PATCHED RenderSystem・**[hook] render_flip fired (count=1)** まで到達(=レンダーチェーン初繋がり)したが、render_flip→dispatch_render→opt-gfx の cluster_should_draw が防御的 assert! で「非有限/非正入力(NaN の .max マスク経路)」を弾いて **panic → JNI 境界 unwind → JVM abort (exit -1073740791)**。起動直後はカメラ/射影が未初期化で NaN が入るのが常態。修正2点: (1) nanite_clusters::cluster_should_draw の assert! を graceful 化 — 非有限入力は false (描画しない) を返す (production JNI で panic=ゲームクラッシュのため。NaN の .max 静寂常時誤描画も早期 return で回避。テスト should_draw_rejects_non_finite → should_draw_returns_false_on_non_finite へ更新) (2) render_flip の dispatch_render を catch_unwind — opt-gfx の他 assert 含め任意のレンダーパス panic が JNI 境界へ unwind してゲームクラッシュするのを最終保護 (throttled warn ログ)。検証: opt-gfx 1492/1492 テスト通過・rsift-jvm cargo check 緑。前回分=run 49 完全同梱)
- run: 49  (ログ #9 解析で flipFrame パッチの ClassFormatError クラッシュ根治 + spam抑制。#9 は class version/spam 解決・bridges ロード成功・[CFLH] PATCHED RenderSystem 確認まで到達したが、MC が `ClassFormatError: Illegal local variable table length 43 in method flipFrame(fyk,fwf)` で exit 2 クラッシュ → render_flip 未発火だった。真因: ClassRewriter の HEAD 注入は LineNumberTable の start_pc を +3 するが **LocalVariableTable/LocalVariableTypeTable を未更新** (全バイトコード +3 シフトで LVT start_pc が命令境界から外れ verifier が弾く)。修正: LVT/LVTT 各エントリ(10byte)の start_pc を +3 (LineNumberTable と同義・length は不変)。副次: flipFrame 実シグネチャは (fyk,fwf)=2 obfオブジェクト引数 (yarn (J)V は旧版) — ワイルドカード descriptor で既にパッチ適用済みなので不変。title_marker の「MC instance null」毎tick spam を抑制 (失敗ログは1回目+200回毎)。前回分=run 48 完全同梱)
- run: 48  (run-47 DIAG で javac 真因2点確定→根治: ①既定javac=JDK17にroll（run-44のJDK25/class69と違いclass61）②bootstrap ソースの ScreenInitPatcher(ASM)/RsiftPacketTap(Netty) がCI classpath不足でコンパイルエラー。対策: (a) この2ファイルをCI javacから除外 — 両者とも実行時反射ロードで未収録時フォールスローセーフ。残り全ソースは反射ベースで単独コンパイル可。(b) javac を2段トライ化: try1=--release 21(JDK21+/25→class65) / try2=フラグ無し(既定JDK17等→class61・MC Java21で下位互換ロード)。(c) 版数チェックを ≤65 受入に緩和(Java25=69のみFATAL)。これで全OSで最新ソース(obf解決ブリッジ含む)が class≤65 でコンパイルされ UnsupportedClassVersionError 根治。前回分=run 47 完全同梱)
- run: 47  (javac --release非認識を JDK9+ finder で試みたが、run-47 DIAG で①既定javac=JDK17 roll ②ScreenInitPatcher/RsiftPacketTap外部depエラー 判明→run-48で根治。前回分=run 46 完全同梱)
- run: 46  (run-45 即死失敗の自己診断化+堅牢化: javac出力捕捉+rc tee / javac成功時のみprebuilt上書き(失敗時フォールバック) / jar・version-check の tee+if化 / cargo build前マーカー tee。**DIAG確定: 既定javac=--release非認識(rc=2)→prebuilt(STALE)フォールバックで緑だが #8 根治には不十分 (run-47でsetup-java根治)**。前回分=run 45 完全同梱)
- run: 45  (ログ #8 根治 2点 + 観測強化: (1) javac --release 21 必須化 — runner 既定 JDK25 が class 69 を吐き MC(Java21) が RsiftScreenHooks 以下全 bootstrap クラスを UnsupportedClassVersionError で拒否 → 11,575 spam + Ui/Mod/PlatformBridge 全ロード失敗していたのを class 65 強制で根治 (生成 class の major=65 を CI で機械検証) (2) CFLH パッチャの flipFrame descriptor を "()V"→""(ワイルドカード) へ — 実シグネチャは (J)V なのに ()V 厳密一致でマッチせず HEAD 注入0 → render_flip が1度も発火せず DX12 チェーン全死していたのを根治 (yarn/mojmap 一次情報で (J)V 確定・flipFrame は RenderSystem 内で一意) (3) CFLH コールバック + nativeOnHook に throttled agent_log 観測追加。前回分=run 44 完全同梱。**注: run-45 はCI即死失敗→run-46で堅牢化**)
- run: 44  (javacステップのcd rsift二重バグ修正 — スクリプト冒頭でcd rsift済みなのにさらにcd rsiftして存在しないrsift/rsiftへ移動→set -e即死していた。前回分=run 43完全同梱)
- run: 43  (Public化後初ビルド — run-42と同内容: 全クラスダンプ+Modsボタン修正+Java obf解決+javac+タイトル定期+spamスロットル。前回分=run 42完全同梱)
- run: 42  (全クラスダンプ+Modsボタン修正+Java obf解決+javac再コンパイル+タイトル定期+spamスロットル。前回分=run 41完全同梱)
- run: 41  (Java bridge obf解決ブリッジ + bootstrap javac再コンパイル + タイトル定期再適用 + spamスロットル。ログ#7のCNFE/NoSuchMethodを根治。前回分 = run 40 完全同梱)
- run: 40  (renderer Wave 1-5 完結: CFLH難読化+present接続+エンジン駆動+live-render有効化。レンダラー差替フルチェーン デフォルト稼働。前回分 = run 39 完全同梱)
- run: 39  (renderer Wave 1+2: CFLHパッチャ難読化対応 + flipFrame→DX12 present接続(opt-in rsift.render.present=1)。前回分 = run 38 完全同梱)
- run: 38  (renderer Wave 1: CFLHパッチャ難読化対応 — flipFrame/tick/screen/network パッチが難読化runtimeで適用される基盤。前回分 = run 37 完全同梱)
- run: 37  (同意ダイアログを最前面確実表示に修正 — run-36 の MessageBoxW がMC裏に隠れるのを MB_TOPMOST|SETFOREGROUND|TASKMODAL で根治。前回分 = run 36 完全同梱)
- run: 36  (初回起動同意GUI実装 — 危険権限を使うMod(公式/第三者問わず)は起動直後にネイティブ Yes/No ダイアログ「○○は...を触ろうとしています。許可しますか？」→OKで承認を永続化し次回以降は素通り。Windows MessageBoxW。前回分 = run 35 完全同梱)
- run: 35  (#6 で #5完治確認→残件の公式Mod未ロードを根治: rsgraphics/rsreplay/rszoom を mod_security 自動承認化 + macOS /private シンボリックリンクの agent_opts テスト修正。本 run で RsGraphics 本体までロードされる Windows bundle を出す。前回分 = run 34 完全同梱)
- run: 34  (Windows/macOS の cargo test 失敗を根治 — mod_security::gq_loader_integration が .so 固定で Windows/macOS ローダに拾われず落ちていたのを platform_extension 使用化。cdylib build は元から成功・テスト修正のみ。追加で opt-level=3→1 でビルド時間も更に短縮。前回分 = run 33 完全同梱)
- run: 33  (CI バンドル高速化 — profile.release の lto=fat/CU=1 を CI のみ env var (CARGO_PROFILE_RELEASE_LTO=false / CODEGEN_UNITS=16) で上書き。run-32 は Windows 17m30s で fat LTO の link.exe 失敗だったのを LTO リンク工程ごと回避し全 OS ビルド時間を半減〜1/3 + Windows link 失敗も根治。Cargo.toml 本番 profile 不変。前回分 = run 32 完全同梱の上に積層)
- run: 32  (wave HR #5 根治 Phase 1+2 完結版ビルド — run 31 は client.txt ハッシュ照合バグ (Mojang 公開値 031a68be… は SHA-1 なのに SHA-256 で照合し必ず不一致→exit=1) を SHA-1 (sha1sum) 照合へ根治。client.txt DL 自体は run 31 で成功済 (11.8MB)。本 run = Phase 1 (クラス名解決基盤) + Phase 2 (メソッド名/descriptor 難読解決・全JNI call site) + client.txt 同梱の完全版を 3 OS ビルド。前回分 = run 31 完全同梱の上に積層)
- run: 31  (wave HQ+ #5 根治 Phase 1: 難読化クラス解決の基盤配線 + client.txt バンドル化 — (a) obf_map を実行時ロード: deferred_init で CFLH install 前に install_from_dir (client.txt 在→Obfuscated/不在→Unobfuscated 安全落下) (b) load_class_with_loader が mojmap→難読を解決 (c) find_minecraft_class + jvmti_events match_wanted_at_runtime が難読名で jcache 照合 (格納/取得一貫) (d) setup に deploy_client_mappings (client.txt を agent 探索先へ配備・不在は warn) (e) payload が 1.21.11 client.txt (sha256 031a68be… 検証) を取得し各 OS バンドルへ同梱。前回分 = run 30 完全同梱の上に積層。**注意**: 本 run は #5 のクラス名解決基盤のみ。メソッド名/型記述子の難読化解決 (minecraft_instance の getInstance/getWindow/setTitle 等) は Phase 2 = 次 run。実機 CNFE 解消の最終確認は次回 rsift-bootstrap.log 待ち)
- run: 30  (wave 219 HP-1 修正出荷 — run 29 は rsift-api コンパイル失敗 (log_bridge.rs が crate に無い `log` facade を参照 → 実際の facade は `tracing`・機械確定は DIAG-Linux exit=101 E0433)。fallback を tracing::error! に修正 + コメント整合。他は run 29 と完全同一 = api/jvm unit ゲート含め HP-1 としての初の 3 OS フル検証)
- run: 29  (wave 219 HP-1: 難読化実行時の名変換層コア対応 — (a) obf_map 新設 (Mojang 公式 client_mappings ProGuard 形式パーサ + mojmap→難読 resolver + RuntimeNaming 両対応。未配線のデッドコード = 本 run では挙動ゼロ変化) (b) P1 根治 = NativeLoader 個別失敗を rsift-api log_bridge 経由で bootstrap ログへ橋渡し (実機 #5 は logger 未初期化で 「mods loaded OK []」の真因 0 行蒸発が発生) (c) payload に cargo test -p rsift-api -p rsift-jvm を 3 OS へ追加 (api/jvm の unit ゲート常設化)。前回分 = run 28 完全同梱の上に積層)
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

# wave HQ+ (#5 根治): 1.21.11 難読化 mappings (agent が vanilla クラス解決に必須)。
# 一次配布元 piston-data (CI runner は到達可)。**SHA-1** 検証で supply-chain 保証。
# (jank.systems mappings guide で一次確認: 1.21.11 client_mappings の sha1 = 031a68be…
#  ※これは Mojang が client_mappings メタデータで公開した SHA-1 (40桁)。SHA-256 ではない)
CLIENT_URL="https://piston-data.mojang.com/v1/objects/031a68bebf55d824f66d6573d8c752f0e1bf232a/client.txt"
CLIENT_SHA1="031a68bebf55d824f66d6573d8c752f0e1bf232a"
RB curl -fL "$CLIENT_URL" -o dist-ci/client.txt
ACT=$(sha1sum dist-ci/client.txt 2>/dev/null | cut -d' ' -f1 || shasum -a 1 dist-ci/client.txt | cut -d' ' -f1)
if [ "$ACT" != "$CLIENT_SHA1" ]; then
  echo "FATAL: client.txt SHA-1 mismatch ($ACT != $CLIENT_SHA1)" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
  exit 1
fi
echo "[trigger2] client.txt OK sha1=$ACT ($(wc -c < dist-ci/client.txt) bytes)"

# wave HR: CI バンドルは #5 実機検証の反復用途 → profile.release の lto=fat / CU=1 を
# env var で上書きしビルド時間を大幅短縮 (Cargo.toml 本番 profile は不変・最適化ビルドは別途手動)。
# 効果: fat LTO のリンク工程 (Windows MSVC で大型 workspace の link.exe OOM/timeout = run-32 Windows
# 17m30s 失敗の最有力因) を丸ごと回避 + 全 OS ビルド時間を概ね半減〜1/3。
export CARGO_PROFILE_RELEASE_LTO=false
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
export CARGO_PROFILE_RELEASE_OPT_LEVEL=1

# wave HR: bootstrap jar を Java ソースから再コンパイル (Java bridge の obf 解決対応を反映)
echo "[trigger2] compiling bootstrap jar from sources..." | tee -a dist-ci/DIAG-$RUNNER_OS.txt
# NOTE: already in rsift/ (cd'd at script top) — no extra cd needed
# wave HS (ログ #8 根治): javac に --release 21 を必須化。MC 1.21.11 は Java 21
# (class file 65) で動くが、runner 既定の javac が Java 25 (class 69) を吐き、
# ログ #8 で RsiftScreenHooks ほか全 bootstrap クラスが UnsupportedClassVersionError
# (class version 69.0, this runtime recognizes up to 65.0) を出して 11,575 件の spam +
# UiBridge/ModBridge/PlatformBridge 全ロード失敗 = Mods ボタン/プラットフォーム橋渡し全死。
# --release 21 で class 65 を強制 (JDK 25 javac の CT.sym が 21 を内包するため確実)。
mkdir -p bootstrap/prebuilt/classes
# wave HS (run-47 確定): ScreenInitPatcher(ASM直接参照) と RsiftPacketTap(Netty直接参照) は
# CI classpath にライブラリが無いためコンパイルエラーになる。両者とも実行時に反射ロードで、
# 未収録時は安全にフォールスルーする (ScreenInitPatcher→RsiftClassTransformer catch→ネイティブ
# CFLHパッチャ、RsiftPacketTap→mod_bridge が無効化ログ)。よって CI コンパイルから除外。
# 残り全ソース(RsiftHooks/各Bridge)は反射ベースで単独コンパイル可能。
find bootstrap/java -name "*.java" ! -name "ScreenInitPatcher.java" ! -name "RsiftPacketTap.java" > /tmp/rsift_srcs.txt
echo "[trigger2] javac sources: $(wc -l < /tmp/rsift_srcs.txt) files (ScreenInitPatcher/RsiftPacketTap excluded — need ASM/Netty, handled as absent at runtime)" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
# wave HS (run-46 で確定): runner 既定 javac が --release を認識しない古い場合が
# ある (rc=2 "Usage")。--release 21 を受理する javac (JDK9+) を PATH / JAVA_HOME /
# 標準JDKインストール先から発見して使う。GitHub runner は temurin-21 等を標準搭載。
echo "[trigger2] default javac=$(command -v javac || echo NONE) java=$(command -v java || echo NONE) JAVA_HOME=${JAVA_HOME:-<unset>}" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
javac -version 2>&1 | head -1 | sed 's/^/[trigger2] javac -version: /' | tee -a dist-ci/DIAG-$RUNNER_OS.txt || true
java  -version 2>&1 | head -1 | sed 's/^/[trigger2] java  -version: /' | tee -a dist-ci/DIAG-$RUNNER_OS.txt || true
# wave HS: runner 既定 javac が JDK17 に roll し --release 21 を拒否 (rc=2) する場合がある。
# 2 段トライで class ≤ 65 (MC Java21 でロード可能) を確実に出す:
#   try1: javac --release 21  (JDK21+/25 なら成功 → class65)
#   try2: javac (フラグ無し)   (try1 失敗=既定JDK17等 → class61, MC Java21 で下位互換ロード)
JAVAC_RC=1
javac --release 21 -d bootstrap/prebuilt/classes @/tmp/rsift_srcs.txt > /tmp/rsift_javac.log 2>&1 || JAVAC_RC=$?
echo "[trigger2] javac --release 21 rc=$JAVAC_RC" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
if [ "$JAVAC_RC" != "0" ]; then
  echo "[trigger2] --release 21 failed (既定javac=JDK17等の可能性) — フラグ無しで再トライ (class<=65 なら MC Java21 でロード可能)" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
  tail -4 /tmp/rsift_javac.log | sed 's/^/    /' | tee -a dist-ci/DIAG-$RUNNER_OS.txt
  rm -rf bootstrap/prebuilt/classes/com
  JAVAC_RC=0
  javac -d bootstrap/prebuilt/classes @/tmp/rsift_srcs.txt > /tmp/rsift_javac.log 2>&1 || JAVAC_RC=$?
  echo "[trigger2] javac (no --release) rc=$JAVAC_RC" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
fi
if [ "$JAVAC_RC" != "0" ]; then
  echo "FATAL: javac failed both attempts — falling back to prebuilt jar" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
  tail -40 /tmp/rsift_javac.log | tee -a dist-ci/DIAG-$RUNNER_OS.txt
fi
# javac が完全成功 (rc=0) かつクラス生成済みの時だけ prebuilt jar を上書き。
# 部分コンパイル (rc!=0 だが一部クラス生成) で壊れた jar を出荷しないための保護。
if [ -d bootstrap/prebuilt/classes/com ] && [ "$JAVAC_RC" = "0" ]; then
  echo "Manifest-Version: 1.0" > /tmp/rsift_manifest.txt
  echo "Created-By: Rsift CI" >> /tmp/rsift_manifest.txt
  if ! (cd bootstrap/prebuilt/classes && jar cfm ../rsift-bootstrap.jar /tmp/rsift_manifest.txt com/); then
    echo "FATAL: jar packaging failed — falling back to prebuilt jar" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
  else
    echo "[trigger2] bootstrap jar compiled OK ($(wc -c < bootstrap/prebuilt/rsift-bootstrap.jar) bytes)" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
    # wave HS (#8 根治検証): 生成 class の major version を検査。major ≤ 65 なら
    # MC Java21 でロード可能 (Java21=65 / Java17=61 等・下位互換)。> 65 (Java25=69) は
    # UnsupportedClassVersionError で即 FATAL。od が無い環境は静黙スキップ。
    VMAJ_HEX=$(od -An -j6 -N2 -tx1 bootstrap/prebuilt/classes/com/rsift/RsiftHooks.class 2>/dev/null | tr -d ' \t\n' || true)
    VMAJ_DEC=999
    [ -n "$VMAJ_HEX" ] && VMAJ_DEC=$(printf '%d' "0x$VMAJ_HEX" 2>/dev/null || echo 999)
    echo "[trigger2] bootstrap class major-version (hex=${VMAJ_HEX:-?} dec=$VMAJ_DEC) — accept <=65 (loads on MC Java21)" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
    if [ "$VMAJ_DEC" -le 65 ] 2>/dev/null; then
      echo "[trigger2] bootstrap classes OK (major=$VMAJ_DEC <= 65)" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
    else
      echo "FATAL: bootstrap class major=$VMAJ_DEC > 65 — MC Java21 rejects with UnsupportedClassVersionError" | tee -a dist-ci/DIAG-$RUNNER_OS.txt; exit 1
    fi
  fi
else
  echo "[trigger2] WARNING: javac produced no classes — using prebuilt jar" | tee -a dist-ci/DIAG-$RUNNER_OS.txt
fi
echo "[trigger2] entering cargo build for RUNNER_OS=$RUNNER_OS" | tee -a dist-ci/DIAG-$RUNNER_OS.txt

case "$RUNNER_OS" in
  Windows)
    RB cargo build -p rsift-setup --release --locked
    RB cargo build -p rsift-api -p rsift-jvm -p rsgraphics -p rsreplay -p rszoom --release --locked
    RB cargo test -p rsift-api -p rsift-jvm --release --locked
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
    cp dist-ci/client.txt dist-ci/windows/client.txt
    cp docs/user/SETUP_BOOTSTRAPPER_JA.md dist-ci/windows/README_JA.md
    (cd dist-ci/windows && tar -a -c -f ../rsift-bundle-windows-x64.zip .)
    ;;
  macOS)
    rustup target add aarch64-apple-darwin x86_64-apple-darwin || true
    mkdir -p dist-ci/macos
    # wave 219 HP: jvm/api unit はホスト arch で 1 回 (cross-target 実行は runner 依存のため避ける)
    RB cargo test -p rsift-api -p rsift-jvm --release --locked
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
      cp dist-ci/client.txt "$APP/MacOS/client.txt"
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
    RB cargo test -p rsift-api -p rsift-jvm --release --locked
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
    cp dist-ci/client.txt dist-ci/linux/client.txt
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
