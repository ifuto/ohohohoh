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

## H. zero-test 第6波: 簿記/環境構築系 8 モジュール消化 + 横断潜伏バグ 3 件修正 (2026-07-22)
- **テスト追加 (opt-gfx 496 → 527, +31)**:
  descriptor_heap_ring (逐次 offset 規則・境界 wrap 保守設計の固定・full wrap・
  reset・**u32 境界回帰**・Dual リング隔離 — 6), enhanced_barriers (transition/UAV
  フィールド厳密表・flush の挿入順+drain — 3), boot_splash (既定状態・
  ライフサイクル・total=0 防衛 — 3), adaptive_shading (skip_stride 表・48/96 の
  厳格大なり境界・モーション 8.0 包含閾値・checkerboard パリティ遷移・
  **ジオメトリ非カリング誠実仕様** — 5), pso_library_cache (miss/hit 帳簿+上書き・
  **12B レコード wire 厳密**・親 dir 生成・既存 blob 無視設計 — 4),
  root_signature_optimized (graphics レイアウト厳密表・コスト重み表・
  **D3D12 64 DWORD 予算不変式** — 3), cpu_occlusion (固定カメラ/adaptive+任意
  カメラの**委譲 bit 一致** 2 経路 — 2), simd_kernels_avx2 (空/全填・
  単 voxel 単 bit・独立走査順オラクル fuzz・z ガード — 4)。
  別途 hzb_2d に回帰テスト +1。
- **修正1 (hzb_2d) — Hi-Z 永久無効化**: テレポート検出で立つ skip_frame が
  誰にも解除されず、以後 cull_boxes が常に早期 return (保守的全可視) となった。
  本クレート唯一の外部呼出経路 `render_pipeline.rs:815
  (cull_boxes_with_camera)` は begin_frame を呼ばないため、1 度の視点移動
  (XZ 2 ブロック or 視線角 0.08 rad) で遮蔽カリングが静かに死ぬ実害があった。
  早期 return を「消費型 1 フレームスキップ」に修正し、回帰テスト
  `teleport_skip_is_consumed_and_recovers` で tested 帳簿の再開を固定。
- **修正2 (descriptor_heap_ring) — u32 overflow**: alloc の `start + count` が
  u32 境界を跨ぐ組合せで debug ビルドは panic・release は暗黙 wrap → 
  `saturating_add` に置換 (非 overflow 入力では厳密同値、wrap 分岐の条件も
  飽和後値で単調)。回帰テスト `no_overflow_when_head_near_u32_max`。
- **修正3 (simd_kernels_avx2) — クロスアーキテクチャ出力乖離**: 非 x86_64
  fallback が「スラブ全域 any() で 0xFFFF 全立ち」の別物実装で x 位置情報を
  喪失していた。全アーキテクチャ単一実装に統一 (x86_64 経路はループ同形で
  出力 bit 不変)。併せて無根拠な「100倍高速化」主張を撤去し、自動ベクトル化
  委譲の実態を明記。
- **文書訂正 (pso_library_cache)**: 「キャッシュがあればヒットとして扱う」は
  虚偽 (実態は読み捨て・復元なし) → new() に「12B ワイヤ形式は全エントリ復元
  に不足するためロードしない設計」と明記。dead な read 分岐を撤去。
- (運用メモ) 本波着手時にサンドボックスの git ref がベースコミットへ巻き
  戻っており、リモートブランチ (CI 緑の最新) を正として `--mixed` 復旧した上で
  作業。リモートの CI 成功群は全てこの後の push でも損なわれない。

## I. zero-test 第7波: eco_render/gpu_culling/gui_settings 消化 (+19: 527→546) + 起動クラッシュ級バグ 2 件等の修正 (2026-07-22)
- **重大修正1 (gui_settings) — 「動画設定」ボタン即死パニック**: `slider_bar` が
  char 位置 pos を**バイト範囲** `pos..=pos` として `str::replace_range` に渡して
  おり、3-byte グリフ「─」の境界要件 (pos と pos+1 の同時 char 境界 = 不可能) で
  **必ずパニック**。既定タブ Performance がオープン直後に row_slider を描画するため
  GUI オープン = 確実にクラッシュ (rsgraphics「動画設定」ボタン → open_global_settings
  / rsift-launcher lifecycle から到達可能)。char 配列置換に修正し、全タブ GUI スモーク
  + slider 幾何 (端点・飽和・縮退長・width 0) を厳密固定。
- **重大修正2 (gpu_culling) — default_frustum 全件カリング矛盾**: far 面 w が -512
  (near z>=0.1 と論理両立不可)・側面 4 面の w も -256 で内法線が裏返り、本テーブルで
  構成した frustum は**全チャンクを不可視**と判定する設計破綻 (現状どこからも
  呼ばれていないため潜伏)。内法線ボックス (z∈[0.1,512], x/y∈[-256,256]) に訂正し、
  可視性意味論 (inside/near/far/left/right/top/bottom 7 箱) を厳密固定。
- **修正3 (eco_render)**: `ChunkSlicePool::new(0).acquire()` の剰余ゼロ除算パニック
  防御 (空スライス返却)。VisGraph 旧コメント「6×6×6」は実装 (8×4×2/64bit bitset) と
  不一致だったため訂正。
- **検証基盤**: culling WGSL をモジュール const (`GPU_CULL_SHADER_WGSL`) へ切り出し、
  naga パース + entry point 実在テストを追加 (frame_pipeline.rs と同パターン、
  GPU 不要の真の構文検証)。
- テスト内訳 (opt-gfx 527 → 546): gui_settings 7 (palette 厳密/tab 表/glyph/
  slider 幾何/preset 上書き表 3 種/adaptive 不変式/GUI ライフサイクル全タブ),
  gpu_culling 6 (POD レイアウト 48/20/128B + bytemuck 往復/frustum 表厳密/可視性
  意味論/短縮 commands 安全性/adaptive≡直接 bit 一致/WGSL naga), eco_render 6
  (mark 冪等 + MAX_BITS ガード/内蔵判定 (63・52・84 の厳密)/pool ローテーション
  + 0 防御/region 集計/estimate 両端 + message 厳密/div_euclid region + 未占有
  スキップ + 洗い替え)。
- 残 zero-test: `full_graph_wiring` (2009 行の統合モジュール — 入力 scaffolding
  設計が要るため次波) のみ。lib.rs は mod 宣言と再エクスポートのみで検証対象
  ロジックを持たない。

## J. naga 0.20 拒否の二分探索確定 + 最終 zero-test モジュール消化 (2026-07-22)
- **naga 0.20 WGSL 拒否の原因確定 (D0→D1→D2→C3→C4)**: 第7波初回 CI が
  lib tests 失敗 (exit 101)。ログが sandbox から取得不能なため CI を 1bit
  オラクル化した二分探索を実施:
  D1 (wave-7 テスト全 cfg 切断) = 緑 → 本番差分と新旧527テストは無罪、
  D2 (naga WGSL テストのみ ignore) = 緑 → `culling_wgsl_parses_with_main_‥`
  単独が実行時失敗と確定 (コンパイルは両波とも通過 = ランタイム失敗)、
  C3 (@compute 手前の宣言プレフィックスを単独パース) = 緑 → main 本体内問題、
  C4 (main 内の**空の if ブロック撤去** + テスト再有効化) = 緑 →
  真犯人は「コメントのみを含む空 if ブロック」だった。
  naga 0.20 のパーサ実読 (gfx-rs/wgpu v0.20.0 タグ) でも trailing comma 受理・
  空ブロック済の `block` ループを確認済なのに lower 側で拒否される実測値が得ら
  れたため、「naga 0.20 は空 if を含むモジュールを parse_strで拒否する」を
  本診断の結論として記録する (エラーメッセージ本体は sandbox 制約で未回収)。
  本番 WGSL は空 if をデッドコードとして完全撤去し設計メモをコメント化。
- **文書訂正 (full_graph_wiring)**: `corner_ao_from_palette` の「戻り値 0..3
  (3=最大遮蔽)」は `ao_bake::corner_ao` が返す **vanilla 輝度スケール**
  (3 = 無遮蔽で最も明るい / 0 = 最大遮蔽) の逆転虚偽だったため訂正。
- **wave 8 (opt-gfx 546 → 555, +9) — 最終 zero-test モジュール
  full_graph_wiring (2009 行) 消化**: 純粋関数層の厳密固定:
  out_sign (6 face 表 + 未知 face フォールバック), view_proj_to_m16
  (列優先転置の逐語表), extract_frustum_planes (単位行列の G-H 面表厳密 +
  スケール不変性 + ゼロ行列 NaN ガード), mean_sigma (空フォールバック
  (0.5,0,0)/(0.1×3) と population 分散の厳密値), material_independent_hash
  (FNV 変形の golden 5 件 (python 事前計算済) + 順序依存性),
  corner_ao_from_palette (3 軸 face の隣接写像表・輝度 3/2/0 ケース・OOB 規約・
  マルチセクション y 階層参照 — 7 ケース 1 テスト),
  collect_all_wgsl (gpu_runtime 全ソースの順序連結に厳密一致 + assoc 委譲),
  ao_refine_quads (FullGraphWiring::new を一時 dir で実構築 = 配線オーケスト
  レーターの生成が opt-gfx 自身のテスト系でも検証される初ケース。
  s1 減光→2 への書き換え・max() での不変側・変更数実測・AO 以外の packed
  フィールド厳密保存)。
- **サンドボックス git ref 巻き戻り (2 度目)**: C3 push 前にローカル ref が
  base commit へ再リセットされており、non-fast-forward 拒否がリモート喪失を
  未然防止。fetch 済みオブジェクトから `reset --hard` で正規復旧。教訓:
  push 前に `git log` で親確認 (親が base しか無ければ巻き戻り発生)。

## K. tick_world 全通読監査 (2026-07-22, 第 9 波)

`FullGraphWiring::tick_world` 本体 (旧行 351-1773、約 1400 行) を全区間通読し、
パニック経路・非決定性・虚偽配線の 3 観点で監査した。全添字アクセスは
`get`/`unwrap_or`/明示境界チェック経由でパニック経路なし。除算・剰余も
`.max(1)` ガードまたは非ゼロ定数分母に限定される。HashMap 反復は report に
`len()` 経由でのみ流出し順序は観測不能 (`gb_handles` の LRU 追い出しのみ
first-key 依存だが試験規模では容量到達しない)。

発見と修正 (全て本コミットで実施):

- **K-1 (虚偽配線) Aokana リージョン登録が無条件 (0,0,0) + svdag 空化**:
  実セクションパレットから構築した実 DAG を `insert_shallow_region(0,0,0,..)`
  で「どのチャンク由来でも原点区画」と虚偽登録し、さらに `self.svdag.take()`
  で実構築物を aokana に移したあと self.svdag へ**空の新規 DAG** を残していた
  (svdag フィールドは実データを永遠に保持しない)。aokana 規約
  (`region_size_blocks=64` = 4x4 チャンク区画) に合わせ、供給チャンクの実座標
  (`cx.div_euclid(4)`, `cz.div_euclid(4)`) で登録するよう修正。svdag には実 DAG
  を保持し aokana には clone を登録する。`section_palettes` がチャンク内 y 帯を
  保持しない制約は `ry=0` 登録としてコードコメントに正直明記。
- **K-2 (単位不整合) bobby `CachedChunk.time_ms`**: `delta_ms * 1000.0` (µs 相当)
  を time_ms 欄へ格納していた。time_ms は eviction 順序付けにのみ使われるため、
  壁時計非依存で単調な**フレーム tick** を擬似時刻に変更 (決定性維持 + 順序情報
  の実体化)。
- **K-3 (符号拡張汚染) region_codec ペイロード混合**: `k.0 as u64` は負座標で
  `0xFFFF_FFFF_xxxx_xxxx` に符号拡張され、`(k.1 as u64) << 32` との XOR で上位
  32bit が破壊されていた。零拡張同士の単射的ビット配置
  `(k.0 as u32 as u64) | ((k.1 as u32 as u64) << 32)` に修正。
- **K-4 (非決定フィールドの明文化)**: `FrameWiringReport` の
  `vanilla_hook_hits_delta` (プロセス全域カウンタ由来 → テスト並行実行で混濁
  し得る), `power_skip_extra` (電源モード判定が壁時計を要求する仕様) の 2 つに
  非決定である旨の doc を明記。残り 15 フィールドは決定性検証で比較可能。
- **K-5 (wave 9: opt-gfx 555 → 558, +3) tick_world 統合テスト**:
  (1) 空入力 4 tick の well-formedness (CLP 既定 0.0 ビット厳密、露光は
  adapt クランプ域内)。 (2) 1 チャンク/全 1 パレット入力での
  **厳密値** (K-1 回帰: aokana_visible_regions==1 を冠点テストから静的導出、
  3 層フィルタ全通過検証済み draw_command_count==1, frb_billboards==24) +
  2 インスタンス交差 bit-決定性 (15 フィールド、f32 は to_bits)。 (3) 601 tick
  で tick%600 SVDAG 再構築 / pso_lib.save / tick%120 CLP 周期跨ぎの交差決定性。
- **K-6 (wave 9 失敗→根治の記録)**: 初回 push で lib テスト exit 101 (2 連続 =
  非フレーク)。診断 B1 (chunked 単独分離) = 緑 → empty テストの
  `visgraph_reachable == 0` 仮定が誤りと確定。visibility_graph::flood_fill の
  設計は「始点 (dist=0) は opaqueness 非適用で結果に常時含む」(同モジュール
  自身のテスト群で固定済み) のため、正しい期待値は 1。空入力でも
  cam_chunk=(0,0) 始点の 1 ノード。テスト期待値を訂正し再実行。
  教訓: 「0 であるはず」は曖昧な直感。厳密値は対象モジュールの固定済み
  セマンティクスから導出する (本監査の基本原則どおり)。

## L. ページング/リージョンコーデック監査 (2026-07-22, 第 10 波)

`out_of_core_paging.rs` (104 行) と `region_zstd.rs` (249 行) を全行読了。
修正 + テストを本コミットで実施。

- **L-1 (サイレント破壊バグ) OutOfCoreMmapPaging のページ置き換え**:
  旧実装は `next_page_idx % max_pages` のラウンドロビン回送で、ページを剥奪
  された旧チャンクの `page_table` エントリを除去しなかった。`max_pages`
  を超える別チャンク書き込み後、**新旧 2 チャンクが同一物理ページを共有し、
  古い側の read が無警告で他チャンクの内容を返す**破壊的バグ。
  さらにモジュール doc の「memmap2 + LRU」は実態と無関係な虚偽だった
  (実体はプレーン File I/O の回送)。真 LRU (read/write で最新化、最古キー
  完全剥奪) + doc 正直化で根治。容量内のページ割当列 (0,1,...) は旧実装と
  同一 (bench digest 互換)。
- **L-2 (パニック) `max_pages == 0` で剰余ゼロ**: write 時 `x % 0` パニック。
  `new` で理由付き Err を返すよう変更 (呼出側は 4096 で誘導されず latent)。
  併せて `max_pages > u32::MAX` / `checked_mul` の usize 溢れも拒否。
- **L-3 (境界パニック) region_zstd `put_chunk/get_chunk`**: `debug_assert`
  依存でリリースでは配列境界パニックだった。bool 返却 / None 拒否に変更
  (全呼出側は戻り値非使用のためソース互換)。
- **L-4 (統計虚偽) region_zstd `stats()`**: Stored (無圧縮) で zstd デコード
  失敗により raw 未計上のまま比を構築。両辺計上で厳密 1.0 となるよう訂正。
  併せて location 24bit+8bit 形式由来の >255 セクタ制約を正直注記。
- **L-5 (陰性確認の記録) gpu_culling WGSL/Rust レイアウト照合**: 先行
  セッションで懸念されていた ChunkBox WGSL ~64B vs Rust 48B 不一致説を
  全メンバオフセットで照合。WGSL storage アドレス空間: min_xyz@0(vec3,
  align16) / is_visible@12 / max_xyz@16 / chunk_idx@28 / tex_id@32 /
  pad0-2@36..48、struct サイズ = roundUp(16,48) = 48B。Rust repr(C):
  同一オフセット・48B。IndirectCommand 20B / FrustumData (uniform)
  96+16+16 = 128B も一致。**不一致は誤認で不一致なし**と確定記録。
- **L-6 (wave 10: opt-gfx 558 → 563, +5)**: paging 4 本 (全域ビット往復、
  0 ページ拒否、LRU 剥奪+read 最新化の被害者決定、超過ペイロード打止め) +
  region 2 本 (OOB 非パニック拒否、Stored 厳密 1.0)。

## M. render_pipeline 全通読監査 (2026-07-22, 第 11 波)

`render_pipeline.rs` (1265 行) を全行通読。パニック経路は確認されず
(`adaptive_mesh_interval` は 1..=8 保証で剰余安全、`camera_section_y() - 1` は
div_euclid 後の縮小域で溢れなし、`mesh.vertices[0]` は空イテレータ短絡で
未到達)。発見と修正:

- **M-1 (実効上の虚偽) motion adaptive shading の「カメラ速度」**:
  `speed = 6.0 / delta_time` は実変位と無関係の一定式で、60fps (=16.6ms) では
  常時 ~375。`set_camera_speed(min(40.0))` で毎フレーム 40 = 最高速固定となり、
  距離/モーション適応シェーディングが静止時も常に最低品質へ倒れていた。
  前フレームとの実変位 / delta_time (blocks/sec) 計測に根治
  (`prev_camera_xyz` 追加)。静止時は厳密 0.0。
- **M-2 (非対称の記録) leaf_fast_path 適用条件**: live 路は
  `low_spec.leaf_fast_path` のみを見て常時 true 適用 (`profile.leaf_fast_path`
  無視)、demo/ノイズ路は両フラグの OR を有効条件とする。現時点では挙動
  据え置きでコメント記録のみ。
- **M-3 (wave 11: opt-gfx 563 → 565, +2)**:
  `camera_speed_measured_from_real_displacement` (feather 強制有効 + live
  ingest で実変位厳密値: 16 b / 16ms = 1000 b/s ビット厳密、静止 0.0 固定)、
  `frame_demo_stats_cross_instance_deterministic` (3 チャンク 2 フレーム ×
  新鮮 2 インスタンスで期間起動 %120 系を除く全カウンタ厳密一致 +
  wiring_subsystems==60 + 静止カメラ速度 0.0)。

### M-4 (zero-day 確定: 空バイト列の bytemuck アライメントパニック)

第 11 波の統合テスト過程で、`render_pipeline::frame()` がデモ (非 live)
経路で**必ずパニック**することを実観測。CI ログがダウンロード不能なため、
catch_unwind + process::exit の終了コード化で byte チャネルを構成し、
B1〜B22 の段階的切り分け (座標系 → サブシステム系 → 交互対照) を経て確定:

`zerocopy_cast::cast_bytes_to_slice` が `bytemuck::cast_slice` を裸で呼び、
長さ検査のみで**アライメント検査がなかった**。空の `Vec<u8>` は dangling
ポインタ (align 1) を返すため、`quads.len()*8` は 0 で長さ検査を通るのに
`PackedPullQuad` (align 4) 側のアライメント検査でパニック
(TargetAlignmentGreaterAndInputNotAligned)。`frame()` の
`low_spec.quad_budget > 0` 経路 (既定プロファイルで有効) が pull バイト 0 の
フレームでこれに到達する — つまり**通常のデモ実行のみで再現する latent
panic** で、frame() を立ち上げるどの既存テスト/ベンチもこれを通っていなかった
ことが、本 wave の matrix 分離により構造的に証明された。

- 修正: `cast_bytes_to_slice` に align 検査を追加 (不整合は `None` 拒否、
  呼出側は従前どおり `unwrap_or_default` で空扱い)。併せて `frame()` 側も
  `!self.gpu_quad_bytes.is_empty()` を前置して防御を二層化。
- テスト追加: `cast_bytes_to_slice_empty_vec_is_none_not_panic`
  (zerocopy_cast, +1) ほか、M-3 の 2 本がそのまま M-4 の回帰検証
  (実 frame のデモ 3 チャンクもパニックせず決定性を保つ) となる。
  opt-gfx 563 → 567 (+4: speed 1 + det_core 1 + det_pull 1 + align 1)。
- 診断装置 (プローブ/終了コード macro) は確定後に全撤去済み。

---

## N. bobby_cache.rs 全通読監査 (wave 12, 2026-07-22)

Bobby ローカルキャッシュのストレージ層 (`.rcc` 形式・索引再構築・LRU) を全行監査。
ドキュメント冒頭の「ストレージ層 (圧縮/耐久性/LRU整合) を完全実装する」という
宣言を検証基準として照合した結果、LRU 整合に関する**一度限りではなく再起動を
跨いで発現する**設計バグを特定・根治した。

### N-1 (重要): rebuild_index の time_ms=0 埋め → 再起動後 LRU が非決定に堕落

索引の再構築時、ディスク上の `.rcc` ヘッダには `time_ms` (offset 20..28) が
保存されているのに**読まずに 0 で埋めていた**。結果、プロセス再起動後は索引の
全エントリが `time_ms == 0` の同時刻となり、`enforce_lru` の `sort_by_key`
(同一キー→元の反復順) が **HashMap の反復順 = 実行ごとに非決定** で削除する。
「最古チャンクから捨てる」という Bobby の purge 意味論が再起動後に完全に失われ、
さらに削除結果が再現しない (= このリポジトリの決定性文化にも違反)。

- 修正: `read_header_time_ms` (先頭 28 バイトだけ検証付きで読む) を新設し、
  rebuild 時に実時刻を復元。読み取り/検証失敗の壊れファイルは 0 (最古) 扱いで
  真っ先に捨てる。
- 併せて `enforce_lru` のソートキーを `(time_ms, dim, cx, cz)` に拡張し、
  時刻同値でも削除順が完全に決定的になることを仕様として明文化した。
- 厳密テスト:
  - `rebuild_restores_time_ms_from_header` — cfg(test) 専用 getter
    `index_time_ms` で rebuild 後の索引を**直接**観測 (旧実装では Some(0) と
    なり決定的に失敗する)。
  - `lru_after_rebuild_evicts_true_oldest` — ~600KB × 3 (~1.8MB) を
    t=100→座標9, t=200→座標5, t=300→座標1 と**時刻と逆相関する座標**で保存。
    rebuild 後に 1MiB 上限で purge すると、旧実装 (全時刻 0) では tie-break の
    座標最小 = 最新チャンクが真っ先に消えて決定的に失敗し、修正後のみ
    「最古 2 件が消えて最新が残る」で緑。ペイロードは LCG 疑似乱数
    (zstd 非圧縮性) でサイズを厳密に制御し、フレーク要因を排除。

### N-6 (中): 次元ディレクトリ名 parse 失敗の `.unwrap_or(0)` → dim=0 汚染

`bobby_cache/<server>/<dim>/` の `<dim>` が数値でない外部混入ディレクトリ
(例: "junk") だった場合、旧実装は `unwrap_or(0)` で **dim=0 として索引**した。
正当な (0,cx,cz) エントリとキー衝突して上書きされる (= 索引の静かな破壊)。
修正: 数値に parse できないディレクトリは skip。
テスト `rebuild_ignores_foreign_dim_dirs` — 手仕込みゴミ混入後に rebuild し
`cached_count == 1` を厳密検証 (旧実装は 2 で決定的失敗)。

### N-5 (中・誠実性): doc の圧縮レベル 19 宣言 vs 実装 13

ヘッダコメントの形式仕様に「level 19 相当で圧縮」とあるが実装は
`encode_all(.., 13)`。**実装は変更せず doc を正直化** (L-1 の
「memmap2+LRU」嘘 doc 正直化と同種の判断: 既存キャッシュ/ベンチの
ビット挙動を変えないことを優先)。

### N-2 (低・防御): parse_chunk_name が 3 セグメントを受理

`"1_2_3.rcc"` のような形式外ファイル名が Some((1,2)) として索引に混入
し得た。`{cx}_{cz}` ちょうど 2 セグメント以外を拒否するよう厳密化。
テスト `parse_chunk_name_strict_two_segments` で Some/None の境界を列挙固定。

### N-4 (低): store の index 更新で poison 黙殺

`if let Ok(mut idx) = self.index.lock()` は poison 時に索引更新を**黙って
捨て**、ディスクと索引を不整合にする。他のロック取得 (unwrap) と統一し、
poison は panic として可視化 (誠実性ルール: 静かな嘘より派手な失敗)。

### テスト数

opt-gfx 567 → **571** (+4: rebuild 時刻 direct / rebuild 後 LRU / dim junk skip /
parse 厳密)。全テスト名は決定的 red→green 対応が設計段階で証明済み
(直感値ではなくモジュール固定セマンティクス = ファイル形式レイアウトと
ソート規則から導出。K-6 教訓の運用)。

---

## O. binary_greedy_meshing.rs 全通読監査 (wave 13, 2026-07-22)

メッシング心臓部 (1533 行) を全行監査。bench 決定性ダイジェストの根幹のため、
新旧実装の等価性主張を手計算で再証明することに主眼を置いた。

### 陰性確認 (再証明済み・問題なし) — O-1..O-15

- bit 化 merge の等価性証明: 旧 `greedy_merge_2d_pull` で「等しい行ブロック内の
  矩形拡張チェックは構造的常真」→ `actual_rows == row_span` 恒等、col_span は
  row_bits の連続 run 長に一致。新 `*_bits` 版の trailing_zeros/ones 走査は
  emit 順序 (row-major・左 run から・行ブロック単位) まで同一。数学的に確認
  (既存 fuzz オラクル 4 件と二重の裏付け)。
- `face_rows_for_slice`: 3 軸の rows マッピング (Y→(z,x)/X→(z,y)/Z→(y,x)) が
  旧 mask 添字規約と一致。slice=15 の pos 面は `col >> 16 == 0` (u16→u32 昇格
  のおかげ) で `bit_15` に正しく帰着 = is_opaque OOB 規約と厳密一致。
- `run_mask = (((1u32 << span) - 1) << col0) as u16`: col0+span ≤ 16 が不変条件
  のため上位溢れなし、切り詰めは正確。
- `extract_bitboard_span`: tz 走査 + `min(64-start)` clamp が全境界で正しいこと
  を手計算確認 (テストで固定化も実施)。
- `emit_pull_quad` の clamp/`block==0` 早期 return/`w.max(1)` は全て構造的 no-op
  の防御 (mask bit ⟺ origin voxel 不透明の対応を 3 軸で検証)。
- `mesh_chunk_column_pull_world` の 64³ window 再パック: x+w ≤ 15+16 = 31 < 64 で
  COORD_BITS 6bit 内、drop 判定・face/light_ao 持ち越しも正しい。
- 非空セクション ⇒ 面 ≥1 (外周存在) が成立するため `is_empty` の取りこぼしなし。

### O-16 (中): 未接続の unsafe SIMD に等価性テスト不足

