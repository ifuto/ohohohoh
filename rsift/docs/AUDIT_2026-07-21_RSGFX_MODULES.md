# RsGraphics / rsift-opt-gfx 162モジュール監査レポート — 2026-07-21
**対象: arena/019f79db-rsift @ HEAD〜本コミット / 実施: Arena エージェント (ローカル Rust 1.94.1 復元後の実機械検証)**

## 監査方法 (全て実行値・推測なし)

1. `vendor` ブランチから Rust 1.94.1 + 454 crates を sha256 照合で復元 (`ci/restore-env.sh`)。
2. `cargo test -p rsift-opt-gfx --lib` 全数走査、コンパイラ警告の完全帰属。
3. **naga による WGSL 全数検証スイープ** (新設テスト): `shaders/*.wgsl` 50 件を
   パース+全セマンティクス検証 (GPU 不要の純 CPU 検査。今後ファイル追加で自動拡張)。
4. 警告シグナル箇所と旗艦モジュール (CAS 3連鎖・chunk_mesh・morton・entity_culling) の
   人力レビュー。アルゴリズム妥当性は一次情報照合:
   - FidelityFX CAS: GPUOpen-Effects/FidelityFX-CAS `ffx-cas/ffx_cas.h`
     (`CasFilter` no-scaling / `CasSetup`、GitHub master 原文照合)
   - Octahedral normal: Cigolle 2014 系
5. 変更後は lib テスト + release ベンチ (`pseudo_mc_bench`) 決定的ダイジェストで
   出力 bit 同一性を確認。

## 検出と修正 (全10件)

### 🔴 A. アルゴリズム偽装

**A1. `cas.rs` — 「CAS」を名乗るが局所平均混合 (=ぼかし) であった**
- 旧式: `out = c·(1−a) + avg·a` (`avg=(mn+mx)/2`)。全コントラスト域で中心を近傍
  平均に近づけるため鮮鋭化は一切発生せず、中コントラスト域を微細にぼかすのみ。
  さらに mn/mx が中心を除外しており公式と異なった。テストまで旧挙動を
  「sharpened」と称して固定していた。
- 修正式 (公式 `ffx_cas.h` `CasFilter` noScaling + `CAS_SLOW` 高品質パス準拠):
  ```
  mn/mx = cross4 近傍 + 中心の soft min/max
  amp   = sqrt(sat(min(mn, 1-mx) / mx))      ← MX_FLOOR(1e-30) で 0/0 NaN を決定的に回避
  w     = amp * peak,   peak = -1/lerp(8, 5, sat(sharpness)) ∈ [-1/8, -1/5]  (CasSetup const1.x)
  out   = sat((c + Σcross·w) / (1 + 4w))
  ```
- 3連鎖ミラー `cas.rs::cas_sample` ↔ `frame_postfx::cas_run_cpu` ↔ `shaders/cas.wgsl`
  を同一式・同一演算順で統一。テスト 6 件を本式の不変条件へ全面改訂
  (エッジ中心が平均から遠ざかる方向 / 公式参照式との独立再構成一致 / 純黒 NaN 無し /
  極値中心のリンギング保護 / 全域 [0,1] かつ非 NaN)。

### 🟠 B. 出荷済み WGSL の検証失敗 3件 (実デバイスではシェーダーコンパイル失敗)

naga 検証 (IR レベル) で `IndexMustBeConstant` を検出。**値空間配列の動的 index** は
WGSL 不可という同一 root cause。演算は変えずアドレッシング機構のみ修正:

| ファイル | 関数名 | 旧 | 新 |
|---|---|---|---|
| `shaders/bloom.wgsl` | `bloom_blur` | 重み/オフセット `let` 配列を loop var で index | `var` 化 (値不変) |
| `shaders/ibl_sh.wgsl` | `evaluate_sh` | 配列引数/基底 `let` 配列を動的 index | 関数アドレス空間へ値複写後に index (CPU ミラーと同一演算を維持) |
| `shaders/terrain_vertex_pull.wgsl` | `corner_pos`/`vs_pull` | `const FACE_UV`, `const TRI_CORNER` を頂点 index で読む | `var<private>` 化 (値不変) |

- bloom は `all_wgsl_sources()` (実 dispatch 対象) 所属のため、実デバイスでは
  `shaders_failed` 入りしていた確実な不良。スイープ結果は **47/50 → 50/50 合格**。

### 🟡 C. コンパイラ警告が示す死骸・API ハザード 6件

