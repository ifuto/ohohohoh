# CI トリガー囮ファイル (ci/TRIGGER.md)

このファイルは `.github/workflows/bench.yml` の `paths` フィルタが監視している
**囮ファイル**です。**このファイルを更新して push するだけで bench-ci が起動します**
(実コードに変更が無いときでも CI を走らせたい用途のための仕掛け)。

## AI エージェントの使い方

1. 下のカウンタを +1 し、日付と理由を 1 行追記する
2. このファイルだけを commit & push する:
   `git add ci/TRIGGER.md && git commit -m "ci: trigger bench (理由)" && git push origin arena/019f79db-rsift`
3. `gh run list --branch arena/019f79db-rsift --limit 3` で run の状態を確認
4. 完了後に結果を見る:
   - `gh run view <run-id> --log` … 全ログ (bench 全出力も "show bench log" ステップに出る)
   - `gh run download <run-id> -n bench-logs-run-<番号>` … `bench-out/` (run1/run2 生ログ、
     run*.filtered.txt 決定的ダイジェスト、diff12.txt、sha256.txt)

## カウンタ

- count: 9
- 2026-07-21: 初回設置 (bench-ci セットアップ)
- 2026-07-21: 初回実起動 (404テスト + pseudo digest + wide 357行 digest ゲート検証)
- 2026-07-21: 完璧追求バッチ検証 (435テスト + pseudo/wide digest + render 14テスト)
- 2026-07-21: caves メッシャー bit カラム化検証 (437テスト + 実測 −81% digest 不変)
- 2026-07-21: 12B greedy 経路のカラム bit 化完遂検証 (439テスト: 12B 等価 fuzz 2件追加)
- 2026-07-21: zero-test 4モジュール消化 + Mods ボタン正直性修正 (456テスト)
- 2026-07-21: zero-test 第2波4モジュール消化 (471テスト: tiling/lod/pool/dag)
- 2026-07-21: zero-test 第3波4モジュール消化 (481テスト: mimalloc/pgo/rayon/zerocopy)
- 2026-07-21: 第3波失敗調査 (zerocopy 空cast の align 固定 + 失敗再現確認)