`bitboard_slice_cull_swar` / `bitboard_slice_cull_avx2` /
`extract_bitboard_span` の 3 関数は**モジュール外のどこからも呼ばれていない**
(将来の隣接チャンク横断カリング用の公開ユーティリティ)。特に AVX2 版は
`unsafe` + `#[target_feature]` で、正準 SWAR 版との照合テストが存在しなかった。
- 対応: `bitboard_avx2_matches_swar_when_available` (検出済み AVX2 時のみ実行、
  全 0/全 1/混在パターン 5 ケースで厳密一致を検証) と
  `bitboard_span_edge_cases` (0/全1/bit63 clamp/複数 run の最下位規約) を追加。
- 併せて「現時点で本番経路からは未呼出」であることを doc に明記 (誠実性:
  「ultra-fast neighbor culling」とだけ謳うと稼働中の最適化と誤読される)。

### O-24 (低・誠実性): `face_culling` パラメータの命名と実体の乖離

`mesh_chunk_column_pull` / `mesh_chunk_column` の `face_culling` は実体として
「Y 空層スキップ」の可否であり、**出力には一切影響しない** (空マスク merge の
省略は純粋な perf スイッチ。true/false で出力 bit 同一)。両関数の doc に明記。

### テスト数

opt-gfx 571 → **573** (+2: span 境界 / AVX2↔SWAR 等価)。

---

## P. frame_worldgen.rs 全通読監査 (wave 14, 2026-07-22)

Phase E の wgpu 実 dispatch 配線 5 系統 (noise upsample / LBVH / bindless /
half-vertex / meshlet cull) + Gribb/Hartmann frustum 抽出を全行監査。

### 事実関係の記録 (接続状況)

CPU 精密ミラー関数群 (`noise_coarse_cpu` 等) と GPU ラッパ (`GpuNoise` 等) は
**現時点では本番フレーム経路からは呼ばれておらず、消費者はモジュール内
テストのみ** (wgpu 配線の実働検証ハーネスとしての位置付け)。doc の
「実 dispatch 配線」は「GPU 配線が実装された」意味では正確だが、フレーム
パイプライン組込みではないことをここに記録する (O-16 bitboard と同種)。

### 陰性確認 — P-1〜P-4

- frustum_planes: identity view_proj での 6 平面が理論値表と一致
  ((1,0,0,1),(-1,0,0,1),(0,1,0,1),(0,-1,0,1),(0,0,1,0),(0,0,-1,1))、
  wgpu z∈[0,1] の near=r2 規約も正しい。正規化は d 項も割る (radius 比較に必須)。
- P-2: coarse→fill を同一 compute pass で連続 dispatch している件は、
  WebGPU 同期モデル (同一 pass 内 dispatch の program order 可視性) で保証
  されるため問題なし。BGL binding 1 の read_write は 2 pipeline 共有由来で妥当。
- P-3/P-4: `lbvh_codes_cpu` の clamp(0, 0.9999)*1024 → 最大 1023 で morton3 の
  10bit 入力域に収まる。span=0 なら NaN/inf→saturating cast の経路があり得るが
  span>0 は呼出契約 (現消費者はテストのみで 128 固定)。

### P-5 (中): `coarse_dims` の stride=0 除算パニックが無契約

public `coarse_dims` は `size / stride` のため **stride=0 で整数 0 除算パニック**
となるが、契約が未記載だった。対応: doc 明記 + メッセージ付き `assert!` で
明示拒否 (静かな未定義動作より派手な失敗の原則)。should_panic テストで固定。
なお既存の呼出規約は全て `coarse_dims` 経由 (coarse/fill 両 CPU ミラーと
`GpuNoise::run`) のため 1 箇所の assert で全経路がカバーされる。

### P-6 (中・誠実性): CPU ミラーが planes 全件走査で「完全一致」に反し得た

WGSL 側は uniform の `[[f32;4]; 6]` 固定配列 + `plane_count = min(6)` のため
**先頭 6 平面のみ**で判定するのに、CPU 精密ミラー (`lbvh_cull_cpu`,
`meshlet_cull_cpu`) は渡された planes を全件走査していた。7 個以上渡すと
CPU≠GPU となり「完全一致」の doc 宣言が嘘になる。
修正: 両ミラーを `.take(6)` に変更し厳密な動作一致を回復。
テスト `cull_cpu_mirrors_gpu_plane_limit_of_six`: 7 番目に全カリング平面を
置いても結果不変であることを両ミラーで固定 (旧実装は決定的に失敗)。

### テスト数

opt-gfx 573 → **575** (+2: 6平面制限 mirror / zero-stride 拒否)。

---

## Q. frame_postfx.rs 全通読監査 (wave 15, 2026-07-22)

CAS / Checkerboard / Exposure / VRS の wgpu 実 dispatch 配線 + 共通
readback ヘルパを全行監査 (read_f32_buffer/read_u32_buffer は wave 14 の
GpuNoise/GpuLbvh/GpuMeshlet 等が依存する読み戻し根幹)。

### Q-1 (重要・誠実性): GpuExposure の apply 経路が未完 — 宣言と実体の乖離

`GpuExposure::new` は `cs_apply` のコンピュートパイプラインを生成して
**即座に `let _ = apply_pipeline;` で破棄**しており、構造体は luma のみ保持・
`run_apply` メソッドは存在しなかった。ヘッダ doc の「Exposure: luma 計算と
露出適用を GPU」という宣言に対し **GPU 適用経路が実在しない嘘**だった
(生成コストだけ毎回支払う dead 生成でもあった)。
根治: `apply_pipeline` を構造体に保持し `run_apply` を実装
(WGSL 確認済: binding 2 = luma_out / binding 3 = dst、cs_apply は
exposure uniform を使用。apply 側はダミー luma バッファを binding 2 に
bind。alpha 透過・成分 clamp は CPU ミラーと宣言どおり一致)。
テスト `apply_run_cpu_exact_bits_and_clamp` は CPU ミラーの
厳密ビット仕様 (0.5*2.0==1.0 厳密、4.0→clamp 1.0、負→0.0、alpha 透過)
を to_bits 比較で固定 (GPU 実行テストは CI に GPU がないため契約側を固定)。

### Q-5 (中): vrs tile=0 で 0 除算パニックが無契約

`vrs_run_cpu` / `GpuVrs::run` の `div_ceil(tile)` は tile=0 でパニック。
motion/var 長さは assert 済みなのに tile は未検査だった。両関数に明示
assert を追加 (should_panic テストで固定)。wave 14 P-5 と同種の
「契約未明記の裸パニック」。

### Q-6 (中): cas/checker CPU ミラーの src 長さ未検査

`cas_run_cpu` / `checker_run_cpu` は w*h より短い src で OOB パニック
(長過剰は静かに余分を無視)。vrs と同じく明示 `assert_eq!` を冒頭に追加
(should_panic テスト 2 件で固定)。

### 陰性確認 — Q-2/Q-3/Q-4/Q-7

- `read_u32_buffer` の f32 経由 to_bits: from_le_bytes→to_bits は Rust が
  ビット完全 roundtrip を保証 (現行 u32 用途は 0/1 マスクで実害なし)。
- CAS の 1e-7 近似照合テスト: cas.rs::cas_sample との演算順同一で実質
  差分ゼロ (強化余地はあるが現状で契約は固定済)。
- checker の alpha 平均: RGB と同じ規則で WGSL/CPU 一致、呼出契約は α=1。
- luma Rec.709: 加算順固定で bitwise テスト済。

### 接続状況の事実記録

wave 14 と同様、本モジュールの CPU ミラー・GPU ラッパの消費者は
モジュール内テストのみ (本番フレーム経路からは未呼出の検証ハーネス)。

### テスト数

opt-gfx 575 → **579** (+4: apply 厳密bit / vrs zero-tile / CAS・checker 長さ拒否)。

---

## R. frame_ddgi.rs 全通読監査 (wave 16, 2026-07-22)

DDGI probe volume の実 2 pass GPU dispatch (probe_rays + atlas_blend) +
CPU 精密ミラー + 実サンプラを全行監査。既存の解析的真値テスト
(埋込み厳密 moments / 空ボリューム sky / 実ツリー bitwise アンカー) は
高品質であり、本 wave は「GPU バッファ設計と公開 API の無検証」という
構造的ギャップに集中した。

### R-8 (高): GPU rays_buf の 64 スロット固定設計が公開 API で未強制

`rays_buf` は `probes_capacity * 64 * 4` バイト (プローブ毎 ray 64 本固定)
で割当てられるのに、`set_inputs` は `ray_count > 64` を**一切検査しなかった**
(4096 上限の dirs_buf チェックのみ)。ray_count=65+ の体積を渡すと dispatch は
probes×ray_count 分を走り、**storage 配列の静かな OOB 書き込み**となる。
CPU ミラー経路は影響を受けないため「GPU==CPU bitwise」検証があっても発見
されにくい形状。

### R-4 (中): march の 129 開始値上限が契約未記載

`probe_march_cpu`/`probe_rays` は t=0.5 から 0.5 刻みで 129 開始値
(`0..=128`)。t < max_dist の全格子点を完全走査できるのは
**max_dist ≤ 65.0** のときのみ。超過時は打ち切りで遠方ヒットが欠落し
「全 miss = 偽の sky irradiance」となるサイレント誤結果。

### R-7 (中): oct_w=0 で sample_visibility が u32 アンダーフロー → OOB

`oct_w=0` は texel_count=0 (空 atlas が定義できる形) となるが、
`oct_texel_from_dir_wgsl` の clamp 分岐 `oct_w - 1` が u32 アンダーフローし
巨大 index で mom/irr の OOB パニックに化ける経路があった。

### R-3 (低〜中): sky>0 が CPU 直構築経路で未強制

`sample_visibility` は `ir[c] / sky[c]` で正規化するが、sky>0 の検査は
GPU `set_inputs` のみで、CPU で `DdgiAtlas` を直接構築する経路は NaN 伝播
の余地があった。

### 根治: `DdgiVolumeDef::validate()` への一元化

上記 4 契約 (oct_w≥1, ray_count∈1..=64, 0<max_dist≤65.0, dims≥1+積 checked,
sky>0) を**純粋関数 validate に集約**し、`set_inputs` (GPU) と
`ddgi_update_cpu` (CPU ミラー) の両入口で強制。GPU なし環境でも全契約が
テスト可能になり、既存の散在チェック (sky ループ) は validate に一本化
(ドリフト源の二本立てを解消)。max_dist=65.0 ちょうどは受理境界として
テスト固定。ヘッダ doc に契約を明文化。

### 副次記録 — R-10: read_f32 読み戻しの 3 重複

同一 map+Wait 読み戻しが frame_hiz::read_u32 / frame_postfx::read_f32_buffer /
本モジュール read_f32 に 3 重複している (いずれもコメントで相互参照済)。
共通化は横断変更となるため今回は記録のみ。

### テスト数

opt-gfx 579 → **582** (+3: validate 受理境界 / 契約違反 7 分野拒否 /
ddgi_update_cpu 入口拒否 should_panic)。

---

## S. frame_pacing / FSR1 3連鎖 / frame_reference 深度監査 (wave 17, 2026-07-22)

ユーザー指示: 「完璧だと思うところまで続けてよい、勝手にデバッグを行う」の継続。
未通読だった `frame_*` 系列のうち 4 モジュール (frame_pacing 82 / fsr1.wgsl・fsr1.rs /
frame_reference 752 / frame_fsr1 357+行) を全行監査。**アルゴリズム偽装 1 件・
GPU 乖離 1 件・実質ハング 1 件**を含む 8 件を修正。opt-gfx 582 → **594** (+12)。

### S-1 (🔴 アルゴリズム偽装) FSR1 RCAS が「ぼかし」だった (wave 1 A1 と同種)

`shaders/fsr1.wgsl::fsr_rcas` とその精密ミラー `frame_reference::fsr1_reference` の
両方が `out = c + lap * sharpness` (lap = 4近傍平均 − 中心) で、中心画素を近傍
平均へ近づける = **鮮鋭化ではなくぼかし**。`fsr1.rs::rcas` (正しい `p - lap*s`、
「加算だとブラーになる」の注意書き付き) や wave 1 で公式照合修正済の
`cas.rs::cas_sample` と符号が矛盾しており、FSR1 GPU 実パス全体
(GpuFsr1Pass: EASU→RCAS 2 dispatch) が「RCAS を名乗る平滑化フィルタ」だった。
- 修正: WGSL とミラーを `c - lap * sharpness` に同時反転 (3連鎖の符詞整合:
  fsr1.rs::rcas ↔ fsr1.wgsl::fsr_rcas ↔ frame_reference::fsr1_reference)。
- 方向性テスト: `fsr1_rcas_sharpens_valley_and_peak` — sharpness=0 (EASU のみ)
  を対照基底とし、谷底は 0 より深く・峰は高くなることを直接固定
  (旧ぼかし符号では決定的に失敗する設計。EASU 位置寄せ値への依存を排除)。
- `fsr1_flat_region_is_identity` の境界期待値を厳密値へ格上げ: OOB=0 近傍で
  辺 ×1.05=126 / 角 ×1.10=132 の「明転ハロー」 (旧: 114/108 への暗転期待は
  ぼかし符号を pin していたため修正に追随)。

### S-2 (🔴 GPU 乖離) frame_reference の深度が透視補正補間だった

GPU 固定機能ラスタライザは `z_ndc = clip.z/clip.w` を**スクリーン空間線形**で
補間する (z_ndc = A + B/w、1/w が画面アフィン ⟹ z_ndc も画面アフィン、
標準透視では clip.z が clip.w のアフィン関数となることから導かれる定石結果を
手計算で再証明)。`raster_tri` は透視補正式 `Σλ(z/w) / Σλ(1/w)` を採っており、
三角形内で w が変化する限り GPU 深度と一致しない (誤差 ∝ λ 加重の 1/w の分散)。
「GPU/CPU 相互検証用参照実装」という存在意義に反するため
`z = w0*za + w1*zb + w2*zc` の画面線形に訂正。これに伴い `sw` (1/w 配列) と
`wa` 引数・`inv_w` ガードは完全デッド化したため除去 (シグネチャは private)。
Hi-Z 3 テストは深度値の微細移動に対して頑健 (意味論固定) で変更後も緑。

### S-3 (🟠 実質ハング) FramePacer::next_present_time の逐次加算ループ

`while t < now { t += interval }` はギャップが interval 比 ~6e10 (タイマー
リセット直後やデバッガ停止後) で実質ハング。さらに `refresh_hz <= 0` / NaN
(= interval ≤ 0) では**無限ループ**。除算による直接推定 + ±1 ステップの
端数補正で O(1) 化し、「now 以上の最早境界」の意味論は保存。契約 fail-loud
(interval が有限正に定まらない refresh_hz は new で拒否) を明文化。
合わせて `record_frame` の NaN/±inf 観測値による EMA 永久汚染を遮断
(非有限は欠測として捨てる)。現呼出側 (full_graph_wiring, 60Hz 固定値) は
影響なし。+6 テスト (巨大ギャップ O(1)/最早性掃引 2000 件/非有限遮断 bit 不変/
NaN-now 旧挙動互換/契約拒否 2 件)。

### S-4 (🟡 3連鎖ドリフト) EASU 勾配チャンネルの不統一

WGSL (R のみ) ↔ ミラー (R のみ) に対し `fsr1.rs::easu_reconstruct` だけが
luma 加重で勾配を計算し、モジュール doc の「WGSL は fsr1.rs と同一数学」声明が
偽になっていた。出荷系 (WGSL↔ミラー) を正として fsr1.rs を R チャンネルに統一
(GPU 可視出力は不変。消費者は full_graph_wiring の tick 連鎖のみで、
prev_frame_color 内部状態は report に流出しないことを grep で確認済)。
WGSL 内の未使用 `fn luma` と fsr1.rs の参照ゼロ `Vec3`/`luma` (workspace 全走査で
参照ゼロ証明) を同一根のデッドコードとして除去。方向性テスト 2 件追加
(R 平坦/G-B 変動で寄せ不発の厳密 bit、R 変動での寄せ発動の厳密 fx2)。

### S-5 (🟡 契約未強制) GpuFsr1Pass の次元チェック不在

0 次元 (wgpu 検証 panic / WGSL 除算不定)、low > full (アップスケーラ想定外の
縮小)、full_w > 2^30 (`full_w*4` u32 溢れ) を純粋関数 `validate_dims` に集約し
`with_sharpness` 入口で強制 (DDGI wave 16 の validate() パターン踏襲)。
受理境界 (等倍 1:1, 1x480→640x480) と全違反分野をテスト固定。

### S-6 (検証基盤) naga による WGSL↔Rust ワイヤ形式の自動突合

`struct Params` の naga 計算レイアウト (span=24B、メンバ offset 表) が
`queue.write_buffer` の実 payload (`[f32; 6]` = 24B) と一致することを
`params_uniform_layout_matches_rust_wire` で固定。uniform レイアウトの
ドリフトを GPU 無し CI で検出可能に (gpu_culling の L-5 レイアウト照合を
自動化したもの)。

### 検証結果 (全て実測)
- lib テスト **594/594** (582 → +12: pacing 6, fsr1.rs 2, frame_reference 1,
  frame_fsr1 3)。
- wide_static_bench structural digest **`004c1cf5fb17bfe8` (rows=357) 不変**。
- pseudo_mc_bench 決定的値 8 点 (S1 24543/314193, C 303895/3646740B,
  A 861656/27572992, ACMR 1.669→0.929, hit 90.9%, edit 24511704B,
  水 116719, palette 248110B) spot 一致。
- rustfmt: 編集 5 ファイルで新規 diff ゼロ (fsr1.rs の 2 件は HEAD 由来の
  既存偏差として保存、増分 0)。clippy: 編集箇所に新規警告 0。
- 残 frame_* 未通読: frame_reuse (426) / frame_vct (615) / frame_hiz (745) /
  frame_pipeline (700)。以降の wave で消化予定。

---

## T. frame_reuse.rs 全通読監査 (wave 18, 2026-07-22)

`frame_reuse.rs` (426 行) を全行通読 + 唯一の実消費者 `render_pipeline.rs` の
呼出構造を合わせて監査。統計系の実害 3 件と帳簿虚偽 1 件を修正。
opt-gfx 594 → **599** (+5)。

### T-1 (🔴 二重計上) render_pipeline の record_miss 併呼

実消費者は `if try_reuse(...).is_none() { record_miss(); }` の構造で、
`try_reuse` 内部のミス計上 (not-found / svo-mismatch) と**常に二重計上**に
なっていた。さらに唯一 1 回しか数えられない経路が「無計上の内部ミス」
(下記 T-2) という入れ子の不整合で、ユーザー向け `frame_reuse_misses`
(HUD 統計) は真値の ~2 倍と静かなズレの混在だった。
根治: 計上を `try_reuse` 内部契約 (**hits + misses == 試行回数**) に一本化し、
呼出側の `record_miss()` を除去。`record_miss` API はバイパス経路専用として
二重計上の罠を doc 明記 (API 面は保持)。

### T-2 (🟠 サイレントミス) full_mesh.absent の無計上早期 return

`let full = entry.full_mesh.as_ref()?;` は Mesh tier 要求に full_mesh が無い
場合 (公開 API 経路、及び遠方で Svo 格納したチャンクへの接近 = LOD tier
移行) に **misses/stale を一切計上せず** None で抜け、hit_rate を偽装した
(wave 2 mesh_cache 修正と同種の非対称)。svo-mismatch 経路と同じく
stale_invalidations + misses (+cumulative) に計上するよう統一。
回帰固定: `mesh_tier_miss_without_full_mesh_is_counted` と不変式テスト
`stats_contract_attempts_equal_hits_plus_misses` (800 試行の決定的掃引で
hits+misses == attempts が常に成立 — T-1/T-2 どちらの型の再発も破る)。

### T-3 (🟠 非決定性) enforce_capacity のタイブレーク不在

容量超過時の追い出し `min_by_key(last_tick)` は、同フレーム一括格納で
last_tick が頻繁にタイとなり、その場合の最小を **HashMap 反復順**
(RandomState 由来 = プロセス毎異なる) に委ねていた = LRU 追い出し結果が
再現しない (決定性文化違反)。`(last_tick, key)` の辞書順最小で完全決定化。
`eviction_tiebreak_is_deterministic`: 528 同一 tick エントリで厳密生存集合
(辞書順小 16 件追い出し) を固定し、2 独立キャッシュ (独立 hasher) の生存
集合完全一致も検証。

### T-4 (🟡 帳簿虚偽) memory_bytes の幻影計上と欠損

`entry_bytes` は wave 2 で除去済みの `sections` 複製 (~8KB/section) を依然
幽霊計上し、実際に保持する `occupied` 列 (u32) を未計上だった (二重に虚偽)。
`store` から `sections` 引数ごと撤去 (コンパイル強制で正直化) し、実保持量
のみの式に整合 (rle runs*4 + occupied*4 + verts*12 + idx*4 + svo)。
参考: `Quantized12ByteVertex` は 12B 要素 Vec なので *12 係数は正しいことを
型確認済 (×12 過剰計上の疑いは否定)。厳密式テスト + 上書き/invalidate 帳簿
テストを追加。

### T-5 (低) stale 境界の厳密固定
`last_tick == stale_before` は生存、`<` は追い出しの off-by-one (ちょうど
STALE_FRAMES=120 無更新で追い出し) を `stale_eviction_boundary_is_exact`
で固定。

### T-6 (デッドコード) rle_fingerprint 除去
wave 2 で `fingerprint` フィールドが除去されて以降、参照ゼロ (workspace
全走査証明) のハッシュ関数を削除。

### 検証結果 (全て実測)
- lib テスト **599/599** (+5)。wide digest `004c1cf5fb17bfe8` 不変。
- rustfmt: frame_reuse は HEAD 比 hunk 純減 (2→1、残る 1 は HEAD 由来の
  build_entry 署名)、render_pipeline は 3→3 で増分 0。
- 統計語彙の変更注意: `frame_reuse_misses` (render_pipeline 経由) は
  真値の約半分に「修正されて」減少する (帳簿の正規化であり、再メッシュ等の
  動作そのものは不変)。

---

## U. frame_vct.rs 全通読監査 (wave 19, 2026-07-22)

`frame_vct.rs` (616 行: GPU VCT dispatch + WGSL 精密ミラー) と両側資産
(`voxel_cone_tracing.wgsl` 全文、`svo.rs::to_gpu_words` 符号化) を照合監査。
opt-gfx 599 → **602** (+3)。

### 陰性確認 (ミラー忠実性 — 最重要項目)
WGSL `svo_sample_lod` / `main` と Rust `sample_lod_words` / `trace_cone_words` を
全行突合: 境界否定形 (NaN→None 一致)・budget 式・ガード off-by-one
(`base+9 >= len`)・solid 集計・下降 select 規則 (NaN→0)・33 回ループ構造
(WGSL for 0..=32 ↔ Rust depth>32 安全弁)・コーンループ演算順
(dist 0.5 開始/diameter max(aperture*2d,1)/weight 蓄積/0.75 刻み) —
**全て一致、乖離なし**。mirror テスト `words_mirror_matches_object_tree_bitexact`
(128 コーン bitwise) と合わせて GPU==CPU 主張は健全と確認。

### U-1 (低・誠実性) new_normalized のゼロ方向 doc 訂正
「ゼロベクトルはそのまま = 命中しない正常入力」は、原点が SVO 体積**内**
にある場合に偽 (退化コーンは包含セルを max_dist まで繰り返しサンプルする
定義で、体積内では飽和ヒットする)。走査規則は不変のまま doc を訂正し、
`zero_direction_cone_samples_containing_cell` (内部飽和/外部ミス/正規化
bit 安定) で意味論を固定。

### U-2 (低・防御) 空コーン列の純 Rust 早期 return
`GpuVct::update` は空入力でも 0 サイズ dispatch/copy/map を発行しており、
厳格ドライバでの検証エッジがあり得た。set_scene 検査後に空列を早期 return
(dispatch 経路と決定的に同値)。`GpuFsr1` 等、他 GPU ラッパでも同パターンを
今後点検する基準とする。

### U-3 (検証基盤) naga offset レベル突合へ格上げ
旧テストは size 一致 (32B) のみでメンバ順入替や pad 位置を検出できなかった。
`vct_struct_offsets_match_wgsl_exact`: Rust repr(C) offset (offset_of! 8+4 件) と
naga 計算 offset (uniform `VctParams` / storage `ConeWgsl` の名前+offset 表)
を逐語固定。vec3<f32> align 16 規則で両空間とも同一レイアウトになることを
GPU 無しで恒久検出可能に (wave 17 S-6 パターンの 2 例目適用)。

### 検証結果 (全て実測)
- lib テスト **602/602** (+3)。rustfmt hunk 増分 0。
- 残 frame_* 未通読: frame_hiz (745) / frame_pipeline (700) の 2 モジュール。

---

## V. frame_hiz.rs 全通読監査 (wave 20, 2026-07-22)

`frame_hiz.rs` (746 行: Hi-Z 3 パス GPU 配線 + QueryCore 還元) と両側資産
(`depth_psychic.wgsl` / `hiz_raster.wgsl` / `hiz_debug_view.wgsl`、`mul_v4`、
CPU ミラー hiz_*_reference) を照合監査。opt-gfx 602 → **604** (+2)。

### 陰性確認 (ミラー忠実性 — 乖離なし)
- downsample: 範囲決め (floor/ceil + min(src_dim))・max 集約・初期値 0.0 まで
  WGSL==ミラー一致。セル空走査の不可能性を証明
  (`(i+1)·sw > i·sw` ⟹ `ceil((i+1)·sw) > floor(i·sw)` when sw>0)。
- raster: 角選択 select 規則 (NaN→lo)・`!(w>1e-5)` ↔ partial_cmp 否定形・
  offscreen 式・セル矩形 floor/ceil clamp + 1 セル下駄・DEPTH_EPS=1e-4 —
  全て一致。`mul_v4` (列優先 M·v) ↔ WGSL `mat4x4 * vec4` も規約一致。
- `aabb_from_mesh` の face 別 d0/d1 表を `corner_pos` 6 分岐に全数照合 — 一致。
- debug_view は可視化のみ (深度→輝度 tint) で判定系に非干渉と確認。

### V-1 (低・fail-silent 防止) 画面寸法 0 の契約不在
`GpuHiz` 構築時の 0 幅/高は downsample の `sw = src/64` を 0 とし、全セルが
空走査で 0.0 (最近深度) に埋まり coverage=0 = **画面全体の誤カリング
(真っ黒) を静かに起こす**設計だった。純粋関数 `validate_screen_dims` に
集約し with_policy 入口で強制 (fail-loud)。受理/拒否境界テスト追加。

### V-2 (検証基盤) naga offset 突合へ格上げ (3 例目)
`HizUniforms` (view_proj@0/hiz_dim@64/_pad@72, span 80) と `BlockBox`
(lo@0/hi@16, span 32) の WGSL 計算 offset が Rust repr(C) と逐語一致する
ことを固定 (旧は size のみで member 順/pad 位置を検出不能だった)。

### 検証結果 (全て実測)
- lib テスト **604/604** (+2)。rustfmt hunk 増分 0。
- 残 frame_* 未通読: frame_pipeline (700 行) のみ (wave 21 で消化)。

## W. frame_pipeline.rs 全通読監査 (wave 21, 2026-07-22)

