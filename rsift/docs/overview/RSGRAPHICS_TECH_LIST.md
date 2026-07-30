# RsGraphics (rsift-opt-gfx) 導入技術一覧

> 機械生成: 全モジュールの先頭 doc コメントから抽出・分類 (2026-07-30 / wave 195)。
> 全 167 モジュール + DX12 専用クレート (rsift-dx12) + JVM 相互運用基盤 (rsift-jvm)。

## アップスケーリング / フレーム生成 / 可変解像度 (10)

| module | 技術 |
|---|---|
| `fsr1` | FSR 1.0 — EASU (Edge-Aware Spatial Upsampling) + RCAS (Robust CAS) の CPU 参照実装。 |
| `fsr2` | FSR 2.0 — FidelityFX Super Resolution (temporal). Jitter + motion-vector |
| `fsr3_fg` | FSR 3 Frame Generation — 実装: モーション誘導の中間フレーム補間。 |
| `cas` | AMD FidelityFX Contrast Adaptive Sharpening (CAS) for `rsift-opt-gfx`. |
| `frame_fsr1` | # FSR1 GPU 実パス (`GpuFsr1Pass`) — RsGraphics 進化 Phase B |
| `drs` | Dynamic Resolution Scaling + CAS settings (Tier 4). |
| `checkerboard` | Checkerboard rendering — render only half the pixels (a 2x2 checkerboard |
| `adaptive_shading` | Adaptive shading — distance/motion pseudo-VRS (no framebuffer resolution change). |
| `vrs` | Variable Rate Shading (VRS) — per-tile shading-rate mask generation. |
| `foveated` | Foveated shading-rate (VRS) mask for `rsift-opt-gfx`. |

## アンチエイリアス (AA) (4)

| module | 技術 |
|---|---|
| `fxaa` | FXAA — Fast Approximate Anti-Aliasing (Timothy Lottes). Post-process pass |
| `smaa` | SMAA — Subpixel Morphological Anti-Aliasing (edge detection + blend). |
| `taa` | Lightweight temporal AA with neighborhood clamp (Tier 6). |
| `taa_ycocg` | TAA neighbourhood clamping in YCoCg space for `rsift-opt-gfx`. |

## グローバルイルミネーション / ライティング / 影 (18)

| module | 技術 |
|---|---|
| `ddgi` | DDGI (Dynamic Diffuse Global Illumination) probe volume sampling for |
| `frame_ddgi` | # RsGraphics 進化 Phase D2 — DDGI probe volume 実 GPU dispatch (`GpuDdgi`) |
| `voxel_cone_tracing` | # 38. Voxel Cone Tracing (`VoxelConeTracing` - GI Global Illumination) |
| `frame_vct` | # RsGraphics 進化 Phase D1 — Voxel Cone Tracing 実 GPU dispatch (`GpuVct`) |
| `restir` | ReSTIR (Reservoir-based Spatiotemporal Importance Resampling) direct |
| `ibl_sh` | Spherical-harmonics (2nd-order, 9 coeffs) image-based ambient for |
| `compute_light_prop` | # 40. Compute Shader-Based Lighting (`ComputeLightPropagation`) |
| `light_cache` | Block light propagation cache (Tier 6). |
| `clustered_lighting` | Clustered (forward+) light assignment. |
| `tiled_deferred` | # 39. Deferred Rendering / Tiled Deferred (`TiledDeferredLighting`) |
| `visibility_buffer` | Visibility Buffer — store primitive + instance IDs instead of a fat G-buffer. |
| `gtao` | GTAO — Ground Truth Ambient Occlusion (horizon-based). |
| `ao_bake` | AO precompute at mesh time (Tier 1) — Minecraft smooth-lighting corner AO. |
| `deinterleave_ao` | # DeinterleaveAO — 低スペック向け AO: デインターリーブ半解像度 + 空間デノイズ |
| `ssr` | Screen-space reflections (SSR) for `rsift-opt-gfx`. |
| `screen_space_shadow` | Screen-space shadows (SSS) for `rsift-opt-gfx`. |
| `shadow_lod` | Shadow Level-of-Detail — scales shadow-map resolution to what the light |
| `exposure` | Auto-exposure (eye adaptation) via a log-luminance histogram for |

## ポストプロセス / 大気 / 透過 (10)

| module | 技術 |
|---|---|
| `bloom` | HDR bloom for `rsift-opt-gfx`. |
| `aces_tonemap` | ACES Filmic tone mapping (Narkowicz 2015 approximation). |
| `motion_blur` | Tile-based motion blur for `rsift-opt-gfx`. |
| `depth_of_field` | Depth-of-field (bokeh) via circle-of-confusion gather for `rsift-opt-gfx`. |
| `parallax` | Parallax occlusion mapping (POM) for `rsift-opt-gfx`. |
| `decals` | Projected (deferred) decals for `rsift-opt-gfx`. |
| `wboit` | Weighted Blended Order-Independent Transparency (McGuire & Bavoil 2013). |
| `volumetric_fog` | Exponential height fog with a dithered raymarch for `rsift-opt-gfx`. |
| `atmospheric` | Analytic atmospheric scattering for `rsift-opt-gfx`. |
| `particle_control` | Sodium Extra 逆輸入 — パーティクル予算・間引き・距離減衰。 |

## チャンクメッシング / 頂点圧縮 / 差分更新 (17)

| module | 技術 |
|---|---|
| `binary_greedy_meshing` | Binary Greedy Meshing — tri-axis face culling + quad merging (cgerikj-style). |
| `chunk_mesh` | # 12-Byte Quantized Chunk Meshing Engine |
| `packed4` | 32-bit (4-byte) voxel vertex packing — Voxel Game Mesh Optimizations layout. |
| `half_vertex` | Half-precision vertex quantization. |
| `vertex_compression_r10g10` | Vertex Compression R10G10B10A2 + FP16 UV + octahedral 法線 - 帯域 1/4 化 |
| `vertex_cache_opt` | Vertex cache optimization — Forsyth's "Linear-Speed Vertex Cache Optimization". |
| `diff_mesh` | Section-level differential mesh updates (Tier 2). |
| `temporal_mesh_diff` | Temporal Mesh Diff — dirty セクションの**決定的追跡**とパレット差分列挙。 |
| `leaf_fast_path` | Leaf block fast path — 葉 voxel の内部を削減するメッシュ前処理 (Sodium-style に着想)。 |
| `pull_mesh` | Index-less pull mesh — SSBO quad list, GPU expands corners via `vertex_index`. |
| `gpu_vertex_pull` | GPU vertex-pull 語彙 — SSBO quads, draw without index buffer。 |
| `branchless_block` | Branchless Block Logic - if(block==water)をLUT + cmovに |
| `simd_kernels_avx2` | カラム OR マスク導出カーネル - greedy メッシュ前段のスラブ走査に使用 |
| `frame_reuse` | Per-chunk frame-to-frame reuse — skip prepare/RLE/mesh/SVO when terrain unchanged. |
| `world_column_store` | Live Minecraft column store — ClientLevel sections → compressed SectionPalette for meshing. |
| `static_be` | FastChest 逆輸入 — ブロックエンティティ (チェスト等) の静的メッシュ化。 |
| `mesh_compactor` | # MeshCompactor — Frostbite 方式 compute ドロー圧縮（メッシュシェーダ不要版） |

## カリング / 可視性 (CPU+GPU) (14)

| module | 技術 |
|---|---|
| `gpu_culling` | # HZB GPU-Driven Occlusion Culling & Multi-Draw Indirect Engine |
| `frame_hiz` | # GPU Hi-Z Occlusion Culling (`GpuHiz`) — RsGraphics 進化 Phase C |
| `hzb_2d` | 2D Hierarchical-Z occlusion — conservative CPU Hi-Z with temporal stability. |
| `cpu_occlusion` | CPU Masked Occlusion — delegates to conservative 2D Hi-Z (`hzb_2d`). |
| `occlusion_complete` | Software occlusion via CPU Hi-Z pyramid (Tier 6). |
| `occlusion_query` | # Occlusion Query (hardware-style visibility queries on stock wgpu) — NEW |
| `entity_culling` | EntityCulling (tr7zw) & Hierarchical Spatial Culling V2 — 実体/ブロックエンティティの高速オクルージョンカリング。 |
| `more_culling` | MoreCulling 逆輸入 — 看板テキスト / 額縁 / 雨 / 葉の追加カリング規則。 |
| `simd_frustum` | SIMD フラスタムカリング — SoA 配置の AABB 群を一括で視錐台内外判定。 |
| `lbvh` | Linear BVH (LBVH) build + frustum culling for `rsift-opt-gfx`. |
| `chunk_cull` | Chunk visibility culling — VisGraph + frustum + empty-column fast path. |
| `visibility_graph` | Visibility Graph + Flood Fill - Sodium 0.5+発展 |
| `meshlet_cone` | Meshlet normal-cone backface culling. |
| `overdraw_sort` | Overdraw reduction — front-to-back sorting + early-Z simulation. |

## LOD / 遠景 / 疎ボクセル構造 (13)

| module | 技術 |
|---|---|
| `distant_lod` | Distant Horizons 完成形 — 量子ツリー高度カラム + スカート付き LOD メッシュ生成。 |
| `lod_hybrid` | 3-tier LOD hybrid — near mesh / mid impostor / far heightmap (flat arrays, SVO far-only). |
| `micro_lod` | Micro LOD & Impostor - Mesh Shader無しLOD、遠景は8x8ダウンサンプル+ベイクAO |
| `billboard_lod` | Billboard LOD for distant flora/trees (Tier 4). |
| `svo` | Sparse Voxel Octree (SVO) — far-LOD / zero-polygon path with branchless DDA leaf refine. |
| `svdag` | # 15. Sparse Voxel DAG (`SparseVoxelDag` / SVDAG) |
| `transform_svdag` | # 16. Transform-Aware SVDAG (`TransformAwareSvdag` - 2025 I3D Molenaar & Eisemann) |
| `gigavoxels` | # 37. GigaVoxels / Brick-Based Streaming (`GigaVoxelsBrickStreaming` - Crassin et al.) |
| `nanite_clusters` | Nanite 風仮想化 — メッシュを ~128 トライアングルのクラスタ (meshlet) に分割し、 |
| `entity_tick_lod` | Distance-banded entity tick scheduling (Tier 5). |
| `fragment_ray_box` | # 33. Ray-Box Intersection on Fragment Shader (`FragmentRayBoxIntersect`) |
| `branchless_dda` | Branchless 3D DDA — voxel grid ray traversal (GPU-friendly, no per-axis branches). |
| `location_encoded_occupancy` | # 32. Encoding Occupancy in Memory Location (`LocationEncodedOccupancy` - 2025 CGF) |

## GPU 駆動 / ドローコール削減 / バリア・PSO (20)

| module | 技術 |
|---|---|
| `execute_indirect` | ExecuteIndirect + GPU-Driven Chunk Batching & Draw Compaction Engine |
| `azdo` | # 18. AZDO (`Approaching Zero Driver Overhead` Orchestrator) |
| `bindless` | Bindless / descriptor-indexing handle packing for `rsift-opt-gfx`. |
| `descriptor_heap_ring` | Descriptor Heap Ring Allocator - D3D12 CBV/SRV/UAVのO(1)確保を実現 |
| `root_signature_optimized` | Root Signature 1.1 + Static Samplers最適化 |
| `enhanced_barriers` | Enhanced Barriersバッチ化 - D3D12 Enhanced Barriers モデル |
| `pso_library_cache` | PSOキャッシュとPipelineLibrary - 初回起動ハング防止 |
| `bundle_reuse` | Bundle再利用 - 静的チャンク描画コマンドの録画・再利用 |
| `vertex_pool` | Vertex pool — single GPU-side buffer slots (no per-chunk VBO alloc). |
| `persistent_vbo_pool` | Persistent mapped VBO pool — one giant GPU buffer, bucket allocator, MDI batch. |
| `gigabuffer` | # 21. Gigabuffer / Mega Buffer (`GigaBufferSuballocator`) |
| `gpu_arena` | # GpuArena & Off-Heap Lock-Free Ring Buffer IPC |
| `triple_buffer` | Triple buffering for CPU→GPU mesh uploads (Tier 2). |
| `subgroup` | Subgroup / wavefront operation helpers for `rsift-opt-gfx`. |
| `instanced_draw` | Instanced Rendering for Non-cube Blocks - 草花作物は同一メッシュをDrawInstanced |
| `render_graph` | Full render graph with resource tracking + topological schedule (Tier 6). |
| `render_pipeline` | Rsift render pipeline — 必須/推奨 + Feather weak-PC (no resolution scaling). |
| `async_compute` | Async compute overlap scheduler for `rsift-opt-gfx`. |
| `frame_pipeline` | # Frame Completion Pipeline (`GpuFramePipeline`) — RsGraphics 進化 Phase A |
| `frame_postfx` | Frame PostFX wiring — CAS / Checkerboard / Exposure / VRS の実 dispatch 配線。 |

## メモリ / 圧縮 / ストレージ / ストリーミング (24)

| module | 技術 |
|---|---|
| `intern_pool` | # InternPool — FerriteCore 思想の汎用等価データ重複排除プール |
| `string_intern` | # Compact String Interning & Short String Optimization (`InlineStr` / `CompactSymbolTable`) |
| `bump_arena` | Bump Arena Per Task - bumpalo代替の自前アリーナ |
| `mimalloc_config` | Global Allocator置換 - mimalloc設定 |
| `pool_slab` | Slab + Object Pool + Generational Allocator for RenderSection. |
| `mesh_cache` | Disk mesh cache — RLE palette header + zstd compressed mesh (skip rebuild on revisit). |
| `region_zstd` | # RegionZstd — セーブデータ (リージョンファイル) の圧縮辞書選定 & スループット層 |
| `out_of_core_paging` | # 25. Out-of-Core Paging (`OutOfCoreMmapPaging` / ファイルページ + 真 LRU) |
| `lockfree_vram_cache` | # 22. Lock-Free VRAM Mesh Cache (`LockFreeVramMeshCache`) |
| `async_chunk_io` | Async chunk I/O with LZ4 + LRU priority load (Tier 3). |
| `sparse_texture` | Virtual (sparse) texturing — zero-allocation intrusive LRU page-table residency (`IntrusiveLruPageTable`). |
| `texture_atlas_virtual` | Sparse Virtual Texture Atlas - VRAM 100MB以下でも高解像度リソパ対応 |
| `mip_streaming` | テクスチャストリーミング — VRAM 予算内で必要な mip/テクスチャだけを常駐させ、 |
| `bc7_ktx2` | BC7 (mode 6) テクスチャ圧縮 + KTX2 コンテナ書き出し。 |
| `texture_atlas` | Texture atlas + texture array + mipmap chain (Tier 1). |
| `texture_budget` | Texture bandwidth budget — compressed atlas + mipmap bias (Feather weak-PC). |
| `soa_layout` | SoA entity layout + XZY indexing + 64-byte alignment (Tier 3). |
| `morton_order` | Morton Order (Z-curve) - 隣接アクセス局所性向上 |
| `spatial_hash` | 3D spatial hash for entity queries (Tier 5). |
| `bobby_cache` | Bobby 逆輸入 — サーバーチャンクのローカルキャッシュ。 |
| `bitpacked_section` | # Bit-Packed & Single-Value Chunk Section Storage (`CompactChunkSection`) |
| `palette_pack` | # PalettePack — 可変ビットパレット圧縮（MC 1.13+ 方式を整備） |
| `section_rle` | Run-length encoding for 16³ section palettes and Y-layer occupancy masks. |
| `section_compress` | Hybrid section storage: Single / RLE (hot) / LZ4 (cold distant columns). |

## CPU 並列 / ジョブシステム (6)

| module | 技術 |
|---|---|
| `dag_scheduler` | DAG Job System - parse->light->ao->mesh->upload依存グラフをトポロジカルに並列実行 |
| `job_system` | # JobSystem — Chase-Lev デック型ワークスティーリング + 優先度 + |
| `rayon_job` | Rayon work-stealing Chunk Meshing - 32セクション並列 |
| `dashmap_registry` | Lock-Free DashMap for Task Registry - ChunkState管理 |
| `stutter_guard` | # StutterGuard — フレーム内一時オブジェクトの撲滅（バンプアリーナ＋ |
| `cpu_saver` | # CPU Overhead Reduction & Cache Optimization Engine (`cpu_saver`) |

## フレームペーシング / 電力 / 低負荷化 / GUI 2D 高速化 (11)

| module | 技術 |
|---|---|
| `frame_pacing` | Frame pacing — 提示タイミングを vsync 境界に揃えてジャンク（カクつき）を消去。 |
| `quality_governor` | # QualityGovernor — UE DynamicRes 式「連続オーバーバジェット → panic 降段」+ |
| `power_policy` | Dynamic FPS 逆輸入 — 非フォーカス/最小化/放置時の電力ポリシー。 |
| `eco_render` | # Eco Region Renderer — Sodium-Surpassing Batch Pipeline |
| `gui_composite` | Exordium 逆輸入 — 3D は高 FPS のまま、GUI/HUD を別レートのオフスクリーン面で描く。 |
| `hud_batch` | ImmediatelyFast 逆輸入 — HUD/テキスト/ネームタグの即時モード描画を 1 ドローコールに集約。 |
| `gui_settings` | Sodium-style modern video settings GUI for RsGraphics. |
| `boot_splash` | Boot splash progress logger — honest: no HWND/GPU override of Mojang screen. |
| `tick_render_split` | Fixed 20 TPS tick clock + render interpolation (Tier 3). |
| `low_spec_stack` | Low-spec “mania” stack — wire cheap, high-impact voxel techniques into the live path. |
| `noise_upsample` | 3D noise upsampling — coarse Perlin grid + trilinear fill. |

## シェーダパイプライン / ランタイム検証 / ベンチ (7)

| module | 技術 |
|---|---|
| `iris_pipeline` | # Iris Shaders / OptiFine Shader Pack Pipeline |
| `gpu_runtime` | # Real wgpu Runtime — 実デバイス生成 + 全 WGSL シェーダーの実コンパイル検証 |
| `frame_reference` | # Frame Reference Rasterizer (CPU) — GPU/CPU 相互検証用の参照実装 |
| `full_graph_wiring` | Full Graph Wiring — 全モジュールを実データで毎フレーム実実行するオーケストレーター。 |
| `bench_harness` | # BenchHarness — HDR ヒストグラム（HdrHistogram 流）＋ CPU パフォーマンス |
| `zerocopy_cast` | bytemuck + zerocopyゼロコピー基盤 - GPU転送コピー0回 |
| `pgo_bolt` | PGO + LTO設定 - ビルド時の最適化フラグを提供 |

## macOS / Apple プラットフォーム (直 binding) (6)

| module | 技術 |
|---|---|
| `apple_backend` | Apple (MacBook) バックエンド選定ポリシー — Metal 先行 + class 別 tier の純粋関数コア。 |
| `apple_canon` | 【機械生成・手編集禁止】Apple Metal/Foundation/QuartzCore 正典 API 表 (canon)。 |
| `apple_ffi_audit` | Apple FFI 監査機 — binding 層ソースと一次情報 canon の機械照合。 |
| `metal_direct` | Metal 直 binding 完全実装 (classic Metal 系) — objc runtime dispatcher 経由。 |
| `objc_rt` | ObjC ランタイム dispatcher 注入層 — 直 binding 検証アーキテクチャの中核。 |
| `gl33_compat` | wgpu-backed OpenGL 3.3 feature equivalents (Tier 1). |

## その他 / 横断基盤 (7)

| module | 技術 |
|---|---|
| `aokana` | # 31. Aokana Framework (`AokanaFramework` - 2025 I3D) |
| `depth_prepass` | Depth prepass + translucent / water dedicated passes (Tier 4). |
| `frame_worldgen` | Frame Worldgen wiring — 値ノイズ upsample / LBVH / Bindless / Half-vertex / |
| `material_batch` | Material batching + opaque/transparent draw-range split (Tier 1). |
| `simd_kernels` | Portable SIMD-ish kernels for culling / greedy runs / popcount (Tier 3). |
| `software_tiling` | Software tile binning — TBDR-inspired tile assignment (32×32 / 64×64 tiles). |
| `tbdr_hints` | TBDR (Tile-Based Deferred Rendering) 向けヒント。 |

## 基盤クレート (RsGraphics が駆動する横断技術)

| crate | 技術 |
|---|---|
| `rsift-dx12` | DirectX 12 Agility SDK 直 binding — GPU Upload Heaps・Enhanced Barriers・Tiled Resources・Shader Model 6.6/6.9 tier |
| `rsift-jvm` | JVM Invocation API 埋め込み・JVMTI agent・JNI Critical natives・DirectByteBuffer ゼロコピー・detour hook・bytecode transpiler・WASM Mod sandbox |
| `rsift-api` | Fabric/NeoForge 統合パリティ層・イベントバス・Netty DirectByteBuffer ゼロコピーネットワーキング・Native DLL Mod ローダー・**Mod セキュリティ層 (wave 195)** |
| `wgpu` (vendored) | ポータブル GPU 抽象化 — 全 WGSL シェーダー実コンパイル検証・Metal/Vulkan/DX12/GLES バックエンド |

## 動作保証の仕組み (品質基盤)

- `rspeed` 監査ツール (116 機能): seal 6 ゲート (san/fmdiff/trailws/台帳/テスト/digest/env)
- 全 wave で TDD (RED→GREEN) + adversarial 変異テスト + MD5 復元検証
- `bench.yml` CI: pseudo_mc_bench / wide_static_bench 決定性ダイジェスト
- Apple 系: SDK 一次情報 canon 正典表 + apple_ffi_audit 独自静的解析機 (R1-R7) + MockObjcRt 動的検証 (Mac 実機不要の 3 層保証)
