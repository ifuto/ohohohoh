# 修正台帳 (Bugfix & Inappropriate-Fix Registry)

**制定: 2026-07-24 / 根拠指示 (原文): 「mdに今まで直したすべてのバグや不適切な
何かを全部記載していって」(2026-07-24)**
**運用 (エージェント義務): 新たな修正・根治をコミットする際は、必ず本台帳へ
同セッション内に追記すること。詳細な問題定式化・検証手順は
`internal/AUDIT_2026-07-21_RSGFX_MODULES.md` の各節を参照 (本台帳は全項目の
索引 + 一行要約)。**

## 凡例

- 深刻度 (audit 見出しの転記): **[C]** critical・**[高]**・**[中高]**・**[中]**・
  **[低]**・**[観]** = 観測・誠実性訂正・判断記録 (コード bug ではない修正)、
  **[基]** = 検証基盤の追加 (naga 突合等)。
- 修正消費テスト数: 監査開始時 363 → wave 92 時点 **949 全緑** (実測、CP 3 件追加)。

## 統計 (数え上げ、推測なし)

| 期 | 範囲 | 項目数 |
| :--- | :--- | ---: |
| A 期: 初回大監査 + zero-test 消化 | 2026-07-21〜22 早段 | 20 |
| B 期: 全通読監査 | wave 9-21 (K-W 節) | 37 |
| C 期: 契約 fail-loud 期 | wave 22-41 (X-AQ 節) | 45 |
| D 期: 厳密ピン/closing 期 | wave 42-71 (AR-BU 節) | 82 |
| E 期: 横断契約クラス期 | wave 72-99 (BV-CX 節) | 141 |
| **合計** | **機械積算値** (計測: `grep -cE '^\| (A期\|[A-Z]{1,3})-[0-9]+'` — 全表の ID 行数。2026-07-24 wave 89: wave 88 計測 (256) が複合 ID 行 9 + 接尾辞行 2 をパターン狭窄で落としていたことを検出・全期再計測で訂正。A 期は漢字 ID (A期-NN) も含む) | **325** |

(「項目数」は下表の行数 = 台帳作成時に機械計測。1 行 = 1 つの修正/根治/
訂正判断。陰性確認・判定記録 (変更なし判断) は修正ではないため原則除外
するが、根拠が重いもの (一次情報照合による維持判断) は [観] として収録。)

---

## A 期: 初回大監査・ゼロテスト消化期 (2026-07-21〜22)

| ID | 度 | 概要 |
| :--- | :- | :--- |
| A期-01 | C | アルゴリズム偽装の根絶 (初回監査・検出全 10 件の中核、詳細は監査方法節) |
| A期-02 | 高 | 出荷済み WGSL の検証失敗 3 件 (実デバイスでシェーダコンパイル失敗) |
| A期-03 | 中 | コンパイラ警告が示す死骸・API ハザード 6 件の処理 |
| A期-04 | 中 | デッドフィールド/見せかけペイロード撤去 (rsift-opt-gfx 11 ファイル) |
| A期-05 | 観 | 見せかけ UI データ撤去 (1 ファイル) |
| A期-06 | 中 | デッドコード除去 (他クレート 7 ファイル、全て参照ゼロ証明つき) |
| A期-07 | 高 | rsift-jvm モジュール二重ロード根絶 (構造欠陥・実害あり) |
| A期-08 | 高 | 既存テストが露呈した実バグ 2 件修正 (HEAD 再現確認済) |
| A期-09 | 観 | ドキュメント主張と実装の乖離訂正 (2 ファイル) |
| A期-10 | 観 | 資産削除 (不要 WGSL 1 ファイル、修正一覧計 21 ファイル+1) |
| A期-11 | 観 | ベンチ方法論の自己修正: フェイク計測の排除 (誠実計測の確立) |
| A期-12 | 中 | 実測駆動の改善群 (digest 不変 / fuzz で出力 bit 同一性を証明) |
| A期-13 | 高 | zero-test 第 6 波: 横断潜伏バグ 3 件修正 (+簿記/環境構築系 8 モジュール消化) |
| A期-14 | 高 | 起動クラッシュ級バグ 2 件修正 (zero-test 第 7 波、eco_render/gpu_culling/gui_settings 消化) |
| A期-15 | 基 | naga 0.20 拒否の二分探索確定 + 最終 zero-test モジュール消化 |
| A期-16 | 中 | caves merge 断片化コスト (47ns/voxel) の解決 |
| A期-17 | 観 | 描画バックエンドラダー (ユーザー方針のコード化、コミット e76248b) |
| A期-18 | 基 | 広域静的ベンチ wide_static_bench 整備 (digest 運用の基盤) |
| A期-19 | 観 | Mod Menu の Fabric 参考再設計 / データ変換層厳密テスト (+31) / 文書乖離訂正 |
| A期-20 | 中 | tick_world 全通読監査の指摘対応 (K 節・wave 9 系の事前処理含む) |
| — | — | (以降、項目は wave 節と同一 ID 体系で記載) |

## B 期: 全通読監査 (wave 9-21)

| ID | 度 | 概要 |
| :--- | :- | :--- |
| K系 | 中 | tick_world 全通読監査 (wave 9) — 各指摘の処置は K 節参照 |
| L系 | 中 | ページング/リージョンコーデック監査 (wave 10) — 各指摘は L 節参照 |
| M-4 | 高 | render_pipeline: 空バイト列の bytemuck アライメントパニック (zero-day 確定) 根治 |
| N-1 | 高 | bobby_cache: rebuild_index の time_ms=0 埋め → 再起動後 LRU 非決定堕落を根治 |
| N-2 | 低 | parse_chunk_name が 3 セグメントを受理 (防御契約化) |
| N-4 | 低 | store の index 更新で poison 黙殺を解消 |
| N-5 | 観 | doc 圧縮レベル 19 宣言 vs 実装 13 の誠実化 |
| N-6 | 中 | 次元ディレクトリ名 parse 失敗の `.unwrap_or(0)` → dim=0 汚染を根治 |
| O-16 | 中 | binary_greedy_meshing: 未接続 unsafe SIMD に等価性テスト不足 → 配線・証明 |
| O-24 | 観 | `face_culling` パラメータ命名と実体の乖離を誠実化 |
| P-5 | 中 | frame_worldgen: `coarse_dims` stride=0 除算パニックを契約化 |
| P-6 | 観 | CPU ミラー planes 全件走査の「完全一致」主張を誠実化 |
| Q-1 | 高 | frame_postfx: GpuExposure apply 経路未完 (宣言と実体の乖離) 根治 |
| Q-5 | 中 | vrs tile=0 の 0 除算パニック契約化 |
| Q-6 | 中 | cas/checker CPU ミラーの src 長さ未検査を契約化 |
| R-3 | 中 | frame_ddgi: sky>0 が CPU 直構築経路で未強制 → validate() 一元化 |
| R-4 | 中 | march の 129 開始値上限を契約化 |
| R-7 | 中 | oct_w=0 で sample_visibility が u32 アンダーフロー → OOB を根治 |
| R-8 | 高 | GPU rays_buf 64 スロット固定設計を公開 API で強制 (validate() 一元化) |
| R-10 | 低 | read_f32 読み戻し 3 重複の集約 |
| S-1 | C | FSR1 RCAS が「ぼかし」だったアルゴリズム偽装を根治 (wave 1 A1 同種) |
| S-2 | C | frame_reference 深度が透視補正補間だった GPU 乖離を根治 |
| S-3 | 高 | FramePacer::next_present_time 逐次加算ループの実質ハング根治 |
| S-4 | 中 | EASU 勾配チャンネル不統一 (3 連鎖ドリフト) 根治 |
| S-5 | 中 | GpuFsr1Pass 次元チェック不在を契約化 |
| S-6 | 基 | naga による WGSL↔Rust ワイヤ形式自動突合の新設 |

## C 期: 契約 fail-loud 期 (wave 22-41)

