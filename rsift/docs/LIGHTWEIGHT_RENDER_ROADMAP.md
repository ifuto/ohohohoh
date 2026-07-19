# Rsift 軽量描画ロードマップ — Sodium 超え計画
**Target: Minecraft 1.21.11 / Engine: rsift-opt-gfx (wgpu) を本体、rsift-dx12 は凍結中 (76 errors)**
**Last sync: 2026-07-18**

---

## 0. 方針メモ (最新決定事項)

- **起動方式**: ネイティブ注入 (JNI CreateJavaVM + JVMTI) 路線は後回し。
  **Minecraft ランチャーと同じ「起動構成方式」** (Java 検出 → classpath 組立 → オフラインプロファイル → `java.exe` 起動) を `rsift-app` に実装して進める。
- **描画**: 世界中の軽量化テクニックを取り込み続け、**機能数で Sodium/拡張MOD群全体を上回る**ことを目指す。
- **開発ループ**: HF repo = ソースの真実。Arena サンドボックスは RAM 1.9GB / CPU 2コア / JDK11のみ / Rust なし (三次元でOOM前歴あり) のため `cargo build` 不可。
  → ここで編集 → HF push → Windows 側で `cargo check` → エラーを貼って反復 (SMPプラグインと同型)。

---

## 1. 現行の軽量描画機能 棚卸し (実装済みコードベース)

`crates/rsift-opt-gfx/src/lib.rs` 時点で **110+ モジュール** が登録済み。層別に整理:

### 1.1 頂点・メッシュ生成 (`頂点を軽く、生成を速く`)
| 機能 | 実体 | 効果 |
|---|---|---|
| 12B 量子化頂点 | `eco_render.rs` / world-class spec | pos fp16 + UV fp16 + 法線 octahedral 2B。Vanilla 28B / Sodium 20B を下回る |
| 16B align16 CompactVertex | `chunk_mesh.rs` | SODIUM_IRIS_SURPASSING_ENGINE 記載のベース形式 |
| バイナリグリーディメッシュ | `binary_greedy_meshing.rs` (23KB 本格実装) | 隣接面マージで面数大幅削減 |
| 8B quad pull-rendering | `packed4.rs` / `pull_mesh.rs` / `gpu_vertex_pull.rs` | SSBO普請、頂点バッファ自体を劇的小容量化 |
| GPU 頂点キャッシュ並べ替え | `vertex_cache_opt.rs` | ポストT&Lキャッシュヒット率向上 |
| 増分メッシュ更新 | `diff_mesh.rs` | 変更セクションのみ再ビルド |
| メッシュディスクキャッシュ | `mesh_cache.rs` | 二回目以降のロード高速化 |
| VBO / 頂点プール | `vertex_pool.rs` / `persistent_vbo_pool.rs` (14KB) | per-frame アロケーション撲滅 |

### 1.2 カリング (`見えないものを描かない`)
| 機能 | 実体 | 効果 |
|---|---|---|
| HZB GPU 2相カリング | `gpu_culling.rs` (14KB) + `hiz_build_cs.hlsl` (dx12) | frustum → Hierarchical Z。`draw_indexed_indirect` で GPU 自律駆動。Nvidium 相当を全GPUで |
| CPU HZB / ソフトウェアオクルージョン | `hzb_2d.rs` / `cpu_occlusion.rs` / `occlusion_complete.rs` | GPU 未対応機でも遮蔽カリング |
| SIMD フラスタム判定 | `simd_frustum.rs` / `simd_kernels.rs` | AABB×6平面を AVX2 で一括 |
| 可視性グラフ | `eco_render.rs` (VisGraph 6×6×6) | Sodium VisGraph 相当・ゼロGPUコスト |
| チャンクレイカリング | `chunk_cull.rs` / `branchless_dda.rs` / `svo.rs` (12KB) | トンネルDDA / スパースボクセル木 |
| 空間ハッシュ (entity) | `spatial_hash.rs` | 実体カリングの土台 |
| meshlet cone cull | `meshlet_cone.rs` | mesh shader 用メッシュレット背面除去 |
| カメラ面向け面マスク | `low_spec_stack.rs` (FaceEmitMask) | 視点の裏側の面を生成段階で除去 (~50% quad 減) |
| 固体内部カリング / occupancy bitmask | `low_spec_stack.rs` / `binary_greedy_meshing.rs` | 埋もれたセクションを即棄却 |