`frame_pipeline.rs` (700 行: wgpu raster → ACES → readback の実フレーム配線) と
両側資産 (`terrain_vertex_pull.wgsl` 150 行 / `aces_tonemap.wgsl` 46 行、
CPU ミラー `frame_reference.rs` の shade 経路) を全行照合監査。
opt-gfx 604 → **607** (+3)。

### W-1 (重大・GPU/CPU 乖離) fs_pull の Lambert 欠落 — 根治
WGSL `fs_pull` は `shade = 0.55 + lao*0.15` (AO のみ) を出力し、vs_pull が
特殊フォーマットで生成した `normal` varying を**一度も参照していなかった**
(dead varying = 脱落の形跡)。一方 CPU ミラー `shade()` は最初から Lambert
(`dot(n, SUN_DIR)` → `0.35 + 0.65·ndl`) を保有しており、frame_reference
ヘッダの「WGSL と Lambert 同一」という記述は偽だった。帰結として GPU 地形は
全 face 同一輝度 (法線方向シェーディング無し = 視覚的に破綻) で、かつ
frame_pipeline の存在意義である GPU==CPU 相互検証が成立していなかった。

根治: WGSL に `const SUN_DIR: vec3<f32>` を新設し、
`ndl = max(dot(in.normal, SUN_DIR), 0.0)` → `li = 0.35 + 0.65*ndl` →
`band * ao * li` (左結合 = Rust ミラーと同一演算順) を実装。
軸平行法線では dot が評価順・fma 不変の厳密同一値となることを根拠として確認
(×±1.0 は厳密、+0.0 加算は厳密、残項 1 個の和は厳密)。

### W-2 (中・doc 偽＋非単位光ベクトル) SUN_DIR の厳密再導出
Rust 側旧値 `[0.4985076, 0.8308459, 0.2492538]` は大ノルム 1.00047 の
**非単位ベクトル**で、doc の「normalize(0.6, 1.0, 0.3)」と各成分 ~2.4e-4
ずれていた (全 repo grep で同一リテラルは 1 箇所のみ = 手書き丸め事故の典型)。
根治: f32 演算規則 (`0.6*0.6 + 1.0*1.0 + 0.3*0.3` → sqrt → 各成分除算、
全て IEEE 単一回丸め) で厳密再導出した正準値
`[0x3eff1da0, 0x3f5498af, 0x3e7f1da0]` = `[0.49827290, 0.83045477, 0.24913645]`
に**両側同時**置換。再導出後の |v|² が bit 厳密 1.0 であることを確認済み。

### W-3 (教訓) double エミュレーションの罠 — 厳密 pin 導出は exact rational で
浮動小数点厳密値の導出に float64 近似エミュレーションを使うと、f32 の真の
単一回丸めと 1 ulp 乖離する境界ケースが存在する (実際 `0.6f32/n` の商は
f32 丸め境界の真上に位置し 0x3eff1d9f/0x3eff1da0 で分裂した)。
本 wave では Python `Fraction` exact rational 演算で各 f32 演算を手続き的に
正しく丸めて正準値を導出し、Rust 実機が同式を再導出して bit 一致を assert
する形で機械保証した。今後の厳密 pin 導出は exact rational 方式を必須とする
(K-6 教訓の補強事例として記録)。

### 陰性確認 (変更不要と判断)
- `build_view_proj`: wgpu z∈[0,1] RH persp / RH view の全成分を手計算照合 —
  一致。`mul_v4`/`mul44` は列優先 WGSL 規約と一致。
- `aces_tonemap.wgsl` ↔ mirror `aces_srgb`: Narkowicz 係数・
  `pow(max(x,0), 1/2.2)`・全画面三角形・exposure uniform 4B ↔ buf size 4 —
  一致。
- draw_mask クリア順序 (draw_calls==0 判定) と全カリング時 clear-only パス — 正常。
- per-chunk `create_buffer_init` チャーン・map+`Maintain::Wait` ポーリング —
  証明用途設計として文書化済み・クレート横断一貫パターンのため変更せず。
- 0 次元フレームは wgpu 深部で panic する既知契約のまま (frame_fsr1/frame_hiz
  式の validate 追加は優先度低・将来検討事項として記録)。

### 追加テスト (+3, fail-loud)
- `sun_dir_is_f32_normalized_direction` (f32 再導出との bit 一致 + 単位長)
- `wgsl_fs_pull_implements_mirror_lambert` (WGSL 側 SUN_DIR リテラル表記一致 +
  fs_pull 本体に dot/max/0.35/0.65/in.normal が存在すること — 退行した時点で fail)
- `shade_exact_bits_matching_wgsl_eval_order` (6 face × 3ch の厳密ビットピン +
  明度順序 top > +X > +Z > 陰面、陰 3 面は bit 同一、band=0 は厳密 +0.0)

### 検証結果 (全て実測)
- lib テスト **607/607** (+3)。rustfmt hunk 増分 0。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- pseudo_mc_bench 決定値スポット全不変 (S1 24543/314193、C 303895 vert/3646740 B、
  861656/27572992、ACMR 1.669→0.929、hit率 90.9%、24511704 B、水 116719、
  palette 248110)。
- **frame_* モジュール全 7 件 (pacing/fsr1/reference/reuse/vct/hiz/pipeline) の
  通読監査が完遂。**

## X. occlusion_query.rs 監査 (wave 22, 2026-07-22)

`occlusion_query.rs` (~1030 行: SW 参照ラスタ + wgpu R32Uint ID パス + QueryCore
ポリシーの 2 バックエンド遮蔽クエリ) を全行照合監査。opt-gfx 607 → **611** (+4)。

### X-1 (中高・描画抜けバグ) near-plane ストラドル面の全スキップ — 根治
SW パス旧実装は `project()` が w ≤ 1e-6 を**コーナー単位**の behind として扱い、
behind コーナーを 1 つでも含む面 (quad) を**全面スキップ**していた。コメントは
「cheap conservative cull」だったが方向が逆: **クエリ箱**の場合 covered=0 →
QueryCore の occlude_after (2 フレーム) で**可視物体が誤カリング**される。
プレイヤーが長い構造物の中に立つ状況 (カメラを飲み込み near plane/背面を
跨ぐプロキシ箱 = Minecraft では日常) で壁がポップアウトする。GPU パスは HW
クリッパが正しく処理するため SW/GPU parity も破れていた。

根治: クリップ空間 (x,y,z,w) で `z_clip >= 0` (wgpu near plane 半空間) への
Sutherland–Hodgman ポリゴンクリップ (交点は 4 成分同時斉次補間) を実装し、
生存頂点のみ NDC 除算 → 従来のスクリーン線形深度ラスタへ。規範的視射影では
z_clip>=0 ⟺ 視深度 >= near であり生存頂点の w>0 が保証されること (カメラ背後は
必ず z_clip<0 で除去) を確認した上で w<=0 は防御ガードとして記録。
x/y 越えは pixel clamp + バリ centric 判定、far はフラグメント毎 z∈[0,1] テスト
(スクリーン線形 z の半空間積として HW クリッパと同値) — 全て両側一致を証明。

### X-2 (低・doc 偽予備軍) 深度タイブレークの ε バイアスを契約として正直化
SW の `z < stored - 1e-5` は GPU `CompareFunction::Less` より僅かに厳しい
(同深度・ニアリータイは先提出が勝つ)。従来無文書だったが、ヒステリシスと
組み合わせフリッカー防止方向にのみ効く仕様としてヘッダに契約明記し、
機械ピン (`identical_depth_tie_favors_first_submission`) で固定した。

### 陰性確認 (既に正当)
深度補間規則 (`z = Σ wᵢ·z_ndcᵢ`、スクリーン線形) は wave 17 S-2 で正した
GPU 固定機能規則と**最初から一致**しており、frame_reference と同型の
透視補正 depth バグは存在しなかった (第 2 インスタンス探索の結論: 陰性)。
`project()`/`perspective_wgpu`/`look_at`/`mat4_mul` の行列数学・OCC_WGSL
配線 (params 64B ↔ mat4x4)・readback row padding (256B)・QueryCore
ヒステリシスも全行照合で乖離なし。

### 追加テスト (+4, fail-loud)
- `software_near_plane_straddling_wall_stays_visible` (カメラ飲み込み回廊箱
  z∈[-95,10.05] — 側壁視深度 7.8〜30 が画面内に来る幾何で前面は far 外に
  排除し「側面が落ちたら確実に 0 になる」判定力を幾何学的に確保)
- `software_fully_behind_camera_covers_nothing` (homogeneous clip が
  カメラ背後除去と同値であること)
- `identical_depth_tie_favors_first_submission` (X-2 契約の機械ピン)
- `clip_polygon_near_cardinality` (全内 3 / 全外 0 / 2-in 4 角形 / 1-in くさび +
  交点 z>=0)

### 検証結果 (全て実測)
- lib テスト **611/611** (+4)。rustfmt hunk 増分 0 (HEAD 由来 3 hunk は規律上温存)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- SoftwareOccluder/project はクレート外利用なし (全 repo grep 確認) —
  digest 非依存を静的に確認済み。

## Y. taa.rs / drs.rs 監査 (wave 23, 2026-07-22)

frame 系外の残モジュール監査へ移行 (frame_* 7 件完遂後のゼロ/単一テスト
モジュール棚卸しから Tier 6 TAA と Tier 4 DRS を選定)。opt-gfx 611 → **616** (+5)。

### Y-1 (doc 偽) reproject_uv の符号
doc「uv_hist = uv + velocity」に対し実装は `uv - velocity` — WGSL
(`hist_uv = uv - vel`) と照合するとコードが正で **doc の記号が逆**
だった。doc を訂正し (実装無変更)、WGSL ミラートークンをテストで機械固定。

### Y-2 (契約差分の正直化) blend clamp の非対称
Rust 側 resolve は blend を [0,1] clamp するが WGSL の `mix()` は clamp 無し。
消費者ゼロ (LightweightTaa/TAA_WGSL は現行カタログ資産、実使用は taa_ycocg
経路) のため配線影響はないが、将来のホストが params にそのまま流すと
両側で逸脱する。WGSL ヘッダにホスト正規化契約を明記。

### Y-3 (中・NaN 永久汚染) drs::push_frame_ms — 根治
`frame_ms.clamp(1.0, 100.0)` は NaN を素通りし (NaN.clamp = NaN)、EMA は
一度汚染されると比較が全て偽となり **scale が静かに永久凍結** する
fail-silent だった。非有限 (NaN/±inf) は観測欠測として捨てる設計に根治
— frame_pacing::record_frame (wave 17, S-3) と同一契約。同一クラスの
防御として resolve の非有限 blend も a=0 (current 素通し) に正規化
(画面への NaN 伝搬遮断)。

### Y-4 (厳密値) EMA/ヒステリシス・resolve のビットピン
全て f32 単一回丸め規則から exact rational (Python Fraction) で厳密導出
(float64 近似エミュレーション禁止 = W-3 教訓の運用事例):
- DRS: 60fps 目標に 25ms 連続観測で初回調整は **9 push 目** (ema>18.0 到達
  push 2 + ヒステリシス 8)、30 push で 3 回調整、scale bits `0x3f599999`
  (0.85)、fps_ema bits `0x42224b15`。
- TAA: resolve(cur=[.5,.5,.5], clamp 後 hist=[.6,.4,.4], blend=0.1) =
  bits [`0x3f028f5c`, `0x3efae147`, `0x3efae147`]; blend=1.0 はクランプ後
  ヒストリと bit 一致。

### 追加テスト (+5, fail-loud)
- resolve_exact_bits_matching_wgsl_mix / non_finite_blend_normalizes_to_passthrough /
  taa_wgsl_parses_and_mirror_tokens_present (naga + ミラー規則トークン)
- push_frame_ms_rejects_non_finite (捨棄・汚染無し・以後回復) /
  hysteresis_timeline_is_deterministic (上記厳密タイムライン)

### 検証結果 (全て実測)
- lib テスト **616/616** (+5)。rustfmt hunk 増分 0 (taa.rs の HEAD 由来
  署名 1 hunk・CRLF 行末は規律上温存、新規分は正準形)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- 直前 push (wave 21+22) の bench-ci run 29908670601 は **success**。

## Z. taa_ycocg.rs 監査 (wave 24, 2026-07-22)

`taa_ycocg.rs` (125 行: YCoCg 変換対 + Jimenez 分散クリップ、
full_graph_wiring の TAA 経路が実消費者) と WGSL ミラー
(`shaders/taa_ycocg.wgsl` — エントリポイント無しのユーティリティ関数集)
を全行照合。opt-gfx 616 → **621** (+5)。

### 陰性確認
RGB↔YCoCg 変換対・clamp_to_variance の演算順は WGSL と完全一致。
変換対の代数逆性を厳密証明 (0.25x+0.5y+0.25z 等の係数和が恒等的に
x,y,z へ戻る)。mean_sigma の空入力既定値 (0.5,0,0)/(0.1×3) は既存の
strict テストがピン済み (full_graph_wiring.rs:2122)。

### Z-1 (中・panic DX) gamma 契約を明示 fail-loud 化
clamp_to_variance は gamma<0 で lo>hi、NaN で境界 NaN となり、どちらも
`f32::clamp` 内部 assert で「std の意味不明メッセージ」のまま panic
していた (レンダーホットパスで DX 最悪)。モジュール入口で
`gamma.is_finite() && >= 0` を契約 assert (fail-loud、should_panic テスト
2 件)。現行消費者 full_graph_wiring は gamma=1.25 固定で契約内 —
挙動変化なし (σ 非負は mean_sigma の sqrt により構造保証)。

### Z-2 (doc 正直化) YCoCg 往復は bit 厳密でない
係数が 2 の冪のため順変換は完全決定的だが、往復は丸めで R/B が 2-3 ulp
ずれる (G は厳密一致)。「可逆」という直感で bit 厳密を期待する罠を doc に
明記し、順変換 bits [0x3f133334, 0xbeb33333, 0x3cccccd0] /
往復 bits [0x3e4cccd0, 0x3f19999a, 0x3f666668] を機械ピン (全て exact
rational 導出 = W-3 方式)。純色 (R/G/B) は厳密を別途ピン。

### 追加テスト (+5, fail-loud)
- ycocg_transform_exact_bits (順/往復/純色厳密ピン)
- clamp_to_variance_exact_bounds_bits (gamma=1: lo/hi は f32 近似値、
  gamma=1.25: 0.375/0.625 厳密、外れ値吸着と内側不変)
- clamp_to_variance_rejects_negative_gamma / rejects_nan_gamma (should_panic)
- wgsl_mirror_is_utility_functions_with_matching_coefficients (naga 実パース
  + 3 fn 実在 + 係数ミラートークン + エントリ無し契約)