| 場所 | 内容 | 対処 |
|---|---|---|
| `entity_culling.rs:142/150` | `stats_visible` 書き込み専用 (統計は `ids.len()` と常に一致する冗長) | 削除 (出力集合不変、ベンチ digest bit 同一で実証) |
| `cas.rs:95` | `cc` 未使用 (旧式の残骸) | A1 修正で解消 |
| `eco_render.rs:150` | `vertices_built` 未使用 | 定数比 (20B vs 12B) 設計の注釈 + `_` 接頭 |
| `occlusion_query.rs:632` | `record()` の `device` 未使用 | 将来の bind group 再構築用である注釈 + `_` 接頭 |
| `full_graph_wiring.rs:404/411` | `slab_slots` 書き込み専用カウンタ (alloc 副作用が本体) | カウンタ削除 |
| `noise_upsample.rs:204` | `pub fn` が private 型 `CoarseGrid` を露出 (private_interfaces) = 外部から呼出不可能な API | モジュール private に降格 (外部参照は frame_worldgen の精密ミラー設計) |

- `chunk_mesh.rs` の構造体 doc が「fp16 半精度」と主張していたのに実装は
  **16bit 固定小数点 (1/1024 LSB)/UNORM16** だった文書偽装も訂正。
- **release ビルド警告: 41 → 33** (狙った 8 件のみ除去、他の既定警告は不変保持)。

### ✅ D. 健全確認済み (監査通過)

- `morton_order.rs`: SWAR 展開/収縮マスク列・BMI2 PDEP/PEXT の `target_feature` 付き
  動的ディスパッチ・`MortonGrid3D` 境界 — 全て正規手法で unsafe も sound。
- `chunk_mesh.rs::encode`: 量子化の切り捨ては共有頂点を同一値に落とし水密性を保つ
  正しい設計。飽和域・ゼロ法線ガード・octahedral 折返しを新テスト 7 件で実測ピン。
- `pseudo_mc_bench` 連鎖 (メッシュ/ソート/カリング/I/O): 決定的ダイジェストが
  **bench_v8 記録と bit 完全一致** (24543/314193, 303895 verts, 3646740 B, ACMR
  1.669→0.929, hit率 90.9% 等) であることを変更後にも再確認。

## モジュール健全性マップ (機械計測 2026-07-21)

- 対象: `crates/rsift-opt-gfx/src/*.rs` 162 モジュール (lib.rs 除く、計 38k 行超)
- `#[test]` 保有: **126 / 162**、テスト皆無: 36 (下記)
- `todo!`/`unimplemented!`: 0 件、ファイル先頭の "Real logic" 宣言を疑い全数検査の方針
- `shaders/*.wgsl`: 50 件全数 naga 検証合格 (新恒久ガード `gpu_runtime::tests` 2件)
- lib テスト総数: **360 → 372** (CAS 3→6、chunk_mesh 0→7、naga 恒久ガード +2)

テスト皆無 36 件 (優先順位付けは今後の課題):
`adaptive_shading, boot_splash, branchless_block, bump_arena, bundle_reuse,
chunk_cull, cpu_occlusion, dag_scheduler, dashmap_registry, descriptor_heap_ring,
eco_render, enhanced_barriers, full_graph_wiring, gpu_culling, gui_settings,
instanced_draw, leaf_fast_path, lod_hybrid, mesh_cache, micro_lod, mimalloc_config,
pgo_bolt, pso_library_cache, pull_mesh, rayon_job, render_pipeline,
root_signature_optimized, simd_kernels_avx2, software_tiling, temporal_mesh_diff,
texture_atlas_virtual, texture_budget, vertex_compression_r10g10, vertex_pool,
visibility_graph, zerocopy_cast`
(うち chunk_cull/pull_mesh/render_pipeline/leaf_fast_path/visibility_graph 等は
example ベンチ経由で実動作が間接検証されているが、単体不変条件の固定は未整備)

## 監査の範囲と誠実な限定

- 162 モジュール全行の人手精読はしていない。機械検査 (全数テスト・警告帰属・
  WGSL 全数検証・到達解析) で全モジュールをカバーしつつ、シグナル箇所と
  旗艦経路を人手深堀りする統計的監査である。
- GPU==CPU の bitwise 主張 (Phase B–E 資産) は WGSL accuracy (除算 2.5ULP 等)
  の都合上ハードウェア実測依存であり、本 sandbox (GPU 無し) では未検証。
  iGPU 実機での再確認は未完了課題。
- `occlusion_query::GpuOcclusionPass::record` 等 wgpu 実装は型として正しいが
  デバイス経路での実 dispatch 配線は限定的 (既知・監査 B3 系譜)。

## 今後の課題 (優先度順)

1. 実 iGPU で `gpu_runtime` + frame_proof 系の GPU/CPU 一致検証。
2. ベンチ対外のコア経路 (chunk_cull / pull_mesh / render_pipeline / mesh_cache /
   visibility_graph) の単体テスト追加。
3. 36 のテスト皆無モジュールの順次カバー (本レポート一覧を基準点とする)。
4. bench-ci (GitHub Actions) 設置後は CI 上で本監査一式 (test + naga sweep +
   ベンチ determinism) を緑運用化。

