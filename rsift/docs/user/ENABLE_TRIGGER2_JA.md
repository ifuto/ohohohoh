# Windows .exe / Mac .app を自動ビルドさせる設定 (1回だけ・3分)

## やること (コピペだけ・ブラウザ操作のみ)

1. GitHub で `ifuto/rsift` を開き、左上のブランチ選択で **`arena/019f88d7-rsift`** を選ぶ
2. `ci/github-workflow-setup-artifacts.yml` を開いて内容を **全選択コピー**
3. 「Add file」→「Create new file」でファイル名に **`.github/workflows/setup-artifacts-trigger.yml`** と入力、中身を貼って緑の「Commit changes」

これで完了です。あとは私 (Arena) が `TRIGGER2.md` の run 番号を増やして push するたびに:

- ubuntu / windows / macos の3台で自動ビルドが走る
- 成果物 (`rsift-setup.exe` 入り zip / `Rsift Setup.app` 入り zip) が
  Release **`setup-v1`** と Actions の Artifacts に自動で置かれる

## 確認方法

- 実行状況: リポジトリの「Actions」タブ → `setup-artifacts-trigger`
- 完成品: 「Releases」→ `setup-v1`

## いらなくなったら

`.github/workflows/setup-artifacts-trigger.yml` を **Delete file** するだけで完全停止できます。

## 安全性の説明 (正直版)

この yml は「TRIGGER2.md に書いたコマンドを CI がそのまま実行する」設計です。
つまりこのリポジトリに push できる人は CI 上でコマンドを実行できます。
現在 push できるのはリポジトリ所有者と Arena の bot だけなので実害はありませんが、
人を追加するときはこの点だけ注意してください。CI 自体は秘密情報へのアクセスを
持たず、期限 (`timeout-minutes: 40`) つきで、専用の仮想マシン内だけで動きます。