### 検証結果 (全て実測)
- lib テスト **621/621** (+5)。rustfmt hunk 増分 0 (0 → 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- naga 0.20 の知見記録: `module.functions.iter()` は (Handle, &Function)
  タプル、非エントリ関数名は Option<String> — 今後の WGSL 関数実在テストは
  `f.1.name.as_deref() == Some(name)` 形を使う。

## AA. texture_atlas.rs 監査 (wave 25, 2026-07-22)

`texture_atlas.rs` (Tier 1: シェルフ packer / Texture2DArray builder /
box mip chain) 全行照合。opt-gfx 621 → **628** (+7)。

### AA-1 (中・fail-silent 経路閉塞) 次元契約の fail-loud 化 — 3 箇所
- `TextureAtlasPacker::new`: dims > 2^31 で next_power_of_two が u32 溢れ、
  release では 0 wrap → .max(64) により**静かに 64x64 ミニアトラス**になる
  経路を契約 assert で閉塞 (should_panic ピン)。
- `TextureArrayBuilder::new`: push 時 `w*h*4` の u32 乗算が 65536x65536
  (=2^34) で wrap → **expect=0 で空ベクタを合法レイヤ受理**する経路。
  new で 0 次元・w*h*4>u32::MAX を契約拒否、push の expect も u64 演算化。
- `generate_mip_chain`: 0 次元 and len != w*h*4 を契約拒否 (旧: 短小で
  途中 panic / 超過で静かに無視)。使用中の内部多重複製 (base + 各レベル
  1 回ずつの余分 clone) も borrow 化でゼロに (出力は完全同一、
  バイトピンで実証)。

### AA-2 (厳密値) シェルフ配置・UV・mip 内容の手計算/独立参照ピン
- 配置遷移を手追跡で厳密導出: 遅延失敗が shelf_y を進める一点 (F 失敗で
  y=40 前進後に G が (0,40) に入る) を含む 5 成功 + 2 失敗列を全矩形座標・
  UV ビット (2^7 除算 = 丸めゼロ)・failed 集計でピン。非重複不変条件も
  全対で検査。
- mip 4x4→2x2→1x1 全バイトを独立整数参照 (Python 整数のみ実装) で導出し
  ピン。特に 1xN → 1x1 の**端クランプ発火ケース** (x*2+1 > w-1 = 0 で
  列複製、4 サンプル平均) は本実装が生む唯一の clamp 経路であることを
  解析確定し `[30,40,50,60]` で固定。

### 陰性確認
シェルフ構造の非重複性 (次シェルフ y は前 shelf_h 加算 · 単一前進/pack)、
vram_bytes の base/3 mip 概算 (Σ4^-i = 4/3 の整数近似として誠実)、
remap_uv の線形 remap、TextureArrayBuilder::vram_bytes 概算は全て
読み合わせで乖離なし。pack_and_mip 既存テストは不変更で緑。

### 追加テスト (+7, fail-loud)
- shelf_layout_matches_hand_derived_geometry (矩形 + UV bits + failed + 非重複)
- mip_chain_exact_bytes (4x4 両レベル + 1x3 クランプ列)
- should_panic 5 件 (packer 2^31 超過 / builder overflow・0 次元 / mip 不整合・0 次元)

### 検証結果 (全て実測)
- lib テスト **628/628** (+7)。rustfmt hunk 増分 0 (CRLF 歴史行末は温存)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AB. triple_buffer.rs / spatial_hash.rs 監査 (wave 26, 2026-07-22)

低レベルプリミティブ 2 件 (どちらも render_pipeline の実消費者あり)
全行照合。opt-gfx 628 → **635** (+7)。

### AB-1 (高・並行バグ) TripleBuffer の単一バッファ退化 — 根治
書き込み先選択 `free_idx(w, r)` は 3 組の対称ペアのみ明示処理で残り全て
`_ => 0` に倒していた。初期 (w=0,r=1) → 2 回目 write で (w=2,r=2)→0 選択、
end で read=0 となり **(0,0) の定常状態に到達**。3 回目以降 free_idx(0,0)
は read スロット自身の **0 を返し** (read≠write 不変条件が (1,1)(2,2)
では偶然成立するが (0,0) で破れる)、**公開中スロットを CPU が上書きする
単一バッファ退化**に永久定着。Mutex があるため torn バイト列にはなら
ないが、produce/consume の非ブロック前提が崩れ、GPU は「完了前フレーム」
を読み得た。実消費者 render_pipeline::upload_ring (:798 write / :1176 read)
で確実に 3 フレーム目から発火する経路。

根治: 書き込み先を「公開スロットの 1 つ先」`(read+1)%3` のローテーションに
置換 — read 不一致が定義より自明な最小不変条件。不変条件 3 条を構造 doc に
機械固定し、スロット列 [2,0,1,2,0,1,2] と begin 各時点の `idx != published`
をピン (旧実装は [2,0,0,0,...] で確実に赤になる回帰検出器)。

### AB-2 (中・fail-silent 経路閉塞) SpatialHash 非有限/逆転の契約化
- insert/move_entity: NaN 座標は float→int cast 規則 (NaN→0、±inf→端飽和)
  で**実世界位置と無関係な「謎セル」**に分類されていた。有限契約を assert。
- query_aabb: 逆転 AABB (min>max) は range 空で**静かに 0 件**、NaN 境界も
  謎セル探索化 — 有限かつ min<=max を契約 assert (半径クエリも半径 NaN を
  f32::max が 0 に直して静寂だったのを契約化)。
- 複雑度 Θ(∏包含セル) と半径 AABB 近似 (距離精査は呼び出し側) を doc 明記。

### 陰性確認
remove の retain + 空セル消去による帳簿整合、同セル move no-op、key の
負側 floor 規則、query 出力の sort+dedup による HashMap 反復順非依存の
完全決定性 — 全て読み合わせ一致 + churn 厳密値ピンで固定。

### 追加テスト (+7, fail-loud)
- write_slots_rotate_without_touching_published (列ピン + 各 begin で idx≠published)
- generations_increase_strictly_per_completed_write (世代単調)
- concurrent_producer_consumer_never_reads_torn_frame (scoped thread 撕裂 smoke)
- churn_sequence_matches_hand_derived_results (5 段の手計算厳密列)
- should_panic 3 件 (NaN 座標 / 逆転 AABB / NaN 半径)

### 検証結果 (全て実測)
- lib テスト **635/635** (+7)。rustfmt hunk 増分 0
  (spatial_hash の HEAD 由来 1 hunk は編集範囲と一致したため正準形へ)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AC. simd_kernels.rs 監査 (wave 27, 2026-07-22)

`simd_kernels.rs` (Tier 3: popcount/merge_runs/Gribb frustum + rayon 並列版)
全行照合。実消費者は low_spec_stack::frustum_culled
(world_column_store::TerrainFrameConstants の p·M 行列を供給)。
opt-gfx 635 → **640** (+5)。

### AC-1 (中・規約逸脱) near 平面の GL 式体積 — 根治
Gribb/Hartmann 抽出の near を `add4(r3, r2)` (w+z>=0 = **z >= -w の GL 式
体積**) としていた。wgpu は z_ndc ∈ [0,1] で視体積は clip.z >= 0 ⟺
`r2` 単体。旧式は誤カリングしない保守側 (near 背面の箱を可視扱いで残す) だが、
doc の「matches terrain VS」と矛盾し frame_worldgen::frustum_planes
(2026-07-21 監査で既知点検証済み) の r2 規則とも非対称だった。
`r2` に根治 (far = sub4(r3, r2) は旧来正しい)。

### AC-2 (doc 偽×2 訂正)
- FrustumPlanes の法線は**内向き** (r3±r0 抽出は内側半空間 dist>=0 を与える)
  — 旧 doc「outward normals」は偽。
- 行列規約の相互供給禁止を明文化: 本関数は p·M (row-vector / world_column_store
  系) 専用。M·p 系 (frame_pipeline::build_view_proj) を流すと転置ずれで
  誤カリングするので frame_worldgen::frustum_planes を使う (二系統は
  一方が他方の転置として成立することを実データで確認済み)。

### 陰性確認
正頂点 (positive vertex) 選択による outside 判定式 `p·v+d < 0`
(符号付き境界 match)、正規化 len の (a,b,c) のみ版 (d 除外 = Gribb 正常)、
batch ビット集合 i/64, i%64 レイアウト、並列版の per-index 独立性
(rayon でも逐次と一致) — 読み合わせ+厳密テストで一致。退化平面
(len 1e-8 床) と NaN 行列の「全可視」同化はカリングの安全側に倒れる
設計として確認。

### 追加テスト (+5, fail-loud)
- identity_view_proj_planes_exact (整数平面 6 面、near=(0,0,1,0) — 旧 GL 式
  (0,0,1,1) では確実に赤)
- perspective_planes_match_hand_derived_gribb (90°/1x1/1..10、exact rational
  導出 bits + z=-near で dist=0 の意味論実証)
- merge_runs_edge_cases_exact (0/full/端 bit/交互/batch 行 index)
- popcount_par_matches_seq_at_threshold (n=0,1,63,64,65,130 で逐次=並列=期待)
- batch_bitset_layout_and_straddle_semantics (跨ぎ=可視・完全外=不可視・
  near 背面カリング + 130 要素の bit 一致・par==seq)

### 検証結果 (全て実測)
- lib テスト **640/640** (+5)。rustfmt hunk 増分 0 (:67 aabb_outside_plane
  の HEAD 由来 1 hunk は規律上温存)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AD. mesh_compactor.rs 監査 (wave 28, 2026-07-22)

`mesh_compactor.rs` (Frostbite 方式 compute ドロー圧縮: CPU 参照実装 +
wgpu 結線用 WGSL カーネル) 全行照合。実消費者は full_graph_wiring:537-567
(実 DrawCandidate → compact_draws → IndirectDrawCmd 生成)。
opt-gfx 640 → **644** (+4)。

### AD-1 (中・規約逸脱) FrustumPlanes::from_view_proj の near GL 式体積 — 根治
本モジュールは column-major 16・M·p 規約の**第 3 の Gribb extractor** で、
wave 27 AC-1 (simd_kernels) と同型の逸脱が残存していた: near を
`add(r3, r2)` (z >= -w の GL 式) としていた。wgpu z_ndc ∈ [0,1] の視体積は
clip.z >= 0 ⟺ `r2` 単体。旧式は誤カリングしない保守側だが、near 背面の
箱を可視扱いで残すため full_graph_wiring 経由で無駄な draw が残存していた。
`r2` に根治し、これで 3 extractor 全てが wgpu z∈[0,1] 規則に統一
(frame_worldgen::frustum_planes の r2 元来正 / simd_kernels AC-1 /
mesh_compactor AD-1)。far = sub(r3, r2) は旧来正しい。

### AD-2 (正直化 3 点 — COMPACT_WGSL 契約注記の確定)
1. **wire format**: WGSL Candidate は Rust DrawCandidate とは別レイアウト。
   当初 doc に「vec3 整列 52B」と記したが、**テスト初回実行で赤**となり
   naga 実測 48B と不一致を検出 — WGSL レイアウト規則で厳密再導出:
   vec3 align=16 により center 0..12 / half_ext roundUp(16,12)=16..28、
   以降の u32 群は align 4 密詰み (28,32,36,40,44)、
   span = roundUp(16,48) = **48B**。直感値 (52B) は誤りであり、
   「実測で潰す」文化が機能した事案として記録。
   (DrawCandidate は repr(Rust) 非 Pod で bytemuck 不可 → 将来の GPU 配線は
   明示ワイヤ変換必須、の契約も明文化。)
2. **出力順非決定**: GPU は atomicAdd 完了順に cmds を詰めるため CPU 参照
   (first_vertex 安定ソート) との逐一致合は成立しない。厳密性が必要な
   opaque 専用経路である旨を契約化 (半透明ソートは別経路)。
3. **sign(pl.xyz)*e vs CPU 正頂点選択**: 成分ゼロ平面で見掛け差
   (sign(0)=0 vs CPU は +e) があるが、ゼロ成分項は内積に寄与しないため
   **数学的に常に一致** — 解析証明を契約注記に明記。

### 陰性確認
正頂点テストの符号境界 (`d < -margin` 系)、max_d2 距離フィルタの二乗比較
(sqrt 不要の正常)、sort_by_key による first_vertex 安定ソート
(同キー入力順保持 — Rust 安定ソート保証)、IndirectDrawCmd の Pod/repr(C)
(wgpu DrawIndirect 同形) — 全て読み合わせ一致。

### 追加テスト (+4, fail-loud)
- identity_extractor_planes_exact_bits (単位 VP 6 面整数ピン、
  near=[0,0,1,0] — 旧 GL 式 [0,0,1,1] では確実に赤)
- near_back_culled_with_wgpu_plane_rule (z∈[-1.6,-0.6] 箱カリング —
  旧 GL 式では残存=赤、跨ぎ [-0.6,0.4] は残存の両側検証)
- compact_draws_exact_survivors_and_stable_order (7 候補シナリオ:
  生存 3 件の厳密 IndirectDrawCmd 列 + first_vertex 同キーの安定性 +
  除外 4 件 (錐台外/vc=0/遮蔽/距離外) の網羅)
- wgsl_layout_matches_pinned_offsets (naga parse + cs_compact entry 存在 +
  Candidate span 48/オフセット 7 個、Cmd span 16、Params span 128
  (planes@0, camera@96, policy@112) の機械ピン)

### 検証結果 (全て実測)
- lib テスト **644/644** (+4)。rustfmt hunk 増分 0
  (HEAD 由来 4 hunk は規律上温存、新規テストは正準形で投入)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AE. simd_frustum.rs 監査 (wave 29, 2026-07-22)

`simd_frustum.rs` (SoA AABB 一括カリング: ポータブル 4-wide + AVX2/FMA 8-lane
自動ディスパッチ) 全行照合。実消費者は full_graph_wiring:480-535
(extract_frustum_planes の 6 平面を SimdFrustum へ供給、
**cull_soa_portable 直叩き** — digest 経路は決定的)。
opt-gfx 644 → **650** (+6)。

### AE-1 (高・安全性) SoaAabbs 等長契約の未強制 — fail-loud 化で UB 根絶
`SoaAabbs` は pub フィールドで外部から 6 Vec 不等長インスタンスを構築可能。
`cull_soa_avx2` は `_mm256_loadu_ps(ptr.add(base))` で各配列から長検査なしに
8 要素ずつ読むため、不等長 SoA を渡すと **ヒープ範囲外読み (UB)** になり得た
(portable 側は index による安全 panic に留まるが契約は不明だった)。
両カリング入口に `SoaAabbs::assert_uniform_len` (6 配列の実長を余さず列挙した
契約メッセージ) を追加 — UB を契約付き panic に格下げ。
`from_aabbs` 経由の構成は常に等長のため実動作への影響なし。

### AE-2 (正直化) ディスパッチ判定の機械依存性を契約化
AVX2 パスは FMA + `c*pz + (b*py + fma(a,px,d))` 評価順、ポータブルは丸めあり
左結合 `((a*x+b*y)+c*z)+d` — 距離が 0 ごく近傍の**境界上の箱で判定が割れ得る**
(機械依存)。`cull`/`cull_fast` の doc に明文化し、厳密決定性経路は
`cull_soa_portable` 直接利用を契約とした (full_graph_wiring:535 が実例)。
両パスとも「真の距離が負なら必ず除去・正なら誤除去しない」保守性は
浮動小数丸めの範囲で同等。また NaN 距離の扱いは両者一致を確認
(scalar `NaN < 0.0` = false ≡ `_CMP_LT_OQ` の ordered=false → ともに可視側)。

### AE-3 (doc 過剰主張の訂正)
SoaAabbs の doc 「cache-line aligned AABB testing」は過剰: `repr(C, align(64))`
は struct 自体の配置のみを整列し、6 Vec のヒープ領域は 64B アライン非保証。
読み出しは loadu 系のため動作上は不問だが、事実関係を訂正。

### 陰性確認
p-vertex 選択規則 (a>=0 → max、3 パス共通)、剰余ループ (4-wide/8-wide 両方)、
空入力早期 return、`simd_frustum.wgsl` がコメントのみのスタブで
gpu_runtime の naga 検証を空モジュールとして受理すること — 全て一致。
保守側カリング (dist==0 触接=可視) の境界規則も 3 パス同一。

### 追加テスト (+6, fail-loud)
- boundary_touching_is_visible_exact_sequence_with_remainder
  (dist==0 触接=可視ピン + n=5 で chunk+remainder 経路厳密列)
- portable_matches_scalar_bitexact_on_structured_set
  (混合符号 6 平面×13 箱: portable は intersects と式順一致で bit 完全等価)
- soa_length_mismatch_panics_portable (should_panic 契約)
- soa_length_mismatch_panics_avx2_when_detected
  (AVX2+FMA 実機検出時のみ: UB ではなく panic 化を catch_unwind で実証)
- avx2_matches_portable_on_integer_geometry_when_detected
  (全積和が f32 正確の整数幾何 19 箱で avx2==portable==scalar 厳密一致 —
  本 sandbox で検出・実実行済み)
- empty_and_wgsl_stub_contract (空入力 + WGSL スタブ=空モジュールの機械ピン)

### 検証結果 (全て実測)
- lib テスト **650/650** (+6)。rustfmt hunk 増分 0
  (HEAD 由来 6 hunk 温存、新規分は正準形)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AF. azdo.rs 監査 (wave 30, 2026-07-22)

`azdo.rs` (AZDO orchestrator: Persistent VBO pool + IndirectBatcher +
DrawCompactor 統合、1 パス 1 MDI 発行モデル) 全行照合。
実消費者は full_graph_wiring:594 (実 cmds/vis_mask → execute_azdo_pass)。
opt-gfx 650 → **652** (+2)。

### AF-1 (軽・メトリクス境界) overhead_saved の count==0 過小計上 — 根治
`saved_calls = commands.len().saturating_sub(1)` は naive (全コマンド個別
発行) ベースラインに対し、生存 0 件時に AZDO が MDI 自体を発行しない
(= len 件全部を回避) ことを無視して常に len-1 で 1 件過小だった。
`len - (count>0)` に根治し、ベースライン定義 (naive 全個別発行 vs AZDO
min(count,1) 発行) と 1500 ns/draw が推定値であることを doc 明文化。
既存パス (count>0 定常) の値は不変。

### AF-2 (doc 過剰主張の訂正)
旧 doc の「persistent ring への push」「MDI コマンドリストの準備」は
**未配線**: 現パスが行うのは compaction のみで `vbo_pool` / `batcher` は
保持されるだけ (将来 GPU パス用)。実装側を偽装するより doc を事実に
合わせる選択 (CPU 参照実装の段階的配線方針として正当)。

### AF-3 (契約 fail-loud) mask 等長の入口強制
`DrawCompactor::compact_and_filter` は短い mask の欠損要素を
**可視扱い** (`unwrap_or(true)`) する暗黙仕様で、呼び出し側の mask ずれを
検出できなかった。`execute_azdo_pass` 入口で等長 assert (全実長つき)。
実経路 full_graph_wiring:587-594 は `cmds.len()` と同長に構築されることを
読み合わせ確認済み (違反不発)。

### 陰性確認
`AzdoOrchestrator::new` の MiB 単位契約 (過去の 1<<20 誤用 OOM 注記が
既に正しく共存)、AtomicU64 Relaxed (メトリクス用途で十分)、
`PersistentVboPool::new(64)` が 48 MiB staging を eager 確保する点
(wiring::new 構築時 1 回、仕様内)、`DrawCompactor` の max_commands が
capacity ヒントでハードキャップでない点 (Vec 成長; 踏み越え時の GPU 側
制約は execute_indirect 監査 wave に送る) — 全て確認・記録。

### 追加テスト (+2, fail-loud)
- overhead_metric_matches_naive_baseline_exact
  (空パス +0 / 2 生存×2 回で +1500×2 / 全不見で +3000 の厳密累積 6000 ns、
  total_overhead_saved_ms == 0.006 ピン — 旧式では 4500 で確実に赤)
- visibility_mask_length_mismatch_panics (should_panic 契約)

### 検証結果 (全て実測)
- lib テスト **652/652** (+2)。rustfmt hunk 増分 0 (HEAD 由来 1 hunk 温存)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AG. diff_mesh.rs 監査 (wave 31, 2026-07-22)

`diff_mesh.rs` (16³ セクション差分 dirty 追跡 + MeshPatch 集約、Tier 2)
全行照合。crate 内消費は render_pipeline:1118-1120 (chunk ingest 時の
カメラ高セクション mark) のみで、patches/dirty の読み出しは外部ブリッジ
(lib.rs:82 で re-export) 経路。opt-gfx 652 → **656** (+4)。

### AG-1 (契約の明文化) 水平隣接チャンク伝播は呼び出し側の責務
`mark_block_dirty` は**垂直**隣接セクション (local_y∈{0,15} で sy∓1) だけを
伝播する。ブロックが chunk 端 (local x/z ∈ {0,15}) の場合、隣接チャンクの
セクションメッシュもこのブロックに依存し陳腐化し得るが、API が chunk 内
座標を受け取らない以上ここで扱えない → doc で契約明文化 (該当時は隣接
チャンクにも同 block_y で呼ぶ)。今回コード変更は設計変更 (シグネチャ拡張)
を伴うため文書側を直す選択 — 現消費 (ingest 時のカメラ高 3 点 mark) では
発火しないため実害なしと根拠付け。

### AG-2 (契約の明文化) MeshPatch 整合性は非検証
`quad_count` と vertex/index bytes の整合 (stride 倍数、quad×6 等) は
本モジュールに検証根拠 (stride 定義) がなく、勝手な assert は正当な
生成側を拒否し得る → 非検証である旨を doc 明文化 (K-6 教訓:
根拠なき直感 assert を入れない)。

### 陰性確認
`block_to_section_y` の div_euclid 規則 (負側も floor 方向で正しく、
-65→-1 拒否 / 320→24 拒否)、bit 操作の u32 << sy (sy<24<32 で wrap なし)、
dirty_sections の bit 昇順走査による完全決定性、patches の同一セクション
位置保持置換 (順序安定)、take_dirty_mask の排他消費 (2 度読み不可)、
dirty/_bits と patches の非整合 (独立) — 全て読み合わせ一致。
既存テスト (marks_neighbors_on_boundary) の y=-64/y=-49 振る舞いは
厳密列ピンで包含確認。

### 追加テスト (+4, fail-loud)
- section_y_mapping_exact_boundaries (hand-derived 厳密列 9 点 +
  範囲外両端で dirty 非残留)
- dirty_bits_exact_with_boundary_clamps_and_take_exclusive
  (0x18 / 1<<23 / 1 / 1<<4 / (1<<23)|(1<<22) bit ピン — 途中 y=300 の
  自前コメント導出誤りを実測前レビューで自己捕捉・訂正済)
- dirty_sections_ascending_deterministic (昇順列ピン + 未登録=空)
- push_patch_replaces_same_section_and_drains_all (位置保持置換、
  drain 全回収で pending 0)

### 検証結果 (全て実測)
- lib テスト **656/656** (+4)。rustfmt hunk 増分 0 (CRLF 維持、両側 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AH. tick_render_split.rs 監査 (wave 32, 2026-07-22)

`tick_render_split.rs` (固定 20 TPS tick 時計 + 描画補間、Tier 3) 全行照合。
実消費者は render_pipeline:44 (FixedTickClock)。opt-gfx 656 → **660** (+4)。

### AH-1 (中・NaN 永久汚染) consume_ticks の NaN dt 凍結 — 根治
`frame_dt_sec.clamp(0.0, 0.25)` は **NaN を素通しする** (clamp の panic 条件は
min>max / 境界 NaN のみで入力 NaN は比較不成立で自己返還)。NaN が accumulator
に加算されると以後全フレームで `acc >= TICK_DT` が偽となり、
**tick が永久凍結**する (drs wave 23・frame_pacing S-3 と同型の観測欠損
汚染)。NaN を観測欠測として drop (戻り 0、状態不変) に根治。
±∞ は clamp が責任を持つ (0.25→5 tick / 0) ため従来どおり受理 — 生成係が
実クロック dt (常に有限非負) の render_pipeline 経路では不発だったが、
pub API としての堅牢性問題。

### 陰性確認
TICK_DT == 0.05 リテラルは bit 一致 (exact rational 検証) で剰余ゼロの
1 tick 着地、1 フレーム dt 0.25 上限 = 最大 5 tick → 既定 max_catchup=10 の
reset 分岐は単一 consume からは**不到達** (catchup < 10 の損失分岐) だが
with_max_catchup 小指定時に確かに発火、n==max_catchup での残余破棄は
spiral of death 防止として妥当、with_max_catchup(0) の 1 矯正 (0 だと
n==0==max で毎フレーム acc reset の永久 0 tick 化を回避 — 既設ガード正当)、
render_alpha ∈ [0,1) (consume 後 acc < TICK_DT が不変条件)、
lerp の評価順 a+(b-a)*alpha — 全て読み合わせ+厳密テストで一致。

### 追加テスト (+4, fail-loud)
- consume_ticks_exact_sequence (0.05→1 tick 残余 0 / 0.024→alpha 0.48f32
  bit 0x3ef5c28f / 0.026 累積で 1 tick / 9.0→5 tick / -3.0→0 tick)
- nan_dt_is_dropped_without_poisoning (NaN で状態不変 + 直後の正常動作 —
  旧実装では永久凍結で確実に赤)
- max_catchup_discards_remainder_and_zero_is_coerced (上限 2 で残余破棄
  alpha=0 ピン + 0→1 矯正)
- lerp_exact_values (2.0/2.5/端点厳密)

### 検証結果 (全て実測)
- lib テスト **660/660** (+4)。rustfmt hunk 増分 0 (CRLF 維持、両側 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AI. soa_layout.rs 監査 (wave 33, 2026-07-22)

`soa_layout.rs` (EntitySoA + XZY indexing + 64B アライン確保、Tier 3) 全行照合。
crate 内消費者なし (lib.rs:109 で re-export のみ)。opt-gfx 660 → **668** (+8)。

### AI-1 (高・ゼロデイ) alloc_aligned_64 は一度もアライン補正を達成していなかった — Aligned64 で根治
旧実装は `vec![0u8; len+64]` を確保後、ポインタ由来のオフセットを
`raw.drain(0..align_off)` で除去する設計だったが、**`Vec::drain` は要素を前方
シフトするだけで先頭ポインタ (アロケーション先頭) は不変**。事後検査
`raw.as_ptr() % 64 != 0` が初期確保が 64 の倍数だった場合を除き必ず発火し、
fallback の `vec![0u8; len]` (64B 非保証) を返していた。すなわち名前に反し
**ほぼ任意の入力で misaligned を静寂返却** — aligned SIMD load
(`_mm256_load_ps` 等) を前提にした消費者は実機 crash し得た。
`std::alloc` Layout(align=64) 直確保 + 自前 Drop の `Aligned64` newtype
(NonNull 保持、Deref/DerefMut → [u8], Send/Sync を SAFETY 注記つきで実装、
len==0 は 1B 確保で Layout 非零要件を回避、alloc null は handle_alloc_error、
dealloc は同一 layout) に根治。crate 内消費者なしを確認した上で API 置換。

### AI-2 (中・静寂 aliasing) xzy_index の範囲外 z ≥ sx 衝突 — fail-loud 化
旧実装は `(x*sx + z)*sy + y` のみで範囲検査なし。`z >= sx` で異なるセルが
同一 index に衝突する (例: sx=16 で (1,0,0) と (0,0,16) が共に 256)。
XZY の z ストライドを sx で仮定する設計は sz == sx の正方形断面 (chunk
16×16) を暗黙前提としていた → 契約を明文化し `x < sx && z < sx && y < sy`
を fail-loud assert。decode 側は有効 index の一貫復元 (相互逆写像) を確認。

### AI-3 (契約) EntitySoa 7 配列等長の強制
SoaAabbs AE-1 と同型: pub フィールド手組みで不等長を作れる。`integrate` は
範囲外 index panic、`to_aos` は `get` の None で**出力を静寂に短く打ち切る**
不誠実な振る舞いだった。`assert_uniform_len` (7 配列の実長全列挙) を両入口で。
`get` 自体は寛容アクセサとして残置 (None は契約内)。

### 陰性確認
`xzy_decode` の除剰合成、`integrate` の x+=vx*dt 左結合、`with_capacity` の
7 配列統一確保 — 全て一致。EntityAos に PartialEq derive を追加
(additive、テスト可読性のため)。

### 追加テスト (+8, fail-loud)
- xzy_index_decode_exact_table (0/306/4095/column-scan +1 連続/代表 5 点逆写像)
- xzy_index_rejects_{x,y,z}_out_of_range (should_panic ×3、
  z=16=sx 等号境界含む)
- aligned64_actually_aligned_zeroed_and_writable (len ∈ 0,1,63,64,65,4096 で
  ptr%64==0 + ゼロ初期化 + Deref/DerefMut RW 疎通)
- integrate_exact_and_aos_roundtrip (2.0/1.5/11.0/0.0/3.0/100.0 厳密 +
  往復一致)
- integrate/to_aos_rejects_non_uniform_soa (should_panic ×2)

### 検証結果 (全て実測)
- lib テスト **668/668** (+8)。rustfmt hunk 増分 0 (CRLF 維持、両側 0;
  struct_lit_width=18 規則による複数行化は正準形で対応)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AJ. entity_tick_lod.rs 監査 (wave 34, 2026-07-22)

`entity_tick_lod.rs` (距離帯別エンティティ tick スケジューリング、Tier 5)
全行照合。crate 内では render_pipeline:130 がフィールド保持のみ
(選択実行は外部ブリッジ経路)。opt-gfx 668 → **674** (+6)。

### AJ-1 (低〜中・静寂飢餓) 非有限 dist の RenderOnly 化 — fail-loud 根治
`band_for_distance` は全比較 `<` の連鎖で、NaN を与えると全不成立 → else の
**RenderOnly に静寂着地**し、そのエンティティは描画され続けながら AI tick
が永久停止する (spatial_hash wave 26 / NaN 系と同型の「静寂欠損」)。
NaN は「遠い」のではなく観測欠損 → `dist.is_finite()` assert で fail-loud 化
(±∞ も有限入力の overflow として Minecraft ±30M blocks 契約外で併置拒否)。
実経路は有限 feed のため不発、pub API としての堅牢性修正。

### AJ-2 (契約明文化) hashed 経路の hash ⊇ positions 前提
`select_tickable_hashed` の返却は「hash ∩ positions ∩ その tick の帯」。
hash に無い id は**距離 0 でも tick されない** — 陳腐 hash による近距離
飢餓を防ぐ責務が呼び出し側にあることを doc 契約化 (テストで実証ピン)。
hash query_radius 出力のソート性 (wave 26) と positions の id 昇順が揃う時、
naive 版と**逐一致**することも実証 (順序非依存ではなく厳密同値)。

### 陰性確認
帯境界の等号は次帯落ち (32→Every2 … 256→RenderOnly)、should_tick の
位相列、advance wrapping (2^k 帯は u64 wrap を跨いでも mod が破れない —
2^k | 2^64)、band 5 値の過不足なし — 全て厳密テストで固定。

### 追加テスト (+6, fail-loud)
- band_boundaries_exact (8 点厳密、等号 4 辺)
- should_tick_phase_sequences_and_wraparound (t=0..9 列×3 帯 + MAX→0 wrap)
- select_tickable_exact_membership_and_order (3-4-5 exact dist、帯×位相×順序)
- select_tickable_hashed_matches_naive_on_sorted_ids (逐一致 + hash 陳腐/
  非網羅の契約実証)
- nan/infinite_position_rejected (should_panic ×2)

### 検証結果 (全て実測)
- lib テスト **674/674** (+6)。rustfmt hunk 増分 0 (CRLF 維持、両側 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AK. execute_indirect.rs 監査 (wave 35, 2026-07-22)

`execute_indirect.rs` (MDI コマンド生成: IndirectBatcher / DrawCompactor /
wire 構造体) 全行照合。crate 内消費は azdo 保持 + full_graph_wiring:594
(compactor 実使用)。opt-gfx 674 → **680** (+6)。

### AK-1 (中・静寂欠落) IndirectBatcher::push の上限静寂 drop — fail-loud 根治
上限到達時の push が `return` で黙って捨てる設計で、超過チャンクは
**誰にも知らされず永久欠落**していた (ポップインより重症)。MDI 1 パス
上限超過は呼び出し側の容量設計ミス → fail-loud assert に根治。
wire 契約 (`start_instance_location` = chunk_id 搬送) も併せて明文化
(標準 instance 起点ではない本エンジン独自 semantics)。

### AK-2 (中・容量逸脱) DrawCompactor の capacity 使い捨て — 契約保持+強制
`new(capacity)` が `Vec::with_capacity` のみに使い捨てられ、
`compact_and_filter` は生存数が capacity を超えても無制限に push し
`header.draw_count` も超過していた。固定長 GPU コマンドバッファ前提の
MDI パスで溢れ/欠落の温床 (wave 30 AF の棚卸し事項を閉じる)。
`capacity` を pub フィールドで保持し、受理件数 > capacity を fail-loud 拒否。
現経路 (azdo 8192 cap / full_graph_wiring chunk_keys ≤ 実シーン範囲) 不発。

### AK-3 (doc) sort 二系統の使い分け明文化
`merge_by_material` (安定: 決定性要求経路) vs
`sort_by_material_and_locality` (unstable + L1/L2 ヒット率の chunk 局所性:
同一キー順は非指定) — 選択基準を doc 明記。

### 陰性確認
wire サイズ (DrawIndexedIndirectArgs 20B / ChunkDrawCommand 36B /
IndirectCommandHeader 16B、align 4、Pod 適格)、mask 欠損=可視扱いは
azdo 入口の等長 assert で契約全体として保護 — 全て突合。

### 追加テスト (+6, fail-loud)
- wire_layout_exact_sizes (20/36/16B + align 4 ピン)
- batcher_capacity_boundary_exact + batcher_overflow_panics
- compactor_capacity_boundary_exact (上限ちょうど + mask 欠損受理) +
  compactor_overflow_panics
- merge_by_material_is_stable (同 material 入力順保持ピン)

### 検証結果 (全て実測)
- lib テスト **680/680** (+6)。rustfmt hunk 増分 0
  (HEAD 由来 signature 1 hunk 温存)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AL. billboard_lod.rs 監査 (wave 36, 2026-07-22)

`billboard_lod.rs` (遠距離草花木の LOD: FullMesh→CrossedPlanes→Billboard→
Culled) 全行照合。実消費者は low_spec_stack / render_pipeline。
opt-gfx 680 → **685** (+5)。

### AL-1 (中・静寂蒸発) NaN dist/帯 NaN の全 Culled 着地 — fail-loud 根治
`select` は `<` 連鎖で、NaN dist は全不成立 → else の **Culled に静寂着地
(植物が描画の場から蒸発)** する。entity_tick_lod wave 34 (RenderOnly 飢餓)
よりさらに重症。`assert!(!dist.is_nan())` で fail-loud 化 (±∞ dist は
Culled として意味が通るため受理 — wave 32 の哲学統一: NaN は観測欠損、
±∞ は比較/clamp が責任を持つ)。また pub フィールドの非単調改変
(full>crossed 等)・帯 NaN でも同じ静寂化が起きるため、単調+有限の帯契約
assert を併設。`for_tier_scale(NaN)` は clamp 素通り → 全帯 NaN になる
経路も入口で拒否 (±∞ scale は clamp 1.5 受理)。

### AL-2 (契約明文化) make_billboard の単位直交基底前提
cam_right/cam_up が非単位なら quad 伸縮・せん断、非直交なら平行四辺形化。
正規化はホットパス優先で行わない (呼び出し側責務) — doc 契約化。
頂点順・uv 写像は厳密ピンで固定。

### 陰性確認
帯境界等号=次帯、tier clamp 両端 (0.5/1.5)、corners/uvs 配列の写像
(斜め基底でも同式)、既存 lod_bands 包含 — 全て一致。

### 追加テスト (+5, fail-loud)
- band_boundaries_exact_with_tier_clamps (境界 8 点 + clamp 両端の
  帯値 24/48/96・72/144/288 ピン + inf dist/scale 受理)
- billboard_exact_corners_and_uvs (身份基底 4 corner + 4 uv 厳密列 + 斜め基底)
- select_rejects_nan_dist / select_rejects_non_monotonic_bands /
  for_tier_scale_rejects_nan (should_panic ×3)

### 検証結果 (全て実測)
- lib テスト **685/685** (+5)。rustfmt hunk 増分 0 (CRLF 維持、両側 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AM. texture_atlas_virtual.rs 監査 (wave 37, 2026-07-22)

`texture_atlas_virtual.rs` (sparse virtual texture atlas: タイル常駐管理)
全行照合。実消費者は full_graph_wiring:1140-1159 (TileCoord 生成 →
request_tiles、evict_lru(0) = 現状ノーオップ)。opt-gfx 685 → **691** (+6)。

### AM-1 (中・文化違反) evict_lru は LRU ですらなく、選択が非決定 — 根治
旧実装は「簡易LRU: 任意のタイルを開放」と称して
`HashSet::iter().take()` していたが、HashSet 反復順は RandomState
シード由来で**プロセスごとに非決定** — 退避タイル集合が実行間で揺れ、
再ストリーミング要求と VRAM キャッシュヒット率まで揺らいでいた
(bit 同一性・再現性の文化に正面違反)。LRU 追跡は本モジュールに
存在しないため、(mip 降順, x, y 昇順) の**決定的ヒューリスティック**
(低詳細 mip から優先退避) に根治し、名前と実態の乖離を doc で正直化
(真の LRU は residency 需要時導入と明記)。消費者は現状 evict_lru(0)
(ノーオップ) のため影響なし、先に契約を正した。

### AM-2 (低・NaN 静寂化) capacity_pages=0 の 0 除算 — 契約拒否
new(…, 0) で `residency_ratio()` が 0.0/0.0 = NaN を静寂返却。
0 容量アトラスは無意味 → new で width/height/tile_size/capacity 全て
1 以上を fail-loud 契約化。

### AM-3 (低〜中・範囲外受理) request_tiles のタイル座標無検査 — fail-loud 化
x*tile_size ≥ width の存在しないタイルも常駐化・ページ消費していた。
u64 評価 (u32 乗算 overflow で範囲内への wrap 化けを防ぐ) で範囲 assert。
現消費 (x=m%32<32, 32*128=4096=width 境界内) 不発。mip 段数は本
モジュールの契約外 (呼び出し側管理) と併記。

### 陰性確認
free_pages LIFO 払い出し、常駐済み再要求の idempotent、同一バッチ重複
dedupe、capacity 枯渇時の部分受理 (生存側のみ) — 既存 4 テストの期待と
整合を厳密列で包含確認。

### 追加テスト (+6, fail-loud)
- evict_is_deterministic_low_mip_first (mip 1 優先退避、LIFO 再払い出し
  page 0 ピン、超過退避は頭打ち)
- evict_selection_is_input_order_independent (fwd/rev 2 経路で同一最終状態)
- new_rejects_zero_capacity (should_panic)
- request_rejects_out_of_range_{x,y} + huge_coord_without_overflow_wrap
  (should_panic ×3)

### 検証結果 (全て実測)
- lib テスト **691/691** (+6)。rustfmt hunk 増分 0 (HEAD 由来 3 温存)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AN. gl33_compat.rs 監査 (wave 38, 2026-07-22)

`gl33_compat.rs` (wgpu-backed GL3.3 等価物: VAO/UBO/Instancing/MDI/TimerQuery
互換、Tier 1) 全行照合。crate 内消費者なし (lib re-export のみ)。
opt-gfx 691 → **696** (+5)。

### AN-1 (契約) InstanceBuffer::from_pod の ZST 受理 — fail-loud 化
ZST (size 0) を渡すと stride=0 の instance buffer が生成されるが、
GPU 側 (wgpu vertex stride 0) では受理されず意味を成さない
→ `size_of::<T>() > 0` の fail-loud assert。

### AN-2 (低・観測欠測混入) TimerQueryCompat::end の 0.0 混入 — 根治
`begin` 未対応の `end` (二重 end 含む) が 0.0 を samples に push し、
`average_ms` を意味なく汚染していた (drs wave 23 の NaN と同型の
「観測欠測の混入」)。begin 未対応なら**記録せず** last_ms を返す
(観測欠測 drop) 契約に根治。

### AN-3 (低・無駄 O(n)) samples 上限維持の Vec::remove(0) — VecDeque 化
120 件超過ごとに全要素 memmove する O(n) front-removal を
(Vec 全体を見れば不要な)data movement。
`VecDeque::pop_front` O(1) に置換 (push_back/pop_front)。
用途は 60–240Hz 程度で絶対量は小さいが、Roofline 的に不要な
data movement は原理上削除対象 (複雑さ増分ゼロで支配的改善)。

### AN-4 (doc) total_triangles の端数切捨て明記
`index_count / 3 * instance_count` の端数 (primitive 未完成分) は
描画規則と同じく切捨て — doc 明文化 (挙動変更なし)。

### 陰性確認
DrawIndexedIndirectArgs (20B Pod)、GlVaoCompat::terrain_compact
(stride 12 = 3×u32 packed、PersistentVboPool VERTEX_STRIDE_BYTES と一致)、
UBO dirty 遷移、FrameUbo 96B、アルゴリズム部分の変更が無いこと
(平均計算式不変) — 全て一致。

### 追加テスト (+5, fail-loud)
- timer_beginless_end_does_not_pollute_samples
  (begin 前 0.0 返却/混入なし、二重 end は前値のみ、samples==2 厳密)
- timer_window_capped_at_120 (130 回で 120 件ピン — 構造のみ、
  実時間値は非決定のため対象外)
- instance_buffer_exact_wire_and_zst_rejected (stride/count/raw 厳密) +
  instance_buffer_rejects_zst (should_panic)
- triangles_fraction_floor_and_ubo_transitions (35/3→11×2=22、
  bytes 20B、UBO dirty 遷移列、FrameUbo 96B ピン)

### 検証結果 (全て実測)
- lib テスト **696/696** (+5)。rustfmt hunk 増分 0 (CRLF 維持、両側 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AO. simd_frustum.wgsl 実カーネル化 (wave 39, 2026-07-22)

ユーザー新指令「スタブや見送り無し、全技術を数学的に正しく実装」に基づく
繰越項目 closing 第 1 弾。wave 29 でピンした「コメントのみスタブ」を
**実 compute カーネルに置換**。opt-gfx 696 → **698** (+2)。

### AO-1: 実装 (1 box = 1 thread、CPU 参照との数学的等価)
- `cs_cull` @workgroup_size(64)、Params (count + 6 平面)、SoA 6 配列
  (storage read)、visibility u32 配列 (read_write) の 9 binding 構成。
- 各レーンは CPU 参照 (`cull_soa_portable`) と**同一式を同一規則**で評価:
  p-vertex 選択 (成分ごと `select(min, max, coef >= 0.0)`)、
  dist = `pl.x*px + pl.y*py + pl.z*pz + pl.w` の左結合、
  `dist < 0.0` で 0 を書き early exit (ok フラグ単調減少なので
  full loop と結果一致 — 証明明記)、NaN 係数は min 側選択・NaN dist は
  可視側で CPU scalar 規則と完全一致。

### AO-2: bit 厳密性の限界を正直化 (一次情報調査 + in-repo 実証)
WGSL 個々の f32 演算は IEEE-754 binary32 で左結合も文法固定だが、
W3C WGSL は設計上**評価戦略 (reassociation / FMA fusion) の裁量を
実装に認める**ため CPU 参照との bit 級一致は保証されない
(§floating-point evaluation 系の方針。文言の厳密 pin は spec 本文
139 chunk のため部分確認に留め High confidence として明記)。
乖離機構自体は wave 29 で x86 実機実証 (FMA vs 左結合) 済みの同一種。
境界割れは保守カリングの範囲で実害なしと論証 (AE-2 と同契約)。

### テスト (+2, スタブピンを実ピンに置換)
- wgsl_kernel_entry_layout_and_bindings_pinned
  (cs_cull Compute/wg(64,1,1)、Params span 112 count@0 planes@16、
  8 global が (0, 0..7) に一意 — 全て naga 実機検証)
- wgsl_mirror_matches_portable_bitexact
  (cs_cull 逐行対応ミラーが cull_soa_portable と 17 箱×6 平面で bit 一致)
- wave 29 の「将来カーネル実装時はこのピンを実エントリ検証へ更新」との
  明記に従い stub ピンを置換 (空入力テストは分離存続)

### 検証結果 (全て実測)
- lib テスト **698/698** (+2)。rustfmt hunk 増分 0 (HEAD 由来 6 温存)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AP. texture_atlas_virtual.rs 真の LRU closing (wave 40, 2026-07-22)

「スタブ・見送り無し」新指令に基づく closing 第 2 弾。wave 37 で
「真の LRU は residency 需要時に導入」と棚卸しした項目を、
需要を待たず数学的に正しい形で今根治する。

### AP-1: evict_lru を真の LRU (アクセス論理時刻) に根治
**旧実装の欠陥**: wave 37 の (mip 降順, x, y 昇順) 決定的ヒューリス
ティックはプロセス非決定性こそ解消したが **LRU ではなく**、毎フレーム
`request_tiles` で要求され続ける使用中タイルでも mip が高ければ優先
退避する誤選択があり得た (使用中 VT ページの追い出し = 再ストリーム
嵐の温床)。
**定式化**: 状態 = 常駐集合 R と各タイルの最終アクセス論理時刻 c(t)。
不変条件 `keys(last_access) == resident ∧ 全時刻一意`。
`request_tiles` は解決済み全タイル (新規・既常駐) に単調採番し、
`evict_lru` は時刻昇順 k 件を除去する。**時刻が一意なので tie-break
規則は数学的に出現しない** — 完全決定性はハッシュシード非依存。
**容量枯渇の扱い**: ページを得られなかったタイルは非常駐のままなので
時刻も記録しない (不変条件維持のための必然的設計)。
**mip ヒューリスティックの包含**: 使用中タイルは毎フレームの request が
recency を更新するため、真の LRU は旧ヒューリスティックの意図
(使用中を残す) を厳密に包含する。mip は退避判定から除去。
**u64 wrap の到達不能性 (見送りではなく証明)**: 2^64 アクセスは
10^9 アクセス/秒の持続でも約 585 年を要するため、カウンタ wrap 対策
(リバース) は不要。証明された境界として doc 明記。
**複雑性**: touch O(1) 償却 (HashMap insert)、退避 O(n log n)、
n = 常駐数 ≤ capacity_pages (消費者配線は 2048) で実害ゼロ。
intrusive O(1) リスト / lazy heap は複雑性増に対し利益なしと棄却。
**セマンティクス変更の正直化**: 「退避選択が入力順独立」は旧実装の
欠陥を覆い隠す性質であり、真の LRU では定義上成り立たない。
wave 37-2 テストは「同一履歴リプレイでの完全決定性」ピンに置換し、
recency 依存性そのものを別テストで厳密ピンした。

### AP-2: 消費者影響分析 (full_graph_wiring.rs)
消費者は毎フレーム `request_tiles` (:1153) を呼ぶため recency は実
データで流れる。`evict_lru(0)` (:1157) は no-op で選択規則変更の
影響なし。構築は `VirtualAtlas::new` 経由のみ (:263) で構造体
フィールド追加 (`last_access`, `access_clock`) はコンパイル安全。
wide bench digest 不変を以後の実測で担保。

### テスト (±: -2 +5、計 701)
- evict_is_true_lru_recency_order (時刻 1..4 の厳密採番、最古 2 件
  退避、mip 0 であれ古ければ退避される包含証明、退避順 LIFO ページ
  循環 pin: 再払い出し page 2、超過退避の頭打ち)
- recent_touch_protects_from_eviction (LRU の定義性質: 再要求で
  時刻 1→4 に更新されたタイルが退避を回避、時刻厳密値 4 pin)
- eviction_is_deterministic_for_identical_history (リプレイ完全
  決定性 + 厳密列 pin: free_pages [0,1,2,3,6,5]、生存 {clk 4,5})
- evict_selection_is_recency_dependent (同一タイル集合・異なる
  recency 履歴で生存集合が変わることを assert_ne + 両側厳密 pin。
  wave 37-2 置換の正直なセマンティクス変更記録)
- lru_invariant_keys_match_resident_across_ops (容量枯渇拒否タイルは
  tick しない・時刻を持たない、退避→再要求後も keys==resident)

### 検証結果 (全て実測)
- lib テスト **701/701** (+3 正味)。rustfmt hunk 増分 0 (HEAD 由来 3 温存)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AQ. lbvh.rs 監査 (wave 41, 2026-07-23)

full_graph_wiring:486-526 で毎フレーム実消費 (実チャンク中心/半径の
LBVH 構築 + 実フラスタムカリング)。simd_frustum との相互検証経路の
片割れ。LBVH_WGSL は frame_worldgen 側に CPU ミラー (lbvh_codes_cpu /
lbvh_cull_cpu) 付きの実 dispatch 版で、スタブ類は本モジュールに無し。

### AQ-1 (中): build 契約の fail-loud 化 ×3
- centers/radii 不等長: 旧実装は radii 不足で **build 内 indexing
  panic**、超過で**末尾を静寂に無視**。→ 等長 assert (全実長列挙)。
- NaN center: `(NaN).clamp(0,0.9999)*1024.0 as u32` の飽和キャストで
  **静寂にグリッド隅 code 0 へ配置** (観測欠測の静寂混入)。
  NaN radius 成分は `.max(0.0)` で**静寂に 0 半径化**。→ 全成分
  finite assert で根治 (観測欠測 drop/拒否哲学の統一、wave 32/36 同型)。
- 逆転グリッド (max < min): span `.max(1.0)` が負 span を静寂に 1.0
  矯正して誤グリッド化。→ min ≤ max 成分wise assert (有限性も)。
- 副次: 死行 `let _ = radii;` 除去。
- 消費者安全性の機械確認: full_graph_wiring は同一 chunk_aabbs から
  centers/radii を同長構築し、min/max は centers の fold で有限 (かつ
  `!chunk_aabbs.is_empty()` ガード下) — 実経路を契約は破壊しない。

### AQ-2 (doc): 暗黙定数 2 件の存在理由を明文化
- `0.9999` clamp: 1.0 到達で nx=1024 → part1by2 の 10-bit マスク
  (`& 0x3ff`) で **0 に巻き戻り反対隅とエイリアス**するのを防ぐ
   correctness-critical 定数。従来無文書。
- `span.max(1.0)`: 契約上合法な退化軸 (min==max、全センター同一平面)
  の 0 除算回避であり、逆転軸の救済ではないこと (AQ-1 で塞いだ後の
  残存意味) を明文化。

### AQ-3 (設計): 単位法線契約の明文化 + run 包含球の丸め収縮根治
- **単位法線前提**: 球カリング `dist < -r` は符号付き距離をワールド
  半径と直接比較するため |n| ≠ 1 でスケールが歪む (|n|>1 は可視物を
  削るオーバーカリング)。全 3 供給経路 (full_graph_wiring::
  extract_frustum_planes / frame_worldgen::frustum_planes /
  simd_kernels::frustum_planes_from_view_proj) が正規化することを
  機械確認 — 契約は実経路と整合し欠落文書を補填。
- **run 半径の丸め収縮 (潜在 bit 同一性破れ)**: 葉球を含む AABB の
  半対角を f32 演算列 (sub/mul/add/sqrt) で計算すると丸めで真の包含
  半径を下回り得、run-reject が「葉を残すべき境界 run」を skip し
  naive との bit 同一性を原理的に破り得た。根治: 派生誤差限界で膨張
  `R̃ = R̂·(1+2^-20) + Σ_i(|mn_i|+|mx_i|)·2^-22`
  (第 1 項: 演算列相対誤差上界 ~2^-21 を覆う。第 2 項: 中心丸め
  mn+mx ≤ 成分毎 ulp/2 の 3 成分合成を覆う)。膨張は reject/accept を
  per-leaf 経路へ落とす**保守方向にのみ**効くため bit 同一性を機構的
  に維持する。
- **残リスクの正直化**: dist 評価自体の f32 丸め差 (葉 vs run レベル)
  の厳密解析的上界証明は未倒立 — 膨張第 2 項の包含余裕は Minecraft
  実スケール (~3e7 座標) で ~6×。実スケールを著しく超える入力は
  本保証の契約外として cull doc に明記。

### テスト (+7)
- accelerated_cull_matches_naive_tangent_sweep (正接平面を 0.5 刻み
  x/y 両軸・両向き + slab で 705 ケース走査。全値は厳密表現可能域
  ≤128/分解能 0.5 で構成し f32 丸めを完全排除 — ずれはレベル間判定
  ロジックにのみ帰着。実行数 705 もピン)
- tied_codes_preserve_original_index_order (安定ソートのタイ順 =
  元インデックス順を order/全受入/片側残存の 3 経路でピン)
- degenerate_grid_all_identical_accepted (min==max 退化グリッド受理、
  全 code 0・cull 動作・全拒否の整合)
- build_rejects_length_mismatch / non_finite_center / non_finite_radius /
  inverted_grid (should_panic ×4)

### 検証結果 (全て実測)
- lib テスト **708/708** (+7)。rustfmt hunk 増分 0 (HEAD 0 → 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (膨張後も出力集合が naive 等価であることの digest 級実証)。

## AR. sparse_texture.rs (+ WGSL) 監査・実カーネル化 (wave 42, 2026-07-23)

full_graph_wiring:264/:1143/:1155 で実消費 (1,024 物理スロットの侵入型
O(1) LRU ページテーブル + mip 選択 + 要求)。texture 系 closing 3 連奏の
第 3 弾 (wave 40 VirtualAtlas LRU / wave 41 lbvh に続く)。

### AR-1: request_with_eviction 追加 (退避キー通知の欠落根治)
旧 `request` は退避発生時に「どの key が消えたか」を返さず、戻りは
確保スロットのみ。GPU ページテーブル配線は旧マッピング unmap → 新規
map の順序が必須で、この情報なしに完全な配線は**不可能**だった
(計算済みの old_key を捨てていた)。`request_with_eviction` を追加し
`request` は第 1 要素の委譲ラッパ (公開 API 不変)。

### AR-2: dense-slot 帰納不変条件の明文化 + 死フィールド削除
空き時新規確保が `resident.len()` をスロットに選べる根拠は
「使用中スロットは常に [0, len) に稠密」という非自明な帰納不変条件
(充填は len を +1、満杯退避再使用は len 不変) で、従来無文書だった。
doc 化 + debug_assert を充填経路に追加。`LruNode.in_use` は書き込み
のみで読まれることのない死フィールドだったため削除 (private で API
不変、ノード 16B → 12B)。

### AR-3: mip_for_distance — NaN 静寂着地の根治 + 式意味論の正直化
旧実装は f32::max の「片側 NaN なら他方を返す」性質で NaN distance を
静寂に 1e-3 クランプ経路へ着地させ、mip 0 (最高品質 = 最大転送量) を
黙って選択していた (観測欠測の静寂混入、wave 23/32/34/36 同型)。
→ `!distance.is_nan()` / world_size, screen_h は正の有限値を契約
assert 化。±∞ distance は受理し `+∞ → max_mip` / `−∞ → 0` に飽和する
ことを仕様化 (±∞ 受理・NaN 拒否の哲学統一)。
式の意味論も正直化: 見掛け高は本来 `screen_h·size/(2·d·tan(fov/2))`
であり、本式は `2·tan(fov/2)=1` (fov ≈ 53.13°) を暗黙固定し、分子も
テクスチャ texel 数ではなく screen_h の代理 — 厳密 texel:pixel 選択
ではなく対数ヒューリスティックであることを doc 明記 (max_mip 側に
のみ厳密な飽和保証)。

### AR-4 (closing): sparse_texture.wgsl 実カーネル化
旧版は phys を計算して**出力せず破棄**する reference (wave 39 同型の
準スタブ)。根治: 出力 storage `outPages` (binding 2) 追加、U に
pageCount 追加して契約外レーンの範囲ガード、退化規則を「直接照会 →
**最細 resident 祖先 mip** (mip-1 → 0 の下降探索) → 全段未常駐なら
INVALID」の教科書的フォールバックに拡張 (旧版は mip 0 のみ参照)。
CPU ミラー `translate_with_fallback` を同一制御フローで導入し、
上傳契約 `table.len() >= page_count * (max_mip+1)` を両側に契約化
(usize 64-bit で乗算 overflow 不発を解析確認済: 最大値は 2^64−2^32)。
gpu_runtime は名前索引の naga 検証のみで struct/binding 変更は安全。

### テスト (+10)
- lru_exact_slot_sequence_pinned (dense 充填 0,1,2 / 中間 touch の
  MRU 昇格 / tail 退避 (10,0)→slot 0 / 次 tail (12,0)→slot 2 の厳密列)
- request_with_eviction_reports_exact_key ((page, mip) 完全一致、
  mip 違いは別エントリ)
- translate_with_fallback_exact_table (直接/祖先退化/全段未常駐/
  page・mip 範囲外の 7 ケース厳密)
- translate_fallback_is_finest_resident_ancestor_oracle (固定 seed
  疑似ランダム 40 マス × 独立実装 rev-find オラクルで全 (page, mip) 突合)
- translate_rejects_short_table (should_panic)
- mip_for_distance_exact_structure (log2 が冪 4.0/16.0/1.0 で厳密に
  決まる構造点列 + ±∞/至近飽和の両端)
- mip_rejects_nan_distance / zero_world_size / nan_screen_h
  (should_panic ×3)
- wgsl_entry_layout_and_bindings_pinned (main Compute wg(8,8,1)、
  U span 8 maxMip@0 pageCount@4、binding (0,0..2) — naga 実機)

### 検証結果 (全て実測)
- lib テスト **718/718** (+10)。rustfmt hunk 増分 0 (HEAD 0 → 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AS. mip_streaming.rs 監査 (wave 43, 2026-07-23)

TextureStreamer (full_graph_wiring:265 512MiB 予算で実消費、毎フレーム
set_size/request)。wave 42 に続く常駐管理系 closing の仕上げ。

### AS-1 (高): 単一テクスチャ超過の静寂予算破壊を fail-loud 化
旧 eviction ループは `while used + sz > limit && !lru.is_empty()` —
sz > limit (有効上限) の単一テクスチャに対し全常駐を退避し尽くした
挙げ句、lru 空で脱出して超過テクスチャを**そのまま挿入**し、
予算契約を無通知で破った (テスト未カバーの潜在経路)。
→ `assert!(sz <= limit)` を eviction 前に配置。これによりループは
「最悪でも全退避で必ず収まる」ことが代数的に保証され、`!lru.is_empty()`
の脱出ガード自体が不要になった (終了性が構造から証明される)。

### AS-2 (高): hysteresis 無検証 pub フィールドの静寂キャスト破壊
NaN を設定すると `1.0 - NaN = NaN` → `(budget·NaN) as u64` の飽和
キャストで limit が 0 化、>1.0 でも 0 化 (全要求で全退避+挿入の
スラッシング)、<0 では limit > budget で**予算超過を静寂許容**。
→ pub フィールドは維持 (外部消費者なし・非破壊優先) し、request 入口で
「[0,1) の有限値」を毎回 fail-loud 検証。budget の f64 変換丸め
(2^53 超) は実用 non-issue として doc 明記。

### AS-3 (中): set_size の常駐中サイズ不更新による帳簿ドリフト
旧 `set_size` は sizes テーブルのみ書き換え、常駐中 id の resident 側
サイズは旧値のまま → used() 帳簿が静寂にズレて実効予算が破られた。
→ 常駐中なら bytes を書き換え used_bytes を差分更新。予算強制点は
request 時のみであること (超過時の退避は次回 request まで遅延) を
契約として明文化。

### AS-4 (中): 未登録 id の request 静寂 no-op を fail-loud 化
旧実装は `if let Some` の else 節なしで**静寂に何もしない** —
テクスチャが永久に非表示のまま無信号。プログラマエラーとして panic 化。

### AS-5 (中): 真の O(1) 侵入型 LRU 化 (毎要求 O(n) の根治)
旧実装は `used()` が全区間総和、touch が `lru.retain` で**毎要求 O(n)**。
侵入型双方向リスト (head=MRU/tail=LRU) + スロットフリーリスト +
used_bytes 差分追跡で全経路 O(1) 化。退避順序は旧 VecDeque 版
(front=LRU 側から退避 / MRU 側へ push) と**完全同一**であることを
追跡導出で固定 (lru_victim_order_exact_sequence で機械ピン)。
nodes はピーク常駐数を超えて伸びない (slots_are_reused_not_grown)。

### AS-6: mip_streaming.wgsl は「シェーダ不要」の正当マーカー (誠実化)
コメントのみのファイルはスタブではなく**構造的正当性を持つ設計判断**:
(1) 常駐決定はリソース生成・破棄・上傳というホスト API 操作で、
GPU シェーダはリソース割当を行えない、(2) 退避→上傳→テーブル更新は
シェーダ実行前に確定が必要で、GPU 側決定は 1 フレーム遅れの自己参照
ループを構成する。マーカー形状 (空モジュール・entry/binding ゼロ) を
naga parse テストで機械ピンし、無断のシェーダ混入を検知可能にした。

### テスト (+11)
- lru_victim_order_exact_sequence (touch 後の犠牲列 0→2 を厳密ピン)
- budget_boundary_equality_inserts_without_eviction (used+sz==limit は
  退避しない厳密等価、+1B で 1 件のみ退避 → 501B)
- resize_while_resident_keeps_accounting_consistent (拡大/縮小の即時
  帳簿反映、新サイズでの連鎖退避 1010→480)
- slots_are_reused_not_grown (100 回出入れで nodes ≤ 4)
- touch_is_accounting_neutral (head 自身 touch の no-op 経路 + tail
  touch の MRU 昇格が帳簿中立)
- request_rejects_unknown_id / oversized_texture / nan_hysteresis /
  hysteresis_one / negative_hysteresis (should_panic ×5)
- wgsl_is_intentionally_shader_free_marker (naga 空モジュールピン)

### 検証結果 (全て実測)
- lib テスト **729/729** (+11)。rustfmt hunk 増分 0 (HEAD 由来 1 温存)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AT. mesh_compactor.rs ワイヤ構造体 closing (wave 44, 2026-07-23)

見送り棚卸し closing 第 3 弾。wave 28 の契約注記 1「将来の GPU 配線は
明示ワイヤ変換必須」を、配線実機に先んじて決定的に解消する
(ワイヤ構造体そのものは CPU 側で完全に決定可能であり、GPU 待ちの
技術的理由は存在しない — 棚卸し解除は正当)。

### AT-1: CandidateWire (48B) — WGSL 配置規則の厳密再現
WGSL struct 配置は「メンバ i+1 の offset = roundUp(align(i+1), end(i))」
(vec3<f32> の trailing gap を後続スカラが再利用する) ため、
中心 @0..12 [穴 12..16] half_ext @16..28 の直後 @28 に vertex_count が
載る。Rust repr(C) では `[f32;3]`(+`_pad0: u32`) で穴を 1 本だけ明示
すれば同型にでき、**`_pad1` を追加すると 32 開始となり GPU 配置と
永久的にズレる**ことを doc で禁則化。オフセット列 (0,16,28,32,36,40,44)
span 48 は wave 28 の naga 実測と閉じた一致 (両側独立ピンで相互
ドリフト検知)。bytemuck::Pod derive 自体がパディング不在をコンパイル
時証明。変換は全フィールド明示 (bool→0/1 の `u32::from`、u16→u32 ゼロ
拡張、予約 frustum_seen=0、穴ゼロ)。

### AT-2: ParamsWire (128B) + policy.z の f32 輸送契約
planes@0 camera@96 policy@112 span 128。`policy.z` は candidate_count を
**f32 として輸送**する (WGSL 側 `u32(params.policy.z)`) ため、2^24 超では
整数厳密性が破れ誤カウント化する — `MAX_COUNT = 1<<24` と構築時
fail-loud で契約化。camera.w / policy.w の予約 0 も明示。
`IndirectDrawCmd` は従来から Pod で Cmd (16B) と同型だったが、
その旨の doc と size/offset ピンを補完して契約を閉じた。

### テスト (+5)
- wire_layout_offsets_match_naga_pins (offset_of! で 0,16,28,32,36,40,44
  /48、Cmd 16、Params 0/96/112/128 — naga ピンとの鏡合わせ)
- to_wire_is_byte_exact (0xAABBCCDD 等の視認パターンで LE バイト位置・
  ゼロ穴・bool 0/1・u16 拡張を 48B 全域機械固定)
- params_wire_is_byte_exact (plane[0]/camera/policy のバイト厳密、
  count 11 → 11.0f32 LE)
- candidates_to_wire_preserves_order_and_len (3 件順序保持 + 144B cast)
- params_wire_rejects_count_over_2pow24 (should_panic)

### 検証結果 (全て実測)
- lib テスト **734/734** (+5)。rustfmt hunk 増分 0 (HEAD 由来 4 温存)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AU. azdo.rs MDI コマンドリスト準備 closing (wave 45, 2026-07-23)

見送り棚卸し closing 第 4 弾。wave 30 で正直化した「persistent ring
push / MDI コマンドリスト準備は未配線」のうち、**本パスの責務内で
CPU 側に決定的に配線可能な部分**を閉じる。

### AU-1: batcher 実配線 (MDI 準備)
`execute_azdo_pass` は compaction しか行わず、`IndirectBatcher` は保持
されるだけだった。生存コマンドを `batcher` に積み直し、
`batcher.as_bytes()` で **GPU 間接バッファへそのまま上傳可能な
決定的バイト列 (ChunkDrawCommand 36B × N)** を提供する経路に根治。
- 順序は compactor と一致 (安定) — 生存集合の二写しを機械ピン。
- push の容量 assert は構造上不発 (生存数 ≤ compactor 容量 =
  max_commands = batcher 容量、wave 35 の fail-loud が保証)。
- wire 契約差分の明文化: batcher::push は `instance_count = 1` に正規化
  (`start_instance_location = chunk_id` 搬送設計のため多インスタンスは
  存在しない)。消費者 (full_graph_wiring:573-577) も instance_count=1
  で構築しており整合。

### AU-2: vbo_pool 未配線は責務分離として明文化 (スタブではない)
persistent ring への頂点 push は `BuiltChunkMesh` を必要とし、
draw コマンドのみを入力とする本パスのシグネチャには存在しない。
「draw 圧縮パス (本パス)」と「メッシュ常駐パス
(`upload_mesh`/`release`/`rebuild_mdi`、独自テストで稼働中)」の
分離は意図的責務境界であることを doc に明記し、追跡可能な形で閉じた。

### テスト (+3)
- pass_wires_survivors_into_batcher_byte_exact (生存 2 件の順序保持、
  start_instance_location = chunk_id 上書き、base_vertex 負値、
  material 引き継ぎの厳密確認)
- pass_batcher_is_cleared_between_calls (連続呼出 clear、残滓なし、
  全除去で空整合)
- batcher_mirrors_compactor_surviving_set (32 件偶数マスクで生存集合・
  順序・全引き継ぎフィールド一致 + 生存 chunk_id 列 100,102,…,130 ピン)

### 検証結果 (全て実測)
- lib テスト **737/737** (+3)。rustfmt hunk 増分 0 (HEAD 由来 1 温存)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (消費者は戻り値 count のみ使用、batcher 後読みなしを機械確認済)。

## AV. bindless.rs 監査 (wave 46, 2026-07-23)

full_graph_wiring:1136・frame_worldgen:613/:1108 で実消費
(ハンドル pack/unpack + WGSL 実 dispatch カーネル cs_unpack_handles)。

### AV-1 (高): pack_handle の静寂切捨てマスクを fail-loud 化
旧実装は `& 0xF` / `& 0xFF` / `& 0xFFFFF` で範囲外入力を**静寂に
切り捨て**、例えば set=16 が set=0 と完全衝突する**非単射**だった
(誤 texture 参照に直結するハンドル衝突)。旧テスト
`index_overflow_wraps_within_field` はこの wrap を**期待仕様として
固定**していた — パッキングの要請は全単射であるため、入力範囲契約
(set<16 ∧ binding<256 ∧ index<2^20) の fail-loud assert に根治し、
当該テストは should_panic ピンに置き換えた (wave 37→40 と同型の
正直なセマンティクス反転)。消費者 3 経路 (full_graph_wiring・
frame_worldgen テスト・frame_proof_extra example) は全て範囲内入力
を機械確認。WGSL 側は契約内で bitwise 一致 (GPU は assert 不能のため
mask 実装のまま防御整合) であることを doc 明記。
**テスト赤検出が自己誤りを捕捉**: 初版テストは `0x1FFFFF+1` を
「2^20」と誤記していたが `0x1FFFFF = 2^21−1` であり、panic メッセージ
実測値 2097152 との不一致で即座に露出した (K-6 教訓の実践確認)。
厳密形は最初の範囲外値 `1<<20 = 1048576` を入力とする。

### AV-2: 死コード Vec3/Vec4 削除
モジュール冒頭の Vec3/Vec4 (四則演算実装付き約 60 行) は pack/unpack
と無関係に残置され、モジュール内・workspace 全体で参照ゼロを機械
確認して削除 (公開 API だが workspace 全 consumer 非参照、
`cargo check --all-targets` で無影響を検証)。

### テスト (+4 正味: -1 +5)
- layout_bit_positions_pinned (set/binding/index の各ビット位置を
  0x8000_0000/0x0800_0000/0x0008_0000 と全 1 端点 0xFFFF_FFFF で固定)
- roundtrip_exhaustive_on_structure (set 全域 16 × binding 6 境界値 ×
  index 6 境界値 = 576 経路の厳密往復)
- pack_rejects_out_of_range_index/set/binding (should_panic ×3、
  index は最初の範囲外値 2^20 で)

### 検証結果 (全て実測)
- lib テスト **741/741** (+4)。rustfmt hunk 増分 0 (HEAD 0 → 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AW. meshlet_cone.rs 監査 (wave 47, 2026-07-23)

full_graph_wiring:1121 で実消費 (面法線クラスタ錐体カリング)。

### AW-1: 可視判定の数学的正当性を検証の上で明文化
錐体 (axis A, 半角 α) 内の `max dot(n, V)` = `cos(max(0, β−α))`
(β = angle(V,A))。「一部でも前面」 ⟺ `max dot > 0` ⟺ `β < α+90°` ⟺
`cos β > −sin α`。よって `cos_angle = −sin(α)` との比較は厳密に正しく、
実装は正しかった — ただし無文書だったため導出を doc に明文化
(境界 == は conservative 可視側処理でこれも正しい)。

### AW-2 (高): NaN 法線の静寂な永久カリングを根治
NaN 法線は `f32::min` の「片側 NaN なら他方」を経て min_cos を偽装し、
`dot >= cos_angle` の NaN 比較で**メッシュレットを永久カリング**して
いた (billboard_lod AL-1 同型の形状蒸発)。観測欠測の静寂混入を拒否:
from_normals / visible の成分 finite assert で fail-loud 化。
また、空 normals で axis がゼロへ退化して静寂に「常に可視」へ着地する
経路も非空 assert で塞いだ (消費者は !is_empty ガード済で整合)。

### AW-3 (高): roundoff による sqrt(負) NaN → 永久カリングを根治
f32 では単位ベクトル同士の dot が丸めで −1−2^-23 まで振れ得るため、
`min_cos` は上界 1.0 に鉗制済みでも**下界は鉗制なし**で、
`1 − min_cos²` が負 → `sqrt(負) = NaN` → cos_angle NaN → 比較恒偽で
**静寂な永久カリング**が起き得た。被開ケ子の `max(0.0)` 鉗制で根治
(数学的根拠: roundoff 超過は高々 2ε で、真の sin は 0 に近い)。
退化 (法線和ゼロ → axis ゼロ → cos_angle = −1 で常に可視、
|ax| < 1e-8 縮小 axis → conservative 側のみ変形) も「誤りにならない
方向にのみ倒れる」設計であることを検証の上で文書化。

### テスト (+7)
- from_normals_single_exact_bits (axis 厳密一致、cos_angle = −0.0 の
  ビットピン、正面/背面/edge-on/カメラ真上の 4 規則)
- antiparallel_degenerates_to_always_visible (ゼロ axis、cos_angle =
  −1.0 厳密、全方向可視)
- nearly_antiparallel_never_produces_nan (eps 1e-20/-30/-38 の 3 構成で
  cos_angle・axis の有限性 + 正面可視の回帰ガード)
- boundary_equality_kept_visible (3-4-5 三平方で正規化除算を 0.8/−0.6
  に正確丸めし、dot == cos_angle の bit 一致を構成 → 可視側ピン)
- from_normals_rejects_empty / rejects_nan_normal /
  visible_rejects_nan_to_camera (should_panic ×3)

### 検証結果 (全て実測)
- lib テスト **748/748** (+7)。rustfmt hunk 増分 0 (HEAD 0 → 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AX. intern_pool.rs 監査 (wave 48, 2026-07-23)

full_graph_wiring:98/:1135 で実消費 (マテリアル ID インターン)、
examples でもベンチ用に消費。

### AX-1 (高・ゼロデイ級): 二重 release の release build 無防備を根治
`release` の refs>0 検査は **debug_assert のみ**で、release build では
二重 release が `0 − 1 → u32::MAX` アンダーフローにより実体を不死化、
さらにスロット再利用後の誤 release が新規オーナーの参照カウントを
奪う永久破壊に発展し得た。全ビルド fail-loud の `assert!` に根治し、
範囲外 ID も明示メッセージ化。
**偽主張の撤去**: 「ABA 防止用」と謳われた `gen` フィールドは一度も
読まれない write-only であり、InternId に世代も入っていないため ABA
検出は原理的に不可能だった。gen を撤去し「intern/release の 1:1 対応は
呼出側規律」を正直化 (write-only な状態保持は将来の誤用を誘う)。
(参考: workspace 内に InternPool::release の実消費者は存在しない)

### AX-2 (高): quantize の i32 飽和誤共有を根治
`(aabb/q).round() as i32` は i32 域外で**飽和潰れ** — Minecraft ワールド
端 (±3e7 blocks) を細かい quant (例 1e-3) で量子化すると 3e10 quanta ≫
2^31 で、**遠方形状が全て i32::MAX に潰れて別形状と誤共有**される
(衝突形状の誤マージに直結)。NaN 入力も飽和 0 → 原点ボックス誤共有。
→ intern_shape 入口で成分 finite + `|v/q| < 2^31` を fail-loud assert 化
(量子化域の明示)。`ShapeCache::new` の `.max(1e-4)` 静寂矯正も
「正の有限値」契約 assert に置き換え (消費者は 0.001/0.01 のみで整合)。

### テスト (+7)
- quantize_rounding_exact_values (round half away from zero の厳密列
  [−0.25,0.75]→[−1,2]、sort+dedup 正準形の順序ピン)
- release_rejects_double_release / release_rejects_out_of_range_id
  (should_panic ×2)
- intern_shape_rejects_nan / rejects_far_coordinates (should_panic ×2、
  3e6 blocks ÷ 1e-3 = 3e9 > 2^31 の現実的到達点で)
- shape_cache_rejects_zero_quant / rejects_nan_quant (should_panic ×2)

### 検証結果 (全て実測)
- lib テスト **755/755** (+7)。rustfmt hunk 増分 0 (HEAD 0 → 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- cargo check --all-targets エラー 0 (gen 撤去は private 構造体内で完結)。

## AY. string_intern.rs 監査 (wave 49, 2026-07-23)

full_graph_wiring:99/:1139 で実消費 (ブロック名シンボルインターン)。

### AY-1 (高): InlineStr の from_utf8_unchecked 前提を pub フィールド経由で
迂回可能だった UB 経路を型レベル封鎖
`InlineStr { pub data, pub len }` は任意コードが `[0xFF; 15]` 等の
**無効 UTF-8 を直接構築**可能で、`as_str()` の unsafe
`from_utf8_unchecked` 前提を破り**即 UB** となり得た。
→ フィールドを private 化し、構築経路を `try_from_str` (&str 由来で
UTF-8 保証) のみに限定 — 不変条件を型で強制。PartialEq/Hash は
0 パディング保証により全 16B 比較と内容等価が一致することを併記。
workspace 内で InlineStr の実利用はゼロ (消費は CompactSymbolTable
のみ) であることを機械確認し、all-targets check で無影響を検証。
len()/is_empty() アクセサを補完。ヘッダ doc の「[u8; 16] 内部に格納」
は実体 ([u8;15]+u8) と微妙に乖離していたため明記に修正。

### AY-2: CompactSymbolTable の決定性を機械ピン (監査で正当性確認)
ID 追加順連番、append-only 安定性、resolve 往復、bytes_saved の
蓄積規則は全て正しく設計されていた — 検証済の上で厳密値をピン化。

### テスト (+3)
- inline_str_boundary_lengths_exact (0/1/15/16B 境界、ASCII・マルチ
  バイト・空の往復、0 パディング一致による内容等価性)
- symbol_table_id_sequence_and_resolve_roundtrip (ID 連番 (0,1,2)、
  再インターン既存化、resolve 往復、範囲外 None)
- bytes_saved_exact_accumulation (miss/hit の蓄積規則を 36+30+30 で
  厳密ピン)

### 検証結果 (全て実測)
- lib テスト **758/758** (+3)。rustfmt hunk 増分 0 (HEAD 0 → 0)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

## AZ. texture_budget.rs 監査 (wave 50, 2026-07-23)

render_pipeline.rs:43/:104/:173 で実消費 (プラットフォーム別圧縮形式
選択・帯域モデル・mipmap bias 保持)。

### AZ-1 (中): mipmap_bias 非有限の静寂許容を fail-loud 化
`from_profile` は bias を Feather 経路でそのまま格納する — NaN が
混入するとサンプラ LOD bias の NaN 化として下流に漏洩し、挙動が
不定になる (wave 32/36 の NaN=汚染哲学に照らし入口拒否が責務)。
非 Feather 経路では bias を破棄するが、**入力契約は統一して検証**
(経路依存の検証抜けは将来のリファクタで穴になる)。
→ `assert!(mipmap_bias.is_finite())` を全経路共通の入口に設置。

### AZ-3 (中・事実誤り訂正): Astc4x4 の bandwidth_factor 0.2 → 0.25
ASTC は**全 block サイズで 128 bit 固定** (KHR_texture_compression_
astc_hdr §C.2.3)。4×4 = 16 texel より **8.00 bpp** であり、
RGBA8 (32 bpp) 比は 8/32 = **0.25** が厳密。
旧値 0.2 = 6.4 bpp は ASTC **5×4** (128/20) の値で、型名・label
「4×4」と矛盾していた (帯域見積りが 20% 過小 = 帯域削減効果を
過大評価する方向の誤り)。BC7 側 0.25 (=128bit/16texel=8bpp) は
正しいことを併せて確認。導出を doc に明記し今後の任意値混入を防ぐ。
(一次情報: Khronos 拡張仕様の footprint/bit-rate 表: 4×4=8.00,
5×4=6.40 bit rate。)

### テスト (+4)
- bandwidth_factor_matches_exact_bpp: factor × 32 = {32, 8, 8} bpp
  を厳密等値でピン (0.25/1.0 は 2 の冪で f32 乗算は厳密)。
- from_profile_rejects_nan_bias / rejects_inf_bias /
  rejects_nan_bias_even_when_discarded (should_panic ×3)。
- 既存 bandwidth_factor_and_labels_exact の Astc4x4 期待値を
  0.2 → 0.25 に訂正 (訂正由来コメント付き)。

### 検証結果 (全て実測)
- lib テスト **762/762** (+4)。rustfmt hunk 増分 0
  (HEAD 1 → 1、同一 hunk = HEAD 由来の既存逸脱のみ)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- cargo check --all-targets エラー 0
  (warnings は rsift-api 側 HEAD 由来の unused import のみ)。

## BA. zerocopy_cast.rs 監査 (wave 51, 2026-07-23)

render_pipeline.rs:419/:712/:787/:793 で実消費 (quad バイト列の Pod 相互
再解釈。`cast_slice_to_bytes` 2 経路 + `cast_bytes_to_slice` 1 経路)。

### BA-1 (中): ZST 宛で `% 0` 剰余ゼロ除算の偶発的パニック → 明示契約化
`cast_bytes_to_slice::<T>` は `bytes.len() % size_of::<T>()` を先に評価
するため、**ZST 宛では任意入力でゼロ除算パニック** (メッセージは
"attempt to calculate the remainder with a divisor of zero" = 実装事故と
区別不能)。bytemuck 自身は ZST 宛を空入力限定で受理する設計
(vendored internal.rs: ZST なら出力長 0 で Ok) であり、ラッパの挙動は
bytemuck 意味論と乖離していた。GPU 転送バッファ再解釈に ZST の正当用途は
存在しないため、入口で `size_of::<T>() > 0` を **assert 契約化**
(fail-loud 哲学どおり、意図と検査位置を一致させる)。

### BA-2 (中): 消費者ゼロの「嘘ヘッダ」API 撤去
`GpuUploadHeader` + `zero_copy_vertex_upload` は workspace 全域で
消費者ゼロ (grep 機械証明) に加えて、返却する static ヘッダが
`vertex_count: 0 / index_count: 0` 固定 — **ペイロード長に関わらず
読み手に偽の枚数を提示する**意味的誤り。doc の「Upload Heap に直接
書き込む想定」は願望スタブ。実消費経路は `cast_slice_to_bytes(..).to_vec()`
直で誠実。ヘッダ前置ワイヤ形式が将来必要になった場合は実カウント保持の
所有型として再設計すべきで、嘘を返す静的実装の温存は利益がない
(bindless Vec3/Vec4 撤去・intern_pool gen 撤去と同規律)。

### BA-3 (観測・render_pipeline wave へ引継ぎ): unwrap_or_default の静寂退化
render_pipeline.rs:787 は `cast_bytes_to_slice(..).map(..).unwrap_or_default()`
で、万が一の非整列/ラギッド時に quad 列を**静寂に空化** → 以後
`!is_empty()` ガードで恒久に quad 消失し得る。現行の本クレート内
生成経路 (to_vec 由来) では実質整列保証されるため発火確率は低いが、
fail-loud 哲学からは逸脱。render_pipeline.rs 本監査で対処する。

### テスト (+2 純増 / -1 撤去 +3 新規)
- cast_bytes_to_slice_rejects_zero_sized_target{,_nonempty} (should_panic ×2)
- cast_bytes_to_slice_alignment_boundary_exact: align(16) 保証バッファの
  +1/+4 オフセットで**配置の偶然に頼らない**決定的な None/Some 境界
  (Quantized12ByteVertex が repr(C, align(4)) であることを前提確認済)
- upload_header_wire_format は撤去 API に伴い削除

### 検証結果 (全て実測)
- lib テスト **764/764** (+2)。rustfmt hunk 3 → 0 (全面改訂に伴い正準化)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- cargo check --all-targets エラー 0 (pub API 撤去の無影響を機械検証)。

## BB. half_vertex.rs + rsift-dx12/vertex.rs 監査 (wave 52, 2026-07-23)
### f16 エンコーダ・クラスタの数学的根治

repo 内に f32→f16 エンコーダが 3 実装あることを棚卸し:
(1) opt-gfx `half_vertex::f32_to_f16` (実消費: frame_proof_extra example
経由の WGSL デコード対)、(2) rsift-dx12 `vertex::f32_to_f16_bits`
(PackedVertex UV パック)、(3) opt-gfx r10g10 `f32_to_f16_bits` (「簡易」
自称版 → wave 53 候補、BB-3 として引継ぎ)。
境界期待値は W-3 規律どおり **Python Fraction 厳密有理数 RNE オラクル**で
導出 (f32/f16 いずれの候補値も f64 で厳密表現可能、tie 判定の等距離は
丸め後も同値に潰れるため f64 比較は安全 — 全演算手続き的丸め)。

### BB-1 (高): half_vertex::f32_to_f16 の「RNE」doc 嘘 → 真の RNE 根治
旧実装は `mant >> 13` の**切捨て**で、doc の "Round-to-nearest-even" と
乖離 (誤差最大 1 ulp、例: 1+3·2^-12 = 0.75 ulp 位置で 0x3C01 であるべきが
0x3C00)。overflow 境界も不正 (65520 以上でも切捨てで 0x7BFF を返した)。
→ 真の RNE を全正常レンジに実装。dropped 13bit が半分超、または半分で
keep 奇数 (ties-to-even) なら繰上げ。mantissa 溢れは指数へ自然に桁上げ
され **65520 → Inf が mechanism として自然実現** (65504/2^16 の tie で
even = Inf 側)。FTZ は宣言どおり維持するが「subnormal グリッド込み RNE
ののち flush」に厳密化 — 2^-14-2^-25 の tie 薄帯は min normal (0x0400) へ
丸め上げ (IEEE FTZ 動作と一致)。NaN は payload 非保持の正準 qNaN
(0x7E00、sign 保存)。decoder f16_to_f32 は全入力厳密 (dyadic 有理数の
各項が f32 に正確表現可能) であり WGSL ビット配置デコードとも bit 一致、
**変更なし**のまま厳密性を全テーブル機械ピン化。

### BB-2 (高・実バグ): rsift-dx12 vertex::f32_to_f16_bits の subnormal RNE
破綻 + cfg(windows) による検証封鎖
subnormal ブランチの丸め式が `round = (keep & 1) | (sticky != 0)` —
**round_bit ゲート欠落**。(a) ドロップ bit 非零なら半 ulp 未満でも常に
繰上げ (2.25·2^-24 → 0x0002 であるべきが 0x0003)、(b) ドロップ 0 の
**厳密表現可能値でも keep 奇数なら +1** (3·2^-24 = 0x0003 厳密が 0x0004 に
化けた)。normal ブランチは正しい RNE だった (round_bit && (sticky||odd))。
→ dropped/half 比較による正しい ties-to-even に根治 (0x3FF+1 → 0x400 の
min normal 遷移は整数桁上げで自然実現)。
さらにこの純粋 bit 演算モジュールは **`#[cfg(windows)]` で gate されて
おり Linux CI/sandbox から一切検証不能だった** (winapi 非依存なのに)。
tests が 0 件として静寂スキップされる状態だった → `pub mod vertex` を
error/phase/win の非 cfg グループへ移動して gate 解除し、実テスト実行を
回復 (fmt hunk は HEAD 2 (cfg 群の HEAD 由来並べ替え逸脱) → 1 に減少)。

### テスト (opt-gfx +4 純増 9 件 / dx12 +3)
- RNE tie/繰上げ厳密ピン (1+2^-11→0x3C00, 1+2^-10+2^-11→0x3C02,
  1+3·2^-12→0x3C01, 負側対称)
- overflow 境界ピン (65504/65519/65520→Inf, ±65520, ±Inf)
- FTZ/subnormal 境界ピン (2^-25 tie→0, 2^-14-2^-24→flush 0,
  2^-14-2^-25 tie→0x0400, 4095·2^-26→0x0400, ±0 符号保存)
- NaN 正準化ピン (qNaN/負 NaN/signaling→quiet)
- **全テーブル機械検証** (65536 全 codeword: decode 分類・単調性・
  再エンコード則 — normal/±0/Inf は恒等、subnormal→±0、NaN→正準)
- **LCG 2^18 サンプル全範囲オラクル突合** (テスト内独立 bisect RNE
  オラクルと codeword 完全一致)
- テスト过程中にテスト側の 2 バグを自己捕捉 (exp フィールド未マスク、
  FTZ 判定を符号合成後に実施) — 実装は指定通りで赤が私のテストを矯正
- dx12: subnormal RNE 10 ピン (★旧バグ回帰 2 件 0x0003/0x03FF 含む) +
  normal/overflow 回帰 9 ピン

### BB-3 (観測・wave 53 第1候補): r10g10 「簡易」f32_to_f16_bits
truncation (doc は「簡易」と正直) に加えて **NaN → Inf 静寂変換**
(exp>=31 → 0x7C00 直落ち) の欠陥あり。pack_r10g10b10a2 の clamp(NaN)
=NaN→as u32=0 静寂着地 (AH-1 同型) など r10g10 全体の NaN 哲学と
絡むため、同モジュール本監査として wave 53 で一体処理する。

### 検証結果 (全て実測)
- opt-gfx lib **768/768** (+4)、dx12 lib **4/4** (+3、gate 解除で実実行化)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (広範 bench は half_vertex 非消費)。
- fmt: half_vertex.rs 0→0 (全面改訂後に正準化)、vertex.rs 0→0、
  lib.rs は HEAD 2 → 1 (cfg 群 HEAD 由来逸脱、増分なし)。
- cargo check -p rsift-opt-gfx -p rsift-dx12 --all-targets エラー 0。

## BC. vertex_compression_r10g10.rs 監査 (wave 53, 2026-07-23)
### BB-3 引継ぎ事項の一体処理 — スタブ全廃と数学的根治

workspace 消費者ゼロ (lib.rs 宣言のみ) の API 完成度棚卸しモジュール。
「スタブや見送り無し・すべて数学的に正しく」の指針に基づき全面根治。

### BC-1 (高): 第 3 の f16 実装 (「簡易」版) を撤去し proven 実装に一元化
旧 `f32_to_f16_bits` は truncation (round 無し) に加え、**NaN 入力を f16
Inf (0x7C00) へ静寂変換**する欠陥があった (exp>=31 直落ち分岐)。
pack_uv の `clamp(0.0,1.0)` は (a) NaN 素通し (AH-1 既知) を経て NaN→Inf
化を完成させ、(b) **タイル UV (範囲外パラメータ) を静寂破壊**していた。
→ wave 52 BB-1 で真の RNE + FTZ 化・全テーブル機械検証済みの
`half_vertex::f32_to_f16` に全面移管 (実装の単一化 = 数学的真実の単一化)。
pack_uv は有限値 assert (±65504 までの任意 UV を正直に保持、
|uv| ≥ 65520 は f16 Inf へ RNE 規約どおり)。

### BC-2 (中): UNORM pack の切捨てバイアス → round-to-nearest
`(x.clamp * 1023.0) as u32` の切捨ては 0.5/1023 の系統的下方向バイアス
(全 texel がわずかに暗く/低く量子化される)。D3D 固定小数変換の
意味論どおり round-to-nearest (f32::round, ties-away) に根治。
併せて成分の有限 assert と a2 ≤ 3 assert を入口強制 (旧: 上位 bit 静寂
マスク)。unpack 逆変換の誤差は半量子 + 除算丸め (テスト境界を 1/1023
から 0.5/1023 + eps へ厳密化)。

### BC-3 (中): compress_vertex_stream の zip 静寂打切り → 等長 assert
長さ不一致のストリームを静寂に最短長で打ち切っていた (AK-1 同型)。
等長 assert 化。併せて positions は chunk-local [0, `CHUNK_POS_SCALE`=64]
有限の契約を assert (範囲外は clamp 先への幾何 welding = 破壊バグ)。

### BC-4 (高・スタブ根絶): normal_oct 常時 0 スタブ → octahedral 完全実装
旧 stream は `normal_oct: 0` を出力 (全法線が物理的に (0,0,1) を偽装)。
Cigolle 系 octahedral 写像を完全実装: L1 正規化 → z<0 半球の
(1-|y|,1-|x|)·sign 折畳み → 16bit×2 SNORM 量子化 (round-to-nearest)。
符号規約 sign(0)=+1 を固定し (0,0,-1) → (32767,32767) の一意性を確保。
復号参照 `unpack_normal_oct` (CPU 検証用・将来 GPU デコード配線の
参照仕様) も実装。契約: 成分 finite・L1>0 (ゼロ法線 assert)。

### 検証 (全て実測/厳密導出)
- 軸・fold 量子化ピン: 期待値は独立手導出 (+X→0x0000_7FFF、-Z→0x7FFF_7FFF、
  (±1,±1,±1)/√3 → round(32767/3)=10922 / round(-2/3·32767)=-21845)。
- 復号往復角度誤差: Python 400k サンプル推定 ≈ 6.4e-5 rad に対し
  15× マージン 1e-3 rad 閾値で LCG 200k 方向 sweep (f64 acos 評価)。
- 軸法線 (±X,+Y,+Z,-Z) は復号厳密往復。
- 12B サイズ compile-time assert (旧 doc「16byte/帯域1/3」は実体 12B/1/4
  と乖離 → 文書訂正)。
- lib **773/773** (+5)、digest `004c1cf5fb17bfe8` rows=357 不変、
  fmt 5→0 (全面改訂に伴い正準化)、all-targets check 0。

## BD. enhanced_barriers.rs 監査 (wave 54, 2026-07-23)
### D3D12 Enhanced Barriers モデルの仕様適合化 (一次情報: Microsoft DirectX-Specs/D3D12EnhancedBarriers.md)

full_graph_wiring.rs:123/:262/:1166-1177 で実消費 (transition + uav_barrier
→ flush。flush 結果は len の debug_assert のみに消費される構造モデル)。
**既存 wiring 出力は全て仕様適合**であることを監査で確認 (All/All sync は
universal 適合、UAV バリアは Compute×UAV / UAV layout×UAV で適合)。

### BD-1 (中): 「分割バリアでオーバーラップ」の doc 嘘 → 全パラメータ API
旧 `transition` は sync を All/All にハードコード — Enhanced Barriers の
核心である細粒度 Sync スコープ (これが overlap を可能にする機構) を
**表現不能**であり、ヘッダ主張は願望だった。全 8 パラメータ指定の
`barrier()` (spec TEXTURE_BARRIER 対応) を追加し、transition は安全側既定
(保守的だが誤りではない All/All) の簡易 API と明確化。ヘッダを実体に
正直化。

### BD-2 (中): 仕様互換性表の機械検証 `TextureBarrier::validate` を全構築経路に強制
一次情報から 3 規則を機械化:
1. **Layout-Access 表**: Common/Present↔{SRV,CopySrc,CopyDst}、
   GenericRead↔{SRV,CopySrc}、RenderTarget↔RT、UAV↔UAV、DepthWrite↔DSW、
   CopySrc/Dst↔同名のみ。NoAccess/Common は任意レイアウト (no-claim 規則)。
2. **Access-Sync 表** (モデル sync 変種への射影): All=universal、
   Draw↔{VB,IB,CBV,SRV,UAV,RT,DSW}、Compute↔{CBV,SRV,UAV}、
   Copy↔{CopySrc,CopyDst}、None↔NoAccess のみ (モデル規則)。
3. **buffer-layout 排他原則** (spec: "Buffer resources have only a linear
   layout, regardless of access type"): VertexBuffer/IndexBuffer/
   ConstantBuffer access は texture barrier では**カテゴリエラー**として
   拒否 — 旧実装は texture バリアに混入可能だった。
また `D3D12_BARRIER_LAYOUT_PRESENT == LAYOUT_COMMON = 0` の**エイリアス**
である一次情報を確認し、Present の互換判定は Common と同一に厳密化。

### BD-3 (低): uav_barrier の subresource 暗黙 0 → 明示引数化
旧実装は subresource を暗黙 0 固定 (transition の全指定 0xFFFFFFFF と
不整合な黙契約)。`uav_barrier(id, subresource)` に明示化し、wiring 呼出は
`(2, 0)` で出力内容を bit 不変に保持 (SUBRESOURCE_ALL 定数も公開)。

### テスト (+5)
- barrier_full_params_exact (全 8 フィールド保持 + 細粒度スコープ例)
- validate_layout_access_table_exact (19 ペア表ピン + 全レイアウト×
  {NoAccess,Common} 許容スイープ)
- validate_sync_access_table_exact (射影表ピン、layout 適合は分離)
- validate_rejects_buffer_access_in_texture_barrier (3 buffer access 拒否)
- barrier_rejects_illegal_combo (構築経路の fail-loud) +
  uav_barrier subresource 引数ピン

### 検証結果 (全て実測)
- lib **778/778** (+5)。digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: enhanced_barriers 6→0 (全面改訂に伴い正準化)、
  full_graph_wiring 21→21 (2 行追加で増分なし)。all-targets check 0。

## BE. packed4.rs 監査 (wave 55, 2026-07-23)
### 基盤 PackedPullQuad 語彙の fail-loud 化 (raw/new 層分離)

基盤データ型 (8B/quad GPU pull 語彙)。消費: binary_greedy_meshing
(emit_pull_quad/リージョン再梱包)、full_graph_wiring (ao_refine)、
pull_mesh (VERTEX 係数)、render_pipeline 経由。

### BE-1 (高): pack 契約の debug_assert 限定 → 実データ入口 new() の assert 化
旧実装は pack_word0/pack_word1 の語彙域検査が debug_assert のみで、
**release ビルドでは超過値が隣接フィールドへ静寂ビット滲出** (GPU 幾何/
テクスチャ破壊) し得た。ただし初版で raw 関数に直接 assert したところ
**wide_static_bench E セクションが意図的語彙外入力 (rng.below(384) 等) で
パニック** — bench は「任意 bit 列の raw throughput 測定」という正当な
ハンマ用途であり、この赤は私の設計誤りを捕捉した。
→ 層分離で解決: raw 語彙関数 (pack_word0/pack_word1) は debug_assert の
デュアルモード (debug=開発時捕捉 / release=無検査スループット) として
残置し、**全実データ生成経路が通る `new()`** に検査を集約 (release でも
fail-loud)。実生成経路 3 件全て `new()` 経由であることを機械確認。
現行供給域も監査で確認 (実セクション 16³ → 座標 ≤15、tex はパレット
アトラス index、ao ≤3 clamp 済、face 機械的 exhaustive、w/h ≤64)。

### BE-2 (中): face_index の `_ => 5` 静寂誤分類 → 軸一意 assert
違法組合せ (複数軸/ゼロ軸) が **-Z (face 5) として静寂着地** (WGSL
face_normal の default も同値の一貫ゴミポリシ)。軸フラグちょうど 1 つを
assert。実生成経路は (Axis,bool) exhaustive match で常に合法 ✓。

### BE-3 (低): WGSL ミラー語彙の表記一致ピン
SHADER_VERTEX_PULL の bit 語彙 (COORD_MASK=63u/TEX_MASK=4095u/全 shift/
face_normal の 0:+X/5:-Z) をテキストピン — 片側のみ変更された場合の
機械検出。

### 雑録: 誤誘導定数 PULL_CHUNK_VOXELS=32 の撤去
実セクションは SECTION_SIZE=16 で、参照設計の語彙定数 32 と矛盾
(消費者ゼロ実測)。語彙 6bit (64 可) はリージョン再梱包経路の 0..64
フィルタと整合 — モジュール doc に規約を明文化。

### テスト (+2 純増)
- word0 厳密 bit 列ピン (0b11_000000000100_000011_000010_000001 独立導出)
- new 語彙超過拒否 ×8 + raw デュアルモード (debug 捕獲/release 構成可)
- face_index 6 面厳密ピン + 違法軸拒否 ×3
- WGSL 表記一致ピン ×12

### 検証結果 (全て実測)
- lib **782/782** (+4。テストの赤 2 回はいずれも私のテスト/設計誤りを
  捕捉: bench ハンマ用途の見落し、debug_assert のデュアルモード性)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (bench 入力列は全く同一のまま層分離のみ実施)。
- fmt 0→0 (正準化適用後)。all-targets check 0。

## BF. pull_mesh.rs 監査 (wave 56, 2026-07-23)
### PullBuiltMesh の空判定を単一真実源化 + draw カウント wrap 遮断

Vertex-pull 経路の CPU 側メッシュ保持型。消費: binary_greedy_meshing
(2 構築経路)、gpu_vertex_pull (draw 判定 + SSBO pool)、frame_pipeline
(フレーム draw 巡回)、render_pipeline (統計)、frame_hiz (AABB テスト)。

### BF-1 (高): pub `is_empty: bool` フィールド → `is_empty()` 導出メソッド化
旧設計は quads と独立した pub bool フィールドで、**非整合状態
(is_empty=true + quads 非空) を構築可能** にし、gpu_vertex_pull の
早期 return が**非空メッシュを静寂消失**させる一方向ハザードだった。
実害の証拠として render_pipeline.rs （旧） :414 に
`pull.is_empty = pull.quads.is_empty();` という**手動再同期行**が存在
(AO refine / material sort による quads 変化の**後**に自分で直す設計 —
同期漏れ経路が型で防げていなかった) 。消費側も `mesh.is_empty` (field)
が 6 箇所に散在。
→ フィールドを撤去し `is_empty() = quads.is_empty()` の導出メソッドに
単一真実源化。非整合状態は型上構築不能になり、render_pipeline の手動
再同期行は削除 (コメントで経緯を明記)。構築側 2 経路 (bgm) は ctor から
フィールドを除去、消費側 6 箇所 (gpu_vertex_pull ×3、frame_pipeline ×1、
frame_hiz テスト ×2、render_pipeline ×1) をメソッド呼出に更新。
**初回コンパイルが E0560/E0615 で旧フィールド参照 4 件を機械列挙** —
前セッション grep の捕捉漏れを型システムが完全列挙し、フィールド→
メソッド移行の機械的完全性が保証された。
なお BuiltChunkMesh (binary_greedy_meshing :1009/:1035、chunk_mesh、
lod_hybrid) は**別 struct** (同名フィールドを持つが本 wave の対象外、
将来の監査対象として記録)。

### BF-2 (中): pull_vertex_count の `as u32 * 6` 静寂 wrap → assert 化
`self.quads.len() as u32 * 6` は巨大 Vec (>715M quads) で **usize→u32
切捨てと ×6 の wrap が直列** で、draw カウントの静寂誤化を起こし得た
(実機では VRAM 規模上到達困難だが契約として)。`n <= u32::MAX/6` を
assert し fail-loud 化。

### テスト (+1 純増)
- is_empty_is_single_source_of_truth: 空構築/非空構築/直接 struct 構築の
  3 経路で is_empty() ≡ quads.is_empty() (非整合構築不能の型保証を
  消費側から確認)

### 検証結果 (全て実測)
- lib **783/783** (+1)。pull_mesh 系 7 件全緑。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (判定行の撤去は出力非影響)。
- fmt: pull_mesh/frame_hiz 0→0 (全適用)、bgm 1→1、gvp 1→1、
  frame_pipeline 0→0、render_pipeline 3→3 (HEAD 由来温存)。
- all-targets check 通過 (warning は既存由来のみ)。
- **インシデント**: git HEAD が 5 度目の base 巻戻りを起こし、確立手順
  (fetch → reset --mixed FETCH_HEAD) で復旧 — 差分が wave 56 3 ファイル
  のみであることを確認済。

## BG. svo.rs 監査 (wave 57, 2026-07-23)
### trace の意味論スタブ根治 (厳密最近接走査化) + 到達不能 dominant の構造根絶

Sparse Voxel Octree (far-LOD 語彙)。消費: frame_vct/frame_ddgi
(WGSL ミラー走査)、frame_reuse (SvoEncoding)、full_graph_wiring、
render_pipeline (:361 from_column)、voxel_cone_tracing。
WGSL (voxel_cone_tracing.wgsl/ddgi.wgsl 共有領域) は CPU sample_lod の
完全ミラーで健全 — 壊れていたのは `trace` のみで、しかも呼出側ゼロ。

### BG-1 (critical): `SparseVoxelOctree::trace` が意味論スタブだった
旧実装の検出問題 (全て実証):
1. **ray-AABB 交差判定が全く無い** — Branch では全 8 子に親の t 区間を
   無条件伝播するため、ray と交差しない占有葉でもヒットし得た
   (幾何から遠ざかる ray でも Some。テストは is_some() のみ検査で潜伏)。
2. `world` は **ray.origin 固定** (stack 初期化で t_enter=0 が全ノードに
   伝播) — ヒット位置情報が完全な嘘。
3. DFS **index 順**の最初の占有葉を返すのみで最近接保証なし。
4. `steps` は 0 固定。push 境界も `if sp < 63` の**静寂ドロップ**。
5. palette fallback も `world: ray.origin` の嘘で、DDA (外部原点を即座
   拒否する実装) が既存テスト経路では実質実行されない死に経路。

根治内容 (「スタブ無し・数学的に正しく」方針に基づく完全再実装):
- **slab 法** ray-AABB 交差を導入し子を関門 (非交差子は push しない)。
- 交差子を t_enter 昇順ソート→降順 push の順序付き DFS: 兄弟ボックスは
  互いに素、子孫区間は親区間に包含されるため、最初に到達する葉が
  **厳密に最近接**であることを構造証明 (コメント明記)。
- `world` = origin + dir × max(t_enter,0) の葉ボックス入射点。
- `steps` = 訪問ノード数 (fallback 時は + DDA ステップ)。
- 凍結軸 (カラム DAG 共有) の縮退子はノード id で重複除去。
- 非有限 ray (NaN/±∞) は入口で拒否 (crate 哲学統一: NaN=欠測は drop)。
- slab 内の 0×∞=NaN は「dir 成分 0 × 境界面一致」のみに発生することを
  場合分け証明し、その軸を全区間受理に正規化 (ソートに NaN が出ない
  ことを構造保証、partial_cmp expect は fail-loud ドキュメント)。
- スタック watermark ≤ 1+7×max_depth (≤43) を証明し assert で fail-loud 化
  (旧来の静寂 push ドロップ撤去)。
- palette fallback の DDA ヒットはボクセル [x,x+1)³ への slab 入射 t で
  world を**厳密復元** (入射側面 z=3.0 等を exact ピン)。

### BG-2 (中): 到達不能 dominant 葉 (+ id≥16 静寂消失ハザード) の構造根絶
両 builder の深度キャップ腕 (`depth >= MAX_DEPTH/self.max_depth ||
size<=1`) は**到達不能**と証明: size は 2 冪半減列を同期して辿るため
depth キャップ到達時に恒に 1x1x1、かつ 1 セルは直前の uniform 判定に
恒に捕捉される。到達不能ゆえ撤去しても**ツリー bit 同一**。
加えて dominant 実装はカウント配列 [u32; 16] で**ブロック id ≥16 を
静寂に対象外**とする潜在バグを抱えていた (全セル id≥16 の領域が
Empty 化し得た) — 腕ごと撤去して構造的に根絶。depth パラメータ・
dominant_block/dominant_block_column (2 fn) も撤去。

### BG-2b (低): from_column の静寂切捨てを fail-loud 化
旧実装は sections 5 本以上を min/take で**静寂切捨て** (実ボクセル消失)、
0 本を高さ 1 退化ツリーに**静寂着地**。契約 assert (1..=4 本) に根治
(live 供給: column_for_mesh=4 固定、demo/noise=4 固定を実測確認)。

### BG-3 (低): WGSL SVO 走査語彙の表記一致ピン
NODE_STRIDE=10u、tag 規則 (w0==0 / w0>=2)、子並び dz*4+dy*2+dx、
粗 LOD solid/8、ミラー注記の 6 表記を VCT_WGSL からピン
(共有領域の byte 同一性は frame_ddgi 側が既に assert)。

### テスト (+9 純増)
- 最近接ヒット + 入射点 world 厳密ピン (y=-1 → leaf y=0 入射、
  world=[8.5,0.0,8.5] / 空気層起点 → 背後葉棄却 → block2 @ y=8.0)
- 非交差/遠ざかり/空気柱 ray → None (旧 DFS では他列占有葉に化け得た)
- dir 成分 0 の退化軸 ray (x=0 入射面、±∞ で NaN 不発) 厳密ピン
- palette fallback 入射 t 復元 (world=[3.5,3.5,3.0]、steps=4 exact)
- 非有限 ray 拒否 ×3
- LCG 混合 (id=100 含有) 4096 voxel 全域 sample_lod ↔ palette 性質一致
  + GPU 語列に tag 2+100 保持 (id≥16 消失なし)
- from_column 契約 should_panic ×2 / WGSL 語彙ピン ×6

### 検証結果 (全て実測)
- lib **792/792** (+9)。svo 系 16 件全緑。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (撤去腕は到達不能でツリー bit 同一、wiring 出力非影響)。
- fmt 0→0 (HEAD 0 のため全適用後)。all-targets check 通過。

## BH. gpu_vertex_pull.rs 監査 (wave 58, 2026-07-23)
### 死に構造 3 件の撤去 (write-only 帳簿/未構築エンジン/ラッパ) + WGSL 契約ピン

Vertex pull 語彙の供給元 (SHADER_VERTEX_PULL/SHADER_MESH_SHADER/
FrameUniforms)。live 消費: frame_pipeline (実描画パス)、
frame_worldgen (meshlet カリング compute 実 dispatch)。

### BH-1 (高): PullSsboPool — write-only 帳簿 (+潜伏バグ 2 件) を撤去
消費実測: render_pipeline :113 (field) / :232 (profile ゲート init) /
:429 (`pool.upload_pull_mesh(&pull);` — **戻り slot を即破棄**) のみで、
`slots`/`generation`/`PullPoolSlot` 全フィールドに reader 皆無
(workspace 全域機械検索)。`adaptive()` は HW プローブ
(`AdaptivePerfEngine::hardware()`) まで実行する死に重さ。
さらに潜伏バグ 2 件を抱えていた:
1. **容量超過時の stale slot**: `qcount > capacity` で warn+None 返却するが
   旧 slot を残す → 消費側が存在すれば旧メッシュへの**静寂バージョン
   スキュー**になる設計 (empty 経路は remove するのに非対称)。
2. `mesh.quads.len() as u32` の暗黙切捨て (BF-2 同型)。
**代替案 (pooled ring SSBO の実 wiring) を検討したが棄却**: 真の ring は
frame 単位 fence が必須 (ring wrap で同一フレーム先行チャンクの SSBO
領域を上書き → submit 後の draw が破壊される) で、draw 経路
(frame_pipeline) の再設計 + sandbox で不可能な実機 GPU 検証を要する。
動作中の per-draw パスを壊すリスクに見合わず、BA-2 と同根拠
「消費ゼロかつ嘘を維持する構造は撤去」で撤去を選択 (根拠をモジュール
doc に記録)。

### BH-2 (高): GpuVertexPullEngine / PullEngineHandle — 構築ゼロの複製実装
live 描画は frame_pipeline が深度 (Depth32Float) + HDR で別建て実施
(frame_pipeline.rs :7,:245 が設計選択を記録)。`GpuVertexPullEngine`
(深度無しサーフェス直結・draw 毎に SSBO 新規生成) は workspace 全域で
**`new` の呼出箇所ゼロ**、そのラッパ `PullEngineHandle` も構築ゼロ。
実機検証不能な未使用 GPU コードの温存は「このパスが動く」という嘘の
維持のため撤去 (PullRenderPath/detect_path/glam_like_identity も一体)。
frame_pipeline の参照コメント 2 箇所は「旧 GpuVertexPullEngine —
wave 58 で撤去済み」と歴史注記に更新。

### BH-3 (記録): 生存側の確認
`vertex_pull_4byte` profile フラグは pull 経路選択 (:383,:624,:848) を
持つ live 語彙のため**温存**。SHADER 両定数と FrameUniforms は live 供給
のまま本モジュールを「語彙の単一供給元」として再定義。

### テスト (+3 純増)
- FrameUniforms 80B/align 4/B多倍数 + WGSL struct 表記ピン ×3
- vs_pull/fs_pull entry・@builtin(vertex_index) 絶対 index 語彙・
  binding 表記ピン ×3
- terrain_mesh_shader.wgsl が compute 実カーネルであることの表記ピン ×2
  (frame_worldgen CPU ミラー注記含む)

### 検証結果 (全て実測)
- lib **795/795** (+3)。撤去物の参照残存ゼロを全 crate grep で機械確認。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (死に構造のため出力非影響)。
- fmt: gpu_vertex_pull 0→0 (全面改訂・全適用)、render_pipeline 3→3
  (HEAD 由来温存)、frame_pipeline 0→0。all-targets check 通過。
- **インシデント**: Rust toolchain が再び消失 (4 度目)、restore-env.sh
  (34 秒) で復旧後に検証。

## BI. chunk_mesh.rs 監査 (wave 59, 2026-07-23)
### BuiltChunkMesh::is_empty 第二真実源の根絶 (BF-1 統一) + encode 非有限 fail-loud + demo 代表域修復

12B 量子化頂点 + BuiltChunkMesh (legacy indexed メッシュ語彙)。消費:
binary_greedy_meshing (構築)、mesh_cache (disk serialize)、frame_reuse、
leaf_fast_path (構築+merge)、render_pipeline (:191 builder, :481 呼出)、
lod_hybrid、vertex_pool、persistent_vbo_pool、rsift-launcher (builder 構築)。

### BI-A (高): `is_empty: bool` フィールド → メソッド化 (BF-1 と同型)
PullBuiltMesh (wave 56) と同じく、vertices と独立の pub フィールドが
**非整合状態を構築可能**にしていた。監査で見つかった実被害の痕跡:
1. lod_hybrid :73 — LOD 簡略化が vertices を置換した後に
   `mesh.is_empty = mesh.vertices.is_empty();` という**手動再同期行**
   (書き忘れれば静寂バグになる設計本身の欠陥の直物証)。
2. mesh_cache テスト — `m.is_empty = true` で「頂点非空だが空」の
   非整合を人為発生 (put_rejects_empty_mesh の入力作りに悪用)。
3. leaf_fast_path テストの `..overlay.clone()` スプレッドが
   vertices 空 + is_empty:false の非整合を暗黙生成。
→ フィールド撤去 + `is_empty()` (vertices 導出) 単一真実源化。
writer 13 箇所/reader 11 箇所/mutation 1 箇所 + コンパイラ追加列挙
8 箇所 (lod_hybrid:73,113、persistent_vbo_pool:137,382、
render_pipeline:367,488、vertex_pool:55,115) を機械移行
(grep 後追いではなくコンパイラの E0560/E0615 が完全性を保証する
移行パターン、wave 56 と同じ堅い手順)。

### BI-B (中): encode() の NaN→0 静寂テレポート遮断
`(x*1024.0) as u16` は NaN→0 飽和で、非有限位置が**メッシュ原点への
静寂テレポート** (幾何破壊) になっていた。全 8 入力 (pos/normal/uv)
に有限 assert を追加 (BC-2 同型の fail-loud 化。実生成経路は voxel
座標ベースで全て有限 — 3511 頂点相当の bench 影響なし)。
範囲超過 (有限) の飽和量子化は現契約どおり維持 (doc 明記)。

### BI-C (中): demo フォールバックの表現範囲違反を修復
MultithreadedChunkBuilder の非-BGM フォールバック (Minimal tier /
Low cpu_cores<4 で到達可能、render_pipeline :481 から呼出) が
y ≤ 100−step の 100 面を生成し、頂点表現範囲 [0,64) 超過分が飽和
量子化で **y≈64 の 1 面に全頂点が重なる垃圾**になっていた。
純粋生成器 fallback_plane_mesh に抽出し y ∈ [0,16) に厳密限定
(SECTION_SIZE 原像) + step 全型 (1/2/4) で高さ一意性ピン。

### BI-D (低): ドキュメント誠実化
- 冒頭「異次元のレンダリング速度」「60% 以上削減」→ 算術記述
  (12B vs 28–32B = 57.1–62.5%、速度効果はボトルネック依存で数値主証せず)。
- `uv_half` の「UNORM16」→ scale 32767 (実効 15bit) 明記 (旧称誤記)。
  デコード消費者が WGSL/他モジュールに存在しない (フィールドは
  mesh_cache の byte serialize 往復のみ) — 語彙値は 32767 で固定・ピン済。
  テスト名 uv_unorm16_endpoints_exact → uv_scale_32767_endpoints_exact。

### 記録 (対応見送りではなく判断): TOTAL_CHUNKS/TOTAL_VERTICES
reader ゼロの vanity telemetry だが嘘ではなく原子カウンタの実コスト
のみ — 撤去価値が churn を下回るため温存 (監査で分類のみ記録)。

### テスト (+5 純増)
- encode 非有限拒否 should_panic ×3 (位置/法線/UV)
- is_empty 単一真実源ピン
- fallback_plane_mesh 表現範囲 (y<16·1024) + 高さ一意性 ×3 step

### 検証結果 (全て実測)
- lib **800/800** (+5)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: chunk_mesh 4→3、bgm 1→1、mesh_cache 0→0 (新規 hunk 修正後)、
  frame_reuse 2→1、render_pipeline 3→3、lod_hybrid 1→1、leaf_fast_path
  0→0、persistent_vbo_pool 6→6、vertex_pool 1→1 (全て HEAD 以下)。
- all-targets check 通過。

## BJ. voxel_cone_tracing.rs 監査 (wave 60, 2026-07-23)
### ConeRay 契約 fail-loud (+∞ max_dist 発散ループ遮断) + 厳密蓄積ピン確立

Diffuse GI コーントレーシング (CPU)。消費: full_graph_wiring :894
(camera 由来 ConeRay 実走査)、frame_vct (lod_from_diameter 一元化、
VCT_WGSL dispatch)。WGSL (voxel_cone_tracing.wgsl main) とのミラーは
ループ条件・f32 演算順まで照合し**真に一致** (under-blend 式
`C += (1−α)aC`, `α += (1−α)a`、dist=0.5、diameter=max(2a·dist,1)、
step=diameter·0.75、閾 0.99/0.001 の全て一致確認)。

### BJ-1 (高): ConeRay 非有限で発散/静寂黒出力 → validate_contract 化
- `max_dist = +∞`: dist が step ≥ +0.75 で伸びても ∞ に到達しないため
  **while 発散** (空 SVO なら CPU 永遠ループ、WGSL 側供給なら
  **GPU ハング = device loss** の実害クラス)。
- NaN origin/dir/aperture: sample 拒否で「静寂に黒が返る」だけで発見不能。
- 負 aperture: cone が負に広がり (diameter=max(負,1)=1 で常時)
  意味論のない退化 ray サンプリングに静寂着地。
→ `ConeRay::validate_contract()` を設置し trace_diffuse_cone 入口で強制
 (全成分有限 + aperture ≥ 0 + max_dist 有限かつ ≥ 0)。live producer
 (wiring: aperture 0.577/max_dist 32.0) は契約適合を実測確認。

### BJ-2 (低): WGSL ミラー語彙ピン
dist 初期値/ループ条件否定形/diameter 式/weight 式/step 式/ミラー注記
の 6 表記を VCT_WGSL からピン。

### 副次確認 (誠実記録): alpha>0.001 ガードは CPU では Option 後の
冗長条件 (実 alpha ∈ {1/8..=1} で常真) だが WGSL では NO_HIT (w=−1)
の実条件 — ミラー対称のため保持を doc 明記。

### テスト (+6 純増、既存 1 件を範囲検査→厳密値に強化)
- lod_from_diameter 2 冪境界 ±1ulp 厳密ピン (k=1..=9 全網羅) +
  NaN/0/負→0、1024 以上・+∞→clamp 10
- 全面 solid → (0.5, 1.0) 厳密 (weight 1 で即飽和)
- 下半分 solid + aperture 16 → weight 系列 0.5, 0.25 の厳密 dyadic 蓄積
  (C=0.375, α=0.75) を独立手導出ピン (dist 系列 0.5→12.5→312.5 終了
  まで全演算 f32 厳密であることを明白性コメントで担保)
- 契約 should_panic ×3 (NaN origin / +∞ max_dist / 負 aperture)

### 検証結果 (全て実測)
- lib **807/807** (+7)。voxel_cone_tracing 系 8 件全緑。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt 0→0 (HEAD 0 のため全適用後)。all-targets check 通過。

## BK. section_rle.rs 監査 (wave 61, 2026-07-23)
### 破損 RLE の静寂 air 注入を fail-loud 化 + write-only 語彙撤去 + cache 破損検証配線

16³ パレット RLE + 層占有語彙。消費: binary_greedy_meshing (RleSection/
layer_masks_from_palette)、frame_reuse (rle/occupied 保持、:343 実消費)、
mesh_cache (disk serialize)、chunk_cull (verdict_column)、render_pipeline
(prepare_column の encode/decode)。

### BK-1 (高): decode の count 総和超過/不足を fail-loud 化
旧実装は `end > VOLUME → break` の**静寂切捨て** + 不足時は末尾 0 初期化
のまま返却で、破損 wire や手組み RleSection が**空気ボクセルを静寂
注入**し得た (ワールド消失バグ種)。decode 入口で Σcount == VOLUME (=4096)
を assert に変更。encode() 経由の RLE は全要素走査から Σ=4096 が不変
条件であることを証明し doc 明記 (count: u16 で最大 run 4096 < 65535 の
余地証明も)。

### BK-2 (高): from_bytes に厳格契約 (長さ完全一致 + Σcount 検査)
旧実装は (a) 末端ゴミ黙認、(b) Σcount 未検査で破損 wire を受理 →
BK-1 の被害源。from_bytes は両者を拒否 (None) する厳格語彙に強化。
検証可能になったことで **mesh_cache の rle ペイロードを実検証に配線**
(旧来は長さだけ見てスキップする write-only 領域 — 破損 rle 混じりの
エントリを拒否→再構築へ倒す実利得)。

### BK-3 (中): 消費ゼロ語彙 2 件撤去
- `encode_row_mask_rle`: workspace 全域に呼出・デコーダ皆無の
  write-only wire (BA-2 同根拠)。
- `RleSection::layer_occupancy`: 消費者ゼロ (層 skip は palette 版が担う)。
- chunk_cull :62-66: visgraph_enabled 時に occupied_section_indices を
  **計算して即破棄**する死に計算を撤去 (cull 熱経路の純粋無駄。
  「隣接無しに全チャンクを occluded 判定しない」設計は正しく温存)。
- doc 冒頭の「10–50×」→ 最悪ケース ~2× 膨張を明記した誠実版に訂正。

### BK-4 (記録): `required_section_indices` ×8 語彙
frame_reuse 内で等価比較のみに消費される閉じた語彙であることを
テストにピンで明文化 (OccupiedVisGraph ノード ID 名前空間結合)。

### テスト (+6 新規、−3 撤去 = 純増 +3)
- decode Σ 超過/不足 should_panic ×2
- from_bytes: 末尾ゴミ拒否、Σ=4095 拒否 (wire バイト改竄)、Σ=4097 拒否
- encode 不変条件ピン (1 run=4096、Σ=4096)
- occupied_section_indices ×8 語彙ピン
- mesh_cache: 破損 rle エントリ拒否 (zstd 往復 + wire offset 厳密導出)
- 既存 is_solid/layer_masks/roundtrip 系は緑維持

### 検証結果 (全て実測)
- lib **810/810** (+3)。section_rle 11 / mesh_cache 10 / chunk_cull 8 全緑。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: section_rle 0→0 (全適用後)、chunk_cull/mesh_cache 0→0。
- all-targets check 通過。
- **インシデント**: Rust toolchain 5 度目の消失 → restore-env.sh で復旧。
- **備考**: 赤 2 件は私のテストバグ (encode_mesh 引数数、air move) を
  コンパイラが捕捉 — 既知の自己誤り検出パターン。

## BL. noise_upsample.rs 監査 (wave 62, 2026-07-23)
### live worldgen のゼロデイ根治: Perlin が全 voxel 定数 0.5 を返していた

3D ノイズアップサンプル worldgen (粗格子 + 三線形補間)。消費:
render_pipeline (:306: live worldstore 不在時の fallback 生成、
profile.noise_upsampling=true — 上位 2 tier) 、frame_worldgen
(補間形ミラーのみ参照)、benchmark_upsample (telemetry)。

### BL-1 (critical — ゼロデイ): perlin3d_dense の定数化
旧実装は格子座標 `wx & 255` + のこり座標 `(wx as f32).fract().abs()`
で評価していたが、全呼出は**整数 voxel** のため fract ≡ 0 → fade ≡ 0
→ 補間項が全消滅し `(dot3(g000,0,0,0)+1)·0.5` = **0.5 を全 voxel で
返却していた**。影響: density=surface=洞窟項の全てが定数化し、
「noise 地形」は x/z に一切変化の無い高さ方向縞模様 (flat strata)
になっていた — モジュールの存在意義全体が偽だった。
Perlin 勾配ノイズは数学的に整数格子点で恒に 0 となるため、周波数
スケール (PERLIN_FREQ = 1/8 voxel⁻¹) を導入し**格子内実数座標**で
評価する根治を実施 (px=wx·FREQ、x0=floor、xf∈[0,1))。負座標でも
floor 定義より正しく動作。出力レンジ [0,1]・シード決定性は維持。
なお格子点 (FREQ 整数倍) で正確に 0.5 となる数学的性質は保存され、
回帰ピンの厳密検査点として利用した。

### BL-3 (中): benchmark_upsample の粗サンプル計数が実装と不整合
実グリッドは範囲両端含む ((span)/stride)+1、span = 15/63/15 で
4×16×4=256。旧式は (16/stride)+1 等で **5×17×5=425 を報告**
(1.66× の見せかけ過大 → speedup の分母分子誤飾)。実装同式に修復し
厳密値ピン (dense=16384 / coarse=256)。

### BL-2 (低): density_to_block の到達不能 `.max(1)` 撤去
wy ≥ 0 では wy%3+1 ∈ {1,2,3} で下限防御は恒到達不能 (証明・明記)。

### 判定記録: digest 非影響の検証
wide_static_bench の "noise" パターンは独自生成器で本モジュール非消費、
bench も当該 fallback 経路を踏まないため structural_digest
`004c1cf5fb17bfe8` rows=357 不変 (worldgen 出力変更は bench 構造に
波及しないことを実測確認)。

### テスト (+4 純増、全て BL-1 の直接的回帰ピン)
- perlin_varies_between_voxels: x 走査で複数 distinct 値 + レンジ +
  同一入力同一 bit の決定性
- perlin_lattice_points_are_exactly_half: 格子 4 点で正確に 0.5
  (数学的性質) + 非格子点で非 0.5 (to_bits)
- upsampled_column_varies_along_x: z 固定 16 列の y プロファイルが
  2 種以上 (旧 flat strata 化の直接回帰) + パレット bit 決定性
- benchmark_counts_match_actual_grid: dense=16384 / coarse=256 厳密

### 検証結果 (全て実測)
- lib **814/814** (+4)。noise_upsample 系 6 件全緑。
- digest 上記のとおり不変。fmt: HEAD 3 → WORK 3 (HEAD 由来温存)。
- all-targets check 通過。

## BM. frame_reference.rs 監査 (wave 63, 2026-07-23)

GPU/CPU 相互検証の CPU 参照ラスタライザ (約 900 行)。消費: なし (src 内)、
**examples frame_proof.rs / frame_proof_extra.rs が render_reference /
render_reference_fsr / write_bmp / hiz 参照 2 fn を実利用** (BMP 実画像出力
とオクルージョン実証)。lib テスト内からの消費多数。参照先: packed4 /
pull_mesh / binary_greedy_meshing / frame_pipeline (build_view_proj, mul_v4)
/ occlusion_query (QueryBox) / frame_hiz (HIZ_DIM, aabb_from_mesh)。
方針メモ (ユーザー指示 2026-07-23): 「消費者ゼロ」を削除理由にしない —
系統価値が上がるなら消費者の追加配線を先に検討する。本モジュールは
examples 消費があり存置は自明だが、以降の wave でも同判定手順を適用。

### BM-1 (中-高): covered_px / avg_lum の doc 二重乖離を根源修正
- **(a) covered_px の過大計上**: 旧実装は raster_tri で「深度テスト通過の
  たび」に計上していたため、同一ピクセルへの重複深度勝利 (手前ジオメトリ
  による上書き) が 2 重計上された。doc「被覆ピクセル数」と乖離。
  初回被覆 (depth == 1.0 sentinel、書き込みは狭義単調減少ゆえ厳密同値) の
  み計上へ修正し、不変条件
  `covered_px == depth.iter().filter(|&&d| d < 1.0).count()` を doc 明文化。
- **(b) avg_lum の sky 混入**: 旧実装の最終ループは color 全ピクセル
  (sky=ACES 後空色を含む 3 万〜16 万画素) の輝度を加算し covered でのみ
  除算していた。被覆が疎なフレームでは sky 数千画素分が分子に混入し
  「被覆ピクセルの平均輝度」を激しく誇飾。depth[i] < 1.0 マスクで被覆
  ピクセルのみ集計へ修正 (マスクと (a) の計数は同一不変条件で閉じ、
  分子分母が同一集合で一致)。GPU 側は frame_pipeline depth_compare=Less・
  クリア 1.0 で z==1.0 は観測不能 (境界明記済)。
- **定量的実害の実測** (アドバーサリアル検証): 旧セマンティクスを一時
  注入すると重なりシーン (壁 2 枚、手前 4 ずらし) で covered_px =
  214652 vs 真の被覆 106560 (約 2.01 倍誇飾)、demo シーン (128²) でも
  3142 vs マスク数で乖離 → 新テスト 2 件が確実に赤になる検出力を実測
  確認後に本源コードを復元 (両テスト緑)。
- 備考: render_reference_fsr 側の被覆統計は「ACES 後の sky 色と異なる
  画素」を全解像度側で計測する自成り規則で、本修正と独立に整合
  (BM-1 と挙動変更なし)。

### BM-2 (低): ACES+sRGB の WGSL ミラー表記ピンを新設
aces_tonemap.wgsl 本文の実測確認 (係数行
`let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;`、
`pow(max(x, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2))`、exposure 乗算位置、
全画面三角形写像 `i32(vid / 2u) * 4 - 1` / `i32(vid % 2u) * 4 - 1`) に
対し include_str! 経由の 8 トークンピンを追加。WGSL 側だけ係数を変更
した退行を fail-loud 化。

### BM-3 (低): write_bmp の全バイト厳密ピンを新設
54B ヘッダ (bfSize / オフセット / DIB40 / 幅 LE / **高さ負値=top-down** /
planes=1 / bpp=24 / BI_RGB / img_size) + BGR 行 + 4B アライン padding +
行順序 (y=0 先頭) を、2x1 (pad=2) と 1x2 (pad=1) の 2 系統 62+62 バイトを
手導出値と厳密照合。alpha 非出力も両値 (0x00/0xFF) で確認。
備考 (自己誤りの捕捉実績): 初稿テスト値の 1x2 bfSize/img_size を誤記
(58/4) していたが脳内再検算 (3+1)×2=8 / 54+8=62 で捕捉訂正 — wave 46/52/
61 以来の「テスト値は必ず手続き的に導出する」規律の実例。

### BM-4 (低): fsr1.wgsl 三連鎖の WGSL 側表記ピンを新設
EASU (R のみ勾配 2 式 / `gx / (gx + 0.5)` / `f.x + (0.5 - f.x) * ex` 等 6
トークン) + RCAS (`(n + s + e + w) * 0.25 - c` / 鮮鋭化符号 `c - lap *
sharp`) をピン。ピクセル挙動の厳密ピンは既存 (flat/valley/edge 3 件) が
担任。fsr1.rs / cas.rs 側のミラーピンは各モジュール監査 wave の担任として
重複ピンを回避 (今後の wave で実施)。

### 健全性確認 (変更なし)
- raster_tri: area 符号除算の両回り対応重心座標、画面線形 z (wave 17 訂正
  由来、GPU 固定機能と同規則)、z NaN は `z < depth[idx]` 不成立で正規拒否。
- quad 棄却 `!(w > 1e-5)` は NaN w 正規拒否、near 後方の保守的棄却。
- hiz_downsample_reference の max 集約・hiz_test_reference の 8 角射影・
  保守的可視 (any_invalid/offscreen → coverage=1) を WGSL 対照で再確認。
- SUN_DIR f32 再導出テスト (wave 21) 含む既存 12 テスト全緑。

### テスト (+4 純増)
- coverage_and_luminance_stay_mask_consistent_under_depth_overlap:
  壁 2 枚の深度オーバーラップ下で covered==マスク厳密一致・輝度独立
  再集計 (u8 量子化誤差解析 ≤0.5/255 に対し 1/255 余裕) を検証。
  ヘルパ assert_mask_consistent を既存 reference_frame_has_real_coverage
  _and_shading にも配線。
- wgsl_aces_mirror_constants_and_fullscreen_triangle (BM-2)
- write_bmp_emits_exact_byte_layout (BM-3)
- wgsl_fsr1_mirror_lexical_tokens (BM-4)

### 検証結果 (全て実測)
- lib **818/818** (+4)。frame_reference 16 件全緑。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: HEAD 0 hunk → 自前 2 hunk 発生のため rustfmt 全適用 → 0 hunk。
- all-targets check 通過。zero-width 文字 0。
- frame_proof example (release) を実実行: exit=0、新セマンティクスで
  covered_px 29925/307200, avg_lum 0.3656 (妥当値)、全 assert 通過。
  出力 BMP は作業ツリーから除去。
- **インシデント**: Rust toolchain 6 度目の消失 (restore-env.sh で復旧)、
  git 破損 8 度目 (HEAD→base 巻戻り、fetch + reset --mixed で復旧、
  差分は frame_reference.rs 1 件のみであることを確認)。

## BN. fsr1.rs 監査 (wave 64, 2026-07-23)

FSR 1.0 (EASU + RCAS) CPU 参照実装 (189 → 約330 行)。消費: full_graph_wiring
(struct 保持 :163、reconstruct 呼出 :1634) / frame_fsr1 (FSR1_WGSL 参照、
DEFAULT_SHARPNESS との対応) / GPU 実パス GpuFsr1Pass (wgpu dispatch)。
live GPU チェーンは frame_fsr1 が担任 (wave 13 実測配線済)、CPU 参照面が
本モジュール。3連鎖: fsr1.wgsl (実シェーダ) ↔ fsr1.rs (参照) ↔
frame_reference::fsr1_reference (GPU/CPU 突合ミラー、BM-4 で WGSL 語彙
ピン済)。

### BN-2 (低-中): easu_reconstruct の死引数 `_c` 撤去 (API 正直化)
旧シグネチャ第 1 引数 `c` (中心画素) は WGSL が 2x2 ブロック 4 サンプル
(p00/p10/p01/p11) のみ参照する規約と無関係な未使用引数であり、呼出側に
「中心画素が意味を持つ」との誤認を与えていた。実害痕跡: full_graph_wiring
:1634 は誤認通りに中心っぽい値 `[aa.r, aa.g, aa.b]` を第 1 引数に供給して
いた (数学的影響はゼロ — 死引数ゆえ)。引数撤去 + wiring 呼出更新で
「渡せば意味がある」の誤認可能性を型で根絶。digest 不変 (calldata 同一)。

### BN-3a (低): Fsr1.sharpness の死に状態を解消 (消費者追加方針の適用)
`Fsr1 { sharpness: 0.2 }` と初期化しても CPU 構造体のどのメソッドからも
sharpness が消費されない死に状態だった (EASU は強度パラメータを持たない
WGSL 規約。GPU パス側が別系統で uniform 保持)。**新方針「消費者ゼロ削除
より消費者追加」に従い** `Fsr1::sharpen` (rcas に self.sharpness を適用)
を追加配線 — CPU でも EASU+RCAS 完全 2 パスが API 上成立。あわせて
wiring :308 を `Fsr1::default()` に集約 (0.2 リテラルの二重真実源を単一化)
し `frame_fsr1::DEFAULT_SHARPNESS` とのドリフトをテストで機械固定。
full_graph_wiring の数学は不変更 (digest 不変)、同ファイル本監査は
大物 wave で実施予定。

### BN-4 (低 — 規格確認、コード変更なし): WGSL `mix` の演算順ドリフトを
### 一次情報で確定し doc 誠実化
W3C WGSL (main ブランチ index.bs 直接取得) で一次確認:
- `mix` の定義は「linear blend (e.g. `e1 * (T(1) - e3) + e2 * e3`)」の
  **例示定義** (規範的演算順ではない)。
- 精度表は「Inherited from `x * (1.0 - z) + y * z`」。
- gpuweb/gpuweb#3260: Vulkan CTS 同等物が差分形 `x + (y - x) * z` を許容。
よって GPU バックエンド間で mix の演算順は非一意であり、CPU ミラー
(差分形) とのバイリニア部ドリフトは規格上起こり得る (±1-2 ulp、u8 の
±1LSB 統計許容内 — frame_reference doc と同一結論)。勾配・エッジ寄せ・
RCAS 部は演算順が全実装で厳密一致することを再確認済。module doc に
一次情報引用付きで明記。

### BN-3 (低): 厳密ビットピン強化 (+独立導出の自己誤り捕捉実績)
- easu_exact_bits_canonical: 全経路 (勾配→応答→位置寄せ→3ch バイリニア)
  の f32 エミュレーション独立導出ビット値固定。
- rcas_clamp_bounds_are_exact: 上下両方向クランプ (1.0 / +0.0 丁度)。
- rcas_exact_bits_non_clamped_and_channel_independent: 非クランプ域ビット
  + チャンネル配置差によるクロスチャンネル混入検出力の確保。
**自己誤り捕捉実績**: 初稿 RCAS 期待値が 1 ulp ずれ (0x3f733333 vs 実機
0x3f733334)。原因は Python エミュレーションが**入力リテラルを f32 に
丸めず f64 で逐次演算**していた導出バグ (W-3 規律の適用ミス) — 赤が
自己誤りを正しく捕捉した (wave 46/52/61 と同型パターン、BM-3 に続き 2
連続)。入力を f32 丸めした再導出で Rust 実機値と全 5 値一致を確認
(EASU 値は入力が正確表現可能領域で不変、RCAS 2 箇所のみ訂正)。

### テスト (+5 純増、823 全緑)
easu_exact_bits_canonical / rcas_clamp_bounds_are_exact /
rcas_exact_bits_non_clamped_and_channel_independent /
sharpen_consumes_struct_sharpness (sharpness=0 恒等 + rcas 一致 +
非ゼロ効果の死に状態回帰遮断) / default_sharpness_matches_gpu_pass_default。

### 検証結果 (全て実測)
- lib **823/823** (+5)。fsr1 系 11 件全緑。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (BN-2/BN-3a は calldata 同一の API 正直化のみ)。
- fmt: fsr1 HEAD=2 → WORK=2 (既存行シフトのみ、新規 0)、
  full_graph_wiring HEAD=21 → WORK=21 (HEAD 由来温存)。
- **fmt 運用インシデント**: /tmp/headcheck が git 破損復旧で陳腐化し
  「HEAD=0」の幻影を表示 → hunk 位置検証で発見、wave 毎再作成規律の
  必要性を再確認 (worktree 再作成で正規 baseline 取得)。
- all-targets check 通過。zero-width 0。
- CI: wave 63 run (71afbf5) **success**。wave 62 run (0e786ec) failure は
  同「lib tests」step で wave 63 側 (818 件の上位互換スイート) が全緑のため
  wave 50 と同型の flaky/infra 確定 (log 取得は sandbox から results
  receiver への接続が EOF 遮断で不可、判定は後続 green 連鎖)。

## BO. cas.rs 監査 (wave 65, 2026-07-23)

FidelityFX CAS (Contrast Adaptive Sharpening) CPU 参照 (276 → 約380 行)。
消費: full_graph_wiring (:1615 状態連鎖) / frame_postfx (cas_run_cpu 精密
ミラー・GpuCas 実 dispatch・CAS_WGSL 参照) / drs (CAS_WGSL 参照) /
fsr1,frame_reference (符号規約参照)。3連鎖: cas.rs ↔ frame_postfx::cas_run_cpu
↔ shaders/cas.wgsl。モジュールは 2026-07-21 監査で公式式へ転換済の
改修品。

### BO-1 (中): cas.wgsl の加算順 1 ulp 分岐 — 3連鎖演算順を真に統一
WGSL 最終合成が `(c + (n + w + e + s) * wg) * rcp_w` で、Rust 両ミラーの
`(n + s + e + w)` と**加算順が不一致**だった。IEEE 加算は非結合 (非可換な
丸め) のため最大 1 ulp 分岐し、WGSL ヘッダの「CPU ミラー (完全一致) …
演算順も同一」の主張が偽だった。WGSL 側を Rust 順に統一 (2 サイト一致の
方式へ 1 サイトを寄せる修復)。min/max 木は厳密演算のため順序非依存で
無害と証明 (変更不要)。GPU 出力への影響は ±1 ulp 以下、sandbox では
GPU 非実行・digest 非消費 (bench は WGSL ソースを行項目に含まないことを
grep で実測確認) のため全側面安全。残差: 公式 CasFilter は近傍項毎に
`b*w+d*w+...` と乗算先出しだが、実数上等価・プロジェクト 3 連鎖の内部
規約として sum-first を採用済 (doc 記録)。

### BO-4 (規格照合 — 変更なし): 公式 ffx_cas.h と全項目一致を一次情報確認
GPUOpen-Effects/FidelityFX-CAS ffx-cas/ffx_cas.h を直接取得し照合:
- CasSetup `sharp=-ARcpF1(ALerpF1(8.0,5.0,ASatF1(sharpness)))` (:389)
  ⇔ cas_peak と完全同一。**sharpness 意味論**: 公式コメント「0 := default
  (lower ringing), 1 := maximum (higest ringing)」(:378) — モジュール doc
  「1 で最大鮮鋭化」は正しく、**誤った逆転ではなかった** (注意喚起して
  いた先入観を一次情報で否定確認)。
- CasFilter noScaling: mn/mx は d,e,f(中心含む)+b,h の cross+中心 (:453-466)
  ✓、CAS_BETTER_DIAGONALS 無しの `1.0-mx` ✓ (:490-492)、sqrt(amp) ✓、
  w=amp*peak ✓、rcpWeight=1/(1+4w) ✓、負ローブ合成 ✓。
- 公式の rcp/sqrt は近似命令 (APrxLoRcpF1/APrxLoSqrtF1) 依存で、本実装の
  IEEE 厳密演算選択と MX_FLOOR ガードは doc 記載通り妥当 (WGSL 側コメント
  の精度注記と整合)。

### BO-2/BO-3 (低): 厳密ピン強化
- cas_peak_exact_bits_and_saturation: 0xbe000000 (-1/8) / 0xbe1d89d9
  (-1/6.5) / 0xbe4ccccd (-0.2f32) + 区間外飽和の端点ビット等価。
- cas_sample_exact_bits_canonical: チャンネル非対称入力で全経路の
  f32 エミュレーション独立導出ビット固定 (ch2 はクランプ上限と共存)。
- wgsl_mirror_lexical_tokens: peak 変換・MX_FLOOR・amp・1+4w・そして
  **加算順 `(c + (n + s + e + w) * wg)` の順序まで契約として表記ピン
  (BO-1 直接回帰)** — 7 トークン。

### 判定記録: 修正対象外 (一次情報で正当性確認済)
- `1+4w` の分母下限: peak ≥ -1/5 より 1+4w ≥ 0.2 > 0 で零除算不可 ✓。
- Mn/mx 木の順序差 (min/max は結合・可換・厳密) ✓ 無害。
- 公式の近似 rcp との差: CPU/WGSL 双方 IEEE 厳密路線は意図的選択
  (cas.wgsl 精度注記)。

### テスト (+3 純増、826 全緑)
cas_peak_exact_bits_and_saturation / cas_sample_exact_bits_canonical /
wgsl_mirror_lexical_tokens。既存 frame_postfx::cas_matches_module_fn_bitwise
含む CAS 系全緑。

### 検証結果 (全て実測)
- lib **826/826** (+3)。wide_static_bench structural_digest
  `004c1cf5fb17bfe8` rows=357 不変 (WGSL 変更は digest 行項目に非含有)。
- fmt: cas.rs HEAD=2 → WORK=2、hunk 位置完全一致 (25, 212 — HEAD 由来温存)。

## BP. aces_tonemap.rs 監査 (wave 66, 2026-07-23)

ACES Filmic (Narkowicz 2015 近似) トーンマッパ (139 → 約220 行)。消費:
frame_pipeline (:261 post パス ACES_WGSL + exposure uniform :330、検証対象
一覧 :660) / full_graph_wiring (:165 状態連鎖) / frame_reference (BM-2 で
WGSL 語彙ピン担任済) / lib.rs export。3連鎖: aces_tonemap.rs ↔
frame_reference::aces_srgb ↔ aces_tonemap.wgsl (GPU 実パス)。

### BP-2 (一次情報照合 — 変更なし): Narkowicz 原著と完全一致を確認
原著ブログ (knarkowicz.wordpress.com 2016/01/06) を fetch_page で直接取得:
- HLSL `saturate((x*(a*x+b))/(x*(c*x+d)+e))`、係数 a=2.51, b=0.03, c=2.43,
  d=0.59, e=0.14 — **式構造・係数とも本実装と完全一致**。
- 著者用法注記「exposure はトーンマップ前乗算、gamma は後」— 本モジュール
  の tonemap_display 構造と同順 (frame_pipeline の post パスも同順)。
- 「1 on input maps to ~0.8」⇔ 実値 aces(1.0)=0.8038 (既存コメントの数値
  を独立導出で確認)。著者明示の限界 (luminance only fit・ブライト過飽和)
  を module doc に誠実転記。

### BP-1 (低): 厳密ビットピン新設 (従来は単調性・範囲の弱 assert のみ)
- aces_channel_exact_bits_canonical: 0.0/0.5/1.0/2.0/5.0 の 5 点を
  f32 エミュレーション独立導出ビットで固定 (乗除加算のみで全環境決定的)。
- exposure_multiplies_before_curve: exposure=2.0 で 0.5 → aces(1.0) と
  ビット一致 (**前乗算位置の厳密検証**)。
- srgb_encode_midpoint_within_1ulp_class_tolerance: powf は libm 依存
  (±1ulp 実装間差) のため bit ではなく 1e-6 許容ピン — 方針を doc 明記。
  契約外 (>1.0/負) の Rust 側飽和挙動も厳密固定。

### BP-3 (低 — doc 誠実化): 負入力は saturate せず wrap する数学的性格
初稿テスト「負入力は厳密 +0.0」は x=-1.0/-30.0 で**偽** (分子が再び正と
なり正リターンに wrap、2 点とも clamp で 1.0) — Python 実機検算で即捕捉
(自己誤り捕捉装置 3 wave 連続稼働)。正確な領域分岐を固定:
0 ≥ x > -b/a (≈-0.01195) は +0.0、x < -b/a は正 wrap。分母は判別式
0.59²-4·2.43·0.14 < 0 より実数全域で厳に正 (0 除算 NaN 不可達) と証明。

### 3連鎖差分の文書化 (BO-1 同型の未然残り火を全点検)
- WGSL `pow(max(x,0),1/2.2)` (上限飽和無し) vs Rust `linear_to_srgb`
  (x≥1 → 1.0 飽和)。実パスでは aces 出力 (≤1.0 clamp 済) のみ到達し
  両者一致。差は契約外直接呼出のみ — module doc に明記 (frame_reference
  の aces_srgb は WGSL 形 (max のみ)、本モジュールは飽和形で並走)。
- WGSL `hdr * u.exposure` (フレームパス uniform) ⇔ Rust `c * self.exposure`
  — 位置一致確認。

### テスト (+4 純増、830 全緑)
aces_channel_exact_bits_canonical / exposure_multiplies_before_curve /
aces_channel_negative_input_behavior_is_deterministic /
srgb_encode_midpoint_within_1ulp_class_tolerance。

### 検証結果 (全て実測)
- lib **830/830** (+4)。structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: HEAD baseline 0 hunk (git 破損 #9 復旧後に再測定 — 前回測定値
  HEAD=1 WORK=1 は toolchain 消失過程の不完全実行と判明し破棄)、
  自前 1 hunk → rustfmt 全適用 0 hunk。
- **インシデント**: Rust toolchain 7 度目の消失 (restore-env.sh 復旧)、
  git 破損 9 度目 (HEAD→base 巻戻り、fetch+reset mixed で復旧、差分
  aces_tonemap.rs 1 件のみ確認)。