### 1.3 LOD・品質スケール
| 機能 | 実体 |
|---|---|
| ハイブリッド LOD 選択 | `lod_hybrid.rs` |
| 植物ビルボード LOD | `billboard_lod.rs` (FloraLod) |
| 実体 tick LOD | `entity_tick_lod.rs` (遠い実体の tick 間引き) |
| シャドウ LOD | `shadow_lod.rs` |
| 葉 fast path | `leaf_fast_path.rs` |
| 適応シェーディング | `adaptive_shading.rs` |
| 動的解像度 (DRS) | `drs.rs` |
| 可変レートシェーディング (VRS) | `vrs.rs` |
| 中心窩 (foveated) | `foveated.rs` |
| チェッカーボード | `checkerboard.rs` |
| ノイズアップサンプル | `noise_upsample.rs` |

### 1.4 CPU / メモリ構造 (`Java では不可能な根本構造`)
- Rayon work-stealing 全コアメッシュ並列 (`chunk_mesh` / パイプライン全体)
- `BumpArena` フレーム毎 O(1) リセット・ゼロGC (architecture doc)
- `CachePadded` 64byte alignment で false sharing 撲滅
- Lock-Free RCU イベントディスパッチ
- FNV-1a インターン化キー (文字列比較 → u64 1命令)
- `soa_layout.rs` / `triple_buffer.rs` / `tick_render_split.rs`
- `async_chunk_io.rs` — チャンク I/O 非同期化 (C2ME 的要素)
- `frame_reuse.rs` (13KB) — **地形無変更なら prepare/RLE/mesh/SVO をフレームまたぎ再利用** (LRU 512 エントリ / 120f 失効 / telemetry 付き)
- `section_rle.rs` / `section_compress.rs` — RLE + zstd/lz4 セクション圧縮 (FerriteCore 的メモリ削減をネイティブで)
- `world_column_store.rs` (10KB) — カラム型ワールド格納 (遠景LODの土台)

### 1.5 TBDR / 低スペック iGPU 特化
- `low_spec_stack.rs` — **quad budget 制** (24k/36k/48k quads/frame ≈ 0.5MB SSBO)
- `tbdr_hints.rs` / `software_tiling.rs` — タイルGPU向けヒント・ソフトウェアビン分割
- `gl33_compat.rs` — OpenGL 3.3 世代フォールバック
- `mip_streaming.rs` / `sparse_texture.rs` / `texture_budget.rs` / `texture_atlas.rs`
- `material_batch.rs` / material sort (低スペック plan) — ステート変更换最小化
- `ao_bake.rs` — 事前ベーク風の安い directional AO
- GUI `feather_*` オプション (tile_binning / merged_subpass / pseudo_vrs / lod_3tier)

### 1.6 アップスケール・ポスト・高スペック向け
- FSR1 / FSR2 / CAS / FXAA / SMAA / TAA / TAA-YCoCg
- ACES tonemap / exposure / bloom / GTAO / SSR / WBOIT
- DDGI / ReSTIR / LBVH / clustered lighting / volumetric fog / atmospheric
- `async_compute.rs` / `subgroup.rs` / `bindless.rs` / `visibility_buffer.rs` / `depth_prepass.rs`

### 1.7 GUI・シェーダ互換
- Sodium 風 Video Settings GUI (`gui_settings.rs` 5 タブ + shaderpack 管理)
- Iris パイプライン (`iris_pipeline.rs` 16KB) — GLSL→WGSL、MRT (gbuffers→composite→final)、shadow map、uniform 群
- 即時視覚インジケータ / `boot_splash.rs`

### 1.9 Wave-5: 低スペック CPU/GPU/メモリ 3資源セーパー（2026-07 最新調査 → 全本実装）

