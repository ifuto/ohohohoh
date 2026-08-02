# Rsift セットアップ (.exe / .app) の使い方

ダウンロードした zip を解凍し、**実行ファイルと dll/dylib を全部同じフォルダに置いて、実行するだけ**です。

| OS | 実行ファイル | 置くもの (例) |
|---|---|---|
| Windows | `rsift-setup.exe` | `rsift.dll` / `rsift_jvm.dll` (起動 agent) / `rsgraphics.dll` / `rsreplay.dll` / `rszoom.dll` / **`rsift-bootstrap.jar`** (Java ブリッジ) |
| Mac | `Rsift Setup.app` (内側に `rsift-setup` と dylib 群) | `.app/Contents/MacOS/` 内に `librsift.dylib` / `librsift_jvm.dylib` / `librsgraphics.dylib` / `librsreplay.dylib` / `librszoom.dylib` / **`rsift-bootstrap.jar`** |

Release `setup-v1` の zip はこれらが全部入った一体梱包です。展開したフォルダでそのまま実行してください。

> **`rsift-bootstrap.jar` は Mod 読み込みの命です** (Mods ボタン・画面フックの Java 側実体)。
> これが無い・破損しているとゲームは起動しても Mod が一切効きません (Mods ボタンが出ません)。
> その状態を setup が検出した場合は、壊れた起動構成を作らないよう登録を中止してログに理由を書きます。
> 2026-08-01 以前の zip には入っていなかったため、必ず最新の zip でやり直してください。

実行すると自動で:

1. 同じ階層の dll/dylib を検出して SHA-256 を計算
2. 検証 (初期配布はハッシュ記録のみ。次回リリースから改竄検出が有効化)
3. あなたの PC に合った RsGraphics 経路を決定
   - Mac (Apple Silicon + macOS 26+) → **Metal 4** (RsGraphics 内で自動選択)
   - Mac (上記以外) → classic Metal (同、自動選択)
   - Windows → RsGraphics 内の wgpu が最適 backend を自動選択 (Vulkan 優先/DX12)
4. `rsift_launch.json` (起動構成) を生成
5. **Minecraft ランチャーへ起動構成を自動登録** (Minecraft 導入済みの場合):
   - `<.minecraft>/versions/rsift-1.21.11/` フォルダ作成 (version JSON + natives)
   - `<.minecraft>/mods/` に RsGraphics / RsReplay / RsZoom を配置
   - `launcher_profiles.json` に起動構成「Rsift」を追加 (既存構成は壊さず、
     上書き前に `launcher_profiles.json.bak_rsift` バックアップを作成)
   - ランチャーを開くと「Rsift」が選べます (初回は数秒の読み込み後に表示)
6. **PrismLauncher へインスタンスを自動登録** (PrismLauncher 検出時のみ):
   - `<PrismLauncher>/instances/rsift/` を作成
     (`instance.cfg` / `mmc-pack.json` / `patches/rsift.json` = 起動引数
     `-agentpath` と `-Drsift.*` を追記型 `+jvmArgs` で登録)
   - `<instance>/.minecraft/mods/` に RsGraphics / RsReplay / RsZoom を配置
   - native 本体は `<instance>/rsift-natives/` に配置
   - **「rsift」名の外部製インスタンスが既にある場合は絶対に上書きせず中止します**
     (ログに理由を記録。手動で退避するか別アカウントで試してください)
   - PrismLauncher を再起動すると「Rsift」が一覧に現れます (Windows 10 想定、
     macOS/Linux も同じ構成で登録)
   - 既定パス: Windows=`%APPDATA%\PrismLauncher`、
     環境変数 `RSIFT_PRISM_DIR` で任意の場所を指定できます
7. 全工程をログに記録

## 同梱 Mod: RsZoom (ズーム)

- ゲーム内で **ズームキー (初期設定: C) を押している間**、視線の先へ滑らかにズームします
  (cubic イーズアウト: 始まりも戻りも速く立ち上がって、しっとり止まる)