| ID | 度 | 概要 |
| :--- | :- | :--- |
| T-1 | C | frame_reuse: render_pipeline record_miss 併呼の二重計上根治 |
| T-2 | 高 | full_mesh.absent の無計上早期 return (サイレントミス) 根治 |
| T-3 | 高 | enforce_capacity タイブレーク不在の非決定性根治 |
| T-4 | 中 | memory_bytes 幻影計上と欠損 (帳簿虚偽) 根治 |
| T-5 | 低 | stale 境界の厳密固定 |
| T-6 | 観 | rle_fingerprint デッドコード除去 |
| U-1 | 観 | frame_vct: new_normalized ゼロ方向 doc 訂正 |
| U-2 | 低 | 空コーン列の純 Rust 早期 return (防御) |
| U-3 | 基 | naga offset レベル突合へ格上げ |
| V-1 | 低 | frame_hiz: 画面寸法 0 契約不在 (fail-silent 防止) |
| V-2 | 基 | naga offset 突合 3 例目 |
| W-1 | 高 | frame_pipeline: fs_pull Lambert 欠落 (GPU/CPU 乖離) 根治 |
| W-2 | 中 | SUN_DIR 非単位光ベクトル + doc 偽の厳密再導出 |
| X-1 | 中高 | occlusion_query: near-plane ストラドル面全スキップ (描画抜け) 根治 |
| X-2 | 観 | 深度タイブレーク ε バイアスを契約として正直化 |
| Y-3 | 中 | drs::push_frame_ms の NaN 永久汚染根治 |
| Y-1/Y-2/Y-4 | 観 | reproject_uv 符号・blend clamp 非対称の誠実化 + EMA/resolve ビットピン |
| Z-1 | 中 | taa_ycocg: gamma 契約の panic DX を明示 fail-loud 化 |
| Z-2 | 観 | YCoCg 往復は bit 厳密でない旨の誠実化 |
| AA-1 | 中 | texture_atlas: 次元契約の fail-loud 化 3 箇所 (fail-silent 経路閉塞) |
| AA-2 | 低 | シェルフ配置・UV・mip 内容の厳密ピン |
| AB-1 | 高 | triple_buffer: 単一バッファ退化 (並行バグ) 根治 |
| AB-2 | 中 | spatial_hash: 非有限/逆転の契約化 |
| AC-1 | 中 | simd_kernels: near 平面の GL 式体積 (規約逸脱) 根治 |
| AC-2 | 観 | doc 偽 2 件訂正 |
| AD-1 | 中 | mesh_compactor: from_view_proj near GL 式体積根治 |
| AD-2 | 観 | COMPACT_WGSL 契約注記の確定等 3 点正直化 |
| AE-1 | 高 | simd_frustum: SoaAabbs 等長契約未強制 → fail-loud 化で UB 根絶 |
| AE-2 | 観 | ディスパッチ判定の機械依存性を契約化 |
| AE-3 | 観 | doc 過剰主張の訂正 |
| AF-1 | 低 | azdo: overhead_saved の count==0 過小計上根治 |
| AF-2/AF-3 | 観 | doc 過剰主張訂正 + mask 等長の入口強制 |
| AG-1/AG-2 | 観 | diff_mesh: 水平隣接伝播は呼出側責務 / MeshPatch 非検証の明文化 |
| AH-1 | 中 | tick_render_split: consume_ticks の NaN dt 凍結根治 |
| AI-1 | 高 | soa_layout: alloc_aligned_64 が一度もアライン達成していなかった (ゼロデイ) → Aligned64 根治 |
| AI-2 | 中 | xzy_index 範囲外 z ≥ sx 静寂衝突 → fail-loud 化 |
| AI-3 | 中 | EntitySoa 7 配列等長の強制 |
| AJ-1 | 中 | entity_tick_lod: 非有限 dist の RenderOnly 静寂飢餓 → fail-loud 根治 |
| AJ-2 | 観 | hashed 経路の hash ⊇ positions 前提明文化 |
| AK-1 | 中 | execute_indirect: IndirectBatcher::push 上限静寂 drop → fail-loud 根治 |
| AK-2 | 中 | DrawCompactor capacity 使い捨て → 契約保持+強制 |
| AK-3 | 観 | sort 二系統の使い分け明文化 |
| AL-1 | 中 | billboard_lod: NaN dist/帯 NaN の全 Culled 静寂着地 → fail-loud 根治 |
| AL-2 | 観 | make_billboard 単位直交基底前提明文化 |
| AM-1 | 中 | texture_atlas_virtual: evict_lru は LRU ですらなく非決定 → 根治 (wave 40 AP-1 で真の LRU 化) |
| AM-2 | 低 | capacity_pages=0 の 0 除算 → 契約拒否 |
| AM-3 | 中 | request_tiles タイル座標無検査 → fail-loud 化 |
| AN-1 | 中 | gl33_compat: InstanceBuffer::from_pod の ZST 受理 → fail-loud 化 |
| AN-2 | 低 | TimerQueryCompat::end の 0.0 観測混入根治 |
| AN-3 | 低 | samples 上限維持 Vec::remove(0) O(n) → VecDeque 化 |
| AN-4 | 観 | total_triangles 端数切捨て明記 |
| AO-1 | 高 | simd_frustum.wgsl 実カーネル化 (1 box=1 thread、CPU 参照と数学的等価) |
| AO-2 | 観 | bit 厳密性の限界を一次情報調査+実証で正直化 |
| AP-1 | 高 | evict_lru を真の LRU (アクセス論理時刻) に根治 (AM-1 の closing) |
| AP-2 | 観 | 消費者影響分析 (full_graph_wiring) |
| AQ-1 | 中 | lbvh: build 契約の fail-loud 化 ×3 |
| AQ-2 | 観 | 暗黙定数 2 件の存在理由明文化 |
| AQ-3 | 中 | 単位法線契約明文化 + run 包含球の丸め収縮根治 |

## D 期: 厳密ピン/closing 期 (wave 42-71)