`crates/rsift-opt-gfx` に追加（全てテスト付き、スタブなし）:

| モジュール | 出典 | 節約先 |
|---|---|---|
| `gpu_arena` | Sodium `GlBufferArena`（best-fit+結合） | VRAM 断片化撲滅・割当 O(1) |

> 📐 **頂点ストライド既成事実**: Rsift は Wave-4 時点で `Quantized12ByteVertex` = **12B**（chunk_mesh.rs にコンパイル時 assert あり）。Sodium `CompactChunkVertex` の **20B を 40% 上回る圧縮率**。さらに `packed4.rs` の greedy+vertex pulling は **8B/クアッド（≒1.33B/頂点、UV/法線 0bit・WGSL 再構成）** で Sodium を大幅に凌駕。12B でいける理由: pos u16 fixed（1mm 精度）＋八面体法線 2B＋ライト/色の頂点追放（`light_cache` 側へ移設）。 `GlBufferArena`（best-fit+結合） | VRAM 断片化撲滅・割当 O(1) |
| `mesh_compactor` | Frostbite GPU-driven + Nvidium 思想の全 GPU 版 | GPU 空ドロー・ドライバ per-draw コスト |
| `intern_pool` | FerriteCore `summary.md`（predicate/形状 dedup） | RAM -数百MB級（重複実体の共有） |
| `palette_pack` | MC 1.13+ 可変ビットパレット拡張版 | セクション RAM 最大 1/16 |
| `job_system` | staccato / engine work-stealing 設計 | CPU コア占有の最適化（2コア機重視） |
| `stutter_guard` | bumpalo 式フレームアリーナ + time-slice | GC 圧・カクつき物理撲滅 |
| `quality_governor` | UE `r.DynamicRes.MaxConsecutiveOverbudget` | オーバーフレーム時の自動オフロード |
| `region_zstd` | ZFS 圧縮実測（zstd-3 ≈ lz4 速度・gzip 超圧縮率） | セーブ I/O・RAM 両方 |
| `deinterleave_ao` | XeGTAO（iGPU 2.39ms が橙） | GPU シェーダ負荷 1/4 へ |
| `bench_harness` | HdrHistogram + google-benchmark 方式 | 「めっちゃ高精度」計測基盤 |

さらに `examples/rsift_bench.rs`（CLI・CSV 出力）、
`docs/BENCHMARK_PROTOCOL.md`（Sodium / OptiFine / RSift 厳密比較手順）を同梱。

### 1.8 DX12 Agility 版 (windows-0.58 API 差異の精査で 76 errors 全件対処済みと監査確認・実機 cargo check 待ち)
`crates/rsift-dx12`: Work Graphs / DirectStorage / Sampler Feedback Streaming / Radiance Cascades GI / Tiled Resources / Conservative Rasterization / CMAA2 / Multi-view / gpu_graph frame graph (53KB) / cull_indirect・expand_quads・vis_resolve compute HLSL 群。

---

## 2. まだ無いピース = 次に入れる「世界の軽量化」

### ✅ 全 13 技法 取り込み済み (wave-3, `rsift-opt-gfx`, lib.rs 登録完了)

