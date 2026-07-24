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

- count: 126
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
- 2026-07-22 (count 73): wave 39 — simd_frustum.wgsl 実カーネル化 (スタブ closing 第 1 弾) (2 件) 完了。CI 緑確認
- 2026-07-22 (count 74): wave 40 真の LRU closing の CI 緑確認
- 2026-07-23 (count 75): wave 41 lbvh 監査の CI 緑確認
- 2026-07-23 (count 76): wave 42 sparse_texture 監査・WGSL 実カーネル化の CI 緑確認
- 2026-07-23 (count 77): wave 43 mip_streaming 監査の CI 緑確認
- 2026-07-23 (count 78): wave 44 mesh_compactor ワイヤ closing の CI 緑確認
- 2026-07-23 (count 79): wave 45 azdo MDI closing の CI 緑確認
- 2026-07-23 (count 80): wave 46 bindless 監査の CI 緑確認
- 2026-07-23 (count 81): wave 47 meshlet_cone 監査の CI 緑確認
- 2026-07-23 (count 82): wave 48 intern_pool 監査の CI 緑確認
- 2026-07-23 (count 83): wave 49 string_intern 監査の CI 緑確認
- 2026-07-23 (count 84): wave 50 texture_budget 監査の CI 緑確認
- 2026-07-23 (count 85): wave 51 zerocopy_cast 監査 + wave 50 CI 失敗の再現性切り分け
- 2026-07-23 (count 86): wave 52 f16 エンコーダ・クラスタ根治の CI 緑確認
- 2026-07-23 (count 87): wave 53 r10g10 根治の CI 緑確認
- 2026-07-23 (count 88): wave 54 enhanced_barriers 仕様適合化の CI 緑確認
- 2026-07-23 (count 89): wave 55 packed4 語彙 fail-loud 化の CI 緑確認
- 2026-07-23 (count 90): wave 56 pull_mesh 単一真実源化の CI 緑確認
- 2026-07-23 (count 91): wave 57 svo trace 根治の CI 緑確認
- 2026-07-23 (count 92): wave 58 gpu_vertex_pull 死に構造撤去の CI 緑確認
- 2026-07-23 (count 93): wave 59 chunk_mesh 監査の CI 緑確認
- 2026-07-23 (count 94): wave 60 voxel_cone_tracing 監査の CI 緑確認
- 2026-07-23 (count 95): wave 61 section_rle 監査の CI 緑確認
- 2026-07-23 (count 96): wave 62 noise_upsample worldgen ゼロデイ根治の CI 緑確認
- 2026-07-23 (count 97): wave 63 frame_reference 監査 (covered/avg_lum 修復) の CI 緑確認
- 2026-07-23 (count 98): wave 64 fsr1 監査 (死引数・死に状態解消、厳密ピン) の CI 緑確認
- 2026-07-23 (count 99): wave 65 cas 監査 (WGSL 加算順 3連鎖統一) の CI 緑確認
- 2026-07-23 (count 100): wave 66 aces_tonemap 監査 (Narkowicz 一次情報照合、厳密ピン) の CI 緑確認
- 2026-07-23 (count 101): wave 67 checkerboard 監査 (wrapping parity 統一) の CI 緑確認
- 2026-07-23 (count 102): wave 68 wboit 監査 (NaN 静寂最近接化遮断) の CI 緑確認
- 2026-07-23 (count 103): wave 69 temporal_mesh_diff 監査 (責務誠実化・決定的 drain) の CI 緑確認
- 2026-07-23 (count 104): wave 70 vrs 監査 (tile 契約・閾値境界厳密固定) の CI 緑確認
- 2026-07-23 (count 105): wave 71 lod_hybrid 監査 (NaN 距離遮断・真値表確定) の CI 緑確認
- 2026-07-23 (count 106): wave 72 vertex_cache_opt 監査 (len%3・cache_size 契約、厳密出力ピン、WGSL ミラー改修) の CI 緑確認
- 2026-07-23 (count 107): wave 73 world_column_store 監査 (非有限カメラ拒否・帳簿差分会計化・窓/行列厳密ピン) の CI 緑確認
- 2026-07-24 (count 108): wave 74 transform_svdag 監査 (BX-1 中間ヒット誤タグの D4 群合成修復・群閉包公理ピン) の CI 緑確認
- 2026-07-24 (count 109): wave 75 vertex_pool 監査 (oversize 拒否の stale slot 一貫化・refresh 契約ピン) の CI 緑確認
- 2026-07-24 (count 110): wave 76 persistent_vbo_pool 監査 (BZ-1 確保リーク rollback・BZ-2 stale slot・BZ-6 配分最適化) の CI 緑確認
- 2026-07-24 (count 111): wave 77 svdag 監査 (CA-1 root_id stale メタデータ根治・厳密構築ピン・不変量精緻化) の CI 緑確認
- 2026-07-24 (count 112): wave 78 aokana 監査 (CB-1 非決定反復の sort 根治・境界/置換ピン・doc 誠実化) の CI 緑確認
- 2026-07-24 (count 113): wave 79 visibility_buffer 監査 (CC-1 pack fail-loud・CC-3 虚構 WGSL ジェネレータ完全実装) の CI 緑確認
- 2026-07-24 (count 114): wave 80 hzb_2d 監査 (CD-1 i8 underflow・CD-2 キー衝突・CD-3 深度近/遠逆転・CD-4 8角厳密射影+凸包scanline・CD-5 ceil mip 等 9 項目) の CI 緑確認
- 2026-07-24 (count 115): wave 81 cpu_occlusion+fxaa 監査 (CE-3 フィールド非伝播罠の語彙ピン・CE-4 shade 厳密 bit ピン) の CI 緑確認
- 2026-07-24 (count 116): wave 82 occlusion_complete 監査 (CF-1 転置射影 [C]・CF-2 深度近/遠逆転・CF-3 complete-dead 三角形判定・CF-4 rect 画面外誤記述・CF-5 project 分割+凸包 scanline・CF-6 hysteresis・CF-7 doc 誠実化・CF-8 厳密ピン) の CI 緑確認用
- 2026-07-24 (count 117): wave 83 full_graph_wiring 第1部監査 (CG-1 転置フラスタム抽出 [高]・CG-7 PSO 常時ミス・CG-2/8 虚偽供給・CG-3-6 誠実化 計8項目、922 全緑) の CI 緑確認用
- 2026-07-24 (count 118): wave 84 full_graph_wiring 第2部監査 (CH-1 FSR1 フランケン近傍・CH-2 異ステージ混在・CH-3 真 FOV 配線・CH-4-6 誠実化 計6項目、922 全緑+fsr1 ピン厳密化) の CI 緑確認用
- 2026-07-24 (count 119): wave 85 full_graph_wiring 第3部監査 (CI-1 gigabuffer 二重虚偽根治 FIFO+単一retry 化・CI-2 patch キー誠実注記 計2項目、923 全緑) の CI 緑確認用
- 2026-07-24 (count 120): wave 85 CI flaky 診断の再実行 (d70e354 同等、ローカル全工程グリーン再現済)
- 2026-07-24 (count 121): wave 86 render_pipeline 再監査第1部 (CJ-1 wiring 入力欠落併合・CJ-3 diff ダーティ帯域・CJ-2 verdict 統一・CJ-4 SVO 決定論選択・CJ-5/6 誠実化 計6項目、927 全緑+adversarial 2 系統検出) の CI 緑確認用
- 2026-07-24 (count 122): wave 87 render_pipeline 第2部 (CK-1 派生キャッシュ prune/invalidate 追随・CK-2 HZB y 帯整合・CK-3/4/5 誠実化 + chunk_origin 語彙突合 計5項目、930 全緑+adversarial 2 系統検出、復旧手順2種完走) の CI 緑確認用
- 2026-07-24 (count 123): wave 88 entity_culling 監査 (CL-2 moved 即再評価・CL-3 ゲート遅延適用完成・CL-5 total valid 化・CL-4 fov_cos_min 化・CL-1/7 誠実化 計7項目、934 全緑+捕捉16件目+adversarial 4 系統) + 台帳全カウンタ機械積算正規化 (256) の CI 緑確認用
- 2026-07-24 (count 124): wave 89 iris_pipeline 監査 (CM-1 composite 命名根治・CM-2 discover→resolve 往復破綻根治・CM-3 fullscreen v 反転根治・CM-4 zip-bomb 実展開長遮断・CM-5 discover 3 点・CM-6 last_uniforms 保持・CM-7 観察 計7項目、941 全緑+adversarial 3 系統+台帳再計測 274) の CI 緑確認用
- 2026-07-24 (count 125): wave 90 bc7_ktx2 監査 (CN-1 KTX2 mip 順序根治・CN-2 DFD u32 直列化根治・CN-3 BC7 DFD 定数正写像・CN-4 残骸除去・CN-5 証明/検算 計5項目、944 全緑+adversarial 4 系統、Bash 機械検算規律初適用) の CI 緑確認用
- 2026-07-24 (count 126): wave 91 gpu_culling 監査 (CO-1 GPU 返り値契約明文化・CO-2 frustum doc 誠実化・CO-3 バッファプール償還成長化・CO-4 chunk_count 契約ピン+trace 誠実化・CO-5 オフセット機械ピン 計5項目、946 全緑+adversarial 1 系統) の CI 緑確認用
