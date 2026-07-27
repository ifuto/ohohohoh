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
| CF-5 | 中 | band 検査の raster 適用は眼前壁級 occluder を全沈默 (非保守) → project (test 用 band 付) / project_screen (raster 用) 分割 + 8 角凸包完全内包 scanline + 背面/far 棄却 |
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
| DD-1 | 中 | bench_harness record が fine_max_us 超過サンプルを末尾 fine スロットへ min(us, len-1) で静寂飽和 → percentile の粗バケットフォールバックが構築上到達不能の死にコード + 超過サンプルが fine_max_us に過小報告 (5,000us が 2,000us 表示) の二重虚偽 → get_mut で [0, fine_max_us] 内のみ記録 (フォールバック復活・保守的上振れ側へ) + 厳密ピン (p80=9,999/p100=999,999 vs 旧 2,000 = 2.5 倍/250 倍過小) |
| DD-2 | 中 | run_timed fine 表が固定 60_000_000us = 呼出毎に 480,000,008 B (457.8 MiB) 確保 (rsift_bench 9 連続・lib テスト毎回、低スペック PC 敵対) → run_timed_fine_max_us = max(60ms, target_ms)・1s cap 化 (最大 8,000,008 B ≈ 1/60、商 59.99994… 厳密ピン) + saturating/clamp 境界 6 点ピン。捕捉 22 件目: 「1/60 未満」の自己断言が数学的誤り (60×8,000,008 > 480,000,008) をテスト赤で捕捉・両側挟み込みへ訂正 |
| DD-3 | 低 | percentile_us p=0 で want=ceil(count·0)=0 → 先頭スロットで即 return 0us = 未観測値の虚偽報告 → nearest-rank 定義 rank∈[1,count] に max(1.0) クランプ (p0=最小値厳密ピン) |
| DD-4 | 低 | doc/label 虚偽訂正: 構造体 doc「buckets[i]=[10^i..10^(i+1))」は bucket0 の 0us 含有と矛盾 → [0..10) 明記、ascii ラベル ">10s" は 10,000,000us 丁度含有区間と矛盾 → ">=10s"、fine フィールド doc「下位 1ms」はパラメトリック範囲と矛盾 → [0..=fine_max_us] + メモリ契約 8·(n+1)B 明記 |
| DD-5 | 観 | Hdr::ascii の消費者がテストのみ → rsift_bench markdown へ per-bench bucket histogram 配線 (消費者追加方針)。併せて証明/契約 doc 注記: Stopwatch ops/sec は new〜finish 壁時計全期間ベース、csv name 非エスケープ前提、time() as u64 切捨ては ≈584,542 年で到達不能、percentile max_us fallback 到達不能 (全バケット合計=count≥want)、run_timed Instant+Duration 加算パニックは fail-loud 側 |
| DE-1 | 中 | branchless_dda inv_dir tiny-dir ガードが +INF 固定で符号を潰す → 負の tiny dir (|d|<1e-8) で step=-1 × inv=+INF = t_delta=-INF となり軸が負方向へ暴走 (本来命中のブロックを取り逃がし範囲外脱出)、整数境界始点では 0·INF=NaN で branchless_axis 比較が全 false 化し無関係な軸を踏み続ける二重の誤動作経路 → inv は符号保持 ±INF (-0.0 は +INF) + tiny lane の t_max/t_delta を直接 +INF 固定する lane 一貫 immobilize へ再設計 (digest 不変性を bench 入力域で構造証明: d_raw は 0.001 格子で負成分最小 |d|≈5.77e-4≫1e-8、ゼロ成分は +0.0、実測 digest も不変) + 厳密ピン 3 (負 tiny 命中 steps=5/整数境界 NaN hijack 遮断 steps=3/inv 符号・eps 境界・-0.0 完全ピン) |
| DE-2 | 低 | trace_section が非有限 origin/dir を静寂受理 (NaN.floor() as i32 = 0 飽和で (0,0,0) 起点の虚偽 trace) → debug_assert fail-loud (release はコンパイルアウトで bench 計時経路と無干渉) + should_panic ピン |
| DE-3 | 低 | 内外範囲判定の else-if が同値条件の再走査 (全軸 0<=v<16 の否定 ≡ 何れか v<0||v>=16 = 排反完備) → else 化 + 証明 doc (挙動完全等価) |
| DE-4 | 観 | doc 群: branchless_axis の tie 優先度 X>Y>Z (argmin 最小添字) 明文化、VoxelHit 全フィールド doc、trace_section の max_steps/steps/範囲外即 None/air=0 契約、eps=1e-8 の根拠 (16³ 最長踏破 48 voxel で drift≦4.8e-7 voxel = 観測不能)、WGSL_BRANCHLESS_DDA は消費者ゼロ scaffold で t_max のみ進行・voxel 座標更新を欠く不完全対称の正直注記 (配線時統一)、NaN 非発生証明 (有限入力では DE-1 遮断後 0·INF 経路なし) |
| DE-5 | 観 | テスト未カバー経路の strict 化: 負方向 slab 対称ピン (vz 15→6 steps=9) 追加 (既存テストは全て正方向のみで step=-1 経路未被験だった) + trailws 抱き合わせ: compute_light_prop.rs:48/56 (LIGHT_PROP_WGSL 生文字列内インデント空白行、WGSL は空白非感性・byte ピン無しを照合済)・entity_culling.rs:419 の trailing whitespace 3 件除去 (HEAD 逸脱 12→11 も 1 件改善) |
| DF-1 | 中 | light_cache propagate_dirty の打ち切りが **pop 後 break でキュー先頭を未処理破棄**、かつ max_steps=0/丁度境界で「queue 空 × 未処理残」でも **dirty=false に確定** → 以後の呼出が `!dirty` 早退で**永久 no-op = ライト未完成のまま完了を詐称** (既存 dirty_cleared テストは dirty 未検査で素通りしていた) → VecDeque 廃止・writes-budget (budget-before) label-correcting pass へ再設計 (dirty = truncated に忠実) + 厳密ピン (0 steps dirty 維持・継続で収束到達) |
| DF-2 | 低 | SectionLights::set の同値上書きが都度 dirty を宣言 → 無駄 re-flood 誘発 (wave 102 DB-4 同型) → 同値 no-op 早期復帰 + dirty 非汚染ピン (packed 不変も実証) |
| DF-3 | 低 | doc 群正直化 5 件: ヘッダ「skylight propagation」は sky flood 未実装 (ニブルは格納のみ) ・消灯/減衰伝播未実装 (除去 BFS 要) を契約明記・opaque 点灯セル自身の発光設計・wrapping_sub の underflow→巨大値→フィルタの安全性・writes ≤ 15×4,096 = 61,440 の u32 非飽和証明 |
| DF-4 | 観 | propagate_dirty 戻り値を改善 pop 数 → **改善書込み数**へ意味変更 (外部消費者ゼロを照合済)+ 2 emitters シナリオ総書込み **2,639** の厳密ピン (Python 独立シム照合) |
| DF-5 | 中 | full re-seed (昇順) + 小 max_steps で予算が先頭冪等セルに燃え frontier が進まない **飢餓 (livelock)** — Python 検算中に budget=64 で不収束タイムアウトを実測発見 → writes-budget 設計で構造排除 (budget-before・冪等再訪は予算消費せず・budget 64 → 50 calls 収束・最終 packed bit 一致を Python 先行検算) |
| DG-1 | 低 | bitpacked_section ヘッダ doc 虚偽群: 存在しない型名 `SingleValueSection` (実体は `CompactChunkSection::{SingleValue,Bitpacked}`)・「4..=15 bit」(実到達 16 を差分ファズ phaseC で実測: 33,000+ 状態で 15→16 遷移。上限は u16 ドメイン 65,536 状態 ≤ 2^16 の構造証明より **16 打止め・17 拡張は到達不能**)・「4,000倍軽量化」(正確に 8,192/2 = **4,096 倍**) を一括訂正 + vanilla 直接フォーマット移行は一次情報未照合と明記・互換入出力経路非存在の消費者警告 |
| DG-2 | 低 | `memory_footprint_bytes` が `rev` HashMap ヒープ (バケット/制御配列) を非計上 — 多数状態時は本体超過し得る誤差だが digest 行 `footprint=*B` 不変のため定義式据置・契約を doc 明記 (実メモリ管理用途禁止) |
| DG-3 | 低 | 静寂破壊 2 経路の fail-loud 化: ①`idx` の範囲外座標が加算で**別セルへ静寂エイリアス** (x=16 → (0,y+1,z) 同一 index)、②`get` の `unwrap_or(0)` が (pub フィールド破壊経由のみ到達の) 域外 pal_id を静寂 air (=0) 化 → debug_assert (release/bench 計時無干渉・戻り値契約不変) + should_panic 2 ピン。**捕捉 25 件目**: 初版ピンが SingleValue 経路 (座標非参照) で不発 → テスト赤が enum 層盲点を捕捉、関門を enum ディスパッチ層へ移設 (idx 層 assert は `BitpackedSection` 直接 API 防御の第 2 層として保持) |
| DG-4 | 観 | `BitpackedSection` pub フィールドの不変量自壊危険 (data レイアウト↔bits_per_block・pal_id < palette.len()・rev↔palette 1:1) を doc 明文化 (読み取り専用契約) |
| DG-5 | 観 | 挙動不変の strict ピン強化: 同値 set 完全 no-op (wave 102 DB-4 同型を Strict 化)・5bit 跨ぎセル spill 厳密往復 (cell 12 = word0:60 / word1:1)・33,000 状態 16bit 全遷移可逆 + 仮設差分ファズ (100k ops・20 境界×2 組・40,300 状態で幅 0→16 完全可逆) 全 Pass を検証記録として台帳化 |
| DG-6 | 低 | **捕捉 26 件目**: 設計中の幅境界述語 (no-span 表現) が **2^k+1 側を区間外と呼ぶ off-by-one** — 機械検算 (n∈{17,33,65,…,32769} 代入で (w-1)<…≤… の両端評価) が出荷前捕捉 → 包含形 `2^(w-1) < n ≤ 2^w` で厳密ピン化 (誤述語自体は未出荷、17↔16 不遷移ゆえ既存単調性テストでは検出不能だった) |
| DH-1 | 中 | morton_order 3D 系 (split_by_3/encode_3d/_fast/decode) の実効ドメイン **10 bit/成分 (0..=1023) が doc 非記載で静寂切捨て** — 生きた被害者 `full_graph_wiring:477` が `0xF_FFFF` (21 bit) マスクで渡し**上位 11 bit 静寂消失** (局所性 sort キー衝突・安定性により不具合確定なし)、`morton_sort_indices` の呼出側 `& 1023` 明示と契約非対称 → 挙動完全一致のまま wiring を & 1023 明示化 (「切捨ては split_by_3 magic チェーン第 1 段が中央強制で前置 `& 0x3FF` 自体も数学的冗長」を adversarial (a) 変体で構造証明) + 全ドメイン契約を各 fn doc 公表 + 切捨て厳密ピン |
| DH-2 | 低 | `split_by_3`/`split_by_2` の `mut a` 未使用 (変異しない束縛) unused_mut 警告 2 件根絶 (lib 警告ベースライン 14→12・lib-test 17→15 を機械改善、api 13 不変) |
| DH-3 | 低 | ヘッダ「BMI2 動的検出を用いた 2D/3D…完全実装」虚偽 (**2D に BMI2 経路は不存在**) + `morton_encode_bmi2` 名称誤導 (非対応 CPU でも SWAR 実行・非 x86_64 は dispatch 自体なし) の誠実化、SWAR≡BMI2 全入力 bitwise 一致を差分ファズ証明 |
| DH-4 | 観 | 負座標 `i32 as u32 & 1023` wrap 契約 (-1→1023=Z 曲線高角、局所性喪失) を doc 明文化 + encode/sort 両経路の厳密ピン (消費者へ offset 正規化を要求する旨) |
| DH-5 | 観 | 消費者ゼロ API の生存確認 (MortonGrid3D/morton_sort_indices/encode_bmi2/decode_*_fast/MortonOrderTest): 削除せず doc 明記 + **morton_sort_indices のキー前計算最適化** (比較毎再評価 O(n log n)→O(n)、安定性含む新旧完全一致を仮設ファズ 64 種で証明) + fast 系 #[inline] 付与 |
| DH-6 | 観 | MortonGrid3D 契約明文化 (SIZE 冪/≤1024・10 bit 全単射・SIZE=1024 は T=u32 で 4 GiB 級注意) + 非冪 assert の should_panic ピン |
| DH-7 | 観 | **3 系統 Morton (morton_order / lbvh::morton3 / WGSL cs_morton) の相互等価ピン不存在** — lbvh part1by2 は第 1 段マスクで 10 bit 化を内包 → 全 u32 ドメイン (境界・内域・wrap ランダム 200k) で bitwise 一致ピンを追加 (将来発散の抑止) |
| DI-1 | 中 | leaf_fast_path の**逐次変異スキャン意味論**: collapse は x+y+z パリティ (= スキャン最先 interior voxel と同位相) のみ → 固体立方体内部でも **3D チェッカー状の半分のみ消える** (旧 doc「only boundary faces remain」は厳密には虚偽)。閉形式 ⌈(a-2)³/2⌉ を Python 独立モデル照合 (a=3..10 → 1,4,14,32,63,108,172,256・全16³ = 1,372) + 境界環 untouched 証明。挙動は bench `delta_idsum` で digest 凍結のため契約公表+厳密ピン (snapshot 化は digest 更新の大規模見直し相当と誠実注記) |
| DI-2 | 低 | is_leaf の 12-iter `contains` → 4×u64 ビットマスク展開表化 (hot loop 改善) + 全 65,536 入力で厳密等価ピン。**捕捉 29 件目**: u16 全域 (65,536) に対し表は 256 bit のみで **block ≥ 256 が index out-of-bounds panic** を生産経路へ持込んでいた (元 contains は全 u16 安全) — 全網羅テストが出荷前に捕捉 → guard (block < 256) で完全等価 + テスト鏡写しの guard 漏れも併せて根治 |
| DI-3 | 低 | LEAF_TYPES の「legacy numeric ids」虚偽寄りコメント (162..=165 は旧 numeric id として不存在) → vanilla 分類上の目安 + hash 空間仮想 id 帯 (200..=205) への誠実注記 |
| DI-4 | 観 | merge_leaf_mesh 消費者ゼロの保持明記 + 同一チャンク前提を debug_assert fail-loud 化 + byte 等価ピン (捕捉 27/28 件目: `color0` 不存在フィールド作文 (E0609)・slice == 誤用 (E0369) をビルドエラーが捕捉→Pod byte 照合へ) |
| DI-5 | 観 | 境界 1 層非走査・wrapping guard の観察 (走査拡張変体は guard が境界 collapse を常に false にするため**証明済み意味的中性**、adversarial 証明) + 既存 a=2/border ピン維持 |
| DJ-1 | 中 | visibility_graph::add_edge の多重辺累積 (multigraph): 呼出毎の無条件 push だったため、毎フレーム同一辺を再登録する消費者 (full_graph_wiring::tick_world は render_pipeline:1112 から毎フレーム呼出) 経由で adjacency Vec が単調増大 — flood 結果は visited 抑止で正しいまま保たれるが、メモリと BFS 走査幅だけが漸次増大する構造 → 冪等化 (単純グラフ維持 + 逆向き再登録無視 + 自己ループ 1 件正規化) で生産者側根治。挙動完全一致 (bench/wiring は各辺 1 回のみ構築するため digest 凍結のまま) |
| DJ-2 | 低 | visited の HashMap<ChunkNode, bool> は bool 値がデッド (key 存在のみ意味) → HashSet<ChunkNode> へ等価置換 |
| DJ-3 | 低 | max_dist ちょうどのノードの展開は子 (max_dist+1 層) が全て pop 即棄却される自明殻 → `dist < max_dist` guard で enqueue 自体を抑止 (結果集合・is_opaque 呼出集合・キャッシュ内容は bit 完全一致で、queue 交通量のみ低減) |
| DJ-4 | 観 | 意味論契約の公表: **遮断測地球定理** (flood 結果集合 = opaque 非始点を頂点除去した誘導部分グラフの半径 max_dist BFS 球に厳密一致 — 独立第二実装 (pruned 層別 BFS) との 20 試行差分ファズで実証)・is_opaque 呼出契約 (各ノード高々 1 回・始点も評価されるが判定値は不使用)・負 max_dist=空+空集合キャッシュ・CAP 全破棄 eviction で is_visible は一時 false (誤 true なし、再 flood で回復)・add_edge 冪等/自己ループ正規化・結果 Vec は BFS 訪問順で決定的 — ヘッダ/メソッド doc + ピン群 |
| DJ-5 | 観 | 閉形式ピン化: 角始点 (d+1)(d+2)/2 (g>d)・中央 1+2d(d+1) (g≥2d+1)・g=8 中央クリップ 59、bench opaque=0 行 (28/85/59) を strict 固定 + **hash3 を Python 移植した完全独立シムで wide_static_bench visibility 全 12 行 (corner/center × g∈{8,16,32} × opaque∈{0,20}pct) の reached/vis_hits を事前予測 → seal 実測と全照合** |
| DJ-6 | 低 | 抱き合わせ san 集合の網羅漏れ根治: repo 全量走査で cp932 不可 (= JIS X 0208 非含有 = 日本文出現不能) CJK 13 字・34 箇所の残留を発見 (透視/沈黙/精確/事実等への誤字、全てコメント/doc/md で挙動無関係) → 全 31 箇所を日本語字に根治 + san 集合を 457 字へ拡張 (機械検算で「452 字」主張は実効ユニーク 445 (447 tokens − 重複 2: U+9992/U+7EC7) の過大申告と判明 → 重複除去 + 12 字追加で 457 に確定しユニーク数を selftest 厳密ピン化)・**捕捉 30 件目**: seal が変更追跡ファイルのみ走査する運用盲点で、集合掲載済の U+5B9E が未変更ファイル (svdag.rs) に潜行していた → 全量走査を wave 運用に併用・**捕捉 31 件目**: 「452」カウント誤り (重複 2 の見落とし) をユニーク数ピンで再発防止 |
| DJ-7 | 低 | 抱き合わせ tooling: fmdiff は HEAD 版を /tmp 孤立ファイルで rustfmt するため `mod` 宣言を含むファイル (jvm/lib.rs 等) の変更が rustfmt の mod 解決失敗で**構造的に seal 不通**だった → `--config skip_children=true` 化 (子 mod 非再帰で孤立評価可、mod 無しファイルの出力不変を旧 changed セット全体で実証) で根治・**捕捉 32 件目**: 私の EOF 修正 (jvm/lib.rs) が seal ゲート 2 で差止められこの制約を発見、併せて rsift-installer/Cargo.toml (CRLF 原生) への LF 末尾改行追加も san ゲート 1 が差止め → 当該 1 件は revert し EOL 専用 wave へ回付 (seal が範囲外混入を機械差止めした 2 連事例 = ゲート実効性の再実証)。EOF 末尾改行根治は 4 ファイル (api/gui-installer/jvm Cargo.toml + jvm/lib.rs) に確定 |
| DK-1 | 低 | branchless_block::select_branchless のコメント不正確 (旧「(cond as u32 * a) + (!cond as u32 * b)を算術で」は実装の `|`/(1-m) 形と不一致) → 数学的完全等価 (m∈{0,1} で片側積は必ず 0、0|x = 0+x = x より OR/加算/XOR が全て一致、ビット共有時も厳密一方選択) を証明記述で誠実化 + OR≡ADD 代表 200 組 pin |
| DK-2 | 観 | transparent フィールド消費者ゼロの意図的保持 → `transparent_branchless()` accessor 整備 (将来の半透明ソート/透過パス向け、削除しない消費者整備方針) + 全 4096 入力+wrap 一致 pin |
| DK-3 | 観 | 12 bit ドメイン契約の公表: アクセサは `id & 4095` で**静寂 wrap** し id ≥ 4096 を下位 12 bit の別 id へエイリアス (u16 全域・vanilla 広域 blockstate 空間は 12 bit を超えうる、プレースホルダ値の仮性とは独立の第 2 近似) — 全 65,536 入力で wrap 契約厳密 pin + wiring:986 の u64→u16 キャストと併せ 2 段 truncate 経路であることも注記 |
| DK-4 | 観 | 構造集計の厳密ピン化: opaque 真 2,730/偽 1,366・transparent 真 820 (i=0 含む)・i%15==0 は 274・**opaque∧transparent 交差 546 個 = フラグ非排他** (最小反例 i=10。「transparent=true なら opaque=false」の消費者仮定は現値で誤り、正式テーブル化時に排他性の契約決定が必要)・light 16 レベル完全一様 256 (=4096/16) |
| DK-5 | 低 | bench blocklut_lookup 3 行 (n=2^18/20/22) の acc を SplitMix64 完全独立シムで事前予測 (2,143,132/8,562,084/34,244,920、E[v]=2/3+7.5≈8.167/iter と整合) → seal 実測照合 |
| DL-1 | 中 | exposure::target_exposure の**メータリング虚偽を根治**: ヘッダ「same metering used by Unreal's Histogram auto-exposure」に対し実装は線形 1/E[L] (算術平均の逆数) で、Epic 一次情報 (「Auto Exposure in Unreal Engine」: Histogram は log 輝度ヒストグラムを解析して平均輝度を決定) の **log 領域加重平均 (幾何平均)** と不一致 — Jensen E[lnL]≤lnE[L] で常に Unreal 式以下 (=実シーンで暗め、2 段シーン実測 新/旧=1.194824、新 8.543594/旧 7.150503)。log 領域計量へ修正 (定数シーンは両式厳密一致で挙動不変=既存 3 テスト維持、変動シーンのみ幾何平均側へ)。GPU パリティ無影響 (log/exp は WGSL 非搭載・luma/apply のみ GPU の責務分離)・digest 行なし・wiring a↔b 同実装比較と範囲 assert 維持。誠実残差異 (bin 数 64 vs 256・較正 18% 中間グレー K vs 1/geo-L 独自規約) を doc 公表 |
| DL-2 | 低 | build_histogram の NaN/inf/退化入力契約公表+厳密 pin: NaN 色は f32::max 非 NaN 優先で l=1e-4 (bin 0)・+inf→bin 255・-inf→bin 0・min≦0 かつ max≦0 の退化範囲は log_max=NaN→range 床 1e-6 → 全 bin 255 静寂確定 (panic なし) → target は exp(9.21…)≈1e4→clamp 20 に厳密確定 (Python 機械検算一致) |
| DL-3 | 低 | adapt 契約公表+厳密 pin: k=1-exp(-speed·dt) は de/dt=s(target−e) の**厳密離散解** (adapt(1,3,4,0.1)=1.659359908)・speed=0 恒等・負 speed/dt は反適応で 0.05 clamp・**NaN は 0.05 への静寂崩落でなく伝播** (clamp は比較 false で self 返却 = fail-visible) — adversarial (b) で max/min 連鎖化による NaN 崩落を 1 RED 検出 |
| DL-4 | 観 | 分位境界契約 pin: lo/hi は `(total*pct/100) as u32` 切捨て・包含は bin 累積区間の整数中点 (bin 粒度依存)・low>hi/空ヒスト/全除外は 1.0 — adversarial (d) で中点→左端変体を 4 RED 検出 (既存定数 2 テストも連鎖検出) |
| DL-5 | 観 | Vec4/Vec3 ops 消費者ゼロの意図的保持明記 (shader 側パリティ利用の API 面) + **捕捉 33 件目**: Vec4 の Mul が Vec3::new を呼ぶ転記 typo (E0061/E0308) — sandbox リセット後の初回コンパイルが捕捉 → 根治 (ゼロデイ記録: 2 度の作成で latent だった同 typo を apparatus が出荷前差止め) |
| DM-1 | 中 | bloom::prefilter のゲイン意味論公表+厳密 pin: f = (l−T)/T.max(1e-4) は**相対ゲイン**で出力輝度 l·f — l>2T で**入力超過の増幅** (T=1,l=4→f=3→出力 12)、T≦1e-4 では発散級 (T=0.001,l=1→f≈999)。luma 保存形 (c·(l−T)/l、増幅しない標準形) とは異なる意図的強め bloom 規約と判定して変更せず (美的設計領域)、契約を doc 化 + l=2T で bit 厳密恒等・l=T 境界 0 (knee 無関係)・小閾値発散の厳密値 pin。呼出側契約: threshold ≫ 1e-4 を明示 |
| DM-2 | 中 | **wiring の bloom 実効ゼロを数学的に確定**: full_graph_wiring:1671 は prefilter(mapped, threshold=1.0, knee=0.5) を **tonemap_display 後** (linear_to_srgb が x>=1.0→1.0 clamp で mapped∈[0,1]³) に適用するため luma ≦ 0.2126+0.7152+0.0722 = 1.0 = threshold → ゲート `l <= threshold` は**常真 → bloom ≡ 0**、composite = (m+0).clamp(0,64) = m の **bit 厳密な恒等写像** (729 点グリッド + (1,1,1) の to_bits 厳密 pin)。現行積分の bloom 段は描画に一切寄与しない (この事実を誇張せず構造的確定として記録)。閾値再調整 (例 0.7/knee 0.3 で実効化) はレンダ結果を変える美的判断のためユーザー設計領域として引継ぎ (私は値を変更しない) |
| DM-3 | 低 | blur_row 契約公表+厳密 pin: 重み [0.0625,0.25,0.375,0.25,0.0625] = 二項核 [1,4,6,4,1]/16 は**全て二進厳密値**かつ和は f32 で厳密 1.0 → 定数保存は 1 ulp 誤差もない **bit 厳密** (to_bits 化)・radius=0 は bit 恒等・dst<src は panic (fail-loud、should_panic pin)・端は edge-clamp (adversarial (d) clamp 除去 → j OOB panic で 2 RED)。 |
| DM-4 | 観 | luma Rec.709 式が 3 系統 (bloom / frame_postfx::luma_run_cpu / exposure 内蔵) で**bit 一致**を xorshift 256 色ランダムで厳密 pin (乗加順序差による 1 ulp 発散の将来混入を apparatus 化)。 |
| DM-5 | 観 | composite/prefilter の NaN 伝播 pin (f32 比較 false で self 返却 = fail-visible、0 側への静寂崩落ではない)・composite 上限 64 clamp 公表・Vec4/Vec3 ops 消費者ゼロの意図的保持明記。**捕捉 34 件目**: Vec4 Mul の Vec3::new 転記 typo を 2 連続 wave で再犯 (捕捉 33 と同一零デイ、E0061/E0308 が即捕捉) → 根治。**捕捉 35 件目**: &mut 借用 closure を `let f` で宣言 (E0596: `let mut f` 必須) を初回コンパイルが捕捉 → 根治。 |
| DN-1 | 低 | cpu_saver の未使用 import `bytemuck::{Pod, Zeroable}` 除去 (構造体皆無で実使用なし、lib 警告 11→10)。併せてヘッダの「分岐予測ミスを完全撲滅する/4x4x4 L1 キャッシュライン最適化/Branchless DDA」を誠実化: 命令選択はコンパイラ依存で「完全撲滅」は静的保証不可・タイル=4³=64voxel 確定だが 64B 一致は voxel=1B レイアウト依存・DDA 本体は branchless_dda (DE 監査済) 側で本モジュールは符号/軸選択プリミティブ |
| DN-2 | 低 | branchless_select 系の契約 doc+厳密 pin: mask=-(cond as i32) 全域で支持集合非交差のため `|`/`+`/`^` 完全等価 (DK-1 同型)、値は if/else と全入力厳密一致。f32 版は to_bits 往復が安定保証で **NaN ペイロード/-0.0/±inf を bit 保持** (quiet 化・正規化しない) — xorshift 200 組厳密照合 + 0x7FC00001/0x80000000 の厳密 pin |
| DN-3 | 低 | BranchlessVoxelStepper 契約公表+厳密 pin: step_direction は +>0→1/-<0→-1/else 0 の境界で -0.0→0 (IEEE で -0.0<0.0 は false)・**NaN→0** (両比較 false)・±inf→±1。advance_axis は {vals}³=343 網羅で**ちょうど 1 軸 true** (異常値含む排他選択)・同値タイ優先 x>y>z・全 NaN→z フォールバックの厳密 pin |
| DN-4 | 低 | LoopTiledVoxelScanner: 到達 index 閉形式 (y>>2)·1024+(z>>2)·256+(x>>2)·64+(y&3)·16+(z&3)·4+(x&3) の **bijection 性** (2bit フィールド置換) + 先頭 8/末尾 4/index64 spot の到達順厳密 pin。CacheLinePrefetcher: prefetch はセマンティクス非観測 (値に無影響のヒント)・x86 では無効アドレス非フォールト (アーキテクチャ保証) の契約 + smoke pin |
| DN-5 | 観 | 消費者ゼロ (lib.rs re-export 経由の公開 API 面のみ) の保持明記 (削除せず将来ホットループ向けプリミティブとして契約ピン化) — 「消費者いなくても消さない」方針踏襲。**捕捉 36 件目**: 私の ad hoc `rustfmt --edition 2024` 走査と seal の fmdiff 正準形 (fg-gated `use` の並べ替え規則) が不一致となり seal ゲート 2 が自己起因逸脱 2 行を差止め → fmdiff 出力を忠実適用 (x86/x86_64 ペアで _mm_prefetch を _MM_HINT_T0 より先に) して根治 (cfg ゲート別グルーピングで意味的自己同一、adversarial 結論は不変) |
| DO-1 | 低 | out_of_core_paging `new()` の未使用 `mut file` 除去 (set_len は &self のため、むしろ write/seek の無い構造) — lib 警告 10→9 |
| DO-2 | 中 | **部分書換えの残滓曝露契約を公表**: write が 64 KiB 未満の場合ページ残部は無改変で、剥奪で譲受したページの尾には旧占有者の残滓が残り、長めの `out` (≦64 KiB) で呼出側はそれを読み得る。ゼロ潰ししない生ストレージ設計 (長さ帳簿は呼出側責務) と確定訂正の上で厳密 pin (100B 書込み後の 100..256 が旧 0xA1 残滓)。**観測リスクは現時点で不在**を誠実記録: 唯一消費者 wiring:747 は 8 byte 書込みのみで read を一切呼ばない |
| DO-3 | 観 | read 側 `Ok(0)` の 2 義性を公表+pin (未登録キー/登録済みで out 空の区別は `page_table.contains_key`、未登録読出しは out 無改変) |
| DO-4 | 低 | 真 LRU の被害者選択・page_idx 割当を独立実装のシャドウモデル (別形態の参照実装) と 1,000 オペ差分ファズで厳密一致を実証 + page_table/lru_order の 1:1 構造不変量を op 毎 50 間引き pin。既存 lru_evicts テストに続く第 2 層網として adversarial (a)(b)(d) の連鎖検出に寄与 |
| DO-5 | 観 | 運用契約 pin: 全 handle の idx < max_pages (剥奪で超過しない)・offset=idx·65536 の算術・バッキングファイルは set_len(cap) で固定 (伸縮しない・再起動時は新 cap で切詰)・**再起動は再装着しない** (物理残存も到達不能、Ok(0) pin)・u32 境界 (u32::MAX+1 を open 前に拒否) |
| DP-1 | 低 | atmospheric::sky_color のモデル形態誠実化: 位相関数 (Rayleigh/HG) と Beer-Lambert 透過率は物理式そのままだが光路は**天頂角に依らず一様 8 km スラブ** (8 段中点則) の静的スタイル化であり、Preetham/Hosek-Wilkie 系のスケール高度・地平伸長は非モデル — ヘッダに公表。厳密 bit pin 化: 位相関数 6 値・sky_color 2 構成 6 成分を Python IEEE f32+ctypes libm 独立シムで事前導出→照合 (rn=(3,4,0)→(0.6,0.8,0)、非変換対称 bit 厳密) |
| DP-2 | 低 | 位相関数の球面正規化 ∫p dΩ=1 を中点 4096 で数値公表 (g=0.76 では max 床 1e-4 は非発動: min d=(1-g)²=0.0576 と解析証明も併記) |
| DP-3 | 低 | **WGSL PI 定数の丸め不足を根治** (`3.14159265` → `3.14159274` = f32::consts::PI 0x40490FDB と bit 一致) — WGSL/CPU 位相関数の非超越部は bit 一致へ。pow(d,1.5) はドライバ依存のため bit 同値は構造不可・normalize 0 振舞差 (CPU self 返却/WGSL NaN) を差異公表 + ソース走査 pin。**捕捉 37 件目**: 修正コメント自身が旧リテラル文字列を含み自己衝突 (pin の negative-anchor 設計ミスをテスト赤が捕捉) → コメント言い換えで根治 |
| DP-4 | 観 | normalize 境界契約 pin (閾 1e-8 内外・0 vec→self 返却・NaN ray→全成分 NaN 伝播・transmittance(0)=1/+inf=0)。**捕捉 38 件目**: Python 独立シムで内部貢献の乗算を右結合的に評価 (Rust 左結合と z 成分 1 ulp 差) → 左結合に訂正し 6/6 成分で実測照合完了 |
| DP-5 | 観 | Vec4/sky_color_v4/Vec4 ops 消費者ゼロの意図的保持明記 (WGSL 側パリティ API 面、消さない方針)。Wgsl 登録 (gpu_runtime collect_all_wgsl) 経路は確認済だがピクセル還流は pipeline 未追跡と誠実注記 |
| DQ-1 | 中 | **fxaa::shade の wiring 恒等証明**: 唯一の Rust 側呼出 full_graph_wiring:1694 は同一色 (sharpened) を 5 引数 (center/n/s/e/w) 全てに与えるため lmin==lmax → contrast≡0 < threshold → `return center` の **bit 厳密な恒等写像** — 本経路の FXAA は描画に一切寄与しない (GPU AA は別経路 fxaa.wgsl 登録、Rust 側は参照実装/将来 CPU フォールバック)。DM-2 (bloom) と同種の構造的確定で誇張なく記録。実効化 (真の近傍サンプリング) はフレームバッファ配線を伴う設計判断のため引継ぎ |
| DQ-2 | 低 | 勾配軸タイブレーク契約 pin: 厳密 `>` のため |gx|==|gy| は **else (E/W) 優先** — gray-luma 線形で 0.7-0.3≡0.9-0.5 となる構成で out=0x3F1EB852・t=0x3ECCCCC8 を Python IEEE f32 シム事前導出→照合 |
| DQ-3 | 低 | **NaN 伝播の位置非対称**を公表+厳密 pin: min/max の NaN 脱落により **n/s の NaN は完全マスク** (gy=NaN→比較 false→else 分岐で E/W 有限なら有限出力) だが、e/w/center の NaN は avg/out へ伝播。マスク側は厳密 0x3F000000、伝播側は is_nan で pin (従来 doc の「非有限の扱いは未規定」を詳細規定へ更新) |
| DQ-4 | 観 | threshold 2 分岐構造の厳密 pin: `base.max(lmax*rel)` は暗所で絶対床 1/256=0x3B800000、明所で相対支配 (例 0x3DAD3A1D)。暗所 0.0055 デルタで発動・+0.5 シフト同輝度差では不発の対蹠を厳密値固定 + **輝度シフトで contrast が丸め変化する事実** (0x3BB43958↔0x3BB43980) の公表 |
| DQ-5 | 観 | luma は Rec.601 (0.299/0.587/0.114) で post チェーン他段 (bloom/exposure の Rec.709) と**係数系混在** — FXAA 伝統に整合した意図的選択だが消費者警告として公表、旧 doc「BT.601-ish」は係数として厳密に Rec.601 そのもののため訂正 + luma(1,1,1)=1.0 厳密 (0.299+0.587+0.114 の左結合和)。**捕捉 39 件目**: NaN pin で `bits(bits(0.5) as f32)` と二重 bits の自分 typo をテスト赤が捕捉 → 根治 |
| DR-1 | 中 | **wiring 側のパーティクル二重カウント+単調累積を根治**: full_graph_wiring:1392-1408 は (1) `begin_tick` が tick 回転のみでカウント類をリセットしないのに `reset_counts` を一切呼ばず active 数が tick を跨いで単調累積し (予算解釈が「同時アクティブ」から「累積総数」へ変質)、(2) `allow()` 内部の note_active に加え成功側で**もう一度** note_active を呼ぶ**二重カウント** (実効予算≈意図の半量、per_kind 256 は 32 tick 級で恒久的間引き支配へ落ち込む構造)。プロトコル「begin_tick → reset_counts → allow (内部カウント 1 本)」に根治 + controller doc に使用規約を strict 化 (digest 経路なし、strict テスト影響なし) |
| DR-2 | 低 | kind_idx>=10 は Other (9) への**静寂クランプ**契約公表+pin (allow 経路では 9 として計上、直接 note_active(10) は no-op の非対称) — DK-3 の 12bit wrap と同族の index ドメイン公表 |
| DR-3 | 低 | 短絡順序の厳密契約公表+pin: 総数予算超過時はカテゴリ予算を**バイパス** (1/8 通過ならカテゴリ満杯でも AllowDecimated)・保護粒子も予算計上 (保護が far 粒子を飢えさせうる意図設計)・dist==max_d はカリングしない (厳密 `>`、ε 超過で CullTooFar・計上なし)・NaN 位置は両比較 false で**拒否しない通常予算評価**へ進む fail-visible |
| DR-4 | 観 | FNV-1a 間引きの厳密ピン化: 独立 Python シムで事前導出 (keep(12345,50,8)=F・tick 0..4 系列 [F,F,F,F,T])・連続 640 ids で n=8→**厳密 80**・n=4→**厳密 160** (均質分布)・adversarial (b) で検出空白 (id=5/6 の 2 値標本では 1/8→1/16 変化が区別不可) を発見→**呼出側レート census pin 追設** (80 vs 40) で強化 |
| DR-5 | 観 | 既存 kind_budget_enforcement の loose `matches!` を決定的着地点精緻化: keep(222,0,4)=false のため厳密 CullKindBudget・id=5 → AllowDecimated (FNV シム事前選定、fail-visible pin) + **捕捉 40 件目**: adversarial 復元手順の anchor が rustfmt による行分割で崩れ anchor assert 失敗のまま変体が golden へ流出しかけた → md5 で即時検知して完全修復 (復元 assert 二重化/流出後の即差替え手順へ強化) |
| DS-1 | 中 | **smaa::edge/blend の wiring 恒等証明**: 唯一の Rust 側呼出 full_graph_wiring:1701 は同一色 (aa) を 5 引数に与え (contrast≡0 → strength=0 → smaa_w=0)、戻り値自体 `let _ = (is_edge, smaa_w)` で破棄 — FSR1 の CH-1 注記と同じ「定数色不変性の実演」形で本経路の SMAA は描画に一切寄与しない (DM-2/DQ-1 と同型の恒等クラス 3 件目)。実効化は近傍テクセル実配線の設計判断のため引継ぎ |
| DS-2 | 低 | edge 厳密 bit pin 化: V/H strength=1.0 厳密・タイ (gx==gy) は厳密 `<` で horizontal=false・tie strength=|gx|=0x3ECCCCCC・lmax 厳密値・非タイ主勾配 0x3F4CCCCC (全て Python IEEE f32 シム=係数 f32 化で事前導出→照合) |
| DS-3 | 低 | NaN 伝播の位置非対称公表+pin: min/max NaN 脱落により center/n/s の NaN は完全マスク (gy=NaN→比較 false→strength=|gx| 有限) だが、e/w NaN は gx 経由で strength へ伝播 (masked strength 0x3DCCCCD0 厳密) |
| DS-4 | 低 | blend 境界契約厳密 pin: strength<=0 → 0.0 厳密・lm=0 で商=1 → clamp 0.5 (0x3F000000)・1/(1+0.5)→clamp 0.5・NaN strength → NaN 伝播 (clamp は比較 false で self 返却) |
| DS-5 | 観 | 閾値 2 分岐 (A'=floor 0x3B800000 支配/contrast 0x3C1374BC で発動 → strength 0x3A831270 厳密、B'=relative 0x3DBA5E37 支配で不発) の DQ-4 同族対蹠 pin |
| DS-6 | 観 | **contrast 通過でも strength=0 になりうる** directionless ケースの公表: 両軸ペアの内部差ゼロ (e==w ∧ n==s) なら全域 contrast が閾値超過でも (0.0, false, lmax) 返却 — doc「strength == 0 means no edge」との表現緊張を誠実化 (方向決定不能のため 0、blend は strength<=0 → 0 の安全側)。**捕捉 41 件目**: 初版シナリオ設計で両軸差ゼロを選んでしまい st_a==0 で失敗 (テスト赤が捕捉) → 公表 pin として昇華＋分岐 pin は aniso 場面に再設計 |
| DT-1 | 中 | **ssr::march の wiring 常時 miss 証明**: 唯一の Rust 側呼出 full_graph_wiring:1537-1568 は sampler に「AABB 内=0.0/外=∞」を与え、surf=0.0 では diff=0-travelled<0 (travelled>0) で hit 条件 diff>=0 が構造的に永不発 → march は常に None、かつ戻り値 `let _ssr_hit` で破棄 — 恒等クラス (DM-2/DQ-1/DS-1) とは別型の「常時 miss+破棄」ゼロ効果構造。実効化は G-buffer 深度実サンプラ配線の設計判断のため引継ぎ (GPU は ssr.wgsl 別経路) |
| DT-2 | 低 | march 境界契約の厳密 pin 群: hit 窓 [0, thickness] 両端 inclusive (上端 diff==thickness で hit (0,0,4.5)=0x40900000 / thickness 1 ulp 低下で下端 diff==0 hit (0,0,5.0)=0x40A00000)・max_dist は厳密 > (等値で sample 実行=コール 2 回 vs 1 ulp 低下で 1 回)・NaN ray fail-safe (全比較 false で誤 hit なし 32 走査)・step_size=0 原地判定/max_steps=0 無条件 None (全値 rq 事前導出照合) |
| DT-3 | 低 | reflect_dir 厳密 pin + ゼロ法線パススルー公表: 軸 (0,0,-1) / 斜 (0.6,-0.8,0) は \|i\|²≡1.0 で全演算厳密 (0x3F19999A/0x3F4CCCCD、往復も厳密復元)・ゼロ法線は d=0 → 正規化済み入射そのもの (0x3F800000)・**末尾 normalize 検出空白の補完** (adversarial (e) で発見): \|pre\|=0x3F7FFFFF に 1 ulp ずれる構成 i=(1,2,3),n=(0,1,0) で normalize 実効を pin (rq 導出 0x3E88D678/0xBF08D678/0x3F4D41B4) |
| DT-4 | 観 | Vec4 消費者ゼロの意図的保持を明記 (WGSL パリティ API 面、bloom/fxaa/atmospheric 他モジュールと同方針) |
| DT-5 | 観 | Rust/WGSL 表現差の公表+走査 pin: 空マーカーは Rust is_infinite() (±∞) vs WGSL SSR_INF=1e30 有限比較 (1e30 有限深度は Rust でも diff 経路不発で帰結同等だが判定経路差を記録)・WGSL uv 写像 pos.xy/res*0.5+0.5 は真の投影でない様式化・surf (view 線形深度) を along-ray 距離として扱う近似 — 消費者警告 |
| DT-6 | 観 | hit 窓が**一方向符号付き** [0, thickness] であることの公表: 典型 SSR の両側 \|diff\| 窓と異なり表面通過後 (diff<0) の再接近は拾わない — step=2.0/surf=5.5 で \|diff\|=0.5 を跨ぐ構成を miss pin として固定 (WGSL 側も同型であることを走査 pin で担保) |
| DU-1 | 低 | static_be::tick 境界契約の厳密 pin 群: 昇格猶予 40 tick (39→動的 / 40→昇格、初期 last_anim=0 基準)・interact 鮮度窓 = promote_after_ticks\*4 = 160 (159→降格 / 160→通過→昇格)・近距離 3.5 は inclusive (0x40600000 で降格=**Static→Dynamic 降格経路の実証**を兼備 / 0x40600001=3.5000002 で escape→昇格復帰、全値 rq 導出)・burst 間隔 20 inclusive 累積 (t=0/20/40 → score=3、間隔 21 でリセット 0、guard 到達で恒久動的)・**非開閉 tick では score 維持** (時間減衰なし設計の pin)・時計逆行 (now<last) は saturating_sub→0 で安全側動的化・**NaN camera_distance は全比較 false で近距離降格が不発し昇格側へ** (SSR の NaN miss fail-safe とは逆側 = fail-safe ではない旨誠実公表) |
| DU-2 | 低 | pos_pack レイアウト厳密 pin (x: bit63-38 / z: 37-12 / y: 11-0 の排他 3 領域、射影復元 pin 付): rq 導出厳密値 5 件 ((1,64,2)=274877915200=0x0000004000002040・(-1,0,0)=0xFFFF_FFC0_0000_0000 (u64 18446743798831644672)・(0,-1,0)=4095・(7,100,9)=1924145385572・x=2^25 は有効で 0x8000000000000000=2^63) + **領域外折り畳み衝突の公表 pin** (x=2^26≡0・y=±2048≡2048、MC 世界境界 ±30M < 2^25 内では単射だが契約として固定)。**捕捉 47 件目**: 初版テストで 3.5+ulp 復帰ケースの期待値を DynamicBlockEntity と誤記 (実装は正しく Static 復帰) → 初回実行のテスト赤が捕捉・実装一致へ修正 |
| DU-3 | 観 | wiring 構造公表: 唯一の Rust 側呼出 full_graph_wiring:1369-1387 は take(4)・kind 常時 Chest固定・opened=false, interacted=false, static_mesh_ready=true 固定・戻り値は let _mode で破棄 — ただし be_entries: HashMap への副作用 (mode 遷移) は永続するため恒等クラス (DM-2/DQ-1/DS-1)・常時 miss+破棄 (DT-1) とも別型の**「決定破棄・状態機械のみ進行」構造** (wiring 同型 soak pin: 200 tick 無操作で 4 エントリ全静昇格・4 区画キー全 distinct・dist 欠損エントリも昇格)。dist 欠損の unwrap_or(0.0) は近距離側 (0.0<=3.5) の安全既定だが、interact 無しでは鮮度窓が不発のため昇格は妨げない挙動の公表 |
| DU-4 | 観 | static_mesh_ready=false 時の契約公表: 現 mode を**保持**して返す (Dynamic 維持だけでなく **Static→Static 維持**もピン、捕捉 47 修正後の実装一致)。Static からの降格経路は burst/anim/interact の 3 系統のみで「メッシュ喪失による降格」経路は設計上存在しない旨の公表 |
| DU-5 | 観 | promote_after_ticks\*4 は u64 直接乗算: 既定 40→160 で不発だが、policy を 2^62 超に変更すると debug ビルドで overflow panic (release では wrap)。既定設定では到達不能のため契約記録のみ (fail-loud 文化に反しない self-contained な前倒し検討として公表) |
| DU-6 | 観 | closed_model_id 全表の完全 pin (6/6 種) + Other→chest フォールバック公表: 汎用 BE の closed モデル id はリソースパックに存在しないため chest に寄せる近似 — promoted_model が kind のみに依存する契約と併せて固定 |
| DV-1 | 観 | temporal_mesh_diff wiring 駆動形状の公表: full_graph_wiring:782-790 は毎フレーム chunk_keys **全件**を無条件 mark_dirty (sy=i%4) — 「変更検出」の本来フィルタは wiring に無く全件 dirty 駆動 (時分割キュー供給源としては一貫、BS-1 の責務範囲と整合)。同型 soak pin (3 フレーム×全件 mark→sorted drain→packed 変換) で実証 |
| DV-2 | 低 | wiring packed_key 厳密 bit レイアウト pin (cx 10bit<<20/cz 10bit<<10/sy 10bit): rq 導出値 ((0,3,4095)=4095・(5,1,5)=5243909=0x500405・cx=-1=0x3FF00000=1072693248) + 折り畳み衝突公表 (cx=1024≡0・sy=-1≡1023・cz=-1 の drive=3072) + **patch 駆動値 packed&0xFFF は cz 下位 2bit と sy 10bit の混在** (CI-2 誠実注記の pin 化、patch_for_block 契約 <4096 を構造的に満たす全領域走査 pin 付) |
| DV-3 | 観 | dirty map の値 (generation) は書込まれるが take_dirty 経路で**消費者ゼロ** (キーのみ返却) の保持明記 pin: map 内部値=直近 generation を実在確認の上、返却に値が現れない設計を誠実公表 (世代カウンタ自体は wave 69 BS で pin 済) |
| DV-4 | 観 | diff_section live 消費者ゼロの継続追認 (census 2026-07-26: full_graph_wiring は patch_for_block のみ呼出、BS-1 候補記録の更新) + 決定性 pin 強化 (全 4096 差異列挙は index 昇順 identity、境界 0/4095 厳密) |
| DW-1 | 低 | transform_svdag `permute_node` の unused_mut 警告根治: y ビットは Y 面 D4 変換で不変のため読み取り専用 (let mut y → let y)。opt-gfx lib 警告 9→8 機械照合 (警告 pin は枠外のため build 警告カウント照合で検証) |
| DW-2 | 観 | wiring 構造公表: full_graph_wiring:908-916 は SVDAG 再構築条件 (svdag.is_none() \|\| tick%600==0) 内で take(16) のみ insert_transform_aware し返り値を let _ = で破棄 — 「決定破棄・副作用 (canonical pool/base_dag 成長) のみ」構造 (DU-3 同型) + 先頭 16 ノード部分列挙制限の公表 (全ノードでない) |
| DW-3 | 低 | canonical 一意性の経路非依存 pin: orbit {oct0,1,4,5} の 3 メンバー (5,1,4) どれから挿入しても同 ID・同 canonical 形 (mask=1/children[0]=42、solo インスタンスでも一致) — insert_node dedup + orbit 閉包 + strict-min 列挙順決定性の連鎖を機械固定 (各 tag は permute 適用で canonical に到達する契約整合付) |
| DW-4 | 低 | y ビット不変性の直接 pin: y=1 octant (2,3,6,7) は全 16 変換で mask が y=1 領域に閉じ occupancy 保存、y=0 側も対称。領域 mask は rq dw_mask.rq で機械導出 (y=1=0b1100_1100=204/y=0=0b0011_0011=51、cover=255・disjoint=0 の assert 通過)。**捕捉 48 件目**: 初版は y=1 を 0b0100_0100 (oct2,6 のみ) と誤記 → テスト赤が捕捉 (実装は正しかった) → rq 導出 mask で根治 |
| DW-5 | 低 | **検出空白補完強化 pin** (wave 113/118/120 に続く 4 件目): adversarial (b) compose apply の作用順交換 (mirror 先行化) が全既存 pin で検出不能 (involution/4乗 pin は作用解釈不感) → **非可換ケース R90∘mirror_x の厳密 pin** (列挙順最初表現 (1,T,F)、手検算 (x,z)→(z,!x)→(!z,!x) 照合) + permute_node 2 段/1 段整合 pin + mirror 先行の別作用非一致 pin を追設し再 RED 達成 |
| DX-1 | 低 | **binary_greedy_meshing::greedy_merge_2d_pull (u16 mask 版) の分類整理**: 警告は lib ビルド dead_code だが実体は **テスト専用オラクルとして生存** (mod tests の旧実装対照 fuzz が参照、:520+ 等価性証明コメント「出力完全同一」) → 削除ではなく `#[cfg(test)]` 付与で is_opaque 系 (:21-23) と同型の「テスト専用保持」に整理、lib 警告 8→7 (directive⑦保持明記に整合) |
| DX-2 | 低 | idx(x,y,z)=x+16y+256z 厳密 pin: idx(15,15,15)=4095 (排他的終端)・全 4096 引数単射完全走査・座標復元 roundtrip (i%16,(i/16)%16,i/256) — svo/section_rle/section_compress/noise_upsample/voxel_cone_tracing/world_column_store/leaf_fast_path 等 10+ ファイルが共有する座標規約を固定 (全値 rq dx_idx.rq で assert 事前導出) |
| DX-3 | 観 | 消費者形状公表: 本番経路は render_pipeline:427 (mesh_chunk_column)・:438 (mesh_chunk_column_pull_world、mesh_section_y0 由来実引数)。frame_reference (3 箇所)・gpu_vertex_pull:52・chunk_mesh:182・frame_reuse:344 は参照レンダ/テスト経路 (census 実 grep 照合) |
| DX-4 | 観 | 等価性オラクル資産の棚卸し公表: 10 テスト体制 (pull u16 対照 fuzz・bitcols face_visible 対照 fuzz・edge 断面一致・12B 経路同系) は既存維持 — adversarial (c) で 5 RED (fuzz 4+flat_layer 1) を確認し検出力の現役性を機械実証。本 wave は doc+1 pin+属性整理のみで mesh 出力 (digest) は完全不変 |
| DY-1 | 低 | aokana insert_shallow_region の `mut dag` unused_mut 警告根治 (root_id 読取+move のみで可変操作なし)、opt-gfx lib 警告 7→6 機械照合、(d) mut 戻しで警告 7 復活+テスト不変の対偶確認 |
| DY-2 | 観 | wiring 実消費公表: full_graph_wiring:930 登録は ry=0 固定 (K-1)・:936 evaluate 結果は report.aokana_visible_regions への**カウント集計のみ**でリージョン選択 (実カリング駆動) に未接続 — 「評価実効・消費は集計型」(恒等/常時 miss とも別型の中間) の誇張なき公表 |
| DY-3 | 低 | リージョン座標スケール厳密契約群: coords*64 は 2^6 乗算のみで i32 安全域 bit 正確 (2^24→0x4E800000・-2^24→0xCE800000)・**min+64 の退化境界** (2^30 スケールで ulp=128 タイ偶数丸め → AABB 厚み 0、実害域 ≦2^20 では 8 ulp 正確)・**i32 乗算溢れ経路** (region ≥2^25 で debug panic/release wrap→符号反転 0xCF000000、wrapping_mul pin、DU-5 同型 2 件目)。全値 rq (dy_vals/dy_max/dy_wrap) 導出。**捕捉 49 件目**: pin 初版の (1<<25)*64 が自身の debug panic を照らし**実装上の overflow ハザードを発見** — テスト赤ではなく panic による捕捉、wrapping 形式で根治的 pin 化 |
| DY-4 | 低 | p-vertex 選択 `>=0.0` vs `>0.0` の**完全等価変異証明** (差は ±0.0 成分のみ、その寄与は ±0.0 で和・判定不変、NaN も同選択) — adversarial (a) 全緑で機械確認・検出不能は証明付きで誠実記録。検出担保は (a') n-vertex 反転で RED 3。斜め平面 pin 追設 (非軸平面の p-vertex 分岐経路を固定、rq 導出 24/-8) |
| DZ-1 | 低 | gpu_culling.rs DeviceExt 完全未使用 import 削除 (使途 0 箇所、census 実 grep) |
| DZ-2 | 低 | persistent_vbo_pool.rs 未使用 import 3 件削除 (Quantized12ByteVertex は mod tests :437 で独立 import 済・PackedPullQuad 使途ゼロ・debug tracing 使途ゼロ、DeviceExt は使用中で温存) |
| DZ-3 | 低 | **DrawIndexedIndirectArgs 同名 3 重複定義の統一**: gl33_compat.rs 独自定義 (CRLF 原生 309 行) を削除し `pub use crate::gpu_culling::DrawIndexedIndirectArgs` へ統一 (レイアウト同一 repr(C) 5 フィールド、Default derive は gpu_culling 側へ移設で等価性保持、真消費型は execute_indirect 版で azdo/full_graph_wiring 経路) — ambiguous glob re-export 警告根治・**捕捉 51 件目** (初版は原生 CRLF を保持した perl 編集に固執 → san ゲート 1 が変更内 CR を拒否し FAIL (原生 CRLF は引継ぎ一括 wave 対象だが変更ファイルは san 適格) → 当該ファイルを LF 正規化で根治・fmt 正準 0 維持) |
| DZ-4 | 低 | occlusion_query::build_vertices private 化 (消費者は自モジュール内 :716 本体+テストのみ、pub 公開面の実需なし) — OccVertex (pub(self)) との private_interfaces 警告根治、将来需要時の再公開方針を保持明記 |
| DZ-5 | 観 | **opt-gfx lib 警告 6→0 完全根治** 達成の機械照合記録 (api 側 13 件は別枠棚卸し)・HEAD 原生 3 ファイル (gpu_culling 161 行逸脱等、fmt 未適用の既往) へ fmdiff 正準形を忠実適用 (wave 文化の現逸脱 0 へ整合)・adversarial 対偶 3 ((a) gl33 戻し→ambiguous 復活・(b) pub 戻し→private_interfaces 復活・(c) import 戻し→unused 警告復活、全て build 照合で機械確認、復元後 0)・**捕捉 50 件目**: wave 125 報告の「1111 全緑」は機械値 1112 (1106+6) の誤記 → 全量再実行の test result で捕捉・訂正 |
| EA-1 | 低 | rsift-api 未使用 import 6 件除去 (worldgen `debug`・neoforge_registries `debug`+`warn`・neoforge_capabilities `debug`・networking `std::collections::HashMap`・runtime `info`)・worldgen `chunk_index` の未使用 `world_height` 引数を `_world_height` 化 (chunk_index は高さ非依存設計、呼出 4 箇所の冗長渡しを混入防止のため明示)・runtime `let mut reg` (advancements lock) の unused_mut 根治 (非可変利用のみ) |
| EA-2 | 低 | **異シグネチャ同名 2 重定義の ambiguous glob 根治 2 件**: ① lifecycle `ServerStartingFn` (= Arc&lt;dyn Fn()&gt;) → `ServerStartingCallback` rename (:66 type・:95 field・:124 register、消費者は自モジュールのみ。neoforge_event_bus 版 `ServerStartingFn` = Fn(&amp;ServerStartingEvent) の :182/189/199 と同名衝突) ② **mod_suite::modules → suite_modules へファイル mv** (fabric_api::modules (公式 Fabric 構造) との glob ambiguous 根治、`modules::`→`suite_modules::` 全 8 箇所+mod_suite/mod.rs:115 binding 文字列、git 記録は delete+add) |
| EA-3 | 低 | adaptive_perf `pick_best_gpu` に `#[cfg(any(test, target_os = "windows"))]` 付与 (呼出元 detect_gpu_fast_flagship/detect_gpu_heuristic の cfg(windows) ブロック+mod tests :750 のみ → 非 windows lib での dead_code 根治、DX-1 cfg(test) 分類と同型整理)・非 windows 対称 stub `read_registry_string`/`enumerate_display_devices` に `#[allow(dead_code)]`+保持明記 (呼出元は全て cfg(windows) 内のため非 windows lib では消費者ゼロ、directive⑦により削除せず対称性契約として保持)・+1 strict テスト `non_windows_stubs_return_empty_contracts` (stub が None/空を返す契約を cfg(not(windows)) で fail-loud pin) |
| EA-4 | 観 | HEAD 原生の **fmt 未適用逸脱へ正準形忠実適用** (neoforge_event_bus 等、DZ-5 と同型の既往逸脱)・**CRLF 原生 3 ファイル LF 正規化** (engine_caps 458 CR 行・mod_suite/mod 233・modules → suite_modules 51、HEAD 機械 grep 値) — 捕捉 51 と同型の「変更スコープ内 CR は san 適格」に基づく必要性駆動先行、`git diff --ignore-space-at-eol --numstat` で内容差分を照合 (LF 化自体は eol のみ) |
| EA-5 | 観 | **rsift-api lib 警告 13→0 機械照合** (opt-gfx lib (DZ-5) に続き 2 crate 目の完全根治)・adversarial 対偶 2: (a) stub 側 `#[allow(dead_code)]` 外し→never used 警告 2 復活・(b) `ServerStartingCallback`→旧 `ServerStartingFn` 戻し→ambiguous glob 警告復活、各 build 照合で警告復活を機械確認・復元 md5 VERIFIED・復帰警告 0・api テスト 49/49 全緑維持 (net +1 = EA-3 pin 追設) |
| EA-6 | 中 | **ゼロデイ級潜伏テスト欠陥の発見・修正**: engine_caps `sm69_requires_score_and_vram` 旧版は `assert!(probe.sm69_eligible)` を無条件要求、しかし `check_sm69_eligibility` は DX12 Agility (Windows+DXGI) を必要条件とし非 windows では常に not eligible → **Linux で構造的に必落ち**。bench.yml が `cargo test -p rsift-opt-gfx --lib` のみで rsift-api テストを走らせないため長期誰にも検出されず (orig 戻しで既往失敗を機械確定、本 wave 変更起因でない)。設計意図 (SM6.9 = DX12 Agility 依存 = Windows 専用) を公表し経路分割 pin 化: windows は eligible・非 windows は not eligible+block_reason に "DX12" 含有を全 PF fail-loud 固定・低スコア 8k は全環境で不可のまま不変。CI 軟点 (bench.yml が api テスト非対象) は棚卸し記録へ |
| EB-1 | 低 | full_graph_wiring HUD 統計バー色 `0xFF30_8040 + (i as u32) << 4` の**優先順位罠**: Rust は `<<` より `+` が強結合のため実評価は `(base + i) << 4` → alpha が意図の 0xFF (不透明) から **0xF3** へ化け base 上位 nibble 欠落 (rq eb_hud.rq 導出機械値 i=0 → 0xF3080400)。実害域は HUD スクラッチバーの色のみ (メトリクス系), base 定数が 0xFF alpha を明示する書き方から不透明意図と判定し `base + (i<<4)` に根治・ピン可能化のため純粋関数 `hud_layer_color` (:2095) へ抽出。+1 strict テスト (i=0..3 golden 0xFF308040/0x50/0x60/0x70 + 全 16 層 alpha=0xFF pin)。adversarial (a) 旧式戻しで 1 RED 機械確認 |
| EB-2 | 低 | corner_ao_from_palette の戻りタプル (u_sign, v_sign) は 3 分岐すべてで out_sign(face) と**常に等しい冗長値**かつ消費者ゼロ (`let _` 破棄のみ) → 5 タプルを 3 タプルに縮小 (slab_slots 削除と同型の純粋整理、実効符号は直下 face_out が out_sign(face) を直引きのため喪失なし、コンパイル中立照合) |
| EB-3 | 観 | decals 評価ループの**恒常空**を census grep (push サイト 0 件・公開登録 API 不在) で機械確認し旧コメント「登録 API 経由の実データがあれば」を虚偽として誠実訂正 — 結合点は directive⑦ で保持 |
| EB-4 | 観 | meshlet_cone の法線は i%6 巡回の 6 軸**合成**列 (実メッシュ面法線未接続、件数のみ chunk_materials 由来) — 旧コメント「実面法線クラスタ錐体カリング」の虚偽部分を誠実訂正 (錐体ビルド/visible 評価は実演のまま) |
| EB-5 | 低 | 消費者不在ローカルメトリクス 2 件削除: frb_above (camera 高さ比較カウント、集計後 `let _` 破棄のみ)・slab_base (確保前 used_bytes スナップショット、同) — slab_slots 削除前例と同型、挙動中立コンパイル照合 (gpu_arena.used_bytes は vram_used_bytes 集計で引続き実評価) |
| EB-6 | 中 | **ゼロデイ級 tools 欠陥 RB-1**: rspeed rq 字句解析の `&src[i..i+3]`/`&src[i..i+2]` **str スライス** が文法外マルチバイト文字 (日本語等) の char 境界でハードパニックし、設計上の fail-loud エラー「解釈できない文字」を bypass (orig 版で "panicked … byte index 35 is not a char boundary" を機械再現済、rq スクリプトに日本語 `//` コメントを初めて書いた実地で発見) → byte スライス比較に根治 (演算子は全て ASCII のため結果完全一致の挙動中立) + selftest ピン 3 件追加 (51→**54**: 日本語コメント受理/非 ASCII 字句エラー rc=2 帰還/日本語文字列受理)、`rspeed selftest` 0 FAIL・RQ.md 文法不変 |
| EC-1 | 低 | **BA-3 解消**: render_pipeline::frame の quad_budget 経路で cast_bytes_to_slice の失敗 (ラギッド/非整列) に `.map(to_vec).unwrap_or_default()` が被さり **全 quad を静寂空化して書き戻す** データ損失パス → pure 部 `apply_quad_budget_bytes` 抽出 (失敗時 bytes 無変更保持で bool 返却)+呼出側 fail-loud warn。生成規約上ほぼ到達不能だが「到達不能」を理由に損失を許容しない。+1 strict テスト (4→2 切詰め順序保持・budget 内不変・ラギッド 8n+1 で false+bytes 無変更)、adversarial 旧式戻しで **1 RED** 機械確認・復元 MD5-VERIFIED |
| EC-2 | 観 | DRS `internal_size` の評価結果は読み捨て (`let _internal`) — 内部解像度の変更は未還元で、ヘッダ行「(no resolution scaling)」の現行効果と整合する計測実演として誠実注記 (将来の解像度スケーリング接続用結合点として保持、directive⑦) |
| EC-3 | 観 | 材料引き当ては chunk_keys × pull_meshes の線形 find = O(n·m) — 両者とも数百スケールで現害は小さい (支配 tex 決定用途)。HashMap 化は冗長メモリとの実効見合いを要検討として棚卸し公表 |
| ED-1 | 低 | overdraw_sort::sort_front_to_back / overdraw_saved の `partial_cmp(...).unwrap()` / `unwrap_or(Equal)` — NaN 中心・NaN カメラが 1 つ混入すると partial_cmp None で **lib 内パニック** (adversarial 復元で panic 実演 RED 機械確認) → `total_cmp` 全順序化 (有限値で結果完全一致 = IEEE-754 bit 全順序、-0.0<+0.0 も決定的、NaN dist2 は最奥配置で決定的・panic なし、M-4 系堅牢化と同型)。+1 strict テスト (NaN 中心で [1,0] 最奥配置・NaN カメラ全 Equal で安定元順序・再実行同一性・overdraw_saved NaN 無 panic)、消費者 census: 本番は full_graph_wiring:704 (report.overdraw_order) のみ |
| ED-2 | 観 | early_z_shaded / overdraw_saved の実消費者はテスト/計測のみ (本番呼出なし、census grep) — 計算量 O(width × spans) 棚卸し、計測器+WGSL 実コンパイル検証資産として保持 (directive⑦) |
| EE-1 | 低 | simd_kernels_avx2::face_visible_bitmask の **x 未ガード** (z は :42 でガード済みの非対称) — `1u32 << x` が **x >= 32 で debug パニック / release では静寂にビット巻付き (x % 32)** → bit0 立ちの mask に対し誤 true を返し得た → `x >= 16 → false` ガード追加 (x ∈ [16,32) は旧結果も false で bitwise 同一、挙動変更域は x>=32 のみ、M-4/DU-5 系堅牢化と同型)。+1 strict テスト (x=16/17/31 false + x=32/33/64/usize::MAX 無 panic false + 有効域不変)、adversarial ガード除去で **実 panic RED** (:52) 機械確認・復元 MD5-VERIFIED。消費者 census: full_graph_wiring:1000/1002 (定数 3,4,5) のみ |
| EE-2 | 観 | 同モジュール消費者形状公表: greedy_mask_avx2/face_visible_bitmask の実呼出は full_graph_wiring の面可視サンプル 1 系のみ (census grep)。全アーキテクチャ bit 同一の既存契約 (ヘッダ歴史注記) と fuzz オラクル (spec_masks 別ループ形状) は現役確認 |
| EF-1 | 低 | async_chunk_io::lz4_roundtrip_store_load の**固定 sleep フレーク** (store→50ms→load→80ms 後に即 assert): CI 高負荷でワーカ (Store 書込/Load 読込) が未完了のまま判定に入り**断続失敗** (成功条件は「ready 非空 OR cached」だがワーカ未走なら両方空) → 5 秒 deadline ポーリング化 (成功条件不変・上限到達のみ失敗でワーカ異常の検出力維持、phase A は path.exists() 待機+Stored 排水、phase B は poll_ready ループ)。発見経路: c6e838c CI run 紅 (lib tests exit 101、4m51s) を受け時刻依存パターン走査で特定 (同 crate 唯一、ee 検証はローカルではフレーク再現不能 = adversarial 非検出として誠実記録、判定は新 push run の帰納確認) |
| EF-2 | 観 | **CI c6e838c run 紅の機械記録**: 45 連緑 (ea5387c..bb79031) の後 lib tests 失敗 (exit code 101 のみ機械判明、ログは results-receiver 接続遮断で取得不能・gh run rerun は "workflow file may be broken" 応答)。同 commit はローカル seal 全 6 ゲート PASS (1116/1116) で差分は simd ガードのみ → 環境/フレーク以外の説明根拠なし。根因帰属は「最も可能性の高い唯一パターン (EF-1)」に限定して誇張なく公表。のちの新 push run で帰納確認へ |
| EG-1 | 観 | restir::estimate の推定構造誠実公表: 真の RIS 推定 radiance·(w_sum/m)·(1/p̂(sel)) に対し本実装は **1/p̂ 正規化省略の簡約形** (p̂=target_pdf ∝ radiance 設計前提の輝度比近似)・単一流は厳密 RIS 選択確率 (w_i/w_sum) に従う・`combine` は受信側 p̂ 再評価なしの naive merge (文献上の実用近似・結合後推定は biased)・wiring 実消費は計測破棄のみ (full_graph_wiring let _restir_estimate、census grep) — doc 明記のみでコード不変 |
| EG-2 | 観 | micro_lod::downsample_palette 宛先写像の単射性公表+注入性ピン: サンプル点 k·f 限定で dst=x/f は一意 (衝突は構造的不出・「最終書込み勝ち」は仕様外)・実引数 {1,2,4,8} (census: full_graph_wiring 経路)・非出力域 (factor 非約数) は捨て近似・max_quads 消費者ゼロ保持 (directive⑦) — +1 strict テスト (充填セル数 f=2/4/8 → 512/64/8・f=3 → 216・値 7/0 のみ、rq eg2_downsample.rq 事前導出)・adversarial (a) step_by 変異 2 RED・(b) AO 閾値 3000→3001 変異 1 RED・復元 MD5-VERIFIED 2 回・捕捉 53 同型 2 件目 (edit アンカの fn 尾部飲み込み重複定義を grep 構造検査で即捕捉・修復、採番なし同型再発運用) |
| EH-1 | 中 | clustered_lighting wiring 座標フレーム不一致の構造公表+ヘッダ虚偽訂正: 旧「view frustum」主張 vs 実装は正規化単位立方体 [0,1]³ 一様分割の様式化参照実装、wiring はセクション局所 [0,16) 座標×輝度半径 1..15 をそのまま供給 (集計破棄のみ消費、DY-2 同型の中間構造) — soak ピン ((2,2,2) r=15 → 全 128,640 帰属/(15,15,15) r=1 → 全域ミス、rq 導出) + WGSL 件数集計パス注記、正規化実配線は設計引継ぎ |
| EH-2 | 低 | ClusterGrid::new fail-loud 契約化: 零次元は全クエリ静寂空化の堕落形 (aabb が 1.0/0=inf 経由 NaN 座標を返しうる) → dims>=1 assert + 総数 u64 事前検査で index の u32 wrap 折り畳み衝突を構造排除 (wrap 不出証明は積保証に帰着)・wiring 実引数は契約内で無影響 + should_panic 4 件 (S-3 同型) |
| EH-3 | 低 | ClusterGrid::index 範囲 assert (他クラスタへの静寂折り畳み拒否、内部呼出は常に範囲内でコスト無視級) + 非対称 3×5×7 全 105 掃引単射 + index(2,4,6)=104 の rq 導出ピン |
| EH-4 | 観 | aabb 厳密 bit 契約 (1/6=0x3E2AAAAB・5*(1/6)=0x3F555556←暗算禁止の実効例・6*(1/6)=厳密 1.0)・tangent 包含両方向 bits pin (r 1ulp 低下で反転)・境界面ライト両隣帰属 conservative pin (ちょうど 2 クラスタ)・非有限 drop / r*r=inf 全域支配 (inf<=inf) |
| EH-5 | 観 | assign_lights O(L×N) 全走査 (wiring 形状 ≈4.1M 球判定/tick) の棚卸し公表 — 範囲制限走査化は f32 境界判定 bit 同一性証明を伴う設計判断のため引継ぎ (EC-3 同型) |
| EI-1 | 中 | **新指令 §7 消化 1**: clustered wiring の座標正規化根治 (セクション局所 [0,16) → /16 で [0,1) へ f32 無丸め) + `_max_cluster_load` 破棄から FrameWiringReport 実フィールド 2 件 (cluster_max_load/cluster_lit_clusters、決定性比較集合入り) への**実消費者配線** — EH-1 の「評価実効・集計破棄」中間構造を解消、正規化形状ピン (実在クラスタ帰属 + 機械導出 golden 95,608) |
| EI-2 | 中 | **新指令 §7 消化 2**: assign_lights を O(L×N) 全走査から axis_range 範囲制限走査へ置換 (出力完全同一を 4 条件包含証明 (pad 包絡/飽和域/非有限/inf 飽和退化) で構造化、f32 端丸め 2^-22 包絡・i64 飽和は f64 予備 clamp 根絶・旧実装オラクル fuzz 突合で機械固定) — **全量スイート 199.53s → 11.08s の機械実測改善** (全走査が suite 支配コストだった実測証左、帰属は範囲制限+正規化の組、単独寄与未分解として誇大主張回避)・**捕捉 54**: r*r inf 飽和クラスの真の乖離 (naive 全域 vs 27) をテスト赤が事前捕捉→(iv) ガード根治・face ピン aabb 添字誤りも同時捕捉 (採番なし同型)・adversarial (e) 1 RED・(f) 2 RED・wiring /16→/8 は決定性不変で**非検出**の誠実記録 |
| EJ-1 | 中 | **新指令 §7 消化 3**: AO セクションの二重中間構造を根治 — 旧版は `opaque_ratio` 合成スライス (全サンプル高 ≤ center=1.0 で遮蔽が常に非発生 = gtao_occ ≡ 1.0 定数退化) + `_ = gtao_occ` 破棄 (EH-1 同型) → 真のパレット高さ場断面 (`section_heightfield_depth`: X-Z 各列の不透明最上 y+1 を 16 正規化、rq 確定刻み 1/16=0x3D800000) に根治し、FrameWiringReport 実フィールド 3 件 (gtao_occ/ao_halfres_mean/ao_halfres_min・det 比較集合入り) 書出しへ・加えて**全消費者ゼロだった deinterleave_ao を半解像度 AO パイプラインとして実配線** (新指令 §7 「接続する」選択、低スペック AO 半解像度化の品質監視)、AoParams radius=1.0/samples=8 の到達網羅を rq 機械確定 (歩幅 2..16px、16×16 全域)・GTAO 逆段差スライス閉形式 golden (1−atan2(2,1)/(π/2)=0x3E972028) が rq 導出 == 実測 bit 一致 (Rust f32::atan(2.0)≡libm atan2f(2,1) 機械確定本環境)・順方向段差 GTAO 退化構造の pin、順/逆 3.tick det 一致 |
| EJ-2 | 中 | **新指令 §7 消化 4**: async_compute planner 全消費者ゼロ (gpu_runtime 経由の wgsl_source 登録のみ) → wiring 経済モデル配線 (決定的作業量 proxy 写像: gbuffer=draw×0.008・shadow=res/1024 2 冪・cull=chunks×0.002・cluster_lit=lit×0.01・post=px×1e-6、PROXY_* 定数固定文書化・絶対 ms 非解釈で saved_pct 比率のみ意味を持つ誠実注記) → report 実フィールド 3 件 (async_overlap/pipelined/saved・det 比較集合入り) + 完全装飾 Vec3/Vec4 (全消費者ゼロを機械 grep 確定) 削除・**捕捉 55**: `Iterator::sum::<f32>` 空集約の **-0.0 bits 0x80000000 を透過**する実装詳細 (rustc 1.94.1 zeroprobe 機械確定、(-0.0).max(-0.0)=-0.0) を plan/overlap 全体で .max(0.0) +0.0 正規化により根治 (テスト bits pin 赤が捕捉、det 比較 toolchain 依存の構造排除)・empty 集約 golden (overlap=2.0・pipelined=2.0・saved=3.549385=0x40632920) を rq 閉形式導出・planner saved_pct 厳密 bits (18.181818=0x4191745D ・saved=100=0x42C80000)・**rq 事前捕捉**: 手書き十進 0.295408 誤表記を rq assert で捕捉・暗算 0x41500000(13.0) 誤りを rq ej_empty_pin で捕捉 (0x42C80000=100 正値、共に採番なし・rq 段階 self-catch) |
| EJ-3 | 観 | deinterleave_ao 誠実注記監査: (1) ao_pixel は回転直交ベクトル **(rx,ry),(−ry,rx) 片側 2 方向のみ**の探索 (反対位相未探索 = 方向非対称バイアス簡易形)、(2) denoise は**最外周 1px 帯を処理せず**再インタリーブ直値透過、(3) 奇数寸法 index 安全 (hw=(w−1)/2 でも 2x+u ≤ w−1 恒成立、panic はバッファ長不足のみ fail-loud)、(4) cost_ratio は 1/4 画素×サンプル比の単純積名目見積 の 4 項目 doc 明記 + strict 4 件 (奇数寸法安全/境界透過 golden (内部 0x3EA00000=0.3125=5/16 exact rq 導出)/eps tight 0.9 raw vs loose (0.9+8×0.5)/9=0x3F0B60B6 差分検出/全エア 1.0 bits exact)、**初版 boundary pin 設計誤認 ((0,w-1) 改造値未反映行、テスト赤捕捉→適正 0.25 を機械 golden、採番なし)** |
| EJ-4 | 低 | AO 検出空白の pin 強化: adversarial (a) 高さ場全列 −1 uniform shift は AO horizon が差分オペレータ (dz のみ関数) およびフラット断面経路とも不変で**構造的非検出**を誠実確定 → ヘルパ出力レベル自体の strict pin (0x3E800000/0x3E000000 census 128+128・y=15 頂上 0x3F800000・空列 0 exact) で変異 RED 化 (再変異 1 RED 実証)、wiring step golden (順 0x3F67FFBA/0x3EF083B1・逆 0x3F67F065/0x3F17FB8C)・adversarial 結果 (a)強化後 1 RED・(b) PROXY_POST 係数 2 RED・(c) planner post 脱落 5 RED・(d) AO radius 2 RED・(e) denoise center 脱落 2 RED・wg WGSL `QueueTag` struct 未使用は棚卸し公表 (wgsl 変更は gpu_runtime コンパイル検証経路に影響するため本 wave 非変更の項目で構造公表のみ) |
| EK-1 | 中 | **新指令 §7 消化 5**: LEO wiring の「ring 内容も payload も未消費」中間構造を根治 — 旧変異は allocate+VecDeque 蓄積+4,096 上限 pop_front のみで decode/payload 全未参照 (wiring refs=3 の見せかけ配線) → **ring 維持と同期して tag を 8 スロット集計 (`leo_tag_dist: [u32; 8]`、pop は decode した旧 tag をデクリメント、Σ==ring len 不変式 debug_assert 常時維持) + payload(=tick) を `get_payload` Option 版で実読出し** → FrameWiringReport 実フィールド 2 件 (leo_tag_dist/leo_latest_tick・det 比較集合入り) の実消費者化。供給関数を pure fn `leo_occupancy_tag` に抽出し u8 wrap 境界を機械 pin (0→1/1/3/7/255→7/256→1/258→2/65535→7/MAX→7、usize as u8 mod 256 truncate) |
| EK-2 | 低 | `get_payload` の OOB 静寂 0 返却 (fail-soft) を `Option<u64>` 化に根治 — 「無効参照 == 0 (有効 payload 0 と混同可能)」の曖昧さを「None」と明確分離、wiring 消費は `expect("allocated index is in range")` で fail-loud 契約維持 (S-3 同型)、module strict pin (OOB=usize::MAX → None、within → Some(0x12345678)) |
| EK-3 | 観 | LEO 誠実注記 4 項目: (1) while ループ高々 7 回終了 (mod 8 回転の自明帰着、fuzz pin padding<=7 で機械カバー)、(2) payload は index からは decode 不能 = アドレス埋込みの正しい言明、(3) **padding 判別の曖昧性** (idx%8==0 の tag=0 ノードと dummy 0 を index だけでは区別不能 → tag=0 を「未占用区画」予約 + wiring が tag を 1..=7 に制約する契約規約)、(4) pool 非 shrink の cumulative model 明示 + 4 strict テスト (tag 8 種 allocation 正規性・padding <=7 fuzz (7,0,1,7,0,2)・decode の payload 非依存代数・option OOB) |
| EK-4 | 低 | ring 削除経路の検出空白を補完: **ring>4096 飽和 strict (4,097 tick) を新設** (旧 pop_front 経路は 601 tick det では未経由) → distribution 飽和+pop decode 対称減算の不変式を golden 化 (adversarial (b) で本経路のみの 1 RED を機械検証)。adversarial 5 系統: (a) dist 維持脱落 10 RED・(b) pop 減算脱落 1 RED・(c) min(7)→6 boundary 1 RED・(d) decode %8→/8 3 RED・(e) get_payload Option→unwrap_or(0) revert 1 RED、復元 MD5-VERIFIED + 変異復元忘れ 1 件を md5 照合が即捕捉 (採番なし同型) |
| EL-1 | 中 | **新指令 §7 消化 6**: `TbdrHints` unit struct 消費者完全ゼロ (crate+workspace 全体 grep 使用 0 機械確定) を根治 — 状態なし・メソッドは pub const 参照返却のみで付加価値ゼロの装飾ラッパ → free fn `wgsl_source()` 様式統一 (ssr/bloom/cas 他 20+ モジュール同型) + struct 削除 (EJ-2 Vec3/Vec4 完全装飾削除先例準拠) + gpu_runtime:92 登録を free fn 経由に一本化 (WGSL 取得の単一公式アクセスポイント化)。strict: free fn ≡ const 同一内容 pin + "No GPU shader required"/"transient" 含有 pin (naga 空受理に代わる内容担保) |
| EL-2 | 低 | TbdrPass 真理値表の検出空白補完: 旧テスト 3 行 ((T,F)/(T,T)/(F,F)) で **(F,T) 行欠落** → strict `truth_table_exhaustive` で bool 4 行全列挙。rq 事前導出: or 変異は (T,T)(F,F) の **2 行**差異・否定脱落は (T,F)(F,T) の 2 行差異 — 初稿は or 変異 diff を「1 行」と暗算誤記し **rq assert が事前捕捉** (rq 段階 self-catch 採番外、diff_count==1 失敗→正 2) |
| EL-3 | 観 | tbdr_hints 誠実注記 4 項目 (module doc 明記): (1) 判定 conservative (writes=false の load-op clear のみアタッチメントは理論上 transient 化可能だが現契約非対象=Persistent 安全側)、(2) wiring 消費は初期化時固定 2 パターン定数入力の info! 評価 (畳み込み可能なハードコード契約・実パス構造と動的接続なし)、(3) **TBDR_HINTS_WGSL は GPU シェーダーではない** (自身が宣言) のに all_wgsl_sources=「実シェーダー一覧」登録で naga 空受理=検証実効ゼロ (分類実態公表・除去は naga 経路+wiring 連結 pin 波及のため設計引継ぎ)、(4) lazy_allocated ≡ (usage==Transient) 同値委托 |
| EL-4 | 低 | FRB `wgsl_source(&self)` の self 不使用装飾レシーバ+消費者テストのみの中間構造を根治 — free fn 化 + gpu_runtime:113 を free fn 経由化 (構造体/new/Default は wiring が max_dynamic_voxels 保持・wiring:952 cap 実消費のため維持)。strict: free fn ≡ const pin・new(7)=7/Default=65536=2^16/new(1<<20)=1048576=2^20 (rq 導出)・cap 写像 (1048576→128/65536→128/7→7) pin・既存 1 テストは呼出形のみ機械追従 (意図不変)。誠実注記: Default 65536 vs wiring 1<<20 の差は意図的 (CG-6 公表構造維持)。adversarial 5 系統: (a) or 変異 3 RED・(b) 否定脱落 3 RED・(c) lazy `==`→`!=` 4 RED・(d) gpu_runtime 参照 revert **非検出** (同一 &str 機能等価・census grep のみ検出経路、誠実記録)・(e) free fn 返却 `""` 破壊 1 RED (strict 内容 pin のみ)、復元 MD5-VERIFIED (tbdr d59d37ee・frb fa24d9ba・gpu 7c7c6fb7) |
| EM-1 | 中 | **捕捉 56: lockfree_vram_cache sentinel 衝突バグ** — 空スロット初期値 u64::MAX と pack_key(-1,-1)≡0xFFFFFFFFFFFFFFFF が同一ビット列 (rq 事前導出: lo/hi 共 0xFFFFFFFF 一致) → 空キャッシュで lookup_or_insert(-1,-1) が **Hit=true 誤報+挿入 skip** (Minecraft 負チャンク座標は通常出現、メッシュ欠落のまま hit 扱いの実害)。TDD: strict テストで修正前 RED 機械実証 → `occupied: Vec<AtomicBool>` 独立占有ビットで根治 (pack_key 写像不変・key 書込み Release 後 true 化・読側 Acquire true 観測⇒key 確定、単一 writer linearizable)、根治により key 値域 u64 全 2^64 利用可能。wiring の hits 統計値は真値に変化しうる (報告誠実化・構造不変) |
| EM-2 | 低 | lockfree_vram_cache new() fail-loud 契約化: max_slots=0 は miss 経路 `% 0` ゼロ除算 panic (EH-2 同型堕落形・現行は構築成功し lookup で初めて panic=TDD RED 実証)・max_slots>u32::MAX は slot_idx: u32 truncate 折り畳み衝突 → new() 先頭 assert 2 件 (割当前評価) + should_panic 2 件。**new_over_u32 は修正前 RED 実証を意図的非実施** (現行 with_capacity(4,294,967,296)≒48GB 割当試行の OOM/abort 危険、根治後 assert で割当前停止する形でのみ GREEN 実証、誠実記録) |
| EM-3 | 観 | lockfree_vram_cache 誠実注記 4 項目 (module doc): (1) Hit 探索 O(N) 線形 (wiring 65,536 slots×chunks/tick・handle `_h` 破棄で GPU 経路未配線=CG-6 同型・未計測推測ラベル)、(2) 「lock-free」精確化: CAS なし・fetch_add round-robin は wait-free・write-write race benign・wiring 単スレッド駆動、(3) generation hit 不変/evict +1 (ABA 検知)・u32 wrap 2^32 で一周 (実害域外 pin)、(4) hits/misses Relaxed 統計 (報告目的適合) |
| EM-4 | 低 | lockfree_vram_cache strict pin 6 件 (pack_key 単射 4 境界・(-1,-1)≡u64::MAX・(1,0)=1/(0,1)=1<<32・非対称/FIFO evict 順・再参照 miss→再挿入 evict 連鎖・hits 0/misses 6 golden/gen hit 不変・evict +1・集計 pin、全厳密値 rq em_vram.rq 事前導出済)。adversarial 5 系統**全 1 RED** (各 pin 検出範囲正確に独立): (a) occupied 除去→sentinel テストのみ・(b) evict %max→%1→fifo pin のみ・(c) hit gen +1 化→gen 不変 pin のみ・(d) pack <<32→16→pack 単射 pin のみ・(e) assert 除去→should_panic のみ。(a) 初回 sed がコメント+if 行 3 行削除で構文破壊→grep 構造検査で即捕捉・edit_file 整流 (捕捉 40 同型採番外)、復元 MD5-VERIFIED 5 回 (lvc 3363896e 三重照合) |
| EN-1 | 中 | **新指令 §7 消化 7**: wiring `_subgroup_reduced`/`_subgroup_mask` の `_` 破棄中間構造 (評価実効・消費なし) を根治 — FrameWiringReport 実フィールド 2 件 (`emissive_high_mask: u64` intensity>8.0 ballot / `subgroup_wave_sum_max: f32` wave 集約 max) 接続 + det 比較集合 2 assert。消費設計: 空 reduce→None→+0.0・全 -0.0 も `.max(0.0)` +0.0 正規化 (捕捉 55 同型) で report 値は常に +0.0 域。golden: empty→mask 0/max=+0.0=0x00000000、chunked (全 id=1→light=1%16=1>0 全ボクセル発光 cap 32 で 32 灯×1.0)→単一 wave sum 32.0=0x42000000 (rq 導出)・lvl=1≤8.0→mask 0 (EJ golden 2 テスト追記) |
| EN-2 | 中 | subgroup::Vec3/Vec4+Add/Sub/Mul trait 実装 (**~60 行**) の crate+workspace 消費者完全ゼロ完全装飾を削除 (grep 使用 0 機械確定・EJ-2 完全同型・保持不可能証明: 消費者ゼロ純粋データ型で配線価値なし) + `use std::ops` 整理。残参照 0 (削除経緯 doc のみ) |
| EN-3 | 観 | subgroup 誠実注記 4 項目 (module doc): (1) reduce は f32 非結合の wave 内 index 順逐次=GPU 実 subgroup reduce (順序実装依存) と bit 一致保証なし (rq 機械導出: [1e20;32] 逐次 0x632D78EB vs 一括乗算 0x632D78EC の **1 ulp 差異**・1e20 wave で 1.0×31 個は ulp 未満全消失 0x60AD78EC 不変)、(2) ballot **j≥64 静寂切捨て** (u64 写像域外・現行 wiring cap 32 で未到達) pin 済、(3) WAVE_WIDTH=32 固定 (AMD wave64 別定数要)、(4) Vec3/Vec4 削除経緯 |
| EN-4 | 低 | subgroup strict 5 件 (reduce golden bits 部分 wave 境界 33/64/65/空・**逐次丸め順序 pin**・**順序消失 pin**・ballot 65 lane 切捨て・WAVE_WIDTH 契約、bits 全 rq 導出 496.0=0x43F80000/32.0=0x42000000/1520.0=0x44BE0000/64.0=0x42800000)。adversarial 6 系統: (a) wave_end min 除去 2 RED・(b) ballot j<64 ガード除去 1 RED (`1u64<<64` shift overflow panic を pin 捕捉)・(c) 初期値 1.0 化 3 RED・(d) wiring reduce max→min **非検出 (全量 1168 緑)** = cap 32 単一 wave で out 全要素同一の構造的非検出 (公表)・(e) `.max(0.0)` 除去 **非検出** = intensity=u8 as f32≥+0.0 で -0.0 構造不出 (防衛仕様として公表)・(f) threshold 8.0→0.5 1 RED (chunked mask 0→0xFFFFFFFF のみ)。採番外 2 件 ((e) perl 複数行置換未適用・(f) sed インデント不一致、各 grep 構造検査で即捕捉→edit_file 整流)、復元 MD5-VERIFIED (subgroup 5b47277c・wiring 8d93abeb) |
| EO-1 | 中 | **新指令 §7 消化 8**: wiring `_casts`/`_caster` (固定引数 24.0) `_` 破棄中間構造を根治 — 全クアッドを仮想 caster として (x,z) ノルム proxy (sqrt(x²+z²)) を casts_shadow/caster_lod へ供給し report 実フィールド 2 件 (lod 0..3 分布 `shadow_caster_lod_dist: [u32;4]`・`shadow_casters_culled: u32`、不変式 Σdist+culled==クアッド数) へ実消費 + det 比較集合 2 assert。誠実注記: proxy は真のスクリーン投影寸法ではなくワールド (x,z) ノルム (ビュー投影未接続)。golden (rq 機械列挙): chunked 24 点→culled=8/cast=16 全ノルム<16→dist [0,0,0,16]・empty→全 0 |
| EO-2 | 低 | shadow_lod `wgsl_source(&self)` self 不使用装飾 (gpu_runtime は const 直接・メソッド消費テストのみ) を free fn 化 + gpu_runtime:100 単一公式経路化 (EL-4 同型)。WGSL は 33 行の実 @fragment シェーダー (tbdr 型非シェーダー問題非該当を機械確認) |
| EO-3 | 観 | shadow_lod 誠実注記 4 項目 (module doc): (1) wiring coverage 供給 draw+16≥16→clamp(1)→shadow_res≡2048 定数退化 (EJ-2 相互参照)、(2) NaN coverage は clamp 透過→round→clamp→`as u32` 飽和 **0** (契約域外静寂退化) pin・NaN px は casts=false→lod=3 一貫除外、(3) caster_lod 境界全 `>=` 閉区間・4≤px<16 は casts=true かつ lod=3 共存形、(4) free fn 化経緯 |
| EO-4 | 低 | shadow_lod strict 4 件 (解像度 golden 704/1152/1600 全整数 exact rq 導出・端点/NaN 飽和 0・NaN/境界閉区間全閾値・new≡default+4.0=0x40800000+wgsl identity)。adversarial 5 系統: (a) min_caster 4.0→0.0 **4 RED** (boundary+new_default+既存 tiny+chunked golden)・(c) >=256→128 **1 RED** (boundary pin のみ・wiring 全 lod 3 非検出=module strict 検出経路)・(d) clamp→max/min NaN 被覆 **1 RED** (NaN 飽和 0 pin)・(e) wgsl revert **非検出** (機能等価・EL-1(d) 同型公表)、復元 MD5-VERIFIED (shadow 3346220d・gpu 253d6798) |
| EO-5 | 低 | shadow proxy 検出空白補完: chunked_inputs では |x| 変異 (z 成分除去) が culled 判定 24 点全一致 (rq eo_adv_b same=24/diff=0) で golden 非検出の構造 → z 寄与で判定反転する点 ([3.9,1.0,1.5]: |x|=3.9<4 だが sqrt(17.46)≈4.178≥4、rq bits 0x4085B668) を供給する wiring strict 新設で proxy 成分構成を golden 化 → adversarial (b) proxy |x| 化 **1 RED** (本 pin のみ検出=補完が正確・chunked は予測どおり不変) で実証 |

---

## 特記事項

1. **ゼロデイ級 (本プロジェクトで初めて発見された潜伏実害) の代表例**:
   M-4 (bytemuck アライメントパニック)、S-1 (RCAS ぼかし偽装)、S-2 (深度
   透視補正)、AI-1 (アライン未達成)、AX-1 (二重 release 無防備)、
   BG-1 (SVO trace 意味論スタブ)、BL-1 (Perlin 全定数化)、
   BZ-1 (確保失敗リーク)、BX-1 (中間ヒット誤タグ)、
   CF-1 (転置射影)、CF-3 (complete-dead 三角形判定)、
   CG-1 (転置フラスタム抽出・コモンモード不発)、
   EA-6 (SM6.9 資格テストの Linux 構造的必落ち・DX12 Agility 必要条件の
   潜伏、CI が api テスト非対象のため長期未検出)。
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
   (2 wave ずつ想定)、
   base 64294c6 時点で 131 テキストファイルが CRLF 含有 (= 原生・本セッション起因でないと機械判定。16,866 CR 行/120 ファイルを一括で触る大差分 + .gitattributes 設計が要るため LF 正規化は専用 wave で実施)、
   bench.yml が rsift-api テスト非対象 (EA-6 で発覚した CI 軟点。api crate を
   CI テスト対象に含めるかどうかは Actions 分数との兼ね合いのため
   ユーザー判断へ。wave 127 時点 13→0 達成の警告 0 状態は seal で担保) の引継ぎ。