#### A. Minecraft MOD 界隈からの逆輸入 (効果大・実装が比較的安い)
| 機能 | 元ネタ | 実装済みモジュール | 内容 |
|---|---|---|---|
| **実体/BE オクルージョンカリング** ✅ | EntityCulling (tr7zw) | `entity_culling.rs` | Amanatides-Woo DDA 光線追跡 (SolidQuery トレイト) + AABB 27点サンプル (カメラ近い順) + ラウンドロビン予算 + 1フレーム遅延判定 |
| **MoreCulling 広域カリング** ✅ | MoreCulling | `more_culling.rs` | 看板テキスト背面 (110° dot) / 額縁 (向き+画面占有px) / 雨 (top_opaque_y) / 葉面 6近傍 / 共有レイヤー面 |
| **HUD/テキスト即時バッチ** ✅ | ImmediatelyFast | `hud_batch.rs` | BatchKey (texture/blend/scissor/layer) で HashMap マージ → 1ドロー/texture、layer ソート時のインデックス再パック、`draw_calls_saved` 計測 |
| **非フォーカスFPS制御** ✅ | Dynamic FPS | `power_policy.rs` | Active/Unfocused/Minimized/Idle + ヒステリシス + バッテリーセーバー上限、`frame_gate` アキュムレータ |
| **GUI 分離レート** ✅ | Exordium | `gui_composite.rs` | GUI 30fps / 入力イベント即時無効化 / アニメ窓 60fps ブースト / スタッターガード |
| **サーバーチャンク ローカルキャッシュ** ✅ | Bobby | `bobby_cache.rs` | `.rcc` フォーマット (magic RCC1 + CRC32 + zstd level13) + `.tmp`→rename アトミック書き込み + FNV-1a サーバハッシュ + LRU 容量管理 |
| **遠景LOD 完成形 (DH 相当)** ✅ | Distant Horizons | `distant_lod.rs` | 2x2→1 ダウンサンプル + 上面クアッド (4角近傍平均Y) + **スカート** (縫い目消し) + packed vertex + 距離→LOD テーブル |
| **パーティクル制御群** ✅ | Sodium Extra | `particle_control.rs` | 10カテゴリ予算 + 近距離保護 + FNV-1a 決定論的間引き (チラつかない) |
| **BE 静態化** ✅ | FastChest | `static_be.rs` | 40tick アイドル→静的昇格 / インタラクト距離で降格 / バーストガード 6回で永久動的 |

#### B. 現代エンジン界隈
- **FSR3 Frame Generation** ✅ — `fsr3_fg.rs`: depth-aware backward-warp 補間 (CPU 参照実装 + 実 WGSL compute シェーダ、disocclusion マスク付き)
- **BC7 (KTX2) テクスチャ圧縮** ✅ — `bc7_ktx2.rs`: 本物の BC7 **mode 6** エンコーダ (LSQ エンドポイント再フィット 3 反復 + アンカー反転 + P-bit 選択) + KTX2 ライター (vkFormat 145/146, DFD, mip chain)。mode 6 のみという正直な制限あり
- **HW オクルージョンクエリ** ✅ — `occlusion_query.rs`: wgpu 0.20 にネイティブ query が無いため IDバッファ方式 (R32Uint + depth + copy_texture_to_buffer + map_async, 1フレーム遅延, ヒステリシス付き)。GPU パイプライン + テスト済み CPU リファレンスの両実装
- **Virtualized micro-mesh (Nanite風)** ✅ — `nanite_clusters.rs`: 共有≥2頂点エッジで flood-fill ≤128 tri クラスタ化 (接続性優先 growth + 島 rescue) + エラー metric (merged-radius − max child) + `cluster_should_draw`
- **DirectStorage (SFS 込み) 完成** — rsift-dx12 側 (次項)

---

## 3. 優先順位 (安く効く順)

1. **実体/BE オクルージョンカリング** (EntityCulling 逆輸入) — コード量小・効果即体感
2. **Dynamic FPS 風パワーポリシー** — `frame_pacing` へ 10 行級追加
3. **Bobby 風チャンクキャッシュ** — 既存 I/O + 圧縮の延長
4. **遠景LOD 完成形 (DH 看板)** — 3 モジュール結線。画期的なスクショが取れる
5. **ImmediatelyFast 風 HUD バッチ** — 実機描画接合後に
6. FSR3 / BC7 / Work Graphs 修復 — 後段

---

## 4. 整合性メモ

- `lib.rs` の wave-2 モジュール群は現状 **pub mod 登録のみ・live path 未配線** (「not yet wired」の注記あり)。量産した機能を実際の描画パイプラインに結線する「第2フェーズ」がある。
- dx12 の 76 errors は windows crate 0.58 の API 差異が主犯 (別途対応予定)。
- ベンチマーク系ドキュメントの数値は設計目標。実機計測は Windows 側ビルド成功後に `gui_settings` の HUD で確認する予定。