| ID | 度 | 概要 |
| :--- | :- | :--- |
| AR-1 | 高 | sparse_texture: request_with_eviction 追加 (退避キー通知欠落根治) |
| AR-2 | 観 | dense-slot 帰納不変条件明文化 + 死フィールド削除 |
| AR-3 | 高 | mip_for_distance NaN 静寂着地根治 + 式意味論正直化 |
| AR-4 | 基 | sparse_texture.wgsl 実カーネル化 (closing) |
| AS-1 | 高 | mip_streaming: 単一テクスチャ超過の静寂予算破壊 → fail-loud 化 |
| AS-2 | 高 | hysteresis 無検証 pub フィールドの静寂キャスト破壊根治 |
| AS-3 | 中 | set_size 常駐中サイズ不更新の帳簿ドリフト根治 |
| AS-4 | 中 | 未登録 id request の静寂 no-op → fail-loud 化 |
| AS-5 | 中 | 真の O(1) 侵入型 LRU 化 (毎要求 O(n) 根治) |
| AS-6 | 観 | mip_streaming.wgsl「シェーダ不要」正当マーカー誠実化 |
| AT-1 | 基 | mesh_compactor CandidateWire 48B — WGSL 配置規則の厳密再現 |
| AT-2 | 基 | ParamsWire 128B + policy.z の f32 輸送契約 |
| AU-1 | 高 | azdo: batcher 実配線 (MDI コマンドリスト準備 closing) |
| AU-2 | 観 | vbo_pool 未配線は責務分離として明文化 |
| AV-1 | 高 | bindless: pack_handle 静寂切捨てマスク → fail-loud 化 |
| AV-2 | 観 | 死コード Vec3/Vec4 削除 |
| AW-1 | 観 | meshlet_cone 可視判定の数学的正当性検証・明文化 |
| AW-2 | 高 | NaN 法線の静寂な永久カリング根治 |
| AW-3 | 高 | roundoff による sqrt(負) NaN → 永久カリング根治 |
| AX-1 | 高 | intern_pool: 二重 release の release build 無防備 (ゼロデイ級) 根治 |
| AX-2 | 高 | quantize の i32 飽和誤共有根治 |
| AY-1 | 高 | string_intern: InlineStr from_utf8_unchecked 前提を pub 経路で強制 |
| AY-2 | 低 | CompactSymbolTable 決定性の機械ピン |
| AZ-1 | 中 | texture_budget: mipmap_bias 非有限の静寂許容 → fail-loud 化 |
| AZ-3 | 中 | Astc4x4 bandwidth_factor 0.2→0.25 (事実誤り訂正) |
| BA-1 | 中 | zerocopy_cast: ZST 宛 `% 0` 偶発パニック → 明示契約化 |
| BA-2 | 観 | 消費者ゼロの「嘘ヘッダ」API 撤去 |
| BA-3 | 観 | unwrap_or_default 静寂退化の観測記録 (render_pipeline wave 引継ぎ) |
| BB-1 | 高 | half_vertex: f32_to_f16「RNE」doc 嘘 → 真の RNE 根治 |
| BB-2 | 高 | rsift-dx12 f32_to_f16_bits subnormal RNE 実バグ根治 |
| BB-3 | 観 | r10g10「簡易」f32_to_f16_bits の観測 (wave 53 引継ぎ) |
| BC-1 | 高 | 第 3 の f16 実装 (「簡易」版) 撤去・proven 実装へ一元化 |
| BC-2 | 中 | UNORM pack 切捨てバイアス → round-to-nearest |
| BC-3 | 中 | compress_vertex_stream zip 静寂打切り → 等長 assert |
| BC-4 | 高 | normal_oct 常時 0 スタブ → octahedral 完全実装 |
| BD-1 | 中 | 「分割バリアでオーバーラップ」doc 嘘 → 全パラメータ API 化 |
| BD-2 | 中 | TextureBarrier::validate を全構築経路に強制 (一次情報適合) |
| BD-3 | 低 | uav_barrier subresource 暗黙 0 → 明示引数化 |
| BE-1 | 高 | packed4: pack 契約 debug_assert 限定 → 実データ入口 new() assert 化 |
| BE-2 | 中 | face_index `_ => 5` 静寂誤分類 → 軸一意 assert |
| BE-3 | 低 | WGSL ミラー語彙表記一致ピン + 誤誘導定数 PULL_CHUNK_VOXELS=32 撤去 |
| BF-1 | 高 | pull_mesh: pub is_empty フィールド → is_empty() 導出メソッド化 (第二真実源根絶) |
| BF-2 | 中 | pull_vertex_count `as u32 * 6` 静寂 wrap → assert 化 |
| BG-1 | C | svo::trace が意味論スタブだった → 厳密最近接走査の実装に根治 |
| BG-2 | 中 | 到達不能 dominant 葉 (+ id≥16 静寂消失ハザード) 構造根絶 |
| BG-2b | 低 | from_column 静寂切捨て → fail-loud 化 + WGSL 走査語彙ピン (BG-3) |
| BH-1 | 高 | gpu_vertex_pull: PullSsboPool write-only 帳簿 (+潜伏バグ 2 件) 撤去 |
| BH-2 | 高 | GpuVertexPullEngine/PullEngineHandle 構築ゼロ複製実装撤去 |
| BH-3 | 観 | 生存側確認記録 |
| BI-A | 高 | chunk_mesh: is_empty 第二真実源根絶 (BF-1 統一) |
| BI-B | 中 | encode() NaN→0 静寂テレポート遮断 |
| BI-C | 中 | demo フォールバック表現範囲違反修復 |
| BI-D | 観 | ドキュメント誠実化 + TOTAL_CHUNKS/TOTAL_VERTICES 判断記録 |
| BJ-1 | 高 | voxel_cone_tracing: ConeRay 非有限で発散/静寂黒出力 → validate_contract 化 |
| BJ-2 | 低 | WGSL ミラー語彙ピン + alpha ガード副次記録 |
| BK-1 | 高 | section_rle: 破損 RLE の静寂 air 注入 → count 総和検査 fail-loud 化 |
| BK-2 | 高 | from_bytes 厳格契約 (長さ完全一致 + Σcount 検査) |
| BK-3 | 観 | 消費ゼロ語彙 2 件撤去 + required_section_indices ×8 記録 (BK-4) |
| BL-1 | C | noise_upsample: perlin3d_dense が全 voxel 定数 0.5 (live worldgen ゼロデイ) 根治 |
| BL-2 | 低 | density_to_block 到達不能 `.max(1)` 撤去 |
| BL-3 | 中 | benchmark_upsample 粗サンプル計数の不整合修復 |
| BM-1 | 中高 | frame_reference: covered_px/avg_lum doc 二重乖離の根源修正 |
| BM-2/BM-3/BM-4 | 低 | ACES+sRGB/fsr1 三連鎖 WGSL 表記ピン・write_bmp 全バイト厳密ピン新設 |
| BN-2 | 中 | fsr1: easu_reconstruct 死引数 `_c` 撤去 (API 正直化) |
| BN-3a | 観 | Fsr1::sharpen 消費者追加 (消費者問題方針の初適用) |
| BN-3 | 低 | RCAS 入力 f64 放置の検算捕捉実績含む厳密ビットピン強化 |
| BN-4 | 観 | WGSL mix 演算順ドリフトを一次情報 (W3C CRD) で確定・doc 誠実化 |
| BO-1 | 中 | cas.wgsl 加算順 1 ulp 分岐 → 3 連鎖演算順の真の統一 |
| BO-2/BO-3 | 低 | 厳密ピン強化 |
| BO-4 | 観 | 公式 ffx_cas.h 全項目一致を一次情報確認 (変更なし判定) |
| BP-1 | 低 | aces_tonemap 厳密ビットピン新設 |
| BP-2 | 観 | Narkowicz 原著と完全一致を一次情報確認 (変更なし) |
| BP-3 | 観 | 負入力は saturate せず wrap の数学的性格を doc 誠実化 + 3連鎖点検 |
| BQ-1 | 低 | checkerboard: is_rendered wrapping_add 化 (debug panic 経路根絶) |
| BQ-2/BQ-3 | 低 | 厳密ビットピン + WGSL 語彙ピン新設 |
| BR-1 | 観 | wboit 簡約単一ターゲット形の明記 (挙動変更は BR 先例で見送り) |
| BR-2 | 中 | NaN depth 静寂最近接化ハザード → 入口 assert 遮断 |
| BR-3 | 低 | 厳密ビットピン新設 |
| BS-1 | 高 | temporal_mesh_diff: 責務外主張の撤回 (doc 嘘の誠実化) + diff_section 配線候補記録 |
| BS-2 | 中 | take_dirty のソート決定的化 (latent 非決定性遮断) |
| BS-3 | 低 | patch_for_block 契約厳格化 |
| BT-2 | 低 | vrs: build_mask tile=0 素朴 0 除算 panic 契約化 |
| BT-1/BT-3 | 低 | 厳密大なり境界固定 + WGSL 語彙ピン |
| BU-1 | 中 | lod_hybrid: NaN 距離の静寂最低詳細 (Far) 化ハズード遮断 |
| BU-2 | 低 | 空メッシュ簡略化早期 passthrough ピン |
| BU-3 | 観 | use_svo_encoding 命名乖離を真値表で確定 (リネーム見送り) |

## E 期: 横断契約クラス期 (wave 72-99)

