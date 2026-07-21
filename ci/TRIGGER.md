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

- count: 17
- 2026-07-21: 初回設置 (bench-ci セットアップ)
- 2026-07-21: 初回実起動 (404テスト + pseudo digest + wide 357行 digest ゲート検証)
- 2026-07-21: 完璧追求バッチ検証 (435テスト + pseudo/wide digest + render 14テスト)
- 2026-07-21: caves メッシャー bit カラム化検証 (437テスト + 実測 −81% digest 不変)
- 2026-07-21: 12B greedy 経路のカラム bit 化完遂検証 (439テスト: 12B 等価 fuzz 2件追加)
- 2026-07-21: zero-test 4モジュール消化 + Mods ボタン正直性修正 (456テスト)
- 2026-07-21: zero-test 第2波4モジュール消化 (471テスト: tiling/lod/pool/dag)
- 2026-07-21: zero-test 第3波4モジュール消化 (481テスト: mimalloc/pgo/rayon/zerocopy)
- 2026-07-21: 第3波失敗調査 (zerocopy 空cast の align 固定 + 失敗再現確認)
- 2026-07-21: zero-test 第4波3モジュール消化 (489テスト: micro_lod/instanced/dashmap)
- 2026-07-21: 第4波 lib 失敗の切り分け再実行 (変更無し同一内容 — フレーク判定)
- 2026-07-21: zero-test 第5波2モジュール消化 (496テスト: bundle/atlas_virtual)
- 2026-07-21: Mod Menu 一覧→詳細2画面化 + 重複/UTF-8 修正 (api テスト +7)
- 2026-07-22: zero-test 第6波8モジュール+hzb横断修正 (527テスト: heap_ring/barriers/splash/shading/pso/root_sign/cpu_occ/simd + Hi-Z 永久無効化/u32 overflow/cross-arch 乖離の3修正)
- 2026-07-22: zero-test 第7波3モジュール消化 (546テスト: gui スライダ即死/frustum 全件カリング矛盾/pool 0除算の重大3修正 + WGSL naga 検証)
- 2026-07-22: 第7波 診断D1 (wave-7 テスト cfg 切断: 本番差分/旧テストの分離)
- 2026-07-22: 第7波 診断D2 (naga WGSL テストのみ ignore で切り分け)