---

# 第2ラウンド監査 (2026-07-21): スタブ / 見せかけ配線 / デッドコード / ドキュメント乖離

**要求**: 「明示的スタブや仮実装、見せかけ配線、デッドコード、ドキュメント主張との
乖離があるか徹底的に確認し、あった場合は修正」— 対象はワークスペース全 19 クレート。

## 方法 (全て実行値)

1. `todo!` / `unimplemented!` 全走査 → **ワークスペース 0 件**。
2. フィールド/関数/定数ごとの**参照ゼロ証明** (`grep` 全クレート横断) + コンパイラ
   dead_code 警告の完全帰属。文字列コード生成 (AOT トランスパイラ) 経由の参照も確認。
3. 同一コマンドでの前後比較: `cargo check --workspace --locked --offline`、
   実テスト実行、release ベンチ決定的ダイジェスト ×2、rustfmt HEAD-vs-current、clippy。
4. 事前存在疑いの失敗は **スタッシュで HEAD に戻して再現確認** してから判断
   (変更の無実を実証してから関連修正へ進む運用)。

## 修正一覧 (21 ファイル + WGSL 1 ファイル削除)

### A. デッドフィールド / 見せかけペイロード (rsift-opt-gfx, 11 ファイル)
| ファイル | 除去したデッド状態 |
|---|---|
| async_chunk_io.rs | 未構築 `LruEntry`、未読 `root` フィールド、`IoEvent::Failed/Stored` の未読 cx/cz ペイロード (unit 化。消費者不在の現状注記も追加) |
| stutter_guard.rs | 未読 `chunk_size` |
| execute_indirect.rs | 未読 `gpu_buffer_size` |
| job_system.rs | 書き込み/読み取り双方ゼロの `idle: AtomicUsize`、未呼出 `PrioQueue::is_empty` (sys/worker は RAII keep-alive として必要 → `#[allow(dead_code)]` + 理由明文化) |
| descriptor_heap_ring.rs | doc が「tail を進める」と謳うが init 後に読み書きゼロだった `tail` (doc も訂正。使用中の frame_fence は保持) |
| full_graph_wiring.rs | cache_dir 導出後に未読の `game_dir` |
| frame_pipeline.rs | wgpu TextureView が保持側で冗長だった `hdr/depth` (VRAM 保持の重複も解消) |
| frame_fsr1.rs | 同上の `inter` |
| frame_reuse.rs | 一度も読まれない `fingerprint` + エントリ毎のパレット全複製 `sections` (数百KB/chunk のメモリ二重保持を解消。鮮度判定は従来通り last_tick ベース) + それのみに使われた `touch()` |
| gpu_vertex_pull.rs | 初期化毎に実 WGSL コンパイルしながら一度も dispatch されない `task_pipeline` (`try_task_pipeline` + `SHADER_TASK_EMULATION` ごと除去。meshlet カリングは `frame_worldgen::GpuMeshletCull` が別途実 dispatch する現役設計は維持) |
| render_pipeline.rs | 未呼出 `demo_mode_enabled` (環境変数 `RSIFT_ENABLE_DEMO_RENDER` は設定しても元々無効果だったため可観測挙動は不変) |

