# Rsift Launcher — ふわモダン UI + Minecraft ランチャー方式起動構成
**Date: 2026-07-18 / Crates: `rsift-app`, `rsift-launch`**

---

## 1. 決定事項

ユーザー方針: 「Minecraftランチャー起動構成方式で十分」
→ **ネイティブ注入 (JNI_CreateJavaVM + rsift_jvm.dll) は後回し**。
公式ランチャーと同じく、**フル構成したコマンドラインで `javaw` / `java` を子プロセス起動**する方式を本実装。

デザイン方針: 「めっちゃくちゃお洒落。モダンでかっこいいけどちょっとふわっとしてる感じ」
→ ふわふわ ambient orbs (aqua×lavender×peach×mint) + パステルパレット + ヒーローカードの浮遊 (sin bob) + PLAY ボタンの呼吸グロー + パステルチップ。

---

## 2. `rsift-launch` 新規モジュール: `subprocess.rs`

| API | 内容 |
|---|---|
| `prepare_vanilla_launch(cfg, java_override)` | version JSON 連鎖解決 → Java 解決 → classpath → natives 展開 → JVM/ゲーム引数の `${...}` 置換 → `LaunchPlan`。**バックグラウンドスレッド向け** |
| `LaunchPlan::spawn()` | `javaw.exe` 優先 (コンソール非表示) で `Command::spawn`。stdout/stderr を pipe。UI スレッドで呼ぶ |
| `list_launchable_versions()` | `rsift-loader-*` 優先 + バニラ直接起動も可 (json+jar 必須) |

置換対応トークン: `${natives_directory}` `${classpath}` `${classpath_separator}` `${launcher_name}` `${launcher_version}` `${rundir}`。version JSON の `-Xms/-Xmx` はプロファイル値で上書き。`-cp` がテンプレに無い場合は自動付与。

既存変更:
- `offline.rs`: `build_game_args` を `pub(crate)` 化 (subprocess から再利用)
- `lib.rs`: subprocess モジュール公開
- 従来の `launch_offline` (JNI 方式) は残存・無変更

---

## 3. `rsift-app` UI 全面リニューアル

### ページ構成 (5ページ)
| ページ | 内容 |
|---|---|
| ▶ ホーム | 浮遊ヒーローカード (bob アニメ)、パステル情報チップ3連 (version/メモリ/解像度)、呼吸グローPLAYピル (300×64, radius 32)、実行中バー (PID表示+終了ボタン)、公式ランチャー呼び出しは副ボタンに格下げ |
| ◆ 構成 | **プロファイル ComboBox 切替 / 新規 / 二段階確認削除**、名前・ユーザー・バージョン・解像度 |
| ▼ インストール | .minecraft パス、Rsift インストール/更新 + Spinner、ログは console_frame |
| ⚙ 設定 (新) | Java 手動パス (空=自動検出, `#[serde(default)] java_override`)、Xms/Xmx GB スライダー、追加JVM引数 |
| ≡ ログ (新) | 起動コマンド + 子プロセス stdout/stderr リアルタイム表示 (600行リング, stick_to_bottom) |

### ふわっと要素
- ambient orbs を 3→**5個** (AQUA_GLOW / AQUA / LAVENDER / PEACH / MINT) + ゆっくりドリフト
- **12個の瞬く小光** (スパークル、sin 位相差でふわっと明滅)
- ヒーローカードが sin bob で上下に浮遊 (±6px)
- PLAY ボタン: 呼吸するグローリング (alpha 20–44) + グラデ + リム光
- サイドバーにアイコン追加・角丸 14px、ヒーローは radius 26/ぼかし影 48

### 起動フロー
1. ホームで PLAY → UI フリーズさせずバックグラウンドで `prepare_vanilla_launch`
2. `LaunchPlan` → UI スレッドで spawn → stdout/stderr を読取りスレッド → ログページへ流す
3. `try_wait` で毎フレーム終了検知 → ステータスバーに表示
4. ランチャーを閉じてもゲームは継続 (Child drop は kill しない)

---

## 4. 変更ファイル一覧
- `crates/rsift-launch/src/subprocess.rs` (新規)
- `crates/rsift-launch/src/offline.rs` (build_game_args → pub(crate))
- `crates/rsift-launch/src/lib.rs` (export 追加)
- `crates/rsift-app/src/app.rs` (全面改修)
- `crates/rsift-app/src/theme.rs` (LAVENDER/PEACH/MINT + hero_card/chip_frame/console_frame)
- `crates/rsift-app/src/profiles.rs` (java_override 追加, list_launchable_versions 使用)

## 5. ビルド・検証ループ (Arena は Rust ビルド不可: RAM 1.9GB)
1. Windows 側で HF から最新を取得
2. `cargo check -p rsift-app` → エラーがあり次第ログを共有
3. `cargo build -p rsift-app --release` → `target/release/rsift-app.exe`
4. 動作テスト: ホームで PLAY → Java 21+ 検出 → `javaw` で 1.21.11 起動 → ログページに出力
