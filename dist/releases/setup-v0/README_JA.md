# Rsift セットアップ (.exe / .app) の使い方

ダウンロードした zip を解凍し、**実行ファイルと dll/dylib を全部同じフォルダに置いて、実行するだけ**です。

| OS | 実行ファイル | 置くもの (例) |
|---|---|---|
| Windows | `rsift-setup.exe` | `rsift.dll` / `rsift_gfx_vulkan.dll` または `rsift_gfx_dx12.dll` |
| Mac | `Rsift Setup.app` (中身は `rsift-setup`) | `librsift.dylib` / `librsift_gfx_metal4.dylib` / `librsift_gfx_metal.dylib` / `librsift_gfx_gl.dylib` |

実行すると自動で:

1. 同じ階層の dll/dylib を検出して SHA-256 を計算
2. 検証 (初期配布はハッシュ記録のみ。次回リリースから改竄検出が有効化)
3. あなたの PC に合った RsGraphics 経路を決定
   - Mac (Apple Silicon + macOS 26+) → **Metal 4**
   - Mac (上記以外) → classic Metal / GL(後続 wave で供給)
   - Windows → Vulkan 優先、無ければ DX12
4. `rsift_launch.json` (起動構成) を生成
5. 全工程をログに記録

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
