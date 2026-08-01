# Rsift セットアップ (.exe / .app) の使い方

ダウンロードした zip を解凍し、**実行ファイルと dll/dylib を全部同じフォルダに置いて、実行するだけ**です。

| OS | 実行ファイル | 置くもの (例) |
|---|---|---|
| Windows | `rsift-setup.exe` | `rsift.dll` / `rsift_jvm.dll` (起動 agent) / `rsgraphics.dll` / `rsreplay.dll` / `rszoom.dll` |
| Mac | `Rsift Setup.app` (内側に `rsift-setup` と dylib 群) | `.app/Contents/MacOS/` 内に `librsift.dylib` / `librsift_jvm.dylib` / `librsgraphics.dylib` / `librsreplay.dylib` / `librszoom.dylib` |

Release `setup-v1` の zip はこれらが全部入った一体梱包です。展開したフォルダでそのまま実行してください。

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

終了コード: `0`=準備完了 / `2`=dll が見つからない / `3`=検証不一致 / `4`=IO失敗 / `5`=自己診断失敗

オプション: `--self-test` (内蔵診断) / `--dry-run` (構成を書かず確認のみ) / `--dir <場所>`

## 注意 (正直な現状)

- `.app` は未署名です。初回は「右クリック → 開く」で Gatekeeper を通してください
- GL 経路 (旧 Mac 向け) のライブラリ本体は後続 wave で供給予定です
