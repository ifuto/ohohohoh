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

- count: 72
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
- 2026-07-22: wave 11 診断B18 (feather-OFF 共通基底で pull→MDI→noise→budget 逐次)
- 2026-07-22: wave 11 診断B19 (211 確定済 → 209 MDI切 から逐次)
- 2026-07-22: wave 11 診断B20 (実デモ列データで tick_world 直接駆動プローブ)
- 2026-07-22: wave 11 診断B21 (全strip基底に1系統ずつ復帰: MDI/noise/occ/budget/pull)
- 2026-07-22: wave 11 診断B22 (交互対照 off/on/off/on: フレーク vs 設定因果)
- 2026-07-22: wave 11 完結 (M-4 zero-day 根治: bytemuck 空align パニック + 診断装置撤去 + 最終 567テスト)
- 2026-07-22: wave 11 診断F1 (M-4 根治後 det 系の最小 guard 再装着)
- 2026-07-22: wave 11 最終形 (guard 撤去: M-4 根治 + det 567テスト 検証)
- 2026-07-22: wave 12 — bobby_cache 全通読監査 (N-1 rebuild 時刻復元で LRU 決定性根治 / N-6 dim汚染skip / N-5 doc正直化 / N-2/N-4 防御) + テスト4本 (opt-gfx 571)
- 2026-07-22: wave 13 — binary_greedy_meshing 全通読監査 (等価性再証明15件陰性 + AVX2/SWAR照合・span境界テスト + face_culling実意味doc化) (opt-gfx 573)
- 2026-07-22: wave 14 — frame_worldgen 全通読監査 (P-5 stride=0明示拒否 / P-6 CPUミラー6平面制限でWGSL完全一致回復 / 未接続状況記録) (opt-gfx 575)
- 2026-07-22: wave 15 — frame_postfx 全通読監査 (Q-1 GPU露出適用の未完配線を根治 run_apply 実装 / Q-5 vrs tile=0 / Q-6 CAS・checker長さ assert) (opt-gfx 579)
- 2026-07-22: wave 16 — frame_ddgi 全通読監査 (R-8 GPU rays 64スロット OOB 未強制 / R-4 march 65.0 上限 / R-7 oct_w=0 OOB化 / R-3 sky 契約 → validate() 一元化で両入口強制) (opt-gfx 582)
- 2026-07-22 (count 57): wave 21-22 — frame_pipeline (fs_pull Lambert 根治/SUN_DIR 正準化) + occlusion_query (near-plane clip 根治) 追加テスト計 7 件の CI 緑確認
- 2026-07-22 (count 58): wave 23 — taa/drs 監査 (NaN 伝搬根治 + 厳密ビットピン 5 件) の CI 緑確認
- 2026-07-22 (count 59): wave 24-25 — taa_ycocg / texture_atlas 監査 (契約化 + 厳密値ピン 12 件) の CI 緑確認
- 2026-07-22 (count 60): wave 26 — triple_buffer 単一バッファ退化根治 + spatial_hash 契約化 (7 件) の CI 緑確認
- 2026-07-22 (count 61): wave 27 — simd_kernels Gribb near 根治 + 厳密平面値ピン (5 件) の CI 緑確認
- 2026-07-22 (count 62): wave 28 — mesh_compactor Gribb near 根治 (第3 extractor 統一) + WGSL wire format 厳密ピン (4 件) 完了。CI 緑確認
- 2026-07-22 (count 63): wave 29 — simd_frustum SoaAabbs 等長契約 fail-loud 化 (UB 根絶) + ディスパッチ契約化 (6 件) 完了。CI 緑確認
- 2026-07-22 (count 64): wave 30 — azdo overhead_saved 境界根治 + doc 正直化 + mask 等長契約化 (2 件) 完了。CI 緑確認
- 2026-07-22 (count 65): wave 31 — diff_mesh 契約明文化 + 厳密列ピン (4 件) 完了。CI 緑確認
- 2026-07-22 (count 66): wave 32 — tick_render_split NaN 永久汚染根治 + 厳密列ピン (4 件) 完了。CI 緑確認
- 2026-07-22 (count 67): wave 33 — soa_layout Aligned64 根治 (ゼロデイ) + 契約化 (8 件) 完了。CI 緑確認
- 2026-07-22 (count 68): wave 34 — entity_tick_lod 非有限 fail-loud + hashed 契約化 (6 件) 完了。CI 緑確認
- 2026-07-22 (count 69): wave 35 — execute_indirect MDI 容量契約 fail-loud 化 + wire ピン (6 件) 完了。CI 緑確認
- 2026-07-22 (count 70): wave 36 — billboard_lod NaN 静寂蒸発 fail-loud + 基底契約化 (5 件) 完了。CI 緑確認
- 2026-07-22 (count 71): wave 37 — texture_atlas_virtual 非決定 evict 根治 + 3 契約化 (6 件) 完了。CI 緑確認
- 2026-07-22 (count 72): wave 38 — gl33_compat timer 混入根治 + VecDeque 化 + 2 契約化 (5 件) 完了。CI 緑確認
