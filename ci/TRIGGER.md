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

- count: 43
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
- 2026-07-22: 第7波 診断C3 (WGSL @compute 手前プレフィックスで main 有無を切り分け)
- 2026-07-22: 第7波 診断C4 (wgpu シェーダから空if除去 = naga 0.20 非受理容疑の根治 + naga テスト再有効化)
- 2026-07-22: 最終 zero-test full_graph_wiring 消化 (555テスト: naga 空if犯人確定 + AO 輝度スケール逆転文書訂正)
- 2026-07-22: wave 9 — tick_world 全通読監査 (K-1..K-3 修正 + K-4 明文化) + 統合テスト3本 (opt-gfx 555→558)
- 2026-07-22: wave 9 lib 失敗の切り分け再実行 (変更無し同一内容 — フレーク判定)
- 2026-07-22: wave 9 診断B1 (chunked 単独切り分け: empty/periodic を cfg 切断)
- 2026-07-22: wave 9 根治 (visgraph_reachable: flood_fill 始点含有設計に期待値訂正 0→1) 全3テスト再有効化
- 2026-07-22: wave 10 — paging 真LRU根治(L-1/L-2) + region OOB拒否(L-3/L-4) + WGSL レイアウト陰性確認(563テスト)
- 2026-07-22: wave 10 根治 (LRU テストのページ譲受トレース期待値誤記 1 箇所訂正: D は page0 譲受)
- 2026-07-22: wave 11 — render_pipeline 全通読 (M-1 実速度計測根治) + 実統計交差決定性 (565テスト)
- 2026-07-22: wave 11 診断B2 (speed 単独切り分け: determinism テストを cfg 切断)
- 2026-07-22: wave 11 診断B3 (determinism 半分分割: det_core / det_pull)
- 2026-07-22: wave 11 診断B4 (det_core 単独切り分け)
- 2026-07-22: wave 11 診断B5 (det_core さらに半分: core_x ビルド/カリング量系 / core_y 判決/wiring/速度系)
- 2026-07-22: wave 11 診断B6 (フィールド不一致の終了コード化: byte チャネル診断)
- 2026-07-22: wave 11 診断B7 (frame/new パニックの catch_unwind 独立コード化)
- 2026-07-22: wave 11 診断B8 (パニックのコンテンツ依存プローブ4本)
- 2026-07-22: wave 11 診断B9 (core_x/y 一時切断でプローブコードを解放)
- 2026-07-22: wave 11 診断B10 (2chunk/occ切/遠方単独/frustum切 4プローブ)
- 2026-07-22: wave 11 診断B11 (occ ON(57) vs OFF(83) の2本のみ解放)
- 2026-07-22: wave 11 診断B12 (シングル4本のみ解放: コンテンツ既知判明)
- 2026-07-22: wave 11 診断B13 (逐次ステージプローブ: prepare→build→単独frame→2chunk)
- 2026-07-22: wave 11 診断B14 (選択切断プローブ: 全strip/HZB切/feather切/MDI+pull切)
- 2026-07-22: wave 11 診断B15 (matrix 逐次化: 201=full_strip 203=no_hzb 205=default)
- 2026-07-22: wave 11 診断B16 (no-HZB 基底で6システム逐次切断: feather/MDI/pull/noise/occ/budget)
- 2026-07-22: wave 11 診断B17 (203 config 確定済のため除外して 207 から逐次)