| ID | 度 | 概要 |
| :--- | :- | :--- |
| BV-1 | 中 | vertex_cache_opt: 半端末尾 index 静寂 drop → fail-loud 契約化 (live 呼出も明示化) |
| BV-2 | 低 | cache_size=0 usize underflow → 入口契約明示 |
| BV-3 | 中 | heap tie-break 規則の厳密出力シーケンス 2 ピン |
| BV-4 | 観 | WGSL CacheScore 例示式を Rust 逐語ミラーへ改修 |
| BW-1 | 中 | world_column_store: 非有限カメラの静寂受理 → FFI 安全形 (panic 回避) の drop 拒否 |
| BW-2 | 観 | ingest 短配列 air 充填は契約外防御との判断記録 (両 FFI 呼出の長さ保証を一次確認) |
| BW-3 | 低 | 帳簿カウンタ差分会計化 (bulk ingest O(n²) 解消) + reconcile 公開 API 化 (消費者方針) |
| BW-4/BW-5 | 低 | mesh_origin 窓・look_at_rh/perspective_rh/mul4 厳密ピン |
| BX-1 | 中 | transform_svdag: 中間 pool ヒット誤タグ (入力→非 canonical) → D4 群合成 compose_tags で根治 |
| BX-2 | 低 | 列挙順決定性ピン (検算で自己誤り 2 件捕捉: octant シナリオ・R90² 表現) |
| BY-1 | 中 | vertex_pool: oversize 拒否時 stale slot → 「None ⇒ slot 無し」一貫化 + oversize_rejects |
| BY-2/BY-3 | 低 | refresh 契約・境界 (cap==fit)/floor ratio/generation wrapping 厳密ピン |
| BZ-1 | 高 | persistent_vbo_pool: index 確保失敗時の vertex 領域永久リーク → rollback_alloc 根治 |
| BZ-2 | 中 | oversize 拒否の stale slot (BY-1 同型・MDI 描画実消費) 一貫化 |
| BZ-3 | 低 | utilization() 0 容量 NaN → 0.0 guard (NaN=欠測哲学) |
| BZ-5 | 低 | rebuild_mdi が slot.mdi_index の真値を書き戻すよう修復 |
| BZ-6 | 低 | 容量配分 3:1 → 2:1 (all-quad 数学的最適、digest 不変を機械確認) |
| CA-1 | 中 | svdag: build_from_volume が root_id を更新しない stale metadata 根治 |
| CA-2 | 観 | 「empty/solid 基底」コメント不一致訂正 + de-facto 契約 (id 0 = 空リーフ) 明文化 |
| CA-3 | 低 | 厳密構築ピン 3 ケース (単一/全 solid/隣接 2 voxel) |
| CA-4 | 観 | 不変量設計の自己誤り捕捉 8 件目記録 (リーフ形分岐へ精緻化) |
| CB-1 | 中 | aokana: evaluate_visible_regions の HashMap 非決定反復 → sort 根治 (BS-2 型) |
| CB-2 | 低 | frustum 境界 (接触=可視)・insert 置換・region_size 64 規約の厳密ピン |
| CB-3 | 観 | doc 誠実化: Hi-Z/visibility buffer は現行未配線 (frustum のみ) と明記 |
| CC-1 | 中 | visibility_buffer: pack_ids 16bit 静寂切捨て → fail-loud 契約化 (AV-1/BE-1 同型) |
| CC-2 | 低 | compute_barycentrics 縮退重心フォールバックの誠実化 + 分母 2 冪の厳密有理ピン |
| CC-3 | 高 | WGSL ジェネレータが虚構レイアウトのスタブ → 真の 12B 完全 resolve + CPU ミラー 3 連鎖 + naga 検証 |
| CD-1 | 高 | hzb_2d: temporal streak の i8 アンダーフロー (129 フレーム連続遮蔽で debug panic / release wrap) → [-4,1] clamp 化 |
| CD-2 | 高 | temporal キー列衝突 ((x,z) 2D・i32 切捨て) で断面共有ストリーク (4 フレーム意味論破壊・フリッカー穴方向) → min 3 成分 to_bits 厳密恒等キー |
| CD-3 | 高 | Hi-Z 深度値の近/遠逆転 (occluder が最近値 raster = 「tile 全域被覆」保証が偽 = false hole) → 最遠値 raster + 厳密最近値 test + 単調性による自己遮蔽不能の証明 |
| CD-4 | 高 | 射影の fov/aspect 因子欠落 (fov=1.0,16:9 の偶然補正) + 回転を無視した軸並行仮定 → 8 角厳密射影 + 凸包 scanline で完全内包 texel のみ raster |
| CD-5 | 中 | mip 奇数幅の floor 除算で最終列/行がピラミッドに伝播しない死亡列 → ceil 除算 (Hi-Z 標準) |
| CD-6 | 中 | moved_significantly が y 移動・fov/aspect 変化を無視 (鉛直テレポート/ズームで stale ピラミッド) → 3D 距離 + Δ射影検出 |
| CD-7 | 中 | 非有限カメラ/非正規ボックスの NaN 伝播偶然頼み → 明示 drop 拒否 (無記録・保守全可視・バッファ無汚染) |
| CD-8 | 低 | build_pyramid 全段の src.clone() アロケーション → split_at_mut 借用分割 (出力 bit 同一) |
| CD-9 | 低 | temporal HashMap 無制限成長 → 16384 エントリ上限・超過で保守全クリア |
| CE-1 | 低 | cpu_occlusion: 固定カメラ版 cull_boxes の消費者ゼロ実測 → 診断再現経路として契約明文化 (削除理由にせず with_camera 推奨) |
| CE-2 | 低 | enabled=false 経路の委譲透過 + 帳簿不変ピン (wrapper/Hzb2D bitexact) |
| CE-3 | 低 | fxaa: Rust フィールドが WGSL ハードコード (1/256・0.166667) に非伝播である罠の契約明文化 + WGSL 文字列 parse での bit 一致語彙ピン |
| CE-4 | 低 | shade 厳密 bit ピン 3 件 (t=0.5/(1+1/6)=3/7 級 0x3EDB6DB3、untouched は入力 bit 完全コピー) + luma 誤係数注入への検出力確認 |
| CE-5 | 観 | WGSL n/s ラベル逆転は abs 比較のみ使用で振る舞い等価との判断記録 + コメント誠実化 |
| CE-6 | 観 | shade 非有限画素の扱いを未規定と doc 明記 (f32::min/max の NaN 脱落・WGSL indeterminate・全面停止回避方針) |
| CF-1 | C | occlusion_complete: project の行列規約が本番と転置不一致 (平行移動が w 語に化けた破壊的射影、p x M 規約 + w>1e-6 必須 + ndc_z 直接返却で根治) |
| CF-2 | 高 | rasterize 深度の近/遠逆転 (texel 被覆保証が偽 = potential false hole、CD-3 同型) → 最遠 raster + 厳密最近 test + strict 大なり (等値境界で自己遮蔽不可能) |
| CF-3 | 高 | 三角形判定が complete-dead 級 (w_i ≡ -λ_i へのトレランス判定で採用領域 = v2 角相対幅 1e-4 の楔のみ) → 3 直接辺関数 + 同符号性で巻き向き不変化 |
| CF-4 | 高 | rasterize_rect が完全画面外 rect を clamp で端列/端行に誤記述 (辺縁遮蔽の偽装作り得) → clamp 前 early-out |
| CF-5 | 中 | band 検査の raster 適用は眼前壁級 occluder を全沉默 (非保守) → project (test 用 band 付) / project_screen (raster 用) 分割 + 8 角凸包完全内包 scanline + 背面/far 棄却 |
| CF-6 | 中 | hysteresis: && 合成の非過早性証明 (衝突は遅延のみ)・HashMap 16384 cap・frames=0 の max(1) 丸めをピン |
| CF-7 | 観 | doc 誠実化: 「SWAR」命名はスカラー実装・lock-free 表現撤回・Halton ジッター現行未適用 (設計予備) の明記 |
| CF-8 | 低 | 厳密ピン体系 (Halton f32 bits・本番行列 nz bits 0x3F7F3994 系・凸包 3712 texel・衝突遅延 +2 フレーム証明) |
| CG-1 | 高 | full_graph_wiring: extract_frustum_planes の行/列転置 (CF-1 同型) — 本番 p×M で 6 面中 5 面破壊・正面点すら棄却・LBVH/SIMD 相互検証はコモンモード不発 → 列ベース根治 + 射影一致 14 点 + bit テーブルピン |
| CG-2 | 中 | 「実 draw indices」注記 vs 連番虚偽 (ACMR オーダー不変・採用経路不存在) → コメント誠実化 (BV 引継ぎ項目解決、実 topology 配線は将来課題) |
| CG-3 | 観 | subsystems_active=60 は実計数でなく仕様定数と doc 明文化 |
| CG-4 | 観 | done() no-op (旧 doc「実効果参照」虚偽) 誠実化 |
| CG-5 | 観 | JobSystem ブロックは no-op 負荷分散実演 (「メトリクス集計」虚偽) 誠実化 |
| CG-6 | 観 | FRB ブロックは組立実演+本数計測のみ (「GPU 入力へ実変換」虚偽・FRB に CPU 入力 API 不在実測) + frb_billboards doc 訂正 |
| CG-7 | 低 | PSO 問合せ/挿入キー不一致で get が常時ミス (miss 100% のでたらめ計器) → 同一キー統一で真のキャッシュ挙動へ |
| CG-8 | 低 | GTAO 標本が「不透明率由来」を偽った len 直読み (filter(|_| true)) → 実パレット不透明率の真の実測へ (出力読捨てで影響ゼロ) |
| CH-1 | 中 | FSR1 EASU へのフランケン 4 近傍 (前/現フレームのチャンネル混在色) → 定数色不変性実演 (厳密恒等の数学証明 + fsr1 flat ピン厳密 bit 化で財産化) |
| CH-2 | 低 | CAS/FXAA の異ステージ近傍混在 (中心=sharpened・近傍=bloomed 等) → 同一ステージ定数近傍統一 (CAS ≈恒等 1ulp 級 / FXAA untouched 厳密 bit コピー) |
| CH-3 | 低 | nanite/more_culling の FOV 70° ハードコード (真値経路不在のゲス値) → FrameWiringInputs.camera_fov_y 追加で render_pipeline 実カメラから真値配線 (max(0.1) clamp は from_camera 規約整合) |
| CH-4 | 観 | IBL「ドーム」は y=max(0.05) 水平リング + SH 重み 0.03 は 4π/N 求積でなく ad-hoc — コメント誠実化 |
| CH-5 | 観 | VCT cone の dir.y=abs ヒューリスティクス (sky-visibility プローブ近似) 誠実化 |
| CH-6 | 観 | exposure histogram 境界 (1e-4〜0.4, p10/95) の変更なし判断記録 |
| CI-1 | 中 | gigabuffer 圧迫時の二重虚偽: 「最古 (LRU)」が `HashMap::keys().next()` (ハッシュ順任意要素・非決定的) + 「実解放して再試行」の再試行コード不存在 (当該確保の静寂欠落) → gb_order FIFO 挿入順キュー + gb_store/gb_alloc_or_evict 抽出 (真 FIFO 最古の実解放 + 単一 retry + 失敗は潔く None の bounded 挙動) + gb_eviction テスト (置換順位不変/oversize 被害者 1 件/retry 成功径路) + adversarial 2 系統 (no-retry/LIFO いずれも検出) |
| CI-2 | 観 | time_slice の patch 駆動キー `packed & 0xFFF` (cz 下位 2bit+sy 10bit 混在) は per-block 真差分でなく決定的駆動値 — 誠実注記 (契約 <4096 は構造的常時保証) |
| CJ-1 | 中 | render_pipeline frame() の pull キャッシュヒット/flora LOD box 経路が wiring 入力 (chunk_keys等) から欠落 → overdraw_order 由来 wiring_priority で欠落列が unwrap_or(usize::MAX) 常時最劣後ソート (cache-warm 近接列の系統的不当降格 + 実描画列と wiring 対象の二重基準) → wiring_only 側車ベクトルで併合 (draw_index_count は 8B 固定から厳密導出) |
| CJ-2 | 低 | verdict Occluded 腕が frame() (通過) と build_chunk_if_visible (skip) で真逆の潜在乖離 (現行 producer 非送出=BK 設計のため非発現) → frame() も skip 側に統一 (現挙動不変) + Occluded 非送出ピン追加 |
| CJ-3 | 中 | ingest_world_column が diff_mesh にカメラ帯 mid_y±16 をマークし、別帯域インジェストの diff 追跡が完全欠落 + カメラ帯誤ダーティ化 → note_ingested_sections 抽出でインジェスト帯域の各セクション中心ブロック (sy*16+8) を正確に 1 セクション/回マーク |
| CJ-4 | 低 | wiring 供給 SVO が `svo_cache.values().next()` (HashMap 反復順任意要素、CI-1 同型の潜在非決定性) → 最小キー決定論選択 svo_for_wiring + 借用衝突回避の所有クローン移譲 (ptr::eq ピン) |
| CJ-5 | 観 | build_chunk の debug_assert は chunk_mesh 型レベル const assert に完全包含される定数比較 (頂数 O(n) 無駄) → 除去 (保証一元化) |
| CJ-6 | 観 | build 予算 (cache ヒットも 1 消費) と chunks_built (ヒット非計上) の語彙非対称はフレーム時間経済として意図的 → 誠実注記で現挙動固定 |
| CK-1 | 中 | 派生キャッシュがワールド prune/再インジェストに追随せず: svo_cache の stale SVO が除去列・旧地形として最小キー選択 (CJ-4) 経由で VCT プローブへ供給され続ける + pull_gen_cache 滞留 → prune_derived_caches (world prune と同語彙 Chebyshev・境界含む) + invalidate_derived_for_column 抽出と pub wrapper 配線、PullGenerationCache.prune_outside 追加 |
| CK-2 | 低 | HZB/CPU occluder AABB が固定 y 0..64 で live メッシュ帯 (origin.y 基点) と 48 ブロックずれ、HZB 統計が誤帯域で系統計測 → hzb_boxes_for 抽出 + mesh_origin 整合帯 (厳密ピン [16,48,32]-[32,112,48]) |
| CK-3 | 観 | verts_per は先頭メッシュのみ代表値 (eco 近似) 誠実注記 |
| CK-4 | 観 | live→デモ切替の 1 フレーム速度スパイク (min(40) 有界) 現挙動固定の明記 (BR-1) |
| CK-5 | 観 | pull モード時 greedy メッシュのプール二重構築・供給 (DX12 未消費) 設計明記 |
| CL-1 | 観 | 「SIMD 8-Wide AVX2/SWAR パケット DDA」主張 vs 実スカラー逐次 (CF-7 同型) — docstring 誠実化 |
| CL-2 | 中 | FastEntityCuller 移動検知が再評価要求を消失 (最長 10 tick vis ラグ、EntityCuller の due\|\|moved 非対称) → moved 分岐で last_eval_tick 巻戻し due 強制 + due カレンダ厳密ピン |
| CL-3 | 中 | ChunkBucketGate 保守 skip が即 false で境界実体を点滅化 (宣言の遅延適用と矛盾) → visibility 保持+bitmask 反映 (初版は continue でビット未設定=実効無修正をテスト赤が捕捉、16 件目) |
| CL-4 | 低 | FOV フィルタ cos 0.35 ハードコード → fov_cos_min フィールド化 (CH-3 同型、既定不変+静的構成前提注記、真値配線は consumer FOV 公開待ち) |
| CL-5 | 中 | CullStats.total が invalid 尾スロット込みで統計歪曲 → valid 数へ根治 (skipped_far=far+gate 合流語彙はピンで仕様固定) |
| CL-7 | 観 | ray_unblocked guard>256 → true は「不確実=可視側」保守方向の誠実注記 (既定 max_distance 経路では構造的不発) |
| CM-1 | 中 | pack_stem Composite(n≥3) が "composite" に潰れ Composite(0) と衝突 (OptiFine `composite1..99` 命名と不整合の pub API 潜在欠陥) → Cow 化で全番号厳密合成・1..=99 全値ピン |
| CM-2 | 中 | resolve_pack_root が ".zip" 必須のため discover_shaderpacks の stem 出力から zip を解決不能 (往復破綻+静寂 Eco fallback+Ready 誤報) → stem/明示の両受理化+pack 不存在 warn |
| CM-3 | 中 | Eco fullscreen WGSL の uv 写像が上下反転 (wgpu NDC y=+1 ↔ texture v=0 に対し v=y*0.5+0.5 を使用、composite/final 恒等コピーの潜伏欠陥) → v=0.5-y*0.5 へ厳密化+写像式回帰ピン |
| CM-4 | 中 | zip deflate の実展開長が無制限 (cap は宣言値のみ) で宣言≠実ストリームの zip-bomb が巨大確保を強制 → `take(宣言+1)` で実展開長を宣言値に縛り fail-closed 化 |
| CM-5 | 低 | discover_shaderpacks 3 点: zip 拡張子 case-sensitive (`.ZIP` 不発)・ドット入り dir 名が file_stem で欠落・自前展開キャッシュ (.rsift_extracted) が候補混入 → case-insensitive 化/file_name 化/隠し名 skip |
| CM-6 | 低 | dispatch_frame_passes が uniform を即破棄し下流エンコーダへ転送不能 → last_uniforms 保持 (消費者追加思想に基づく供給面確保、f32 bit 等価ピン) |
| CM-7 | 観 | for_tier は Full を返さない (PerformanceTier 4 値網羅・Full は手動 opt-in、低スペック優先と一致) / plan 簡略 (deferred・properties・dimension 未実装) は module doc 記載 scope と一致のため設計固定 |
| CN-1 | 重大 | KTX2 レベルデータ/インデックスの双方向逆転 (spec: データ最小 mip 先頭・index entry i=mip i の規則に対し旧実装は mip0 先頭データ+rev index で index[0] が最小 mip を参照 → 取込側で mip ピラミッド全反転の機能実害) → offsets rev 走査+index 正順+mipPadding レベル間のみへ根治 (KTX2 spec・libktx validator・glTF PR 一次照合) |
| CN-2 | 重大 | DFD descriptorBlockSize を u32 誤直列化 (Khronos DFD ブロックヘッダは u16×4) → 宣言 28/実書込 26 (Python 再現) + BDFD 2B ずれ invalid DFD → khr_df.h 確定値の完全 BDFD (blockSize 40/DFD 44B) u16 直列化実装+宣言 vs 実バイト構造テスト恒久排除 |
| CN-3 | 中 | DFD model=2 誤値 (YUVSDA の意味) +「2=BT709」誤コメント + srgb_hint が両分岐 2 の死コード → BC7=134/BT709=1/TRANSFER_SRGB=2・LINEAR=1/CHANNEL_BC7_DATA=0/dims N-1/plane0=16/bitLength=127 の正写像 (khr_df.h・libktx 実 dump 一次照合) |
| CN-4 | 低 | 破損期残骸 `// ZZPROBE_MARK` + 二重空行 2 箇所 + `let _ = i;` 除去・末尾改行 (KTX2 writer 周辺清掃) |
| CN-5 | 観 | anchor swap 厳密安全性の証明 (加重対称 w[15-i]=64-w[i]+加算可換で idx0≤7 保証)、A_WEIGHT4=round(i*64/15) 全値機械検算 (MS BC7 公式表一致)、量子化端点の最終 index 再割当は品質余地として棚卸し |
| CO-1 | 中 | dispatch_adaptive_culling GPU 経路の返り値が提出総数 (可視数ではない) かつスライス不変である契約欠落 (CPU 経路=可視数との混同誘発) → doc 明文化 (CF-7 型の誠実化、挙動不変) |
| CO-2 | 低 | default_frustum の doc「perspective matrix decomposition」虚偽 (実体は固定軸平行ボックス・camera_pos 未反映) → 誠実注記へ訂正 |
| CO-3 | 中 | GpuBufferPool exact-fit 成長が +1 増減で全再確保し得る設計目的違反 → pool_capacity pure fn 抽出 (min 64/next_power_of_two) 償還成長化+slab 表厳密ピン |
| CO-4 | 低 | frustum.chunk_count≠boxes.len の呼出誤りが stale tail cull を誘発 → debug_assert 契約ピン / trace「zero alloc」虚偽 →(pooled buffers) 誠実化 |
| CO-5 | 観 | ChunkBox WGSL storage オフセット (0/12/16/28/32/36→48B) と Rust repr(C) の完全一致を Python 検算+offset_of! 6 点機械ピン / FrustumData camera_pos・hzb_enabled のシェーダ非参照は CPU 一元化の設計固定 |
| CP-1 | 低 | GUI 行 char 幅の三重非整合 (枠線 76 に対し toggle 75・slider 64 で右壁ずれの視認実害、旧 slider パニックとは別欠陥) → GUI_ROW_CHARS 契約 const+行ビルダ pure 抽出 (label 43/42)+枠線一致機械ピン |
| CP-2 | 低 | biome blend UI 下限 1 が vanilla OFF (0) を選択不能に (低スペ最重視で最軽量 OFF 欠落の語彙欠陥) → BIOME_BLEND_MIN/MAX (0,7) pub const 化+OFF=左端・pos16 幾何ピン |
| CP-3 | 低 | open_global_settings が poisoned mutex を静寂無視 (BW-1 系 fail-loud 文化に反) → debug! 通知+into_inner() 復元で開き切る慣用句へ |
| CP-4 | 観 | shader_profile 4 値表は PerformanceTier 4 値と整合 (High 時 speed_first 2 択)、probe_and_cache/hardware 混在は OnceLock 共有で実害なし、slider 退化語彙は既ピンと設計確認 |
| CQ-1 | 中 | fsr_rcas 外周 4 近傍の範囲外 textureLoad が WGSL 規格上「不定値」(ゼロ保証なし、一次: wgsl/index.bs §textureLoad 17,925 行) でベンダ非決定的 → WGSL を端画素 clamp へ根治 + CPU ミラーの「OOB=0」規約を clamp 3連鎖へ同時根治 (平坦領域を厳密恒等化、ハロー値 126/132 を機械再導出で廃止) |
| CQ-2 | 中 | validate_dims ガード 1 オフ (full_w=2^30 で full_w·4=2^32 が u32 ラップする境界を受理+ceil 上げ幅未考慮) → MAX_ROW_SAFE_FULL_W=2^30−64 const 厳密化 (Python: padded=0xFFFFFF00/+1 で 2^32、境界 ±1 テスト反転+行バイト厳密ピン) |
| CQ-3 | 低 | FSR1 サンプラ address_mode が Default 暗黙 (EASU 外周端画素規約が既定値依存) → ClampToEdge を u/v/w 明示固定 (ミラー px() clamp と規約整合露出) |
| CQ-4 | 低 | frame_reference「OOB textureLoad=0、WGSL 準拠」コメントが規格文言に反する虚偽 → clamp 規約の正確な記述へ訂正 (仕様誤読の伝播遮断) |
| CQ-5 | 観 | Params uniform (24B @0/8/16/20) の vec2 16B アライン懸念は RequiredAlignOf 表照合で誤検出確定 (16 化強制は array stride/struct 間隔のみ) — 変更なし |
| CQ-6 | 観 | EASU/RCAS バインド群分割・inter view 寿命・dispatch ガード・readback 256B アライン・draw_calls=2・DEFAULT_SHARPNESS 双方向ピンを仕様通り確認 — 変更なし |
| CR-1 | 中 | GpuArena::alloc のアライン繰上げ u64 オーバーフロー (巨大 size がラップ→小 need 化→誤認割当) → checked_add で確保不能 None 化 (境界 4 点ピン) |
| CR-2 | 低 | GpuArena::free の size 不一致で used_bytes 会計静寂破壊の窓 → debug_assert(seg.size == h.size) 追加 |
| CR-3 | 低 | gen カウンタの死に状態 → generation() pub accessor 消費者配線 + 追従ピン |
| CR-4 | 低 | HazardQueue::reclaimed_count 別名削除 (reclaim 直接呼出し統一) + best-fit/分割/併合正準レイアウト Python 検算ピン |
| CR-5 | 低 | wave 92 残留の GUI_ROW_CHARS 未利用警告 → label 幅を契約から導出 (TOGGLE/SLIDER_LABEL_CHARS) で真 consumer 化・単一真実源 (警告 28→27) |
| CR-6 | 観 | SPSC メモリ順序正準性・free 二重 merge index dance・free_by_size bucket 整合の仕様一致確認 + gpu_culling DeviceExt 警告は 3d601f4 初版由来と確定 (棚卸し管理へ) |
| CS-1 | 中 | sort_nearest_first 距離² i32 オーバーフロー (|d|≤46341 ラップ、i64 でも越境) → i128 厳密化+巨大座標 4 点順序ピン (テスト赤 2 捕捉経由) |
| CS-2 | 低 | FaceEmitMask 0.15 魔数 → FACE_MASK_AXIS_THRESHOLD pub const (文献照合)+軸 5 ケース厳密 bit ピン |
| CS-3 | 低 | SectionOccupancy doc 残留語彙乖離 («layers[y] is unused» 等英語解説) → xz/y_any 実装同期 doc クリーン |
| CS-4 | 観 | FaceEmitMask::ALL 退化 (count_ones()<3) は単位ベクトル制約で下限=3 の数学証明 → 到達不能だが防衛保持・文書化 |
| CS-5 | 観 | solid_interior_cull 範囲 1..S-1 恰好確認 (shell=1352 ピン)・render_pipeline filter→edit 順既良・PullGenerationCache 語彙 wave 87 一致 |
| CT-1 | 中 | job_system pending を POP 時 (実行開始) 減算 → wait_idle 早期復帰で完了保証破綻 → worker_loop/drain_one 双経路で実行完了後減算へ (語彙=in-flight+queued 未終了件数に確定) |
| CT-2 | 中 | steal 横取り後の一括 Background 強制化 → FrameCritical 静寂背景化 → Priority::from_raw+Vec<(Priority,Job)> 転送・requeue_stolen pure 分離で優先度属性保持 (テスト赤=自己誤り捕捉 19 件目: 半切捨て fall-through 期待設計) |
| CT-3 | 中 | parallel_for(count=0) が count.max(1) で f(0..1) 1 件静寂実行 (要求 0 実行の逆セマ) → count==0 early return (消費者ガード非依存の API 契約化) |
| CT-4 | 観 | Priority 判別子 (FC=0/N=1/BG=2)↔バケット index 一対一を from_raw 内部規約化・wait_idle 排水参加+50µs ポーリング・Condvar 5ms timeout+Drop notify_all/join 終結手順 全て契約内 |
| CT-5 | 観 | 消費者 full_graph_wiring:498/504 (quads>0 ガード付 parallel_for + wait_idle) のみ — 0 判定語彙で挙動変化なし、CT-1 で完了保証が初めて真に成立 |
| CU-1 | 低 | EMA「約 16 フレームで半減期」誤記 (実=半減期 11.20/時定数 16.67、n=16 残存 37.16%) → EMA_ALPHA pub const+f64 bit ピン (n=11>8333, n=12<8333)・降段 EMA リセット 40000.0 bit ピン |
| CU-2 | 中 | GovernorConfig 無検証 (NaN/反転帯/0 閾で比較全不成立→カウンタ恒常リセットのガバナ静寂沈黙 or 毎フレーム発火病理) → validate() fail-loud+new 構築強制 |
| CU-3 | 低 | frames 未読フィールド → frames() pub accessor 消費者追加 (fps/期間算出の一次情報)+observe 毎厳密 +1 ピン |
| CU-4 | 低 | upshift doc「最後に落としたものから」虚偽 (実=静的逆優先度 RD 先返上) → 誠実化+タンパー順序ピン (10,RD,0)(15,SH,2)(20,SH,1)(25,SH,0)、quality_score を外部 levels 改竄耐性 saturating 化 (u8 underflow panic 根絶) |
| CU-5 | 観 | render_scale_pct l.min(5) 防御・降段無クールダウン (UE 一次情報「即座に」一致)・upshift 非 EMA リセット (good 収束済で設計通り)・cooldown 実効語彙 (抑制 F+1..F+cd-1、F+cd 再開 — 5 間隔列で実証) |
| CW-1 | 中 | mesh_cache zstd::decode_all 展開無制限 (数 KB → GB 展開ボム経路) → 正当最大 ≈40.7MiB Python 検算の 1.6 倍余裕 64MiB cap (Decoder+take) で fail-loud |
| CW-2 | 中 | put 直接 File::create+write_all 静寂破棄 (部分書込でも true 偽装・ログ虚偽・クロスプロセス裂け読み) → tmp 全量検証+rename 原子置換+失敗 warn/false/tmp 削除 |
| CW-3 | 低 | encode セクション数 (len as u16) 静寂縮退 (decode 誤読) → len>u16::MAX は fail-loud Err (境界 65535 受理/65536 拒否ピン) |
| CW-4 | 観 | trailing garbage 受理 (wire 耐性・意図保持)・v1 は RLE 検証なし (非格納=設計)・32-bit は対象外・sync_all なしは miss 治癒で許容・stats tuple 語彙消費者一致 |
| CW-5 | 観 | key_path インジェクション非成立・invalidate prefix 衝突ピン済・put は LOD simplify 済保存+tier drift は render_pipeline 第 3 部へ・ast-grep 横断: write_all 静寂型 0 件・decode_all 残存 region_zstd.rs:107/170 起票 |
| CX-1 | 中 | region_zstd build_file セクタ数 `& 0xFF` 静寂ラップ (256 超で読不能ファイル返却、Stored >1MiB で到達可) → build_file_checked Result+build_file expect (境界 1,044,475/1,044,476 ピン) |
| CX-2 | 中 | scan_file location スパン無検証 (ヘッダ重複・file 境界超過を正常受理) → offset≥2 かつ span≤file.len の InvalidData fail-loud 検査 |
| CX-3 | 低 | get_chunk/stats 無制限 decode_all (CW-5 起票) → private 自己生成データ限定で CW-1 disk 経路と危険度相違・cap 不導入+provenance doc 明文化 (誠実格下げ) |
| CX-4 | 観 | build_file 冪等・offset 24-bit/len u32 非到達コメント照合・auto() 出典 (ZFS 系) 一致 |
| CX-5 | 観 | timestamps epoch scaffold doc 済・Location default=absent 一貫・put 二重書き leak なし |
| CY-1 | 中 | nanite_clusters clusterize indices.len()/3 切捨てで末尾 1-2 index 静寂 drop (wave 72 と同種) → assert! fail-loud (契約 doc・空/1 三角受理ピン) |
| CY-2 | 中 | next_seed 素数刻み走査の数学的破綻: gcd(31,len)≠1 (len≡0 mod 31) で剰余列が len/31 位置 (3%) のみ巡回 → 残り三角が静寂消失 (wiring 実経路 identity index で直撃 62→2 クラスタ、Python 厳密シム検証) → step を len と互いに素な最小奇数へ (完全置換化)、gcd=1 の既存入力で走査順完全一致立証 |
| CY-3 | 中 | clusterize 非有限頂点無検査: radius が max-scan ガードで 0.0 へ・error が .max(0.0) 非伝播で +0.0 へ静寂潰れ should_draw 常時描画へ誤分類 (実機検証) → 入口 assert! 遮断 + vertex_offset 死に計算除去・scaffold doc 誠実化 |
| CY-4 | 中 | 親鎖未配線の構造嘘: parent_of 計算後 `let _=` 破棄で parent 恒 u32::MAX (doc の誤差ツリー不成立) → parent=partner 実配線 (深さ 1)・stride/parent_of 死にコード除去・level scaffold 注記 |
| CY-5 | 中 | cluster_should_draw doc 式に proj_factor 省略+「誤差う」誤記+clamp 未記載・f32::max NaN 非伝播で NaN cam→d=1.0 マスクの静寂誤カリング → 有限性 assert! (cam/proj>0/eps/error/sphere)+doc 完全書換 (厳密不等号・clamp 2 所・error=0 常時採用明記) |
| CY-6 | 低 | error_metric_nonzero_for_parents が error>=0.0 恒真 assert (nonzero を何も検査せず) → meshlets[0].error>0+any(>0) 実質化 |
| CY-7 | 低 | max3(a,b) が 2 引数 max (命名嘘) → 除去し f32::max 二項直接化 |
| DA-1 | 中 | distant_lod downsample 奇数寸法で縁カラム静寂脱落 (doc と矛盾) → 偶数 fail-loud + samples.len()==w*h 構造不変量明示 |
| DA-2 | 中 | 上面 quad 角 Y スワップ ([y00,y10,y11,y01] → 正 [y00,y01,y11,y10]): 傾斜全セルで上面ねじれの静寂幾何破壊 (flat テスト不可視) → 位置対応訂正+厳密ピン |
| DA-3 | 中 | mkv origin 設計破綻 (origin≠0 で全頂点 0 平面崩壊) + f32 2^24 精度壁 + u16 静寂 clamp → 整数ドメイン ローカル pack 再設計 + fail-loud 契約 (lod<16/extent/セル数 u32) |
| DA-4 | 中 | lod_for_distance NaN が全比較 false で最遠 LOD 5 静寂逃走 → 有限・非負 assert (wave 71 BU-1 同型)+境界 12 点ピン |
| DA-5 | 低 | Y 量子化 trunc で平均 0.5m 下落バイアス → f32::round 最近接化 (12.5→13/8.5→9 ピン) |
| DA-6 | 低 | merge_4 タイ「先着」コメント虚偽 (実は max_by_key 仕様の後勝ち) → 訂正+0xBBBB 厳密ピン+意味論 6 ピン |
| DA-7 | 低 | doc 群: 「小LODs」U+00E3 文字化け混入・skirt「4-8m」虚偽 (実 [1,4])・_lod 死引数・縁複製重み・n==0 不到達・half 命名嘘 訂正/除去 |
| DB-1 | 低 | palette_pack doc「322 種超 16bit フォールバック」未実装虚偽 (vanilla 9bit 設計の化石) → 訂正 (bits≤12 完結の数学的証明+ピン) |
| DB-2 | 低 | doc「最大 ~1/4」虚偽方向: 全 4096 相異で 14,760B = 1.8018 倍膨張 → worst-case 含む誠実表訂正 + 実測 bit ピン |
| DB-3 | 低 | memory_bytes が rev HashMap 非計上で過小表示 → 永続層定義値と doc 明確化 |
| DB-4 | 低 | set 単一値同値上書きで 512B 静寂確保 (10B→522B no-op 不変で) → 早期復帰 no-op 化 + 到達不能死にコード除去 |
| DB-5 | 観 | get/set 座標 debug_assert: release 範囲外は (z+1,0) エイリアス → 据置+doc 契約 |
| DB-6 | 観 | 語跨ぎなし pack (MC 1.16+ 同型)・needed_bits 境界ピン・read OOB loud 確認 |
| DB-7 | 観 | パレット単調増加 doc 追認・ratio 空列 1.0 定義注記・stats_for 一次情報性 |
| DC-1 | 高 | render_graph 依存構築が RAW のみ全順序ペアで実 Feather グラフが真のサイクル (translucent ↔ taa RMW 双方向) となり Kahn が 2 パス静寂脱落 (schedule=[0,1]/cost=6/barriers=2、弱テスト素通り) → forward-hazard 化 (i<j の RAW∪WAR∪WAW、構築上 DAG) + 完全ピン (schedule=[0,1,2,3]/cost=11/barriers 8) |
| DC-2 | 中 | Kahn 残留ノード静寂脱落を止める防御なし → assert_eq!(schedule.len(), n) fail-loud (旧 RAW-only 逆戻し adversarial で 5 テスト捕捉を実証) |
| DC-3 | 中 | barrier_count() が minimal `.max(1)` / non-minimal `len*2` の虚構メトリクス (実 8 を 16 と報告) → 両モード実本数正直化 (消費者 trace のみ) |
| DC-4 | 低 | RMW パス内 read→write 進行 (after_pass=自身) の未明文化 → Barrier doc「使用状態推移点列で外部発行バリア列ではない」明文化 |
| DC-5 | 観 | minimal の retain(from!=to)/dedup_by は構築上到達不能 (推移交互性) → 証明 doc 注記のうえ防御維持 |
| DC-6 | 観 | 空グラフ well-formed 性・reads 重複 no-op 性確認 → empty_graph テスト追加 (DC-2 assert 0==0 受理) |
| DC-7 | 観 | passes()/merge_ao/merge_water は消費者ゼロ scaffold → 将来 wiring 用温存+doc 注記 (削除方針外) |