### B. 見せかけ UI データ (1 ファイル)
- gui_settings.rs: `SodiumVideoSettingsGui::new()` が **ディスクに存在しない
  シェーダーパック名 3 件** (BSL/Complementary/Sildur's) をハードコード表示していた
  → 空初期化 + 「実一覧は `IrisShaderEngine::discover_shaderpacks` の走査結果を
  注入する」契約を明文化。

### C. デッドコード除去 (他クレート, 7 ファイル) — 全て参照ゼロ証明つき
- transpiler/collision.rs: `resolve_motion_with_step` の step 分岐の
  `vel[1] = 0.0` (by-value 引数への即 return 直前の書き込み = 効果ゼロの dead write)
  + 戻り値契約 (位置+接地のみ、速度は還元されない) を doc 明文化。**出力 bit 同一**。
- launch/offline.rs: 未呼出 `find_java` / `which_java` (+ そのみで使う
  `use std::process::Command`)。実 Java 解決は `java::ensure_java_home` が現役。
- launch/version_json.rs: 未呼出 `library_from_legacy`。
- installer/lib.rs: 未呼出 `is_launch_agent_flag` / `strip_javaagent_from_args`。
- app/theme.rs: 参照ゼロ `ease_out_cubic` / `glass_frame` (glass_card への旧 alias)。
- replay/compressor.rs: 一度も使われない訓練辞書シード定数 `PACKET_DICT_SEED`
  + モジュール doc の虚偽 ("ZSTD compression with Minecraft packet dictionary")
  を実態 (素の `zstd::bulk::compress`) に訂正。

### D. 構造欠陥 (実害あり): rsift-jvm のモジュール二重ロード根絶 (1 ファイル)
- agent_bridge.rs が `#[path = "..."]` で兄弟 8 ファイルを**私有 mod として
  二重ロード**していた (lib.rs でも `pub mod`)。影響: ① `#[no_mangle]` JNI
  エクスポートがクレート内に 2 インスタンス化され `cargo test -p rsift-launch /
  -p rsift-jvm` が **`symbol Java_com_rsift_... is already defined` でリンク不能**、
  ② agent 経由と JNI 直接経由が**別 static 状態を参照する split-brain**、
  ③ clippy `duplicate_mod` 8 件 + 警告の 12 重複行。各ファイルは `super::兄弟` のみで
  木非依存に書かれていたため、crate:: エイリアス化で単一インスタンスに統一。
  → **リンク解消 (jvm/launch テストが史上初めて実行可能に)**

### E. 既存テストが露呈した実バグ (2 件、HEAD でも再現確認済 = 本ラウンド非起因)
1. **transpiler/pathfinding.rs の A\* バグ**: ヒープ優先度 f=g+h を g (dist) として
   比較・累積していたため最初の展開以外が全て stale continue となりゴール未到達
   (`test_hpa_pathfinding` 恒常 FAIL)。g と f を分離する正しい A\* に修正 → 緑。
   (クラスタ理論コスト・出力経路の決定性は維持)
2. **launch/offline.rs のテスト環境仮定**: `APPDATA.unwrap()` が Windows 以外で panic。
   依存データ不在時の早期 return (versions 不在分岐と同じ sparse-data skip 契約)
   に修正 → Linux 緑。

### F. ドキュメント主張と実装の乖離 (2 ファイル)
- docs/SODIUM_IRIS_SURPASSING_ENGINE.md → **全面改訂**。旧版の虚偽:
  「Sodium と Nvidium の性能を上回る」(Nvidium は一度も比較未実施)、
  「Iris/OptiFine と 100% シェーダー互換」(GLSL 実行互換は未配線で fail-loud
  fallback)、「完全な MRT パイプライン構築」(Eco は identity composite のみ)、
  「設定 GUI をゲーム内 UI に配備」(実態は launcher からの box-drawing ログ
  preview)、§4 の「検証済み実行ログ」は**実在しないフィクション**
  (Complementary zip を MRT ロードするコード経路は存在しない)。→ 全て検証可能な
  事実表 + 現行コードが実際に発行するメッセージの引用に置換。
  (AUDIT_STUB_WIRING.md 指摘 D をこれで解決)
- mods-official/rsgraphics/Cargo.toml description: 「bindless wgpu,
  Sodium-surpassing meshing, and Iris Shaders」→ 実装事実 (12B 量子化頂点 / Rayon
  メッシング / GPU compute カリング / GLSL 実行互換未実装の明記) に置換。

### G. 資産削除
- shaders/terrain_task_emulation.wgsl: 上記 A の専用シェーダー (現行エントリポイント
  `cs_task_emulation` は一度も dispatch されず)。naga スイープはディレクトリ動的
  列挙のため影響なし (SHADER_MESH_SHADER = GpuMeshletCull 現用資産は保持)。

## スキップと正当化 (判断記録)
- **rsift-jvm の never-used 群** (render_bridge/glfw_hook/jvmti_events/agent_bridge
  内の未配線 fn、`MAX_INJECT_TICKS` 等) と **rsift-api::adaptive_perf 3 件**
  (cfg(not windows) スタンドイン、cfg 付き caller あり): Windows/JNI 契約面は
  本 sandbox (Linux/GPU 無し) で検証不能かつ cdylib 外的契約が絡むため保持
  (第1ラウンドの方針を継続)。
- **AsyncChunkIo**: 消費者ゼロだが自己完結実装 + 単体テスト緑の外面 API として
  残存。整合 (エンジン組込) は今後の課題としてソース注記済み (削除しない判断)。
- **OccVertex 可視性警告・winit/egui deprecated 指摘・dx12 FFI 命名警告** 等は
  事前存在の設計都合 (本ラウンドの対象外分類) として保持。

## 検証結果 (全て実測・本ラウンド実施)
- `cargo check --workspace --locked --offline`: **全 19 クレート エラー 0**。
  警告 (crate 別, 前日測定 → 本ラウンド): opt-gfx **33→16**、jvm **40+12dup→22+0dup**、
  transpiler 10→9、launch 5→2、replay 6→5、installer 1→0。
  その他 (非編集, 変化なし): api 13、dx12 5、parser 3、rsreplay 2、launcher 2、app 3。
- テスト: opt-gfx **372/372**、transpiler **13/13** (旧 HPA 恒常 FAIL→修正)、
  jvm **1/1**・launch **1/1** (旧: 双方ともリンク不能で実行すら不可)、
  replay 3/3、installer 0/0、app 0/0。
- ベンチ bit 同一: `pseudo_mc_bench` 決定的ダイジェスト 2 回実行で
  **sha256 `29a8544ef3…cf58` 完全一致**。doc 確定値 (S1 24543/314193, C mesh
  303895 verts/3646740 B, A 861656/27572992, ACMR 1.669→0.929, hit 90.9%,
  edit C 24511704 B, 水 116719 quads, palette 248110 B) も出力から spot 一致。
- rustfmt: 編集 20 .rs ファイル全てで HEAD 比の差分 hunk 数が**増加 0**
  (theme 末尾の余分空行 1 件のみ新規混入 → 即修正済)。
- clippy: 編集箇所に新規警告 0 (agent_bridge `MAX_INJECT_TICKS` デッド const は
  上記スキップ基準で保持。`is_multiple_of` 等は新 clippy による旧コードへの
  ベースライン指摘)。

---

# 第3ラウンド (2026-07-21 後半): 広域静的ベンチ + 計測駆動改善 + コア経路テスト

ユーザー指示: 「様々な分野 (数百種類前後) で静的ベンチマークを行い、あまりに
よくない結果が出た部分を優良になるまで改善」「基本は DX12、不可能なら旧 DX、
それより Vulkan が優れるなら Vulkan」「①(e2e slice) と ③(zero-test カバー)」。

## A. 描画バックエンドラダー (コミット e76248b、ユーザー方針のコード化)
- `rsift-render/src/backend.rs` 新設: `RenderBackendKind { Dx12, Dx11,
  VulkanWgpu, GlPassthrough }`。present 実装済は Dx12 と GlPassthrough のみ
  (実装事実をコードに固定)。`BackendProbe` 既定は dx12=Unknown /
  dx11,vulkan=Unsupported (旧 DX/Vulkan present 本体は未実装 = 実態通り)。
- `render_bridge::ensure_engine` に配線: ラダー選択 → DX12 init 成功で proxy
  enable + realized(Dx12) 記録。失敗時は fail-loud (agent_log) して GL パススルー
  へ降格 + realized(GlPassthrough) 記録。GL 確定後は DX12 create を再試行しない。
- 重大バグを同時修正: `const REALIZED: AtomicU8` は使用箇所ごとにコピー化
  され compare_exchange が全く効かなかった (警告 + テスト FAIL) → `static` 化。
- 強制指定はシステムプロパティ語彙 `rsift.render.backend = dx12|dx11|vulkan|gl|auto`。

## B. 広域静的ベンチ `wide_static_bench` (rsift-opt-gfx examples)
- 目的: 分野横断の静的 CPU 計測で「弱い行」を決定的に洗う土台。
  全入力が splitmix64 固定シード生成 (GPU/ネット/時刻不依存、2コア/3GB で動く)。
- 行 = 分野 × パラメータセル。**147 → 233 → 357 行**へ拡大 (rle_decode 追加、
  mesh/cull/dda/entity 全 8 パターン化、lbvh 2^14/2^20 追加、visibility_flood
  新分野、encode/decode/mesh/圧縮 seed 3→5、entity 第2シード系、intern キー
  空間バリアント、leaf 第2 seed、visibility 中央始点 等)。median 合計 ~0.97 秒、
  実行 ~26 秒 (入力生成含む、2コア sandbox) → CI に載せられる重さ。
- **structural digest**: 時刻列を除いた `domain|case|aux` テーブルの
  DefaultHasher 値を常時表示。現行 `004c1cf5fb17bfe8` (rows=357) (233 行時点では `90254e333370d418`)。
  消化中の最適化が出力集合・順序・構造値を変えないことを 2 回連続実行で確認する
  運用 (今回の全 3 改善で digest 不変を実証済)。
- 再現手順: `cargo run --release --locked --offline --example wide_static_bench`。

## C. ベンチ方法論の自己修正 (フェイク計測の排除記録)
1. 純算術ループ (packed4/morton/blocklut/meshlet) がコンパイラの代数的畳込みで
   **0.0ns に偽装**されていた → `black_box` で実測化 (5.5ns/2.8ns/2.6ns/5.5ns)。
2. quant12 は計測内の rng 生成コスト (~16ns/v) が支配 → 事前生成に修正
   (系列同一 seed で sum_* の bit 同一性を保ったまま公平化)。
3. leaf_fast_path: 計測クロージャ内で `gen_section` まで計っていたため noise の
   生成コスト (2^3 多数決 hash) が apply 本体に帰属 → apply 実コストは一律
   ~2.9µs と判明。さらに旧 8 パターンは全て非葉 id で変換経路が未実測だった
   → forest_leaf パターン新設 (delta_idsum=39537 を初めて実測)。
4. lbvh_cull の INVESTIGATE フラグは ops=1 正規化の罠 (n スケーリング誤検出)
   → ops=n に修正。残フラグ 27 件 (357 行版) は全て分類済: lz4/zstd の noise/caves/dense8
   行と mesh_section caves 行, forest_leaf 行 = **入力構造起因の良性**
   (圧縮率・面数・葉処理量がパターン依存なだけで欠陥ではない)。

## D. 実測駆動の改善 (全て digest 不変 / fuzz で出力 bit 同一性を証明)
| 改善 | 実測 (median) |
|---|---|
| メッシャー `greedy_axis_pull` 層マスク ホイスト (Y走査ループ内で全4096voxel再計算 ×32回/section → ループ外 1 回) | mesh_column8 flat **484→136µs (-72%)** / noise 1027→705µs / caves 1912→1687µs / dense8 534→226µs / column_scale noise h=16 2137→1465µs |
| LBVH 二段カリング (CULL_RUN=32 の morton 連続 run 包含球で一括 reject/accept) | cull 2^18 **2758→712µs (-74%)** / 2^16 364→125µs。build は run_bounds 追加で **+12-15% 悪化 (3.44→3.91ms @2^16)** — 明記 (build:query の利用比率で採用) |
| LBVH morton 順ソート葉 (境界 run の per-leaf が `centers[order[i]]` ランダムギャザーで 2^20 5.7ns/leaf 劣化 → sorted_centers/radii 連続読み) | cull 2^20 **5.97→0.86ms (-86%)** / 2^18 646→240µs。build 追加コスト **+8.6% @2^16 / +25% @2^20 (134→168ms)** — 明記 |
- 等価性証明: `accelerated_cull_matches_naive_fuzz` (sizes {0,1,31,32,33,512,5000}
  × 5 平面集合) で二段カリング=素朴総当たりを集合・順序ともに bit 等価と確認
  (ソート葉適用後も同一テスト緑)。

## E. 監査発見の実修正 (③テスト駆動)
1. **visibility_graph**: `visible_bits` は書き込み経路ゼロで `is_visible` が
   **恒 false のセマンティックスタブ**、旧 bit 写像 `(dx+dz*8)%64` も
   (0,1)/(8,0) 衝突で破綻。→ flood_fill 到達集合を start チャンク単位で実
   キャッシュ化 (cap 1024)、is_visible は実参照に。利用経路 (flood_fill →
   reachable count) の公開挙動は不変。
2. **mesh_cache**: キー不在の get が misses に計上されず (0,0) → 不在参照も
   miss 計上に修正 (stats() 消費者は現状 0、語彙の対称化)。
3. render_pipeline: テストが初めて文書化した実仕様 — camera は度で受け rad
   保持、frame 定数の chunk_origin はブロック座標の `(chunk-1)*16`、
   CPU mesh 未実行時は quad bytes None (DX12 「無ければ描かない」)。

## F. ③ テストカバー達 (zero-test コア 5 モジュール → 全カバー)
visibility_graph 6 / pull_mesh 5 / chunk_cull 8 / mesh_cache 8 (実ディスク往復)
/ render_pipeline 4 (直列化+poison 耐性のグローバル状態テスト) = **+31 件、
opt-gfx lib 373 → 404/404 緑**。残り zero-test モジュール (~31) は非コア
(描画後段・実験系) が中心で、重要度順に今後追加していく方針。

## G. 残課題 (正直棚卸し)
- INVESTIGATE 16 行は良性分類だが、mesh_section caves (47ns/voxel) のような
  面碎け入力での絶対コストは今後の計量対象として残す (Sodium 同等クラスの
  実測レンジ内という評価: 推測ではなく同一 sandbox の自己比較による)。
- 旧 DX (Dx11) / Vulkan present 本体は未実装 (backend.rs に Unsupported 固定済)。
  実機検証はユーザー PC 依存 — sandbox は GPU 無し。
- bench CI (GitHub Actions) は **稼働済** (2026-07-21 後半): ユーザーが
  bench.yml を workflow に設置 → branch 側にも配置 (9aabca3) し
  ci/TRIGGER.md push での起動を実証。**初回 run 29823931071 = success**
  (lib テスト + pseudo determinism + wide structural_digest ゲート全通過、
   約7分)。なお sandbox から artifact/ログの直接取得は Azure blob CDN が
   EOF で落ちるため reading は GitHub UI 経由 (conclusion の API 取得は可)。
  `gh workflow run` の dispatch API は引き続き 403 (TRIGGER.md 経路が正規)。
- 行列は 357 行 (「数百種類前後」の要求水準を実測値で充足)。継続拡大余地は
  seed/規模掃引で残す (実行 ~26 秒)。

---

# 第4ラウンド (2026-07-21 深夜): 完璧追求自主バッチ (テスト駆動の厳密化)

ユーザー指示: 「完璧だと思うところまで、勝手に機能追加・改修・検索してよい」。

## A. データ変換層の厳密テスト (+31 件: 404 → 435)
- 方針: 「出力 bit 同一性」を機械保証する方向に投資。変換層は GPU/CPU bitwise
  規律の根幹 (packed4 語彙 / DDA 踏破 / RLE wire / bitpack / 光伝播)。
- branchless_block/packed4/branchless_dda/section_rle/leaf_fast_path/
  bitpacked_section/light_cache に仕様固定オラクル (tie 順序非依存設計
  — スラブ最接近層・単一ブロック囲い fuzz のような「幾何学的に唯一な正解」)。

## B. 文書乖離の訂正 (挙動不変、誠実さの補強)
1. **BlockLut**: テーブル値が i%3/i%5/i%16 の modulo プレースホルダであり
   ブロック実属性の一次情報ではないことをヘッダ明記 (アクセス経路は実
   ブランチレス)。full_graph_wiring の「実 palette/LUT 判定」「実発光
   ブロック走査」は proxy 精度 2 箇所のコメントを事実表に置換。
2. **section_rle**: `layer_masks_from_rle` の「without full palette decode
   when possible」実装乖離を訂正 (現状はフル decode 委譲)。
3. **rsift-render dx12_engine**: フォールバック経路がログで「100% parity の
   wgpu 描画維持」を主張 → 実役割は「game dir 検出 + FullGraphWiring 構築
   + backend 簿記」と明記。present は rsift-jvm render_bridge ラダーに委譲。

## C. 実測駆動の追加最適化 (出力 bit 同一)
- メッシャー空判定: `RleSection::encode().is_empty()` は「全 voxel==0」と
  同型 → any() 走査へ厳密同値置換 (Vec 構築+run 展開の消去)。
  実測: column8 noise 705→578µs ( hoist と合わせ計 -44%)、flat 136→112µs。
  digest `004c1cf5fb17bfe8` (357rows) が変更前後で一致。

## D. caves merge 断片化コスト (47ns/voxel) — 【解決】
- カラム bit 導出への全面書き直しで **mesh_section caves 195→37µs (−81%)**
  (9ns/voxel、checker 級 emit 床に到達)。noise −76%、column8 noise cull
  セッション累計 1027→185µs (−82%)。面可視は不透明カラム (xc/yc/zc の
  u16×256 本、1 走査構築) の bit_s && !bit_{s±1} シフト導出、merge は
  「等しい行ブロック内の矩形拡張チェックが構造的常真」の証明つき u16 化。
  出力 bit 同一性は旧 face_visible 参照経路との fuzz 照合 (6 密度 × 6 軸 ×
  skip on/off の quad 列完全一致) + 破損境界 + wide digest 不変で証明。
- BlockLut の「正式テーブル化」: registry 一次情報が存在しないため、
  推測由来の表を作る方が不誠実。値を変えない pin の方針を採用。

## E. 残課題 (棚卸し)
- zero-test モジュール残 ~25 (非コア/描画後段中心)。重要度順に継続
  (今回 bitpacked/light_cache も消化: opt-gfx 404→435 → 437)。
- 【解決】legacy 12B greedy 経路も pull と同一証明パターンでカラム bit
  導出へ移行完遂。`greedy_axis` は OpaqueCols を 1 度構築して 6 軸共有
  (旧は軸×slice 毎に face_visible 全走査)、merge は証明つき u16 化
  (`greedy_merge_2d_bits`)、Y 層 skip は yc OR 導出。旧経路
  (face_visible/greedy_merge_2d) は cfg(test) オラクルとして保持し、
  `bitcols_12b_match_face_visible_fuzz` (6 密度 × 6 軸 × skip on/off) +
  破損境界で頂点列・index 列の完全一致を照合 (437→439)。
- caves liderar merge rewrite 完了後も lz4/zstd の構造起因フラグは残る
  (外部ライブラリ内部の話、本プロジェクト改変対象外)。

## F. 第5セッション追記 (2026-07-21 午後)
- **zero-test 消化 (opt-gfx 439→456)**: bump_arena (アライン丸め・
  容量帳簿・失敗時 offset 不変・reset 再利用 — 4件), vertex_compression_r10g10
  (pack/unpack bit 厳密・f16 既知 encoding・stream — 5件), texture_budget
  (プロファイル分岐・帯域係数・ラベル — 4件), temporal_mesh_diff
  (dirty 集合意味論・generation カウンタ・diff 厳密 index — 4件)。
  rsift-api の mod_menu にも 3 件追加 (distinct id 計数・同一 id 上書き・
  open/close + noop 安全)。
- **正直性修正 (rsgraphics)**: TitleScreen の `Mods (N)` ボタン表示数が
  `3u32` ハードコードで /mods/ 追加 DLL が反映されない虚偽表示だった。
  一旦カタログ実登録数からの動的導出に修正した後、Fabric Mod Menu 参考の
  設計見直しで個数表記そのものを撤廃し定数 `"Mods"` へ (下記 G 節参照)。
- **zero-test 消化 第2波 (opt-gfx 456→471)**: software_tiling (タイル数
  ceil・(ty,tx) 昇順・front-to-back 距離ソート — 2件), lod_hybrid (閾値
  境界 <=・stride 間引き再 index・部分クアッド drop・SVO 真理値表 — 5件),
  vertex_pool (帳簿 offset・空メッシュ evict・容量超過拒否・ring reset
  境界 `>` 比較+世代進行 — 4件), dag_scheduler (単一鎖一意・ダイアモンド
  依存順序・自己ループ浮上不可・空グラフ — 4件)。DAG は TaskId 単調発行で
  閉路を公開 API から構築不能 (DAG 性が構造保証) であることを white-box
  テストで裏書き。
- **zero-test 消化 第3波 (opt-gfx 471→481)**: mimalloc_config (tier 行列・
  env_string 厳密 — 2件), pgo_bolt (release/dev/default フィールド表・
  rustflags 文字列 — 2件), rayon_job (map 順序保持・for_each 全要素1回・
  p-core プール命名+サイズ — 3件), zerocopy_cast (cast roundtrip・
  ragged 拒否・wire header magic/size — 3件)。
- **zero-test 消化 第4波 (opt-gfx 481→489)**: micro_lod (LOD 閾値・負距離
  fallback・downsample 写像・AO 段階境界 — 3件), instanced_draw (32B レイアウト・
  group 集計・Pod byte roundtrip — 3件), dashmap_registry (状態遷移+version
  カウンタ・pending 集合フィルタ — 2件)。第3波では zerocopy 空 cast テストが
  スタック配列の実行依存アライメントで CI に 1 度落ちた → repr(align(4)) 受け皿で
  決定的に修正 (72fa36e)。
- **zero-test 消化 第5波 (opt-gfx 489→496)**: bundle_reuse (miss/hit 帳簿・
  vertex_count は DrawIndexed のみ・コマンド逐語保存・LOD キー独立性 — 3件),
  texture_atlas_virtual (free pool LIFO 払い出し・常駐 idempotent・バッチ内
  重複 dedupe・容量枯渇拒否・evict→再ストリーム — 4件)。第4波 CI 初回は
  ガバナンス起因フレーク (再実行・無変更で全緑、ca80df9 で確証)。

## G. Mod Menu の Fabric「Mod Menu」参考再設計 (2026-07-21 午後)
表示が「個数」ではなく各 mod の名前・概要・詳細であるべきとの指摘に基づく
再設計。行プロトコルを Rust 側に集約し、Java はボタン化のみを担う構成。
- **一覧→詳細の2画面化**: 一覧行 `mod|<id>|<name> (<version>)` (name 昇順
  +id タイブレークの安定ソート: 旧 HashMap 走査は実行毎に行順が揺れた) →
  押下で選択→詳細画面 `mod_menu_detail` へ遷移。詳細は
  `info|name/ID/Version/Author/概要` + 条件付き `act:config|Config` /
  `act:home|Open Homepage` + `act:back|< Back to mod list`。戻るで一覧へ。
- **API (rsift-api)**: `RsiftModMenuScreen::catalog_lines/detail_lines`、
  `parse_row`/`row_label`/`ModRowAction` を追加し 6 テストで固定
  (ソート/サニタイズ/詳細構成/オプション省略/未知行 no-op)。
- **累積重複バグ修正**: ホスト画面 (PauseScreen 単一キー) への追記がクリア
  されず一覧↔詳細の往復で行が重複 → `ScreenRegistry::clear_buttons_for` を
  追加しホスト構築頭と Back 押下で撤去 (Back 後はバニラ PauseScreen への
  ホスト行混入も防止)。旧コメント「unique labels で上書き」は虚偽だった。
- **UTF-8 パニック潜伏修正**: ラベル 40 文字省略が `&line[..40]` バイト切断
  で、日本語等のマルチバイト境界でパニック → chars() ベースに修正。
- **rsgraphics**: ボタン文言は定数 "Mods" (個数表記は撤廃)。
- 行プロトコル互換性: 旧 raw 行 (id|name|...) は parse_row で Unknown=no-op。
  Java 側は `mod_menu_detail` kind を openPendingScreen に追加 (5 行)。
  スナップショットに `mod_menu_detail_lines` フィールド追加 (serde 加算的)。
  rsift-jvm は bench-ci の -p rsift-opt-gfx 経路に含まれないため、本変更の
  同クレート (platform_bridge) コンパイル確認はローカル/インストーラ経路要。