- ズームキーは **Minecraft 本体の「設定 → コントロール (キー設定)」画面に
  「key.rsift.zoom」として表示され、好きなキーに変更できます** (変更はバニラと同じ
  options.txt に保存されます。チャット入力中はバニラのキー規約どおり反応しません)
- 倍率・速度は **タイトル画面 → Mods → RsZoom → Config** で変更できます
  画面はマイクラ本来のボタン/文字スタイルです (行をクリックすると値が変わり、
  `.minecraft/config/rszoom.cfg` に自動保存)
- キー入力は Minecraft の入力機構 (バニラ KeyMapping) から読むので **全 OS 共通**で、
  生キー状態の直接読取り (`input_capture`) の申告は不要になりました (権限は空)

## ログを送ってください (後で診断します)

失敗しても成功しても、実行フォルダにこれができます:

- `rsift_setup_log.txt` … 人間用 (そのまま読めます)
- `rsift_setup_log.jsonl` … 機械用

この 2 ファイルを送ってもらえれば、環境・dll のハッシュ・判定・失敗箇所が全部分かります。

### ゲームは起動するのに Mods ボタンが無い場合

ゲームを起動した後に作られる **エージェントの起動ログ** があります:

- `rsift-bootstrap.log` … `<.minecraft>/versions/rsift-1.21.11/` 内、
  または `<.minecraft>/` 直下 (Prism の場合は `<instance>/.minecraft/`)

これを併せて送ってもらえれば、agent がどこまで動いたか (jar 認識・フック注入・
Mod ロードの成否) が全部分かります。

### 「Vanilla 判定」かどうかの見分け方 (wave 205 で追加)

最新版 (2026-08-02 以降) では、Rsift が正しく動いているとき **2 箇所** に目印が出ます:

1. **ウィンドウのタイトルバー** … `Minecraft* 1.21.11 - Rsift (RsGraphics Render)`
   と表示されます (バニラの `Minecraft* 1.21.11` 表記に接尾する形です)。
   これが出ていれば「Vanilla 判定」ではなく Rsift が入っています。
2. **F3 デバッグ画面** … F3 を押したときの行の中に
   `RsGraphics Render (Rsift)` という 1 行が追加されます。
   (ただし、入る場所はゲーム内部の F3 行リストを解析して安全に足す設計なので、
   条件に合う行メソッドが見つからない環境ではログに理由を残して**追加しません**。
   タイトルだけでも判定材料になります)

### rsift-bootstrap.log の新しい行の意味 (wave 205)

- `[Rsift] mod_dir from game cwd: ...` … Mod フォルダを正しい場所
  (ゲームディレクトリ側の `mods`) に解決できた合図です。
  旧版は `rsift-natives\mods` を見に行って `WARN mod_dir does not exist`
  (= Mod が 1 個も読めていない状態) になる欠陥があり、これを根治しました。
- `[JVMTI] ClassFileLoadHook enabled (exact-spec indices)` …
  クラスフックの装着が成功した合図です (公式仕様どおりの番号で入れます)。
- `[JVMTI] game ClassLoader captured via CFLH (first class: ...)` …
  ゲームのローダー捕捉に成功した合図です。旧版はこれが永遠に見つからず
  フックが 0 個でした。
- `RetransformClasses rc=0 (...)` … 起動初期に読み込み済みだったクラスへの
  後追いパッチが成功した合図です (`rc=0` が正常)。
- `f3_marker` / `title_marker` の行 … 上記 2 つの目印の成否と理由です。

起動後にこのログをもう一度送ってもらえれば、根治が実機で効いたかを
1 行ずつ機械的に照合できます。

終了コード: `0`=準備完了 / `2`=dll が見つからない / `3`=検証不一致 / `4`=IO失敗 / `5`=自己診断失敗

オプション: `--self-test` (内蔵診断) / `--dry-run` (構成を書かず確認のみ) / `--dir <場所>`

## 注意 (正直な現状)

- `.app` は未署名です。初回は「右クリック → 開く」で Gatekeeper を通してください
- GL 経路 (旧 Mac 向け) のライブラリ本体は後続 wave で供給予定です