---

## 特記事項

1. **ゼロデイ級 (本プロジェクトで初めて発見された潜伏実害) の代表例**:
   M-4 (bytemuck アライメントパニック)、S-1 (RCAS ぼかし偽装)、S-2 (深度
   透視補正)、AI-1 (アライン未達成)、AX-1 (二重 release 無防備)、
   BG-1 (SVO trace 意味論スタブ)、BL-1 (Perlin 全定数化)、
   BZ-1 (確保失敗リーク)、BX-1 (中間ヒット誤タグ)、
   CF-1 (転置射影)、CF-3 (complete-dead 三角形判定)、
   CG-1 (転置フラスタム抽出・コモンモード不発)。
2. **自己誤り捕捉実績** (テスト赤/検算が設計ミスを検出した記録):
   BM-3 bfSize、BN-3 RCAS 入力 f64、BP-3 負入力 wrap、BX-2 ×2
   (octant シナリオ・R90² 表現)、CA-4 不変量設計、BZ 注入手順修正、
   CB シナリオ包絡、CD hull_len 頂点数 (Python 検算が実行前捕捉)、
   CD フレームカウント off-by-one (テスト赤が捕捉)、
   CE fxaa untouched シナリオ (contrast 0.0897 > 0.0833 で発動する
   設計ミスをテスト赤が捕捉、等輝度 pure red 設計へ訂正)、
   CF w2 恒等式引継ぎミラー (前セッション検算の全 False 出力が符号
   解析を強制、楔定式化へ精緻化)、CF doc 過剰主張「常に偽」(符号代数が
   楔生存を捕捉、「相対幅 1e-4 の v2 角楔」表現へ訂正)、
   CG identity テストへの \n リテラル混入 (cat -A 検査が捕捉、正規 3 行へ)
   (CL ゲート修正の初版がループ末尾の bitmask 反映をスキップ
   (テスト赤が実行前に捕捉)
   (合計 16 件。いずれも「テスト赤=自己誤り捕捉装置」規律の実績として
   台帳に残す)。
3. **一次情報照合で「変更なし」判定** (誤修正抑止の記録):
   BO-4 (GPUOpen ffx_cas.h)、BP-2 (Narkowicz ACES)、BN-4 (W3C WGSL CRD)、
   BD (Microsoft D3D12EnhancedBarriers.md)、AZ-3 (Khronos ASTC)。
4. **引継ぎ棚卸し** (未消化・将来 wave の対象):
   BA-3 render_pipeline:787 unwrap_or_default 静寂空化、
   temporal_mesh_diff::diff_section 将来配線候補、
   binary_greedy_meshing::greedy_merge_2d_pull dead fn、
   full_graph_wiring「実 draw indices」注記 vs 連番の意味論差異 (BV 発)、
   aokana Hi-Z/visibility buffer 統合 (CB-3 で未配線明示)、
   render_pipeline.rs (1389 行) / full_graph_wiring.rs (2415 行) の本監査
   (2 wave ずつ想定)。
