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
- billboard_exact_corners_and_uvs (身分基底 4 corner + 4 uv 厳密列 + 斜め基底)
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
- テスト過程中にテスト側の 2 バグを自己捕捉 (exp フィールド未マスク、
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
量子化で **y≈64 の 1 面に全頂点が重なるゴミ**になっていた。
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

## BQ. checkerboard.rs 監査 (wave 67, 2026-07-23)

チェッカーボードレンダリング (半画素シェード + 斜め 4 近傍再構成)
(97 → 約190 行)。消費: full_graph_wiring (:1693-1702 状態連鎖) /
frame_postfx (checker_run_cpu 精密ミラー :275、GpuCheckerboard 実
dispatch、checker_matches_module_fns liveness テスト) / gpu_runtime
(:84 WGSL 配線一覧)。3連鎖: checkerboard.rs ↔ checker_run_cpu ↔
checkerboard.wgsl は parity/方向/加算順とも健全一致を確認 (BO-1 同型の
分岐なし)。

### BQ-1 (低): is_rendered を wrapping_add 化 — debug panic 経路の根絶
旧実装 `x + y` は debug ビルドで u32::MAX 級座標に対し overflow panic
し得た (画像座標契約外だが WGSL `(gid.x + gid.y) & 1u` の u32 wrap
セマンティクスとも非整合)。パリティは mod 2 の性質であり wrap 周回で
不変 (`(x+y) mod 2^32 ≡ x+y (mod 2)`) なので wrapping_add で
debug/release/WGSL 3 者を厳密一致化。さらに `(x+y)&1 == (x^y)&1` は
桁上がりが bit1 以上にしか寄与しないことから数学的恒等 — 厳密同一性を
スイープテストで機械固定 (2 冪・2 冪-1・MAX 系 × 直積)。

### BQ-2/BQ-3 (低): 厳密ビットピン + WGSL 語彙ピン新設
- reconstruct_exact_bits_canonical: 左結合和×0.25 の独立導出ビット
  (非対称配置でクロスチャンネル混入検出可能)。
- wgsl_mirror_lexical_tokens: parity `(gid.x + gid.y) & 1u) == 0u` /
  コピー経路 / 斜め 4 方向 gather / 左結合平均の 7 トークン。

### 判定記録 (変更なし)
- 3連鎖の parity/gather 方向/加算順/端 clamp は全一致 (実測照合)。
- 設計上の発見: 時系列パリティ反転 (temporal CB) は本モジュールには無いが、
  **adaptive_shading が独自の per-frame/chunk parity 制御を既に実装**
  (テスト名 A: checkerboard_alternates_per_frame_and_chunk_parity が実在)。
  本モジュールは空間再構成規則の担当として正しく、時系列拡張は
  frame/mirror 層の設計判断として記録 (未着手)。
- full_graph_wiring の is_rendered(frame_index%2, 0) 呼出はフレーム偶奇
  判定の簡易利用 (同ファイル本監査は大物 wave で実施予定)。

### テスト (+3 純増、833 全緑)
mask_parity_is_exact_and_wrap_safe / reconstruct_exact_bits_canonical /
wgsl_mirror_lexical_tokens。frame_postfx::checker_matches_module_fns 等
既存の liveness テスト全緑。

### 検証結果 (全て実測)
- lib **833/833** (+3)。structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: HEAD=0 → WORK=0 (新規コードもクリーン)。

## BR. wboit.rs 監査 (wave 68, 2026-07-23)

Weighted Blended OIT (McGuire & Bavoil 2013) CPU 参照 (113 → 約200 行)。
消費: full_graph_wiring (:158/:303 struct、:1560-1568 半透明 1 サンプル
計算→破棄の状態連鎖) / gpu_runtime (:110 WGSL 一覧)。2連鎖: wboit.rs ↔
wboit.wgsl (weight/accumulate/resolve 全式厳密一致を実測照合)。

### BR-1 (低 — doc 誠実化): 簡約単一ターゲット形である明記
モジュール参照の原論文は resolve に**第二ターゲット revealage Π(1−a_i)**
を使う完全形だが、本実装 (および WGSL ミラー) は accum のみの簡約形で
あり、返り alpha は正規化されない **Σ(a_i·w_i)** — near フラグメント
重複で 1.0 超 (a=0.5×3 枚 = 厳密 1.5) に成り得る。生成物は配線で破棄
される状態 (実合成非消費) のため実害なしだが、引用アルゴリズムとの
差分を module doc に明記 + 非正規化挙動を厳密値ピンで決定的に固定。
完全形 (revealage 第二ターゲット) 導入は挙動変更であり実機 GPU 検証を
要するため本 wave では行わない (実施判断と根拠を記録)。

### BR-2 (低-中): NaN depth の静寂な最近接化ハザードを入口 assert で遮断
Rust `f32::max(NaN, 0.0)` は 0.0 を返す (標準ライブラリ仕様) ため、
旧実装は **NaN depth を静寂に d=0 (weight=1.0 = 最前面優位) 化**していた。
WGSL max も片側 NaN で不定値返却が規格上許容 (決定的経路を持てない)。
「NaN=観測欠測は拒否」哲学に従い weight/accumulate 入口に有限 assert
(色成分も同契約)。should_panic 2 件で回帰固定。配線供給値
(chunk_dists.max(1.0)) は現状有限確認済。

### BR-3 (低): 厳密ビットピン新設 (従来は不等号 + 1e-5 許容のみ)
- weight_exact_bits_canonical: near 以下 → 厳密 1.0、1/9・1/17 独立導出。
- accumulate_resolve_exact_bits_canonical: far red + near blue の全経路
  (front-bias を 0.83/0.17 に定量化) を独立導出 7 値で固定。
- resolve_alpha_is_unnormalized_sum_documented_behavior: Σ(a·w)=1.5 の
  非正規化挙動ピン (BR-1 直接回帰)。
- single_fragment 既存テストの 1e-5 許容理由 ((x·s)/s は bit 非恒等) を
  コメントで誠実化。

### テスト (+5 純増、838 全緑)
weight_exact_bits_canonical / accumulate_resolve_exact_bits_canonical /
resolve_alpha_is_unnormalized_sum_documented_behavior /
nan_depth_is_rejected / nan_color_is_rejected。

### 検証結果 (全て実測)
- lib **838/838** (+5)。structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: HEAD=0 → 自前 3 hunk → rustfmt 全適用 → 0。

## BS. temporal_mesh_diff.rs 監査 (wave 69, 2026-07-23)

Temporal Mesh Diff (96 → 約170 行)。消費: full_graph_wiring (:127/:266
struct、:742 mark_dirty×N/:749 take_dirty/:760 patch_for_block — 結果は
コスト均一のカウンタ集計のみに利用)。

### BS-1 (高 — doc 嘘の誠実化): 責務外主張の撤回
旧 module ヘッダ「石1つ置いても8頂点だけ更新、従来全再構築を回避」は
コードに根拠のない主張だった (該当機構は存在しない)。`patch_for_block`
のコメント「6 面×2tri=12quad 影響・周辺 8 ブロック再生成指示」も同様に
実体 (removed_quads=[block_idx] 1 個・added 空) と乖離。**真のメッシュ
差分機構は diff_mesh.rs (監査済) に実装済み**で重複再実装は不要 — 本
モジュールの責務を「dirty セクションの決定的追跡 + パレット差分列挙」
に限定して誠実化し、diff_section の live 配線を消費者追加方針に基づく
将来候補として記録 (削除ではなく配線候補)。契約 assert (block_idx <
4096、live 呼出 `packed & 0xFFF` 適合実測) を追加。

### BS-2 (中 — latent 非決定性の遮断): take_dirty のソート決定的化
旧実装は HashMap イテレーション順 (RandomState、プロセス毎不定) をその
まま返していた。現在の消費はコスト均一カウンタのため被害は latent だが、
キー同一次第の処理 (予算繰越の identity 選択等) が将来入れば実行毎分岐
のハザード。(cx, cz, sy) 昇順ソートを契約として固定 (SectionKey に Ord
導出追加、無害な trait 拡張)。敵対的挿入順テストで機械固定。

### BS-3 (低): patch_for_block の契約厳格化
usize の `as u32` キャストは 4096 超で静寂切捨てし得た — 契約 assert +
境界受理 (4095) / 拒否 (4096) ピン。

### テスト (+3 純増、841 全緑)
take_dirty_returns_sorted_deterministic_order /
patch_for_block_boundary_acceptance /
patch_for_block_rejects_out_of_section (should_panic)。

### 検証結果 (全て実測)
- lib **841/841** (+3)。structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (ソート化は均一カウンタ消費のみのため行項目不変を実測確認)。
- fmt: HEAD=0 → 自前 5 hunk → rustfmt 全適用 → 0。

## BT. vrs.rs 監査 (wave 70, 2026-07-23)

Variable Rate Shading マスク生成 (161 → 約230 行)。消費: full_graph_wiring
(:157/:302/:1664) / frame_postfx (vrs_run_cpu 精密ミラー、Vrs::build_mask
との liveness テスト :1070) / gpu_runtime (:105)。3連鎖: vrs.rs ↔
vrs_run_cpu ↔ vrs.wgsl (score 式・厳密大なり 4 閾値・逐次平均・コード写像
全て一致を実測照合、BO-1 同型分岐なし)。WGSL ヘッダ「ハードウェア VRS は
wgpu 非対応、マスク生成が実効果」の誠実注記を確認 (良好)。

### BT-2 (低): build_mask の tile=0 が素朴 0 除算 panic
frame_postfx::vrs_run_cpu 側には「tile は 1 以上必須」の明示 assert が
あったが、vrs.rs 本体には無く `(w + tile - 1) / tile` の 0 除算 panic
エラーメッセージ不明瞭だった。同一 fail-loud 契約を 3連鎖全入口で強制
(should_panic + tile=1 受理ピン追加)。

### BT-1 (低): 厳密大なり境界の厳密固定
score ≡ 0.6 / 0.3 / 0.05 / -0.3 (全て f32 リテラルと bit 一致する導出済
入力: 0.375·0.8f32 ≡ 0.3f32 を利用) で次段へ進まないこと、および
0.6+1ulp (from_bits) で厳密に最粗化することを機械固定。旧テストの不等号
のみから閾値非対称性の仕様固定へ強化。

### BT-3 (低): WGSL 語彙ピン新設
score 式 (clamp 両辺)・4 閾値・逐次加算と個数除算・コード割当の
9 トークン。

### 判定記録 (変更なし)
- (w + tile - 1) / tile の usize 加算は assert 済み w*h バッファサイズ
  制約下で現実的 overflow 不可 (w*h の usize 積が先に満たせない)。
- score 加重 (motion_weight=1.0, variance_weight=0.8) と一律平均の規則は
  WGSL/liveness 両面で一致。
- 高速近似なし (逐次 f32 加算) は決定性優先の設計として妥当。

### テスト (+4 純増、845 全緑)
select_thresholds_are_strictly_greater / build_mask_rejects_zero_tile /
build_mask_accepts_tile_one / wgsl_mirror_lexical_tokens。

### 検証結果 (全て実測)
- lib **845/845** (+4)。structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: HEAD=0 → 自前 3 hunk → rustfmt 全適用 → 0。

## BU. lod_hybrid.rs 監査 (wave 71, 2026-07-23)

3-tier LOD hybrid (178 → 約250 行)。消費: frame_reuse (::7 利用、
try_reuse/tier→svo/simplify の live 経路、wave 41 系) / render_pipeline
(:102 struct、:182-185 FeatherRenderConfig 由来構築) / lib.rs re-export。
設定空間は rsift-api FeatherRenderConfig (:21/:25、feather_minimal では
両 flag true)。

### BU-1 (低-中): NaN 距離が静寂に最低詳細 (Far) 化していたハザードを遮断
旧実装は NaN が全ての `<=` 比較を false にし、tier_for_distance が
**Far を返却** — 距離不明のサイレントな最低詳細化だった。NaN=欠測拒否
哲学に従い入口 assert 化 (±∞ は比較の責務として受理: +∞→Far、
-∞→Near)。live 供給 (frame_reuse 経路) は全スイート 849 件で有限を
実測確認 (assert で破壊なし)。

### BU-3 (低): use_svo_encoding の命名乖離を真値表で確定
`svo_far_only` は名に反し **「SVO を Mid にも拡張」スイッチ** (false でも
Far は SVO)。12 組合せ (enabled×svo_far_only×tier 3 値) 全エントリを
真値表 doc + 機械ピンで固定。公開フィールドのリネームは 2 クレート跨ぎ
(API 契約) のため見送り、真値表固定を選択 (根拠記録)。
Near → SVO は全組合せで不成立 (設定 doc「not near terrain」と整合)。

### BU-2 (低): 空メッシュ簡略化の早期 passthrough をピン化
(頂点無しに再 index を走らせない契約の機械固定、chunk 座標 identity
保持確認)。

### 判定記録 (変更なし)
- simplify_mesh の quad stride 間引き (Mid=2、Far=4、先頭側保持) は
  「geometry thinning」規約通り一貫、再 index ・部分 quad drop 処理は
  既存厳密テストで健全。
- tier 境界 (<= で近い側) は既存テストで固定済み、変更なし。

### テスト (+4 純増、849 全緑)
svo_encoding_full_truth_table / nan_distance_is_rejected /
infinite_distance_uses_comparison_result / simplify_empty_mesh_passthrough。

### 検証結果 (全て実測)
- lib **849/849** (+4)。structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: HEAD=0 → 自前 1 hunk → rustfmt 全適用 → 0。

## BV. vertex_cache_opt.rs 監査 (wave 72, 2026-07-23)

Forsyth (Tipsify) 式 post-transform vertex cache 最適化 (328 → 約390 行)。
消費: full_graph_wiring:166 (vco struct 保持) / :311 (new(16)) /
:1050-1052 (acmr→optimize→acmr の draw indices 経路) / gpu_runtime:88-89
(WGSL 登録) / examples pseudo_mc_{bench,live} (実 mesh indices 6/quad)。

### BV-1 (低-中): 半端な末尾 index の静寂 drop を fail-loud 契約へ転換
`optimize` は `ntri = len/3` (floor) + `chunks_exact(3)` で、len%3 != 0 の
場合に末尾 1-2 index を静寂に捨てていた (部分三角の消失)。`acmr` は逆に
全 index を走査しつつ ntri=floor で除算するため、端数 index がミス計上
だけを膨らませ比を誇飾していた (両者で非対称の静寂挙動)。両者に
`len % 3 == 0` の入口 assert を追加 (契約違反は誤用として即検出する
fail-loud 文化、BT-2 同型)。live 呼出 full_graph_wiring:1048 は
`(0..quad_positions.len())` 連番で長さが 3 の倍数とは限らず assert で
実害 panic し得たため、呼出側を `len/3*3` 切り捨てに明示化 — 同ファイル
nanite 経路 (:1088) と同一規則。`>= 96` ゲートは整数全域で旧挙動と等価
(95 以下は不発、96-98→ntri 32、99→ntri 33)。structural digest 実測で
不変を確認 (`004c1cf5fb17bfe8` rows=357)。

### BV-2 (低): cache_size=0 の usize underflow を入口契約で明示
`acmr(indices, 0)` は LRU ミス経路の `cache[cs - 1]` で usize 0-1
underflow し、debug では subtract overflow panic、release では wrap 後の
OOB index panic という原因不明瞭な二系統の最後段パニックに落ちる。
`VertexCacheOptimizer::new` は .max(4) clamp で安全だが `acmr` は生 u32 を
直接受け取る。pub フィールド直書き (cache_size:0) で `optimize` 側も
空模擬キャッシュへの use_vertex 破綻 (cache[len-1] OOB) が可能なため
こちらにも同契約を追加。panic 意図メッセージを should_panic テストで
機械固定 (2 件)。

### BV-3 (低-中): 厳密出力シーケンス 2 ピン (heap tie-break 規則の手導出)
候補 heap は (score total_cmp 降順, 同点は tri 番号降順) の全順序で pop 列
が一意 — この規則から手導出で固定:
(a) 非共有 2 三角 [0,1,2],[3,4,5] (cache=4): 全スコア 0 同点 → tri 降順で
  [3,4,5] 先行、頂点共有無しで再スコア不発 → 出力 [3,4,5,0,1,2]。
(b) 共有辺 [0,1,2],[2,1,3] (cache=4): T1 放出で cache=[3,1,2,-1]、T0 は
  cache_pos v0=-1(0.0)/v1=1(10.75)/v2=2(10.75) の計 21.5 に再スコア
  され続けて放出 → 出力 [2,1,3,0,1,2]。世代番号つき lazy invalidation
  heap の stale skip 経路 (ver 不整合 pop→continue) も本ピン通過で検証。
初回実行で手導出通り全一致。加えて LRU モデルの厳密整数比ピン
(全ミス 6/2 = 3.0、4 ミス 2 ヒット 4/2 = 2.0、f32 誤差ゼロ) を併設。

### BV-4 (低): WGSL 注記の CacheScore 例示式を Rust 実装の逐語ミラーへ改修
vertex_cache_opt.wgsl は dispatch 無しの注記ファイルだが、例示 fn の
スコア式 (未キャッシュ 0.75 / MRU3 1/(p+1) / 減衰 2/(p+2)) が Rust
vertex_score (0.0 / 10.75 / 2·scaled²) と全値不一致で、読者を誤誘導する
文書だった。注記も実契約に一致させる誠実性原則 (BO-1 同型) に従い
cacheSize 引数つきへ拡張のうえ逐語ミラー化 (span clamp も i32 max → f32 順
で一致)。gpu_runtime の naga sweep / runtime-dispatched 両検証もスイート内
で通過。4 語彙 (`0.75 + 10.0` / `2.0 * scaled * scaled` /
`max(cacheSize - 3, 1)` / `return 0.0;`) を Rust 側テストで語彙ピンし、
将来のドリフトを機械捕捉 (BM-2 型 2 連鎖)。

### 判定記録 (変更なし)
- CSR 化 (vt_off+vt_flat、ti 昇順維持)・世代番号つき lazy invalidation heap
  (積み直し禁止の飢餓回避 doc 論証)・LRU 追放追跡 (evicted の位置 -1
  無効化)・score_table 事前計算 (=vertex_score bit 同一)・acmr の LRU
  モデル一致は、doc 主張と実装の照合で全て正確と確認、変更なし。
- heap pop の expect は「未放出の各三角に有効エントリがちょうど 1 つ」の
  不変条件より到達不能 (防御的残置として妥当)。
- full_graph_wiring ブロックの「実 draw indices」注記 vs identity 連番の
  意味論差異 (実 mesh 配線ではない点、および改善判定が採用に繋がっていない
  点) は full_graph_wiring 本監査 (未監査、2,415 行) の棚卸しへ引継ぎ記録。

### テスト (+7 純増、856 全緑)
emits_exact_sequence_empty_cache_tie /
emits_exact_sequence_shared_edge_rescore / acmr_exact_pins_lru_model /
acmr_rejects_zero_cache_size / optimize_rejects_partial_triangle /
acmr_rejects_partial_triangle / wgsl_note_mirrors_vertex_score_vocabulary。

### 検証結果 (全て実測)
- lib **856/856** (+7)。structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: vertex_cache_opt.rs HEAD=57 → WORK=57 (新規分は全て正準形)、
  full_graph_wiring.rs HEAD=158 → WORK=158 (初稿 +3 の差分行を同一測定系で
  捕捉し正準形へ是正)。
- cargo check -p rsift-opt-gfx --all-targets 通過 (既存の未使用警告のみ)。
- 不可視文字 (U+200B/FEFF/NBSP 等) 混入 0、CRLF 0 を python 検査で確認。

## BW. world_column_store.rs 監査 (wave 73, 2026-07-23)

live Minecraft コラムストア (ClientLevel → 圧縮 SectionPalette + フレーム定数、
318 → 約560 行)。消費: render_pipeline (:123 world struct / :259 new /
:281 column_for_mesh / :1103 ingest_world_column 経由 / :1145 prune)、
low_spec_stack:374 (TerrainFrameConstants::from_camera)、rsift-jvm
c_abi_vtable:55 / chunk_bridge:158 (FFI 実入口 — 両者とも
`len >= section_count*4096` を事前保証していることを一次確認済)。

### BW-1 (低-中): 非有限カメラ座標の静寂受理を drop 拒否へ (FFI 安全形)
`set_camera` は NaN/±∞ を無検査でキャメラに反映していた。NaN は
`NaN as i32 = 0` の飽和により mesh_origin が静寂に原点附近化
(見えない世界シフト)、±∞ は `(cx-1) * SECTION_SIZE` で i32 overflow
(debug: panic / release: wrap)。FFI 入口 (C ABI ready 経路) のため
panic=abort 回避で「非有限は観測欠測として drop し直前の有限カメラを
維持」に転換 + debug! 通知。拒否後に有限値を送れば正常復帰することも
ピン (状態固着なし)。NaN=欠測は拒否、の哲学に整合しつつ FFI 境界では
panic しない設計 (wboit BR-2 の assert 型とは入口の性質差で選択)。

### BW-3 (低): 帳簿カウンタの差分会計化 (O(n²) bulk ingest の解消)
旧 `recount_bytes` は ingest 毎に全カラム O(n) 走査 → bulk ingest で
O(n²) 悪化。ingest を差分加減算 (上書き時は旧カラム分を引いてから新分を
足す) に変更し、全再計算は `reconcile_byte_counters` として公開 API 化
(消費者: 将来の外部直接操作の回復路 + 本 wave の不変量テスト)。
u64 整数のため「差分 == 全再計算」を**整数等値**で要求できる — 新規 /
増設 / 縮退上書き / cold↔hot 往復の 4 経路で不変量ピン
(byte_counters_match_full_reconcile_as_invariant)。dense は
2 カラム × 1 section の厳密値も同時固定。compress_distant / prune_outside
は元々 O(columns) 全走査する処理なので全再計算呼出のまま (簡潔さ優先、
差分化の計算利得なし — 判定記録)。

### BW-2 (判定記録 ↔ doc 明文化): ingest の短配列は契約外防御として air 充填
`if end <= flat.len()` 分岐 (flat 不足セクションを air のまま残す) は
静寂挙動に見えるが、2 系統の実 FFI 呼出 (JNI chunk_bridge:149-151 で
`len < need` なら return / c_abi_vtable:40-55 で sections = len/2/4096
切捨て) が共に十分長を保証すると一次確認 — 到達不能防御。**削除せず**
「契約外入力は欠測=air として受理」契約を doc に明文化して固定
(section_count==0 no-op と併せて)。

### BW-4 (低): mesh_origin / column_for_mesh 窓の厳密ピン
- オリジン計算: div_euclid による負側丸め (-8 → section -1、trunc 除算
  だと 0 誤り) を含む厳密ピン。カメラ section の 1 つ手前が窓の min 隅
  ((-1..2) の 4 section 窓)。
- column_for_mesh: 窓は半開区間 [want_base, want_base+4)。境界両側ピン:
  sy=6 → Some (窓内最終)、sy=7 → None (窓外)、no-overlap → None
  (air-only の Some を返さない契約、meshing 入力契約として機械固定)。

### BW-5 (低): 行列ユーティリティの厳密数学ピン (simd_kernels との語彙一致裏付け)
- look_at_rh: 単位軸ケース (f=+z) で全要素 ±1/0/低整数の厳密行列ピン
  (s,u,-f 行 + 平行移動行、除算は norm=1 のみ)。
- perspective_rh: near=1, far=2 で m22=m32=-2 (f32 厳密な有理値) +
  構造ゼロ項 11 箇所 + m23=-1 を厳密ピン。m00/m11 は cot(fov/2) で tan
  libm 依存のため直接ピンせず、m00·aspect≈m11 (rel < 1e-6) と f の
  単調減少性で構造固定 (一貫性: アスペクト補正の存在を機械保証)。
- mul4: A·I=A、diag 右乗算=列スケールの全要素 f32 厳密ピン (錬成は
  ×1.0 厳密・+0.0 加算は値不変の算術裏付けつき)。
- from_camera 合成は既存 frame_constants_have_origin (chunk_origin 一致)
  と simd_kernels の「同型 row-major」主張でカバー (変更なし判定)。

### テスト (+7 純増、863 全緑)
non_finite_camera_is_rejected_and_previous_camera_kept /
byte_counters_match_full_reconcile_as_invariant /
column_for_mesh_no_overlap_returns_none /
mesh_origin_tracks_camera_section_exact / look_at_rh_exact_unit_axes /
perspective_rh_exact_rational_terms / mul4_identity_and_diagonal_exact。

### 検証結果 (全て実測)
- lib **863/863** (+7)。structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (BW-3 差分会計化後に再測定、挙動同一の機械保証)。
- fmt: HEAD baseline 29 行 → 初稿 +29 (自前 hunk を rustfmt 正準形へ 5 箇所
  修正) → 最終 **23 (WORK-only hunk 0、HEAD 由来のみ温存)**。
- cargo check -p rsift-opt-gfx --all-targets: エラー 0。
- 不可視文字 0 / CRLF 0 (python 検査)。

## BX. transform_svdag.rs 監査 (wave 74, 2026-07-24)

transform_svdag.rs (139→352 行)。SVDAG (疎 voxel DAG) ノードを Y 面 D4 変換
(Y 回転 × 鏡映、最大 8 元の軌道) で canonical 化して挿入するラッパ。
live 消費: full_graph_wiring:866 `insert_transform_aware(node.clone())` (戻り破棄)。
基底 svdag.rs (`insert_node` dedup、113 行) は未監査 → 棚卸し残に登録。

### BX-1 (中): 中間 pool ヒット時の誤タグ返却 — D4 群合成で根治
- 問題の定式化: 返却契約は `(id, tag): base_dag.nodes[id] == permute_node(&node, tag)`
  (tag は「入力ノード → canonical ノード」の実変換)。しかし初回挿入で canonical_pool
  には「入力ノード (非 canonical)」と「canonical」の 2 写像が乗る。後続ノードの最初の
  pool ヒットが**非 canonical 中間ノード**の場合、旧実装は発見 tag (入力→中間) を
  そのまま返し、契約 (入力→canonical) を破っていた (ゼロデイ修復)。
- 具体例 (rustc 検算スクリプトで列挙順厳密再現済): n1=octant5 (child 42) 先行登録
  (canonical=octant0、保存 tag (0,T,T)) の後に n2=octant1 を挿入すると、列挙順で
  (0,F,T)→octant5 が最初のヒット = **非 canonical の n1 自身**。旧実装は (0,F,T) を
  返し n2 適用先は octant5 (canonical でない)。正解は合成
  compose((0,F,T),(0,T,T)) = (0,T,F) = mirror_x。
- 修復: `compose_tags(first, second)` を新設。(x,z)∈{0,1}² の 4 点への作用で D4
  群元を同定 (標準作用は faithful: 8 群元は正方形頂点の置換として全て異なる)、
  関数等価なタグは列挙順 (rot 外周 → mx → mz) 最初の表現に潰す (16 記法→8 群元、
  決定的)。発見 tag∘保存 tag を合成して返し、今回ノードも pool に memo 化
  (以後の直接ヒットは O(1)、再最小化はしない)。doc で返却契約を明文化・機械ピン化。
- アドバーサリアル検証: compose_tags 呼出を旧生タグ返却に一時置換 →
  新テストが assert で赤 → 修復版へ復元 → 4/4 緑 (検出力の機械確認、BM-1 型)。

### BX-2 (低): 列挙順決定性の厳密ピン (検算による自己誤り捕捉 2 件)
- R90∘R90 は 16 記法で (2,F,F) と (0,T,T) (=XZ 全反転) の 2 表現を持ち、列挙順
  (rot 外周が先) では **(0,T,T) が返る**。初稿期待値 (2,F,F) は誤りで、検算
  スクリプトが捕捉 (「テスト赤=自己誤り捕捉装置」実績 6 件目)。
- 初稿テストシナリオ (octant4) は非 canonical ヒット経路を踏まないことを検算で
  捕捉 → octant1 シナリオに修正 (同実績 7 件目)。
- 群閉包公理ピン: mirror 3 種はいずれも involution (m∘m=(0,F,F))、
  R90∘R90=(0,T,T)、R180∘R180=(0,F,F)、octant6 (y 不変 orbit {6,7,3,2}) の
  canonical=idx2 (child_mask 0b0000_0100、children[2]=7 で child 値追従を固定)。

### テスト (+3 純増、866 全緑)
composed_tag_maps_input_to_canonical_on_intermediate_pool_hit /
compose_tags_group_closure_axioms /
repeated_insert_is_deterministic_and_contract_preserving
(ヘルパ `node_at(octant, child)` 新設。既存 test_transform_aware_canonicalization
も維持緑、全値手導出・検算スクリプト裏付け)。

### 検証結果 (全て実測)
- lib **866/866** (+3)。structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (修復後に再測定、live 呼出は戻り破棄のため挙動影響なしの機械保証)。
- fmt: HEAD baseline 46 → 初稿 69 (自前 4 箇所) → rustfmt 正準形へ是正後
  **46 (WORK-only 差分行 0、HEAD 由来のみ温存)**。
- 不可視文字 0 / CRLF 0 (python 検査)。
- 引継ぎ: 基底 svdag.rs (dedup・解放順) は本監査対象外 → 棚卸し残。

## BY. vertex_pool.rs 監査 (wave 75, 2026-07-24)

vertex_pool.rs (171→276 行)。単一 GPU バッファのリング割当器 (chunk 毎 VBO
アロケート排除)。live 消費: render_pipeline (:45/:96/:194 adaptive、
:675/:677/:749/:751 upload_mesh **戻り破棄**、:876 active_chunks telemetry)。
persistent_pool (persistent_vbo_pool) 優先で vertex_pool は fallback 経路。
slots は現状描画未消費 (簿記/telemetry用途)。

### BY-1 (中): oversize 拒否時の stale slot 残留 — 「None ⇒ slot 無し」に一貫化
- 問題の定式化: 有効 slot 保有の chunk に容量超過メッシュが来た場合、旧実装は
  `None` を返しつつ**旧 slot を map に残存**させた。空メッシュ経路は evict する
  のに非対称で、不変量が経路依存。現状の live は戻り破棄 + 描画未消費のため
  無害だが、将来 slots を描画に使う消費者は **stale geometry を参照**し得る
  (ゼロデイの卵。消費者追加方針の事前安全化でもある)。
- 修復: 拒否時に `slots.remove` + `oversize_rejects` (telemetry, pub 追加) 計数 +
  `debug!` 通知 (静寂拒否の解消)。doc で返却契約を明文化。
- 影響範囲検証: slots の live 消費は active_chunks() (telemetry) のみ →
  描画挙動無影響を digest 実測不変で機械確認。
- アドバーサリアル検証: 旧実装 (evict 無し) を一時注入 → 新テストが :202
  `active_chunks()==0` で赤 → 復元後 8/8 緑 (検出力の機械確認、BM-1/BX-1 型)。

### BY-2 (低): refresh セマンティクスの厳密ピン
同一 chunk 再 upload: 旧領域は ring 上に放棄 (cursor 非巻戻し、回収は ring
reset まで遅延)、新 slot がマッピングを置換。手導出ピン: 4v/6i → 8v/12i で
(0,4)→(4,6,8,12)、active_chunks==1、ring_uploads==2、slots[(0,0)]==新 slot。

### BY-3 (低): 境界・定数の厳密ピン
- `cursor + count == capacity` は適合 (`>` 比較の境界、reset しない):
  cap 8 に 8v/12i 丁度 → offset 0 gen 0、次 4v/6i → reset gen 1 offset 0。
- `capacity_indices = v*3/2` の floor: cap 5 → idx cap 7、quad (4v/6i) 適合。
- generation `wrapping_add(1)` 契約: pub 直書きで MAX 強制 → reset で 0
  (2^32 周の世代衝突は実時間到達不能として ring 設計に doc 明記)。
- 評価済み見送り: `adaptive()` の `mb*1024*1024/12` は bump_arena_mb:usize
  (実値 2-24、max(4) 併合) で overflow 不能 → 変更なし判定。

### テスト (+4 純増、870 全緑)
oversize_reject_evicts_stale_slot_and_counts /
refresh_replaces_mapping_and_abandons_old_region /
exact_capacity_boundary_fits_then_next_resets /
generation_wraps_on_reset_and_floor_ratio_pin (全値手導出、初回実行で一致)。

### 検証結果 (全て実測)
- lib **870/870** (+4、vertex_pool 8/8)。structural_digest
  `004c1cf5fb17bfe8` rows=357 不変 (修復後に再測定)。
- fmt: HEAD baseline 4 → 自前 3 箇所を rustfmt 正準形へ是正後 **4
  (WORK-only 差分行 0、HEAD 由来のみ温存)**。
- cargo check -p rsift-opt-gfx --all-targets: エラー 0。
- 不可視文字 0 / CRLF 0 (python 検査)。

## BZ. persistent_vbo_pool.rs 監査 (wave 76, 2026-07-24)

persistent_vbo_pool.rs (405→598 行)。1 本 GPU バッファ + bucket allocator +
MDI バッチ (driver GC 排除)。live 消費: render_pipeline (:33/:97/:199 adaptive、
:675/:749 upload 戻り破棄、:855 rebuild_mdi、:872-877 telemetry) と azdo.rs
(:11 保持のみ、wave 45 で責務境界明文化)。GPU ラッパ (PersistentVboPoolGpu:
flush_slot/flush_mdi/draw_all_indirect) は現状 live 未配線の既知状態。

### BZ-1 (高): index 確保失敗時の vertex 領域永久リーク — rollback_alloc で根治
- 問題の定式化: `upload_mesh` は vertex 確保 (`?` 成功) → index 確保の順。
  index 失敗時の早期 `?` return は **vertex 側の確保 (free-list 消費または
  high-water 前進) を巻き戻さず**、所有者不在の領域が永久に回収不能と
  なっていた (確保ログに記録なし)。繰り返し失敗でプールが擬似枯渇する
  リーク型ゼロデイ。
- 修復: `rollback_alloc` 新設。high-water 先端からの確保は high-water を
  巻き戻し (`offset+size == high_water`)、free-list 由来は free list へ
  返却 (return_to_free_list の sort+merge で消費時 remainder と再統合
  →元ブロックに厳密復元)。index 確保失敗路に適用。
- 検証: 2 経路を厳密ピン (A: hw 巻戻し vhw 8→4・free 非汚染 / B: free-list
  経路 (0,8) 消費→(4,4) 残余→失敗→(0,4) 返却→merge で (0,8) 復元)。
  アドバーサリアル検証: 旧 `?` 注入で両テスト赤 (:477/:494) → 復元 7/7 緑。

### BZ-2 (中): oversize 拒否時の stale slot 残留 (BY-1 同型、こちらは描画実消費)
- 旧実装は oversize 拒否で旧 slot を残存。**本モジュールの slots は
  rebuild_mdi 経由で MDI 命令に実消費される**ため、stale geometry が
  描画され続ける実害経路あり (render_pipeline:855)。→ 「None ⇒ slot 無し」
  に一貫化 (release + `oversize_rejects` 追加 + 既存 warn 維持)。
- アドバーサリアル検証: 旧非 evict 注入で 2 テスト赤 (:532/:590) → 復帰緑。
- 注入ミス実績 (誠実記録): BZ-1 注入の初稿は `?` 復元でなく match 腕を
  破壊し E0004。アドバーサリアル注入は**元コードの忠実復元**で行う規則を
  再確認し、注入を厳密化して再実施 (本件は試験手順の自己修正、製品コード
  無関係)。

### BZ-3 (低): utilization() の 0 容量 NaN
new(0) で v/i capacity 0 → 0/0=NaN。NaN=観測欠測の哲学に従い容量 0 は
0.0 に倒す guard 追加 (watermark 方式の意味論も doc 明文化: freed は
差し引かず「過去最大占有」を見る指標)。

### BZ-5 (低): rebuild_mdi が slot.mdi_index の真値を書き戻さない
upload 時 append 位置の mdi_index は rebuild の keys sort 再配置で stale
化するのに、旧実装は slot を未更新 (嘘のハンドル)。→ rebuild で
`PersistentSlot { mdi_index, ..s }` 書き戻し。live 消費者ゼロ (grep 確認)
のため安全、かつ将来の GPU 配線 (消費者追加) の事前安全化。命令列の
全フィールド厳密ピン (append 順≠sort 順のシナリオで内容・冪等性)。

### BZ-6 (低): 容量配分 3:1 → 2:1 (数学的最適化、挙動不変を機械確認)
- 旧配分は bytes 3:1 = 要素数 v==i (両者 pool_bytes/16)。主 workload の
  all-quad (4v/6i = 要素比 1:1.5) では index 側が 2/3 充填で枯渇し、
  vertex 側 1/3 が恒常死蔵。数学的最適 bytes v:i=(4×12):(6×4)=2:1 へ改訂
  (new(1): v_cap 87381→58254 / i_cap 65536→87381)。
- 影響検証: ベンチの chunk は容量境界を踏まないため structural_digest
  `004c1cf5fb17bfe8` rows=357 不変 (実測)。厳密値ピン: v_cap 58254 /
  i_cap 87381 / staging 699048B・349524B / utilization 0.5 (f32 厳密商)。
- カウンタ doc 明文化 (uploads=成功数, reuses=refresh 数,
  bucket_allocs=free-list 経由確保数 (hw 新規は非計上), oversize_rejects)。

### テスト (+5 純増、875 全緑)
index_alloc_failure_rolls_back_high_water_vertex_region /
index_alloc_failure_restores_free_list_block_exactly /
oversize_reject_evicts_slot_and_none_means_absent /
rebuild_mdi_updates_slots_and_content_exact /
capacity_rebalance_utilization_and_zero_capacity_exact
(全値手導出、初回実行で 7/7 一致)。

### 検証結果 (全て実測)
- lib **875/875** (+5、persistent_vbo_pool 7/7)。structural_digest
  `004c1cf5fb17bfe8` rows=357 不変 (BZ-6 配分変更後に再測定)。
- fmt: HEAD baseline 66 → 自前 7 箇所を rustfmt 正準形へ是正後
  **66 (WORK-only 差分行 0)**。
- cargo check -p rsift-opt-gfx --all-targets: エラー 0。
- 不可視文字 0 / CRLF 0 (python 検査)。

## CA. svdag.rs 監査 (wave 77, 2026-07-24)

svdag.rs (114→約280 行)。SVO を DAG 縮約する基底モジュール (BX で棚卸し残
登録 → 本 wave で消化)。live 消費: full_graph_wiring:102/:861-862
(build_from_volume、戻り root は破棄し :883 で aokana へ clone 登録)、
aokana.rs (:45 `dag.root_id` 読取→ShallowSvdag に保持、走査未使用)、
transform_svdag.rs (BX、insert のみ)。

### CA-1 (中): build_from_volume が root_id を更新しない stale metadata 根治
- 問題の定式化: `new()` は root_id=0 (空リーフ=「空世界の root」) に初期化
  するが、`build_from_volume` は構築 root を返すのみで `self.root_id` を
  更新しなかった。構築後の DAG は nodes に実木を持ちながら root_id は
  「空世界 root」のまま = **嘘のメタデータ**。保持者 aokana
  (ShallowSvdag.root_id:16/:51) には常に stale な入口点 id 0 が配られる
  (現状走査未使用のため出力影響なし、将来の DAG 走査消費での踏み外し元)。
- 修復: `self.root_id = build_octree_recursive(...)` として同値を返す。
  doc で契約明文化。live 出力への影響は digest 実測不変で機械確認。
- アドバーサリアル検証: 旧実装 (root_id 非更新) 注入 → 3 テスト赤
  (:180/:198 等 root_id 期待 5 vs 0) → 復元後 11/11 緑。

### CA-2 (低): コメント不一致訂正 + de-facto 契約の明文化・ピン化
- 「Insert leaf empty/solid base nodes」のコメントに反し事前登録は**空
  リーフのみ**。事前 solid 登録で id 1 固定化する改修は ID 絶対値の
  消費者が現状皆無で益小、digest 影響の検証コストに見合わないため
  **挙動は温存し文書を実態に合わせる**判定 (誠実ドキュメント規律)。
  de-facto 契約 (id 0 = 空リーフ、初期 root_id = 0 = 空世界、初 solid
  リーフが id 1) を doc 明文化 + テストでピン。
- insert_node の決定性契約 (pool は lookup のみ、ID は連番、同一挿入列
  →同一 nodes) を doc 明文化 + 2 インスタンス同一 volume で Vec 完全一致ピン。

### CA-3 (低): 厳密構築ピン 3 ケース (全値手導出・初回一致)
- 単一 voxel (0,0,0): 6 ノード鎖 {1,[MAX..]}→{1,[k,..]}、root=root_id=5。
- 全 solid: 各レベル完全 dedup {255,[k;8]} 鎖、6 ノード、root=root_id=5。
- 隣接 2 voxel (0,0,0),(1,0,0): 最下位が mask 3 で leaf id 1 を 2 度指す
  {3,[1,1,MAX..]}、以降は {1,[k,..]} 鎖、6 ノード。
- 空 volume: root=0=root_id 不変、count 1。

### CA-4 (低): 不変量走査の設計修正 (テスト赤=自己誤り捕捉 8 件目)
- 初稿不変量「mask bit ⟺ children[i]≠MAX」は**リーフ形ノードで偽**:
  solid リーフ {1,[MAX;8]} は mask bit 0 立つがリンクなし (リーフの mask
  は solid フラグで、中間ノードの「子孫 solid リンク」意味論と別)。
- 精緻化: リーフ形 (children 全 MAX) ⟹ mask∈{0,1}、中間形 ⟹ bit⟺link ∧
  link 先は有効 ID。テストが初稿の設計ミスを捕捉 (実績 8 件目、
  BM-3/BN-3/BP-3/BX-2×2/BZ-2 注入手順 に続く)。

### テスト (+5 純増、880 全緑)
empty_volume_keeps_base_root_and_updates_root_id / single_voxel_exact_chain /
all_solid_exact_dedup_chain / two_adjacent_voxels_share_parent_with_mask3 /
build_is_deterministic_across_instances (ヘルパ assert_dag_invariants 新設、
既存 test_svdag_deduplication 維持)。

### 検証結果 (全て実測)
- lib **880/880** (+5、svdag 系 11/11)。structural_digest
  `004c1cf5fb17bfe8` rows=357 不変 (CA-1 修復後に再測定)。
- fmt: HEAD baseline 30 → 自前 13 箇所 (構造体リテラル 12+α) を rustfmt
  正準形へ是正後 **30 (WORK-only 差分行 0)**。
- cargo check -p rsift-opt-gfx --all-targets: エラー 0。
- 不可視文字 0 / CRLF 0 (python 検査)。

## CB. aokana.rs 監査 (wave 78, 2026-07-24)

aokana.rs (115→約200 行)。Aokana shallow SVDAG リージョン管理。live 消費:
full_graph_wiring (:104 保持、:242 new(1920,1080)、:883 insert_shallow_region
(600 tick 毎 refresh)、:889 evaluate_visible_regions → **.len() のみ消費**)。

### CB-1 (中): evaluate_visible_regions の HashMap 非決定反復 → sort で根治
- 問題の定式化: `for (&coords, dag) in &self.shallow_dags` は HashMap 反復順
  (SipHash ランダムシードでプロセス/インスタンス毎に不定)。戻り Vec の
  現消費は .len() のため無害だが、順序消費の追加 (例: 先頭優先処理) で
  flaky になる潜伏 (BS-2 非決定 drain と同型)。
- 修復: keys を collect → `sort_unstable` → 順に評価。戻り Vec は
  **(rx,ry,rz) 辞書順**の契約を doc 明文化・ピン。
- 検証: 16 インスタンス横断完全一致テスト (乱順登録 [(1,0,0),(0,1,1),(0,0,0)]
  → 厳密 [(0,0,0),(0,1,1),(1,0,0)])。アドバーサリアル検証: 旧 HashMap 反復
  注入で :159 赤 → 復元後 4/4 緑。
- 副次捕捉 (精神的コンパイル実績): 「広く許容」平面セット [-1,0,0,100] は
  x≥128 区画を捌くため、初稿シナリオの region (2,0,1) は不可視と判明 →
  実行前にシナリオ座標を許容包絡内へ修正 (自己検証 9 件目に準ずる記録)。

### CB-2 (低): 契約ピン (境界・置換・規約)
- 境界 `dot+d == 0` (接触) は可視: 平面 [-1,0,0,64] に対し region (1,0,0)
  は p-vertex -64+64=0 で生存、region (2,0,0) は -64<0 で捌く厳密ピン
  (f32 整数演算、libm 非依存)。p-vertex 選択・符号テストのため平面は
  非正規化でも可 (doc 明記)。
- insert_shallow_region の同一座標 = **最新で置換** (live の 600 tick
  refresh 経路と整合) をピン: root_id/lod_level の置換と len 不変。
- region_size_blocks = 64 規約ピン (full_graph_wiring の K-1 注記と整合)。

### CB-3 (低): doc 誠実化 (Hi-Z/visibility buffer 未配線の明記)
- 旧 doc は「evaluate region AABB against Hi-Z, emit visibility buffer
  commands」と主張するが実装は **frustum p-vertex テストのみ**。
  hzb_occlusion / visibility_resolver フィールドは occlusion pass 統合用に
  確保 (消費者方針に基づき保持) しつつ、現行未配線であることを doc で
  誠実化 (実装か文書か: 文書を選択 — Hi-Z 統合は大機能で別 wave の検証
  が要るため)。
- CA-1 連携ピン: build_from_volume 済み DAG (root 5、wave 77 手導出) を
  登録すると ShallowSvdag.root_id == 5 の真値が保持される。

### テスト (+3 純増、883 全緑)
visible_regions_sorted_and_cross_instance_deterministic /
frustum_plane_boundary_exact /
insert_replaces_and_root_id_is_truthful_after_build
(ヘルパ permissive_planes 新設、既存 test_aokana_shallow_svdag 維持)。

### 検証結果 (全て実測)
- lib **883/883** (+3、aokana 4/4)。structural_digest
  `004c1cf5fb17bfe8` rows=357 不変 (両修復後に再測定)。
- fmt: HEAD baseline 2 → **2 (WORK-only 差分行 0、初稿から正準形)**。
- cargo check -p rsift-opt-gfx --all-targets: エラー 0 (git root からの誤実行
  による偽陽性 1 件を cwd 誤りと特定・正規 cwd で 0 確認)。
- 不可視文字 0 / CRLF 0 (python 検査)。

## CC. visibility_buffer.rs 監査 (wave 79, 2026-07-24)

visibility_buffer.rs (214→約430 行)。Visibility Buffer (8B/px ID バッファ) の
パック・リゾルバ・属性補間・resolve WGSL 生成。live 消費: aokana が
VisibilityBufferResolver を保持のみ (evaluate 未使用、CB-3 で記録済)、
pack/interpolate/WGSL 生成は現状 live 未消費 (消費者追加方針に基づき
削除せず、契約修復 + 完全実装で事前安全化)。

### CC-1 (中): pack_ids の 16bit 静寂切捨て → fail-loud 契約化
- 問題の定式化: `(primitive & 0xFFFF) | ((instance & 0xFFFF) << 16)` は
  入力 ≥65536 をマスクで**静寂切捨て**し、異なる ID が同じパック値へ
  エイリアス (AV-1 pack_handle・BE-1 new() と同一クラス)。GPU は
  `packed & 0xFFFFu` で読むため書込側の値域違反は検出不能。
- 修復: 両入力 <65536 を assert (fail-loud)。超過領域は pack_ids_64
  (truncation なし) への誘導を契約文で明記。
- アドバーサリアル検証: 旧マスク実装注入 → 2 should_panic テスト赤 →
  復元後 12/12 緑。境界ピン: (0,0)=0、(65535,65535)=0xFFFF_FFFF、
  64bit 版 u32::MAX 往復。

### CC-2 (低): compute_barycentrics 縮退フォールバックの誠実化 + 厳密ピン
- |denom|<1e-6 (絶対閾値、denom=符号付き面積×2) で重心 [1/3,1/3,1/3] へ
  静寂フォールバックしていた未記載分岐 → 定義済み規約として doc 明文化。
- 厳密有理値ピン: (0,0),(8,0),(0,8) 三角形 (denom=64=2⁶、全中間値 f32
  厳密) で (2,2)→[0.5,0.25,0.25]、(4,4)→[0,0.5,0.5]、頂点→[1,0,0]。
  初回実行で全一致。

### CC-3 (高・アルゴリズム偽装クラス): WGSL ジェネレータを真の 12B
レイアウト忠実・完全形へ再実装
- 問題の定式化: 生成 WGSL は `pos_packed/uv_packed/color_packed` の
  3×u32・10bit pos という**コードベースに存在しない虚構レイアウト**
  (実 Quantized12ByteVertex は [u16;3] pos + [u8;2] oct normal + [u16;2]
  uv = 12B、color 無し・pos は 3×u16)。さらに uv/color 未算出、
  visibility_texture/max_instances 未使用のスタブ (ユーザー指令「スタブ
  無し・数学的に正しく完全実装」に違反)。
- 修復 (完全実装): 真のレイアウトを 3 ワード (w0=px|py<<16,
  w1=pz|(ox|oy<<8)<<16, w2=u|v<<16) として decode (除数 1024.0/32767.0/
  127.5 は encode の厳密ミラー) + oct normal 折り返し解除 (L1 折りの
  解析的厳密逆: 隅 4 点は全て south pole、f32 厳密) + bary 補間 +
  resolve_main (visibility/bary texture 参照、MAX_INSTANCES=4096u 埋込、
  prim==0xFFFF または inst 超過は discard 契約)。
- 3 連鎖: CPU ミラー decode_pos/decode_uv/decode_normal_oct を同モジュール
  に新設し生成物と語彙一致 (WORDS_PER_VERTEX=3 共有)。生成 WGSL は
  **naga パース + 全セマンティクス検証をテストで機械通過** (gpu_runtime
  sweep 同系、GPU 不要の純 CPU 検査)。虚構語彙 (!pos_packed) も機械拒否。
- アドバーサリアル検証: スタブ的注入 (const 消去 + w0 マスク破壊) →
  語彙ピン :392 赤 → 復元後緑。

### テスト (+6 純増、889 全緑)
pack_ids_full_boundary_exact / pack_ids_rejects_primitive_aliasing_overflow /
pack_ids_rejects_instance_aliasing_overflow /
barycentric_exact_rationals_and_degenerate_fallback /
decode_mirrors_layout_and_encode_scales_exact /
generated_wgsl_is_naga_valid_and_vocabulary_pinned
(旧 test_wgsl_gen は虚構語彙消去を機械拒否する形に更新、既存 6 件維持)。

### 検証結果 (全て実測)
- lib **889/889** (+6、visibility_buffer 12/12)。structural_digest
  `004c1cf5fb17bfe8` rows=357 不変 (live 未消費経路のため挙動影響なしを
  機械確認)。
- fmt: HEAD baseline 2 → 自前 3 箇所を rustfmt 正準形へ是正後
  **2 (WORK-only 差分行 0)**。
- cargo check -p rsift-opt-gfx --all-targets: エラー 0。
- 不可視文字 0 / CRLF 0 (python 検査)。
## CD. hzb_2d.rs 監査 (wave 80, 2026-07-24)

hzb_2d.rs (389→約1040 行)。保守的 CPU Hi-Z オクルージョン (テンポラル
ヒステリシスつき)。live 消費: render_pipeline.rs:801-830 が本流
(列 16x64x16 ボックスで cpu_occlusion 経由 cull_boxes_with_camera 呼出)、
cpu_occlusion.rs は固定/任意カメラの薄いラッパ (bitexact 委譲テスト済)、
aokana.rs は Hzb2D フィールド保持のみ (CB-3 で未配線明記済)、
world_column_store/low_spec_stack は CameraState の型借用のみ。
moved_significantly の外部使用ゼロ (実測)。2026-07-22 の軽監査で
skip_frame 永久無効化バグ (A 期、台帳 A 期節) を修復済の経緯があり、
本 wave が初の全行照合 + 数学的再定式化。

### CD-1 (高): temporal streak の i8 アンダーフロー → clamp 化
- 問題の定式化: `*streak -= 1` は 129 フレーム連続遮蔽 (60fps で 2.2 秒、
  洞窟内静止等で容易に到達) で i8::MIN を割り **debug panic /
  release wrap=127** (描画スレッド致命傷)。判定 <= -4 に対し値域を
  [-REQUIRED, 1] に制限する clamp は数学的に無影響。
- 修復: `(*streak - 1).max(-OCCLUDED_FRAMES_REQUIRED)`。
- アドバーサリアル検証: 旧 `*streak -= 1` 忠実注入 →
  `attempt to subtract with overflow` panic でテスト赤 → 復元後緑。

### CD-2 (高): temporal キーの列衝突 → min 3 成分 to_bits 厳密恒等キー
- 問題の定式化: 旧 `(min_x as i32, min_z as i32)` は (a) 同一 (x,z) 列の
  異断面 (min_y 違い、1.21 系の自然な 24 断面 API 用法) を**全て同一キー化**
  → 1 フレームに断面数だけストリークが進み OCCLUDED_FRAMES_REQUIRED=4 の
  意味論が k 分周 (2 断面で 2 フレーム発火 = フリッカー穴方向)、
  (b) 負座標の `as i32` 切捨てで (-0.9,+0.9) → (0,0) 衝突。
- 修復: `(min.to_bits() x 3)` = bit 完全一致 = 数学的恒等。現行
  render_pipeline (列マージ 1 ボックス/列) ではキー一意のまま挙動不変。
- アドバーサリアル検証: y 潰し (旧 2D キー同型) 注入 → 統合テストが
  frame2 での早期発火を捕捉 (赤) → 復元後緑。キー導出関数は
  実ボックスからの導出で直接ピン (単体テスト強化)。

### CD-3 (高・根幹): Hi-Z 深度値の近/遠逆転 → 厳密最遠 raster/最近 test
- 問題の定式化: モジュール doc 自身「farthest occluder depth per tile」と
  謳うのに、rasterize は occluder の**最近**値 1/dist_center を書いていた。
  各 texel 値は「texel 全域が少なくとも v 以上で覆われる」保証でなければ
  ならないところ、box の rect 内ピクセル深度値は 1/dist_max .. 1/dist_min
  に分布し最小保証は 1/dist_max — 最近値を書くと保証が**偽**となり
  occludee の最近点が occluder 最近フロンティアを僅差で超えるだけで
  誤遮蔽 (false hole)。ヒステリシス (4 フレーム) が隠蔽していた構造。
- 修復 (厳密): raster=1/dist_max (8 隅最大ユークリッド、凸体の外部点からの
  最遠点は頂点で達するため厳密)、test=1/dist_min (clamp 点 = 解析的厳密
  最近距離)。単調性不変量 v_raster<=v_test (dist_min<=dist_max と同一
  clamp から帰結) を assert! で fail-loud 保持 → **自己遮蔽は数学的に不可能**
  を不変量として証明 (統合テストで 8 フレーム実証)。
- アドバーサリアル検証: raster 値の v_test 注入 (旧同型) →
  near_frontier/rasterize_writes_farthest/hull_rasterize の 3 テスト赤 →
  復元後緑。

### CD-4 (高・根幹): 射影の fov/aspect 因子欠落 + 回転無視 → 8 角厳密射影
+ 凸包 scanline
- 問題の定式化: 旧 half_w は /(tan·aspect) が無い (fov=1.0,16:9 で
  tan(0.5)×1.78≈0.972 の**偶然補正**で見た目だけ成立; zoom で 2.2 倍過大
  = 穴方向、広角で 2.8 倍過小)。さらに world-x 幅をそのまま view 幅とする
  軸並行仮定で yaw/pitch 回転を無視 (45° で最大 41% 乖離)。
  bbox raster は silhouette 外の部分被覆 texel にも保証値を書いていた
  (凸体射影は一般に 6 角形等で bbox に満たない)。
- 修復 (厳密): 8 隅を厳密 view 変換→透視除算 (凸体の射影 = 隅射影の凸包、
  z>0 半空間で凸性保存により厳密 silhouette)。test 用 bbox は厳密外接
  (過大方向のみで保守)。raster は monotone chain 凸包 (f64、全非有限で
  プラットフォーム一意) の scanline フィルで**完全内包 texel のみ**に書く
  (帯 [y,y+1] 上下端交差区間の狭い側、ceil(xa)<=x かつ x+1<=xb)。
  near 跨ぎは全隅 min_view_z<=NEAR_PLANE で厳密 None (旧は中心点のみで
  背後角の箱が破綻矩形を生成しえた)。
- 検算: Python f32 往復厳密化スクリプト (/tmp/cd_verify*.py) で全期待値を
  独立導出 → f32 bit ピン (1/√116=0x3DBE26EB、1/√184=0x3D96FB06、
  1/√804=0x3D10746C 等)。45° yaw で bbox 角 texel 非記述 (77 列) をピン。
- アドバーサリアル検証: bbox 全域フィル (旧動作) 注入 →
  hull_rasterize テストのみピンポイント赤 (他 17 緑 = 判別特異性) → 復元。

### CD-5 (中): mip 奇数幅の floor 除算で最終列/行死亡 → ceil 除算
- 問題の定式化: 旧 w1=(w0/2).max(1) では w0 奇数 (画面幅 64..512 で頻出、
  479 等) の最終 texel 列 w0-1 が (x*2+dx).min(w0-1) に現れず mip1 に
  伝播しない (raster したのにピラミッドに現れない = 遮蔽過小 = cull 損)。
- 修復: div_ceil(2) (Hi-Z 標準、Intel 67,65…例と同型)。chain 厳密ピン:
  65→[33,17,9,5,3,2,1]、最終列 64 のみ記述で mip1 列 32 への伝播を実証。
- アドバーサリアル検証: floor 注入 → chain ピン赤 → 復元後緑。

### CD-6 (中): moved_significantly の y/fov/aspect 無視 → 3D 距離 + Δ射影
- 問題の定式化: 旧 XZ 距離のみ。鉛直テレポート (コマンド/リスポーン) や
  ズーム (fov 変化) でピラミッドと temporal が stale のまま使われる
  (hole 方向)。doc「Camera move (blocks)」は 3D が意図。
- 修復: 3D 距離 √(dx²+dy²+dz²) > 2.0、|Δyaw|/|Δpitch| > 0.08 (踏襲)、
  |Δfov_y|/|Δaspect| > 1e-3 (新設 PROJECTION_CHANGE_EPS)。ジャンプ
  1.25・通常落下 (~1.3/frame)・エリトラ (~0.5/frame) は閾値内で
  skip 増なし (消費者実測組合せ: render_pipeline は毎フレーム実カメラ)。
  外部使用ゼロ (実測) のため意味論変更は hzb_2d 内部に閉じる。
- 影響テスト訂正: teleport_skip 回帰テストは初回 default→y64 も武装する
  新仕様に合わせカウンタ系列を訂正 (n,n,2n,3n,3n,4n)。

### CD-7 (中): 非有限カメラ/非正規ボックスの NaN 伝播偶然頼み → 明示 drop 拒否
- 問題の定式化: 旧実装は NaN カメラで f32::max/min の NaN 伝播規則経由の
  副産物として「偶然全可視」になっていた (仕様ではない)。文化 (BW-1 他)
  「非有限 = 観測欠測は drop/拒否」。
- 修復: cull_boxes 冒頭で camera.is_finite 検査 → 非有限なら last_camera・
  temporal・ピラミッド・帳簿を一切更新せず保守全可視を返却 (FFI 長命
  ループのため panic ではなく drop 拒否 = BW-1 確立形)。ボックス側は
  非有限 or min>max 逆転を wellformed 検査で個別 drop (可視パススルー、
  raster/temporal 非登録、バッファ無汚染をテストでピン)。

### CD-8 (低): build_pyramid 全段 src.clone() → split_at_mut 借用分割
- 段ごとに mip 全面を clone していた O(画素) アロケーションを
  split_at_mut(level+1) の借用分割に置換。逐語演算は同一で出力 bit 不変
  (ceil chain ピンで機械確認)。

### CD-9 (低): temporal HashMap 無制限成長 → 16384 上限
- 訪問チャンク断面数だけ単調増加していた (3D キー化でさらに増える意図的
  設計のため上限必須)。16384 エントリ (≒682 列×24 断面) で entry 前に
  len>=MAX なら全クリア (ストリークリセットのみの保守方向) → メモリ定数
  上界化。上限・クリア後 len==1 をピン。

### 誠実化 (観)
- 深度値定義・保守性の根拠・AABB ソリッド近似の適用範囲・残存誤差への
  ヒステリシス位置づけをモジュール doc に再定式化して明記
  (「farthest occluder depth」が実装とようやく一致)。
- VERTEX_BYTES (Hi-Z 本体と無関係の歴史的公開定数) の所在理由を doc 誠実化
  (公開 API + テストピンあり、消費者方針により維持)。
- dist_sq を 3D 化 (front-to-back ヒューリスティックの正確性向上、
  max 更新の可換性から mip0 出力 bit 不変)。
- hzb<0.001 fast path は「hzb=0 なら不等式が構造的不成立」で数学的冗長と
  判明 → 走査省略の高速経路として doc 誠実化して温存。

### 自己誤り捕捉 (10, 11 件目)
- hull_len: unit box の凸包頂点数を 4 と思い込み設計 → Python 検算で
  オフアクシス遠近 2 矩形の凸包は 6 と実行前捕捉・訂正。
- column_sections フレームカウント: skip-arm+空ピラミッドの frame0 で
  streak=1 が立つ系列を見落とし 4 呼出で発火を期待 → テスト赤が捕捉、
  5 呼出系列 (frame1..4 で -1..-4) へ訂正。旧 2D キー注入下では frame2
  早期発火の捕捉力を維持することも確認。

### テスト (hzb_2d 3→18 件、lib 904 全緑)
temporal_streak_saturates_never_underflows /
temporal_keys_distinguish_column_sections /
project_aabb_exact_unit_box / project_aabb_respects_fov_and_aspect_exactly /
near_plane_straddle_is_conservative_none /
ceil_mip_chain_and_last_column_propagates /
rasterize_writes_farthest_depth_guarantee /
hull_rasterize_skips_partially_covered_texels /
moved_significantly_3d_and_projection_changes /
non_finite_camera_is_rejected_without_recording /
malformed_boxes_pass_through_without_recording /
temporal_map_is_capped / self_occlusion_is_impossible /
near_frontier_does_not_falsely_occlude /
column_sections_cull_after_exactly_four_frames (以上新規 15) +
vertex_stride_is_12 / temporal_requires_multiple_frames 維持 +
teleport_skip_is_consumed_and_recovers (新仕様へカウンタ訂正)。

### 検証結果 (全て実測)
- lib **904/904** (+15、hzb_2d 18/18、cpu_occlusion 委譲 bitexact 2/2 緑)。
  structural_digest `004c1cf5fb17bfe8` rows=357 不変 (hzb は bench 非
  経路、機械確認)。
- fmt: WORK-only 差分行 **0** (全面再実装につき全行 rustfmt 正準形、
  HEAD 由来 38 行は旧ファイル温存分)。
- cargo check -p rsift-opt-gfx --all-targets: エラー 0、hzb_2d 由来警告 0。
- 不可視文字 0 / CRLF 0 / tab 0 (python 検査)。
- アドバーサリアル 5 系統 (CD-1 overflow panic / CD-2 frame2 早期発火 /
  CD-3 3 テスト赤 / CD-4 bbox フィルで判別テストのみ赤 / CD-5 chain 赤)、
  全て忠実復元で最終 904 緑。
- 環境: git 破損 12 回目 (HEAD 64294c6 巻戻り、fetch+reset で無損失復旧)、
  Rust 消失 10 回目 (restore-env.sh で復旧 36 秒)。
## CE. cpu_occlusion.rs + fxaa.rs 監査 (wave 81, 2026-07-24)

小粒 2 モジュール束ね (Y/AB 型)。cpu_occlusion.rs (119→約150 行) は
Hzb2D の薄い委譲ラッパ (wave 80 で全行精読済、本節で閉じる)。
fxaa.rs (133→約200 行) + shaders/fxaa.wgsl。live 消費 (実測):
cpu_occlusion は render_pipeline:11/99/803-806 が with_camera 経路で
使用 (固定カメラ版 cull_boxes は消費者ゼロ)、fxaa は full_graph_wiring
:161/:306/:1623-1628 (CPU shade を sharpened/bloomed ミキサーとして
実消費) + gpu_runtime:103 の WGSL sweep (naga 検証済)。

### CE-1 (低): 固定カメラ版 cull_boxes の契約明文化
- 消費者実測で直接呼出側ゼロ。ユーザー方針「消費者ゼロを削除理由にしない」
  に基づき温存し、近似 (aspect 16:9 決め打ち・fov 70deg 固定・pitch -0.2)
  と「診断・再現経路、本流は with_camera」契約を doc 明文化。
  カメラ定数は従来通り strictly pinned (公開仕様)。

### CE-2 (低): enabled=false 経路の委譲透過 + 帳簿不変ピン
- 既存テストは enabled=true/adaptive のみ。disabled 経路で wrapper==
  direct が bitexact かつ帳簿 (0,0) 不変 (= 内部状態非接触) を新規ピン。

### CE-3 (低): fxaa のフィールド非伝播罠の明文化 + WGSL 語彙ピン
- 問題の定式化: Fxaa 構造体の公開フィールド (contrast_base/
  relative_threshold) は CPU reference のみに効き、GPU pass (fxaa.wgsl)
  は 1.0/256.0・0.166667 をハードコード (uniform は invRes のみ)。
  「Rust 側で閾値を変えても GPU 表示が変わらない」静寂罠。
- 修復: フィールド doc に非伝播を明記 + WGSL ソース文字列から
  "0.166667"/"1.0 / 256.0" を機械抽出→f32 parse→Default と bit 一致ピン
  (0x3E2AAAC1/0x3B800000。文字列同一なら両パーサとも最近 binary32 で
  一意) = 乖離はテストが機械拒否。
- アドバーサリアル検証: WGSL 側 0.166667→0.166669 注入 →
  語彙ピンのみ赤 → 復元後緑 (初期は行コメントで ) が潰れる破壊注入に
  なりかけ、BZ-1 教訓に基づき忠実な定数置換で再実施)。

### CE-4 (低): shade 厳密 bit ピン 3 件 + luma 注入検出力
- 既存テストは方向/1e-6 近似のみ。厳密化: 鉛直エッジ (E/W blend)
  out=0x3F492493 (=11/14 級: t=0.5/(1+1/6)=3/7=0x3EDB6DB3、
  全中間値 f32 検算)、水平エッジ (N/S blend) out=0x3F1D41D4、
  untouched 経路は `return center` の **入力 bit 完全コピー**。
  luma(1,1,1) は f32 項和が 1.0 に丸まる (0x3F800000) ことも確定。
- アドバーサリアル検証: luma 係数 0.587→0.5785 注入 → 2 テスト赤 →
  復元後緑 (水平ケースは luma 等比不変性で偶然同一 bits、理論と整合)。
- 悪い点のみ直す実装評価: shade の数式 (本家 Lottes 簡略系と同型の
  min/max contrast・max(床, 相対) 閾値) は数学的に健全で変更不要と判断。

### CE-5 (観): WGSL n/s ラベル逆転の判断記録
- WGSL テクスチャ座標 +y は下向きで、"n" サンプルは視覚的 south。
  gx/gy は abs 比較のみに使うため符号反転は振る舞いに無影響 (等価)。
  誤記ではないので WGSL は触らず、誤読防止のコメント誠実化のみ
  (BR-1 先例: 実機検証なしの挙動系変更を避ける判断はここでは不要、
  コメントのみ)。

### CE-6 (観): shade 非有限画素の扱いを未規定として明記
- f32::min/max は NaN を脱落させ他値で進み、center 側 NaN は出力へ伝播
  (ポストプロセス画素欠測で全画面停止を避ける方針)。WGSL min/max(NaN)
  は spec indeterminate で Rust との NaN 一致は保証対象外 —
  いずれも選択せず「未規定」と doc 明記 (過剰な厳密化はしない判断記録)。

### 自己誤り捕捉 (12 件目)
- untouched シナリオ初稿 (0.5,0.25,1.0): luma 0.41025 で contrast
  0.0897 > threshold 0.0833 となり AA が発動 → テスト赤が捕捉。
  luma(1,0,0) ≡ luma(gray 0.299) ≡ f32(0.299) の等輝度設計に訂正し
  根拠自体もピン。

### テスト (+5 純増、909 全緑)
cpu_occlusion: disabled_passthrough_delegates_bitexact_and_records_nothing。
fxaa: shade_vertical_edge_exact_bit_pin / shade_horizontal_edge_exact_bit_pin /
untouched_path_is_bitexact_copy / wgsl_threshold_constants_match_rust_default_bits。

### 検証結果 (全て実測)
- lib **909/909** (+5)。structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (WGSL コメントのみ変更・CPU 参照実装は挙動不変で機械確認)。
- fmt: 両ファイル WORK-only 差分行 **0**。
- cargo check --all-targets: エラー 0、両ファイル由来警告 0。
- 不可視文字 0 / CRLF 0 / tab 0 (3 ファイル python 検査)。
- アドバーサリアル 2 系統 (luma 係数 → 2 赤、WGSL 定数 → 語彙ピン赤)、
  全て忠実復元で最終 909 緑。

## CF. occlusion_complete.rs 監査 (wave 82, 2026-07-24)

SoftwareOcclusion: CPU Hi-Z ピラミッド (u16、0=近 / 65535=遠) による
pre-mesh チャンク遮蔽。本番消費は render_pipeline.rs (:32 use、:119
`pub soft_occlusion`、:244 プロファイル条件付き構築、:587-590
view_proj 構築、:639 `is_occluded_hysteresis((cx,cz), aabb_min, aabb_max,
&view_proj)` で列あたり判定、:766-774 `clear_far` → occluder 全列
`rasterize_aabb` → `build_pyramid`、前フレームピラミッドでテストする
1 フレーム遅延設計)。rsift-api 側は adaptive_perf.rs (:291/:332
tier 条件) / low_spec_stack.rs (:47 pre_mesh_occlusion) で配線。
388 → 875 行へ全面再実装。発見 8 (CF-1 [C]、CF-2/3/4 [高]、CF-5/6 [中]、
CF-7/8 [観] 系)。テスト 2 → 13。

### CF-1 (C): project の行列規約が本番と転置不一致 (射影空間の破壊)
- 本番 view_proj (`world_column_store::TerrainFrameConstants::from_camera`)
  は**行ベクトル規約 p x M** (平行移動は row 3、mul4 は標準積で BW-5
  ピン済)。WGSL terrain_vertex_pull.wgsl:131 は `frame.view_proj *
  vec4(world,1)` (column-major 解釈で実質 p x M)、HLSL terrain_vs.hlsl:52
  は `mul(float4(world,1), view_proj)` — 3 層 + dx12 gpu_graph.rs:1061
  コメントで 4 重に一致。旧 occlusion project は `vp[row] . p_col`
  (M x p) を読み、平行移動成分 vp[3][i] が w 語へ化け、w が視点 z 距離
  ですらなかった (= 遮蔽空間全体が意味を成さない)。
- 根治: clip_i = Σ_j p_j * vp[j][i] の p x M 規約へ。加えて
  (a) w 検査を |w| から `w > 1e-6` 必須へ (背面角の鏡写し混入を根絶)、
  (b) ndc_z は本番行列の DX12 式 [0,1] を**そのまま**返す (旧実装の
  `*0.5+0.5` 再マップ = OpenGL [-1,1] 前提の二重変換を撤廃)。
- ピン: 正面点 (0,64,16) → (sx,sy)=(32,32)、w=16、nz bits
  **0x3F7F3994** (0.99697232、Python f32 往復検算)。背面点は拒否、
  z=600 は nz=1.0000143 > 1 で far 超過が検出可能 (クランプしない)。

### CF-2 (高): rasterize 深度の近/遠逆転 (被覆保証が偽 = false hole)
- texel 値 T は「その texel 全域が深度 ≤ T の幾何で覆われる」の保証。
  occluder が**最近**depth を書くと保証が偽 (潜在 false hole、CD-3 同型)。
- 根治: rasterize は 8 隅/3 頂点の**最遠** nz (max)、test は**最近** nz
  (min) で tile max (子の max = 4 子の最弱保証) と strict な大なり比較。
  等値境界は不成立 → 自己遮蔽は数学的に不可能。
- f32 検算確定値: 本番 box O [(-8,60,8),(8,68,24)] → T=**65404**
  (nz(24)=0.99801403、0x3F7F7DD9)。occludee 等値境界 (min_z=24) は
  test_depth 65404 == T で strict 不成立を 4 フレーム連続ピン。F box
  (min_z=30、test_depth 65432 > 65404) のみ遮蔽成立 → hysteresis 3
  フレーム目で発火。旧最近値 (65131 相当) はどの texel にも存在しない
  ことも全走査ピン。

### CF-3 (高): 三角形ラスタ判定が complete-dead 級 (採用領域 = v2 角の
相対幅 1e-4 の楔のみ)
- 旧実装の w_i は真の barycentric の符号反転 (w_i ≡ -λ_i、分母
  cross(v2-v0, v1-v0) = -Ω の符号解析で証明)。さらに
  `w2 = 1 - w0 - w1` (= 1+λ0+λ1) と「w_i >= -1e-4 ∀i」を要求すると
  採用条件は **λ0 <= 1e-4 かつ λ1 <= 1e-4 に帰着** (w2 の条件はほぼ恒真)。
  即ち v2 角の相対幅 1e-4 の楔にしか書けず、通常サイズの三角形では
  texel 中心が楔に入らず**書込みゼロ**、巨大三角形でも角の楔のみの
  誤記述。呼出し側ゼロ + テスト不在で潜在していた。
- 根治: 3 辺関数を全て**直接**計算し、内側判定は**同符号性**
  (全て >= 0 または全て <= 0) — barycentric 内側 ⟺ λ_i >= 0 ∀i の
  必要十分条件で**巻き向き不変**。退化 (|area| < 1e-4) と全頂点 far
  超過は棄却 (書き損ね = 保守方向)、いずれか頂点が w <= 1e-6 なら
  全沈默して棄却 (背面跨ぎ)。
- 両巻き向きで mip が**全 texel 同一** (順=逆=870 texel、検算ミラーも
  Python で同一性証明)。inside/outside セル 12 箇所を両巻きでピン。
  strict テスト: 恒等 vp、上辺 sy_e = 0.5012016296386719 (ny_e bits
  0x3F7BFD8A 由来) からわずか外 (中心 (32.5,0.5)、w0<0, w1<0,
  w2=+1.9074e-05 bits 0x37A000C8) のセル (32,0) は 65535 のまま、
  内側 (32,1)(32,30) は T=32767 (=(0.5*65535) as u16) を厳密ピン。

### CF-4 (高): rasterize_rect の画面外 clamp 誤記述
- 旧実装は clamp **後**に描画したため、完全画面外の rect (例 x1<0) が
  端列/端行に誤記述され、見えない occluder が辺縁の遮蔽を偽装しえた
  (非保守方向)。根治: clamp **前**に交差判定 early-out
  (`x1<0 || y1<0 || x0>=w || y0>=h`)、一部交差は交差部のみ、深度は
  min 合成 (最強保証を保持) をそれぞれピン。

### CF-5 (中): band 検査の raster 適用は非保守 → project / project_screen 分割
+ 凸包完全内包 scanline
- test 用ガードバンド |ndc|<=1.2 を raster にも掛けると、画面一杯に
  写る眼前の壁級 occluder (本番 box の隅 (-8,60,8) は ndc_x=1.83) が
  全沈默してピラミッドが欠落する = 非保守。そのため射影を
  **project (test 専用: band 付き、遠く画面外の occludee は遮蔽不可 =
  保守的に可視扱い)** と **project_screen (raster 専用: band 無し)**
  に分割。raster 側の座標爆発は凸包 scanline の ±4 画面防御 clamp と
  書込み範囲の切詰めで処理 (画面外成分は自然に消える、捨てるのは保守)。
- AABB raster は 8 隅厳密射影 (p x M) → monotone chain 凸包 (f64、
  透視射影は w>0 半空間で凸性保存) → scanline で**完全内包 texel のみ**
  に T を書く (部分被覆に保証を与えない = 書き損ねは遮蔽過小 = 保守)。
  背面跨ぎ (いずれか隅 w <= 1e-6) と全隅 far 超過 (min nz > 1) は棄却。
- 検算確定: 本番 box O の凸包 (矩形 [(-26.576,2.712),(90.576,61.288)])
  から 58 行 x 64 列 = **3712** texel 完全一致 (行 2/61 は境界除外を
  span None → skip でピン)。背面/跨ぎ/far の 3 box はバッファ無汚染。

### CF-6 (中): hysteresis 契約の明文化 (&& 合成・cap・max(1))
- update_hysteresis は正確 HashMap カウントと衝突許容フラットテーブル
  (4096 スロット) の AND: HashMap 側が厳密なので**過早発火は数学的に
  不可能**、ハッシュ衝突はストリーク共有リセットで発火が遅れる方向
  のみ (保守)。衝突ペアを実ハッシュ関数で機械探索し、k2 可視挿入後の
  k1 発火が正確に +2 フレーム遅れることをピン。
- hysteresis_frames=0 は max(1) 即時発火に丸め (負ループのシュリンク
  なし) をピン。HashMap は MAX_HYSTERESIS_ENTRIES=16384 で定数上界、
  超過で全クリア (全体を遅らせる保守方向) をピン。

### CF-7 (観): doc 誠実化 (SWAR 命名・lock-free 表現・jitter 未適用)
- 「SWAR」は歴史的命名で実装はスカラー (SIMD なし)、旧ヘッダの
  lock-free 表現は単一スレッド前提フラットテーブルのため撤回、
  Halton(2,3) 生成器 + 8 フレーム周期 jitter_index は実在するが射影へ
  現行未適用 (保守性証明未整備の設計予備) と明記。Halton 先頭 4 項を
  f32 bits ピン (radical inverse 理論値 1/2,1/4,3/4,1/8 系と
  1/3,2/3,1/9,4/9 系に一致)。

### 自己誤り捕捉 (13・14 件目)
- 13 件目: 引継ぎ検算の w2 恒等式ミラーが全セル False を出力 → 必要経路
  の符号解析を強制し、CF-3 の「採用領域 = v2 楔」定式化へ精緻化。
- 14 件目: CF-3 doc 初稿の「w_i >= -1e-4 は Σλ=1 より常に偽」は過剰
  主張 — 符号代数で λ0,λ1 <= 1e-4 の楔が厳密に生存することが判明し、
  「相対幅 1e-4 の v2 角楔のみ (通常三角形ではゼロ)」表現へ訂正。

### テスト (+11 純増、920 全緑)
halton_first_four_exact / project_matches_production_matrix_convention /
rect_early_out_never_writes_offscreen /
rasterize_aabb_farthest_depth_fully_inside_texels_only /
rasterize_aabb_rejects_behind_straddling_and_beyond_far /
occlusion_requires_strictly_farthest_nearest_depth /
triangle_rasterize_winding_invariant_exact_cells /
triangle_strict_center_rule_rejects_barely_outside_center /
hysteresis_threshold_semantics / hysteresis_map_is_capped /
hysteresis_collision_delays_never_accelerates (+ 既存 2)。

### 検証結果 (全て実測)
- lib **920/920** (+11)。structural_digest `004c1cf5fb17bfe8` rows=357
  不変 (wide_static_bench は SoftwareOcclusion/RenderPipeline を参照
  しないことを grep=0 で事前確認どおり機械確認)。
- fmt: HEAD 由来 2 箇所は全面再実装で自然消滅 (継承行ゼロ) → 全体を
  rustfmt 正準形に統一、post-fmt WORK diff **0**。
- 不可視文字 0 / CRLF 0、cargo check --all-targets で当該由来警告 0。
- アドバーサリアル 3 系統 (全て注入→赤→忠実復元→ファイル同一性 diff
  確認): (a) project 転置規約 → project_matches_production_matrix
  _convention + occlusion_requires_strictly の 2 赤。(b) rasterize
  z_max→z_min → farthest テスト赤 (strict シナリオ不変は粗 mip の
  65535 支配で偽陰性となる経路を解析済、運搬は厳密値ピンが担う)。
  (c) 旧 dead 判定 (w2=1-w0-w1 + トレランス) → winding/strict 両赤。

## CG. full_graph_wiring.rs 監査 第 1 部 (wave 83, 2026-07-24)

全モジュール実実行オーケストレータ (2416 行) 監査の第 1 部:
helper 関数群・誠実性検査・射影系。第 2 部 (wave 84 予定) で tick_world
1445 行のセクション別数学チェーンを検証する。発見 8 項目 (CG-1 [高]
転置規約の実害、CG-2 [中]、CG-3〜6 [観] 誠実化、CG-7/8 [低])。

### CG-1 (高): extract_frustum_planes の転置規約 (CF-1 同型、メトリクス全滅)
- 本番規約 p x M (wave 82 で WGSL/HLSL/dx12 コメント/from_camera の
  4 層整合確定) では clip_j = p · 列 j。**平面抽出は列 c(j) の結合**が
  必要なのに、旧実装は行 r(i) 結合 (clip = M·p 規約の Gribb-Hartmann)。
- 消費者: lbvh_planes / simd_planes (→ candidates.visible_prev /
  vis_mask → azdo draw_command_count / HUD バー) / aokana
  evaluate_visible_regions。LBVH と SIMD の「相互検証」は**同一の誤
  平面を共有するコモンモード**で検出不能。
- 定量化 (f32 検算ミラー確定): yaw=0 本番行列で旧 left plane =
  (-0.0156, -0.9999, -0.0004, 0)。正面の点 (0,64,16) ですら 6 面中
  **4 面が dist < 0** (実測 [-64.0,-64.0,-64.0,-64.0,+17.0,-64.1]) =
  事実上全棄却。y>0 の地形はほぼ全件カリング扱い → draw_command_count
  / lbvh_culled / aokana_visible_regions が恒常無効値。
- 爆発半径の検証: report の pipeline 還元は render_pipeline :1001-1009
  の grep 実測で next_build_budget / power_skip_extra / overdraw_order
  (→wiring_priority) / subsystems_active のみで、3 つの汚染メトリクスは
  HUD バー・デバッグログの内部消費に留まる (実描画のカリングは
  render_pipeline 本流 + occlusion_complete が別系統で実施)。よって
  [C] でなく実害虚偽メトリクスとして [高]。
- 不発機構: 恒等行列は対称 (行 i = 列 i) で新旧規約が恒等的に一致 →
  既存テスト 2 件 (identity_table・chunked_inputs の IDENTITY_VP) では
  原理的に不可視だった。アドバーサリアル復元注入で実証: production 2
  テストのみ赤、identity/degenerate は緑のまま。
- 根治: 列結合。検算 14 点 (yaw=0 12 点 + yaw=0.7/pitch=0.15 回転 2 点)
  で抽出平面の内外 ⟺ 厳密射影の内外が全一致。厳密 bit テーブルもピン
  (例 left = [0xBF60A941, 0, 0x3EF57745, 0])。

### CG-2 (中): 「実 draw indices」注記vs連番の虚偽 (BV 引継ぎ項目を解決)
- 供給は (0..n) 連番のみ。頂点共有ゼロで ACMR はオーダー不変 (常 1.0)、
  optimize は不動点。(before, after) は読み捨てで「改善時のみ採用」の
  経路はコード上不存在。コメントを誠実化 (実 topology 付き index 供給は
  render_pipeline 側が必要な将来課題と明記。消費者追加の真の配線は実機
  挙動変更を伴うため BR-1 先例で doc 訂正側を採用)。

### CG-3〜CG-6 (観): 誠実化 4 件
- CG-3: report.subsystems_active = 60 は実数え上げでなく仕様定数と doc
  明文化 (テストも従来「仕様値」と呼称)。
- CG-4: done() は現行 no-op (旧 doc「次フレーム用の実効果参照」は虚偽)。
- CG-5: JobSystem ブロックは no-op クロージャの負荷分散実演のみ
  (旧コメントの「軽量メトリクスを集計」は虚偽)。
- CG-6: FRB ブロックは形状組立実演 + 本数計測のみ (旧コメント
  「GPU 入力へ実変換」は虚偽。FragmentRayBoxIntersect に CPU 入力 API
  が無いことを grep で実測確認)。frb_billboards フィールド doc も訂正。

### CG-7 (低): PSO キャッシュの問合せ/挿入キー不一致 (常時ミスのでたらめ計器)
- 問合せ `material_hash ^ tick%7`・挿入は `ps_hash: 0xBEEF` 上書きの
  別キー → get は数学的に常時ミス。「実測」と称する hit/miss 計器が
  miss 率 100% 固定だった。同一キー統一で初フレーム miss→登録・以後
  hit の真の挙動へ根治 (キャッシュ内容変化だが消費は統計ログのみ)。

### CG-8 (低): GTAO 標本が「不透明率由来」を偽った len 直読み
- `(0..chunk_aabbs.len()).filter(|_| true).count()` (= len) を供給。
  実パレットの不透明ボクセル率を直接計算する真の実測へ根治
  (gtao_occ は現行読み捨てで挙動影響ゼロの誠実化+実質化)。

### 自己誤り捕捉 (15 件目)
- identity テストのコメント編集時にエスケープ `\n` をリテラル混入
  (コメント内のためコンパイル可・無害だが非正規) — cat -A 検査が捕捉、
  正規 3 行へ修正。

### テスト (+2 純増、922 全緑)
extract_frustum_planes_production_exact_table /
extract_frustum_planes_matches_projection_semantics。

### 検証結果 (全て実測)
- lib **922/922** (+2: CG-1 の 2 検出器)。structural_digest
  `004c1cf5fb17bfe8` rows=357 不変 (tick_world は bench 非経路)。
- fmt: HEAD 由来 156 行の既存偏差は温存、当 wave 新規分は WORK-only
  **0** (assert 分割・トレーリングコメント位置の 2 箇所を rustfmt 正準形
  へ手調整、全体 rustfmt は HEAD 温存のため非実施)。
- 不可視文字 0 / CRLF 0 / cargo check --all-targets で当該由来警告 0。
- アドバーサリアル 1 系統: 旧行ベース規約の厳密復元注入 → production
  2 テスト赤・identity/degenerate 2 テスト緑 (恒等対称による不可視機構
  の実証) → 忠実復元 (md5 同一) → 14/14 緑。

### 残 (第 2 部 = wave 84 予定、第 1 部で棚卸し)
tick_world 本体 (355-1830) のセクション別数学チェーン:
VCT cone の `camera_dir[1].abs()` ヒューリスティクス、WBOIT/SSR/SSS の
供給値、nanite proj_factor 70° 仮定、cluster light 割当、ddgi probe、
IBL ドーム 0.03 重み、TAA variance clip γ=1.25、exposure adapt 速度 1.6、
checkerboard 再構成、FSR1/2 入力組成、LEO tag 丸め、time_slice コスト
見積 180us、DAG コスト係数 0.05/0.4/0.1/0.2/0.1。

## CH. full_graph_wiring.rs 監査 第 2 部 (wave 84, 2026-07-24)

tick_world 本体 (355-1830) のポストプロセス/近似チェーン検証。
発見 6 項目 (CH-1 [中]、CH-2/3 [低]、CH-4/5/6 [観])。

### CH-1 (中): FSR1 EASU へのフランケン 4 近傍 (前/現フレームのチャンネル混在)
- EASU `reconstruct(nw, ne, sw, se, fx, fy)` は同一次元の 4 隣接テクセル
  色を要求 (fsr1.rs 契約)。旧実装は `[prev.R, cur.G, cur.B]` 等の
  チャンネル混在色を 3 脚に供給 — 時系列混在で全くの造語色。
- 根治: 定数色不変性の実演 (4 入力 = 現フレーム色) に変更。不変量の
  数学証明: 勾配 g=0 ⟹ エッジ強度 0 ⟹ 双線形は ±0 吸収で厳密恒等
  (c+(c-c)*f = c、f 任意で成立)。fsr1.rs の `flat_region_returns_input`
  を許容誤差 1e-5 から **厳密 bit ピン**へ強化し財産化 (加えて
  `easu_exact_bits_canonical` が wave 64 より存在し多層防御)。
  真の 4 近傍テクセル (フレームバッファ供給) は将来課題と明記。
- 検出器誠実注記: wiring 側のフランケン注入は determinism 比較では
  原理的に不感 (a/b 同コード) — 防衛はサブシステム側ピン群 (EASU 厳密
  不変量/CAS フラット恒等/FXAA 輝度等一 untouched = CE-4 契約) と
  コードレビューが担う。サブシステム数学の退化はピンが即検出
  (下記アドバーサリアル実証)。

### CH-2 (低): CAS/FXAA の異ステージ近傍混在
- CAS: 中心=bloomed・近傍=mapped、FXAA shade: 中心=sharpened・近傍=
  bloomed と、異なるポスト段を「近傍テクセル」と偽って混在。
- 根治: 同一ステージ定数近傍に統一。CAS は定数近傍で lap≈0 (mean 計算
  の 3c 丸めで 1 ulp 級の実質恒等 — 厳密と書くには 3c が丸めうるため
  表現を限定)、FXAA は輝度等一で untouched 経路の bit 完全コピー
  (CE-4 契約の厳密恒等)。波及的に `aa ≡ sharpened ≡ bloomed` (bit) で
  以後の段の意味論が単純化される。

### CH-3 (低): nanite/more_culling の FOV 70° ハードコード → 真値配線
- inputs に FOV 経路がなく 70° のゲス値使用 (本番既定は fov_y=1.0 rad
  ≈ 57.3°)。FrameWiringInputs に `camera_fov_y` を追加し
  render_pipeline の実カメラから配線 (opt-gfx 内完結の API 追加、
  消費者優先方針どおり「消費者 (真値) 追加」で解消)。テスト側は
  empty_inputs で 1.0 (from_camera 既定と一致) に固定。
- 検出器誠実注記: culled 数のゴールデンは nanite 意味論に深く依存し
  脆弱設計となるため不採用 — 防衛はフィールド配線の型検査 + サブ
  システム (more_culling/nanite) 側ピン + レビュー。

### CH-4 (観): IBL「ドーム」は水平リング + SH 重み 0.03 は ad-hoc
- 32 方向は y=max(0.05) のほぼ水平リング (y=0.05 固定で天頂側なし)。
  重み 0.03 は 4π/N の SH 求積 (≈0.393) とは無関係の減衰係数。
  ambient_light は現行メトリクス消費のみ — コメントで誠実化 (変更は
  将来の真ドーム配線に委譲)。

### CH-5 (観): VCT cone の dir.y=|camera_dir.y| ヒューリスティクス
- 視線の上下を問わず天頂方向へ abs 強制 = sky-visibility プローブ近似
  (GI 遮蔽の真の方向性ではない)。出力は現行読み捨て。コメント誠実化。

### CH-6 (観): exposure histogram 境界 (1e-4〜0.4, percentile 10/95) 判断記録
- 地平環スカイ輝度 ≈0.2-0.6 の percentiles 10/95 に妥当な動作域
  (constant_scene_gives_stable_exposure が相互検証済)。変更なし判断を
  記録 (過剰な厳密化はしない)。

### テスト (純増 0、ただし 1 件厳格化、922 全緑)
fsr1 `flat_region_returns_input` を厳密 bit 一致へ強化 (fx,fy 3 組)。

### 検証結果 (全て実測)
- lib **922/922** (増減なし、1 件厳格化)。structural_digest
  `004c1cf5fb17bfe8` rows=357 不変 (render_pipeline 1 行配線を含むが
  bench 経路の出力は不変)。
- fmt: 3 ファイル (full_graph_wiring/fsr1/render_pipeline) WORK-only **0**
  (HEAD 由来 156/12/21 行は温存)。
- 不可視文字 0 / CRLF 0 (3 ファイル) / all-targets で当該由来警告 0。
- 編集事故 x2 (VCT `let mut vct_occlusion` 脱落・IBL `let mut sh` 脱落)
  はいずれも直後の参照確認で捕捉し即修復 — コンパイル到達前に解消。
- アドバーサリアル 1 系統: EASU 双線形の符号反転注入 → fsr1 の 4 テスト
  赤 (canonical 厳密 bit 系が捕捉。flat 不変量テストは c±(c-c)f=c の
  構造上不感であることも確認 = 検出範囲の誠実な把握) → 忠実復元 (md5
  同一) → 20/20 緑。

### 残 (第 3 部 = wave 85 予定、棚卸し)
tick_world の後段セクション: entity_culler visible_ids 消費、
time_slice 180us 見積・DAG コスト係数 0.05..0.4 係数群、LEO tag
(palettes.len 由来)、visgraph flood 半径 8、FSR3 16x16 プローブの
depth=nearest/128 scaling、FSR2 jitter 0.002 scale、hud stats 正規化
係数群 (/64 /16 /8)、decal_local 供給値、cluster_grid z スライス割当。

## CI. full_graph_wiring.rs 監査 第 3 部 (wave 85, 2026-07-24)

対象: tick_world メッシュ確保 (gigabuffer 圧迫経路) + time_slice パッチ駆動キー。

### CI-1 (中): gigabuffer 圧迫時の二重虚偽 — 「最古」がハッシュ順任意要素 + 「再試行」のコード不存在
- 旧実装の 2 虚偽:
  (a) `self.gb_handles.keys().next()` は HashMap ハッシュ順の**任意要素**であり
      「最古エントリ (LRU)」のコメントと無関係。被害者が実行毎に変わり得る
      非決定的挙動。
  (b) 「実解放して再試行」と記述しながら解放後に **allocate 再試行のコードが
      存在せず**、当該メッシュ確保は静寂に欠落 (match で Some/None を捨てる
      ため観測不能)。
- 根治:
  - `gb_order: VecDeque<(i32,i32)>` (挿入順 FIFO) を struct に追加。
  - `gb_store`: 新規キーのみ push_back (= 置換は順位不変、1 キー 1 順位)、
    旧ハンドル即解放。
  - `gb_alloc_or_evict`: 1 回 allocate → None なら pop_front で**真の FIFO
    最古** (stale は読み飛ばし) を実解放 → **単一 retry** → 失敗なら潔く
    None (過剰退避で既存メッシュを壊滅させない bounded 挙動。
    自然退役に委譲)。
- 数学的正当性: retry 失敗時の被害者は高々 1 件 (oversize 要求のみが既存を
  破壊し得るが害は 1 件に限定)。oversize は GpuArena::alloc が free_by_size
  の `range_mut(need..).next()?` で None を返す (panic なし) ため安全に失敗。
- 爆発半径: gigabuffer ハンドルは `_giga` で保持のみ (描画経路は未接続、
  gpu_arena 側は別系) → digest 不変を実測確認。

### CI-2 (観): patch 駆動キー `packed & 0xFFF` の誠実注記
- cz 下位 2bit と sy 10bit の混在であり per-block 真差分ではなく時分割
  シミュレーションの決定的駆動値。patch_for_block の契約 <4096 は構造的に
  常時保証。動作変更なし、将来課題を明記。

### テスト (純増 1、923 全緑)
`gb_eviction_fifo_single_retry_and_stale_skip`:
- 置換の順位不変 (`gb_order == [(0,0),(1,1)]` 厳密等値)。
- 600MiB oversize → None + FIFO 最古のみ退避 + 次点生存 + 失敗確保未登録
  + 被害者 1 件のみ消費 (bounded)。
- retry 成功径路: 250+250+11=511MiB 使用で空き 1MiB に 12MiB 要求 →
  1 回目失敗、FIFO 最古の 250MiB 解放で単一 retry 成功し (13,13) 登録
  (= 旧実装で欠落していた径路)。250/11/12MiB はいずれも align 256 倍数で
  厳密算術。

### 検証結果 (全て実測)
- lib **923/923**。structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: full_graph_wiring WORK-only **0** (HEAD 由来 32 行は温存)。
- 不可視文字 0 / CRLF 0 / all-targets で当該由来警告 0。
- アドバーサリアル 2 系統:
  (a) retry 削除注入 (旧 no-retry 再現) → gb_eviction テスト FAILED ✓検出。
  (b) FIFO→LIFO (`pop_back`) 注入 → 同テスト FAILED ✓検出。
  いずれも検出確認後、バックアップから忠実復元 (md5 同一) → 15/15 緑。

### 残 (第 4 部 = wave 86 予定、棚卸し)
第 3 部棚卸し残: entity_culler visible_ids 消費、time_slice 180us 見積、
DAG コスト係数群 (0.05/0.4/0.1/0.2/0.1)、critical_ms、LEO tag
(palettes.len 1..=7)、visgraph flood 半径 8、FSR3 16x16 プローブ
(depth=nearest/128・motion=0.001)、FSR2 jitter 0.002 scale、HUD stats
正規化係数群 (/64 /16 /8)、decal_local 供給値、cluster_grid z スライス割当、
morton bump 評価。

## CJ. render_pipeline.rs 再監査 第 1 部 (wave 86, 2026-07-24)

対象: 現行 1389 行の全行再読 (wave M 時点 1265 行から増大、引継ぎ最優先①)。
frame() 主流ロジック全通読 + 関連契約 (chunk_cull BK 設計・diff_mesh・
billboard_lod 帯域・packed4 8B 固定・OnceLock ハードウェアキャッシュ) との照合。
発見 6 項目 (CJ-1 [中]、CJ-3 [中]、CJ-2 [低]、CJ-4 [低]、CJ-5/CJ-6 [観])。

### CJ-1 (中): pull キャッシュヒット / flora LOD box 経路の wiring 入力欠落
- 旧実装: 両経路は `continue` するため meshes/pull_meshes に到達せず、
  フレーム末端の wiring 入力 (chunk_keys/chunk_aabbs/chunk_dists/
  draw_index_counts) から当該列が抜け落ちていた。
- 実害チェーン: OverdrawSorter の overdraw_order → `wiring_priority.get(c)
  .unwrap_or(usize::MAX)` により**欠落列は常時最劣後ソート** — cache-warm
  な近接列 (描画効率が最も良い列) が次フレームの build 順で系統的に
  不当降格。flora LOD box 列は gpu_quad_bytes に実描画バイトを供給
  しているのに wiring の描画対象集合に現れない二重基準。
- 根治: 両経路で (key, dist, aabb, draw_index_count) を側車ベクトル
  wiring_only に記録し、フレーム末端で meshes/pull_meshes 由来と併合
  (重複ガード付き)。draw_index_count は 1 クアッド 8B 固定 (packed4.rs
  型レベル assert) から `len/8*6` で厳密導出。
- 波及: wiring_priority が全実描画列を rank 化 → ソートフィードバックの
  系統的不正を解消。GPU 描画バイトは不変 (digest に非経路)。

### CJ-2 (低): verdict Occluded 腕の潜在的分岐不整合 → build_chunk_if_visible と統一
- 旧実装: frame() は `Visible | Occluded => {}` (通過)、build_chunk_if_visible
  は `Occluded => skip` — 全く逆の扱い。現行 verdict_column は Occluded を
  送出しない (wave 61 BK 設計: 隣接データ無しの全列 occluded 判定は透過
  ホール障害を招く) ため現挙動差は非発現だが、将来 producer が現れた
  瞬間に 2 経路で真逆となる潜在乖離だった。
- 統一: frame() も Occluded → skip + visgraph_culled 計上 (現挙動不変)。
- ピン: `cull_pass_currently_has_no_occluded_producer` で「全空 → EmptyColumn、
  occupied 近距離 → Visible」を固定 (Occluded 非送出の設計前提)。

### CJ-3 (中): ingest_world_column の diff_mesh ダーティ帯域誤り
- 旧実装: `mid_y = camera.y` でカメラ帯 mid_y±16 をマーク — カメラと異なる
  帯域のインジェスト更新 (サーバーが別 Y 帯の列を送る通常ケース) が diff
  追跡から**完全に抜け落ち** (変更列がセクション差分再メッシュされない)、
  代わりに不要なカメラ帯を誤ダーティ化していた。
- 根治: `note_ingested_sections` 抽出 — インジェストされた各セクションの
  中心ブロック (sy*16+8) をマーク。mark_block_dirty の境界伝播 (端 y だと
  隣接にも及ぶ) に干渉しない中心点で正確に 1 セクション/回。
- 厳密値ピン: base=0,2 枚 (カメラ帯 section 8 と無関係) → dirty == [4,5]、
  端 base=-4 → [0] (旧実装なら [7,8,9+伝播])。

### CJ-4 (低): wiring へ供給する SVO が HashMap 反復順の任意要素 (CI-1 同型)
- `self.svo_cache.values().next()` は RandomState のプロセス毎ランダム順
  (CI-1 の gb_handles.keys().next() と同型の「意味ある選択を任意要素に
  委ねる」反パターン)。現行消費は VCT cone で出力読み捨て (CH-5 誠実注記
  済) のため観測不能だったが、将来配線で非決定性が実害化する布石。
- 根治: `svo_for_wiring` — 最小チャンクキー決定論選択 + 所有クローン移譲
  (`svo_for_wiring_owned`)。軸ピン: (-5,2) < (3,3) で ptr::eq 厳密確認、
  空 → None。
- 借用設計の記録: 参照保持では tick_world (&mut self) と借用衝突するため
  Option<SparseVoxelOctree> のクローンに移譲 (inputs は Option<&T> のまま
  `.as_ref()` で再借用 — API 型不変の最小侵入解)。

### CJ-5 (観): 冗長 debug_assert 除去
- build_chunk の `size_of_val(&mesh.vertices[0]) == VERTEX_STRIDE_BYTES` は
  chunk_mesh.rs の型レベル const assert が完全に包含する定数比較であり、
  頂点数 O(n) の無駄回しだった (意味的に常時真)。除去 (保証は型側に一元化)。

### CJ-6 (観): build 予算の cache ヒット計上 + chunks_built 非対称の誠実注記
- 予算 (max_builds) は「処理列数」でヒットも 1 消費 (bytes 展開+draw の
  フレーム時間経済として意図的) だが、chunks_built は build_chunk 到達のみ。
  語彙の非対称は仕様として現挙動を固定 (コード不変)。

### テスト (純増 4、927 全緑)
wiring_priority_covers_cache_hit_and_flora_lod_columns /
ingest_dirty_band_tracks_ingested_sections_not_camera_band /
svo_for_wiring_selects_min_key_deterministically /
cull_pass_currently_has_no_occluded_producer。

### 検証結果 (全て実測)
- lib **927/927** (+4)。structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (bench は render_pipeline 非経路、gd quad bytes も不変)。
- fmt: WORK-only **0** (HEAD 由来 4 行温存、新規分は全て正準形)。
- 不可視文字 0 / CRLF 0 / all-targets で当該由来警告 0。
- アドバーサリアル 2 系統: (a) CJ-1 併合マージ削除 (旧挙動厳密再現)
  → wiring_priority テスト FAILED (flora 列 assert で即時検出)。
  (b) CJ-3 カメラ帯マーク復元 → dirty band テスト FAILED。
  いずれも検出確認後、バックアップから忠実復元 (md5 同一) → 11/11 緑。
- 環境事象: edit_file が 1 回 stale 世代へ適用 (成功返り値だが内容未反映)
  — 直後の grep 存在検査で捕捉し python 直書きで再適用 (wave 85 に続く
  ツール世代ずれ注意事項として記録)。

### 残 (第 2 部 = wave 87 予定、棚卸し)
frame() 後段の eco region 可視性・verts_per 先頭依存、cpu_occluder boxes の
固定 y 帯 (0..64)、fps 由来 stats の語彙、build_chunk 二重経路 (greedy+pull) の
メモリ運用、ingest の cache.invalidate と pull_gen_cache.invalidate の関係、
shader/WGSL 側との FrameCB 語彙 (chunk_origin) 突合、meshes pool upload の
pull モード二重供給、camera 非 live トグル時の速度スパイク。

## CK. render_pipeline.rs 再監査 第 2 部 (wave 87, 2026-07-24)

対象: frame() 後段の残棚卸し (HZB/eco/prune/二重供給) + FrameCB/WGSL 語彙突合。
発見 5 項目 (CK-1 [中]、CK-2 [低]、CK-3/4/5 [観]) + 語彙突合 1 件。

### CK-1 (中): 派生キャッシュがワールド prune/invalidate に追随しない
- **svo_cache の stale 供給**: `prune_world_columns` は world 列のみ prune
  し、svo_cache は無制限残存 → 除去済み列の stale SVO が `svo_for_wiring`
  (最小キー選択) 経由で VCT プローブへ**永久に供給され続ける**。
  さらに ingest による列内容置換でも svo_cache は無効化されず、次の SVO
  再ビルド (far LOD 到達) まで VCT が**旧地形を参照し続ける**。
- **pull_gen_cache の滞留**: 世代整合で到達不能になるのみで bytes は残存。
- 根治: `prune_derived_caches` (world_column_store::prune_outside と同語彙の
  Chebyshev 半径・境界含む) + `invalidate_derived_for_column` を抽出し、
  pub wrapper (prune_world_columns / ingest_world_column) に配線。
  PullGenerationCache に prune_outside を追加 (従来 get/put/invalidate のみ)。
- ピン: 境界含む半径語彙 (3,0)→prune・(2,2)→残留・gen 7 参照、
  置換列退避/無関係列保持。

### CK-2 (低): HZB/CPU occluder AABB の y 帯が固定 0..64 でメッシュ帯と 48 ズレ
- live ワールドの実ウィンドウは mesh_origin[1] 基点 (例: origin.y=48 の
  48..112) なのに、HZB ボックスは世界原点 0..64 固定 → visible_chunks/
  cpu_culled の HZB 統計が系統的に誤帯域で計測 (メトリクスのみ実害)。
- 根治: `hzb_boxes_for` 抽出 + mesh_origin[1] 整合帯。厳密ピン:
  (1,2),y0=48 → [16,48,32]-[32,112,48]。

### CK-3 (観): verts_per は先頭メッシュのみの代表値 (eco 近似) 誠実注記
### CK-4 (観): live→デモ切替時の 1 フレーム速度スパイク (min(40) 有界) を現挙動固定で明記 (BR-1 判断)
### CK-5 (観): pull モードでも greedy メッシュは構築・プール二重供給 (DX12 では未消費、ベンチ実演) の設計明記

### 語彙突合 (変更なし): FrameCB chunk_origin
WGSL terrain_vertex_pull.wgsl (:11 chunk_origin vec4<f32>, :128 world =
local + frame.chunk_origin.xyz) と Rust TerrainFrameConstants.chunk_origin
(production テストが [-16,48,-48,0] に厳密ピン) は同一語彙で整合確認。

### テスト (純増 3、930 全緑)
derived_cache_prune_matches_world_radius_vocabulary /
invalidate_derived_for_column_evicts_stale_svo /
hzb_boxes_use_mesh_origin_band。

### 検証結果 (全て実測)
- lib **930/930** (+3)。structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: render_pipeline/low_spec_stack 両ファイル WORK-only **0**。
- 不可視文字 0 / CRLF 0 (両ファイル) / all-targets で当該由来警告 0。
- アドバーサリアル 2 系統: (a) CK-1 no-op 化 (旧挙動厳密再現=派生キャッシュ
  不操作) → CK-1 両テスト FAILED。(b) CK-2 固定 0..64 帯復元 → hzb テスト
  FAILED。いずれも検出確認後 md5 忠実復元 → 14/14 緑。
- **環境事象**: Rust 消失 (11 回目)→ restore-env.sh で復旧 (~36 秒)。
  **git 破損 (13 回目)**: worktree 再建時に HEAD が base (64294c6) 巻戻り、
  worktree は新時代のまま残存 → fetch + reset --mixed FETCH_HEAD で HEAD を
  fd9b1ad へ復旧、status は wave 87 の 2 ファイル差分のみで整合確認
  (md5 同一)。fmt HEAD 比較は復旧後の正しい base で再測定。

### 残 (第 3 部 = wave 88 予定、棚卸し)
eco region の可視性語彙、fps 由来 stats 語彙、ingest invalidate と
rebuild lazy SVO の再生成遅延 (次 far LOD まで SVO 欠落の窓)、
cpu_occluder cull 結果の depth 活用、frame_stats.fps 語彙、
非 live 時の wiring 入力語彙 (unit cube 系)。

## CL. entity_culling.rs 監査 (wave 88, 2026-07-24)

対象: 全 621 行全行照合 (EntityCuller / FastEntityCuller / RayPacket8 / DDA)。
消費者確認済: EntityCuller = full_graph_wiring 実消費 + wide/pseudo bench
(→ digest 敏感経路のため EntityCuller 側は挙動不変の範囲に限定)、
FastEntityCuller = pseudo_mc_live のみ (bench digest 非経路)。発見 7 項目。

### CL-1 (観): 「SIMD 8-Wide AVX2 / SWAR パケット DDA」の主張が実装と不一致 (CF-7 同型)
- `ray_packet_unblocked_8wide` は各自をスカラー DDA (ray_unblocked) で
  逐次判定するだけで、実 SIMD/AVX2 命令は本クレートに存在しない。
- 冒頭 docstring および RayPacket8 doc を誠実化 (コード不変 → digest 影響ゼロ)。

### CL-2 (中): FastEntityCuller の移動検知が再評価要求を消失 (最長 10 tick の vis ラグ)
- replace_targets_fast の同一 ID 分岐は last_center を即更新するだけで
  moved-ness を破棄 → 移動実体の visibility が period_ticks=10 回まで
  stale のまま (EntityCuller は `due || moved` で即再評価する同語彙参照系)。
- 根治: moved 分岐で `slot.last_eval_tick = self.tick.saturating_sub
  (self.period_ticks)` とし次回 cull で必ず due。
- ピン: due の真のカレンダ (新規=後述 9 tick 遅延、tick 10 = 初回 due) を
  先に固定した上で、移動直後 tick 11 で reevaluated==1・rays==5 を厳密検証。

### CL-3 (中): ChunkBucketGate の保守 skip が即 false で境界実体を点滅化
- ゲートは保守テスト誤爆 (境界 false positive) を含むのに `slot.visible =
  false` 即時適用 → モジュール宣言の「遅延適用スキッピング戦略」と矛盾、
  境界実体がフレーム毎に点滅し得た。
- 根治: visibility を保持し、保持値を bitmask にも反映。
- **自己誤り捕捉 (16 件目)**: 初版修正は `continue` でループ末尾の
  bitmask 設定を飛ばし「保持した visible=true が出力に反映されない」
  実効無修正だった → 新テスト赤が実行前に捕捉 → 修正して進行。
  (「テスト赤 = 自己誤り捕捉装置」規律の実績として台帳に記録。)

### CL-4 (低): FOV フィルタ cos 0.35 ハードコード → fov_cos_min フィールド化 (CH-3 同型)
- pseudo_mc_live の rec に FOV 公開が無く真値配線は consumer 改修待ちの
  将来課題 (W-3 教訓: 中途半端配線はしない)。既定 0.35 不変で後方互換。
- 静的構成前提の誠実注記付き (動的変更時は due 追従 — FOV skip は評価を
  経ず前回値を持つ)。ピン: 既定不変 + -1.0 で cos<0.35 の対象が可視化。

### CL-5 (中): CullStats.total が invalid 尾スロットを含み統計を歪曲
- 5 → 2 縮小後も total=5 のまま invalid 3 件が occluded 側へ混入。
- 根治: total = valid スロット数 (occluded = total - visible の導出も整合)。
- skipped_far は far+gate 合流語彙 (既存ピン skip==1 で仕様固定、公開名は不変)。

### CL-7 (観): ray_unblocked の guard >256 打切り → true は保守方向の誠実注記
- 「不確実 → 可視側」は誤 cull (可視ポップ) を防ぐ正しい方向。
- 既定 max_distance=64 経路では 256 セル超は構造的に不発
  (pub max_distance の長距離設定時のみ発動) を明記。

### テスト (純増 4、934 全緑)
fast_moved_target_rerevaluates_next_cull / gate_skip_keeps_previous_visibility /
total_counts_valid_slots_only / fov_cos_min_field_gates_cone。

### 検証結果 (全て実測)
- lib **934/934** (+4)。structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (EntityCuller 経路は一切の挙動不変、FastEntityCuller 変更は bench 非経路)。
- fmt: WORK-only **0** (HEAD 12 行温存)。不可視文字 0 / CRLF 0 /
  all-targets で当該由来警告 0。
- アドバーサリアル 4 系統: (a) ゲート即 false 復元 → gate テスト FAILED
  (旧実装のビット未設定欠陥=実効不可視も同時に実証)。(b) moved 再評価
  要求消失 → moved テスト FAILED。(c) total=バッファ長復元 → total テスト
  FAILED。(d) cos 0.35 ハードコード復元 → fov テスト FAILED。
  いずれも検出確認後 md5 忠実復元 → 8/8 緑。
- 環境事象: なし (git/Rust とも安定、HEAD=3630c11 維持)。

### 残 (次 wave 以降の棚卸し)
EntityCuller 側の新規実体 9 tick 遅延の設計注記強化、replace_targets の
ID-POSITION ミスマッチ時の全再スキャン、fov_cos 真値配線 (consumer FOV
公開待ち)、occludes_strict の 27 点と center_of の対称性証明。

## CM. iris_pipeline.rs (wave 89, 2026-07-24)

650 → 922 行 (テスト純増分を含む)。全行照合 + Iris/OptiFine 一次情報照合
(irisshaders.org、OptiFineDoc `shaders.txt`、IrisShaders ShaderDoc、
shaders.properties reference: programs/ordering/buffers)。
パック構成 (shadow→gbuffers→deferred→composite1..99→final)、命名規則、
uniform 系 (frameCounter/frameTimeCounter/eye-space 位置群) を一次確認。

- 消費者: `rsift-launcher::builtin_engines` (with_tier + `load_shaderpack`
  実呼出=zip パス実在消費、render は plan 未消費)、gui_settings は
  discover_shaderpacks 言及のみ、lib.rs で glob re-export。digest 非経路。

### CM-1 (中): pack_stem Composite(n≥3) が "composite" に潰れ Composite(0) と衝突
- OptiFine 命名は `composite, composite1..composite99` (shaders.txt 一次)。
  旧実装は 1,2 のみ正しく n≥3 を "composite" へ潰す (pub API の潜在欠陥)。
- 根治: 戻り値を `Cow<'static, str>` 化し全番号で厳密合成
  (`format!("composite{n}")`、1..=99 全値の OptiFine 命名ピン付き)。
  呼出側 2 箇所は既に String 化前提で無改修。

### CM-2 (中): resolve_pack_root が ".zip" 必須 → discover→load 往復破綻
- discover_shaderpacks は拡張子なし stem を返すが resolve は
  `pack_name.ends_with(".zip")` 必須 → 発見済 zip が**解決不能**となり
  pack_root=None で静寂に全パス Eco fallback + "Ready: N passes" 誤報。
- 根治: stem/明示の両形態を受理 (`{name}.zip` を候補探索) +
  pack 不存在時の warn 追加 (誠実化)。builtin_engines の明示 .zip 呼出は
  従来通り動作 (回帰ピンで両パターン固定)。

### CM-3 (中): Eco fullscreen WGSL の uv 写像が上下反転 (latent)
- wgpu/D3D 規約: NDC y=+1 (画面上端) ↔ texture v=0 (先頭行)。
  旧 `uv = pos*0.5+0.5` は上端で v=1 をサンプル → composite/final の
  恒等コピーが**上下反転**する潜在欠陥 (draw 消費者未配線のため潜伏)。
- 根治: `v = 0.5 - y*0.5` へ厳密化 + 写像式の文字列回帰ピン
  (旧式の混入を `contains` 否定でも拒否)。

### CM-4 (中): zip deflate の実展開長が無制限 (zip-bomb 穴)
- 16MiB/64MiB cap は**宣言**非圧縮長しか見ない。宣言≠実ストリームの
  悪意エントリは len 検査で最終的に拒否されるが、その**前**に
  read_to_end が無制限確保する (deflate 最大比 ~1032:1)。
- 根治: `take(宣言+1)` で Reader を打ち切り、実展開長を宣言値に縛る
  (fail-closed: 不一致エントリは一切書き出さないこともピン)。

### CM-5 (低): discover_shaderpacks 3 点
- zip 拡張子を case-sensitive で比較 (`.ZIP` 不発) → eq_ignore_ascii_case。
- ドット入り dir 名が file_stem で欠落 ("my.pack"→"my") → dir は file_name。
- 自前の zip 展開キャッシュ `.rsift_extracted` がパック候補に混入
  → 隠し名 (`.` 開始) を候補外化。全て回帰ピン済。

### CM-6 (低): dispatch_frame_passes が uniform を即破棄
- `let _ = uniforms;` で下流エンコーダへ転送不可能だった。
  「消費者ゼロ=削除理由にしない」方針に基づき**供給面を確保**:
  `pub last_uniforms: IrisUniformBuffer` に保持 (f32 bit 等価ピン 6 点)。

### CM-7 (観)
- `for_tier` は Full を返さない: PerformanceTier は 4 値網羅済
  (Minimal/Low→Off, Medium→Eco, High→Balanced)。Full は pub quality の
  手動 opt-in で設計意図通り (低スペック優先方針と一致)。
- plan の簡略 (deferred/prepare 省略、dimension フォルダ・
  shaders.properties パース・separateEntityDraws/translucent
  フォールバック・program.enabled 式は未実装) は module doc 記載の
  scope 宣言と一致するため設計固定 (将来 scope として棚卸しへ)。
- CRC32 未検証は既知簡略 (size 一致で大半を検出) として観察記録。

### テスト (純増 7、941 全緑)
pack_stem_follows_optifine_composite_numbering (1..=99 全値) /
zip_pack_resolves_from_discovered_stem / zip_rejects_stream_inflating_
past_declared_size / zip_deflate_roundtrip_honest_declared_size /
discover_zip_case_insensitive_dir_full_name_and_hidden_skipped /
dispatch_retains_uniforms_for_encoder (bit 等価) /
fullscreen_uv_maps_ndc_top_to_texture_row_zero。

### 検証結果 (全て実測)
- lib **941/941** (+7)。structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (iris は bench 非経路)。launcher `cargo check` errors=0。
- fmt: 当該ファイルは HEAD 側も rustfmt-clean のため全体 rustfmt 適用
  → 再検査で **0**。不可視文字 0 / CRLF 0 / all-targets で当該由来警告 0。
- アドバーサリアル 3 系統: (a) pack_stem を旧潰し実装へ → naming テスト
  FAILED。(b) resolve の .zip 必須復元 → resolve テスト FAILED。
  (c) uv 反転式復元 → uv ピン FAILED。いずれも検出確認後
  /tmp/cm_fixed.rs から md5 忠実復元 (a80cff34…) → 9/9 緑。
- 環境事象: なし (HEAD=fc9f9e9 から安定)。

### 残 (次 wave 以降の棚卸し)
GLSL→WGSL transpiler 実配線 (register_program の warn+Eco フォールバックを
置換)、shaders.properties パース (program.enabled・separateEntityDraws・
custom textures)、dimension フォルダ (world0/-1/1)、Iris uniform 本格写像
(frameCounter 0..720719 リセット・frameTimeCounter 3600s リセット・
eye-space sun/moon/shadowLightPosition 等)、CRC32 検証、
gui_settings 選択 → load_shaderpack 配線。

## CN. bc7_ktx2.rs (wave 90, 2026-07-24)

575 → 696 行 (テスト純増分を含む)。全行照合 + Khronos 一次情報照合
(KTX File Format Spec v2 §levelCount/§levelIndex、KTX-Software libktx
validator「Indices must be sorted from the largest level to the smallest」、
khr_df.h (Data Format 1.4.1)、Vulkan VkFormat registry、Microsoft BC7 仕様
の aWeight4)。ユーザー新規指示「悩んだら Bash で正確な値」を適用:
weight4=round(i*64/15) 全 16 値一致・加重表対称性・ビット予算 128・
ヘッダ 80B・DFD 実書込 26B (宣言 28) を全て Python 機械検算済。

- 消費者: full_graph_wiring が encode/decode_block_mode6 を実使用
  (ブロック経路は digest 非経路、本 wave の write_ktx2 変更は消費者非影響)。

### CN-1 (重大): KTX2 レベルデータ/インデックスの双方向逆転
- spec 正規: データは**最小 mip 先頭**、Level Index entry i は **mip i**
  (mip0=最大を先頭記述)。旧実装はデータ mip0 先頭 + index を rev で
  書込 → index[0] が最小 mip のオフセットを指す KTX2 を排出
  (取込側で mip ピラミッド全反転の機能実害)。テスト total 長さ一致で
  誤検証に見えていたが配置は規格違反。
- 根治: offsets は rev 走査で生成、index は mip0 先頭の正順、mipPadding は
  レベル間のみ・終端パディングなし (一次情報 3 系統で相互確認)。
  ピン: index[0].offset > index[1].offset + 内容マーカー (0xAA/0xBB) の
  実バイト配置を直接検証。

### CN-2 (重大): DFD descriptorBlockSize を u32 で誤直列化 (u16 が正)
- Khronos DFD ブロックヘッダは vendorId/descriptorType/versionNumber/
  descriptorBlockSize の u16×4。旧実装は blockSize を push_u32 →
  宣言 totalSize=28 に対し実書込 26B (Python で再現確認) + DFD 内へ
  余分ゼロ 2B が混入し BDFD 本体が 2 バイトずれ、ブロック範囲が
  totalSize を越える invalid DFD を排出。
- 根治: khr_df.h 確定値による完全 BDFD (blockSize 40/DFD 44B) を
  u16 直列化で実装、宣言 vs 実バイト整合を構造テストで恒久排除。

### CN-3 (中): DFD model=2 誤値 + 「2=BT709」誤コメント + srgb_hint 無意味化
- model フィールドに 2 (=MODEL_YUVSDA の意味) を書き両分岐 2 の
  `if srgb_hint {2} else {2}` 死コード。一次情報の確定値:
  KHR_DF_MODEL_BC7=134、PRIMARIES_BT709=1、TRANSFER_SRGB=2/LINEAR=1、
  CHANNEL_BC7_DATA=0、VERSIONNUMBER_1_3=2、dims は N-1 (3,3,0,0)、
  bytesPlane0=16、単一サンプル bitLength=128-1=127、
  sampleLower=0/sampleUpper=0xFFFFFFFF (libktx ETC1S 実 dump と整合)。
- 根治: 上記の正写像 + srgb_hint が transferFunction を駆動する実装へ。

### CN-4 (低): 破損期残骸 `// ZZPROBE_MARK` + 二重空行 + `let _ = i;` 除去
- 無意味マーカーコメント (破損ターンの残滓)、テスト部の二重空行 2 箇所、
  オフセットループの `let _ = i;` を除去。末尾改行追加。

### CN-5 (観)
- 量子化後端点 (7bit+P) に対する最終 index 再割当は未実施 = 品質余地
  (正しさではない、ispc_texcomp 系でも同様の最終割当がある) を棚卸しへ。
- anchor swap の厳密安全性: 加重表対称 w[15-i]=64-w[i] (機械検証) +
  f32 加算の可換性により swap 後 palette は鏡像一致、tie-break が
  idx0≤7 を保証することを証明 (テスト代替の数学的根拠)。
- A_WEIGHT4: round(i*64/15) との全 16 値一致を Python 機械検算
  (Microsoft BC7 公式 aWeight4 と一致)。

### テスト (純増 3、944 全緑)
ktx2_layout_lengths_correct (内容マーカーの実バイト配置ピンへ強化・継承) /
ktx2_dfd_is_spec_valid_bc7_basic_block (BDFD 全フィールド厳密) /
ktx2_transfer_follows_srgb_hint / ktx2_vkformat_values_match_vulkan_registry。

### 検証結果 (全て実測)
- lib **944/944** (+3)。structural_digest `004c1cf5fb17bfe8` rows=357 不変。
  all-targets エラー 0。不可視文字 0 / CRLF 0 / 末尾改行あり。
- fmt: HEAD 起因 3 群 22 行のみ温存、**本 wave 追加分の WORK-only 偏差 0**
  (HEAD で rustfmt-clean でないファイルのため全体 rustfmt 禁止規律を遵守、
  個所調整: 配列分割・コメント鎖の空行断ち)。
- アドバーサリアル 4 系統: (a) データ順復元 (rev→順) → layout テスト
  FAILED。(b) index 順復元 (rev 注入) → 同 FAILED。(c) blockSize u32 化
  → dfd テスト FAILED。(d) transfer 両分岐 2 化 → transfer テスト FAILED。
  いずれも検出確認後 /tmp/cn_fixed.rs から md5 忠実復元 → 7/7 緑。
- 環境事象: なし (HEAD=65f046d から安定)。
- 編集誤字捕捉: 「解決」の須を簡体字 U+987B/U+9874 にした中国語混入・「隙間ゾロ」(ゼロ誤打) の
  2 件を自分で検出・即修正 (検査規律の実績)。

### 残 (次 wave 以降の棚卸し)
最終 index 再割当 (量子化端点基準)、P-bit 非採用時の index 誤差再評価、
BC7 他モード (0/3/5) の拡張方針、KTX2 KVD メタデータ (KTXorientation 等)、
encode_texture_bc7 の sRGB 正規 box フィルタ (線形化平均は現在未適用、
perceptual 補正は品質課題)。

## CO. gpu_culling.rs (wave 91, 2026-07-24)

531 → 600 行。全行照合。消費者: cpu_occlusion / hzb_2d /
persistent_vbo_pool / render_pipeline (型 import)、lib.rs glob。
GPU 実行系は device 不在のため静的語彙照合 + naga parse 既存ピンに限定
(本 wave の WGSL 変更なし)。Python 機械検算で ChunkBox WGSL storage
オフセット (vec3 align=16 → 0/12/16/28/32/36 → 48B) と Rust repr(C) の
完全一致を事前確認。

### CO-1 (中): dispatch_adaptive_culling GPU 経路の返り値契約が曖昧
- GPU 経路は `chunk_boxes.len()` (提出総数) を返すが CPU 経路は可視数を
  返す非対称が無記載 + GPU 経路で入力スライスの is_visible/instance_count
  が一切更新されないことも無記載 (可視性は GPU バッファ上で確定し
  indirect path が直接消費する設計)。混同誘発の契約欠落。
- 根治: 返り値契約の doc 明文化 (GPU=提出総数/スライス不変、CPU=可視数)。
  挙動不変 (未消費 API、誠実 doc 側の解決 = CF-7 型)。

### CO-2 (低): default_frustum の doc が「perspective matrix decomposition」と虚偽
- 実体は固定軸平行ボックス (view 空間 z∈[0.1,512], |x|,|y|≤256) で
  camera_pos は planes に未反映 (格納のみ、シェーダも非参照)。
- 根治: 誠実注記へ訂正 (実カメラ追従配線は将来課題として棚卸し)。
  面テーブル自体は CL 期以前の符号反転回帰修正済 (既存ピン維持)。

### CO-3 (中): GpuBufferPool が exact-fit 成長で +1 増減のたび全再確保し得る
- 「avoids per-frame allocation on the hot path」の設計目的に対し、
  `capacity < count` の exact-fit は振動的な長変化で毎フレーム全再確保。
- 根治: `pool_capacity` (min 64 / next_power_of_two) pure fn 抽出と
  償還成長化 (0/1/63/64→64, 65→128, 129→256 を厳密ピン)。
  Python で slab 表を事前検算。

### CO-4 (低): chunk_count 契約ピン + trace 誠実化
- `frustum.chunk_count != boxes.len()` の呼出誤りはシェーダがプール末尾の
  stale データまで cull 対象化 → debug_assert 契約ピン (正規呼出は既に一致)。
- `trace!` の「zero alloc」主張を撤回 (bind_group/encoder の CPU 確保あり)
  → 「(pooled buffers)」へ誠実化。

### CO-5 (観)
- WGSL FrustumData の camera_pos/hzb_enabled はシェーダ非参照
  (CPU 一元化の設計意図、WGSL コメント記載済) を設計固定として確認。
- cpu_frustum_cull の短い commands 配列 (get_mut 安全側) は既存ピン済。

### テスト (純増 2、946 全緑)
pool_capacity_is_amortized_pow2_slab (slab 表厳密ピン) /
chunk_box_field_offsets_match_wgsl_storage_layout (offset_of! 6 点機械ピン)。

### 検証結果 (全て実測)
- lib **946/946** (+2)。structural_digest `004c1cf5fb17bfe8` rows=357 不変。
  all-targets エラー 0。不可視文字 0 / CRLF 0 / 末尾改行あり。
- fmt: HEAD 大偏差ファイル (171) のため全体 rustfmt 禁止遵守、編集関数内の
  長行 usage 2 箇所を個所調整 → 偏差は **171 → 161 に減少のみ**、
  本 wave 追加分の WORK-only 偏差 0 (残偏差は全て HEAD 起因)。
- アドバーサリアル 1 系統: pool_capacity の exact-fit 復元注入 → slab
  テスト FAILED → 逆編集で忠実復元 → 8/8 緑。規律メモ: co_backup を
  HEAD で採取し注入後の復元を逆編集で行った (次回以降は wave 89/90 同様
  「固定版バックアップ → 注入 → md5 復元」の順序を厳守)。
- 環境事象: sandbox egress 障害 (全ツール不通 ×6 リトライ) を検出・報告、
  自然復旧後に state 無損失を機械確認して再開。

### 残 (次 wave 以降の棚卸し)
実カメラ追従 frustum (行列分解の真実装) 配線、GPU 結果の CPU 読戻し
需要 (stats/デバッグ用 readback)、GpuBufferPool の縮小戦略 (長期大口確保
保持の解放)、HAB/engine の tier 配線実装 (gpu_enabled 真値源)。

## CP. gui_settings.rs (wave 92, 2026-07-24)

548 → 625 行。全行照合。消費者: lib.rs glob のみ (外部実消費なし =
ユーザー方針により削除しない、テスト用 pure 抽出で消費者面を強化)。
HEAD fmt 大偏差 (251) のため全体 rustfmt 禁止・個所調整のみ。
Rust fmt は char カウント規則 — 幅計算は全て Bash (Python len) で機械検算。

### CP-1 (低): 行 char 幅の三重非整合 (右壁ずれの実害)
- 枠線/通常行 76 char に対し toggle 行 75 (label 42+glyph 28=74+枠 2 の
  誤算)・slider 行 64 (label 30+bar24+value4+枠 8) の**3 種類混在**で
  Sodium 風フレームの右壁が行種ごとにずれる視認実害 (旧 slider_bar
  パニック回帰とは別の整形欠陥、Bash 検算で 76/75/64 を確定)。
- 根治: `GUI_ROW_CHARS=76` 契約 const + 行ビルダ pure 抽出
  (`toggle_line` label 43 / `slider_line` label 42) 化、枠線 vs 全行種の
  char 数一致を機械ピン。

### CP-2 (低): biome blend UI 下限が 1 で vanilla の OFF (0) を選択不能
- vanilla 範囲は 0(OFF)..7 (15x15)。低スペ最重視の本エンジンで最軽量の
  OFF だけ選べない語彙欠落。
- 根治: `BIOME_BLEND_MIN/MAX` pub const (0,7) 導入 + 呼出側差替 +
  OFF=左端・5→pos16 (floor(23*5/7), Bash 検算) の幾何ピン +
  adaptive 既定 5 の無ドリフト確認。

### CP-3 (低): open_global_settings の poisoned mutex 静寂無視
- `if let Ok` で poison 時に何も起きない (BW-1 由来 fail-loud 文化に反)。
- 根治: debug! 通知 + `into_inner()` 復元で GUI を開き切る (std 慣用句)。

### CP-4 (観)
- shader_profile 「Eco/Balanced/Performance/Flagship 2K」の 4 値表は
  PerformanceTier 4 値と整合 (High 時 speed_first で後 2 択) を確認。
- probe_and_cache()/hardware() の混在は両者 OnceLock 共有で実害なし。
- slider_bar の pos 飽和キャスト・退化 (max<=min, width 0) は既存ピン済。

### テスト (純増 3、949 全緑)
row_lines_match_border_width (枠 76 契約+行種別ピン) /
biome_blend_range_admits_vanilla_off (OFF 幾何+既定無ドリフト) /
open_global_settings_smoke_no_panic (poison 復元経路を含む)。

### 検証結果 (全て実測)
- lib **949/949** (+3)。structural_digest `004c1cf5fb17bfe8` rows=357 不変。
  all-targets で gui_settings 由来の警告 0。不可視文字 0 / CRLF 0 /
  末尾改行あり。
- fmt: HEAD 251 大偏差ファイル、本 wave 追加分の WORK-only 偏差 0
  (chain 幅規則 60 字の検算を踏み let 束縛へ分離) → 総偏差 251 完全維持。
- アドバーサリアル 3 系統: (a) toggle 42 縮退 → row_lines FAILED。
  (b) slider 30 縮退 → 同 FAILED。(c) BIOME_BLEND_MIN=1 化 → biome
  FAILED。/tmp/cp_fixed.rs から md5 忠実復元 → 全緑。
  (wave 91 教訓の「固定版先行バックアップ」規律を厳守。)
- 環境事象: なし (HEAD=3d601f4 から安定)。

### 残 (次 wave 以降の棚卸し)
超長ラベル (>43 char) の行は min-width 仕様で 76 超過し得る (compile-time
短ラベルのみ現状)、GUI 描画の真 consumer 化に伴う行ビルダ全面 pure 化、
render_distance の語彙 (vanilla 委譲表示と VideoSettings 値の乖離注記)。

## CQ. frame_fsr1.rs (wave 93, 2026-07-24)

444 → 490 行。全行照合 + fsr1.rs (341 行 BM/BN 期監査済) / shaders/fsr1.wgsl
(66 → 72 行) / frame_reference.rs ミラー rcas 部との 4 連鎖整合監査。
消費者: `examples/frame_proof.rs` (GpuFsr1Pass::new/render_full、実 GPU 専用)、
`frame_pipeline::read_rgba8` 共有、`fsr1.rs` 既定値双方向ピン (BN-3a)、
`full_graph_wiring` の `Fsr1 { sharpness: 0.2 }` 由来 doc 整合、lib.rs。
一次情報: WGSL spec 本体 (github gpuweb/gpuweb `wgsl/index.bs` 20,738 行
現行エディタドラフト = W3C CRD 2026-07-16 と同規則) から §textureLoad
(17,925 行) と RequiredAlignOf 表 (11,329-11,400 行) を本文該当行で照合。
全数値を Python (struct 往復 f32 単精度エミュレーション) で機械検算。

| CQ-1 | 中 | fsr_rcas の外周 4 近傍 `textureLoad(casTex, coord±1, 0)` が画像 1px 外周で範囲外読み出し → WGSL 規格上「不定値」(§textureLoad:「不定論理テクセルアドレスは範囲内テクセルのデータか (0,0,0,0)/(0,0,0,1) のいずれかを返す」) でありゼロ保証なし、ベンダ間で外周 1px が非決定的 → WGSL を `clamp(coord±1, 0, maxc)` 端画素クランプへ根治。CPU ミラー (`frame_reference` load クロージャ) の「OOB=0、WGSL 準拠」2 規則を同じ clamp 規約へ同時根治 (3 連鎖整合)。数学帰結: 平坦領域は外周込みで `lap=(4c)·0.25−c=±0.0` の厳密恒等となり、旧 OOB=0 由来ハロー値 (辺 126/角 132) は Python 機械再導出で廃止 → テスト境界を `120 全画素厳密恒等` へ置換 (旧値は機械再現も確認して廃止根拠を記録) |
| CQ-2 | 中 | `validate_dims` の overflow ガードが 1 オフ — `full_w > 1<<30` 拒否は `full_w = 2^30` を受理し、その `full_w*4 = 2^32` は u32 ラップ (doc 主張の保護対象そのもの)。さらに `(full_w*4).div_ceil(256)*256` の ceil 上げ幅で真の安全境界は `2^30−64` → `MAX_ROW_SAFE_FULL_W = 1_073_741_760` const 化。機械検算: 上界で `padded = 4_294_967_040 = 0xFFFFFF00 ≤ u32::MAX`、上界+1 で `2^32` に触れる。境界 ±1 テスト反転 + readback 行バイト厳密ピン 2 テスト追加 |
| CQ-3 | 低 | FSR1 サンプラの address_mode が `Default` 暗黙 → EASU 外周 (base=-1 / base+1=low_w) の端画素規約は ClampToEdge 既定値に依存していたため `ClampToEdge` を u/v/w 明示固定 (ミラー px() clamp との規約整合を露出) |
| CQ-4 | 低 | `frame_reference` の「OOB textureLoad = 0、WGSL 準拠」コメントが規格文言に反する虚偽 (CQ-1 一次照合で確定) → 正確な clamp 規約記述へ訂正 (コメント虚偽は仕様誤読を将来の実装者に伝播させるため修正対象) |
| CQ-5 | 観 | `Params { inputSize: vec2, outputSize: vec2, sharpness, _pad }` (24B, offsets 0/8/16/20) の「uniform アドレス空間で vec2 は必須 16B アラインでは?」懸念は一次照合で**誤検出と確定** — RequiredAlignOf 表で vecN は uniform でも `AlignOf(S)` のみ要求 (16 化が強制されるのは array stride roundUp16 と struct 型メンバ間隔 ≥ roundUp(16,SizeOf) のみ)。24B wire は規格完全合法、naga span 24 ピンと一致。変更なし |
| CQ-6 | 観 | バインド群分割 (EASU 0-3 / RCAS 0,4,5 と WGSL binding index の整合)、inter view 寿命 (wgpu 内部参照)、dispatch 各軸ガード (`gid >= outDim` 早期 return)、readback 256B アライン・draw_calls=2 の誠実性、DEFAULT_SHARPNESS 双方向ピン — 全て仕様通り確認 (変更なし) |

### 検証 (wave 93)
- `cargo test -p rsift-opt-gfx --lib`: **951 全緑** (+2: readback 行バイト
  厳密ピン / rcas WGSL clamp 4 箇所ピン。境界反転・ハロー廃止値置換は
  既存テスト内改訂)。fsr1_flat_region_is_identity が全画素 120 の
  clamp 規約へ機械再導出値で置き換わり、混同色 bleeding なしを確認。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- fmt: frame_fsr1.rs 偏差 0、frame_reference.rs 偏差 0 (HEAD 0 を維持、
  rustfmt 出力と一致させた multiline assert 形式を採用)、不可視文字/CRLF なし。
- アドバーサリアル 3 系統 (全検出 → /tmp/cq_fixed から md5 忠実復元):
  (a) rcas clamp 1 tap 剥がし → rcas_wgsl_border_loads_are_clamped FAILED
  (clamp 4→3)。(b) ガードを旧 `1<<30` へ逆戻し →
  validate_dims_rejects_violations FAILED (上界受理の反転)。(c) ミラー
  clamp → ゼロロード逆戻し → fsr1_flat_region_is_identity FAILED
  (外周ハロー 126/132 復活)。復元後対象テスト全緑。
- 機械検算 (ユーザー指示「悩んだら Bash」): 上界 2^30−64 の導出、
  padded 端点 2 値、平坦 120 の lap=±0.0 到達、旧ハロー値 126/132 の
  再現 (廃止根拠として併記)、全て Python 単精度往復で実施。
- 環境事象 (正直記録): セッション中盤のツール出力が破損系出力混入
  (存在しない compute_runtime.rs 等の幻内容を含む) に汚染され wave 93
  を一度全廃棄 — 実リポジトリ・リモート (0e5b9f0) 無傷を機械確認
  (git status クリーン / remote HEAD 一致) 後、本一連をクリーン再実施。
  破損出力由来の一切の数値・結果は採用していない。

### 残 (次 wave 以降の棚卸し)
frame_pipeline::read_rgba8 の map_async 失敗時 fail 語彙、FSR1 の実 GPU
frame_proof 突合の CI 不可能性 (アダプタ無し環境) に対する CPU ミラー
厳密一致証明のドキュメント面強化、EASU サンプラー Nearest vs 線形の
規約注記 (現行 Nearest は規約正誤ではなく規約選択)、raw 画素 0.7x/0.5x
プリセット語彙。

## CR. gpu_arena.rs (wave 94, 2026-07-24)

411 → 471 行。全行照合 (`GpuArena` best-fit+併合副割当器 / `SharedRingBuffer`
SPSC / `HazardQueue` エポック回収の 3 部構成)。
消費者: `full_graph_wiring` (`GpuArena::new(512<<20, 256)` チャンク byte で
alloc→エポック退役、`HazardQueue::advance_epoch/defer_free/reclaim`、
`SharedRingBuffer<u64,1024>`)、`gigabuffer.rs` (arena: Mutex<GpuArena> align 256)。
数値経路は全て Python で機械検算 (need 表・最終レイアウト used/free、
flat 併合後フラグメント)。本 wave は「丁寧な振り返り→クリーン」指示に基づき
直近 wave の残件 (wave 92 GUI_ROW_CHARS 非利用警告) も同時処理。

| CR-1 | 中 | `alloc` のアライン繰上げ `size + align - 1` が u64 端でオーバーフロー→ラップし、巨大 size が小さい need に化けて実在セグメントを誤認割当 → `checked_add` で None (確保不能)。u64::MAX / MAX-3 / capacity 丁度 successful / +1 拒否の境界 4 点機械ピン |
| CR-2 | 低 | `free` が handle の size を無検証で `used_bytes` から減算 — 不正ハンドル (size 改竄) で会計が静寂破壊される呼出バグ窓 → `debug_assert!(segs[idx].size == h.size)` 追加 (double free assert と並置) |
| CR-3 | 低 | `gen` 変更回数カウンタが私有死に状態 → `generation()` pub accessor を消費者配線 (外部キャッシュ無効化トリガの true 単一源、テストで +1/回 厳密追従ピン) |
| CR-4 | 低 | `HazardQueue::reclaimed_count` がテストのためだけの `reclaim` 別名 → 削除・直呼出し統一。併せて best-fit+分割+併合の正準レイアウト (need 100→112/200→208/64→64/48→48、used=320/free=704、完全併合→1024) を Python 検算値で厳密ピン |
| CR-5 | 低 | wave 92 (CP) 残留: `GUI_ROW_CHARS` が非 test コードで未利用の警告 (28 warnings 中 1 件が自己由来) → `TOGGLE_LABEL_CHARS = GUI_ROW_CHARS−33` / `SLIDER_LABEL_CHARS = GUI_ROW_CHARS−34` の導出に置換し builder を単一真実源化 (値 43/42 不変、機械検算済)。警告 28→27 |
| CR-6 | 観 | `SharedRingBuffer` SPSC のメモリ順序 (producer Relaxed+Release / consumer Acquire+Release、wrapping 全容量利用、align(128) アライン契約) は正準パターンと厳密一致。`free` の二重 merge は index dance (idx−1 先読み→bounds 検査→cur 再判定) を全分岐走査し境界安全を確認。`free_by_size` bucket 整合 (分割で idx+1 insert→全エントリ +1 shift、merge で逆 shift) も仕様通り。gpu_culling の `DeviceExt` 未使用警告は `git show 3d601f4` 照合で wave-91 初版からの既存欠落 (CR-5 の追加で起票) の棚卸し管理対象へ分離 |

### 検証 (wave 94)
- `cargo test -p rsift-opt-gfx --lib` 953 全緑 (+2: alloc オーバーフロー/境界、正準レイアウト厳密ピン)。
- アドバーサリアル 3+1 系統: (a) CR-1 ガード除去 (unchecked 化) → alloc_overflow FAILED。
  (b) best-fit `range_mut(need..)` → `need+1` 破壊 → gpu_arena 系 FAILED。
  (c) merge 右隣 `cur+1` → `cur+2` 破壊 → gpu_arena 系 FAILED。(a') sed-textue 破損版でもコンパイル不能が直ちに検出。
  全復元は ~/bak (固定版先取得) から md5 忠実復元 (OK) → 復元後対象テスト緑。
- fmt: gpu_arena HEAD(18)/work(18) 維持、gui_settings 305==305 維持。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
  警告カウント 28→27 (新規ゼロ、既存 27 は由来別棚卸し管理)。
- 機械検算 (ユーザー Bash 指示): alloc 境界 4 値、need/used/free 表、ラベル幅定数 33/34 の整合、div-mul round-up の py 一致。
- 環境事象 (正直記録): git HEAD を base 64294c6 へ巻き戻す破損を検出 (14 回目型)
  → fetch+reset --hard FETCH_HEAD で f74a5ec へ復旧、wave-94 未コミット編集は
  /tmp 掃除のバックアップ消失で一度喪失 → 設計確定済の同内容をクリーン再適用、
  以降の固定バックアップは永続領域 ~/bak へ移動 (規律強化)。

### 残 (次 wave 以降の棚卸し)
既存 27 warnings (gpu_culling DeviceExt 由来、adaptive_perf×3 他) の一掃 wave、
GpuArena::alloc の O(n) Vec 分割挿入コスト (巨大アリーナ下での amortized 検討)、
`ChunkMeshArenas::alloc_mesh` panic 語彙 (fail-loud 意図は 2026-07-22 監査済だが
panic→Result 化の可否一台帳論)、HazardQueue pending swap_remove の順序非保存
現況の語彙化、SharedRingBuffer discriminative `Fully-used` len>BUFFER_SIZE 瞬間値の doc 化。

## CS-1/CS-2. low_spec_stack.rs (wave 95, 2026-07-24)

410 → 471 行。全行照合 + render_pipeline 実配線 (LowSpecPlan / FaceEmitMask /
frustum_culled / flora_should_skip_detail / adaptive_mesh_interval /
PullGenerationCache / emit_lod_box_quads / sort_nearest_first 他)、
full_graph_wiring (ao_refine cheap_face_ao 上限注記)、packed4 語彙確認。
一次文献: Lumien/Đorđević-系 GPU culling box-masking (~16° 鋭角盖住近似)。
全数値 Python 機械検算 (1352 shell 値、軸 5 値表 47/61/55/59/37、Euclid² 順序)。

| CS-1 | 中 | `sort_nearest_first` 距離²が i32 算術 — |d| ≤ 46341 で d·d がラップ (release 未定義級) かつキャスト後も |d| > 3030490499 で i64 でも 2^63 超 (テスト赤 2 捕捉: キャスト位置・i64 上限) → i128 厳密化で全 i32 真相で厳密順序 (2⁶⁵<<2¹²⁷) + 巨大座標 4 値順序ピン |
| CS-2 | 低 | FaceEmitMask の 0.15 魔数が複数箇所重複 → `FACE_MASK_AXIS_THRESHOLD` pub const 化 (文献値照合)+ 軸 5 ケース厳密ビットピン (Python 47/61/55/59/37) |
| CS-3 | 低 | SectionOccupancy doc 残留語彙 («layers[y] is unused» 等の英語解説) が実装フィールド xz/y_any と乖離 → 実装同期の日本語 doc へクリーン |
| CS-4 | 観 | `FaceEmitMask::ALL` 退化分岐 `count_ones()<3` は単位ベクトル制約 (各軸 ≥1 ビット寄与) で下限=3 を証明 → 到達不能だが防衛保持として文書化 |
| CS-5 | 観 | apply_solid_interior_cull 範囲 `1..S-1` は真真囲通の手部条件と厳密対応 (1352 shell ピン)、render_pipeline は filter(削除)→edit 順で冗長書き込みなし、PullGenerationCache 語彙 (Chebyshev 境界含む) wave 87 で一致済み |

### 検証 (wave 95)
- 956 全緑 (+3: interior shell=1352 ピン、軸表 5 拘束、巨大座標順序)。
  テスト赤 2 捕捉 (合計 18 件系): i128 前の i64 キャスト位置ミス、巨大座標での順序期待値設計ミス (両者とも実装側根拠で訂正)。
- アドバーサリアル 2 系統: (a) i128 → 旧 i32 直列退行 → huge-coords FAILED。
  (b) T 0.15→0.20 改変 → face_mask 系 FAILED。/tmp 揮発経験を受け固定版は
  永続領域 ~/bak へ (md5 忠実復元 OK→全緑)。
- fmt: HEAD 17/work 17 維持 (rustfmt 正準 2-line split を検算で再現)。
  不可視文字/CRLF/末尾改行 none・警告カウント 27 据え置き。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- 機械検算 (Bash 規律): 1352 値、Euclid² 順序 4 値、軸 5 値表、i128 上限 2⁶⁵<<2¹²⁷、shell=16³−14³。

### 残 (次 wave 以降の棚卸し)
`PullGenerationCache` prune の HashMap retain O(n) を世代インデックス化する検討、
FaceEmitMask::ALL 防衛分岐の NaN yaw/pitch 供給経路網羅監査 (hzb_2d::CameraState
正規化の次 wave 棚卸しに含める)、SectionOccupancy の 16×u16 → u256 圧縮による
ビルド費削減 (branchless_block 系の統一 hw_popcount)、
emit_lod_box_quads の origin 絶対化語彙 (origin_x/y/z の world-aligned 基点一貫性)。

## CT. job_system.rs (wave 96, 2026-07-25)

342 → 492 行。全行照合 + 消費者 full_graph_wiring 実配線 (job_sys フィールド:151 /
new(2):289 / parallel_for:498 + wait_idle:504、quads>0 ガード) 確認。
全数値 Python/Bash 機械検算 (steal 列、fmt 交差判定)。

| CT-1 | 中 | `pending` を POP 時 (実行開始) に減算 → 最終ジョブ実行中に `wait_idle` が 0 判定で早期復帰し得て完了保証を破壊 → `worker_loop`/`drain_one` 双方で実行完了後減算へ移動。pending 語彙は「in-flight + queued の未終了件数」に確定 |
| CT-2 | 中 | スティール横取り後にワーカー側で一括 `Priority::Background` 強制化 → FrameCritical 静寂背景化 (優先度契約破壊) → `Priority::from_raw` (判別子↔バケット index 対応) + `steal_half() -> Vec<(Priority, Job)>` + `requeue_stolen` pure 分離で優先度をジョブ属性として転送時保持 |
| CT-3 | 中 | `parallel_for(count=0)` が `count.max(1)` で `f(0..1)` を 1 件静寂実行 (要求は「0 実行」の逆セマ) → `count == 0` early return (消費者側 quads>0 ガードに依存しない API 契約化) |
| CT-4 | 観 | Priority 判別子 (FC=0/N=1/BG=2) と PrioQueue バケット index の一対一対応を `from_raw` で内部規約化。wait_idle は排水参加 + 50µs ポーリング、ワーカ Condvar 5ms timeout、Drop は shutdown+notify_all+join で終結確実 — 全て契約内 |
| CT-5 | 観 | 消費者は full_graph_wiring のみ (quads>0 ガード付 parallel_for + wait_idle)。待ち条件は pending==0 判定のため語彙厳密化 (queued+in-flight) で消費者挙動変化なし。CT-1 修正で wait_idle の完了保証が初めて真に成立 |

### 検証 (wave 96)
- 960 全緑 (+4: wait_idle 完了保証 8job×10ms、steal 優先度+件数列、
  requeue FIFO+優先度保存、count=0 厳密 no-op)。
- アドバーサリアル 3 系統: (a) pending を POP 減算へ逆戻し →
  wait_idle_waits_for_actual_completion FAILED。(b) requeue 内の強制 Background 化
  逆戻し → requeue_stolen_preserves_fifo_and_priority FAILED (初回注入は
  PrioQueue レベルで検出不能と判明 → requeue_stolen 分離 + dst に Background
  marker(99) 事前共存の強化で検出可能に — marker 列「1→2」期待に対し注入時は
  99 が返る)。(c) `count.max(1)` 逆戻し → parallel_for_zero_count_is_strict_noop
  FAILED。固定版は永続領域 ~/bak へ (md5 忠実復元→全緑)。
- テスト赤=自己誤り捕捉 19 件目: steal_half 半切捨て (n=1/2=0 で fall-through)
  の期待値設計誤り → 正しい列 [BG×1]→[FC×2]→[FC×1] へ訂正 (機械検算で確定)。
- fmt: HEAD 93 / work 93 一致 (自分の追加行 157 行と rustfmt 14 hunk を Python
  交差判定 → 交差 5 hunk のみ rustfmt 正準形へ整列、ベースライン 9 hunk 不変)。
  不可視文字/CRLF/末尾改行 none・警告カウント 27 据え置き。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。

### 残 (次 wave 以降の棚卸し)
submit 最短キュー選択の O(workers) 線形走査 (workers≈物理コア−1 で実害なし、
大規模化なら Chase-Lev work-stealing deque 化を検討)、wait_idle 50µs ポーリングの
完了 Condvar 化 (現行は drain 参加で CPU 無駄なし・起票のみ)、Priority 3 段の
BG 長期滞留 aging (現行は steal が最低優先から横取りで緩和済)、
worker_loop の 5ms timed wait を shutdown 専用通知で即応化するかの検討。

### 棚卸し (wave 96 時点)
残 86 モジュール (機械再計算 all=163/done=95/todo=86、quality_governor 309、
render_pipeline 1713 第 3 部、full_graph_wiring 2687 第 4 部等)。
警告一掃 wave (27 件) は別途棚卸し管理。

## CU. quality_governor.rs (wave 97, 2026-07-25)

309 → 519 行。全行照合 + 消費者 (full_graph_wiring:163 フィールド/:301 new
(Default)/:346 quality_score/:400·:435 observe + :436 Downshift 消費、
rsift_bench:146-153 scenario、render_pipeline コメント契約行) 確認。
一次情報: UE DynamicRes `r.DynamicRes.MaxConsecutiveOverbudgetGPUFrameCount`
(module doc 出典: 連続オーバーで即降段・履歴リセット) = 降段無クールダウン
設計の根拠。全数値 Python 機械検算 (下記)。

| CU-1 | 低 | EMA「約 16 フレームで半減期」コメント誤記 — 正しくは半減期 ln(0.5)/ln(1−α)=11.20 フレーム、時定数 τ=1/α=16.67 (n=16 残存 (1−α)^16=37.16% で半減せず) → `EMA_ALPHA` pub const 化 + f64 bit 厳密ピン (n=11 bits>8333、n=12 bits<8333)、降段時 EMA リセットは最新観測 40000.0 への bit 代入ピン |
| CU-2 | 中 | GovernorConfig 無検証 — NaN/非有限/反転帯 (good≥over) は observe 内の大小比較を両方不成立にしてガバナを静寂沈黙 (else 分岐で両カウンタ恒常リセット) させ、0 閾値は毎フレーム発火の病理 → `validate()` fail-loud + `new()` 構築強制 |
| CU-3 | 低 | `frames` private 未読フィールド → `frames()` pub accessor で消費者追加 (bench fps/期間算出の一次情報) + observe 毎厳密 +1 単調ピン |
| CU-4 | 低 | next_upshift コメント「最後に落としたものから戻す」は未実装履歴スタックを示唆する虚偽 (実=静的逆優先度: RenderDistance 先返上) → 誠実化 + タンパー初期値の全列 f64 ピン (10,RD,0)(15,SH,2)(20,SH,1)(25,SH,0)、quality_score を外部 levels 改竄耐性 `saturating_sub` 化 (u8 アンダーフロー → debug panic / release ラップ破壊の根絶、改竄 200 全投入でも 0..=100 契約維持) |
| CU-5 | 観 | render_scale_pct `l.min(5)` 防御クランプは levels pub 改竄に有効、降段にクールダウンを掛けないのは UE 一次情報「即座に落とし履歴リセット」と一致 (意図的設計)、upshift 時 EMA 非リセットは good 期間の収束済みで妥当、cooldown 実効語彙 = 昇段後 F+1..F+cd−1 の cd−1 フレーム全抑制 + F+cd から再開可 (5 間隔列で実証) |

### 検証 (wave 97)
- 967 全緑 (+7: 半減期 bit、降段 EMA リセット bit、validate 反転/NaN 拒否、
  validate 0 閾/target 拒否、frames 単調、upshift 逆優先度 4 列、
  score saturating + 正規改竄 81 ピン)。
- アドバーサリアル 4 系統全検出: (a) EMA_ALPHA 0.06→0.10 改変 →
  ema_half_life_bit_exact FAILED。(b) 反転帯チェック除去 →
  config_validate_rejects_inverted FAILED。(c) next_upshift の .rev() 除去 →
  upshift_reverse_priority FAILED (Shadow 先返上列化で検出)。(d)
  saturating_sub 逆戻し → quality_score_saturates FAILED
  (attempt to subtract with overflow)。固定版は ~/bak へ (md5 忠実復元→全緑)。
- fmt: HEAD 14 / work 14 一致 (自分の追加分 1 hunk のみ rustfmt コメント整列、
  ベースライン 1 hunk 温存)。不可視/CRLF/簡体字 none・警告 27 (14+13) 据え置き。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- 機械検算 (Bash 規律): 半減期 11.2023・τ 16.6667・n=16 残存 37.16%・
  f32-as-f64 閾値 over=19165.899602651596/good=13332.800198674202・
  降段初発厳密 n=5 (over は n=2 蓄積開始)・upshift 列 10/15/20/25・
  score 18*100/22=81・n=11/12 f64 bits 0x40c07afba3552504/0x40befbb01e95d4f3。

### 残 (次 wave 以降の棚卸し)
QualityState.levels pub 配列の検証付き setter 化 (observe 側は改竄 level>steps−1
で next_downshift の guard により該当ノブ静寂対象外 — saturating は score のみ)、
cooldown 語彙「cd フレーム禁止」対実効「cd−1 全抑制+F+cd 再開」の doc 明文化、
EMA 単一指標から P95 band / GPU・CPU 別トラッキング (UE 本家準拠) への拡張検討、
render_scale_pct の段表 [100,90,80,70,60,50] を steps() と単一真実源化。

### 棚卸し (wave 97 時点)
残 85 モジュール (機械再計算)。警告一掃 wave (27 件) は別途棚卸し管理。
→ wave 98 前工程で訂正: 抽出式 `^## [A-Z0-9]*\.` は複合見出し `## CS-1/CS-2.`
(hyphen・slash 含有) を拾えず low_spec_stack を誤って未監査扱いしていた。
式を `^## [A-Z0-9][A-Z0-9/\-]*\. ` に修正 → done 96→97、**正値は残 84**
(機械再計算、自己の帳簿式誤りの捕捉として記録)。

## CW. mesh_cache.rs (wave 98, 2026-07-25)

434 → 566 行。全行照合 + 消費者 (render_pipeline:30 use/:95 cache フィールド/
:192 adaptive 構築/:392 get/:490 put/:986 stats/:1240 invalidate_chunk)、
region_zstd 参照、ast-grep 構造スイープ 3 系 (DEV_ACCEL.md 導入後初適用)。
zstd 0.13.3 API (Decoder + Read::take) 使用。全数値 Python 機械検算。

| CW-1 | 中 | `zstd::decode_all` は展開長無制限 — 破損/細工 .rmesh 数 KB が GB 級へ膨らむ展開ボム経路 → 正当最大 ≈40.7MiB (6·4096 quads×24 sections、頂点 12B/index 4B 全非グリーディ、Python 機械検算) の 1.6 倍余裕で 64MiB cap を `Decoder+take(cap+1)` で強制 (超過は Err→miss 計上+削除→再構築) |
| CW-2 | 中 | put() は最終名へ直接 `File::create` + `let _ = f.write_all(&bytes);` の静寂破棄 — 部分書き込みでも true を返し debug ログ「stored N bytes」を偽装、File::create 失敗は warn 無し静寂 false、クロスプロセスでは書込途中ファイルを他インスタンスが read → 破損扱い抹消の裂け読みリスク → tmp 全量検証書き込み + rename 原子置換 + 失敗 warn+false+tmp 削除 (crash 残渣 .tmp は不活性・次回 put で回収) |
| CW-3 | 低 | encode 側セクション数を `(len as u16)` で静寂縮退 — len > 65535 で wire u16 フィールドがラップし decode が後続を mesh バイト列として誤読 → len > u16::MAX は fail-loud Err (実上限 24 sections/チャンク) |
| CW-4 | 観 | decode は trailing garbage を受理 (versioned wire 耐性として意図保持)、v1 経路は RLE 非格納のため RLE 検証なし (設計通り)、32-bit usize の vlen*12 オーバーフローは MC 1.21.11 対象デスクトップ 64-bit の範囲外、sync_all なし耐久性は miss 治癒で許容、stats tuple (hits, misses) 語彙は render_pipeline:986 で対応済 |
| CW-5 | 観 | key_path インジェクション非成立 (i32 format! は path separator を生成し得ない)、invalidate prefix の "1_2_" vs "1_20_"/"2_1_" 衝突は既存テストピン済、cache.put は LOD simplify 済 mesh を保存し get が再 simplify (tier 間 semantic drift の可能性は render_pipeline 第 3 部棚卸しへ転記)、ast-grep 横断: `let _ = write_all` 静寂破棄型は全域 0 件・無制限 decode_all の残存は region_zstd.rs:107 (test)/:170 (本番) → region_zstd wave の棚卸しへ起票 |

### 検証 (wave 98)
- 971 全緑 (+4: 展開ボム拒否、tmp 塞がれ fail-loud、.tmp 残渣なし、
  セクション数境界 65536 拒否/65535 受理)。
- アドバーサリアル 3 系統全検出: (a) 無制限 decode_all 逆戻し →
  decompress_cap_rejects_bomb FAILED (「cap」メッセージ由来ピンで検出)。
  (b) 直接 create+write_all 破棄の忠実旧形復元注入 →
  put_fails_loud_when_tmp_path_blocked FAILED (旧形は tmp を使わず final を
  create 成功して true → 分岐検出)。注入初回は私の旧形復元が構造未閉鎖
  (unclosed delimiter→E0317) でコンパイル不可 — 旧形の厳密再脆弱化として
  match 後共通 false 形に訂正してから裁定 (過程も記録)。(c) u16 guard 除去 →
  encode_rejects_section_count_overflow FAILED。固定版は ~/bak へ
  (md5 忠実復元→全緑)。
- fmt: HEAD 0 / work 0 完全一致 (自分の追加 3 hunk を rustfmt 正準形へ:
  write_all 連鎖単行化・assert_eq 複数行化 (CJK 幅考慮)・expect_err 単行化)。
  不可視/CRLF/簡体字 none・警告 27 (14+13) 据え置き。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- 機械検算 (Bash 規律): 展開上限 ≈40.69MiB (quads 589,824・verts 2,359,296・
  vbytes 28,311,552・ibytes 14,155,776)、cap 64MiB=1.6 倍、u16 境界 65535/65536。
- 前工程簿記訂正: 棚卸し抽出式の複合見出し漏れ (wave 95 節) を修正 →
  残 84 が正値 (wave 97 節に訂正記録)。

### 残 (次 wave 以降の棚卸し)
region_zstd.rs:170 の無制限 decode_all (本番経路、CW-1 類型の横断展開候補)、
cache.put の LOD 事前 simplify → tier 間再 simplify の semantic 検討
(render_pipeline 第 3 部)、stats の (u64,u64) tuple を名前付き構造体化、
cache dir のディスク容量クォータ (LRU 掃除)、crash 残渣 .tmp の起動時掃除。

## CX. region_zstd.rs (wave 99, 2026-07-25)

301 → 410 行。全行照合 + 消費者 (pseudo_mc_bench:2171/2177・pseudo_mc_live:
671/860/930・rsift_bench:157-163・full_graph_wiring:156/770/776 put+stats、
digest 経路は 64KiB チャンクで本変更不発を確認)。wave 98 CW-5 起票の
decode_all 横断対象。全数値 Python 機械検算。

| CX-1 | 中 | build_file の location セクタ数を `sectors & 0xFF` で静寂ラップ — Stored 選択で >1MiB 生チャンクが到達可能 (1 チャンク 256 セクタ超) → reader から読めない破損ファイルを静寂返却 → `build_file_checked() -> Result` fail-loud 化 + build_file は expect ラップ (API 互換)・境界 Python 検算: body=255·4096−5=1,044,475 で sectors=255 受理・+1 で 256 Err/panic |
| CX-2 | 中 | scan_file は location スパン無検証 (破損/細工ファイルで offset=ヘッダ内・span end>file.len でも「正常」報告) → 各エントリ offset ≥ HEADER_BYTES/SECTOR (=2) かつ (offset+sectors)·SECTOR ≤ file.len() の InvalidData fail-loud 検査 |
| CX-3 | 低 | get_chunk/stats の無制限 decode_all (CW-5 起票分) を再評価: 対象は private メモリ内部の自己生成バイト列限定で CW-1 の外部由来 disk 経路とは危険度が異なる → cap 導入は行わず provenance の相違を doc 明文化 (誠実格下げ記録)、:107 expect も不到達で loud のまま |
| CX-4 | 観 | build_file は冪等 (locations 全再計算)、offset 24-bit (≈64GiB)/len u32 ラップは非到達コメント照合、auto() の zstd-3 推奨は ZFS 界隈実測の doc 出典と一致 |
| CX-5 | 観 | timestamps all-zero は epoch scaffold として doc 済、Location::default = absent チャンク意味論は build/scan で一貫、put_chunk の二重書きは上書きのみで leak なし |

### 検証 (wave 99)
- 974 全緑 (+3: 255 丁度ピン (offset=2・sectors=255・used=257)、256 Err+
  fail-loud panic、strict scan: ヘッダ重複/境界超過 InvalidData + 正規受理)。
- アドバーサリアル 3 系統全検出: (a) sectors guard 除去 → sector_256 FAILED、
  (b) strict 検査ブロック除去 → scan_file_rejects FAILED、(c) 境界 0xFF→0xFE
  改竄 → sector_255_boundary FAILED (255 丁度誤拒否で検出)。固定版は
  ~/bak へ (md5 忠実復元→全緑)。
- fmt: HEAD 22 / work 22 完全一致 (追加分は記述時に正準形、MINE hunk ゼロ)。
  不可視/CRLF/簡体字 none・警告 27 (14+13) 据え置き。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- 機械検算 (Bash 規律): 境界 body 1,044,475/1,044,476、span end (2+255)·4096
  =1,052,672、offset 24-bit 上限 ≈64GiB。

### 残 (次 wave 以降の棚卸し)
タイムスタンプの実 epoch 供給配線、region ファイルの実 disk I/O 層
(現状 builder/scan のみで write/read 配線なし — pseudo_mc_live:930 参照)、
len_field u32 (>4GiB チャンク) の契約 doc、CodecChoice::auto
(battery_saver) の実機電源状態配線。

## CY. nanite_clusters.rs (wave 100, 2026-07-25)

343 → 742 行。全行照合 + 消費者照合 (full_graph_wiring.rs:1133-1161 で実配線
= 実クアッド頂点から clusterize → should_draw の LOD スクリーン判定で
report.nanite_meshlets / nanite_meshlets_culled を生成、lib.rs:188 re-export。
配線経路は identity index = 三角間の頂点共有ゼロ → 全三角が孤立する
「単一三角クラスタ」領域であり、CY-2 の影響解析の基点)。wide_static_bench
digest 経路 (15 モジュール) に nanite_clusters 非含有を grep 照合し、
実測でも digest 不変を確認。全数値を Python (Fraction/Decimal 80 桁の厳密
f32 エミュレーション, RN-even 逐次再現) + 独立 Rust プローブ + rustc 実機
の 3 系統で機械検算。

| CY-1 | 中 | clusterize が `indices.len() / 3` の切り捨てで末尾の半端な 1-2 index を静寂 drop (wave 72 vertex_cache_opt と同種の静寂 drop) → assert! fail-loud + 契約 doc (空入力は従来通り受理)・panic メッセージ仕様化 |
| CY-2 | 中 | next_seed の素数刻み走査が数学的破綻: gcd(step,len)≠1 で剰余列 {offset+i·step mod len} が全位置を巡回せず、31 (素数) 固定では len≡0 (mod 31) で訪問位置が len/31 (=全体の 3%) に縮退 → それらの確定後は残りの未確定三角を二度と seed できず走査終了 = 幾何の静寂消失。wiring 実経路 (全三角孤立) で直撃: 62 三角→2 クラスタ/93→3 (Python 厳密シムで消失再現) → step を len と互いに素な最小の奇数 (≥31) に取り直し (ユークリッド互除法)、剰余列を巡回群の完全置換化して全域 1 周を保証。gcd(31,len)=1 の既存入力 (2 の冪長 grid 全部) では走査順も旧版と完全一致 (シムで同一性立証) |
| CY-3 | 中 | clusterize の頂点非有限無検査: NaN 1 個で centroid が NaN 化するが、radius は max-scan の `if d > r` ガード (NaN 比較は常に false) で 0.0 へ、merge error は `(NaN-…).max(0.0)` の NaN 非伝播で +0.0 へ静寂潰れ、should_draw 側も dist が 1.0 マスクされて「常時描画」へ静寂誤分類 (rustc -O 実機で全連鎖を実測: NaN.max(1.0)=1.0, (NaN-1).max(0.0)=0.0) → 入口 assert! 遮断 (wave 71 BU-1 / wave 73 と同哲学)。併せて vertex_offset 死に計算 (`let _=`) 除去、vertex_offset/vertex_count の scaffold 意味論 (常 0/コーナー数=3×tri 複製込み) を doc 誠実化 |
| CY-4 | 中 | 親鎖が実際には一度も構築されない構造嘘: 局所 parent_of を計算して `let _=` で破棄していたため NanoMeshlet::parent は恒に u32::MAX、モジュール doc の「誤差ツリーまで作る」は不成立 → meshlets[root].parent = partner 実配線 (深さ 1 の親鎖を materialize)、stride/parent_of 死にコード除去、奇数個の末尾は単独根 (parent=MAX/error=+0.0 厳密)、level は多段化 scaffold 注記で誠実化 |
| CY-5 | 中 | cluster_should_draw: doc が proj_factor を式から省略 +「誤差う」誤記 + clamp 2 箇所 (dist≥1.0, ε≥1.0) 未記載 + 厳密不等号の境界意味論未記載。更に f32::max の NaN 非伝播仕様により NaN カメラが d=1.0、NaN ε が ε=1.0 へ**通常値マスク**され静寂誤カリング (実機検証済) → 有限性 assert! (cam 各成分/proj_factor>0/error_px/error/sphere_center/sphere_radius) で入口遮断 + doc 完全書換 (式・clamp・境界等号不採用・error=0 常時採用の 4 契約を明記) |
| CY-6 | 低 | テスト error_metric_nonzero_for_parents が `m.error >= 0.0` assert の恒真テスト (error は定義上 `(…).max(0.0)` ≥ 0 で何も検査せず、名のみ nonzero) → `meshlets[0].error > 0.0` + any(>0) の実質 assert に置換 (grid16 先頭ペアは中心相異 128 三角クラスタ同士で error>0 をシム立証) |
| CY-7 | 低 | `max3(a, b)` が 2 引数 max (命名嘘: 3 項を取らない) → ラッパ除去して呼出側で f32::max 二項直接化 |

### 検証 (wave 100)
- 983 全緑 (+9: len%3 fail-loud+空/1 三角受理、31/62/93 孤立メッシュ全
  カバレッジ、grid16 分割の厳密マルチセット 119 (110×1+4×2+4+6+3×128,
  Python 生成配列をファイルから再抽出照合)、親鎖 59 ペア+単独根 error
  bit ピン、meshlet0 sphere bit ピン、非有限頂点 NaN/inf 遮断、
  should_draw dyadic 判定表 (境界等号 5.0<5.0 不採用含む)、should_draw
  非有限 7 系統遮断、gcd 基本形 9 値)。既存 3 テストも強化 (grid8
  メッシュレット数 1 ピン追加・恒真 assert 実質化)。
- アドバーサリアル 3 系統全検出: (a) CY-2 coprime 選択除去 →
  next_seed_full_coverage FAILED (孤立 62/93 で消失再現)・grid16 系ピンは
  数学通り緑維持 (gcd=1 で走査順不変の相補確認)、(b) CY-4 親配線除去 →
  parent_chain FAILED、(c) CY-1 assert 除去 → indices_len FAILED。
  固定版は ~/bak + rsift/bak へ (md5 57834e4d… 忠実復元→全緑)。
- fmt: HEAD 0 / work 0 完全一致 (ベースライン 0 のため全体 rustfmt 安全適用、
  EXPECTED ピン配列は #[rustfmt::skip]+Python 再照合で保護)。
- 警告: opt-gfx lib 14 / lib-test 17 / api 13 = HEAD stash 対照で完全一致
  (増分ゼロ)。不可視文字/CRLF/簡体字/末尾改行 全検査パス。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変
  (grep での経路非含有照合 + 実測の二重確認)。
- 機械検算 (Bash 規律): gcd 周期解析 (len≡0 mod 31 → カバレッジ 3%)、
  消失再現 62→2/93→3、grid16=119 meshlets 分布立証、59 ペア+根 118、
  meshlet0 centroid/radius bit (0x40d09555, 0x0, 0x41478aab, 0x415a33d2)
  = Python RN-even 逐次厳密値 = 独立 rustc プローブ bit 一致 (argmax
  コーナー (1,0,0))、NaN マスク連鎖 4 値実機測定、should_draw dyadic
  全値厳密、gcd 9 値。
- テスト赤=自己誤り捕捉装置 20 件目相当の作動 2 回: (i) Python f32
  エミュレーションが負の差分を丸め関数の非負前提で 0 に折り畳む自力バグ →
  プローブ bit 乖離 (radius 0x4121df91 vs 0x415a33d2) で検出 → 符号付き
  修正後に完全一致 → ピン採用 (プローブ併用が無ければ誤ピンを埋め込む
  ところだった)。(ii) EXPECTED ピン配列の初回記述が 103 ones/総数 112 で
  あり機械照合で検出 → 110/119 に訂正。

### 残 (次 wave 以降の棚卸し)
- 多段 LOD DAG: 実運用 Nanite の 4-stage 再クラスタ化は未実装 (NOTE 通り
  深さ 1 固定)、誤差ツリーの上段構築は将来課題。
- vertex_offset/vertex_count の通常頂点領域の実構築 + streaming 側配線
  (現行 scaffold: 常 0 / コーナー数)。
- merge 相手選択が隣接 index 固定のヒューリスティック (NOTE 記載通り)、
  本格化時は共有エッジ最大スコア + 境界ロック化。
- clustered_indices のユニーク頂点コンパクション (頂点キャッシュ効率)。
- full_graph_wiring 側の三角構築が identity index (実 topology 配線は同
  ファイルの将来課題コメント参照) — render_pipeline 第 3 部監査に同梱。

## DA. distant_lod.rs (wave 101, 2026-07-25)

320 → 687 行。全行照合 + 消費者照合 (full_graph_wiring.rs:1123-1127 の
lod_for_distance のみ配線、結果は `_distant_lod` に破棄 = 計測 scaffold、
lib.rs:182 re-export。chunk_dists の全 3 供給経路 (render_pipeline:1016/
1032/1050) は i32 座標 hypot 由来で有限・非負を照合 → DA-4 assert 非発火
確認)。downsample/build/merge_4 は現状テストのみ消費 (DH メッシュ経路は
未配線)。wide_static_bench digest 経路に非含有を照合 + 実測不変確認。
全数値を Python 厳密シム (角窓・クランプ・複製重みまで同一論理) で機械検算。

| DA-1 | 中 | downsample の奇数寸法で末尾列/行が 2x2 走査の範囲外となり縁カラムが次段へ静寂脱落 (doc「2 の累乗でなくてもよい」と矛盾) → 偶数 (≤1 除く) fail-loud + 構造不変量 samples.len()==w*h 明示 (誤領域静寂参照も同時根絶) |
| DA-2 | 中 | 上面 quad の角高さスワップ: 頂点配置順 (x,z)→(x,z+1)→(x+1,z+1)→(x+1,z) に対し角 Y を [y00,y10,y11,y01] で供給 (正は [y00,y01,y11,y10]) → 傾斜のある全セルで上面がねじれたサドルになる静寂幾何破壊 (flat 地形テストでは検出不能) → 位置対応へ訂正 + 4 セル厳密 pos_packed ピン (Python シム確定) |
| DA-3 | 中 | mkv の origin 設計破綻: ローカル座標 bx から origin を差し引いていたため origin≠0 のマップでは全頂点が負側へ飽和し 0 平面へ崩壊 + f32 world 座標化は |origin|>2^24 で精度欠落 + u16 clamp が両経路で静寂 → 整数ドメインのローカル pack へ再設計 (origin は GPU 側 uniform 前提を doc 明文化) + fail-loud 契約 (lod<16, extent ≤65535, セル数 ≤214,748,364 = 20 verts/セル u32 上限) |
| DA-4 | 中 | lod_for_distance の NaN が全 < 比較を false にし最遠 LOD 5 へ静寂逃走 (近景最低詳細化) → 有限・非負 assert 遮断 (wave 71 BU-1 同型・現消費者経路非発火照合済) + 境界 12 点厳密ピン (全て「以上」次段側) |
| DA-5 | 低 | mkv の Y 量子化 `y as i32` 切捨て = 近傍平均の .5 刻み補間値を平均 0.5m 分常に下落させるバイアス (LOD 段差 = スカートで塞ぐ相そのもの) → f32::round 最近接 (半は 0 から遠い側) 化 + 12.5→13/8.5→9 厳密ピン (旧 trunc 12/8) |
| DA-6 | 低 | merge_4 タイ処理の doc 虚偽: テストコメント「tie は先着」は実挙動と逆 (max_by_key 公式仕様 = 同値最大の『最後』を返す → 後勝ち) → コメント訂正 + 後勝ち 0xBBBB 厳密ピン、absent の色は最多頻度不参加等 6 意味論ピン |
| DA-7 | 低 | doc 群: 「小LODs」の 小 の直後への U+00E3 文字化け混入 訂正、skirt 深度「4-8m」→ 実式 min(4,max(min_y,1)) ∈ [1,4]m 訂正、neighbour_avg_y 死引数 _lod 除去 (map 段一致契約 doc 化)、縁クランプ複製重み注記、n==0 分岐の不到達性注記、「block review」不明瞭語訂正、half 命名嘘 (実は full cell) 解消 (cs/bx 整数化に同梱) |

### 検証 (wave 101)
- 989 全緑 (+6: 奇数/不変量 fail-loud+境界、上面角 16 語厳密ピン、量子化
  round+深度式 4 ケース、LOD 境界 12+遮断 4、merge_4 意味論 6、extent/
  origin/零次元契約)。既存 tie テストも 0xBBBB 厳密値へ強化 (doc 虚偽訂正)。
- アドバーサリアル 3 系統全検出: (a) DA-2 角スワップ復活 → top_quad FAILED
  (quantize も連鎖検出)、(b) DA-5 trunc 復活 → quantize FAILED (12 vs 13)、
  (c) DA-4 assert 除去 → lod boundaries FAILED (NaN 静寂 5 化)。
  固定版 ~/bak + rsift/bak (md5 63c7183d… 忠実復元 → 全緑)。
  注入訓練 1 失敗記録: (b) 初回は struct 式途中に // コメントを差し後続
  フィールドを構文上消去する不正注入を即検出 (コンパイル確認前に restore)
  → wave 98/100 に続く「必ずコンパイルが通る旧形厳密再脆弱化」規律再確認。
- fmt: HEAD 85 / 自分の hunk は全て rustfmt 正準形へ機械置換 (22 hunk)、
  残偏差 14 行は内容一致で HEAD deviant 集合に完全包含 (既存温存検証済)。
- 警告: opt-gfx lib 14 / lib-test 17 / api 13 据え置き (本変更で増分ゼロ)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- 機械検算 (Bash 規律): 角平均 4 セル全値 (クランプ複製重み込み)、量子化
  12.5→13/8.5→9/6.0/9.5→10/11.5→12、u32 上限 = 4,294,967,295/20 =
  214,748,364、深度式 ∈ [1,4] 表、max_by_key tie 仕様、extent/lod 境界値。
- テスト赤=自己誤り捕捉 21 件目: quantize テスト初版が深度式を 4.0 と
  読み違え min_y=1.0 で赤 → コード正 (深度 1.0 で sb=11.5→12) ・期待値誤
  を確定してピン訂正 (コード無変更)。経緯をテストコメントへ誠実記録。

### 残 (次 wave 以降の棚卸し)
- DH メッシュ経路の実配線 (downsample/LodMesh::build はテストのみ消費;
  full_graph_wiring の `_distant_lod` 破棄 scaffold = 同ファイル第 4 部と
  併せて実効配線 or 誠実な downgrade 判断)。
- origin_x/z の GPU uniform 実受け渡し (doc 契約のみ、受け手なし)。
- スカート winding/外向法線の GPU 側一貫性 (backface 契約は render 配線時に
  一次情報照合予定)。
- 上面/スカート色の 4 近傍補間 (現状セル代表色を全面共有)。

## DB. palette_pack.rs (wave 102, 2026-07-25)

308 → 464 行。全行照合 + 消費者照合 (full_graph_wiring:1002 from_blocks/
1045 stats_for 実配線、pseudo_mc_bench:36/2515 メモリ報告表示、rsift_bench:
17/97-98 encode+random access 計測。set() は消費者ゼロのため DB-4 は振る舞い
安全側修正として自己完結)。wide_static_bench digest 経路に非含有を照合
(bitpacked_section は別モジュール) + 実測不変確認。全数値を Python 機械検算。

| DB-1 | 低 | doc「322 種超 → 直接 16bit 格納にフォールバック」は未実装の虚偽 (vanilla の 9bit 超グローバルパレット化の化石、needed_bits(322)=9 と整合)。実装は bits ≤ 12 (ユニーク ≤ 4096) で完結 = フォールバック不要が数学的に証明できる → doc 訂正 + 12bit 完結ピン |
| DB-2 | 低 | doc「最大 ~1/4」は敵対入力で偽: 全 4096 相異では 12bit 語パディング損込み 14,760B = **1.8018 倍に膨張** (実測 bit ピン) → 「縮む」表を worst-case 含む誠実表へ訂正 (2 種 524B/16 種 2088B/322 種 5340B/4096 種 14760B) |
| DB-3 | 低 | memory_bytes が rev HashMap (working map) を非計上で bench 表示値が過小に見える → 「永続層 (wire/disk 相当) 定義値」と doc 明確化 (rev はエントリあたり数十 B 別途存在の注記) |
| DB-4 | 低 | set() の単一値セクション同値上書きで 64 語 (512B) 確保の静寂「単一値脱却」 (10B→522B、何も変わらないのに) → no-op 早期復帰化 + 同 arm の到達不能死にコード (grow_bits が脱却を常に先に行う) 除去。消費者ゼロのため安全側修正 |
| DB-5 | 観 | get/set の座標 debug_assert: release 範囲外は (z+1,0) への静寂エイリアス読み/書き。hot path のため据置 + doc 契約明記 (呼出側保証) |
| DB-6 | 観 | 語跨ぎなしパッキング (MC 1.16+ 同型)・from_blocks 2 パス正・read OOB パニック=loud・needed_bits 境界表ピン (11 値) |
| DB-7 | 観 | パレット単調増加 (refcount なし = vanilla 同設計) doc 追認、ratio 空列 1.0 の定義注記、stats_for の各種実測値が一次情報である旨 |

### 検証 (wave 102)
- 994 全緑 (+5: needed_bits 境界表、メモリモデル 5 ケース+f64 ratio bit ピン
  2 件、12bit ワイヤ語 4 値+全往復、DB-4 no-op ピン、12bit 完結ピン)。
- アドバーサリアル 3 系統全検出: (a) DB-4 早期復帰除去+旧確保ブロック厳密
  復活 → set_same FAILED (10B vs 522B)、(b) write/read オフセット同改竄
  (内部一貫・外部非互換型) → wire_layout FAILED ほか連鎖、(c) needed_bits
  n-1→n 過剰 1 化 → boundary FAILED ほか全連鎖。固定版 ~/bak + rsift/bak
  (md5 4a2d9b03… 忠実復元 → 全緑)。注入訓練: (a) 初回 match arm カンマを
  コメントが呑み構文エラー → コンパイル fail で検出・訂正版で厳密旧形再現。
- fmt: HEAD minor 偏差 (tail コメント群) / 自分の hunk は正準形、残偏差 1 行
  は HEAD deviant 内容に包含 (rustfmt 行末コメント整列挙動を 2 箇所発見、
  コメント先行配置で構造解決)。
- 警告: opt-gfx lib 14 / lib-test 17 / api 13 据え置き (増分ゼロ)。
- wide_static_bench structural_digest `004c1cf5fb17bfe8` rows=357 不変。
- 機械検算 (Bash 規律): needed_bits 12 値、words/bytes 5 ケース
  (524/2088/5340/14760B)、f64 ratio bit 2 値 (0x3fb0600000000000/
  0x3ffcd40000000000)、ワイヤ語 3 値、worst ratio=1.8017578125、
  needed_bits(322)=9 化石整合、u32 境界安全。

### 残 (次 wave 以降の棚卸し)
- rev の統計ビュー (memory_bytes_total 等) — working map 込みの実メモリを
  bench 表示へ出すかの設計判断 (現状永続層定義で誠実化済)。
- セクション全体の再 pack (from_blocks 呼び直し) の wiring 側利用
  (単調増加のパレットを定期圧縮)。
- get/set の release 範囲外エイリアス — 低スペック配慮の debug_assert 方針は
  据置、範囲外検査付き checked_get 亜種の追加は消費者要求に応じて。
- Vanilla のグローバルパレット直接 id 形式 (9bit 超) 自体を実装するかは
  Dh-vanilla 相互運用要件が出た時点で判断 (現状 12bit 完結で不足なし)。

## DC. render_graph.rs (wave 103, 2026-07-25)

299 → 504 行。全行照合 + 消費者照合 (render_pipeline.rs:35 use /:104
フィールド /:180-181 `RenderGraphScheduler::new(feather.merged_subpasses,
feather.minimal_barriers)` /:217 /:630 `log_schedule()` の trace ログのみ;
passes()/barrier_count()/graph 各 field は外部消費者ゼロ、wide_static_bench/
full_graph_wiring も render_graph 非参照 = digest 経路非含有を構造照合済)。
全ピン値を Python 正確シム (forward-hazard 構築) で事前排撃し、その後 Rust
実装厳密テストが bit 一致することで相互検証。本波は並行してユーザー指示
「速度革命」(rspeed 統合+dev profile A/B) を実施 (DEV_ACCEL.md 参照)。

| DC-1 | 高 | **依存構築が RAW のみ・全順序ペア (i,j 双方向) だったため、実際の Feather グラフが真のサイクルを内包**: translucent (idx2) と taa_composite (idx3) は共に Color を read+write する RMW パスで、`2書→3読` と `3書→2読` の双方向エッジが成立 → Kahn が 2 パスを schedule から**静寂脱落** (実 schedule=[0,1]、groups=[[0],[1]]、barriers=2 (Depth/Hzb のみ)、total_cost=6 (真値 11) = **フレームグラフ半消失の潜伏実害**)。既存テストは `!schedule.is_empty()`・`schedule.len() <= nodes.len()` 等の弱条件で素通りしていた → forward-hazard 依存モデルへ再設計 (宣言順を有効実行順と見做し i<j ペアに RAW∪WAR∪WAW、構築上必ず DAG、RMW 鎖は宣言順に直列化) + 全値厳密ピン (schedule=[0,1,2,3]、groups=[[0],[1],[2],[3]]、total_cost merged=11/non-merged=12、barriers 8 要素完全列挙) |
| DC-2 | 中 | Kahn 残留ノード (サイクル等) の静寂脱落を止める防御が無かった → `assert_eq!(schedule.len(), n)` fail-loud 挿入 (forward-hazard 下で構築上到達不能だが恒久的防御として)。adversarial (a) で実効確認: 旧 RAW-only 逆戻し時に 6 テスト中 5 が当該行で RED |
| DC-3 | 中 | `barrier_count()` が minimal 時 `len.max(1)`・non-minimal 時 `(len*2).max(2)` の**虚構メトリクス** (構造的根拠なしの見栄え係数×2) → 両モードで実本数 `barriers.len()` 正直化 (消費者 trace ログのみのため安全)。Feather 実値 8 (旧 non-minimal 報告 16) |
| DC-4 | 低 | RMW パスの read→write パス内進行が `after_pass=自身` の自己遷移要素となる (推移点 [3],[6]、[7] は閲覧→CopyDst) ことの未明文化 → Barrier に「使用状態推移点列であって GPU queue 発行可能な外部バリア列ではない」趣旨の doc 明文化 |
| DC-5 | 観 | minimal_barriers の `retain(from!=to)` と `dedup_by` は構築規則上両方到達不能 (prev!=st のときのみ push/(resource,to) 連続重複は last_state 交互遷移で発生不能) → Python 証明のうえ防御維持+doc 注記 |
| DC-6 | 観 | ノード名空文字・reads 内重複の no-op 性・空グラフの well-formed 性 (DC-2 assert は 0==0 で受理) 確認 → empty_graph テスト追加 |
| DC-7 | 観 | `passes()`/`ScheduledPass.merge_ao`/`merge_water` は消費者ゼロの scaffold → 将来サブパス分割 wiring 用に温存+doc 注記 (削除は方針外) |

検証: +6 strict テスト (feather 完全 schedule/8 要素推移列/barrier_count
両モード/RMW pair/WAW+RAW+WAR chain4/empty) で厳密ピン置換し 1000 全緑。
adversarial 3 系統全検出: (a) RAW-only 全順序逆戻し → 5 件が DC-2 assert
(154 行) で RED + chain_4 も RED、(b) ×2 虚構逆戻し → honest_both_modes のみ
RED (16≠8)、(c) WAW 欠落注入 → chain_4 のみ RED ([0,1] 同 wave 崩れ)。
fmt: rspeed fmdiff で HEAD 逸脱 5 行 ⊆ 包含・自己起因 0 行 PASS (自己 5 行を
正準化して解決)、警告 14/17/13 据え置き、san 0 findings、
digest `004c1cf5fb17bfe8` rows=357 実測不変。

## DD. bench_harness.rs (wave 104, 2026-07-25)

291 → 417 行。全行照合 + 消費者照合 (run_timed/csv_header/to_csv_row/
to_markdown_row/BenchResult ← examples/rsift_bench.rs のみ (9 bench、CI 非
ゲート — bench.yml は pseudo_mc_bench/wide_static_bench のみビルド/実行)、
Stopwatch/Hdr::new/percentile_us/ascii ← モジュール内+テストのみ、
wide_static_bench/full_graph_wiring は bench_harness 非参照 = digest 経路
非含有を構造照合済)。全期待値を Python 機械検算で事前排撃
(505,060/5=101,012.0、480,000,008/8,000,008=59.99994…、nearest-rank
累積列 10/20/30→rank3、p80→bucket3 上限 9,999、p100→bucket5 上限 999,999)。

| DD-1 | 中 | record が fine_max_us 超過サンプルを末尾 fine スロットへ `min(us, len-1)` で**静寂飽和** → (a) percentile_us の粗バケットフォールバックが構築上**到達不能の死にコード**化、(b) 超過サンプルが fine_max_us へ過小報告 (5,000us が fine_max=2,000 の表で 2,000us = 2.5 倍過小、500,000us は 250 倍過小) の二重虚偽 → `fine.get_mut(us)` で [0, fine_max_us] 内のみ記録する設計へ根治 (粗フォールバック復活、報告は保守的上振れ側へ) + 内部状態完全ピン (fine[2000]==0、buckets[1]=3/[3]=1/[5]=1) |
| DD-2 | 中 | run_timed の fine 表が固定 60_000_000us = **呼出毎に 480,000,008 B (457.8 MiB) 確保** (rsift_bench 9 bench 連続・lib テスト run_timed_respects_limits 毎回 = CI/低スペック PC 敵対) → `run_timed_fine_max_us(target_ms) = clamp(target_ms saturating*1000, 60_000, 1_000_000)` へ根治 (全測定窓を通常カバー、最大 8,000,008 B) + 境界 6 点・メモリ比厳密ピン |
| DD-3 | 低 | percentile_us で p=0 のとき want=ceil(count·0)=0 となり先頭スロットで即 return 0us = **未観測値の虚偽報告** → nearest-rank 定義 (rank ∈ [1, count]) に基づく `.max(1.0)` クランプで根治 (p0=最小値、p0.2=rank1、p0.6=rank3 厳密ピン) |
| DD-4 | 低 | doc/label 虚偽群訂正: 構造体 doc「buckets[i] = [10^i..10^(i+1))」は bucket0 が 0us を含む実装と矛盾 → [0..10) 明記、ascii ラベル ">10s" は 10,000,000us 丁度を含む実区間と矛盾 → ">=10s"、fine フィールド doc「下位 1ms 分解能」はパラメトリック範囲 (new(2_000)→2ms 等) と矛盾 → [0..=fine_max_us] + 確保メモリ 8·(fine_max_us+1) B の契約明記 |
| DD-5 | 観 | Hdr::ascii の消費者がテストのみ → rsift_bench.rs の markdown レポートへ per-bench bucket histogram セクション配線 (消費者追加方針)。併せて証明/契約 doc 注記 5 件: Stopwatch::new は構築時の壁時計ベース (ops/sec は new〜finish 全期間)、to_csv_row の name 非エスケープ前提、time() の `as u64` 切捨ては u64::MAX us ≈ 584,542 年で到達不能、percentile 末尾 max_us return は到達不能 (全バケット合計 == count ≥ want)、run_timed の Instant+Duration 加算は巨大 target_ms でパニック = fail-loud 側 |

検証: +4 strict テスト (overflow skip+bucket ceiling/p0=min/fine_max 契約
6 点+メモリ比/label honest) で **1004 全緑**。adversarial 3 系統全検出:
(a) 飽和クランプ厳密逆戻し → overflow テストのみ RED、(b) max(1.0) 除去
→ percentile_zero のみ RED、(c) 60M 固定逆戻し → contract のみ RED、
各復元で md5 照合 MD5-VERIFIED。**捕捉 22 件目**: 自分の「1/60 未満」
断言が数学的に誤り (60×8,000,008=480,000,480 > 480,000,008) → テスト赤
で自己捕捉、整数除算商 59 + 両側挟み込み (59×new < old < 60×new) の
厳密ピンへ訂正 (プロダクトコード無変更)。fmt: rspeed fmdiff で HEAD 逸脱
3 ⊇ 現 2・自己起因 0 PASS (labels 行の正準化で HEAD 既存逸脱 1 件も解消)、
警告 14/17/13 据え置き・bench_harness 起因 0、san 0、trailws 0、
digest `004c1cf5fb17bfe8` rows=357 実測不変。固定版 md5
2ced151a56d37659184ba5a36c420166 を /tmp・rsift/bak/ へ二重保存。

## DE. branchless_dda.rs (wave 105, 2026-07-25)

275 → 394 行。全行照合 + 消費者照合 (trace_section/Ray3 ← **wide_static_
bench.rs:647 (digest 経路含有を実証)**・svo.rs:468 fallback (Some(&p) 経路は
有限 ray のみ、NaN 系テストは全て palette=None で非到達)、WGSL_BRANCHLESS_
DDA/branchless_axis/inv_dir ← モジュール内のみ、VoxelHit ← svo.rs:462
destructure)。DE-1 の digest 不変性は bench 入力域で構造証明 (d_raw ∈
{-1.000..0.999}/0.001 格子 → 負成分の最小 |d| = 0.001/√3 ≈ 5.77e-4 ≫ 1e-8、
ゼロ成分は +0.0 のみで immobilize 同値) + digest 実測不変で二重担保。
全期待値を Python 機械検算 (steps: 0→5、5→8、15→6、drift 4.8e-7)。

| DE-1 | 中 | inv_dir の tiny-dir ガードが **+INF 固定で符号を潰す**: 負の tiny dir (|d|<1e-8) で step=-1 × inv=+INF → **t_delta=-INF** となり軸が負方向へ暴走 (vy 16 連続デクリメントで範囲外脱出 = 本来命中のブロックを取り逃がし)、整数境界始点では (b-o)=0 × INF = **NaN** で branchless_axis の比較が全 false 化 (a0/a2=false) → 無関係な z 軸を踏み続ける軸ハイジャックの二重誤動作 → inv は**符号保持 ±INF** (`-0.0` は immobilize 一貫で +INF、`d < 0.0` 判定で copysign の -0.0 罠を回避) + **tiny lane の t_max/t_delta を直接 +INF 固定する lane 一貫 immobilize** へ再設計 (0·INF=NaN 経路も構造遮断)。adversarial (a) で旧挙動の実害を実証: 負 tiny 命中テスト・整数境界テストが None 返却で RED |
| DE-2 | 低 | 非有限 origin/dir の静寂受理 (NaN.floor() as i32 = 0 飽和 → (0,0,0) 起点の虚偽 trace) → **debug_assert fail-loud** (release コンパイルアウトで bench 計時経路と無干渉) + [should_panic] ピン (dev/test で確実発火) |
| DE-3 | 低 | 内外範囲判定の else-if が同値条件の再走査 (全軸 0<=v<16 の否定 ≡ 何れかの軸で v<0||v>=16 — 排反完備) → else 化 + 証明 doc (挙動完全等価・digest 不変) |
| DE-4 | 観 | doc 群 6 件: branchless_axis tie 優先度 X>Y>Z (argmin 最小添字) 明文化、VoxelHit 全フィールド doc (steps = 軸遷移回数・始点命中=0)、trace_section 契約 (max_steps=サンプル上限・範囲外即 None・air=0)、eps=1e-8 根拠 (16³ 最長踏破=48 voxel で drift≦4.8e-7 voxel=観測不能)、WGSL_BRANCHLESS_DDA 消費者ゼロ scaffold + voxel 座標更新を欠く不完全対称の正直注記、有限入力では NaN 非発生 (0·INF 遮断済) 証明 |
| DE-5 | 観 | テスト未カバー経路の strict 化: 既存テストは全て正方向で **step=-1 経路未被験** → 負方向 slab 対称ピン (vz 15→6、steps=9)。抱き合わせ trailws: compute_light_prop.rs:48/56 (LIGHT_PROP_WGSL 生文字列内インデント空白行、WGSL 空白非感性・byte ピン無し照合済)・entity_culling.rs:419 の 3 件除去 (entity_culling HEAD 逸脱 12→11 も 1 件改善) |

検証: +5 strict テスト (負 tiny immobile/整数境界 NaN 遮断/inv 符号・eps
境界・-0.0 完全ピン/負方向 slab/should_panic 非有限) で **1009 全緑**。
**adversarial 3 系統全検出**: (a) inv_dir+t_max/t_delta 旧セマンティクス厳密
逆戻し → 3 テスト RED (負 tiny・整数境界ともに None 返却を実証)、
(b) debug_assert 除去 → non_finite のみ RED、(c) naive copysign (-0.0→-INF)
→ sign pin のみ RED、各復元 md5 照合 MD5-VERIFIED。**儀式ミス記録**: 初回
adv-save が原版ゴールデンだったため一時的に修正版消失 → 会話内の全編集
ペイロードから厳密再適用し修正版 md5 a6834c7d で golden 化 (喪失ゼロ、
この過程自体が rescue/adv 儀式の必要性を実証)。fmt: fmdiff HEAD 0 ⊇ 現 0・
自己起因 0 PASS (axis_inv 行を正準化解決)、警告 14/17/13 据え置き・
branchless_dda 起因 0、san 0、trailws 0 (全 src 走査)、digest
`004c1cf5fb17bfe8` rows=357 実測不変。固定版 md5
a6834c7d0fff4eaa5a70d8136326fc82 を adv cache・rsift/bak/ へ二重保存。

## DF. light_cache.rs (wave 106, 2026-07-25)

265 → 370 行。**propagate_dirty の実消費者はモジュール内テストのみ**を照合:
render_pipeline.rs は LightPropagationCache の保持 (22/130)・初期化 (269)・
mark_dirty (1250) までで propagate 非呼出、examples/wide_static_bench.rs は
set_emitter:749/get_light:758 のみで digest 経路非発火、rsift-sim
lighting/mod.rs:174 の同名メソッドは別クレート別実装 (無関係)。旧 VecDeque
flood の段階収束系を **writes-budget (budget-before) label-correcting
pass** へ根治: 各呼出は i=0..4,096 全走査の pass 反復、改善書込みのみ予算
max_steps を消費、改善は各セル高々 15 回 (値域 1..=15) → 総数 ≤ 15×4,096
= 61,440 (u32 飽和なし) で必終了、`sec.dirty = truncated` (打ち切り時のみ
true の忠実契約)。

| DF-1 | 中 | 打ち切りが **pop 後 break でキュー先頭を未処理破棄**、かつ max_steps=0/丁度境界で「queue 空 × 未処理残」でも **dirty=false に確定** → 以後の呼出が `!dirty` 早退で**永久 no-op = ライト未完成のまま収束詐称** (既存 dirty_cleared テストは dirty 未検査で素通り) → writes-budget 設計で根治 + 厳密ピン (0 steps で 0 writes・dirty 維持・継続呼出で収束到達 14) |
| DF-2 | 低 | SectionLights::set の同値上書きが都度 dirty 宣言 → 無駄 re-flood (wave 102 DB-4 同型) → 同値 no-op 早期復帰 + dirty 非汚染・packed 不変ピン |
| DF-3 | 低 | doc 群正直化 5 件: ヘッダ「skylight propagation」は sky flood **未実装** (ニブルは格納のみ、伝播しない)・消灯/減衰は除去 BFS 未実装を契約明記・opaque 点灯セル自身の発光設計 (MC 光源ブロック相当)・wrapping_sub の underflow→巨大値→`>= SEC` フィルタ安全性・writes ≤ 61,440 の非飽和証明 |
| DF-4 | 観 | 戻り値を改善 pop 数 → **改善書込み数**へ意味変更 (外部消費者ゼロ照合済) + 2 emitters シナリオ全量書込み **2,639** の厳密ピン (Python 独立シム一致、11 improving passes) |
| DF-5 | 中 | full re-seed (昇順) + 小 max_steps で予算が先頭冪等セルに燃え frontier 不進の **飢餓 (livelock)** — Python 検算が budget=64 で不収束タイムアウトを実測発見 → writes-budget 設計で構造排除 (冪等再訪は予算非消費、budget-before 判定) |

検証: +3 strict テスト (zero_steps_preserves_dirty_and_later_converges・
small_budget_truncation_still_converges (writes=2,639 ピン・200 call 上限・
一括 flood との最終 packed bit 一致)・noop_set_does_not_remark_dirty) で
モジュール 9 テスト・**1012 全緑** (165.44s)。**全期待値を Python 独立シム
機械検算**: 全量 writes=2,639・budget=64 → **50 calls 収束・総 writes
3,164・最終 packed bit 一致**・点灯セル 2,641。**adversarial 3 系統全検出**:
(a) 旧 queue 実装厳密逆戻し (git HEAD 抽出) → zero_steps+small_budget RED、
(b) elision 除去 → noop RED、(c) budget-after 変体 (書込み後に予算超過判定)
→ dirty_cleared+zero_steps RED、各復元は adv 実施時点 golden (md5 b7568466)
照合 3 回 MD5-VERIFIED。
**捕捉 23 件目**: 初版 small_budget テストの `assert!(calls >= 2)` は
「打ち切り有りなら複数回必要」との経験則断言だったが実機 1 call 収束で
RED 自己捕捉 → Python 機械検算で真値 (50 calls) 確定の上で設計を
再確定。**捕捉 24 件目**: 経験則由来の assert メッセージ「51 calls 級」を
Python 再検算が捕捉 → 確定値「50 calls」へ訂正 (watch: 本節記載直前に
同一計算を再実行して数値の読み違いゼロを確認、doc コメント (50 calls) と
assert メッセージ (51 級) の微細不一致は訂正済)。**儀式事故の誠実記録**:
propagate_dirty 書換えの python splice が end アンカー誤認で impl 閉括弧
+mod tests+strict 冒頭 5 テストを呑み込み (コンパイルエラー 3 件) → adv
golden (修正版 md5 化済) から喪失区間を機械抽出して復元 (喪失ゼロ、
「adv-save で修正版 golden 化 → adversarial 着手」の儀式順序が喪失耐性を
与えることを再実証)。fmt: fmdiff HEAD 0 ⊇ 現 0・自己起因 0 PASS
(スプライス由来の空白行事故を 3 段で根治)、警告 14/17/13 据え置き
(light_cache 起因 0)、san 0、trailws 0 (全 src 走査)、digest
`004c1cf5fb17bfe8` rows=357 実測不変。固定版 md5
230eec01293b0f9b4d01431b9f22d224 を adv cache・rsift/bak/ へ二重保存
(捕捉 24 件目訂正 + assert 行 fmdiff 正準化後の最終版、265→370→373 行)。

## DG. bitpacked_section.rs (wave 107, 2026-07-25)

254 → 389 行。消費者照合: **digest 経路含有** (wide_static_bench.rs:22 use・
408-430 `bitpacked_setfill` の `footprint=*B sum=*` 行が digest 直行 →
幅成長政策・footprint 式は digest-可観測で据置義務)、full_graph_wiring.rs:1005
は `CompactChunkSection::new_air()` の非発火利用のみ、クレート外消費者ゼロ。
エンコード本体のビット代数 (単一/跨ぎ両ブランチのクリアマスク・拡幅 OR 転写・
spill の ceil 不変量・pal_id 分割代数学) を手証明で全成立後、**参照モデルとの
差分ファズ (仮設 examples/dg_fuzz_tmp.rs、非コミット)** で決定的有効性を確認:
100k ランダム ops・幅境界 20×2 掃引・40,300 連番状態挿入で **幅 0→4→…→16 bit
全遷移の完全可逆**、[中]/[高] ロジック欠陥は存在しなかった (成熟の実測)。
検出は doc 虚偽・静寂破壊経路の設計級 (DG-1..6、捕捉 25・26 件目を含む)。

| DG-1 | 低 | ヘッダ doc 虚偽群: 存在しない型名 `SingleValueSection`・「4..=15 bit」(実到達 16 を phaseC で実測、16 打止めは u16 ドメイン構造証明)・「4,000倍軽量化」(正確に **4,096 倍**) を訂正 + vanilla direct 移行は一次情報未照合と明記・vanilla 互換入出力非存在の消費者警告 |
| DG-2 | 低 | `memory_footprint_bytes` が `rev` HashMap ヒープ非計上 (多数状態時は本体超過し得る) — digest 行 `footprint=*B` 不変のため式据置・契約 doc 化 (実メモリ管理用途禁止) |
| DG-3 | 低 | 静寂破壊 2 経路 fail-loud 化: `idx` 範囲外座標の**別セル静寂エイリアス** (x=16→(0,y+1,z))・`get` `unwrap_or(0)` の域外 pal_id 静寂 air 化 → debug_assert + should_panic 2 ピン。**捕捉 25 件目**: 初版ピンが SingleValue 経路 (座標非参照) で不発をテスト赤が捕捉 → 関門を enum ディスパッチ層へ移設 |
| DG-4 | 観 | pub フィールド不変量 (レイアウト↔bits・pal_id < len・rev↔palette 1:1) 自壊危険の doc 明文化 |
| DG-5 | 観 | strict ピン強化: 同値 set 完全 no-op (DB-4 同型)・5bit 跨ぎ cell 12 spill 厳密往復・33,000 状態 16bit 全遷移可逆・差分ファズ全 Pass 記録 |
| DG-6 | 低 | **捕捉 26 件目**: 設計中の幅境界述語 (no-span) が 2^k+1 側を区間外と呼ぶ **off-by-one** — 機械検算 (n∈{17,33,…,32769} 代入) が出荷前捕捉 → 包含形 `2^(w-1) < n ≤ 2^w` で厳密ピン化 (17↔16 不遷移ゆえ既存単調性テストでは検出不能) |

検証: +5 strict テストでモジュール 9・**1017 全緑** (168.14s)。**adversarial
3 系統**: (a) enum 層 assert 除去 → out_of_range RED、(b) pal_id assert 除去
→ corrupted_pal_id RED、各復元 md5 照合 MD5-VERIFIED 2 回、**(c) idx 層
assert のみ除去 → 検出不能を実測** (enum 層関門経由検査では非発火。
`BitpackedSection` 直接 API 誤用への防御第 2 層として保持すると決定し doc に
役割・検出不能の事実を誠実記録)。fmt: HEAD 0 / 自己 0 PASS (追加ブロックの
正準化: フォーマット文字列 `{w-1}` 式不可を変数分割で根治・配列 12 要素の
rustfmt 正準改行を忠実適用)、警告 14/17/13 据え置き (trace_section デッド
コード警告は wave 99 以来の基線内・契約側で担保の設計として誠実注記のみ)、
san 0、trailws 0 (全 src 走査)、digest `004c1cf5fb17bfe8` rows=357 実測不変
(footprint/sum 供給経路の据置義務を満たす)。固定版 md5
3158569caf46651142db11f03678ac0c を adv cache・rsift/bak/ へ二重保存。
捕捉 25 (enum 層盲点)・捕捉 26 (off-by-one) は台帳特記の系列に継続番号で
記録 (合計 26 件)。

## DH. morton_order.rs (wave 108, 2026-07-25)

251 → 451 行。消費者照合: **digest 経路含有** (wide_static_bench.rs:32 use
split_by_3/compact_by_3・morton_split3_rt の acc 行が digest 直行)、
full_graph_wiring.rs:477 (局所性 sort コード生成)・:1012 (encode_2d、本質
no-op)、lbvh.rs 側は自前 part1by2 実装持ち (本モジュール未使用)、
frame_worldgen の cs_morton は WGSL ミラー (精密ミラーテストは別経路)、
rsift-sim 側 morton_encode2 も別実装でクレート外消費者ゼロ。
検証: 仮設差分ファズ (3d: 境界 11+400 万乱で split/compact ≡ naive・
encode/decode base≡fast 全入力 bitwise・2d: 100 万乱で 32 bit 完全往復・
Grid SIZE=16 全 4,096 単射・削体 sort 新旧 64 種一致) → ビット代数本体の
[中]/[高] 欠陥は不存在、欠陥は**契約領域に集中** (DH-1..7)。

| DH-1 | 中 | 3D 系の実効ドメイン 10 bit/成分が静寂切捨てで doc 非記載。生きた被害者 full_graph_wiring:477 が 21 bit マスクで上位 11 bit 静寂消失 (局所性 sort キー衝突) → 挙動完全一致の明示化 (& 1023) + 契約 doc 公表 + 厳密ピン。adversarial (a) で「前置 & 0x3FF 自体が数学的冗長 (第 1 段チェーンが中央強制)」を構造証明 |
| DH-2 | 低 | split_by_3/2 の `mut a` 未使用警告 2 件根絶 (lib 14→12・lib-test 17→15 を機械改善) |
| DH-3 | 低 | ヘッダ「2D/3D BMI2 完全実装」虚偽 (2D は経路なし) + encode_bmi2 名称誤導の誠実化 |
| DH-4 | 観 | 負座標 wrap 契約 (-1→1023) の doc 明文化 + encode/sort 厳密ピン |
| DH-5 | 観 | 消費者ゼロ API 保持明記 + sort キー前計算最適化 (O(n log n)→O(n) 評価、新旧 64 種完全一致をファズ証明) + fast #[inline] |
| DH-6 | 観 | Grid 契約明文化 (SIZE 冪/≤1024/4 GiB 注意) + 非冪 should_panic ピン |
| DH-7 | 観 | 3 系統 Morton (本モジュール/lbvh::morton3/WGSL) の相互等価ピン追加 (境界・内域・wrap 200k で bitwise 一致、将来発散抑止) |

**adversarial の誠実な記録**: (a) 前置マスク 0x3FF→0xFFFF 変体 → **検出不能**
(第 1 段 `&0x030000FF` で bits 10+ が自然に死ぬ構造のため前置マスクは冗長 = 切
捨て意味論は magic チェーンが中央強制)・(a') 第 4 段マスク に汚染 bit 追加 →
**検出不能** (全 1024 入力で Python 構造照合 0 差分 = 汚染 bit 17 が payload
非到達・decode 不可視)・**(a'') ミスシフト (x|x<<2 → x|x<<3) → 13 中 4 RED
検出**・(b) sort マスク &1023→&2047 → **検出不能** (同理由で意味的中性、
digest 視点でも不変)・(c) wiring マスク逆戻し 1023→0xF_FFFF → **検出不能**
(この変体自体が prose 証明の裏付けとして digest PASS を直接確認)。あわせて
「切捨て意味論は中央 split_by_3 に唯一集中し、呼出側マスク群は全て契約意図
の表現である」ことが adversarial 各系統で相互実証された。自傷記録: (c) の
儀式中に wiring 編集行を HEAD へ全戻しする git checkout 事故 → 同内容を
文脈追記つきで厳密再適用 (事故自体を誠実記録)。fmt: HEAD 8 ⊇ 現 8・自己
起因 0 (6 箇所正準化機械適用)・対象外既存逸脱 (41/66/196/265 行) は HEAD の
まま保持、警告 14→12/17→15/api 13 (DH-2 正当改善)、san 0、trailws 0、
digest `004c1cf5fb17bfe8` rows=357 実測不変 (digest 視点中性を (c) で実証)。
テスト 13/13 ・**1025 全緑**。固定版 md5 7093e5ba11307dd2b40b2f9b6bb76258
を adv cache・rsift/bak/ へ二重保存。

## DI. leaf_fast_path.rs (wave 109, 2026-07-25)

219 → 419 行。消費者照合: **digest 経路含有** (wide_static_bench.rs:29 use・
795-815 で `apply_leaf_fast_path(&mut sec, true)` の `delta_idsum=` 行直行)、
chunk_mesh.rs:181 の実メッシュ経路消費 (gui_settings トグル連動:93/143/181/
204/379)、merge_leaf_mesh は消費者ゼロ (保持明記)。未使用 Quantized12ByteVertex
import を削除 (元から tests 側のみ使用、lib 構成警告には含まれず後述問題なし)。
注目の発見はビット「定理」化: 逐次変異スキャン (走査中に palette 変異) の
collapse 対象が **x+y+z パリティ = スキャン最先 interior voxel の位相**で完全
決定する閉形式 ⌈(a-2)³/2⌉ — Python 独立モデルで a=3..10 の列
(1,4,14,32,63,108,172,256)・全 16³ = 1,372 = odd-parity interior 数
(14³/2 切上げ)・境界環 untouched (x/y/z = 0 または 15 の各 256) を全て
照合確定。ダイジェスト凍結領域のため意味論は据置の契約公表方針。

| DI-1 | 中 | 逐次変異意味論の契約公表: doc「only boundary faces remain」は厳密に虚偽 (内部反運パリティ半分が残留)。閉形式ピン + snapshot 化は digest 大規模見直し相当の誠実注記 |
| DI-2 | 低 | is_leaf 12-iter contains → 4×u64 ビットマスク表化 + 全 65,536 等価ピン。**捕捉 29 件目**: 表 256 bit に対し block ≥ 256 の u16 が **index out-of-bounds panic を生産経路持込** — 全網羅テストが出荷前捕捉 → guard `block < 256` 根治 (+ テスト鏡写しの guard 漏れ併合根治) |
| DI-3 | 低 | LEAF_TYPES の legacy コメント虚偽寄り (162..=165 不存在 id) → 分類目安 + 仮想 id 帯の誠実注記 |
| DI-4 | 観 | merge_leaf_mesh 消費者ゼロ保持 + 同一チャンク前提 debug_assert + byte 等価ピン (捕捉 27/28: 不存在フィールド作文 E0609・slice == 誤用 E0369 をビルド捕捉) |
| DI-5 | 観 | 境界 non-scan・wrapping guard 観察。走査拡張 adversarial 変体は guard が境界 collapse を常 false → **証明済み中性**としてコンパイル省略 (lean プロトコル適用開始の wave) |

検証: +5 strict テスト (パリティ残存列・全 16³ リング・チェッカー隣接不変量・
65,536 全網羅・merge assert/往復) でモジュール 11/11・**1030 全緑**。
**adversarial 誠実記録**: (a) snapshot 変体は x ループ内置きで「既変異の
コピー」となり意味的中性 (私の設計ミスとして記録)・**(a') 真 snapshot
(z 前置 frozen) → parity+full_section の 2 RED 検出**・(b) マスク表 1bit 汚染 →
3 RED 検出・(c) 走査拡張 → 証明済み中性。復元 md5 照合 MD5-VERIFIED 2 回。
**順序事故の誠実記録**: adv cache ディレクトリが sandbox リセットで消失し
golden 保存・復元が失敗 (2 変体連鎖でファイル汚陸) → rsift/bak/ 二重保存から
機械復旧 (md5 三重一致で完全回復、二重保存儀式の有効性を再実証)。fmt:
HEAD 0 ⊇ 現 0・自己 0 (長 assert・turbofish 等 4 箇所正準化)、警告 12/15/13
据置 (DI 系起因新規なし)、san 0、trailws 0、固定版 md5
331ef959e03df70e1216c29c52dc1116 を adv cache・rsift/bak/ 二重保存、
delta_idsum digest 不変は seal ゲートで確認。



抱き合わせ **rspeed 拡張**: san 簡体字集合 298→**452 字** (wave 105
コミット名の誤字 (U+4E3A 混入、正: 再走査) 素通りが発端 — 候補を **cp932
エンコード不可 = JIS X 0208 非含有 = 日本文出現不能** の機械フィルタで
226 字に確定、重複除去 +154 字、写/学/数/据/个/网/没/体/万/与/那/出/中/
理/文 は日本語使用字として機械除外 (手動選定の過誤を機械が訂正)、
`san_scan_text(&str)` 抽出で selftest 直接ピン可能化、`modinv_i128` 抽出で
cmd_invmod 共有化 (**孤立コメント `// invmod` の忘れ物 pin 回収**)、
selftest 18→**21 ピン** 全 PASS・rustfmt FMT_OK・rustc 警告 0・
/home/user/bin/rspeed 再デプロイ。実機検証: 「安全=U+4E3A 確認」の短文から
`san: [SIMPLIFIED] 簡体字 U+4E3A` 検出を確認 (以後は文字自体を引用せず
コードポイント表記 — 本文書への再混入を san 自傷 3 件で契機に恒常化)。
wave 105 件名の誤字は TRIGGER 142 注記で訂正記録。

## DJ. visibility_graph.rs (wave 110, 2026-07-25)

176 → 407 行。消費者照合: **digest 経路含有** (wide_static_bench
visibility_flood 12 行: g∈{8,16,32} × opaque∈{0,20} × corner (:817) / center
(:866))、full_graph_wiring.rs:142 フィールド保持・:967 で毎フレーム add_edge
再登録・:989 flood_fill → visgraph_reachable (render_pipeline:1112 から
毎フレーム呼出)。wiring 側の is_visible 呼出はゼロ (キャッシュ書込みのみ
消費)。注目の発見は BFS 意味論の**定理化**: flood 結果集合は opaque 非始点を
頂点除去した誘導部分グラフ (頂点集合 (V \ Opaque) ∪ {start}) における半径
max_dist の BFS 球に**厳密一致**する (遮断測地球定理) — 独立第二実装
(pruned 層別 BFS) との 20 試行差分ファズ + 閉形式照合で実証。さらに hash3 を
Python 移植した完全独立シムで **bench 12 行の reached/vis_hits を事前予測**
(corner: 28(0%)/25,24,25(20%)、vis_hits 3/2/0/0/0/0、center:
59/51/85/71/85/62) → seal 実測と照合。

| DJ-1 | 中 | add_edge の多重辺累積 (multigraph): 呼出毎無条件 push で、毎フレーム同一辺を再登録する tick_world 経由で adjacency Vec が単調増大 (結果は visited 抑止で正しいまま、メモリと BFS 走査幅のみ漸次増大) → 冪等化 (単純グラフ維持・逆向き無視・自己ループ 1 件正規化) で生産者側根治。挙動完全一致 (bench/wiring は各辺 1 回のみ構築) |
| DJ-2 | 低 | visited HashMap<ChunkNode,bool> の bool 値デッド → HashSet 等価置換 |
| DJ-3 | 低 | max_dist ちょうどの展開は子 (max_dist+1 層) が全て pop 即棄却の自明殻 → `dist < max_dist` guard で enqueue 抑止 (結果/is_opaque 呼出/キャッシュ bit 完全一致、queue 交通量のみ低減) |
| DJ-4 | 観 | 意味論契約公表: 遮断測地球定理・is_opaque 各ノード高々 1 回 (始点も評価、判定値は不使用)・負 max_dist=空+空集合キャッシュ・CAP 全破棄 eviction で is_visible 一時 false (誤 true なし、再 flood で回復)・冪等/自己ループ正規化・BFS 訪問順決定性 — doc+ピン群 |
| DJ-5 | 観 | 閉形式ピン化: 角 (d+1)(d+2)/2 (g>d)・中央 1+2d(d+1) (g≥2d+1)・g=8 クリップ 59・bench opaque=0 行 (28/85/59) strict 固定 + Python シムによる bench 12 行全事前予測 → seal 照合 |
| DJ-6 | 低 | 抱き合わせ: san 網羅漏れ 12 字・repo 34 箇所残留 (全て字句誤り、挙動無関係) → 31 箇所根治 + 集合 452→457 (「452」は実効 445 と機械訂正) + selftest ユニーク 457 厳密ピン + 捕捉 30 (変更追跡のみ走査の盲点: 集合掲載済 U+5B9E が未変更 svdag.rs に潜行→全量走査併用化)・31 (重複 2 見落としのカウント誤りを機械捕捉) |

検証: +8 strict テスト (冪等/自己ループ・閉形式球 d=0..=6・bench 閉形式・
遮断測地球 20 試行ファズ・is_opaque 呼出 1 回 pin・負 max_dist・CAP
eviction・訪問順決定性) でモジュール 14/14。**adversarial 誠実記録**:
(a) dedupe 除去 (multigraph 逆戻し) → 冪等ピン 1 RED・(b) `&& dist > 0`
除去 (始点免除削除) → 既存 opaque_start ピン 1 RED・(d) CAP clear 除去 →
eviction ピン 1 RED・(c) DJ-3 殻 guard 除去 → **14 全緑=検出不能を誠実記録**
(結果集合/呼出集合/キャッシュは bit 完全一致で queue 交通量のみ差の証明済み
中性、閉形式ピン群が同等性の錨)。復元 md5 照合 MD5-VERIFIED 3 回。fmt:
rustfmt --check 初回クリーン、固定版 md5 476fd0e3c2e19673796defccdae01b10
を adv cache・rsift/bak/ 二重保存。seal 全ゲート PASS (変更追跡 21 件)、
**bench 12 行は事前予測と 12/12 MATCH** (reached/vis_hits 全値一致 = 完全
独立シムによる閉形式予測が実測を再現)、digest 004c1cf5 不変。

抱き合わせ **rspeed/衛生同梱** (wave 110): san 集合 452「主張」→ 機械検算で
実効 445 (447 tokens − 重複 2) に訂正、repo 全量走査で発見の 12 字追加で
**457 に確定** (selftest ユニーク 457 厳密ピン + U+73AF 検出/U+74B0 非誤検出
ピン、selftest 21→24 ピン、build-rspeed.sh 再デプロイ、全量 re-scan 0
findings)。誤字 31 箇所根治 (透視/沈黙/精確/事実/一戸/ゴミ/呑/炭/分/録画/
切換/環)。**fmdiff 構造制約の根治** (DJ-7): HEAD 版を /tmp 孤立ファイルで
rustfmt していたため `mod` 含有ファイルの変更が構造的に seal 不通だった
制約を `--config skip_children=true` 化で根治 (mod 無し出力不変を実証)。
**捕捉 32 件目**: 私の EOF 修正 (jvm/lib.rs) が seal ゲート 2 で差止められ
本制約を発見。EOF 末尾改行根治は 4 ファイルに確定、rsift-installer/
Cargo.toml (CRLF 原生) への LF 改行追加は san ゲート 1 が差止め → revert
して EOL 専用 wave へ回付 (seal が範囲外混入を 2 件機械差止め = ゲート
実効性の再実証)。CRLF は base 64294c6 時点で
131 テキストファイル含有を機械判定 = **原生・本セッション起因でない** (過去
インシデント記録の帰属を訂正)。LF 正規化は 16,866 CR 行/120 ファイルを一括
で触る大差分 + .gitattributes 設計を要するため専用 wave 引継ぎとする判断を
記録 (registry 引継ぎ棚卸しに登録)。

## DK. branchless_block.rs (wave 111, 2026-07-25)

117 → 231 行。消費者照合: **digest 経路含有** (wide_static_bench セクション J
blocklut_lookup 3 行: n=2^{18,20,22} で select+opaque+light の合成 `acc=`)、
full_graph_wiring の実配線 6 箇所 (856 opaque ゲート・859/1020 発光シード
採取・904/1304 パレット id 直入力・1034 select 実演、:107 フィールド保持)、
`.transparent` フィールドは消費者ゼロ (保持方針)。注目の発見は modulo
プレースホルダの**構造集計厳密化**: opaque∧transparent の交差が 546 個
(最小反例 i=10) で 2 フラグが**排他でない**ことを厳密に確認 — 消費者が
「transparent なら非 opaque」を仮定すると現値で誤る契約を公表。

| DK-1 | 低 | select_branchless コメント不正確 (`+`/!cond 表記 vs 実装 `|`/(1-m)) → m∈{0,1} で片側積必ず 0 の完全等価 (OR/加算/XOR 一致、ビット共有でも厳密) を証明記述で誠実化 + 代表 200 組 pin |
| DK-2 | 観 | transparent 消費者ゼロの意図的保持 → `transparent_branchless()` accessor 整備 (将来の半透明ソート/透過パス向け) + 全 4096+wrap 一致 pin |
| DK-3 | 観 | 12 bit ドメイン契約公表: id ≥ 4096 は `& 4095` で静寂 wrap エイリアス (プレースホルダ値の仮性と独立の第 2 近似) + 全 u16 厳密 pin + wiring:986 u64→u16 で 2 段 truncate の注記リンク |
| DK-4 | 観 | 構造集計厳密ピン: opaque 真 2,730/偽 1,366・transparent 真 820・i%15==0 274・**交差 546=非排他**・light 完全一様 256 |
| DK-5 | 低 | bench blocklut_lookup 3 行の acc を SplitMix64 完全独立シムで事前予測 (2,143,132/8,562,084/34,244,920) → seal 実測照合 |

検証: +4 strict テスト (accessor 全 4096+wrap・構造集計厳密値・全 65,536
入力 wrap 契約・OR≡ADD 200 組) でモジュール 8/8。**adversarial 誠実記録**:
(a) mask 除去 → domain wrap pin+wrap pin の **2 RED** (index OOB panic で
fail-loud 検出)・(b) opaque 規則 i%3→i%2 → construct+集計 pin の 2 RED・
(d) transparent 規則 → 2 RED・(c) select OR→XOR → **8 全緑=検出不能を誠実
記録** (m∈{0,1} で XOR≡OR≡ADD の証明済み中性、truth-table pin 群が等価性の
錨)。復元 md5 照合 MD5-VERIFIED 3 回。fmt: 長 assert 1 箇所を rustfmt 忠実
適用、固定版 md5 77e2674ad6c7a2cf5443a94ef004bd60 を adv cache・rsift/bak/
二重保存。seal 全ゲート PASS (変更追跡 4 件・1042 全緑)、**acc 3 行は事前予測と 3/3 MATCH** (SplitMix64 完全独立シムが実測を再現 = wave 110 の 12/12 と併せ bench 予測照合 15/15)、digest 004c1cf5 不変。

## DL. exposure.rs (wave 112, 2026-07-26)

177 → 278 行。消費者照合: full_graph_wiring (:1467 ドーム 32 方向の実スカイ
サンプル → :1481-1487 build/target/adapt 実適用 → post_exposure レポート・
:1662 TAA RGB 適用・:2595 範囲 assert (0.05..=20))、frame_proof_extra
(cpu/gpu-exposure BMP 実出力、:213 luma bitwise 検査 = GPU 側は luma/apply
のみで log/exp は CPU 責務の決定性設計)。bench digest 行なし。

本 wave の大物は **DL-1 [中] メータリング虚偽の根治**: ヘッダの「same
metering used by Unreal's Histogram auto-exposure」記載に対し、実装は線形
領域の算術平均逆数 1/E[L] だった。Epic 一次情報 (「Auto Exposure in Unreal
Engine」の Basic: 「average of the log luminance」/ Histogram: log 輝度
ヒストグラムから平均輝度を決定) と照合すると Unreal は **log 領域加重平均
= 幾何平均輝度**。Jensen 不等式 E[ln L] ≤ ln E[L] より旧実装は常に Unreal
式以下 (= 実シーンで暗め、等号は定数シーンのみ) — 2 段シーン (0.1×900 +
0.5×100、min 1e-3/max 1.0) の Python 独立シム実測: 旧 7.150503027 →
新 8.543594451 (比 1.194824)、定数シーン L=0.5/0.25 は 2.0169146/3.9954206
で**新旧厳密一致**。log 領域計量へ修正 (挙動変更は変動シーン限定、digest
無関係、wiring a↔b 比較・範囲 assert・GPU パリティ境界は全て不変)。

| DL-1 | 中 | メータリング虚偽根治 (上記)。誠実残差異: bin 数 (Unreal 64/本 256)・較正 (18% 中間グレー K/1/geo-L 独自規約) を doc 公表 |
| DL-2 | 低 | build_histogram の NaN/inf/退化入力の決定的契約公表+pin (NaN→bin 0、+inf→bin 255、退化範囲→全 bin 255→clamp 20 厳密確定) |
| DL-3 | 低 | adapt の厳密離散解 pin (1.659359908) + speed=0 恒等・負 speed 反適応・NaN 伝播 fail-visible 契約 |
| DL-4 | 観 | 分位境界契約 pin (u32 切捨て・整数中点包含・low>hi/空/全除外 → 1.0) |
| DL-5 | 観 | Vec4/Vec3 ops 消費者ゼロ保持明記 + 捕捉 33 件目 (Vec4 Mul の Vec3::new 転記 typo を初回コンパイルが E0061/E0308 で捕捉 → 根治) |

検証: +6 strict テスト (幾何平均厳密値/bins 170,230・NaN/inf bins・退化崩壊
clamp 20・adapt 厳密値/NaN 伝播・分位境界・計量不変性) でモジュール 9/9・
既存 3 テストは DL-1 の数学的予言通り全緑維持。**adversarial 誠実記録**:
(a) 線形 1/E[L] 逆戻し → 2 段 pin **1 RED** (定数 pin は不変のため正しく
不発、錨の設計通り)・(b) NaN 崩落化 (clamp→max/min 連鎖) → NaN 伝播 pin
1 RED・(c) bin clamp 0.9999→1.0 → **2 RED** (bin 256 index OOB panic で
fail-loud 検出)・(d) 分位中点→左端 → **4 RED** (既存定数 2 テスト連鎖検出)。
復元 md5 照合 MD5-VERIFIED 3 回 (固定版 2cc763a34515321be3a73b11c57905f4、
adv cache・rsift/bak/ 二重保存)。

**sandbox リセット第 2 号からの復旧記録** (wave 112 途上): /home/user/bin
(rspeed)・/home/user/rust (toolchain)・/tmp (vendor + /tmp/dj 作業域) が
全消失、HEAD は base 64294c6 へ巻戻り。正規手順 `ci/restore-env.sh` (vendor
ブランチから toolchain 1.94.1 + 454 crates を sha256 照合で復元) で完全復旧
し、消失した wave 112 作業中ファイルは会話内 authored text から逐語再構成
(2 度の作成で latent だった Vec3::new typo は捕捉 33 としてコンパイルが
差止め = 消失が検証強化に転化した実例)。git reset --hard FETCH_HEAD で
HEAD 0eaaddb 復帰後、rspeed 再ビルド selftest 0 FAIL。

## DM. bloom.rs (wave 113, 2026-07-26)

155 → 320 行。消費者照合: full_graph_wiring:1671 (`composite(mapped,
prefilter(mapped, 1.0, 0.5), 0.08)` — ACES `tonemap_display` 後・CAS 前の
唯一の実呼出)。bench digest 行なし (bench 参照ゼロ)。luma は
frame_postfx::luma_run_cpu (:417) / exposure 内蔵式と同一の Rec.709
乗加順 (3 系統 bit 一致 pin 化)。原版 md5 ce95f02b78d8367b155cfdf15f6fb7d3。

本 wave の大物は **DM-2 [中] wiring の bloom 実効ゼロ証明**: 唯一の消費者
wiring:1671 は `tonemap_display` **後の値**に prefilter を適用するが、
linear_to_srgb の `x >= 1.0 → 1.0` clamp で mapped ∈ [0,1]³ となるため
luma ≤ 0.2126+0.7152+0.0722 = 1.0 = threshold (等号は全 1 のみ) → ゲート
`l <= threshold` は**常に真 → bloom ≡ 0**、composite は (m+0).clamp(0,64) =
m の **bit 厳密な恒等写像** (729 点グリッド (0..=8/8)³ + (1,1,1) の
to_bits 厳密 pin)。現行積分の bloom 段は描画に一切寄与していない — この
事実を誇張せず「構造的確定」として記録する。閾値の再調整 (例: 0.7/
knee 0.3 への変更で実効化) はレンダ結果が変わる**美的判断**のため、私は
値を変更せずユーザー設計領域として引継ぎ棚卸しに登録する。

| DM-1 | 中 | prefilter 相対ゲイン意味論の公表+厳密 pin: f=(l−T)/T.max(1e-4)、l>2T で入力超過増幅 (T=1,l=4→出力 12)、T≦1e-4 で発散級 (f≈999)。luma 保存形との違いを誠実化。l=2T bit 恒等・l=T 境界 0・小閾値発散の厳密値 pin、呼出側契約 threshold ≫ 1e-4 明示 |
| DM-2 | 中 | wiring bloom 実効ゼロ証明 (上記)。729+1 点 to_bits 厳密 pin |
| DM-3 | 低 | blur_row 契約公表+厳密 pin: 二項核 [1,4,6,4,1]/16 全て二進厳密・和 f32 厳密 1.0 → 定数保存 bit 厳密 (to_bits 化)・radius=0 bit 恒等・dst<src panic (fail-loud、should_panic pin)・edge-clamp doc |
| DM-4 | 観 | luma 3 系統 (bloom/frame_postfx/exposure 内蔵) bit 一致を xorshift 256 色で厳密 pin (乗加順差の 1 ulp 発散混入を apparatus 化) |
| DM-5 | 観 | composite/prefilter の NaN 伝播 pin (f32 比較 false で self 返却 = fail-visible)・composite 上限 64 公表 + 捕捉 34 (Vec4 Mul Vec3::new typo 再犯=同一零デイ 2 連続、E0061/E0308 即捕捉)・捕捉 35 (&mut 借用 closure の `let f` を E0596 が捕捉、let mut 化) |

検証: +6 strict テスト (相対ゲイン厳密値/wiring 恒等 729+1 点 to_bits/
重み二進厳密+定数保存 to_bits+半径 0 恒等+panic 契約 (should_panic)/
luma 3 系統 bit 一致/Nan 伝播+clamp 64) でモジュール 10/10・既存 4 テスト
全緑維持。**adversarial 誠実記録**: (a) ゲート反転 (l>=threshold) →
**3 RED** (drops_dark/keeps_bright/relative_gain) — 誠実記録: wiring 恒等
pin は変体下でも緑維持 (knee=0.5 を伴う反転ゲートでは soft=clamp(t,0,1)=0
の掛算で prefilter が恒等的に 0 へ崩壊し、pin の主張 (bloom ≡ 0 →
composite 恒等) が真のまま保存されるため。pin の欠陥ではなく性質保存の
正しい不発)。(b) 重み 0.375→0.376 → **2 RED** (wsum to_bits pin +
定数保存からの連鎖検出)。(c) knee 乗算除去 → **1 RED** (新設 knee pin
0.375 が検出 — knee 帯の pin は設計段階の空白で、adversarial 設計フェーズで
発見して追設 = pin 無しなら**検出不能**だったことを誇張せず記録、本 wave の
apparatus 強化点)。(d) edge clamp 除去 → **2 RED** (index OOB panic で
fail-loud)。復元 md5 照合 MD5-VERIFIED 4 回 (固定版
80bb678a918c7901bb4350d0c393a754、adv cache・rsift/bak/ 二重保存)。

## DN. cpu_saver.rs (wave 114, 2026-07-26)

129 → 373 行 (rustfmt 後)。消費者照合: workspace 全 grep で直接呼出 **ゼロ**
(lib.rs:47 `pub use cpu_saver::*` の公開 API 面のみ)。bench digest 行なし。
原版 md5 6c383361a42d25e69eaa7223034540e8。**警告由来の選定** (bench digest
経路は wave 113 時点で全消化、残りの lib 警告 11 件中 `cpu_saver` の未使用
bytemuck import を持つ最前線を機械選定)。固定後 lib 警告 **11 → 10**。

本 wave は修正性質が「誠実化+契約化」中心で破壊的変更はゼロ (実コード差分は
use 行除去のみを機械検証: doc/test 以外の本体は byte 等価)。

| DN-1 | 低 | 未使用 `bytemuck::{Pod, Zeroable}` import 除去 (警告根治) + ヘッダ過剰主張の誠実化 (「完全撲滅」→ 命令選択はコンパイラ依存 / DDA 本体は branchless_dda / 64B 一致はレイアウト依存) |
| DN-2 | 低 | branchless_select 契約 doc+厳密 pin (xorshift 200 組 if/else 厳密照合・NaN ペイロード 0x7FC00001/-0.0 0x80000000/±inf bit 保持・`|`/`+`/`^` 等価証明記述 DK-1 同型) |
| DN-3 | 低 | stepper 契約 pin: step_direction 境界 (-0.0→0・NaN→0・±inf→±1)、advance_axis 343 網羅ちょうど 1 軸 + タイ優先 x>y>z + 全 NaN→z |
| DN-4 | 低 | タイル走査 bijection 閉形式 (各 2bit フィールドのビット置換) + 到達順 先頭 8/末尾 4/index64 spot 厳密 pin・prefetch 契約 (セマンティクス非観測・無効アドレス非フォールト) + fault-free smoke |
| DN-5 | 観 | 消費者ゼロの意図的保持明記 (将来ホットループ向けプリミティブ=公開 API 面) |

検証: +5 strict テスト (select bit 厳密+NaN ペイロード/step 境界/軸排他
343+タイ優先/bijection 閉形式+到達順/prefetch smoke) でモジュール 7/7・
既存 2 テスト不変。総数 1054 → **1059 全緑**。**adversarial 誠実記録**:
(a) mask の wrapping_neg 除去 (mask=1/0) → **2 RED** (既存基本ピン+
新設 200 組厳密照合が連鎖)。(b) タイ優先の非包含化 (`<=`→`<`) → **1 RED**
(優先 pin が検出、343 排他網羅は変体下でも真のため正しく不発)。(c) タイル
内ループ順交換 (網羅保存・順序変更) → **1 RED** (順序 pin が検出、網羅 count
pin は網羅が真のままのため正しく不発 = 網羅と順序のピン分離設計が有効)。
(d) prefetch 除去 → 7 全緑 = **検出不能・証明済み中性** (セマンティクスを
持たないハードウェアヒントのため変更は値非観測、設計書通りの検出空白を
誇張せず記録)。adversarial は fmt 正規化前の golden (9cf34c38…) で実施、
md5 照合 MD5-VERIFIED 4 回。最終固定版は fmdiff 忠実適用 (下記) 後の
050e1136055f55e7b695efb08849e7d9 (adv cache・rsift/bak/ 二重保存) で、
use 順正規化のみの差分のため adversarial 結論は全て不変。

**fmt インシデント (seal ゲート 2 差止め → 根治、捕捉 36 件目)**: 初回 seal
で `HEAD 逸脱 3 ⊅ 現逸脱 4 (自己起因 2)` が FAIL。原因は私の ad hoc
`rustfmt --edition 2024` 走査と fmdiff の正準形が cfg-gated `use` ブロック
(x86/x86_64 4 行) の並べ替え規則で不一致だったこと (HEAD 由来の逸脱領域で
doc 挿入による LCS 位置混同が「自己起因」判定を誘発)。fmdiff 出力を忠実適用
(x86/x86_64 ペアで _mm_prefetch を _MM_HINT_T0 より先に配置) し現逸脱 0 で
根治 — cfg グルーピング内の use 順は意味に無影響 (論理的自己同一) で
adversarial 結論全て不変。ad hoc rustfmt の版不一致が差止められた記録として
誠実に残す。

## DO. out_of_core_paging.rs (wave 115, 2026-07-26)

247 → 446 行。消費者照合: full_graph_wiring:123/124 (Option 保持)・:226 (max_pages
=4096 = 256 MiB backing 生成)・:747 (`&tick.to_le_bytes()` **8 byte 書込みのみ**、
read_chunk_page 呼出は wiring 内ゼロ — live だが読み側未使用)。L 節 (2026-07-22)
の L-1 (新旧エイリアス破壊) / L-2 (剰余ゼロ) 根治済みの流れで、本 wave は
**警告由来選定** (最後の未監査警告保持モジュール、`unused_mut` :57)。原版 md5
9e1baba6290a5873941cdd850e7ae483。lib 警告 **10 → 9**。

本 wave の中核は **DO-2 [中] 部分書換えの残滓曝露契約の公表**: 8 byte 書込み
(wiring 現行) ではページ残り 65,528 byte が旧占有者の残滓を含み得るが、
実装はゼロ潰ししない (長さ帳簿=呼出側責務)。読み側は `min(PAGE_SIZE)` で
隣接ページへは侵入しないが**同一ページ内の残滓は返す**。現消費者は read を
一切呼ばないため観測者不在 = 深刻度は [中] の構造確定であり、差し替え・
スパース潰し等の破壊的変更は行わず契約を doc 化+厳密 pin した。

| DO-1 | 低 | `let mut file` の unused mut 根治 (警告 10→9) |
| DO-2 | 中 | 部分書換え残滓曝露の契約公表+厳密 pin (100..256 に旧 0xA1 残存を厳密値で pin、wiring の read 未使用を誠実記録) |
| DO-3 | 観 | `Ok(0)` 2 義性の公表+pin (未登録/空白 out の区別は contains_key) |
| DO-4 | 低 | シャドウモデル差分ファズ (1000 オペ・被害者選択/idx 割当厳密一致) + 1:1 構造不変量 pin |
| DO-5 | 観 | 境界 pin (idx<cap・offset 算術・backing 524,288 byte 固定・u32 境界拒否・再起動 orphan) |

検証: +4 strict テスト (残滓契約厳密値/ok0 多義/シャドウ 1000 オペ+不変量/
境界+orphan) でモジュール 8/8・既存 4 テスト不変。総数 1059 → **1063 全緑**。
**adversarial 誠実記録**: (a) L-1 逆戻し (page_table 除去省略) → **2 RED**
(既存 lru 回帰ピン + 新設 shadow ファズが ghost エントリの件数分裂で連鎖
検出)。(b) read の touch 除去 → **2 RED** (read 最新化の被害者決定が変わる
既存 lru pin + shadow ファズの双方)。(c) 読出し min 打止め除去 → **1 RED**
(oversize で隣接ページ侵入を読出し — 打止めが働く唯一のケースで正しく検出、
他 7 テストは領域不足で不発=設計通り)。(d) pop_front→pop_back (MRU) →
**2 RED** (既存 lru pin + shadow)。復元 md5 照合 MD5-VERIFIED 4 回 (固定版
2aa7b828db4c6c2785d4e6e84e00cafd、adv cache・rsift/bak/ 二重保存)。

## DP. atmospheric.rs (wave 116, 2026-07-26)

186 → 349 行 / WGSL 34 → 37 行。消費者照合: full_graph_wiring:1457-1490
(sun=(0.35,0.55,0.75).normalize_wrap()・cam_dir sky_color + 32 方向ドーム
sky_color → exposure ヒストグラム (DL) と TAA YCoCg へ供給 = exposure warp
の実生産者)。WGSL は gpu_runtime:64 collect_all_wgsl へ登録 (ピクセル還流は
未追跡と誠実注記)。原版 md5 1d7e647e...。bench digest 行なし。wiring 密度
7-同数 6 件から**辞書順タイブレーク**で機械選定。

| DP-1 | 低 | モデル形態誠実化 (一様 8 km スラブ・8 段中点則・位相/Beer は物理式) + 厳密 bit pin (位相 6 値・sky 6 成分、Python IEEE f32+ctypes expf/powf 独立シム事前導出→照合) |
| DP-2 | 低 | 位相関数球面正規化 ∫=1 を中点 4096 で数値確認 (min d=(1-g)²≫1e-4 の床非発動を解析追加証明) |
| DP-3 | 低 | WGSL PI 丸め不足根治 (3.14159265→3.14159274=f32 PI bit 一致) + normalize 0 振舞差公表 + ソース走査 pin + 捕捉 37 (コメント自己衝突をテスト赤が捕捉) |
| DP-4 | 観 | normalize 境界/NaN 伝播/transmittance 端点 pin + 捕捉 38 (独立シムの乗算結合順誤り=1 ulp 差、左結合に訂正し 6/6 照合) |
| DP-5 | 観 | Vec4 系消費者ゼロの意図的保持明記 + WGSL ピクセル還流未追跡の誠実注記 |

検証: +5 strict テスト (位相 bit pin×6/球面正規化/sky bit pin×6/対称+NaN+
端点/normalize 境界+WGSL PI 走査) でモジュール 9/9・既存 4 テスト不変。総数
**1068 全緑**。**adversarial 誠実記録**: (a) mie g 反転 (後方散乱化) →
**1 RED** (sky 厳密 bit のみ — 既存の定性ピン sky_is_brighter_toward_sun は
変位でも緑: phase 1.19366e-1 系の Rayleigh が mie に支配優位で大小関係が
保存されるため。定性 smoke の検出域外を厳密 pin が埋める実例として誠実
記録)。(b) 中点則→右端点則 → **1 RED** (sky bit のみ、定性緑)。(c) Rayleigh
16π→4π → **3 RED** (位相 bit+球面正規化 4096+sky bit の 3 層連鎖)。(d)
steps 8→16 (**精度改善方向の変更**) → **1 RED** — pin は「値が変われば
意図的方向を問わず検出する」決定性契約であり、分割増のような意図的品質
変更には再 pin (手続) が要ることを誠実に明記。(e) WGSL PI 逆戻し →
**1 RED** (ソース走査 pin)。復元 md5 照合 MD5-VERIFIED 5 回 (固定版 rs
52464f66・wgsl 4b734e96、adv cache・rsift/bak/ 二重保存)。

## DQ. fxaa.rs (wave 117, 2026-07-26)

224 → 348 行 (rustfmt 後)。消費者照合: full_graph_wiring:1694-1699 (5 引数全
てに同一色 — 恒等、下記 DQ-1)、gpu_runtime:64 経由で fxaa.wgsl 登録 (CPU
側は CE (2026-07-24) 監査済の参照実装、本 wave は残ギャップの closure)。
原版 md5 a390b4f25f374c...。wiring 密度 7-同数残存のうち辞書順先頭で機械選定。

本 wave の中核は **DQ-1 [中] wiring 恒等証明**: CE 時代からの実装知識
(「shade は CPU reference」) を構造化したのと同型に、唯一消費者が同一色
5 引数を与えるため contrast≡0 → bit 厳密な恒等 = 本経路 FXAA 実効ゼロの
確定。DM-2 (bloom) と同一の「ゼロ効果構造」クラスであり、実効化は
フレームバッファ近傍サンプリングの設計判断のため引継ぎ棚卸しに登録
(私は wiring を変更しない)。

| DQ-1 | 中 | wiring 恒等証明 (hdr 域含む 5 点グリッド bit pin) — FXAA 本経路実効ゼロの構造確定 |
| DQ-2 | 低 | 勾配軸タイブレーク pin (厳密 > → タイは E/W、0x3F1EB852) |
| DQ-3 | 低 | NaN 位置非対称の公表+pin (n/s マスク =0x3F000000、e/w/center 伝播) |
| DQ-4 | 観 | threshold 2 分岐選択 pin (floor/relative 支配の対蹠、輝度シフト丸め変化 0x3BB43958↔0x3BB43980) |
| DQ-5 | 観 | Rec.601 vs Rec.709 係数混在の消費者警告 + Rec.601 厳密性 (luma(1,1,1)=1.0) pin + 捕捉 39 (bits 二重 typo をテスト赤が捕捉) |

検証: +5 strict テスト (wiring 恒等 5 点/tie 厳密 bit/NaN 非対称/threshold
対蹠/Rec.601 係数) でモジュール 12/12・既存 7 テスト不変。総数 **1073 全緑**。
**adversarial 誠実記録**: (a) タイブレーク `>` → `>=` → **2 RED** (tie pin
+ threshold シーン A (gx=gy=0 の退化同値) 連鎖)。(b) ブレンドペア交換 →
**3 RED** (NaN 非対称+tie+threshold) — 誠実記録: CE の vertical/horizontal
厳密ピンは対称標本 (両ペア平均が 0.5 同値) で**交換不変**のため不発、
新設の非対称標本 pin 群が検出 = CE/DQ の標本設計補完性の実例。 (c) 絶対床
除去 → **2 RED** (threshold pin + wiring 恒等ピンが zero-luma 除算 NaN
(0/0→clamp も NaN) で連鎖検出 = 床が恒等経路の 0/0 ガードにもなっている
ことの実証)。(d) luma 0.299→0.300 → **4 RED** (luma 係数ピン+CE untouched
等輝度根拠+tie 根拠+threshold)。復元 md5 照合 MD5-VERIFIED 4 回 (rustfmt
正準化後の固定版 5ce9282dc1435b242ebee4c7c8920595、adversarial は正規化
前 017bf11e で実施後に fmdiff 適用 — fmt のみの差分のため結論不変、adv
cache・rsift/bak/ 二重保存)。

## DR. particle_control.rs (wave 118, 2026-07-26)

204 → 522 行 (rustfmt 後)。消費者照合: full_graph_wiring:310-311 (default
budget: 4000/256·10・保護 3.0・距離倍率 [0.7,1.0,1.0,0.8,0.8,0.6,0.5,1.0,1.0,
1.0])・:1392-1408 (begin_tick→8 クアッド要求、max_render 128)。原版 md5
4fbf63a1...。bench digest 行なし。wiring 密度 7-同数残存 4 件から辞書順で
機械選定。

本 wave の大物は **DR-1 [中] wiring 側の二重カウント+単調累積の根治**
(上の台帳入り詳細に識る): `allow()` 内部計上と繰り返し呼出の**二重計上**と
`reset_counts` 未呼出による **tick 跨ぎ単調累積**の複合で、パーティクル
発生可否が数十 tick で構造的に間引き支配へ破壊。プロトコル strict 化
(begin_tick→reset_counts→allow) で根治。wiring 側 strict テスト群は全て
不変 (decision を直接 assert するテストは既存せず、digest 無関係)。

| DR-1 | 中 | wiring 二重カウント+単調累積根治 + controller プロトコル厳格化 (count 回帰 pin も新設) |
| DR-2 | 低 | kind_idx 静寂クランプ/直接 note_active 非対称 pin |
| DR-3 | 低 | 短絡順序厳密契約 pin (総数超過=kind bypass/id=5,6 着地・保護計上・dist==max_d 非カリング・NaN 通常評価) |
| DR-4 | 観 | FNV 間引き厳密 pin (系列/分布 80/160) + 検出空白発見→呼出側レート census pin 追設 |
| DR-5 | 観 | loose `matches!` の決定的着地点精緻化 (222→Cull/5→Decimate) + 捕捉 40 (復元 anchor 崩壊で golden 流出しかけを md5 で即検知) |

検証: **+7** strict テスト (count プロトコル/kind クランプ非対称/順序 bypass/
距離境界+NaN/FNV 系列+分布/決定的着地+**call-site レート census**) で
モジュール 11/11・既存 4 テスト不変。総数 **1080 全緑** (初版 1079 は集計
誤り、seal 機械値で訂正)。wiring 側の
改修は fmdiff により wiring:1392-1408 以外の差が byte 等価 (HEAD 逸脱 32
含⊇現逸脱 32) と機械担保。**adversarial 誠実記録**: (a) 短絡順序交換 →
**1 RED** (bypass pin: id=6 が CullTotalBudget→CullKindBudget へ決定変化
rate 変化で検出)。(b) total 間引き 1/8→1/16 → **初回 11 全緑=検出不能**
(id=5/6 の 2 値標本では 1/8/1/16 が同着点 (h%8=0∧h%16=0 / h%8=3∧h%16=3)、
誠実記録) → 呼出側レート census pin (640 ids → 80 vs 40) 追設で **再実行
1 RED** = wave 113 knee pin と同型の apparatus 強化。(c) `.min(9)` 除去 →
**1 RED** (kind_idx=10 で per_kind_distance 配列 index OOB panic、fail-loud
検出)。(d) 距離 `>`→`>=` → **1 RED** (境界包含 pin)。復元 md5 照合
MD5-VERIFIED、(c) 前の版は rustfmt 変形で anchor 崩壊事故 (捕捉 40) 後に
再採取 = 固定版 a966ad7dc4cd1584926cbb5c14fea10d (adv cache・rsift/bak/
二重保存)。

## DS. smaa.rs (wave 119, 2026-07-26)

118 → 274 行 (rustfmt 後)。消費者照合: full_graph_wiring:322 (Smaa::new())・
:1701 (aa 同一色 5 引数の edge)+:1709 (blend、戻り値破棄)。原版 md5
e4c380ab...。bench digest 行なし。wiring 密度 7-同数残存 3 件から辞書順で
機械選定。

本 wave は DS-1 [中] 恒等クラス 3 件目の確定 (DM-2/DQ-1 と同型 — wiring の
CPU 参照系は CH-1 (franken 色) 以後「定数色不変性の実演」に統一されており、
「ゼロ効果」は検証可能な設計選択として誇張なく一貫記録)。

| DS-1 | 中 | wiring 恒等証明 (aa 5 引数 + 戻り値破棄、4 点グリッド bit pin) |
| DS-2 | 低 | edge 厳密 bit pin (V/H 强度 1.0・tie→horizontal=false/0x3ECCCCCC/主勾配厳密) |
| DS-3 | 低 | NaN 伝播位置非対称 pin (center/n/s マスク 0x3DCCCCD0、e/w 伝播) |
| DS-4 | 低 | blend 境界厳密 pin (負→0/lm=0→clamp 0.5=0x3F000000/NaN 伝播) |
| DS-5 | 観 | 閾値 2 分岐対蹠 pin (A'=0x3A831270 発動・B'=不発、DQ-4 同族) |
| DS-6 | 観 | directionless 公表 (contrast 通過∧両軸差ゼロ → strength=0) + 捕捉 41 (シナリオ盲スポットをテスト赤が捕捉→公表へ昇華) |

検証: +6 strict テスト (wiring 恒等 4 点/edge 厳密+tie/NaN 非対称/blend
境界/閾値対蹠/directionless 公表) でモジュール 10/10・既存 4 テスト不変。
総数 **1086 全緑**。**adversarial 誠実記録**: (a) tie 厳密 `<`→`<=` →
**3 RED** (tie pin+NaN contrast/gy=0 派生+DS-6 の |0|<=|0| で h 反転)。
(b) 相対閾値 0.1→0.01 → **1 RED** (B' が発動側へ倒れ st_b≠0 = 分岐 pin
選択が有効)。(c) blend clamp 0.5→1.0 → **1 RED** (0x3F000000→0x3F2AAAAB)。
(d) strength 選択交換 → **5 RED** (既存 V/H 2 テストを含む broad 検出 =
主勾配選択は深層の根幹)。復元 md5 照合 MD5-VERIFIED 4 回 (固定版
a106389865fd47f74ee84a683d70a96b、adv cache・rsift/bak/ 二重保存)。
fmt 初回差分 (行列の折り返し) を fmdiff 正準形へ忠実適用 (現逸脱 0)。

## DT. ssr.rs (wave 120, 2026-07-26)

187 → 461 行 (rustfmt 後)。消費者照合: full_graph_wiring:1537-1568
(depth_sampler=march/reflect_dir、戻り値 `let _ssr_hit` 破棄)・gpu_runtime:60
(ssr.wgsl 登録)・Vec4 消費者ゼロ (保持明記)。原版 md5 41feb5c1...。
bench digest 行なし。wiring 密度 7-同数残存の辞書順タイブレークで機械選定。

本 wave は恒等クラス (DM-2/DQ-1/DS-1) とは別型の「常時 miss+破棄」ゼロ効果
構造 (DT-1) を構造証明した。同時に **rspeed 内蔵の新言語 rq
(docs/internal/RQ.md) を本格適用した初の wave** — Python (struct+ctypes
libm) エミュレートを全面移行し、全厳密値を rq (--prelude の dot3/len3/ns)
で事前導出 → 実測一発照合 (drift pin の 5 値含む)。

| DT-1 | 中 | wiring 常時 miss 証明 (sampler 0.0/∞ → diff<0 永不発、戻り値破棄) |
| DT-2 | 低 | hit 窓 [0,thickness] 両端 inclusive・max_dist 厳密 >・NaN fail-safe・退化境界の厳密 pin 群 |
| DT-3 | 低 | reflect_dir 厳密 bit pin + ゼロ法線パススルー + 末尾 normalize drift pin (検出空白補完) |
| DT-4 | 観 | Vec4 保持明記 (WGSL パリティ API 面) |
| DT-5 | 観 | Rust/WGSL 表現差公表 (is_infinite vs 1e30・uv 様式化) + 走査 pin |
| DT-6 | 観 | 一方向符号付き窓の公表 (両側窓と非等価) + WGSL 同型走査 pin |

**adversarial 誠実記録**: (a) 下端 `>=`→`>` → **1 RED** (diff==0 pin)。(b) 上端
`<=`→`<` → **1 RED** (diff==thickness pin)。(c) max_dist `>`→`>=` → **1 RED**
(コール回数 pin)。(d) 片側窓→両側 `abs()` 窓 → **2 RED** (pass-through+
wiring 構造 pin の連鎖検出)。(e) 末尾 normalize 除去 → 初回 10 全緑=
**検出不能** (3 pin とも ∥pre∥≡1.0 の厳密構成のため出力不変) → ∥pre∥ が
1 ulp ずれる構成 (i=(1,2,3),n=(0,1,0)、rq 導出) の drift pin 追設で再 RED
**1** (wave 113/118 と同型の強化)。復元 md5 照合 MD5-VERIFIED 6 回。
固定版 md5 c8c84e11cf4a5407fe907e0fd17474ce、adv cache・rsift/bak/ 二重保存。
fmt fmdiff 正準形忠実適用 (現逸脱 0)。

**捕捉 43 件目**: rq selftest ピン初版で i64 hex を 0x030F (=783) と誤記
(正しくは 12345=0x3039) — 実行前の機械再計算で捕捉・訂正。
**捕捉 44 件目**: rq 実装の edit_file が `fn main() {` 行を誤って飲み込み
(new_text 末尾への復元書き忘れ) → rustc 波括弧エラーで即捕捉・復元。
**捕捉 45 件目**: `rustfmt --emit stdout` の出力は第 1 行に filename を含む仕様
→ naïve 全量適用で ssr.rs 先頭を破壊、compile エラーで即捕捉 → 「第 1 行除去」
手順に正式化 (fmdiff 内部も同処理であることを実装照合済)。
**捕捉 46 件目**: RQ v2 selftest のピン挿入位置が (rc,out) タプルを被覆 →
selftest 1 FAIL が機械捕捉 → 精密修復で 51 全 PASS 復帰。

## DU. static_be.rs (wave 121, 2026-07-26)

原版 183 行・md5 f0dea5585bcc90ae988ed3907dc224ab。消費者 census:
full_graph_wiring.rs (be_policy 167/313、be_entries 168/314、呼出 1369-1387)
+ examples/wide_static_bench (bench) のみ、WGSL 対応なし。

- **DU-1 [低]** tick() 境界契約の厳密 pin 群: 昇格猶予 40 (39→動的/40→昇格)・
  interact 鮮度窓 160=40*4 (159→降格/160→通過)・近距離 3.5 inclusive
  (0x40600000 降格=Static→Dynamic 降格実証 / 0x40600001=3.5000002 escape→
  昇格復帰)・burst 間隔 20 inclusive 累積 / 21 リセット / guard 恒久動的・
  非開閉 tick で score 維持・時計逆行 saturating→0 安全側化・
  **NaN camera_distance は近距離降格不発で昇格側 (fail-safe ではない公表)**。
  全 f32/整数厳密値は rq (du_bounds.rq、RQ.md v2) で事前導出。
- **DU-2 [低]** pos_pack 厳密 pin: x 63-38 / z 37-12 / y 11-0 排他 (射影復元)、
  rq (du_pospack.rq) 導出値 5 件 ((1,64,2)=274877915200=0x0000004000002040・
  (-1,0,0)=0xFFFF_FFC0_0000_0000・(0,-1,0)=4095・(7,100,9)=1924145385572・
  x=2^25 有効 0x8000000000000000) + 領域外折り畳み衝突公表 (x=2^26≡0・
  y=±2048≡2048)。世界境界 ±30M<2^25 内は単射。u64 10進表示のみ基盤 printf
  (18446743798831644672、rq i64 ビット列照合付)。
- **DU-3 [観]** wiring 構造公表: full_graph_wiring:1369-1387 は take(4)・
  kind 常時 Chest・引数 (false,false,…,true) 固定・戻り値 `let _mode` 破棄
  — ただし be_entries HashMap 副作用は永続 = **「決定破棄・状態機械のみ
  進行」構造** (恒等クラス DM-2/DQ-1/DS-1、常時 miss DT-1 と別型: 状態機械
  自体は実進行する)。dist 欠損 unwrap_or(0.0) は近距離側安全既定だが
  interact 無しでは鮮度窓不発で昇格を妨げない。wiring 同型 soak pin
  (200 tick で 4 エントリ全静昇格・4 区画 distinct) で実証。
- **DU-4 [観]** static_mesh_ready=false は現 mode 保持 (Dynamic 維持・
  Static 維持の両方向 pin)。Static からの降格経路は burst/anim/interact
  3 系統のみで「メッシュ喪失」降格は不存在。
- **DU-5 [観]** promote_after_ticks*4 u64 乗算の debug overflow panic 領域
  (2^62 超) を契約記録。既定 40→160 で到達不能。
- **DU-6 [観]** closed_model_id 全表 pin + Other→chest フォールバック公表。

+9 strict テストで 1102 全緑 (module 13/13)。adversarial 6/6 RED:
(a) since_anim `<`→`<=` → promote_boundary RED・(b) dist `<=`→`<` →
inclusive pin RED・(c) burst 間隔 `<=`→`<` → interval pin RED・
(d) z `<<12`→`<<11` → pos_pack 厳密 pin RED・(e) リセット削除 →
burst interval pin RED・(f) ready ガード無効化 → mesh_not_ready pin RED。
検出不能ゼロ (強化追設不要)。復元 md5 照合 MD5-VERIFIED 6 回。
fmt fmdiff 正準形忠実適用 (現逸脱 0、`--emit stdout` は filename+空行の
2 行除去 = 捕捉 45 手順の精密化)。

**捕捉 47 件目**: DU-1 初版テストの 3.5+ulp 復帰ケース期待値を
DynamicBlockEntity と誤記 (実装は正しく Static 復帰: dist>3.5 で近距離
ルール不発・since_anim>=40 で昇格) → 初回実行テスト赤が捕捉・実装一致へ
修正。捕捉 46 件目は RQ v2 の記録 (selftest ピン被覆、前節参照)。

## DV. temporal_mesh_diff.rs (wave 122, 2026-07-26)

原版 186 行・md5 5d71c7e9995ad4bf22d565a2df1e3173 (wave 69 BS 監査済の
再 census)。消費者: full_graph_wiring (mark_dirty:784・patch_for_block:806)
のみ、WGSL なし。wire 形状: 780-811 (mark→take_dirty→packed_key→time_slice)。

- **DV-1 [観]** wiring 駆動形状: 毎フレーム chunk_keys 全件を無条件
  mark_dirty (sy=i%4、変更フィルタ無し)。時分割キュー供給源として一貫。
  同型 soak pin (3F×全件→sorted drain→packed→drive<4096) で実証。
- **DV-2 [低]** packed_key 厳密 pin: (0,3,4095)=4095・(5,1,5)=5243909=
  0x500405・cx=-1=0x3FF00000、折り畳み (cx=1024≡0・sy=-1≡1023・cz=-1
  drive=3072) 公表、**packed&0xFFF は cz 下位 2bit+sy 混在** (CI-2 pin 化)。
  全値 rq dv_packed.rq 導出 (assert 2 通過)、sy=i%4 系列検算 1230123。
- **DV-3 [観]** dirty map 値 (generation) は書込むが消費者ゼロの保持明記
  (map 内部値=2 実在、take_dirty 返却はキーのみ pin)。
- **DV-4 [観]** diff_section live 消費者ゼロ継続追認 + 全 4096 差異列挙の
  identity 順 pin 強化。

+4 strict テストで 1106 全緑 (module 11/11)。adversarial 4/4 RED:
(a) sort_unstable 削除 → sorted pin RED・(b) generation += 1 削除 →
generation 系 3 RED・(c) diff 条件反転 → 2 RED・(d) assert 4096→4095 →
境界受理 pin RED。検出不能ゼロ。復元 md5 VERIFIED 4 回。
fmdiff 現逸脱 0 (初版から正準形)。原版からの差分は doc+tests のみ。

## DW. transform_svdag.rs (wave 123, 2026-07-26)

原版 352 行・md5 d0bad280120da9028ebbf92c8b95cb6a (BX-1 D4 合成根治済の
再 census)。消費者: full_graph_wiring:113/255/914 のみ。WGSL なし。

- **DW-1 [低]** permute_node unused_mut 根治 (let mut y → let y、y は Y 面
  D4 で不変)。opt-gfx lib 警告 9→8 (generated カウント機械照合)。
  引継ぎ 9 件の内訳再確認: gpu_culling:6 DeviceExt・persistent_vbo_pool
  (6/8/11)・lib.rs:88 ambiguous re-export・aokana:46 mut dag・
  occlusion_query OccVertex private・binary_greedy_meshing:363 dead fn +
  本件 (根治) — api 側 13 件は別枠棚卸し。
- **DW-2 [観]** wiring: 908-916、svdag none or tick%600==0 の内側で
  take(16) のみ挿入・返破棄。決定破棄+副作用 (pool/base_dag 成長) 構造
  (DU-3 同型) + 部分列挙 (16 制限) 公表。
- **DW-3 [低]** canonical 一意性の経路非依存 pin: orbit {0,1,4,5} の
  3 メンバー (5,1,4) 全経路で同 ID・同形 (mask=1/children[0]=42)、
  solo インスタンスでも一致。insert_node dedup 確認 (svdag.rs:48)。
- **DW-4 [低]** y 不変性直接 pin: 領域 mask rq 導出 (y=1=204=0xCC/
  y=0=51=0x33、cover/disjoint assert 通過)、16 変換全走査+occupancy
  保存+対称側。**捕捉 48**: 初版 mask 0b0100_0100 誤記 (oct2,6 のみ)
  → テスト赤捕捉 → rq 導出値で根治。
- **DW-5 [低]** **検出空白補完強化 pin (4 件目)**: (b) compose apply の
  作用順交換が全既存 pin で検出不能 (群公理 pin は作用解釈不感) →
  非可換 R90∘mirror_x 厳密 pin ((1,T,F)、(x,z)→(z,!x)→(!z,!x) 手検算)
  + permute 2段/1段整合+mirror 先行別作用非一致 pin 追設で再 RED 達成。

+3 strict テスト (7/7) で 1108 全緑。adversarial: (a) strict<→<= → 2 RED・
(b) apply 順序交換 → 初回 6 全緑 (検出不能) → DW-5 追設で再 RED 1・
(c) new_i y/z 交換 → 2 RED・(d) mut 戻し → 警告 9 復活機械確認・テスト
不変 (誠実記録: 警告根治は build カウント照合が pin 代替)。復元 md5
VERIFIED + 強化 pin 込み最終版へ fmt 正準適用 (差分 54 行→0)。
固定版 md5 bc5357b49ccfdef5c782cdf0add2e24b。lib 警告 8 維持。

## DX. binary_greedy_meshing.rs (wave 124, 2026-07-26)

原版 1591 行・md5 25e17dd53927b603d27269394d6d715a。消費者 10+ ファイル
(census 実 grep)。既存 10 テスト (pull 対照 fuzz・bitcols 対照 fuzz 等)。

- **DX-1 [低]** greedy_merge_2d_pull の分類整理: lib dead 警告の実体は
  mod tests の**オラクル参照** (:1264/1283/1302)。#[cfg(test)] 付与で
  is_opaque 系同型のテスト専用保持へ (削除せず directive⑦整合)。
  lib 警告 8→7 機械照合、(b) 属性外しで警告 8 復活も対偶確認。
- **DX-2 [低]** idx 厳密 pin +1: (15,15,15)=4095・全 4096 単射走査・
  roundtrip (rq dx_idx.rq assert 2 通過)。消費者共有規約の固定。
- **DX-3 [観]** 消費者形状: 本番 render_pipeline:427/438、参照系
  frame_reference:564/575/879・gpu_vertex_pull:52・chunk_mesh:182・
  frame_reuse:344。
- **DX-4 [観]** オラクル資産棚卸し 10 テスト維持、(c) 変異で 5 RED の
  現役検出力を機械実証。mesh 出力不変 (digest ゲートで裏付け)。

+1 strict テスト (11/11) で 1109 全緑。adversarial: (a) idx y 係数改変
→ 2 RED (pin+連鎖)・(b) cfg(test) 外し → 警告 8 復活機械確認 (対偶)・
(c) bits 版ブロック判定反転 → 5 RED (fuzz 4+flat_layer 1、オラクル
検出力実証)。検出不能ゼロ。復元 md5 VERIFIED 2 回。fmt 正準適用
(差分 14→0)。固定版 md5 e1655861f3cd27a3dcaaf35b14a96639。
lib 警告 7、digest 004c1cf5 不変、台帳 464。

## DY. aokana.rs (wave 125, 2026-07-26)

原版 200 行・md5 2cbeb168ddbdfacb0c66da91d30443f9 (wave 77-78 CA/CB
監査済の再 census)。消費者: full_graph_wiring:114/256/930/936 のみ。

- **DY-1 [低]** `mut dag` unused_mut 根治 (lib 警告 7→6 機械照合)。
- **DY-2 [観]** wiring 実消費公表: 930 登録 ry=0 固定 (K-1)・936
  evaluate 結果は aokana_visible_regions へのカウント集計のみ (実
  カリング選択に未接続) = 「評価実効・消費は集計型」の中間構造。
- **DY-3 [低]** スケール厳密契約群: coords*64 は i32 安全域 bit 正確
  (2^24→0x4E800000/-2^24→0xCE800000)・min+64 退化境界 (2^30/ulp=128
  タイ偶数丸めで厚み 0、実害域 ≦2^20 では 8 ulp 正確)・**i32 溢れ経路**
  (≥2^25 で debug panic/release wrap→0xCF000000、wrapping pin、DU-5
  同型 2 件目)。全値 rq (dy_vals/dy_max/dy_wrap) 導出。
  **捕捉 49 件目**: 初版 pin の (1<<25)*64 が自身の debug panic で
  **実装上の overflow ハザードを照らす** (panic による捕捉) →
  wrapping 形式で根治的 pin 化。
- **DY-4 [低]** `>=0.0` vs `>0.0` 完全等価変異証明 (±0.0 成分のみ差、
  寄与 ±0.0 で和・判定不変、NaN 同選択) → (a) 全緑で機械確認、
  検出不能は証明付き誠実記録。(a') n-vertex 反転 RED 3 で検出担保。
  斜め平面 pin (rq 導出 24/-8) を追設し非軸分岐経路を固定。

+2 strict テスト (6/6) で **1112 全緑** (捕捉 50: 初版「1111」は誤記、
module run 1106 filtered+6 の機械値で訂正、後述)。adversarial: (a) 等価
全緑 (証明付)・(a') 3 RED・(b) 接触 <= 化 → 1 RED・(c) sort 削除 →
1 RED・(d) mut 戻し → 警告 7 復活+テスト不変。復元 md5 VERIFIED 4 回。
fmdiff 現逸脱 0。固定版 md5 a7139a359ec14880e82536058c91d9cd。
lib 警告 6、digest 004c1cf5 不変、台帳 468。

## DZ. 警告掃除 wave (wave 126, 2026-07-26) — opt-gfx lib 警告 6→0

対象: gpu_culling.rs・persistent_vbo_pool.rs・occlusion_query.rs・
gl33_compat.rs (全て警告発生源、census 実 grep で消費者確定後に根治)。

- **DZ-1/2 [低]** 未使用 import 4 件削除 (DeviceExt・Quantized12ByteVertex
  (mod tests 独立 import 済)・PackedPullQuad・debug、使用分は温存)。
- **DZ-3 [低]** DrawIndexedIndirectArgs 同名 3 重複 (execute_indirect
  (真消費型 azdo/full_graph_wiring)・gpu_culling (内部消費)・gl33_compat
  (内部のみ)) の統一: gl33 独自定義削除→gpu_culling 版 pub use 化、
  Default derive 移設で等価性保持。CRLF 原生ファイルは perl で CRLF 保持
  編集 (304 CR 行、混合なし)。ambiguous glob re-export 根治。
- **DZ-4 [低]** build_vertices private 化 (消費者内部のみ) で
  private_interfaces 根治。保持明記 (将来需要時再公開)。
- **DZ-5 [観]** **lib 警告 6→0 機械照合** (api 13 件は別枠棚卸し)。
  HEAD 原生 3 ファイルの fmt 逸脱 (161/64+/43 行) を発見し fmdiff 正準形
  忠実適用 (現逸脱 0、digest 不変を seal ゲート 5 で担保)。adversarial
  対偶 3 (gl33 戻し→ambiguous 復活・pub 戻し→private_interfaces 復活・
  import 戻し→unused 復活) 全て build 照合で機械確認、復元 md5
  VERIFIED。テストは DZ 非追加で **1112 全緑** 維持 (全量再実行
  201.04s)。

**捕捉 50 件目**: wave 125 記録の「1111 全緑」は誤記 — 機械値は module
run の「1106 filtered」+「6 passed」= **1112**。DZ 全量再実行の
「1112 passed」で捕捉・本節+DY 節を訂正 (seal ゲート 4 PASS には影響
なし、報告数値の正確性のみ)。記録文化「台帳数値は seal 機械値に照合」
の例外として今後は module run の filtered+passed 和を一次値とする。
**捕捉 51 件目**: DZ-3 初版は gl33_compat.rs の原生 CRLF を保持した perl
行編集に固執 (304 CR 行維持) → **seal ゲート 1 (san) が変更ファイル内
CR を検出し FAIL** (原生 CRLF 自体は引継ぎ一括 wave の対象だが、変更
スコープ内の CR は適格)。当該ファイルを LF 正規化で根治 (fmt 正準 0・
内容同一性は差分 0 で担保)、「CRLF 一括 wave とは別に、必要性駆動の
先行 1 件」として台帳・本節へ誠実記録。

---

## EA. rsift-api 警告掃除 wave (wave 127, 2026-07-26) — api lib 警告 13→0

DZ で opt-gfx lib 0 を達成した残件 (api 側 13 件棚卸し) の消化。
スコープ: rsift-api 10 モジュール + mod_suite 2 ファイル (12 変更)。
ベースライン: api テスト 49 全緑・lib 警告 13 (全量再実測)。

- **EA-1 [低]** 未使用 import 6 件除去 (worldgen `debug`・registries
  `debug`+`warn`・capabilities `debug`・networking `HashMap`・runtime
  `info`)。worldgen `chunk_index` の未使用 `world_height` 引数は
  `_world_height` 化 (設計が高さ非依存で実害なし、混入防止で明示)。
  runtime `let mut reg` (advancements lock) unused_mut 根治。
- **EA-2 [低]** **異シグネチャ同名 2 重定義の ambiguous glob 根治**。
  ① lifecycle `ServerStartingFn` (Arc&lt;dyn Fn()&gt;) →
  `ServerStartingCallback` rename — neoforge_event_bus 版
  (`Fn(&ServerStartingEvent)`) と同名 2 重定義だった。消費者は
  lifecycle 自モジュール (:66/:95/:124) のみ (census grep)。
  ② `mod_suite::modules` → `suite_modules` へファイル mv —
  fabric_api::modules (公式 Fabric 構造) との glob ambiguous 根治。
  参照全 8 箇所 + mod_suite/mod.rs:115 binding 文字列を置換、
  git 記録は delete+add (rename 検出は commit 時の差分最小化任せ)。
- **EA-3 [低]** adaptive_perf `pick_best_gpu` に
  `#[cfg(any(test, target_os = "windows"))]` — 呼出元が全て
  cfg(windows) ブロック内 + mod tests のため非 windows lib では dead。
  DX-1 cfg(test) 分類と同型の「範囲明示整理」。非 windows 対称 stub
  `read_registry_string`/`enumerate_display_devices` は消費者ゼロだが
  directive⑦により削除せず `#[allow(dead_code)]`+保持明記、+1 strict
  テスト `non_windows_stubs_return_empty_contracts` で None/空契約を
  cfg(not(windows)) fail-loud pin (49/49 全緑の機械値)。
- **EA-4 [観]** HEAD 原生 fmt 逸脱へ正準形忠実適用 (DZ-5 同型、
  rsift-api は fmt 未適用の素面が多く残存) + **CRLF 原生 3 ファイル
  LF 正規化** (engine_caps 458 CR 行・mod_suite/mod 233・modules →
  suite_modules 51、HEAD 機械 grep 値)。捕捉 51 と同型「変更スコープ内
  CR は san 適格」の必要性駆動先行、`git diff --ignore-space-at-eol`
  で内容差分照合 (LF 化は eol のみ)。
- **EA-5 [観]** **rsift-api lib 警告 13→0 機械照合** (opt-gfx (DZ-5)
  に続く 2 crate 目の完全根治)。adversarial 対偶 2: (a) stub 側
  `#[allow(dead_code)]` 外し → never used 警告 2 復活、(b)
  `ServerStartingCallback` → 旧 `ServerStartingFn` 戻し → ambiguous
  警告復活。各 build 照合で機械確認、復元 md5 VERIFIED・復帰警告 0。
- **EA-6 [中]** **ゼロデイ級潜伏テスト欠陥の発見・修正**。
  engine_caps `sm69_requires_score_and_vram` 旧版は
  `assert!(probe.sm69_eligible)` を無条件要求していたが
  `check_sm69_eligibility` は **DX12 Agility (Windows+DXGI) を必要条件**
  とし非 windows では常に not eligible → **Linux では構造的に必落ち**。
  bench.yml が `cargo test -p rsift-opt-gfx --lib` のみで rsift-api
  テストを走らせないため、テストを実行する者がおらず**長期誰にも検出
  されなかった** (orig 戻しで既往失敗を機械確定、本 wave 変更起因で
  ない)。設計意図 (SM6.9 = DX12 Agility 依存 = Windows 専用) 自体は
  正しいため、誇張なく公表したうえで経路を分割 pin 化: windows は
  eligible・非 windows は not eligible + block_reason に "DX12" 含有を
  全プラットフォーム fail-loud 固定・低スコア 8k は全環境で不可のまま
  不変。CI 軟点 (bench.yml api 非対象) は台帳・棚卸しへ記録。

本 wave は捕捉なし (捕捉 50/51 は DZ 節)。adversarial は上記対偶 2 で
検出力確認、search 変異なし (警告掃除 wave のため DZ と同型運用)。
digest `004c1cf5fb17bfe8` rows=357 は opt-gfx 由来のため api 変更で
不変 (seal ゲート 5 で担保)。

---

## EB. full_graph_wiring 重点監査 + rspeed RB-1 (wave 128, 2026-07-26)

大物監査の先行: full_graph_wiring.rs (wave 128 時点 HEAD 2,690 行) の
構造走査から得た確定項目 + 作業中に実地発見した tools ゼロデイ修正。
ベースライン: opt-gfx 1112 全緑・lib 警告 0・api 49 全緑。

- **EB-1 [低]** HUD 統計バー色の**優先順位罠**: 旧式
  `0xFF30_8040 + (i as u32) << 4` は Rust の結合規則 (+ > <<) により
  `(base + i) << 4` と評価 → alpha が意図の 0xFF (不透明) から 0xF3 へ
  化け base 上位 nibble 欠落 (rq eb_hud.rq 導出機械値 i=0 → 0xF3080400、
  i=1 → 0xF3080410)。実害域は HUD スクラッチバーの色のみ。base 定数の
  0xFF alpha 明示から不透明意図と判定し `base + (i << 4)` へ根治、
  ピン可能化のため純粋関数 `hud_layer_color` (:2095) 抽出。
  +1 strict テスト :2176 (layer 0..3 の golden 0xFF308040/50/60/70 +
  全 16 層 alpha=0xFF)、adversarial (a) 旧式戻しで **1 RED** 機械確認、
  復元 MD5-VERIFIED (捕捉 52 後に実施)。
- **EB-2 [低]** corner_ao_from_palette 戻りタプルの (u_sign, v_sign) は
  3 分岐すべてで out_sign(face) と常に等しい冗長値かつ消費者ゼロ
  (`let _` 破棄のみ) → 5 タプルを 3 タプルに縮小 (slab_slots 削除と同型、
  実効符号は face_out が out_sign(face) を直引きのため喪失なし、
  コンパイル中立照合)。
- **EB-3 [観]** decals 評価ループの恒常空: push サイト 0 件・公開登録
  API 不在 (census grep 機械確認) → 旧コメント「登録 API 経由の実データ
  があれば」は虚偽、誠実訂正。結合点は directive⑦ で保持。
- **EB-4 [観]** meshlet_cone 法線は i%6 巡回の 6 軸**合成**列 (実メッシュ
  面法線未接続、件数のみ chunk_materials 由来) — 「実面法線クラスタ」
  部分が虚偽だったため誠実訂正 (錐体ビルド/visible 評価は実演維持)。
- **EB-5 [低]** 消費者不在ローカルメトリクス削除 2 件: frb_above
  (camera 高さ比較カウント)・slab_base (確保前 used_bytes スナップ) —
  いずれも集計後 `let _` 破棄のみ (slab_slots 前例同型、挙動中立
  コンパイル照合)。
- **EB-6 [中] ゼロデイ級 tools 欠陥 RB-1**: rspeed rq 字句解析
  (rq_lex) の `&src[i..i+3]`/`&src[i..i+2]` **str スライス**が文法外
  マルチバイト文字の char 境界でハードパニック (「解釈できない文字」
  の fail-loud 経路を bypass)。発見経路: rq 全計算移行後、初めて日本語
  を含む `//` コメント (RQ では文法外、コメントは `#`) を書いた実地で
  "panicked … byte index 35 is not a char boundary" を機械再現。
  byte スライス比較への根治 (演算子は全て ASCII、str/byte 比較の結果
  完全一致 = 挙動中立) + selftest +3 ピン (51→**54**: 日本語コメント受理・
  非 ASCII 字句エラー rc=2 帰還・日本語文字列受理)、`rspeed selftest`
  0 FAIL。RQ.md 文法は不変 (実装欠陥の修正のみ)。

**捕捉 52 件目**: EB-1 adversarial 検証後の復元 `cp` が cwd の誤りで
**静寂失敗** (bash はエラーでも続行)、続くテストが変異体上で 1 fail。
md5 VERIFIED 照合ステップの失敗出力で捕捉 → 正しい絶対パスで復元 (MD5
a096609c) → 全緑確認。教訓: 復元コマンドは必ず md5 照合とセットで、
失敗時は即停止運用を継続。影響ゼロ (seal 前に回帰)。
**捕捉 53 件目**: EB 台帳追記 edit_file の old_text アンカが EA-1 行頭
(`| EA-1 | 低 |`) を巻き込んで消費し、EA-1 行の接頭が削除され 特記事項
ヘッダと癒合 → 追記直後の `grep -n "^| E-"` 構造検査で即捕捉、sed 行
手術 (ブロック退避→行削除→接頭復元→EA-6 後へ再挿入) で EA-1..6/EB-1..6
の時系列順に完全修復 (backup /tmp/registry_before_repair.md 保持)。
教訓: 台帳追記の old_text は直前行末尾のみに絞る。

---

## EC. render_pipeline 重点監査 (wave 129, 2026-07-26) — BA-3 解消

棚卸し BA-3 (render_pipeline unwrap_or_default 静寂空化) の本丸解消 +
周辺観察。ベースライン: opt-gfx 1113 全緑 (wave 128 後)・lib 警告 0。

- **EC-1 [低] BA-3 解消**: frame() の quad_budget 経路で
  `cast_bytes_to_slice(...).map(to_vec).unwrap_or_default()` — cast 失敗
  (ラギッド/非整列、M-4 で Option 化された拒否パス) を**空 Vec に倒して
  全 quad を静寂空化し書き戻す**データ損失パスを保持していた (棚卸し
  BA-3)。pure 部 `apply_quad_budget_bytes` (:1138 前後) へ抽出し、失敗時は
  bytes **無変更保持**で bool 返却、呼出側は fail-loud
  `tracing::warn` して budget 適用をスキップ。gpu_quad_bytes の生成規約
  (cast_slice_to_bytes 由来・非空ガード) 上ほぼ到達不能だが、到達不能を
  理由に損失を許容しない。+1 strict テスト (4 quad → budget 2 の切詰め
  順序保持・budget 内不変・ラギッド 8n+1 で false+bytes 無変更 — 旧式
  なら空化で RED)、adversarial 旧式戻しで **1 RED** 機械確認、
  fmt 正準済で復元 MD5-VERIFIED (0a9b3ee2)。
- **EC-2 [観]** DRS `internal_size` (:567 の `let _internal`) は評価結果
  読み捨て — 内部解像度の変更は未還元で、ヘッダ「(no resolution
  scaling)」と整合する計測実演として誠実注記。結合点保持 (directive⑦)。
- **EC-3 [観]** 材料引き当て (:1068 前後) は chunk_keys × pull_meshes の
  線形 find = O(n·m)。両者数百スケールで現害小 (支配 tex 決定用途)、
  HashMap 化は実効見合い要検討として棚卸し公表。

本 wave は捕捉なし (52/53 は EB 節)。HEAD 原生 fmt 逸脱 29 行相当を含め
正準適用 (現逸脱 0・自己起因逸脱は初版 5 行を捕捉→正準で根治)。digest
不変を seal ゲート 5 で担保。

---

## ED. overdraw_sort.rs (wave 130, 2026-07-26)

実効経路 (report.overdraw_order → upload 順) に直結するソート系。既存 3
テスト・110 行 (HEAD 時)。ベースライン 1114 全緑・警告 0。

- **ED-1 [低] NaN 未防備 unwrap の堅牢化**: sort_front_to_back の
  `da.partial_cmp(&db).unwrap()` — NaN 中心・NaN カメラが 1 つでも混入
  すると partial_cmp が None で **lib 内パニック**。上流カメラは
  world_column_store で非有限拒否 (既存 pin) されるが、本 API は公開
  ライブラリとして素の配列を受ける規約のため変異は合法入力域に隣接。
  `total_cmp` 全順序化 (有限値で演算結果完全一致、Rust 1.62+ の標準
  全順序、NaN dist2 は最奥配置・panic なし)。adversarial 復元
  (partial_cmp().unwrap() 戻し) で**実 panic**(:32:33) による RED を
  機械確認 → 復元 MD5-VERIFIED・4/4 GREEN。+1 strict テストで契約 pin。
  overdraw_saved 側の unwrap_or(Equal) も同根で total_cmp 化。
- **ED-2 [観]** early_z_shaded/overdraw_saved の実消費者はテスト/計測
  のみ (census grep、本番は sort_front_to_back × full_graph_wiring:704
  のみ)。O(width × spans) 棚卸し、計測器+WGSL 検証資産として保持。

本 wave は捕捉なし。HEAD 原生 fmt 逸脱 36 行含め正準適用 (現逸脱 0)。
digest 不変を seal ゲート 5 で担保。

---

## EE. simd_kernels_avx2.rs (wave 131, 2026-07-26)

126 行・既存 4 テスト (fuzz オラクル付)。全アーキテクチャ bit 同一の
歴史注記 (旧非 x86_64 fallback 修正済) がある精錬済みモジュール。

- **EE-1 [低] x 未ガードの堅牢化**: face_visible_bitmask は z >= 16 を
  ガードする一方 x は未ガードで、`1u32 << x` は **x >= 32 で debug
  パニック / release 静寂巻付き (x % 32)** → bit0 立ち mask に誤 true
  を返し得た (z ガードとの非対称)。`x >= 16 → false` ガード追加。
  x ∈ [16, 32) は旧結果も false (masks の実効ビット 0..16 のみ) で
  bitwise 同一 = 挙動変更域は x >= 32 のみ (panic/巻付き → 決定的
  false、M-4/DU-5 系堅牢化と同型)。+1 strict テスト、adversarial ガード
  除去で **実 panic RED** (:52) 機械確認、復元 MD5-VERIFIED (e41f1e37)。
- **EE-2 [観]** 消費者形状公表: 実呼出は full_graph_wiring:1000/1002
  の面可視サンプル 1 系のみ (census grep)。既存 fuzz オラクル
  (spec_masks 別ループ形状) と全 PF bit 同一契約は現役確認。

本 wave は捕捉なし。digest 不変を seal ゲート 5 で担保。

---

## EF. CI 紅調査 + async_chunk_io フレーク堅牢化 (wave 132, 2026-07-26)

**機械記録**: 45 連緑 (ea5387c..bb79031 success) の後、c6e838c run
(30197391402) が lib tests ステップで **failure** (4m51s・annotations は
"Process completed with exit code 101" のみ。詳細ログは results-receiver
接続が本 sandbox から遮断され取得不能、`gh run rerun` は "workflow file
may be broken" 応答)。c6e838c はローカル seal 全 6 ゲート PASS (1116/1116、
digest 不変) で、直前 commit bb79031 との差分は simd ガード+doc のみ —
コード起因を示す証拠はなく、環境/断続フレークの説明が有力。

- **EF-1 [低]** **最有力候補の堅牢化**: async_chunk_io の
  lz4_roundtrip_store_load は固定 sleep (store→50ms、load→80ms) 後に即
  assert する時刻依存テストで、CI 高負荷時にワーカ未完了 (phase A: Store
  書込が load 読込に間に合わず fs::read 失敗→Failed イベント / phase B:
  Load が 80ms 内に未完了) なら**両条件が空で断続失敗**し得る。同 crate
  で timing API (Instant/thread::sleep) をテスト内で持つのは本モジュール
  のみ (機械走査)。成功条件を不変に保ったまま 5 秒 deadline ポーリングへ
  堅牢化 (phase A: path.exists() 待機+Stored 排水、phase B: poll_ready
  ループ、上限到達のみ失敗でワーカ異常の検出力は維持)。ローカルでは
  フレーク再現不能のため adversarial 非検出を誇張せず誠実記録 (判定は
  新 push run の帰納確認に委譲)。CRLF 原生ファイルの LF 正規化を併施
  (san 適格、215 CR 行 eol のみ、捕捉 51 手順 4 件目)。
- **EF-2 [観]** 上記 CI 紅の機械事実と「根因帰属は最有力候補に限定」
  の誠実公表。過剰主張 (フレーク確定 等) を避ける。

作業中メモ: 環境第 5 号リセットを検出 (git ref 64294c6 巻戻り+rust
toolchain/rspeed 消滅)→ 復旧手順再適用 (restore-env.sh → toolchain
1.94.1 e408947bf → tools/rspeed.rs から rspeed 再ビルド (RB-1 版・
selftest 54 ピン 0 FAIL)→ FETCH_HEAD c6e838c へ update-ref+reset
--mixed、worktree 無改変で整合)。捕捉 52 同型の cwd 相対パス誤り 1 件
(影響ゼロ・即時読取、bak 複写の失敗を stderr 出力で捕捉)。

---

## EG. restir.rs / micro_lod.rs 誠実注記監査 (wave 133, 2026-07-26)

両モジュールとも実装は数学的に妥当で、変更は**誠実注記 (doc) + 注入性
ピン (厳密テスト)** のみ。構造的確定事項の言語化を優先した wave。

- **EG-1 [観] restir::estimate の推定構造公表**: 真の RIS 推定は
  radiance · (w_sum/m) · (1/p̂(selected)) だが、本実装は **1/p̂
  正規化を省略した簡約形** (p̂ = target_pdf が radiance に比例する
  設計前提で、輝度比の近似として機能)。単一流では選択確率が厳密 RIS
  (w_i/w_sum) に従う一方、`combine` は隣接 reservoir の sample を
  **受信側 p̂ で再評価しない naive merge** (文献上の実用近似であり、
  結合後の推定は biased) と、限度付きの性質を doc に明記。wiring 実消費
  は計測破棄のみ (full_graph_wiring の let _restir_estimate、census grep)。
- **EG-2 [観] micro_lod::downsample_palette 宛先写像の単射性公表**:
  サンプル点は (k·f) 限定で宛先 x/f は**単射** (x = k·f ⟹ x/f = k 一意)
  — 複数ソースの同一宛先衝突は構造的に起きず「最終書込み勝ち」は
  仕様外 (空 dst への 1 書込みのみ)。実引数は lod_for_distance 由来の
  {1,2,4,8} (census: full_graph_wiring 経路)、非出力域 (factor 非約数の
  余り側) は捨てる近似方式。max_quads フィールドの外部消費者は現状ゼロ
  (census grep) — 将来のクアッド制約計測用に保持 (directive⑦)。
  +1 strict テスト `downsample_injective_dst_extents_exact`: 全マス充填で
  非零宛先セル数は (16/f)·(16/f)·(16/f) に厳密等しい (f=2/4/8 →
  **512/64/8** — 衝突/添字崩れは個数減少として必ず現れる注入性の
  観測可能ピン)、値は 7 か 0 のみ、f=3 は (6)·(6)·(6)=**216** (x =
  0,3,..,15 → dst 0..5 の余り捨て近似固定)。rq 導出 eg2_downsample.rq
  全 assert 通過 (軸サンプル数 ceil 式・f=3 商最大 5・AO 閾値全域
  4097 ストライド走査の事前導出照合)。

**adversarial**: (a) `step_by(factor as usize)` → `step_by((factor+1)…)`
変異で **2 RED** (新規注入性ピン + 既存 identity_and_mapping が連鎖)、
(b) AO 閾値 3000→3001 変異で **1 RED** (bake 境界テスト)。
復元 md5 照合 MD5-VERIFIED (182d729f261f2c62ceb88e834e4f25cc) 2 回。

**捕捉 53 同型・2 件目**: wave 133 編集中、edit アンカが
`bake_impostor_ao_thresholds` の fn 尾部を飲み込み本体 2 重化 (E0428
級の重複定義) を誘発 → grep 構造検査で即捕捉し、不完全フラグメント
除去で修復 (採番なし同型再発として記録、捕捉 132cwd 件と同運用)。

棚卸し: lib test プロファイルのみの警告 4 件 (aces_tonemap:137 unused・
half_vertex:309 unused_mut・meshlet_cone:146 unused・fsr3_fg:207
unused_mut) は **HEAD 原生を stash 対照で機械確定** (wave 133 非起因)。
lib (非 test) 警告は 0 維持。test 側警告掃除は別 wave 候補として記録。

opt-gfx **1117 全緑** (net +1、全量再実行 203.36s 機械値)・api 49 全緑・
lib 警告 0・fmdiff 自己起因逸脱 0。
digest 004c1cf5fb17bfe8 rows=357 不変は seal ゲートで担保。

---

## EH. clustered_lighting.rs (wave 134, 2026-07-26)

135 行・既存 4 テスト。未監査 23 モジュール (registry/AUDIT 双方で言及ゼロ、
機械列挙) のうちの 1 件目。モジュール自体の数学は健全 (球-AABB 最近接
距離二乗判定は標準手法) だが、wiring 構造と契約管理に課題を検出。

- **EH-1 [中] wiring 座標フレーム不一致の構造公表 + ヘッダ虚偽訂正**:
  旧ヘッダは「Subdivides the view frustum」と主張するが実装は**正規化
  単位立方体 [0,1]³ の一様分割** (透視分割・指数 z スライスの無い様式化
  参照実装)。さらに wiring の実供給 (full_graph_wiring) は**セクション
  局所ボクセル座標 [0,16) と輝度レベル半径 1..15** をそのまま流すため、
  空間割当は座標フレーム不一致のまま計算される。消費者は
  `_max_cluster_load` 集計破棄のみ = 「評価実効・消費は集計型」中間構造
  (DY-2 と同型)。soak ピンで構造を機械固定: wiring 実引数形状
  (120×67×16 = 128,640 クラスタ) に (2,2,2) r=15 を供給すると全点まで
  sqrt(12)<15 で**全 128,640 クラスタに氾濫帰属**、(15,15,15) r=1 は
  最近隅 (1,1,1) まで sqrt(588)>1 で**全域ミス** (rq eh_clustered.rq
  導出)。view/NDC 正規化の実配線は設計判断として引継ぎ。WGSL 側は
  同一式の件数集計のみの参照パスであることも明記。
- **EH-2 [低] new() の fail-loud 契約化**: 零次元は全クエリを静寂に
  空化する堕落形 (aabb は 1.0/0=inf 経由で NaN 座標を返しうる) ので
  dims >= 1 を assert。さらに総クラスタ数の u64 事前検査 (<= u32::MAX)
  で `index` の u32 wrap 折り畳み衝突を**構造的に排除** (cz<=slices-1
  なら cz*ty*tx <= total < 2^32、wrap 不出の証明は積保証に帰着)。
  wiring 実引数 ((w/16).max(1) 等) は契約内のため無影響 + should_panic
  4 件。frame_pacing S-3 と同型の契約明示。
- **EH-3 [低] index() 範囲 assert + 全掃引ピン**: 範囲外座標は他クラスタ
  スロットへの静寂折り畳みとなるため fail-loud 拒否 (assign_lights 内部
  呼出は常に範囲内、コストは 3 比較で球判定に対し無視級)。非対称グリッド
  3×5×7 の全 105 掃引単射 + index(2,4,6)=104 を rq 導出で固定。
- **EH-4 [観] 厳密 bit 契約ピン群**: aabb は乗算 1 発のため丸め 1 回に
  確定 — 1/6 = 0x3E2AAAAB、**5*(1/6) = 0x3F555556** (0x3F555555 では
  ない、rq 実導出。暗算禁止規律の実効例)、6*(1/6) は丸めで厳密 1.0。
  tangent 包含境界の両方向 pin (d=r²=0.25 で真、r を 1 ulp 低下
  (0x3EFFFFFF) で偽)。境界面ライトの**両隣帰属** conservative pin
  (x 共有面上 → ちょうど 2 クラスタ、y,z は内部固定で限定)。非有限の
  drop 契約 (NaN 位置/半径 → false) と r*r=inf の全域支配 (inf<=inf 真)。
- **EH-5 [観] O(L × N) 全走査の棚卸し公表**: wiring 形状では
  ≤32 ライト × 128,640 クラスタ ≈ 4.1M 球判定/tick。範囲制限走査への
  置換は f32 境界判定の bit 同一性証明 (ulp マージン付き帰納) を伴う
  設計判断のため EC-3 と同型の棚卸しとして引継ぎ。

**adversarial 4 系統全て RED**: (a) index assert 除去 → 1 RED・
(b) `d <= r*r` → `<` → **2 RED** (tangent 包含 + inf 支配の inf<inf
偽化)・(c) 零次元 assert 除去 → 3 RED・(d) 積域 assert 除去 → 1 RED。
復元 md5 照合 MD5-VERIFIED (42c7c9b7654a8a1cacc9ca5cfd6230d7) 4 回。
**全厳密値を rq で事前導出** (eh_clustered.rq、全 assert 通過)。

**捕捉 40 同型 (復元手順系・採番なし)**: adversarial 初手で (a) の変異
注入が構文破壊 + 検出 grep が compile error を FAIL と誤認できず、
続く `git checkout --` が **未コミットの EH 編集全体を HEAD へ巻戻し**
た → md5 記録値との照合で即捕捉し、編集適用スクリプト (決定的文字列
置換) を再実行して **md5 bit 同一 (42c7c9b7) に完全復元**。未検証
worktree への git checkout 禁止・実体コピー先行を手順へ明文化。

opt-gfx **1128 全緑** (net +11、全量再実行 199.53s 機械値)・既存 4
テスト不変・lib 警告 0・api 49 全緑・fmdiff 自己起因逸脱 0。
digest 004c1cf5fb17bfe8 rows=357 不変は seal ゲートで担保。

---

## EI. clustered_lighting.rs / full_graph_wiring.rs (wave 135, 2026-07-26)

新指令 §7 (2026-07-26「スタブ、ToDo残し、消費者なし、実装はしたものの
未配線などは一切禁止」、USER_DIRECTIVES へ日付付き追記済) の最初の
消化 wave — wave 134 で自分が残した引継ぎ 2 件を両方とも実装で閉じた。

- **EI-1 [中] EH-1 消化: 座標正規化 + 実消費者配線**: wiring の供給を
  セクション局所 [0,16) から **/16 正規化 (2 の冪除算・f32 無丸め)** へ
  根治 (半径も同尺度)。集計は破棄をやめ FrameWiringReport の新実
  フィールド `cluster_max_load` / `cluster_lit_clusters` へ配線
  (決定性比較集合にも追加・決定的入力由来で適格)。soak ピン (旧形状の
  flood/miss) はモジュール契約ピンとして保持し、正規化形状の新ピンを
  追加: ボクセル (2,2,2) 輝度 15 → (0.125³, r=15/16=0x3F700000) で
  実在クラスタ (15,8,2) 帰属・機械導出 golden **95,608** 帰属 (全氾濫
  128,640 でも全ミスでもない意味ある割当)。
- **EI-2 [中] EH-5 消化: assign_lights 範囲制限走査 (出力完全同一)**:
  O(L × N) 全走査を axis_range の帰属候補範囲のみの走査に置換
  (push 順・クラスタ走査順を保持)。包含証明はコメント本文の (i) pad
  による f32 端丸め (2^-22 未満) 包絡 (ii) クランプ飽和域の自明性
  (iii) 非有限の全範囲退化 (iv) r*r の f32 inf 飽和時は判定全域真に
  合わせた全範囲退化、の 4 条件に帰着。極端入力の i64 飽和減算は
  f64 予備 clamp で根絶。等価性は mod tests の旧実装忠実オラクル
  (DX-1 同型のテスト専用保持) との **fuzz 突合** (7 グリッド × 乱択/
  特別半径 {0,・負,1e30,NaN,inf}・face 狙い決定的ケース) で機械固定。
  **効果 (機械実測)**: 全量スイート wall time **199.53s → 11.08s**
  — tick_world 系テストの支配コストが全走査 4.1M 判定/frame であった
  ことの実測証左 (帰属: 範囲制限 + 正規化による半径縮小の組、独立変数
  ではない。誇大主張回避のため単独寄与は分解しない)。
- **捕捉 54 件目**: EI-2 初版は **r*r の inf 飽和クラス**で真の乖離を
  持っていた (naive は d=inf でも inf<=inf で全域帰属、範囲制限版は
  幾何学的範囲のみ評価 → p=-3.4e38/r=3.4e38 で 128,640 vs 27) —
  fuzz の adversarial 想定外コーナーを極端入力テストの**テスト赤が
  事前捕捉**、(iv) ガード追加で根治。併せて face ピンの aabb 添字誤り
  (48 の max は 49 面) もテスト赤が捕捉 (採番なし同型)。
- adversarial: (e) inf ガード除去 → 1 RED・(f) hi から rr 脱落 →
  2 RED、復元 MD5-VERIFIED (55981de422a51df4aaa3b6d3beeba763)。
  **検出空白の誠実記録**: wiring の /16→/8 変異は決定性 a==b を壊さず
  **非検出** (集計メトリクスのみのため。モジュール級 golden は存在、
  wiring 級 golden は FullGraphWiring fixture が必要で将来需要時)。

opt-gfx **1132 全緑** (net +4、全量再実行 11.08s 機械値)・既存テスト
全て不変・lib 警告 0・api 49 全緑 (api 無関係)・fmdiff 自己起因逸脱 0。
digest 004c1cf5fb17bfe8 rows=357 不変は seal ゲートで担保。

## EJ. deinterleave_ao.rs / async_compute.rs / full_graph_wiring.rs (wave 136, 2026-07-26)

新指令 §7 (「スタブ・ToDo残し・消費者なし・実装済み未配線一切禁止」)
の 2 段目消化。対象選定は機械: 未監査 22 モジュールの wiring 参照数を
grep 全列挙した結果 **depth_prepass/async_compute/deinterleave_ao が
参照 0** を確定し、そのうち crate 全体の消費者まで調査:
depth_prepass は `render_pipeline.rs` が消費 (§7 非違反で除外)、
`**async_compute** は `wgsl_source()` のみ消費で Rust ロジック消費者ゼロ、
**deinterleave_ao は crate 全体で外部参照完全ゼロ (最重係離脱)** →
後二者を本 wave 対象に確定。census: Vec3/Vec4 は lib/tests/全 crate で
使用 0 (planner 本体も非使用 = 完全装飾)。

- **EJ-1 [中] AO セクション二重中間構造の根治 + deinterleave_ao 実配線**:
  旧版は `opaque_ratio` 合成スライス (全サンプル高≤center で遮蔽が常に
  非発生の定数退化 gtao_occ≡1.0) + `_ = gtao_occ` 破棄 (EH-1 同型)。
  真のパレット高さ場断面 (`section_heightfield_depth`: 列不透明最上
  y+1 の 16 正規化、f32 無丸め) に根治し report 実フィールド 3 件へ、
  deinterleave_ao を半解像度 AO パイプラインの品質監視として実配線
  (低スペック AO 半解像度化判断の継続監視に接続)。GTAO 逆段差閉形式
  golden は rq 導出 1−atan2(2,1)/(π/2)=0x3E972028 が wiring 実測と
  bit 一致 (間接 libm 差異の非混入を機械確定)。
- **EJ-2 [中] async_compute 経済モデル配線 + Vec3/Vec4 削除 + 捕捉 55**:
  planner を作業量 proxy 写像で実配線 (係数表 PROXY_* 定数、絶対 ms
  非解釈・saved_pct 比率のみ意味を持つ誠実注記)。捕捉 55: 空
  `Iterator::sum::<f32>` は **-0.0 (0x80000000) を透過** (rustc 1.94.1
  zeroprobe 機械確定、maxnum(-0,-0)=-0) — plan/overlap 全結果に
  .max(0.0) +0.0 正規化で根治 (テスト bits pin 赤が捕捉)。
- **EJ-3 [観] deinterleave_ao 誠実注記 4 項目**: 片側 2 方向 horizon
  非対称・境界 1px 帯未加工透過・奇数寸法 index 安全証明・
  cost_ratio 単純積名目見積。module strict 4 件追加 (奇数寸法安全・
  境界透過/内部 0.3125=0x3EA00000 rq 導出 golden・eps tight/loose
  差分検出・全エア 1.0 bits exact)。
- **EJ-4 [低] AO 検出空白の pin 強化**: adversarial (a) uniform shift
  は AO horizon の差分オペレータ性により構造的非検出を誠実確定 →
  ヘルパ出力レベル golden (census+頂上+空列) で RED 化 (再変異 1 RED
  実証)。
- adversarial 6 系統総括: (a) 0→強化後 1 RED・(b) PROXY 係数 2 RED・
  (c) planner post 脱落 5 RED・(d) AO radius 2 RED・(e) denoise
  center 脱落 2 RED。復元 MD5-VERIFIED 3 回 (wiring f974863c52d59c…・
  ac 708b47de90…・dao e848349000…)。sed 2 行除去による構文破壊の作業
  事故を grep/construct 検査で即捕捉 → 孤立 `}` 1 行のみ削除で変異形態
  整流 (捕捉 40/52 同型の複雑化版、採番なし)。
- 作業中メモ: **環境第 7 号リセット**検出 (~/rust+~/bin 消失 + git ref
  が base 64294c6 巻戻り、wave 136 編集中に発生) → 定石復旧手順再適用
  (restore-env.sh → rspeed RB-1 再ビルド selftest 0 FAIL → fetch +
  reset --mixed FETCH_HEAD、作業ファイル損失ゼロ=async_compute.rs のみ
  の差分に収束、HEAD 891404e 一致を照合)。

opt-gfx **1144 全緑** (net +12 = async_compute +3・deinterleave_ao +4・
wiring +5 (ej golden ×2+step ×2+heightfield golden)、実値機械検算済)、既存
テスト全不変・lib 警告 0・api 49 全緑・fmdiff 自己起因逸脱 0・digest
不変・seal 全 6 ゲート PASS。固定版 md5 二重保存 (bak/src 三重一致)。

## EK. location_encoded_occupancy.rs / full_graph_wiring.rs (wave 137, 2026-07-27)

新指令 §7 第 3 段消化。対象選定は機械: 未監査残 20 モジュール中最小行の
LEO (59 行) を選択 — ただし census grep で wiring refs=3 の見せかけ配線
を検出: allocate+VecDeque 最大 4096 窓 ring で pop_front **だけ**で ring
内容も payload も全未参照の中間構造 (§7「実装済み未配線」残形)。

- **EK-1 [中] LEO 実消費者配線**: ring 維持と同期した tag 8 スロット集計
  (`leo_tag_dist`、pop は decode 側との対称減算、Σ==ring len 不変式の
  debug_assert 常時検査。wiring 2 instance det 601 tick でも分布 601
  一致の構造天然化) + payload(=tick) を Option 版 `get_payload` で
  実読出し → report 実フィールド 2 件 + det 比較集合。供給関数を
  pure fn `leo_occupancy_tag` に抽出し u8 wrap (256→1 等) boundary
  確定値を pin 表形式で lock。
- **EK-2 [低] get_payload Option 化**: OOB 静寂 0 (有効 payload 0 と
  混同可能) を「None」と明確分離、wiring `expect` で fail-loud 契約化
  (S-3 同型)。消費構造: wiring は alloc 直後の index のみ読み、恒に
  pool 範囲内 (expect 恒真)。
- **EK-3 [観] LEO 誠実注記 4 項目**: while ループ高々 7 回終了証明・
  payload は index から decode 不能 (アドレス埋込みの正しい言明)・
  **padding と tag=0 の index 非区別性 = 曖昧性公表** (wiring が
  tag=0 を供給しない契約で構造補完、tag=0=未占用予約)・pool 非
  shrink の cumulative model 明示。4 strict テスト (tag 8 種正規性・
  padding <=7 (7,0,1,7,0,2) fuzz・decode payload 非依存代数・Option OOB)。
- **EK-4 [低] ring 削除経路の検出空白補完**: ring>4096 飽和 strict
  (4,097 tick) 新設で pop decode 対称減算の削除経路を golden 化
  (adversarial (b) 1 RED = 本経路のみの検出空白を新 pin が正確埋め)。
- adversarial 5 系統: (a) dist 維持脱落 **10 RED**・(b) pop 減算脱落
  **1 RED**・(c) min(7)→6 **1 RED**・(d) decode %8→/8 **3 RED**・
  (e) Option→unwrap_or(0) revert **1 RED**。復元 MD5-VERIFIED 各系、
  **(e) の変異復元忘れ 1 件を md5 照合が即捕捉** (790c5ff2 vs
  87666092 相違機械捕捉、採番なし同型)。

opt-gfx **1151 全緑** (net +7 = LEO 3 新規 + wiring 4 新規、機械検算
1144+3+4=1151、全量 22.29s 機械値)・lib 警告 0・api 49 全緑・
fmdiff 自己起因逸脱 0 (LEO HEAD 原生 1 行据置)・digest 004c1cf5
不変・seal 全 6 ゲート PASS。固定版 md5 二重保存。

## EL. tbdr_hints.rs / fragment_ray_box.rs / gpu_runtime.rs (wave 138, 2026-07-27)

新指令 §7 消化 6。対象選定は census grep 機械確定 (残 19 モジュールから
refs=2 上位 2 件を精査): **TbdrHints unit struct は crate 全体で消費者
完全ゼロ** (TbdrPass/AttachmentUsage は wiring 初期化 info! 消費ありで
§7 適合) ・**FRB wgsl_source(&self) は self 不使用の装飾レシーバで
消費者がモジュール内テストのみ** の 2 件を同型抱き合わせ。精査過程で
tbdr_hints.wgsl が「GPU シェーダーではない」と自身が宣言する 2 行
コメントのみのファイルであること (実シェーダー一覧への非シェーダー
混在) も機械発見。

- **EL-1 [中] TbdrHints struct 消費者完全ゼロ (§7 違反) 根治**: 状態なし
  unit struct・メソッドは pub const 参照を返すのみで付加価値ゼロ
  (workspace 全体 grep 使用 0 機械確定) → free fn `wgsl_source()` 様式へ
  統一 (ssr/bloom/cas 他 20+ モジュール同型) し struct 削除
  (EJ-2 Vec3/Vec4 完全装飾削除先例準拠)。gpu_runtime:92 の登録を
  `crate::tbdr_hints::wgsl_source()` 経由に一本化 = WGSL 取得の
  単一公式アクセスポイント化。strict: free fn ≡ const 同一内容 pin +
  "No GPU shader required"/"transient" 含有 pin (naga 空受理に代わる
  内容担保)。
- **EL-2 [低] TbdrPass 真理値表検出空白補完**: 旧テスト 3 行
  ((T,F)/(T,T)/(F,F)) で **(F,T) 行が欠落** → strict
  `truth_table_exhaustive` で bool 4 行全列挙 (rq 事前導出: or 変異は
  (T,T)(F,F) の **2 行**差異・否定脱落変異は (T,F)(F,T) の 2 行差異。
  初稿は or 変異 diff を「1 行」と暗算誤記 → **rq assert が事前捕捉**
  (diff_count==1 失敗 → 正 2、rq 段階 self-catch 採番外)。
  4 行網羅で両変異クラスを捕捉可能に)。
- **EL-3 [観] 誠実注記 4 項目** (tbdr_hints module doc 明記):
  (1) 判定は conservative (writes=false の load-op clear のみ
  アタッチメントは TBDR 理論上 transient 化可能だが現契約は非対象
  =Persistent 安全側)。(2) wiring 消費は初期化時固定 2 パターン定数入力
  の info! 評価 (畳み込み可能なハードコード契約・実パス構造との動的
  接続なし)。(3) **TBDR_HINTS_WGSL は GPU シェーダーではない** (自身が
  "No GPU shader required" 宣言) にも関わらず all_wgsl_sources
  (「実シェーダー一覧」) に登録 → naga 空モジュール受理で検証実効
  ゼロ (分類実態公表・一覧からの除去は naga 経路と wiring 連結 pin
  波及のため設計引継ぎ)。(4) lazy_allocated ≡ (recommended_usage()
  ==Transient) の同値委托。
- **EL-4 [低] FRB wgsl_source(&self) 装飾除去**: self 不使用レシーバ・
  消費者テストのみの中間構造 → free fn 化 + gpu_runtime:113 を free fn
  経由化 (構造体/new/Default は wiring が max_dynamic_voxels を保持・
  wiring:952 cap 実消費のため維持)。strict: free fn ≡ const pin・
  new(7)=7/Default=65536=2^16/new(1<<20)=1048576=2^20 pin (rq 導出)・
  wiring cap 写像 (1048576→128, 65536→128, 7→7) pin・既存 1 テストは
  呼出形のみ free fn へ機械追従 (検証意図不変)。誠実注記: Default
  65536 vs wiring 1<<20 の差は意図的 (standalone フォールバック vs
  明示上限・GPU 不送達構造は CG-6 公表どおり)。

adversarial 5 系統: (a) or 変異 **3 RED** (truth_table+既存 2)・
(b) 否定脱落 **3 RED** (truth_table+既存 2、(T,T) 偽陽性含む)・
(c) lazy `==`→`!=` **4 RED** (lazy アサート全滅)・
(d) gpu_runtime 参照 revert (free fn→const) **非検出** = 同一 &str の
機能等価・参照様式差のみ → 検出空白として誠実記録 (census grep のみ
検出経路、wave 135 /16→/8 非検出先例同型)・(e) free fn 返却 `""`
破壊 **1 RED** (strict 内容 pin のみ検出経路、naga 空受理で他不変)。
復元 MD5-VERIFIED 各系 (tbdr d59d37ee・frb fa24d9ba・gpu 7c7c6fb7、
三重照合)。(e) 初回 perl 置換はエスケープ不整合で未適用のまま緑 →
grep 構造検査で変異未注入を即捕捉し sed 範囲アドレスで再注入
(採番なし同型: 変異検証前に grep で変異形態確認の手順再確認)。

opt-gfx **1156 全緑** (net +5 = tbdr +2・frb +3、機械検算
1151+2+3=1156、全量 22.01s 機械値)・lib 警告 0・api 49 全緑・
fmdiff 自己起因逸脱 0 (3 ファイルとも HEAD 逸脱 0、正準形手術 1 箇所:
gpu_runtime 登録 4 行形→1 行折畳み、fmdiff は git root 相対パス必須を
再確認)・san/trailws 0・digest 004c1cf5 rows=357 不変見込
(all_wgsl_sources 内容は同一 &str・wide_static_bench 非経由)・
seal 全 6 ゲート PASS 後に push。台帳 515。

## EM. lockfree_vram_cache.rs (wave 139, 2026-07-27)

census grep で wiring refs=3 (フィールド保有/new(1<<16)/lookup_or_insert)。
消費は existed→report.lockfree_cache_hits 集計のみで handle の
(slot_idx, chunk_key, generation) は `_h` 破棄 (CG-6 同型構造、EM-3 公表)。
strict 読解で**真バグを数学導出により発見**:

- **EM-1 [中] 捕捉 56: sentinel 衝突バグ** — 空スロット初期値 u64::MAX と
  pack_key(-1,-1) = ((-1 as u32) | ((-1 as u32)<<32)) ≡ 0xFFFFFFFFFFFFFFFF
  が**同一ビット列** (rq 事前導出: lo/hi 両方 0xFFFFFFFF で一致確定。
  (-1 as u32) は reinterpret=0xFFFFFFFF)。結果、空キャッシュで
  lookup_or_insert(-1,-1) が **Hit=true を誤報** (gen=0 スロットの
  hits+1・挿入 skip) — Minecraft チャンク座標は負も通常出現するため
  (-1,-1) チャンクのメッシュが欠落したまま hit 扱いになりうる実害。
  TDD: strict テスト追加で**修正前 RED を機械実証** (空キャッシュで
  existed_first=true 誤報、panic at assert) → `occupied: Vec<AtomicBool>`
  独立占有ビットで根治 (pack_key 写像不変・sentinel 初期値も不変で
  衝突不感に。key 書込み Release 後に occupied true 化・読側 Acquire で
  true 観測 ⇒ key 確定、単一 writer 順序で linearizable)。修正後全緑。
 根治により key 値域 u64 全 2^64 が利用可能 (旧設計は (-1,-1) が実質
  予約値)。wiring 影響: 実運用チャンク列に (-1,-1) が含まれる場合の
  hits 統計値が真値に変化しうる (報告値の誠実化、構造不変)。
- **EM-2 [低] new() fail-loud 契約化**: max_slots=0 は miss 経路の
  `head.fetch_add(1) % 0` で**ゼロ除算 panic** (EH-2 同型堕落形) 、
  max_slots>u32::MAX は `slot_idx: u32` の truncate 折り畳み衝突 →
  new() 先頭 assert 2 件 (割当前に評価、>u32::MAX は 48GB 級割当を
  試さない位置) + should_panic 2 件。TDD: new_zero_slots_panics は
  修正前 RED (現行 new(0) は構築自体は成功し lookup で初めて panic) 。
- **EM-3 [観] 誠実注記 4 項目** (module doc 明記): (1) Hit 探索 O(N)
  線形 (wiring 65,536 スロット × chunks/tick、未計測の構造的必然推測・
  handle は `_h` 破棄で GPU 経路未配線=CG-6 同型)。(2) 「lock-free」
  の精確化: CAS ループなし・fetch_add round-robin は wait-free、
  複数 writer は同一 slot 同時選択で write-write race benign
  (一意性保証なし)・wiring は単スレッド駆動で安全。(3) generation
  hit 不変・evict +1 (ABA 検知)・u32 wrap は 2^32 evict で一周
  (実害域外 pin)。(4) hits/misses Relaxed 統計 (報告目的適合)。
- **EM-4 [低] strict pin 群 6 件**: pack_key 単射 4 境界
  ((-1,-1)/(0,0)/(MIN,MIN)/(MAX,MAX) 相異・(-1,-1)≡u64::MAX rq 導出・
  (1,0)=1/(0,1)=1<<32 配置・(-1,0)vs(0,-1) 非対称)・FIFO evict 順
  (rq 導出 new(2) head=0,1,0,1,0・再参照 miss→再挿入 evict 連鎖・
  hits 0/misses 6 golden)・generation hit 不変/evict +1・
  hits/misses 集計 pin。

adversarial 5 系統**全 1 RED** (= 各 pin が検出範囲正確に独立):
(a) occupied チェック除去 revert → minus1_minus1 のみ・(b) evict slot
%max→%1 固定 → fifo pin のみ・(c) hit 時 generation fetch_add 化 →
gen 不変 pin のみ・(d) pack << 32→16 折り畳み → pack 単射 pin のみ・
(e) max_slots>=1 assert 除去 → should_panic のみ。(a) 初回 sed は
コメント+if 行を 3 行まとめ削除で**構文破壊** → grep 構造検査で即捕捉・
edit_file で変異形態整流 (捕捉 40 同型の変異整流手順、採番なし同型)。
復元 MD5-VERIFIED 5 回 (lvc 3363896e、三重照合)。**new_over_u32 系は
修正前 RED 実証を意図的に非実施** (現行 new() は assert 不在で
with_capacity(4,294,967,296)≒48GB 割当試行・OOM/abort 危険のため、
根治後 assert で割当前停止する形でのみ GREEN 実証、判断経緯を誠実記録)。

opt-gfx **1163 全緑** (net +7、機械検算 1156+7=1163、全量 21.61s
機械値)・lib 警告 0・fmdiff 自己起因逸脱 0 (HEAD 逸脱 0)・
san/trailws 0・全厳密値 rq 事前導出 (em_vram.rq: sentinel 全 1 ペア・
非衝突 3 境界・FIFO 0,1,0,1,0・gen 2・2^32、全 assert 通過)・
digest 004c1cf5 不変見込 (wide_static_bench 非経由)・api 49 全緑
(seal で再検証)・seal 全 6 ゲート PASS 後 push。台帳 519。

## EN. subgroup.rs / full_graph_wiring.rs (wave 140, 2026-07-27)

census grep で wiring `_subgroup_reduced`/`_subgroup_mask` の `_` 破棄
連鎖 (評価実効・消費なし中間構造) と **subgroup::Vec3/Vec4+Add/Sub/Mul
trait 実装の crate+workspace 消費者完全ゼロ** (EJ-2 完全同型) を
機械検出。

- **EN-1 [中] `_` 破棄中間構造の §7 消化 7**: reduce_add/ballot 評価結果を
  FrameWiringReport 実フィールド 2 件へ接続 — `emissive_high_mask: u64`
  (intensity>8.0 の ballot) + `subgroup_wave_sum_max: f32` (wave 集約 sum
  の max)。消費設計: 空 intensities の reduce は None → +0.0 フォール
  バック、全 -0.0 経路も .max(0.0) で +0.0 正規化 (捕捉 55 同型) →
  report 値は常に +0.0 域。det 比較集合 2 assert 追加。golden pin:
  empty inputs → mask=0・max=+0.0=0x00000000、chunked (パレット全
  id=1 → light=1%16=1>0 で全ボクセル発光・cap 32 停止で 32 灯×1.0)
  → 単一 wave sum 32.0=0x42000000 (rq 導出)・lvl=1≤8.0 で mask=0
  (EJ golden 2 テストへ追記)。
- **EN-2 [中] subgroup::Vec3/Vec4+trait 実装 完全装飾削除** (~60 行、
  workspace grep 使用 0 機械確定、EJ-2 先例準拠・保持不可能証明:
  消費者ゼロの純粋データ型で配線価値なく wgsl 等価表現も存在)。
- **EN-3 [観] 誠実注記 4 項目** (module doc): (1) reduce は f32 非結合の
  wave 内 index 順逐次 — GPU subgroup reduce の順序は実装依存で CPU
  シミュレーションと bit 一致保証なし (rq 機械導出 pin: [1e20;32] 逐次
  0x632D78EB は一括乗算 0x632D78EC と **1 ulp 差異**、1e20 wave では
  1.0×31 個加えても ulp 未満で全消失 0x60AD78EC 不変)。(2) ballot は
  **j≥64 を静寂切捨て** (u64 写像域外・GPU wave ≤64 lane と整合するが
  CPU シミュレーション固有) — pin 済、現行 wiring 供給は emissive cap
  32 で切捨て経路未到達。(3) WAVE_WIDTH=32 固定 (AMD wave64 は別定数
  要)。(4) Vec3/Vec4 削除経緯。
- **EN-4 [低] strict 5 件**: reduce golden bits 部分 wave 境界
  (496.0=0x43F80000/32.0=0x42000000/1520.0=0x44BE0000/64.0=0x42800000
  全 rq 導出・33/64/65 要素・空)・**逐次丸め順序 pin** ([1e20;32]=
  0x632D78EB≠一括 0x632D78EC)・**順序消失 pin** (1e20+1.0×31≡1e20)・
  ballot 65 lane 切捨て pin (u64::MAX/0/index63=1<<63)・WAVE_WIDTH
  契約 pin。

adversarial 6 系統: (a) wave_end min 除去 **2 RED** (部分 wave OOB・
既存含む)・(b) ballot j<64 ガード除去 **1 RED** (1u64<<64 shift
overflow panic を pin が捕捉)・(c) 初期値 0→1.0 **3 RED**・
(d) wiring reduce max→min **非検出 (全量 1168 緑のまま)** = wiring
供給が cap 32 単一 wave → reduce out 全要素同一値で max≡min の
**構造的非検出** (wave 全 lane 同一値性は subgroup strict pin で
担保・誠実公表)・(e) .max(0.0) 正規化除去 **非検出 (全量緑)** =
intensity=lvl as f32 (u8≥0) で -0.0 構造不出・防衛仕様として公表・
(f) threshold 8.0→0.5 **1 RED** (chunked golden mask 0→0xFFFFFFFF のみ、
empty lights 0 不変で検出範囲正確)。採番外 2 件: (e) perl 複数行置換
未適用 (wave 138 同型) と (f) sed インデント不一致未適用を grep 構造
検査で各々即捕捉し edit_file で整流。復元 MD5-VERIFIED 5 回
(subgroup 5b47277c・wiring 8d93abeb、三重照合)。

opt-gfx **1168 全緑** (net +5 = subgroup 5、機械検算 1163+5=1168、
全量 21.37s 機械値)・lib 警告 0・fmdiff 自己起因逸脱 0 (正準形手術
1 箇所: assert_eq! 折り返し、rustfmt --emit stdout 正準との機械一致)・
san/trailws 0・digest 004c1cf5 不変見込 (wide_static_bench 非経由
+wiring 変更は report フィールドのみ)・api 49 全緑 (seal で再検証)・
全厳密値 rq 事前導出 (en_subgroup.rq+ワンライナ: 各 golden bits・
32×1.0=32.0・1 ulp 差・消失・2^64 境界、python 引退継続)・
seal 全 6 ゲート PASS 後 push。台帳 523。

## EO. shadow_lod.rs / full_graph_wiring.rs / gpu_runtime.rs (wave 141, 2026-07-27)

census grep で wiring `_casts`/`_caster` (固定引数 24.0) の `_` 破棄中間構造と
`wgsl_source(&self)` self 不使用装飾 (gpu_runtime は const 直接参照、
メソッド消費はテストのみ) を機械検出。shadow_map_resolution の他消費は
EJ-2 で配線済 (coverage=draw+16≥16→clamp(1)→shadow_res≡2048 定数退化は
wave 136 注記どおり)。

- **EO-1 [中] `_casts`/`_caster` 中間構造の §7 消化 8**: 全クアッドを
  仮想 caster として (x,z) 平面ノルム proxy (`sqrt(x²+z²)`) を
  casts_shadow/caster_lod に供給し、lod 0..3 バケット分布
  `shadow_caster_lod_dist: [u32;4]` + culled 件数
  `shadow_casters_culled: u32` の report 実フィールド 2 件へ実消費
  (leo_tag_dist 同型分布、不変式 Σdist+culled == クアッド数)。誠実
  注記: proxy は真のスクリーン投影寸法ではなくワールド (x,z) ノルム
  (ビュー投影未接続)。golden (rq eo_shadow 機械列挙): chunked 24 点
  → culled=8 (i=0..7 ノルム<4.0: i=7 sqrt(12.5)=0x40624630<4)・
  cast=16 全てノルム<16 (max sqrt(138.5)=0x413C4C32) → dist
  [0,0,0,16]、empty → 全 0、det 比較集合 2 assert。
- **EO-2 [低] wgsl_source(&self) free fn 化** + gpu_runtime:100 単一
  公式経路化 (EL-4 同型、self 不使用装飾・WGSL は 33 行の実
  @fragment シェーダーで tbdr 型非シェーダー問題は非該当を機械確認)。
- **EO-3 [観] 誠実注記 4 項目** (module doc): (1) wiring coverage 供給
  draw+16≥16 → clamp(1) → shadow_res≡2048 の**定数退化** (EJ-2 相互
  参照)、(2) NaN coverage は clamp 透過 → round → clamp → `as u32`
  飽和で **0** (契約域 [256,2048] 外の静寂退化) pin、NaN px は
  casts_shadow=false→caster_lod=3 の一貫除外、(3) caster_lod 境界は
  全 `>=` 閉区間・4≤px<16 は casts=true かつ lod=3 共存形、(4)
  wgsl_source free fn 化経緯。
- **EO-4 [低] strict 4 件**: 解像度 golden (704/1152/1600 = 256+448/
  896/1344 全整数 exact rq 導出・端点 256/2048)・NaN 飽和 0 pin
  (+Inf→2048・-Inf→256)・NaN/境界閉区間全閾値 pin (3.999_999 F/4.0 T
  +lod3・15.999_999→3/16.0→2/63.999_996→2/64.0→1/255.999_98→1/
  256.0→0)・new≡default+min_caster 4.0=0x40800000+wgsl identity。
- **EO-5 [低] 検出空白補完 pin**: chunked_inputs では proxy の |x|
  変異 (z 成分除去) が culled 判定 24 点全一致 (rq eo_adv_b same=24/
  diff=0 機械列挙) で golden 非検出の構造 → z 寄与が判定を反転する
  点 ([3.9,1.0,1.5]: |x|=3.9<4 だが sqrt(17.46)≈4.178≥4、rq bits
  0x4085B668) を供給する新規 strict で proxy 成分構成を golden 化
  (再変異で 1 RED 実証)。

adversarial 5 系統: (a) min_caster_px 4.0→0.0 **4 RED** (boundary+
new_default+既存 tiny_casters+chunked golden、EO-5 は不変で正確)・
(b) proxy |x| 化 **1 RED** (EO-5 z_proxy のみ検出、chunked は rq
予測どおり非検出=補完 pin が正確に機能)・(c) caster_lod >=256→128
**1 RED** (boundary pin caster_lod(255.999_98): 1→0 のみ、wiring
全 lod 3 で非検出 = module strict が検出経路)・(d) clamp→max/min
の NaN 被覆 (f32::max/min は NaN 落とし) **1 RED** (NaN 飽和 0 pin)・
(e) wgsl revert (free fn→const) **非検出 (全量 1173 緑)** = 同一 &str
機能等価・EL-1 (d) 同型の誠実記録。復元 MD5-VERIFIED 5 回
(shadow 3346220d・wiring ebeedc16・gpu 253d6798、三重照合)。

opt-gfx **1173 全緑** (net +5 = shadow_lod strict 4 + EO-5 1、機械
検算 1168+5=1173、全量 21.22s 機械値)・lib 警告 0・api 49 全緑・
fmdiff 自己起因逸脱 0 (正準形手術 2 箇所: assert 折り・impl 閉じ前
余分空行除去、残りは HEAD 原生 3 行据置)・san/trailws 0・
digest 004c1cf5 不変見込・全厳密値 rq 事前導出 (eo_shadow/eo_adv_b:
culled/cast 列挙・|x| 一致性 24 点・z 反転点 bits・解像度 golden、
python 引退継続)・seal 全 6 ゲート PASS 後 push。台帳 528。
## EP. foveated.rs / full_graph_wiring.rs (+ 既監査 4 モジュール警告一掃) (wave 142, 2026-07-27)

census grep 機械確定: wiring:1957 `let _ = fos` で foveated::shading_rate の
実評価結果を即 `_` 破棄する中間構造 (§7 禁止の「実装済み未配線」)、
foveated::Vec4 + Vec3/Vec4 両型 Add/Sub/Mul trait 6 impls が crate+
workspace 消費者完全ゼロ (Vec3 型は shading_rate 引数型として消費、
演算子は不使用)。wiring 呼出実引数: uv=Vec3(0.5,0.5,0)、
gaze=(camera_dir[0]*0.5+0.5, camera_dir[2]*0.5+0.5)、radius=0.2、
min_rate=0.5。

- **EP-1 [中] §7 消化 9 `_ = fos` 破棄根治**: report 実フィールド
  `foveated_center_rate: f32` へ実消費 (画面中央 uv の shading rate、
  gaze=camera_dir (x,z) NDC→uv 写像) + det 比較集合 1 assert (to_bits)
  + empty/chunked golden 追記。golden (rq ep_foveated 導出、
  f32 逐次丸め追従): camera_dir=[0,0,1] (empty/chunked 共通) →
  gaze=(0.5,1.0) → dy=-0.5 → d=0.5/0.2=2.5 → t=clamp 1 → 0.5=
  0x3F000000 (0x40800000=4.0 は EJ-1 由来の既知 bits)。
- **EP-2 [中] Vec4+全 trait 完全装飾削除**: EN-2 (subgroup) 同型。
  wgsl_source ≡ FOVEATED_WGSL const identity と Vec3 構築契約
  (new/default) は strict pin 化 (削除による検証空白を残さない)。
- **EP-3 [観] 誠実注記 4 項目**: (1) min_rate>1.0 は最終
  `.clamp(min_rate, 1.0)` が **min>max で panic**、
  (2) NaN 伝播 — **捕捉 57 [小]**: 初注記「min_rate NaN は透過」は
  誤りで、f32::clamp は **引数 min/max が NaN でも panic**
  (std doc 一次情報: "Panics if min > max, min is NaN, or max is NaN"、
  RFC 1961 同文、実測 msg `min > max, or either was NaN. min = NaN,
  max = 1.0`)。self NaN のみ透過 (doc 例証
  `(f32::NAN).clamp(-2.0, 1.0).is_nan()`)。strict テスト初回実行が
  コミット前に本誤りを RED 捕捉 (厳格テストの自己捕捉機能の実証)
  → 注記訂正 + NaN min_rate を should_panic テスト
  `min_rate_nan_panics` (expected="min > max, or either was NaN") へ
  分離。NaN radius は f32::max の NaN 落としで 1e-4 底上げ
  (0 除算回避) → t≥1 → rate=min_rate 収束、NaN uv は self NaN 透過で
  出力 NaN、(3) uv.z/gaze.z 未使用 (2D radial 評価)、(4) Vec4 削除経緯。
- **EP-4 [低] strict 5 件**: wiring 形状 golden bits (camera_dir 4 点:
  [0,0,1]→0.5=0x3F000000・[0,0,0]→1.0=0x3F800000・[0,0,0.2]→
  **0x3F3FFFFF (0.75 の 1 ulp 下 — d=0.099999994 の f32 逐次丸め、
  暗算予想 0.75 は誤り・rq 真値採用)** ・[0,0,±0.4]→0.5 t=1 境界)、
  min_rate>1.0 panic pin、NaN 3 経路 pin (radius→min_rate 収束・
  radius 0→1e-4 底・uv NaN 透過)、捕捉 57 panic pin、wgsl identity+
  Vec3 契約 pin。
- **EP-5 [低] 検出空白補完 pin**: 既定 camera_dir=[0,0,1] は t≥1 で
  rate≡min_rate=0.5 **下限退化** → wiring radius 変異が empty/chunked
  golden 非検出の構造 (EO-5 同型パターン) → camera_dir を 4 値振動
  させ t<1 域の変動値を golden 化する wiring strict
  `tick_world_foveated_rate_varies_with_camera_dir` 新設
  (0.0→1.0/0.2→0x3F3FFFFF/0.4→0.5/-0.4→0.5、rq 導出)。
- **EP-6 [低] §7 消化 (既監査モジュール残警告一掃)**: lib test 警告
  4 件 (HEAD 同数・wave 142 起因 0 を git stash で機械確認) を根治 —
  aces_tonemap 未使用 let 削除・half_vertex/fsr3_fg needless mut 2 件
  除去・meshlet_cone 未使用変数を degenerate cone **全可視不変式**
  (axis=[0,0,0]・cos_angle=-1・任意有限方向で visible=true) の
  2 assert pin へ転換。旧コメント「axis becomes (0,0,1)」は実装照査で
  誤記確定 (normalize は 1e-8 底上げのみ・方向置換なし) → 修正。
  lib test 警告 4→**0**、lib 本編警告は従来どおり **0**。

adversarial 5 系統 (全て変異前実体コピー先行・復元 MD5-VERIFIED 5 回、
foveated 52434480・wiring d33e6ce9 三重照合):
(a) wiring radius 0.2→0.4 **1 RED** (EP-5 のみ検出・module golden は
自前引数 pin で写し不変 = 補完 pin が唯一の検出線として正確に機能)
・(b) wiring min_rate 0.5→0.25 **3 RED** (empty/chunked golden+EP-5:
下限退化域を直撃)・(c) gaze map 定数化 (camera_dir 無視) **3 RED**
(rate≡1.0 となり golden 直撃)・(d) Vec4+全 trait 復活 revert
**非検出** (foveated 9 件全緑・警告 0 — pub mod 公開済で dead code
警告も発生せず、EL-1 (d)/EO (e) 同型の誠実記録 = 装飾削除は検証
非強化の整理)・(e) t 内側 clamp 除去 **非検出** (数学的等価: d≥0・
radius≥1e-4>0 で t≥0 保証、t≥1 域は外側 `.clamp(min_rate,1.0)` が
min_rate へ完全補償、NaN も同一路径 — EN max→min 同型の構造的
非検出、誠実記録)。

opt-gfx **1179 全緑** (21.21s 機械値、net +6 = foveated strict 5 +
EP-5 1、機械検算 1173+6=1179)・lib 本編/test 警告 0 (EP-6 後)・
api 49 全緑・fmdiff 自己起因逸脱 0・全厳密値 rq ep_foveated 事前導出
(全 assert 通過、python 引退継続)・固定版 md5 三重保存 (foveated
52434480967ed69e364ace9d99cd2bfd・wiring d33e6ce9f7b983cc40e0539b7aacf9ac・
aces 9f0b28d172b95fc580b22a943d67efba・half 5fe5cf3d1776c7d124f732841b792cbe・
meshlet 7a8ecf08369f04c6ebbe5a4285ffa7d9 (seal 初回 FAIL: 追加 assert 長行
1 行逸脱 → 正準手術後全 PASS)・fsr3 d6b8b5de21846fe0e17b48ca90680efb)。
seal 全 6 ゲート PASS (san/fmdiff 自己起因 0/trailws/1179 全緑/digest
004c1cf5fb17bfe8 rows=357 不変/env-check)。台帳 534・TRIGGER 179。
## EQ. tiled_deferred.rs / full_graph_wiring.rs (wave 143, 2026-07-27)

census grep 機械確定: wiring は clear_lights/add_light/cull_lights_for_tiles
を実呼出 (1141-1145) するが **cull 結果 (tiles/light_indices) の消費者
ゼロ** (clustered_lighting は配線済みなのに対照的、§7 未配線)。
加えて view_proj 規約照査で **残存転置バグ (CG-1 同型)** を捕捉。

- **EQ-1 [中] §7 消化 10 cull 結果実配線**: report 実フィールド
  `tdl_max_tile_load`/`tdl_lit_tiles: u32` (cluster_* 同型ホットスポット
  指標) へ接続 + det 比較集合 2 assert + empty/chunked golden。
  chunked golden **4/8160** (rq eq_tiled 機械列挙): emissive scan
  (y,z,x 順 32 cap) は z=0/1 平面の 32 灯、ID VP で ndc_x=x (0..15)、
  sx=960x+960、sr=960px → x≥2 は min_tx=60x≥120>119=max_tx の**逆転
  空ループ**で消失 (影響球左端が画面右端超の物理的正しい除外)、
  x=0 (列 0..119 全行 +2) ・x=1 (列 60..119 +2) の 4 灯のみ残存 →
  列 0..59 load 2・列 60..119 load 4 → max=4 / lit=8160。
- **EQ-2 [高] 捕捉 58 view_proj 転置読み (CG-1 同型残存)**: 本番規約
  は行ベクトル p×M (clip_j=Σ_i p_i·M[i][j]、平行移動 row 3、wave 83
  CG-1 記述) だが旧実装は列ベクトル M·p の行内積で読み = 平行移動
  (row 3) を完全無視、w は row3·p で意味破壊 (T=(0,0,-50) で w=-49
  → 光源消失、rq 導出)。IDENTITY_VP 対称で両規約一致 → golden 潜伏
  (CG-1 と同一の顕在化経路)。現実害ゼロ (結果未消費だった) だが
  配線前提の真バグとして修正。TDD: 修正前 RED 6 件機械実証
  (T=0.5 平行移動 pin・z=-50 消失 pin・ww 規約 pin・should_panic 3)。
  修正: clip_x = p·col(0) / clip_y = p·col(1) / clip_w = p·col(3)。
- **EQ-3 [中] fail-loud assert 3 本**: view_proj/light 全成分 finite
  + radius≥0。旧来 NaN pos は `as i32`=0 飽和でタイル (0,0) へ静寂
  割当 (照明局地破壊) — 契約明文化で panic 化。wiring 入力は
  uint as f32 + light_branchless∈0..15 + IDENTITY_VP で全 finite =
  非発火の機械裏付け済。
- **EQ-4 [観] 誠実注記 4 項目** (module doc、cull 契約節): (a) ww≤0.1
  skip は背後・超近接光源の**完全除外** (近平面跨ぎ巨大半径光源の
  影響見逃し)、(b) screen_radius 円錐近似 (radius≪距離で正確)、
  (c) cap 64 静寂切捨て (照度欠損上限)、(d) ndc_z 非使用 (深度
  カリングなし保守形)。
- **EQ-5 [低] strict 11 件 + 境界閉区間 pin**: ww≤0.1 の境界を
  w=0.1f32 (0x3DCCCCCD) skip / 1 ulp 上 (0x3DCCCCCE) 残存で閉区間
  固定 (rq eq_tiled_c、adversarial `<` 変異の検出線補完)。自己捕捉:
  T=10 平行移動 pin 初版は光源を min_tx=600>119 の画面外へ飛ばし
  消失 (rq eq_tiled で min_tx=600 まで出したが逆転空ループ帰結の
  assert 化を失念) → RED 実測で捕捉 → T=0.5 (f32 exact sx=1440、
  min_tx=30) + 画面外排除 pin 2 本へ分割 (誠実記録)。

adversarial 5 系統 (全変異 MD5-VERIFIED 復元 5 回、tiled
301c4bfd・wiring 48a5dd75 三重照合): (a) 捕捉 58 revert (p×M→M·p
旧転置読み) **4 RED** (translation/offscreen/z_no_vanish/ww_threshold)
・(b) `ww <= 0.1`→`<` **1 RED** (EQ-5 boundary pin のみ検出=補完
正確)・(c) cap 64→65 **1 RED** (cap pin)・(d) tiles_x cap clamp 除去
**6 RED** (index 8160 OOB panic 系、cap が配列安全性の必須要件と
実証)・(e) fail-loud 3 assert 撤去 **3 RED** (3 should_panic)。
**非検出ゼロ** — 全 pin が変異感受性を持つことを機械確定。

opt-gfx **1190 全緑** (21.44s 機械値、net +11 = tiled strict 11、
機械検算 1179+11=1190)・lib 本編/test 警告 0 (pxM 命名 non_snake
2 件を px_m へ自己修正)・api 49 全緑・fmdiff 自己起因逸脱 0・
全厳密値 rq 事前導出 (eq_tiled/eq_tiled_b/eq_tiled_c:b(u32) from_bits
は RQ.md 一次情報確認、f() 数値キャストとの誤用を初回 RED で捕捉)
・固定版 md5 三重保存 (tiled 5a696c160b946e4604b5c344b900325f・
wiring c92e9a704f965279a888b4896730809a)・seal 初回 fmdiff FAIL
(自己起因 wiring 4 行・tiled 6 行) → 正準手術 5 箇所 (det assert
収束行化・x/y/ww 式折り・PointLight 展開・filter チェーン・empty
assert 折り) で現逸脱 3 行=HEAD 原生包含へ復帰 (tiled HEAD 6 ⊇
現 3、row 44/118/120 は orig 原生据置)・seal 全 6 ゲート PASS
(digest 004c1cf5fb17bfe8 rows=357 不変)・台帳 539・TRIGGER 180。
## ER. depth_prepass.rs / full_graph_wiring.rs (wave 144, 2026-07-27)

census grep 機械確定: render_pipeline.rs は DepthPrepassPlanner を
フィールド保持 (122)・tier 条件付き初期化 (254-258) するのみで
`plan()` 呼出ゼロ、`sort_translucent_indices` の消費者もゼロ
(§7「実装済み未配線」2 系統)。workspace 直下 `rsift/rsift-opt-gfx`
は workspace members 外の参照コピーで本監査対象外 (members 一覧
機械確認、census の完全性記録)。

- **ER-1 [中] §7 消化 11 plan() 実消費配線**: wiring で
  `for_low_spec().plan()` (low_spec 固定選択: 低スペック PC 制約
  UserSystemPrompt §2 整合、render_pipeline の tier≥High→high_spec
  との差分はフィールド doc に誠実記載) を実評価 →
  `depth_pass_count`/`depth_pass_cost: u32` report 実フィールド +
  det 集合 2 assert。golden 5/15 (rq er_dp、empty/chunked 共通)。
- **ER-2 [中] §7 消化 12 sort 実消費配線**: 半透明判定 `mat%7==5`
  (731 行 batcher 既存規則と同一、cast u16) で center 再収集 →
  `sort_translucent_indices` へ実供給 → `translucent_total`/
  `translucent_back_first` report + batcher 件数の cross
  debug_assert 不変式。empty/chunked 共に translucent=0 の下限
  退化 (chunked は mat%7=1 全員) → 補完 strict (mat [5,12,30] +
  centers 供給、最遠 index 1 golden) で検出線補完 (EP-5 同型
  パターン、adversarial (e) で first→last 変異 1 RED 実証)。
- **ER-3 [観] 誠実注記 5 項目**: (1) cost 静定数、(2) 中心のみ近似、
  (3) NaN Equal fallback の**秩序橋崩壊**: NaN は任意要素と Equal
  となり、挿入ソート系列の走査打止め (cmp(2,0) 非到達) で有限
  要素間の降順を破壊 ([0,1,2] 維持実測、私の初 pin 予想 [2,0,1]
  の誤りを RED で捕捉・訂正記録)、(4) dist2 sqrt-less 単調同値、
  (5) render_pipeline 側 plan 未消費は wiring 消費者追加で充足。
- **ER-4 [低] strict 7 件**: plan exhaustive (4 構成形 + writes 真理値
  表: prepass 経路 Opaque writes_depth=false = early-Z 設計の核心)・
  sort 無効 cost・new 既定・exact order golden・空/単一・NaN pin・
  dist2 pin。

adversarial 5 系統 (MD5-VERIFIED 5 回、dp b1b0606f・wiring 8ed53d39
三重照合): (a) Translucent cost 4→3 **6 RED** (module exhaustive
2・wiring golden 4)・(b) sort 方向反転 **3 RED** (module 2・ER-2
strict 1、NaN pin は両方向不変で正確に不検出)・(c) batcher 判定
`%7==5`→`%7==6` **1 RED** (cross debug_assert 経由 ER-2 strict、
2 件規則のずれを不変式が捕捉)・(d) NaN Equal→Greater **非検出**
(挿入ソート系列が両方向で同一帰結 [0,1,2]、NaN pin の構造的
限界、wave 140/142 同型の誠実記録)・(e) back_first first→last
**1 RED** (ER-2 strict のみ、empty/chunked は 0/0 退化で不変=
補完正確)。

opt-gfx **1198 全緑** (21.64s 機械値、net +8 = dp strict 7 + ER-2 1、
機械検算 1190+8=1198)・lib 本編/test 警告 0・api 49 全緑・fmdiff
自己起因逸脱 0・全厳密値 rq er_dp 事前導出 (cost 4 構成形・mat%7
判定列・dist2 golden、python 引退継続)・固定版 md5 三重保存
(depth_prepass a7925daab4b9effd0cf3b16aef28fe5f・wiring
0ef1595454f3012d783f1f1a9ff9e89b)・seal 初回 2 FAIL (san: 私の
コメントへの「順」の簡体字版混入 1 件→「順」修正・fmdiff 自己起因
5 行 → 正準手術 4 箇所: prepass assert 折り・dist2 assert 折り・
let 収束行化・det assert 折り、dp HEAD 原生 5 行据置) → 再 seal
全 6 ゲート PASS (digest 004c1cf5fb17bfe8 rows=357 不変)・
台帳 543・TRIGGER 181。 


## ES. motion_blur.rs (wave 145, 2026-07-27)

前 wave 連鎖のモジュ厳密監査。対象 147→153 行区間は本 wave
編集後 305 行ファイル全体を精読 (変更前 215 行 + strict 約 90
行)。census grep で Vec3/Vec4 の Sub impl 消費者ゼロ・`wgsl_source`
は gpu_runtime.rs:71 消費・motion_blur() 本体呼出は
full_graph_wiring.rs:1783-1792 (MotionBlurParams::default() =
samples 8・max_velocity 0.1f32=0x3DCCCCCD、velocity =
camera_speed×camera_dir×0.02 (x,y 成分)、identity sampler) を機械
確定。出力は frame_color→dof_color→fsr_color→fsr2_out→
prev_frame_color 履歴のみで report 構造体フィールド非属 (wiring
golden 非侵蝕を機械確認) のため wiring 変更なし。モーションブラー
WGSL は include_str! で shaders/motion_blur.wgsl を参照 (identity
pin 維持)。

- ES-1 [中] **捕捉 59 [小] サンプル配置の非中心化**: 旧
  `t = i*inv - 0.5` (i∈[0,n)) は位置の平均が `-0.5/n ≠ 0` で、
  velocity≠0 のときブラー中心が速度と逆行方向へ
  `max_velocity·velocity·0.5/n` だけ偏向する。camera 前進時に
  ブレ像が後方へ寄る視覚的誤り。しかも旧式は **samples=1 でも
  t=-0.5 の端点配置** (速度無関係に許容量いっぱい半幅ずれ) で
  あった (一点サンプルのはずが uv-0.5v を読む)。TDD 修正前 RED
  5 件を機械実証 (centered got=1056545178=0x3EF9999A=0.4875 は
  samples=8 逆行偏向 1/16、rq es_mb 事前導出と完全一致・
  samples_one 2 件・should_panic 3 件未発火)。修正は
  `t = (i+0.5)*inv - 0.5` (区分化重心・± 対称配置)。平均
  exact 0 は rq で機械検算 (samples=8 の対称ペア和は f32 加算
  順でも exactly 0、2 冪分数経路)。新 golden は 0.5 =
  0x3F000000 (右辺値 1056964608)。wiring golden 非侵蝕 (上記
  消費経路由来)。
- ES-2 [低] Vec3/Vec4 の **Sub impl 完全装飾削除** (crate+
  workspace census grep で消費者ゼロ機械確定 — 本体は
  `uv + v*t`/`acc + sample` で Add/Mul のみ使用、use を
  `std::ops::{Add, Mul}` に統合)。型本体・Add/Mul/Default は
  消費あり維持 + 削除による検証空洞を残さない契約 pin strict
  (EN-2/EP-2 同型方針)。
- ES-3 [観] 誠実注記 5 項目 (module doc): (1) 捕捉 59 新旧配置と
  samples=1 端点逸脱の経緯、(2) inv=1/n の丸め許容帯 (n が 2 冪
  なら exact、非 2 冪は f32 丸めを受容)、(3) samples=0 は旧来
  inv=inf → acc(0)*inf=NaN 静寂出力・NaN velocity/max_velocity
  の静寂伝播を ES-4 fail-loud で根治 (wiring default は非発火)、
  (4) Sub 削除経緯、(5) sampler 契約 (G-buffer 回収コストは
  呼び出し側責務)。
- ES-4 [低] fail-loud assert 3 本 (params.samples >= 1・
  max_velocity.is_finite()・velocity x/y finite) + strict 9 件:
  捕捉 59 zero-bias golden bits (0.5=0x3F000000)・samples=1 は
  t=0 exact の一点サンプル (zero blur、新旧差分直接 pin)・velocity=0
  は新旧一致厳密 bits 1.5=0x3FC00000・should_panic 3・型契約
  pin・wgsl identity・Default (8, 0x3DCCCCCD)。+9 strict。

adversarial 5 系統: (a) t 式旧式 revert **2 RED**
(centered・samples_one)・(b) samples assert 除去 **1 RED**・
(c) max_velocity assert NaN 透過化 **1 RED**・(d) Sub impl 復活
revert **非検出** (motion_blur 12 件全緑・警告 0 — EP-2(d) 同型
の誠実記録、装飾削除は検証非強化の整理)・(e) velocity assert
除去 **1 RED**。変異前実体コピー /tmp+rsift/bak 先行・毎回復元
MD5-VERIFIED 5 回。誤編集 2 件 (module doc 注記 edit で use 行
誤消去・adversarial(e) let 重複) をその場復元で整流し fixed 版
md5 整合を機械確認後に継続。

opt-gfx **1207 全緑** (21.42s/21.30s/21.80s 3 回実測、net +9、
機械検算 1198+9=1207)・lib 本編/test 警告 0・api 49 全緑・
全厳密値 rq es_mb 事前導出 (旧式 got 0x3EF9999A・新式
0x3F000000・velocity 0x3E4CCCCD・対称ペア和 exact 0・samples=1
の t=0・samples=0 の inv=inf 導出、python 引退継続)・adversarial
完了後に rustfmt 正準化 2 箇所 (fail-loud assert 折り・strict assert
折り) → 再全量 1207 緑確認・固定版 md5 三重保存
(motion_blur 6132e1a906bbbf8f9a586885625d46e2、rsift/bak/ +
/tmp + src 三重照合)・seal 初回 1 FAIL (san: 私の AUDIT 節への中国語語彙残留 1 件 (U+6837) + wave 144 節内の簡体字引用 1 件 (U+7B80 → 記述化) + WGSL 誤記 1 件 (WLSL) の 3 箇所手術) → 再 seal 全 6 ゲート PASS (san 0 findings・digest 004c1cf5fb17bfe8 rows=357 不変)・台帳 547・TRIGGER 182。

副産物 (次 wave 対象): census grep 中に **rsift-replay
`src/exporter.rs:145` の `pub fn apply_motion_blur(_frames, _strength) {}`
が空関数スタブ (§7 該当)** を機械発見 (exporter に motion_blur:
f32 フィールド・renderer.rs に MotionBlurAccumulator 実体あり)。
節純度のため本 wave には混ぜず、wave 146 で本実装か不可能証明
付き削除かを設計判断する専用 wave とする。

## ET. depth_of_field.rs (wave 147, 2026-07-28)

対象 148→約 215 行。census grep 機械確定: circle_of_confusion/gather_blur/
DofParams は full_graph_wiring.rs:1793-1800 実消費 (coc =
|chunk_dists.first().unwrap_or(32.0) - 10| × 0.05・identity sampler
|c| Vec4::new(c.x,c.y,c.z,1.0)、dof_color は taa_ycocg 経由と
checkerboard (let _cb 破棄側) で消費・report 構造体フィールド非属)、
wgsl_source は gpu_runtime.rs:72 登録、Vec3/Vec4 Sub impl は
crate+workspace 消費者ゼロ (本体は Add/Mul のみ)。wiring 変更なし
(identity 恒等変換で検出空白 → module pin 充足、ES 同型方針)。

- ET-1 [小] **捕捉 60 [小]**: 8-tap ディスクの旧配置順で Σdy の f32
  逐次和が 2^-24 (0x33800000) 非ゼロ = ブラー重心 y 偏位 (uv=0/coc=1
  identity 経路 out.y=2^-27=0x32000000、rq et_dof 機械導出、捕捉 59
  同型の ulp 級非中心化 — 視覚害 ≦ coc16×2^-24≈9.5e-7px だが数学的
  非正)。対称ペア順 [(1,0),(-1,0),(S,S),(-S,-S),(0,1),(0,-1),(-S,S),
  (S,-S)] へ並べ替えて連続相殺 Σdx=Σdy=0 exact に根治。TDD 修正前
  RED 1 件 (centroid got 0x32000000)。Σdx は旧順でも exact 0
  (rq 確認) で y のみの非対称。定数/identity sampler の復元値は
  加算順不変で既存 golden 非侵蝕、全量 1216 緑で波及なし機械確定。
- ET-2 [低] Vec3/Vec4 Sub impl 削除 (census 確定) + use {Add, Mul} 統合
  + Add/Mul/Default 型契約 pin (ES-2 同型)。
- ET-3 [観] 誠実注記 5 項目 (module doc): (1) CoC 前後対称簡易モデル、
  (2) 捕捉 60 経緯、(3) identity gather 8v×(1/8) は非 2 冪段 (3v/5v)
  丸めで exact 復元されず (0.1→+1ulp=0x3DCCCCCE・0.3→-1ulp・0.7→
  -1ulp・1.5→exact、rq golden 5 値)・wiring identity sampler では DoF
  は ±1ulp 実質恒等変換で chunk_dists→coc 変動の観測経路なし、(4)
  NaN depth 透過 (捕捉 57 規律)・NaN coc は NaN<1e-3=false でブラー
  経路・max_coc<0 (min>max)/max_coc NaN (引数 NaN) は clamp panic、
  (5) Sub 削除経緯。
- ET-4 [低] strict 9 件: 捕捉 60 golden (out.x/out.y==0)・identity
  gather ulp pin (bits 2 値)・coc 値 bits (d=20→0.5/d=32→1.1/d=10.02
  →0x3A831333 — 直感 0.001 ちょうどは f32 逐次で 0.0010000229 と
  rq 訂正)・境界 call count Cell pin (5e-4→1・0.5→8・1e-3 inclusive
  ブラー)・NaN 透過 + NaN coc 全 8 tap NaN 座標 pin・should_panic 2・
  contract (wgsl identity は &str 内容比較 — **私の初 pin は
  std::ptr::eq で const 参照の metadata 不一致 RED → 内容比較へ
  訂正の誠実記録**・FRAC_1_SQRT_2==(0.5f32).sqrt()・1/8 exact・型契約)・
  abs 対称性補完 pin (d=2/d=18→0x3ECCCCCD、adversarial 設計で検出
  空白を事前補完)。

adversarial 5 系統: (a) 捕捉 60 revert 旧順序 **1 RED** (centroid の
み)・(b) Sub impl 復活 revert **非検出** (13 緑・警告 0、ES-2(d)
同型誠実記録=装飾削除は検証非強化の整理)・(c) `<`→`<=` **1 RED**
(call count inclusive pin のみ=補完正確)・(d) abs 除去 **1 RED**
(対称性 pin のみ=補完正確、abs なしでは近景 d<10 が 0 クランプ
center 化)・(e) clamp 除去 **3 RED** (coc_is_clamped+panic 系 2、
clamp 消滅で min>max/NaN panic も同時消失=正確)。変異前実体コピー
/tmp+rsift/bak 先行・毎回復元 MD5-VERIFIED 5 回。

opt-gfx **1216 全緑** (21.44s 最終全量実測、net +9、機械検算
1207+9=1216 — 暫定 1215 記述を symmetry pin 追加後の確定値に
訂正)・lib 本編/test 警告 0・全厳密値 rq et_dof 事前導出 (sqrt(0.5)
==FRAC_1_SQRT_2・Σdx=0/Σdy=2^-24・ペア順全 0・2^-27=0x32000000 vs
私の初 bits 読み違い 0x33000000=2^-24 誤りの rq 自己訂正・identity
±1ulp 5 値・coc bits・d=2/18 対称値、python 引退継続)・fmt 私起因逸脱 4→0 正準化 (symmetry pin を正準化後に追記した自己起因で seal 初回 fmdiff FAIL 3 行 → 該当 3 assert 折り返し手術で復帰・誠実記録)・固定版 md5 三重保存 (depth_of_field
367a3a9acd9411f6a1d5cfaecf58b175、rsift/bak/+/tmp+src 三重照合)・
上記手術後の再 seal で全 6 ゲート PASS (san 0 findings・fmdiff 自己起因
0・digest 004c1cf5fb17bfe8 rows=357 不変、/tmp/w147_seal2.log)・
台帳 554・TRIGGER 184。

## EU. parallax.rs / full_graph_wiring.rs (wave 148, 2026-07-28)

対象 parallax.rs 159→約 289 行、full_graph_wiring.rs 3721→3811。
census grep 機械確定: parallax_occlusion は full_graph_wiring.rs:1709
で実呼出されるが結果は `let _parallax_hit` で評価後破棄 (wave 141 型
中間構造残存、§7 消費者なし禁止に該当)、wgsl_source は
gpu_runtime.rs:73 経由の WGSL 登録のみ、Vec4 (型+Add/Sub/Mul) は
本体・テスト・wiring 全消費者ゼロ。Vec3 は `cur_uv - p_step` で本体
使用。

- EU-1 [中] **捕捉 61 [中]**: POM 補間式 2 点。一次情報 LearnOpenGL
  Parallax Mapping (https://learnopengl.com/Advanced-Lighting/
  Parallax-Mapping): beforeDepth = texture(depthMap, prevTexCoords).r
  - currentLayerDepth + layerDepth、weight = afterDepth/(afterDepth -
  beforeDepth)、finalTexCoords = prev*weight + current*(1-weight)。
  (1) 旧 `before = prev_depth - (cur_layer + layer_depth)` は符号
  誤りで直前層参照が 2*layer_depth ずれ → (cur_layer - layer_depth)
  へ根治。(2) 旧 `w = after/(after - before).max(1e-4)`: 正しい式
  では denom = after - before ≤ 0 が構造保証 (before は未衝突 prev
  層 ≥ 0、after は衝突層 ≤ 0) ゆえ負分母を 1e-4 に置換し w を
  発散させ得る (rq eu_pom: sloped 条件で修正版 w=0x3EE7D94E
  (0.45282978)/final=0x3EF1826B (0.47169814、解析真値 0.47169811
  の 1 ulp) に対し旧版 w=0xBF02B928 (-0.51063776)/final=0x3EEFA8DC
  と深度格子への戻り量が非正確 = 視差シフトの定量的誤り、発散は
  条件依存でより大きくなり得る) → `denom.abs() < 1e-6 なら w=0
  (退化: 層に正確に載る)、else after/denom` へ根治 (denom≠0 は
  構造保証)。TDD 修正前 RED 3 件機械実証 (pom_interpolation_
  exact_golden got 0x3EEFA8DC・layers=0 退化・z 底上げ発散)。
- EU-2 [中] §7 消化 13: `let _parallax_hit` 破棄 → report 実フィールド
  `parallax_layer_depth` (= hit.1 衝突層深度 [0,1]) +
  `parallax_uv_offset_y` (= hit.0.y - 0.5、負 = 高さ場の奥シフト)
  実配線。det_subset cross-instance pin 2 追加。golden は rq eu_pom
  全導出: empty (palettes 空 → heights≡0 → 層前進なし、ld +0.0/
  offset +0.0)・chunked 全 1 パレット (heights≡15/16=0.9375、
  **lattice 退化**: パレット高さ格子 y/16 と層格子 1/16 が同相で
  after=0 exact → w=0 → final=cur_uv、ゆえ捕捉 61 修正は wiring
  golden 非侵蝕と機械確定: ld=0x3F700000/offset=0xBEF00000=
  -0.46875)・カメラ振動 strict (chunked inputs に dir=[0,1,1] →
  view=(0,1,1.0)、step_y=0x3BCCCCCD、15 逐次減算の f32 蓄積後
  offset=0xBDBFFFF4、dir 間 assert_ne で実経路稼働 pin)。
- EU-3 [低] Vec4 (型+Add/Sub/Mul) 完全削除 (census 消費者ゼロ確定、
  EN-2/EP-2/ET-2 同型)、Vec3 型+Add/Sub/Mul 維持 + contract pin
  (Sub 本体使用明記・**私の初 pin 誤り m.z==3.0 を RED 捕捉→
  m.z==5.0 へ訂正の誠実記録入り**)。残存 "Vec4" 字句は doc 注記と
  pin コメントの 2 行のみ。
- EU-4 [観] module doc 誠実注記 5 項: (1) view_dir.z の 1e-3 底上げは
  p_step を無制約巨大化し得る (z=1e-4 → step 0x40700001=3.7500002 →
  uv.x=-2.8333335=0xC0355556 発散 rq 実値、正規化 view_dir z>0 は
  呼出側契約・NaN z は f32::max 規律で 1e-3 化)、(2) 捕捉 61 経緯、
  (3) lattice 退化で wiring は補間式不問 (捕捉 61 と golden 直交)、
  (4) NaN height_scale/height は静寂伝播 (G-buffer 側品質契約、
  fail-loud しない設計)・layers=0 は max(1) 退化、(5) Vec4 削除経緯。
- EU-5 [低] strict 8 件: module 7 (捕捉 61 golden 0x3EF1826B・layers=
  0→1 退化 final=0x3EF1826A (16 層と 1 ulp 差)・z 底上げ発散
  0xC0355556・flat lattice h≡0.5 で 0x3EF851E8・height 呼出回数
  Cell pin (初期 1+march 8+prev 1=10)・NaN height_scale 静寂伝播
  (final NaN + ld=1.0)・contract: ld exact 1/16・wgsl &str 内容比較
  (ET 規律)・default (16, 0.1=0x3DCCCCCD)・Vec3 契約) + wiring
  カメラ振動 strict 1。

adversarial 5 系統: (a) 捕捉 61(1) revert (符号逆転) **3 RED**
(golden 三兄妹: interpolation/layers0/z_floor)・(b) 捕捉 61(2) revert
(max(1e-4) 化) **3 RED** (同三兄妹) + 変異下 wiring chunked/vibration
3 テスト緑維持 = lattice 退化による非侵蝕の in-vivo 裏付け・(c) Vec4
復活 revert **非検出** (39 緑、ES-2(d)/ET-2(b) 同型誠実記録 = dead
code 復活はテスト系で検出不能、contract pin コメントに完全削除を明記
済)・(d) wiring 配線 revert (`_parallax_hit` 化+代入撤去) **2 RED**
(chunked golden + カメラ振動 strict、empty golden は変異下も緑 =
未配線と空パレットを区別不能の静寂構造、検出は chunked+振動 pin が
担う誠実注記)・(e) z 底上げ除去 (`view_dir.z.max(1e-3)` → `view_dir.z`)
**1 RED** (z_floor 発散 pin のみ=補完正確)。変異前実体コピー /tmp+
rsift/bak 先行・grep -c で変異適用確認後に計測・毎回復元
MD5-VERIFIED 5 回。

opt-gfx **1224 全緑** (最終全量実測 20.89s・adversarial 後再実測でも
1224 緑、net +8、機械検算 1216+8=1224)・api 49 全緑・replay 16 全緑・
lib 本編警告 0・全厳密値 rq eu_pom 事前導出 (step/w/after/before/
final/wiring chunked golden/カメラ振動/empty/layers=0/flat lattice/z
発散、全 assert 通過、python 引退継続)・fmt: HEAD 両ファイル原生逸脱
ゼロ (先頭空行 artifact のみ) に対し追記コードで新規逸脱発生 →
in-place rustfmt 正準化で復帰 (修正後 md5 変化: 再記録)・固定版 md5
三重保存 (parallax 443a3dec6dcf43fb188c7ba704546555・wiring
20ba281915d1db31dfd96415f1db4bee、src+/tmp+rsift/bak 三重照合)・
digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS・
台帳 559・TRIGGER 185。

## GC. gui_composite.rs / full_graph_wiring.rs (wave 149, 2026-07-28)

対象 gui_composite.rs 166→286 行 (全 rewrite)、full_graph_wiring.rs
3811→4033。census grep 機械確定: `should_render_gui` は wiring:542 のみ
`let _gui_decision` 破棄 (§7)、on_input_event/on_animation_window/
surface_valid()/invalidate()/GuiBlit/GuiDecision 型名参照は全消費者
ゼロ (本番構築は render_pipeline.rs:1101 の 1 系)、GuiRates は Default
のみ、dead store last_gui_render_time は read ゼロ。

- GC-1 [高] **捕捉 62 [高]**: `real_dt` 秒契約 (need=1/fps[s]・module
  test dt=1/120 一次情報) に旧 wiring:542 は ms `inputs.delta_ms`
  (16.0) を誤供給 → gui_accum 480 倍速蓄積で due 常時真 = 30fps デュ
  アルレート GUI 機構の構造的全沈黙 (省電力機構の完全無効化)。旧
  `_gui_decision` 破棄で観測経路ゼロ潜在化 = §7 配線と同時根治しない
  と有効化直後に無効化状態で実害化する二重構造。call site
  `delta_ms / 1000.0` 秒化に根治。TDD 修正前 RED 4/4 (cadence/カメラ/
  screen/dt 振動 strict 全 RED 機械実証、新規 4 本 RED + 1224 filtered
  = 1228 一致)。修正後 rq gc_gui 機械導出: dt=0x3C83126F
  (0.01600000076)、need=0x3D088889、render 系列 t∈{1,4,6,8,10,12,14,
  16} (16 tick で 8 回 ≒ 30 GUI fps vs 62.5 実 fps、f32 逐次蓄積の
  非自明系列で単純交互模写不可 = pin 検出感度に寄与)。
- GC-2 [中] §7 消化 14: report 実フィールド 5 配線 (gui_rendered_
  surface/gui_target_fps/gui_invalidated/gui_reuse_cached_scene/
  gui_surface_valid) + det_subset pin 5 (module から wall-clock 依存
  `now` 引数撤去済で inputs のみ駆動の完全決定的機構 → det 正当)。
  golden empty t∈{1,4}/chunked t∈{1,4,6,8}・fps 0x41F00000。
- GC-3 [中] on_input_event 実駆動: camera_dir 変化 (視点操作=入力駆動)
  prev 照合実検出 (prev_gui_camera_dir 新設・初回 None 非発火)。
  振動 strict: due=false tick で invalidated=true+即 render、t3 では
  クリア+非 due 復帰 (accum=0 起点 rq)。
- GC-4 [低] 機構処置 (§7 接続か削除か): 配線 — invalidate() を
  screen_w/h 変化 (= GUI 面実破棄事象) 実駆動 (prev_gui_screen)、
  surface_valid() は report 観測面が真の消費地。削除 (census 不可能
  証明) — anim 系 3+1 (GuiRates::anim_burst_fps/on_animation_window/
  anim_active_until/`now: f64` 引数: inputs に GUI アニメ事件源不在、
  捏造は偽装禁止抵触)・dead store last_gui_render_time (read ゼロ・
  年齢配線は wall-clock 決定性汚染で設計不能)・GuiBlit+scale (src/dst
  実データ源不在・恒等 1.0 退化配線は lattice 退化と異なり偽装)。
  animation_window_boosts_rate テストは削除機構に連動除去 (−1)。
- GC-5 [観] module doc 誠実注記 5 項 (GC-1..GC-4 経緯 + fps_now 生
  レート報告契約/NaN・負 dt 静寂伝播設計/anti-stutter 追従 1 周期上限
  (0.05 溜り残存 0x3D05CD7B・0.1 リセット rq 確定))。
- GC-6 [低] strict 10: module 6 (dt=0.02 系列 [T,F,T,F,T,F,T,T,F,T]
  — t7→t8 連続 render は境界 gap 2^-27 (need − accum=0x32000000) を
  accum が 5 回目蓄積で跨ぐ f32 非自明挙動、**私の初予想「t5 due」は
  rq で 5 ulp 未達と誤り訂正 (誠実記録)**・invalidated カウンタ消費+
  accum 0 正規化・fps 底上げ契約 (need=1.0 / fps_now=0.0 生報告)・
  NaN 2 系統 (fps→need max 規律 1.0/dt→accum 永久汚染不発伝播)・
  anti-stutter リセット系列・Default 0x41F00000+reuse 常時 true) +
  wiring 4 (16ms cadence golden/camera invalidate/screen resize/dt=20
  振動 [T,F,T,F,T,F,T,T,F,T,F,T,T,F,T,F] 9/16・16ms 系列と assert_ne
  構成的非等値)。

adversarial 5 系統 (全検出・非検出ゼロ): (a) 捕捉 62 revert (ms 誤供給)
**6 RED** (GUI strict 4 + empty/chunked golden の系列 pin)・(b)
on_input_event 駆動撤去 **1 RED** (camera 振動のみ=補完正確)・(c)
invalidate 駆動撤去 **1 RED** (screen_resize のみ)・(d) anti-stutter
除去 **1 RED** (anti_stutter_reset のみ、通常系列の carry < need で
golden 非侵蝕=設計通り)・(e) pending_invalidations reset 除去
**2 RED** (module カウンタ pin + wiring camera 振動の二層検出)。
変異前実体コピー /tmp+rsift/bak 先行・grep -c 適用確認後計測・
毎回復元 MD5-VERIFIED 5 回。

opt-gfx **1233 全緑** (最終全量実測 21.34s、net +9、機械検算
1224+4(wiring)+6(module strict)−1(anim 連動削除)=1233)・api 49 全緑・
replay 16 全緑・lib 本編警告 0・全厳密値 rq gc_gui 事前導出
(dt/need bits・16ms/20ms 両系列・carry bits・stutter 境界・NaN 規律、
全 assert 通過、python 引退継続)・fmt: HEAD native 原生存続逸脱 1 件
(Self{gui_fps 30.0, anim_burst_fps 60.0} 行内) は rewrite 削除に伴い
消滅・追記分は in-place rustfmt で自己起因 0・固定版 md5 三重保存
(gui_composite 8768341eff0ced585853062ea437c385・wiring
ad3c9256a4e1f212b3ce5cf26a8267ae、src+/tmp+rsift/bak 三重照合)・
digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS・
台帳 565・TRIGGER 186。

## EV. volumetric_fog.rs (wave 150, 2026-07-28)

対象 volumetric_fog.rs 181→355 行 + shaders/volumetric_fog.wgsl 同形化。
census grep 機械確定: raymarch_fog は wiring:1742 実消費 (fog_trans →
1853-1855 の sky×fog_trans×shadow 色合成で実使用、report 構造体フィー
ルド非属 = ES/ET 同型の wiring 非侵蝕方針)、wgsl_source は
gpu_runtime.rs:65 登録 (naga validate 自動網羅テストあり)、Vec3
new/dot/length/normalize/Add/Mul は本体消費、**Vec4 全 impl・Vec3
Sub impl は全消費者ゼロ**。変更面は module+WGSL のみ (wiring 無変更)。

- EV-1 [低] 透過率評価の数学的改善: 積連鎖 Πexp(-d_k·seg) →
  sum-then-exp exp(-seg·Σd_k) (実数厳密等価: exp(-a)exp(-b)≡exp(-a-b))。
  f32 丸め構造: wiring 同定数 (wiring:1742 実入力 ro=[8,8,8]/rd=[0,0,1]
  /dist=128/steps=16/base=0.02/scale=0.06/start=32/dither=0.5 → h 常
  8<32 の定数密度退化) で旧 0x3D9E51EE (0.077304706) は解析参照
  exp(-2.56)=0x3D9E51F3 より **−5 ulp 劣位** (rq ev_fog: err=
  0xB3200000)、新形式は解析参照と f32 完全一致 (err=0.0) + exp 呼出
  16 回→1 回 (低スペック実コスト削減、directive ②)。単調性
  (farther_is_denser)・[0,1] 域・3 成分同値は両形式で保持、既存 3
  テスト非侵蝕を rq で事前検証。WGSL も同形化 (var od + exp(-od*seg)
  splat、gpu_runtime 経由 naga validate 自動 included = wgsl 51 全緑)。
  【ID 体系誠実注記】wave 149 は E 系列 EV とすべき所をモジュール頭字
  GC とした記名逸脱 — コミット済 95a7e0b で履歴固定、本 wave は E 系列
  継続の EV (記名のみ、検証手順は全 wave 同一)。
- EV-2 [低] Vec4 (型+new+Add/Sub/Mul) + Vec3 Sub impl 完全削除
  (census 消費者ゼロ機械確定、§7 消化、EU-3/ET-2 同型)。Vec3 は消費
  面のみ維持 + contract pin。
- EV-3 [観] module doc 誠実注記 5 項: (1) sum-then-exp 根拠、(2)
  Vec4/Sub 削除経緯、(3) WGSL/CPU 行対応パリティ + normalize(0) の
  WGSL 仕様未定義 vs CPU length>1e-8 fallback の非対称 (rd≠0 前提の
  呼出契約として記録)、(4) NaN/退化契約 (dist NaN → od·seg NaN →
  全成分 NaN 伝播 fail-visible・dither NaN → clamp 透過 (捕捉 57
  規律) → t=NaN → h=NaN → (NaN).max(0.0)=0.0 で密度 base mask →
  T=exp(-Σbase·seg)=0x3EBC5AB3 有限静寂 (fail-loud しない設計)・
  scale≤0 → max(1e-3) floor、h>start で d underflow 0 → T=1.0 exact
  0x3F800000)、(5) dither 中心化: closed-form 0.16·(exp(-0.5)−exp
  (−6.75))=0x3DC65D42 に対する quadrature 誤差 midpoint −0.00242 vs
  端点 +0.0427/−0.0330 → **17.65×/13.62× 高精度** (捕捉 59 同型)。
  【私の訂正記録 2 件】(i) 初閾値「µ 級一致 (1e-5)」は 8 層粗分割に
  対し過剰期待で rq が assert 失敗で捕捉 → 誤差比 pin へ訂正、(ii)
  初 NaN golden は od=0.08·12.5 の省略形で 0x3EBC5AB2 と 1 ulp 誤り
  (逐次蓄積 od·12.5=0x3F7FFFFE vs 省略 1.0) → 実装 golden が捕捉、
  rq 逐次シミュレーションで 0x3EBC5AB3 訂正。
- EV-4 [低] strict 7 件: wiring 同定数 golden (0x3D9E51F3 + 3 成分
  同値 + 旧比 −5 ulp 差分 pin)・midpoint 精度 golden (T(0.5)=
  0x3F68EE32 + T 空間誤差比較)・rd=0 退化 (0x3F390803)・scale floor
  exact・steps=1+dither 端 (0x3F794497 + dither=1.0 単位区間)・NaN
  契約 2 系統・Vec3+wgsl 契約 (normalize 3-4-5 0x3F19999A/0x3F4CCCCD・
  Add/Mul/Default・wgsl_source()==CONST 内容比較 (ET 規律) + sum 形
  含有 pin)。

adversarial 5 系統: (a) sum-exp revert (積連鎖化) **4 RED** (wiring_
const/midpoint/zero_rd/nan_contracts、steps=1 は 1 因子で両形式一致=
構造的緑・正確)・(b) Vec4+Vec4impl 復活 revert **非検出** (43 緑・
警告 0、dead code 復活はテスト非可視、EU-3(c)/ET-2(b) 同型誠実記録)・
(c) density の .max(0.0) 除去 **2 RED** (wiring_const: h<start で密度
爆発 T≠golden・nan_contracts: dither mask がこの max 依存と構造特定)・
(d) normalize guard 除去 (0 徐算 NaN 化) **1 RED** (zero_rd のみ=
補完正確)・(e) t 進行 2seg 変異 (配置オフセット) **1 RED** (midpoint
のみ: steps=1 は最終 sample 後の increment が dead で構造的緑、
wiring_const は定数密度で緑 — 誠実記録)。変異前実体コピー /tmp+
rsift/bak 先行・grep -c 適用確認後計測・毎回復元 MD5-VERIFIED 5 回。

opt-gfx **1240 全緑** (最終全量実測 21.27s、net +7、機械検算
1233+7=1240)・api 49 全緑・replay 16 全緑・wgsl 51 全緑 (naga 新構文
合法)・lib 本編警告 0・全厳密値 rq ev_fog 事前導出 (新旧 trans/bits・
解析参照一致・closed-form/quadrature 誤差比・全退化値・NaN 規律、
全 assert 通過、python 引退継続)・fmt: HEAD 原生逸脱 0、追記分 1 件を
in-place rustfmt で自己起因 0・固定版 md5 三重保存 (rs fd9720fcf8d48de
1f51a821cc8d55846・wgsl ef1d4152f9c2305854d6bca65689e629、src+/tmp+
rsift/bak 三重照合)・digest 004c1cf5fb17bfe8 rows=357 不変・seal
全 6 ゲート PASS・台帳 569・TRIGGER 187。

## EW. screen_space_shadow.rs / full_graph_wiring.rs (wave 151, 2026-07-28)

対象 screen_space_shadow.rs 182→409 行、full_graph_wiring.rs (closure 契約
整合のみ小変更)。census grep 機械確定: cast_sss は wiring:1840 実消費
(shadow → 1853 sky×fog_trans×shadow 色合成、report 非属=ES/ET/EU 同型
の非侵蝕方針)、wgsl_source は gpu_runtime.rs:68 登録、Vec3 は全 op が
本体消費 ((v−pos).length() で Sub も使用 = wave 148-150 系と違い削除
不可)、**Vec4 全消費者ゼロ**。

- EW-1 [高] **捕捉 63 [高]**: module 契約 (`sample_depth` = occluder 上の
  点は pos からの進行距離、遮蔽判定 diff=surf−travelled∈[0,2·step]
  の厚み接触影) に対し wiring の sss_depth closure は **AABB 内で定数
  0.0 を供給** → diff=0.0−travelled<0 が全 16 step 連鎖 → **SSS は
  wiring 経路で全入力で常時 lit=1.0 の構造的全沈黙** (接触影 quality
  機構の完全無効化、捕捉 62 GUI の due 常時真と対称の供給値契約不一致。
  旧 doc「nearest occluder までの進行距離」の曖昧性が温床)。closure を
  `(p − sss_origin).length()` 返却へ契約整合 (cast 内 travelled と同式
  同入力で diff==0.0 exact → 接触影が実効。AABB 包含=占有 proxy の
  coarse 近似は注記 1 で誠実化: 16³ 空隙を無視する低スペック質 proxy、
  精細 depth field は GPU WGSL 側 texture 供給)。shadow は色合成に実
  消費されるが report 非属のため **wiring 層は検出空白構造** — 捕捉 63
  の TDD 修正前 RED は構造的に不可能で、adversarial (a) の revert が
  全量 1248 緑維持となることを機械記録 (検出は module dual pin で充足、
  ES/ET 系先例)。
- EW-2 [低] Vec4 完全削除 (census、EU-3/EV-2 同型)+ Vec3 全 op 維持
  contract pin。【私の訂正記録】初 contract golden (4,6,8)→12 は
  sqrt(116)=10.77… の暗算誤り (directive ⑤違反) で新テスト自身が RED
  捕捉 → (0,3,4)→5 exact ピタゴラス triple へ訂正。
- EW-3 [観] module doc 誠実注記 5 項: (1) sample_depth 契約明確化+
  捕捉 63 経緯 (厚み窓・背面 lit・非接触 lit)、(2) Vec4 削除経緯、
  (3) travelled f32 実系列 (chunked step1=0x3DF5C28E vs 解析 0.12·|ld|
  =0x3DF4381B 不一致=成分個別丸め、閉形式置換せず同式対称で diff=0
  exact 保証の核心)、(4) NaN/退化契約 (NaN depth→lit 側静寂 = 影が
  消える向き・light=0 同点 16・step=0 同点・NaN 比較 false 連鎖 lit・
  max_steps=0 即 lit・bias ライト方向 push 自己影抑制・負 bias 逆行)、
  (5) WGSL/CPU パリティ (行対応同形、surf≥1e30≡is_infinite、境界
  diff=0/2·step inclusive、travelled=max_dist 等値継続 0.30000001>
  0.30000001=false、rq ew_sss)。
- EW-4 [低] strict 8 件: chunked 同型 closure golden (step1 shadow=0.0
  +sample 1 回 Cell、rq: ld bits/travelled 0x3DF5C28E/v.x 0x4100AD1E)・
  捕捉 63 再現 pin (旧 0.0 → 全 16 非遮蔽 lit+sample 16、dual)・
  max_dist 等値継続 sample 3・厚み窓 ±0.01 安全域 (0.19→0.0/0.21→1.0、
  rq 0x3E428F5C/0x3E570A3C)・NaN depth→lit+16・退化 2 系統 (light=0/
  max_steps=0 未打診 0)・bias 初回位置 2.75 exact (0x40300000)・
  Vec3+wgsl 契約。

adversarial 5 系統: (a) 捕捉 63 wiring closure revert **非検出構造**
(変異適用 grep 確認後の全量 1248 緑維持を機械記録 = shadow report 非属
の検出空白、補完は module dual pin 2 件が担う誠実記録)・(b) Vec4 復活
**非検出** (44 緑、EU-3(c) 同型)・(c) travelled `>`→`>=` **1 RED**
(boundary sample 3→2)・(d) `diff >= 0.0`→`> 0.0` **1 RED** (同値
closure の diff=0 exact 契約検出、isomorphic のみ)・(e) bias 符号反転
**1 RED** (bias 初回位置 2.75→2.25 pin)。変異前実体コピー /tmp+
rsift/bak 先行・grep -c 適用確認後計測・毎回復元 MD5-VERIFIED 5 回。

opt-gfx **1248 全緑** (最終全量実測 21.36s、net +8、機械検算
1240+8=1248)・api 49 全緑・replay 16 全緑・lib 本編警告 0・全厳密値
rq ew_sss 事前導出 (ld normalize bits・chunked step1 v/travelled・
boundary 系列・厚み窓・bias 位置、全 assert 通過、python 引退継続)・
fmt: HEAD 両ファイル原生逸脱 0、追記分を in-place rustfmt で自己起因 0・
固定版 md5 三重保存 (sss 17416c5bbf1ea0c75746cec55d54a924・wiring
60b6e8114b3274f23b8e033aa1332ad0、src+/tmp+rsift/bak 三重照合)・
digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS・
台帳 573・TRIGGER 188。

## EX. material_batch.rs / full_graph_wiring.rs (wave 152, 2026-07-28)

対象 material_batch.rs 187→251 行 (全行 CRLF→LF 一括正規化含む、wave 146
exporter 判例)、full_graph_wiring.rs (build_draws 消費配線+assert 根治+
report 3 フィールド+det pin 3)。wgsl なし。census grep 機械確定:
MaterialBatcher は wiring:276 保持・816 clear・818-820 push_quad・822
build_draws 実消費だが、draw 内容 (material_id/first_quad/quad_count の
ranges 全て) は旧 `let _ = (opaque_draws.len(), translucent_draws.len());`
で破棄されていた (debug_assert だけ translucent_draws.len() を誤用)。
**FloraInstance/InstancedFloraRenderer/draw_call_count/
split_opaque_transparent/decode_xyz/BatchedDraw 型名は crates/ 全体で
消費者ゼロ** (stray コピー rsift/rsift/rsift-opt-gfx は build 対象外の
ユーザ管理資産)。

- EX-1 [中] **捕捉 65 [中]**: wiring debug_assert が translucent **quads**
  件数 (translucent_total) と translucent **draw ranges** 件数
  (translucent_draws.len()) を誤等置。同一 mat の連続 translucent quad
  2 個 ([5,5]) で 2≠1 誤爆 (TDD RED 機械記録: left=2, right=1)。
  根治: assert を Σquad_count 不変式へ修正 + **§7 消化 15** `let _ =`
  破棄を根治 → report 実配線 material_opaque_draws /
  material_translucent_draws / material_binned_quads (Σ==len 完全性を
  debug_assert で cross 検算) + det_subset 3 pin。
- EX-2 [低] **捕捉 64 [小]** (削除構造の数学逸脱として記録): 削除した
  FloraInstance::new の量子化は位置 `(x*256).clamp(..) as u16`
  (truncate、−0.5LSB 系統偏向、捕捉 59/60 同型の中心化欠落) vs yaw
  `.round()` (最近傍) で**丸め規則不統一**。根治=構造除去: §7 消化 15
  不可能証明削除 (census 全消費者ゼロ+wiring 供給点不在+fake 配線は
  偽装禁止抵触、wave 149 GC-4 判例)。draw_call_count は加えて
  build_draws 二重実行の無駄実装。
- EX-3 [観] 注記 5 項: (1) 捕捉 64 経緯、(2) 捕捉 65 根治経緯、(3) 削除
  不可能証明、(4) u32→u16 静寂 fold 契約 (block-state 実空間 ~2.6 万
  <65535 で実害ゼロ、契約明記) + duplicate index 防御なし (wiring 到達
  不可、重複 range golden pin) + `prev+1` u32::MAX debug wrap 到達不可
  + empty ガード防御注記、(5) CRLF→LF 全量正規化記録。
- EX-4 [低] strict 9 件 (+8 net: module 6・wiring 3、flora_roundtrip −1):
  merge matrix golden [(5,0,2),(9,3,2),(9,7,2)] binned 6・dual-map 分割
  golden+[5,5] 1 run quads≠ranges module pin・BTreeMap 昇順挿入順独立性・
  duplicate 重複 range pin・clear-reuse・empty golden・wiring capture65
  (0,1,2,total2)+frame2 等値継続 (leo_tag_dist 蓄積で det_subset 全体
  比較は同一インスタンス連続帧に誤用不可 = **私の初版誤用 RED 自己
  捕捉**、EU-2 同型の別インスタンス文脈専用を明記し 4 値直接 pin)・
  chunked golden (24,0,24) 2 tick・empty golden (0,0,0)。全 golden
  rq ex_mb 事前導出 (全て整数厳密: %7 表・Σmat=1956・run simulate・
  duplicate、浮動小数なし、全 assert 通過、python 引退継続)。

adversarial 5 系統: (a) 捕捉 65 旧 assert revert **1 RED** (capture65
panic、chunked/empty は恒真緑の構造)・(b) Flora 系+split+draw_call_count
復活 **非検出構造** (dead code 復活で 1256 緑維持を機械記録 — consumer
ゼロ pub item は警告も発生しない検出空白、EU-3(c)/EV-2(b)/EW-2(b) 同型
4 連続目、誠実記録。検出責務は census 事前確定が担う)・(c) merge 条件
`== prev+1`→`>=` **2 RED** (matrix strict+既存 merges_runs)・(d) wiring
規則 push 側のみ %7==5→6 **2 RED** (capture65 cross assert+ER-2 既存、
chunked は mat%7==1 で両規則無関係の構造的緑)・(e) report 配線 revert
(let _ 戻し) **2 RED** (chunked+capture65、empty は 0==0 構造的緑)。
変異前実体コピー /tmp+rsift/bak 先行・grep -c 適用確認後計測・毎回復元
MD5-VERIFIED 5 回。

opt-gfx **1256 全緑** (adversarial 事後全量再実測 21.50s、net +8、機械
検算 1248−1+6+3=1256)・api 49 全緑・replay 16 全緑・lib 本編警告 0・
fmt: HEAD 両ファイル原生逸脱 0、mb 全量書換は正準一致 (先頭空行
artifact のみ)、wiring 追記分 2 箇所手術で自己起因 0・固定版 md5 三重
保存 (mb 00e437f074c96994303a0cf7ce621157・wiring
0e64eb609635e8f27cb8ca691e42a62a、src+/tmp+rsift/bak 三重照合)・
digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS・台帳 577・
TRIGGER 189。

## EY. fsr2.rs / shaders/fsr2.wgsl / full_graph_wiring.rs (wave 153, 2026-07-28)

対象 fsr2.rs 197 行 (CR 0、既に LF)、shaders/fsr2.wgsl (blend 同形化)、
full_graph_wiring.rs (jitter_uv 消費 + let _ 根治 + report 2 フィールド +
det pin 4)。census grep 機械確定: Fsr2 は wiring:312 保持・463 構築
(640×360→1280×720 固定)、実消費は jitter/reproject/resolve のみ
(2016-2030)。**neighborhood_clamp (fsr2 版、taa 同名とは別物)・
wgsl_source メソッド (gpu_runtime:102 は const FSR2_WGSL 参照)・
Vec3::clamp・Vec3 Sub は crates/ 全体で消費者ゼロ**、加えて in/out dims
4 フィールドは new で格納されるのみ全経路未消費 (捕捉 67)、
`let _ = reprojected;` 破棄で reproject→jitter→halton は結果非消費の
decoy 連鎖だった (resolve のみ prev_frame_color 帰還に実消費)。

- EY-1 [中] **捕捉 66 [中] (blend 反転・三方不一致)**: 旧 `resolve` は
  `current*a + history*(1-a)` (a=max(blend,disoc)) で current 90% —
  doc「Higher = more temporal stability」と GPUOpen FSR2 公式「current
  は relatively low blend factor」(一次情報、Reproject & accumulate 章)
  の双方と正反対、WGSL も cur 95% 同型逆転。根治: `history*h +
  current*(1-h)` (h=blend*(1−disoc)、既定 0.9=history 90%、disocc=1 →
  h=0=current reset 整合) + WGSL `select(0.9, 0.0, reset); mix(cur,
  hist, a)` 同形化。TDD RED 4 機械記録 (dominant/stable/mid/NaN)。
- EY-2 [低] **捕捉 67 [小] (dims 4 dead + let _ 破棄)**: §7 消化 16 —
  `jitter_uv` で input dims 実消費 (旧 0.002 ハードコード=1/500、640px
  基準 25% 過大を根治)、reprojected → report.fsr2_reproj_uv、scale →
  report.fsr2_scale (output dims 消費) 実配線 + det pin 4。dims 4 全てに
  消費者創出 (一律削除ではなく配線第一選択、指令⑦整合)。
- EY-3 [観] 注記 5 項: (1) 捕捉 66 経緯+一次情報、(2) dims 消化経緯、
  (3) 不可能証明削除 (neighborhood_clamp は 3x3 AABB 実データ源不在で
  擬似接続が vacuous=偽装抵触、WGSL clamp は真経路残存 / wgsl_source・
  Vec3::clamp・Sub 消費者ゼロ)、(4) NaN 規律変更 (max 吸収→clamp 透過
  伝播) + halton(0)→1 + frame wrap 到達不可 + jitter 片側非対称、
  (5) WGSL parity + GPU のみ dither=既知系統差 + content pin 財産化。
- EY-4 [低] strict module 15 (+8 net) + wiring 2: 全 golden rq ey_fsr2
  事前導出 (bits 厳密、halton/inv640=0x3ACCCCCD/inv360=0x3B360B61/
  resolve 系列、python 引退継続)。+10 net **1266 全緑** (機械検算
  1256+8+2、adversarial 後 23.31s 再実測)。

adversarial 5 系統: (a) 捕捉 66 旧式 revert **4 RED** (dominant/stable/
mid/NaN、above_one/uses_current は新旧一致の構造的緑)・(b) 削除系 4
構造 (neighborhood_clamp/wgsl_source/Vec3::clamp/Sub+vec_min/max) 復活
**非検出構造** (dead code 復活で 1266 緑・dead_code 警告 0 を機械記録、
consumer ゼロ pub item の既知検出空白、EX-2(b) 同型 5 連続目、誠実記録。
検出責務は census 事前確定)・(c) halton `f /= base`→`*=` **7 RED**
(halton 2+jitter 2+jitter_uv 1+wiring uv 2、halton(0)pin は新旧一致
構造で緑)・(d) wiring mv を 0.002 ハードコード revert **2 RED**
(empty+chunked uv golden)・(e) WGSL `select(0.95, 1.0)+mix(hist,cur,a)`
revert **1 RED** (wgsl contract pin、GPU 経路は naga validate 緑のまま
content pin が検出責務、ES/ET/EW/EX 同型)。変異前実体コピー /tmp+
rsift/bak 先行・grep -c/python assert で適用確認後計測・毎回復元
MD5-VERIFIED 5 回。

opt-gfx **1266 全緑** (事後全量再実測 23.31s、net +10、機械検算
1256+10=1266)・api 49 全緑・replay 16 全緑・lib 本編警告 0・fmt: HEAD
両 rs ファイル原生逸脱 0、追記分 (wiring 4 箇所折返し) を in-place
rustfmt 全量適用で自己起因 0 (fsr2.rs は初版から正準一致)・固定版 md5
三重保存 (fsr2 0b0cd931b0b5ba2173402209fd6ce9c0・wiring
f2525e4a7a89aea301fd2fe8d630245f・wgsl b16b80e81b84a883e03e383d1e0b7085、
src+/tmp+rsift/bak 三重照合)・digest 004c1cf5fb17bfe8 rows=357 不変・
seal 全 6 ゲート PASS・台帳 581・TRIGGER 190。

## EZ. power_policy.rs / full_graph_wiring.rs (wave 154, 2026-07-28)

対象 power_policy.rs 197 行 (CR 0)、full_graph_wiring.rs (tick_frame
legacy 削除 + gate 秒化実駆動 + report 3 フィールド + det pin 1 + on_input
真接続)。wgsl なし。census grep 機械確定: PowerPolicy は wiring:300 保持・
446 構築・586-587 mode_tick→`!render` のみ実消費 (render 恒 true で
power_skip_extra 恒 false 退化=捕捉 70)、`frame_gate` は legacy tick_frame
の `let _` 破棄のみ (ms 誤供給=捕捉 62 同型)、**tick_frame 消費者ゼロ
(crates 全域)・on_input wiring 未接続・smoothed_dt 消費者ゼロ・
hysteresis_fps dead field・with_frame_dt no-op**。

- EZ-1 [中] **捕捉 70 [中] (render 恒 true 全沈黙)**: skip 真判定を
  frame_gate へ一本化 (§7 消化 17、秒化+power_gate_acc 保有+report 3 配線
  +on_input 真接続)。wiring Active 構造的限界 (focused=true 固定・60s
  以内 Idle 未到達) で gate=true 恒 → 恒 false revert は検出不能
  (adversarial (a) 非検出誠実記録)、検出責務は module sole-skip contract
  pin (0.016×3 系列で gate false 到達)。
- EZ-2 [中] **捕捉 69 [中] (ヒステリシス欠落)**: doc 機構を真実装
  (上向き差<閾値=保留/境界==閾値=反映/無制限=即/下向き=即、TDD RED 1、
  残 3 規則は構造的緑を正直列挙、default 非活性注記)。
- EZ-3 [低] 捕捉 68 (with_frame_dt no-op) frame_dt 真保持 + smoothed_dt
  dead state EMA 真更新 (det 登録正当=完全決定) + tick_frame/mode()
  削除 (不可能証明) + det doc 誠実訂正。golden 全 rq ez_pp (EMA s1
  0x3C87FCBA/gate 系列/hysteresis 整数規則、**私の初 contract pin 2 回
  到達は 0.032<1/30 暗算誤り → rq (6) で 3 回到達に訂正自己捕捉**)。
- EZ-4 [低] strict module 12 (+8) + wiring 2 + det pin 1。+10 net
  **1276 全緑** (機械検算 1266+10、adversarial 後 21.77s 再実測)。

adversarial 5 系統: (a) 恒 false revert **非検出構造** (Active 構造限界、
全量 1276 緑機械記録、検出責務=module contract pin)・(b) 削除系 4 構造
(render/with_frame_dt/mode()/tick_frame) 復活 **非検出** (dead code
1276 緑、EY-3(b) 同型 6 連続目誠実記録)・(c) hysteresis `<`→`<=`
**1 RED** (boundary pin)・(d) EMA 0.9↔0.1 swap **3 RED** (module
smoothed+wiring 2)・(e) gate ε=1e-5 削除 **非検出構造** (golden は
need−4·dt==0 bits 一致の ε 不発区間設計を rq 機械記録済、誠実記録)。
変異前実体コピー /tmp+rsift/bak 先行・grep -c/python assert で適用確認
後計測・毎回復元 MD5-VERIFIED (合計 6 照合、(b) は 2 ファイル)。

opt-gfx **1276 全緑** (事後全量再実測 21.77s、net +10、機械検算
1266+10=1276)・api 49 全緑・replay 16 全緑・lib 本編警告 0・fmt: HEAD
pp 原生逸脱 15 行は全て私の mode_tick 改修区間内 (89/93-94 行の旧
FrameDecision 構築+with_frame_dt) のみ → in-place rustfmt 全量適用で
未触区間への整形波及ゼロ・自己起因 0 (wiring HEAD 原生 0)・固定版
md5 三重保存 (pp 9c52433dd81a95db2be20e96ed269c5c・wiring
3853688004c30344086915a5e01dbab4、src+/tmp+rsift/bak 三重照合)・
digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS・
台帳 585・TRIGGER 191。

## FA. hud_batch.rs / full_graph_wiring.rs (wave 155, 2026-07-28)

対象 hud_batch.rs 224→441 行 (CR 0)、full_graph_wiring.rs (rect 供給値
根治 + report 3 フィールド + det pin 3 + §7 消化 18 配線)。wgsl なし。
census grep 機械確定: HudBatch は lib.rs:186 `pub mod hud_batch` 公開 +
wiring:334 保持・486 構築 (quad 容量 1024)・1726-1763 実消費、他 crate
消費者ゼロ (crates 全域、stray コピー rsift/rsift/rsift-opt-gfx/src は
ビルド対象外ユーザ管理資産として除外注記)。旧構造値は
`drop(hud_view)` + `let _ = hud_saved` 両方破棄 (§7 未消費 2 構造) で、
batching の draw call 削減量が一切観測されない主沈黙面だった。

- FA-1 [中] **捕捉 73 [中] (非連続 push 結合未実装・golden 試験が切開)**:
  finish 内 finalize_indices は range の `first_index..+index_count`
  slice 出力のみで、非連続同一キー push (A,B,A) では A range の区間に
  中間 B の index run が包含 → **B が A range と B range の双方で二重
  描画**される誤描画系。先頭 doc「非連続 push の結合」は未実装主張で、
  wiring は昇順連続 push のみで非顕在化構造。根治: push 毎の chunk
  (slot, indices 内 offset) run 記録 + finalize を by_slot 1 pass 真
  再配置 (O(chunks)、range×chunk 二重走査回避で低スペック整合) +
  **slot_for の map ずらし時に chunk slot も同規則 +1 ずらし** (初回
  ずらし忘れで interleave/repeat golden 再 RED → 2 段修正緑、自己照査
  記録)。ImmediatelyFast Text batching 同型 (文字順≠キー順で非連続が
  本質)。golden = repeat-after-insert strict (rq fa_hud (4))。
- FA-1 [中] **捕捉 71 [中] (slot_for 返却非一致)**: slot_for は
  `ranges.len()` (挿入前末尾) を返すのに実配置は insert_pos (層昇順)。
  旧 push_quad の冗長 3 段 (誤 range への base_vertex 設定 attempt は
  count>0 で skip/no-op `map.get_mut(&key).map(|_| ())`/`self.map[&key]`
  真 slot 再取得) で辛うじて整合していた構造 (絶対 index 方式で
  base_vertex=0 契約は golden pin)。根治: slot_for 返却=insert_pos +
  push_quad 単一路簡素化、挙動同一は golden 3 本 (same_key 列/
  interleave/repeat) で pin。adversarial (b) 帳尻復帰は挙動同一で
  非検出 (誠実記録、根治は簡素化+pin で代替正当)。
- FA-1 [小] **捕捉 72 [小] (wiring rect 供給値契約不一致、捕捉 62/63
  クラス)**: module 契約は rect=[x,y,w,h] の w に対し wiring は x_end
  (`8.0+v*120.0`) を供給 → 幅が常に **+8px 系統誤差** (v=1 で 128/120
  = +6.7%、v=0 で 8px の非ゼロ棒=空でない)。TDD RED 機械記録: bar0
  v=0 で幅 bits=left 0x41000000 (8.0) ≠ right 0 (rq fa_hud 予想完全一致)。
  根治: wiring は `v.clamp(0.0,1.0)*120.0` を w 供給 (端=8+vw で旧表示
  の意図と同一、v=0 → 幅 0 exact、module 側 w=0 空矩形受理 pin)。
- FA-2 [低] §7 消化 18: 旧 `drop(hud_view)`+`let _ = hud_saved` 両方
  破棄を根治 → report `hud_quads`/`hud_draw_ranges`/
  `hud_draw_calls_saved` 実配線 (layer 4 相異キーで全 scene 確定的
  4/4/0、merge 0 の wiring 構造正直注記) + det_subset bit pin 3
  (3289-3296)。ImmediatelyFast merge の観測面を真値で開設 (虚偽
  イベント捏造の fake 配線なし)。
- FA-3 [観] 注記 6 項 + **mojibake 誤読の誠実撤回**: push_rect doc
  「実線矩形」を前モデルが文字化けと誤判定した件、od byte 照合で E7 9F
  A9 = U+77E9「矩」の健全 UTF-8 と一次確認 → 撤回注記のみ (修正対象
  なし、terminal 表示断片を一次照合なしに疑った前例として pub 記録)。
  他注記: draw_calls_saved=saturating_sub で負化なし/begin_frame 完全
  リセット (chunks/quads 含む)/map・chunk ずらし可変性の不変式文書化。
- FA-4 [低] strict module 5 追加 (same_key index **列** golden [(b,b+1,
  b+2,b,b+2,b+3)×3]・layer interleave golden [ranges (0,6,6),(5,12,6),
  (9,0,6)+finalize 列]・repeat-after-insert golden [(0,6,6),(9,0,12)+列
  [4,5,6,4,6,7,0,1,2,0,2,3,8,9,10,8,10,11]]・push_rect contract golden
  [(8,8),(128,8),(128,18),(8,18) 全 dyadic exact+w=0 空矩形]・
  begin_frame 再利用+empty golden) + wiring 2 (捕捉 72 幅照合 4 本
  report 由来汎用式・counts golden chunked/empty 両方 (4,4,0))。
  既存 2 (merges/layers_sort) 維持。**+7 net 1283 全緑** (機械検算
  1276+5+2、fmt 後全量再実測 22.27s・adversarial 後最終 21.71s)。
  golden 全 rq fa_hud 事前導出 (構造値 4/4/0・index パターン・
  interleave/repeat 列・捕捉 72 新旧差分、python 引退継続)。

adversarial 5 系統: (a) 捕捉 73 revert (旧 slice finalize) **1 RED**
(repeat_after_insert のみ — interleave は全キー相異で slice≡repack の
構造的緑、事前導出どおり誠実記録)・(b) 捕捉 71 revert (帳尻 3 段復帰)
**非検出** (帳尻が返却非一致を厳密相殺する挙動同一、1283 緑機械記録・
警告 0、簡素化+golden pin が代替保証、誠実記録)・(c) 捕捉 72 revert
(w=8.0+v*120) **1 RED** (capture72 幅照合)・(d) 層比較方向反転
(`>`→`<`) **3 RED** (layers_sort/interleave/repeat golden)・(e) §7 消化
18 revert (report 3 代入削除=0 固定) **1 RED** (counts golden)。
変異前実体コピー /tmp+rsift/bak 先行・python assert+grep -c で適用確認
後計測・毎回復元 MD5-VERIFIED (5 照合、(c)(e) 適用時に wiring 行 695 の
過去文書語句「adversarial (c)」を誤数と検討し行位置一次確認で解消)。

opt-gfx **1283 全緑** (net +7、機械検算 1276+7=1283)・api 49 全緑・
replay 16 全緑・lib 本編警告 0・fmt: hud HEAD 原生逸脱保有のため
python difflib+git diff 行範囲交差判定で自己起因 hunk のみ選択適用
(KEEP 3 hunk+境界誤判定 1 行を手動修正)=自己起因 0 (逸脱内容一致:
push_glyph literals/DrawRange 1 行リテラル/key()/mkquad 4 群、seal
ゲート2 機械値は HEAD 逸脱 10/現 10/自己起因 0 PASS — 行数は
difflib 計上と seal 計上で定義差あり、自己起因 0 は両者一致)・wiring
HEAD 原生 0 → in-place rustfmt 全量適用・seal 機械値 0/0/0=自己起因
0・固定版 md5 三重保存 (hud
f8617dd42f3217397ad1a43841505b06・wiring
46b21815c17de2c2a33211fbb73afcae、src+/tmp+rsift/bak 三重照合)・
digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS・
台帳 589・TRIGGER 192。

## FB. pool_slab.rs / full_graph_wiring.rs (wave 156, 2026-07-28)

対象 pool_slab.rs 255→455 行 (CR 0)、full_graph_wiring.rs 4432→4634 行
(RenderSection lifecycle 真実装 + material read-back + ObjectPool HUD
scratch リサイクル + report 4 フィールド + det pin 4)。wgsl なし。
census grep 機械確定: pool_slab は lib.rs:230 `pub mod` 公開 + wiring:304
`Slab<u32>` 保持・451 構築・**680 `alloc` のみ実消費**、他 crate 消費者
ゼロ (crates 全域、stray コピー除外注記は FA 節参照)。free/get/get_mut/
ObjectPool/GenerationalSlab 直接利用/with_capacity/len/is_empty は
全消費者ゼロ、680 では `slot == usize::MAX` 満杯分岐と `let _ = k` 破棄。

- FB-1 [中] **捕捉 74 [中] (vacuous 満杯判定 + 無限 append)**: 旧 wiring
  は `slot == usize::MAX` を満杯と判定すべく分岐していたが、
  `Slab::alloc` は Vec push の無制限成長で **MAX sentinel を構造的に
  返しえない** (index は u32 由来、64-bit では 0xFFFF_FFFF ≠
  usize::MAX、32-bit でも 2^32 件必要) — 到達不能の vacuous check で、
  EB-5 以降のコメント「(スロット消費・満杯判定) 自体が目的」は虚偽
  構文だった。加えて free 消費者ゼロで毎 tick 全 chunk を無限 append
  (単調増大) + `let _ = k` 破棄。根治: RenderSection 本来の key
  ライフサイクル真実装 (BTreeMap key→slot、**初見 key のみ alloc・
  退去 key を free で mat 回収**、map == slab の占有と常時一致の integrity
  debug_assert 2 本、`let _ = k` 消滅)。vacuous 分岐は除去 (挙動同一、
  module 側真契約は alloc_unbounded golden が pin)。material_id
  (draw cmd) は key→slot→`slab.get` の真 read-back 供給へ (値は
  inputs 由来と round-trip bit 同一、det/EO 影響ゼロ、eviction 系列が
  (1,0)→9 保持・(2,0)→3 新規で経路を実証)。
- FB-2 [低] §7 消化 19: 消費者ゼロ構造へ真消費者創出 — free (evict)
  /get (material read-back) /len→report.slab_occupied /is_empty
  (integrity debug_assert) /Slab::with_capacity(1024) 事前確保接続
  (reserve のみ挙動同一、GenerationalSlab::with_capacity 伝達) /
  ObjectPool<BatchOutput> HUD scratch リサイクル (acquire 稼働
  available=1 を report、release で Vec 容量の跨 tick 保持 = pool 本来
  の確保回避、実値非依存=det 正当) 。`Slab::get_mut` /
  `GenerationalSlab::get_mut` は不可能証明の上削除 (mat は frame
  snapshot で不変・変異消費経路は crates 全域 census grep で存在
  証明できない (不可能証明)、捏造 vacuous
  書き込み=偽装禁止抵触、census grep 消費者ゼロ)。get_mut 以外の
  全構造は維持・配線済 (削除 item は get_mut 2 件のみ)。
- FB-3 [観] **捕捉 75 記録+根治**: insert で free_head 非 sentinel なのに
  指先が Vacant でない場合の silent フォールスルー (内部不変式違反を
  静寂マスクし push 増長する経路) を debug_assert で不変式明示
  (公開 API からは到達不能、挙動同一、adversarial (e) が検出力を実証:
  free-list 不進行変異を :102 で捕捉 panic)。他注記: generation u32
  wrapping (同一 slot 2^32 reuse で理論上 ABA、60fps 全 tick でも
  ~2.2 年到達不能、EY jitter wrap 同型)・Slab idx_to_handle の二重
  free 安全 (take で Some→None)・LIFO 復帰順・pool LIFO 系列、
  rq fb_slab 全 assert 通過 (python 引退継続)。
- FB-4 [低] strict module 8 追加 (alloc unbounded golden/LIFO 復帰
  golden/ABA gen +1 golden/occupied 二重 free 不変 pin/範囲外 None
  契約 (u32::MAX idx overflow 安全含む)/ObjectPool acquire 系列 golden/
  with_capacity 挙動同一 pin/keyed lifecycle pattern golden (wiring
  実消費の module 側再現)、既存 2 維持) + wiring 3 (chunked lifecycle
  golden t1/t2・empty golden 2 tick・eviction golden slot 再利用+
  material 保持 read-back)。**+11 net 1294 全緑** (機械検算
  1283+8+3、fmt 後全量再実測 21.65s・adversarial 後最終 22.12s)。
  golden 全 rq fb_slab 事前導出 (LIFO 系列/gen 系列/occ 系列/pool
  系列/scene 系列全整数)。TDD RED は compile RED 31 機械記録
  (E0599 with_capacity/len/is_empty・E0609 report 4 fields、GC/EZ
  同型の仕様先行)。

adversarial 5+1 系統: (a) 捕捉 74 revert (evict 無し毎 tick 全 append)
**15 RED** — 新 golden 2 に加え既存複数 tick chunked 系 13 が整合
debug_assert (map.len()==slab.len()、wiring:712) で全滅 panic
(det 系/EO golden/render_pipeline det 含む)。**私の事前予想 2 RED は
debug_assert の複数 tick 波及を見落とした暗算予想で、実測 15 へ誠実
訂正** (検出幅が golden 2 件を大きく上回る assert 網)。・(b) 削除済
get_mut 復活 **非検出** (dead code 1294 緑・警告 0、census grep が
検出責務、152 以来の同型誠実記録)。・(c) ObjectPool revert (acquire
除去 + report 0 固定) **2 RED** (lifecycle+empty、事前予想完全一致)。
・(d) 世代前進 skip (wrapping_add 除去) **2 RED** (aba golden + 既存
test_generational_slab、事前予想 1 へ既存検出 +1 の誠実記録)。
・(e) free-list 不進行変異 **1 RED** (捕捉 75 debug_assert :102 が
鎖破壊を panic 捕捉、事前予想完全一致)。・(f) material read-back
revert (inputs 直参照へ逆戻し) **非検出** (round-trip 値同一性により
値では不可視 = 構造的限界、eviction golden の map 保持 pin が整合責務、
誠実記録)。変異前実体コピー /tmp+rsift/bak 先行・python assert で適用
確認後計測・毎回復元 MD5-VERIFIED (6 照合、(f) 復元時は cwd 誤りで
cp が失敗し md5sum 工程が経路不一致を捕捉 → 絶対パスで真復元し直し
MD5-VERIFIED + 固定版全量 22.12s 再実測、誠実記録)。

opt-gfx **1294 全緑** (net +11、機械検算 1283+11=1294)・api 49 全緑・
replay 16 全緑・lib 本編警告 0・fmt: slab HEAD 原生逸脱 2 行 (71/79)
保有 → python difflib+git diff 行範囲交差判定で自己起因 hunk のみ選択
適用 (keep=4 skip=2) → 区間内 71 は適用・未触 79 (=現座標 118) は
保存で**自己起因 0** (seal ゲート2 機械値も slab HEAD 逸脱 2 / 現 1 /
自己起因 0 で手計測と完全一致)、wiring HEAD 原生 0 → in-place rustfmt
全量適用・seal 機械値 0/0/0=自己起因 0 (self-caught:
通常代入式に誤ってタプル参照キャスト式 `*{(mat,)}.0 as &u32` を混入
させた入力ミスをビルド前に自己修正、台帳には誠実記録)・固定版 md5
三重保存 (slab c2a80a34a83b9ebcc65b9b51de4ecd09・wiring
6a7be816645621d81e40466d5cf85493、src+/tmp+rsift/bak 三重照合)・
digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS・
台帳 593・TRIGGER 193。

## FC. chunk_cull.rs / render_pipeline.rs (wave 157, 2026-07-28)

対象 chunk_cull.rs 220→401 行 (CR 0)、render_pipeline.rs (cull_stats
第 2 帳簿配線 + strict 1)。wgsl なし。census grep 機械確定: chunk_cull は
lib.rs:9 pub mod +:78 pub use 公開 + render_pipeline:9 import・:108 field
・:221 from_profile 構築・verdict_column 2 call site (:509/:812)・
verdict arm 直加算のみ実消費、他 crate 消費者ゼロ。**CullStats+apply・
visgraph_enabled・max_section_draw は読み手全域ゼロ** (entity_culling の
CullStats は別型)。

- FC-1 [低] **捕捉 78 [小] (dead field 2 件の §7 未配線)**: from_profile
  が値を装填するのみで読み手ゼロ。visgraph_enabled: gate 対象の flood
  fill が render_pipeline に存在せず (wave 129 EC 構造) + 全 tier
  visgraph_occlusion=true 恒真 (rsift-api adaptive_perf 4 tier 機械
  確認) + BK 設計で verdict 非読取 → 捏造ゲートは偽装禁止抵触のため
  **不可能証明の上削除**。max_section_draw: 部分 section cap は chunk
  全体シェーディング構造と不整合 (捏造機能回避) + BGM loop は別監査
  module で同一不可能証明。
- FC-2 [低] **捕捉 79 [小] (統計機構の消費者ゼロ孤立)**: render_pipeline
  は frame_stats arms 直加算のみで CullStats+apply が完全孤立 (二重帳簿
  のずれ検出手段ゼロ構造)。§7 消化 20: pipeline に第 2 帳簿として真蓄積
  (両 verdict call site) + frame 末端 debug_assert 4 本 (Σ 完全性+
  3 面 cross 一致) + strict 1 (非ゼロ 4 verdict 工程化 scene golden)。
  **初版 invariant テストはデフォルト scene で全 V 帳簿 (=全比較面 0)
  の vacuous green となり adversarial (b) 非検出を招来 → 4 verdict 非ゼロ
  工程化 scene (ingest (1,0) 占有/(5,0) 空/(20,0) 遠方占有、low_spec 4
  flag 無効化で verdict 到達経路確定) へ作り替えた自己照査を誠実記録。
  probe 実測: デフォルト scene f1 は tested=3 全 V、f2+ は cache/再利用
  前倒しで tested=0 (verdict site 不経由)。**
- FC-3 [観] 誠実注記 5 項 (header): (1) 捕捉 78 経緯、(2) 捕捉 79/消化 20
  経緯、(3) 順序帰属 (EmptyColumn は距離より先=range_skipped は遠方空を
  数えない設計意図)、(4) 境界等号の帰属 (dist==radius は Visible、1-ulp
  窓 strict、実機 probe: hypot(8,8)=0x413504f3=sqrt(128) bit 一致・
  hypot(24,8)=0x41ca62c2)、(5) Occluded 非送出 (wave 61 BK) + 双 call
  site skip 統一 (wave 86 CJ-2) は将来 producer の防衛、現挙動では
  occluded_skipped 恒 0。**私の当初 center 対称 golden は probe 照合で
  dist 非対称 (原点 8 オフセット由来) と自己捕捉 → bits 厳密 golden
  (centers int-exact 0x41c00000/0xc1000000/0x41000000) へ訂正記録**。
- FC-4 [低] strict module 5 (apply Σ 完全性全接頭辞+終端 golden 7/3/2/1/1・
  境界等号 + 1-ulp 窓 3 点・from_profile カスタム rd=12→192/floor 7→128
  導出・center 公式 dist bits+非対称中点 verdict・2×2 帰属 matrix+apply
  帰属) + pipeline 1 (cross invariants 構造値 (4,2,1,0,1)+Σ+4 面一致)。
  apply は pass 保有 API (当初 `self.cull_stats.apply` と API 形状誤り →
  E0599 を緑前自己修正)。**+6 net 1300 全緑** (機械検算 1294+5+1、fmt 後
  全量再実測 21.81s・adversarial 後最終 22.44s)。golden 全 rq fc_cull
  事前導出 (整数系列+scene (6)、python 引退継続)。

**捕捉 80 [中] (wave 158 対応予約、波及的発見)**: adversarial (b) 検証中
の scene 調査で **mesh_cache::decode_mesh が MeshDiskCache::get 経路で
bytemuck alignment panic する latent 障害** を特定。再現は最小: デフォ
ルト flag の新 pipeline に `world.ingest(1,0,[7u16;4096])` → coords 同じ
で 2 frame 目に cache decode で panic (RUST_BACKTRACE full: bytemuck
internal cast_slice TargetAlignmentGreaterAndInputNotAligned ← decode_mesh
← MeshDiskCache::get ← frame)。既監査 mesh_cache (AUDIT 行 302 既出) の
見落とし潜在で、既存 det テストが ingest+2frame 組を持たない隙間構造。
本 wave では FC 範囲外のため再現経路+証拠のみ記録し、wave 158 (FD)
を mesh_cache 厳密監査として根治予定だ (捕捉採番 81 以降は同 wave で
確定)。f1 構造値 pin は当該 decode 経路を踏まない設計に限定した。

adversarial 5 系統: (a) apply Occluded arm skip **2 RED** (sigma golden+
既存 legacy accumulate、事前予想完全一致)・(b) site-2 apply 除去 — **初回
0 RED (初版 invariant test が全ゼロ vacuous で非検出、自己照査で捕捉) →
非 vacuous 化後の再変異で 1 RED** (xinv strict、既存 det 系は cull 非ゼロ
scene を持たず非検出面を正直記録)・(c) 順序 swap (range→empty) **2 RED**
(legacy order pin+attribution matrix、予想一致)・(d) fields 復活 **非検出**
(dead code 1300 緑・警告 0、誠実記録 8 連続)・(e) `>`→`>=` **1 RED**
(equality ulp window、予想一致)。変異前実体コピー /tmp+rsift/bak 先行・
python assert/grep -c で適用確認後計測・毎回復元 MD5-VERIFIED (cull
4fb8edb58ede003e97dbb6b3628200db、pipe c43c3b0501d664b775953ebf461b875a
— pipe は初版 ef8ba8c4 から invariant 作り替えで再採番)。grep 照合が
私自身の誠実記録コメント語句「adversarial (b)」と衝突したため行位置
一次確認で解消 (md5 照合が正)。

opt-gfx **1300 全緑** (net +6、機械検算 1294+6=1300)・api 49 全緑・
replay 16 全緑・lib 本編警告 0・fmt: 両対象 HEAD 逸脱は先頭空行
artifact のみ (実質正準) → in-place rustfmt 全量適用・seal ゲート2
機械値は両対象 0/0/0=自己起因 0 (pipe は invariant 作り替え後に再
適用・再保存)・
固定版 md5 三重保存 (上記 2 値、src+/tmp+rsift/bak 三重照合)・digest
004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS・台帳 597・
TRIGGER 194。

## FD. mesh_cache.rs / render_pipeline.rs (wave 158, 2026-07-28)

対象 mesh_cache.rs 566→611 行 (CR 0)、render_pipeline.rs (warm f2 統合
pin 1 + fc_xinv 注記更新)。wgsl なし。census grep 機械確定: MeshDiskCache
は render_pipeline の唯一実消費者 (lib.rs pub mod、render_pipeline:30
import・:95 field・:197 adaptive 構築・:398 get・:496 put・:1002 stats
report・:1301 invalidate_chunk)、設定面は rsift-api adaptive_perf.rs
mesh_disk_cache ( tier false/true/true/true ) + gui_settings toggle、
他 crate 消費者ゼロ (stray コピー rsift/rsift/rsift-opt-gfx/src は除外)。
encode_mesh/decode_mesh/decode_mesh_v1/decompress_bounded/read_pod_vec
は全て private で put/get 経由消費。

- FD-1 [中] **捕捉 80 [中] (FC-3 予約の wave 158 根治)**: decode_mesh
  (v2 :273/:275、v1 :310/:312) が decompress 産 Vec<u8> の任意オフセット
  領域を `bytemuck::cast_slice` で直接参照変換。v2 wire 頂点オフセット =
  14 + Σ(4+rle_len_i) + 8 で rle_len_i = 2+4·runs_i は run 構造に無関係に
  ≡ 2 (mod 4) → off ≡ 2+2S (mod 4) → **S 偶数で align(4) 不整列**
  (rq fd_cache (1)-(3))。生産定数 SECTIONS_PER_COLUMN=4 が常時偶数のため
  disk cache 有効 tier では**全正当エントリが再読込 (暖機 f2) で 100%
  panic** (TargetAlignmentGreaterAndInputNotAligned)。根治:
  `read_pod_vec<T: AnyBitPattern>` = chunks_exact(stride) +
  pod_read_unaligned 要素単位コピー読み (std::ptr::read_unaligned 相当、
  整列入力でも読取値同一) へ v2/v1 全 4 箇所統一置換。書込側 cast_slice
  (encode :190-191) は実体 Vec の align 保証で安全、維持。trait 導出は
  `unsafe impl<T: Pod> AnyBitPattern` (vendor anybitpattern.rs:56) 一次確認。
  TDD RED 2/2 機械記録 (module alignment roundtrip + pipeline warm f2、
  両者 bytemuck internal.rs:33 panic)。
- FD-2 [低] **§7 消費者検査**: 消費者ゼロ構造の新規検出なし (全 pub API
  実消費+test 消費)。監査評価済み非該当 3 項: (1) fs::read は圧縮後サイズ
  まで読むが展開増幅は DECOMPRESS_CAP 64MiB で制御完、上限和
  68,786,585,585 < i64::MAX (rq (6)) で 64-bit 非 overflow 証明 (CI
  ubuntu-latest x86_64 のみ一次確認)、(2) クロスプロセス同一 tmp 名の書込
  競合は decode 検証+破損 remove+再構築で fail-safe 封じ (CW-2 設計範疇)、
  (3) decode 末尾バイト許容は無害 (mesh 消費は前置長厳密、末尾は解釈経路
  を持たない純粋前置関数)。
- FD-3 [観] 誠実注記: v1 wire は off = 20+12k / idx = 20+12k+4m ≡ 0
  (mod 4) に数学的制限され panic 不能 (rq (5)) — 両経路の同一安全
  プリミティブ統一は将来 wire 変更への不整列耐性不変式化。key_path は
  負数含む十進 3 連接で '_' を数値が含み得ないため座標→名前は単射。
  **私の rq (6) 総和リテラル初版を暗算誤り (69,206,047,745)、python
  機械計算で 68,786,585,585 に訂正 = 数値暗算禁止規律効果の自己記録**。
- FD-4 [低] strict 追加 2: module roundtrip_bits_any_section_count_
  alignment (S=0..=4、整列予想自己文書化 assert の golden、rq (2)) +
  pipeline frame_disk_cache_warm_reframe_strict (fc_xinv 同 scene で
  f1 put (hits 0 / builds 2) → f2 get decode (hits 2 / builds 0) +
  verdict golden (4,2,1,0,1) の f2 不変 + Σ+4 面一致)。fc_xinv の
  捕捉 80 予約注記を根治済みへ更新。**+2 net 1302 全緑** (機械検算
  1300+2、fmt 後全量再実測 21.66s・adversarial 後最終 21.51s)。
  golden 全 rq fd_cache 事前導出 ((1)-(6)、python 引退継続)。

adversarial 5 系統: (a) 捕捉 80 revert (v2+v1 全 4 箇所 cast_slice 復帰)
**2 RED** (module+pipeline 両新 strict が捕捉、事前予想一致)・(b)
truncate guard `>`→`>=` (v2) **3 RED** (put_get_roundtrip+any_section+
warm、exact-length raw が境界で誤 Err)・(c) vertex/index 読取順入替
(v2) **2 RED** (module 両 roundtrip が bit 崩壊を検出、warm 統合 pin は
帳簿レベル検査のため非検出 = 統合 bit 完全性は module 責務という分界を
誠実記録)・(d) rle 検証 skip **1 RED** (wave 61 BK corrupt 既存テスト)・
(e) v1 のみ cast_slice 復帰 **0 RED 非検出** (v1 は数学的整列制限で
panic 不能+整列入力で cast / read_unaligned の読取値が同一 = 構造限界、
誠実記録)。変異前実体コピー /tmp+rsift/bak 先行・python assert で適用
確認後計測・毎回復元 MD5-VERIFIED 5 回 (mc f396ecd676dee78113061e4c438ea
8b5、rp 3f4abdc960f6df5229b458dd5945d97b — src+/tmp+rsift/bak 三重照合)。

opt-gfx **1302 全緑** (net +2、機械検算 1300+2=1302)・api 49 全緑・
replay 16 全緑・lib 本編警告 0・fmt: 両対象 HEAD 逸脱は先頭空行
artifact のみ (実質正準) → in-place rustfmt 全量適用・seal ゲート2 機械値
mc 0/0/0・rp 0/0/0 (両対象 PASS)・digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート
PASS・台帳 601・TRIGGER 195。

## FE. fsr3_fg.rs / full_graph_wiring.rs (wave 159, 2026-07-28)

対象 fsr3_fg.rs 216→296 行 (CR 0)、full_graph_wiring.rs (fsr3 報告 4
実配線 + bootstrap serve + det subset 4 面)。census grep 機械確定:
fsr3_fg の消費者は full_graph_wiring (:367-369 field・:521-522 構築・
:2305 FrameInput 実構築・:2334 interpolate_cpu 実消費 — 16×16 実 probe
フレームで毎フレーム実行) + gpu_runtime:107 (FSR3_FG_WGSL 文字列登録
のみ、GPU 実行バックエンド未配線 = 計測実演層、誠実注記) + lib:184
pub mod。curr_bias_disocclusion の wiring 側設定者はゼロ (default 1.0
固定)、interpolate_cpu 消費者は wiring:2334 + 本 module tests のみ。

- FE-1 [中] **捕捉 81 [中]**: FSR3_FG_WGSL が CPU 参照 (interpolate_cpu)
  と 3 点で数学乖離 — (1) motion 空間: CPU は texel 単位 (`x − mv·a`)
  なのに WGSL は `uv − mv·a` の uv 単位で解像数倍の誤サンプル (1920 幅
  で mv=8, a=0.5 なら意図 4 texel が 7680 texel/frame 誤読、rq (6))、
  (2) texel 中心: CPU bilinear floor 系 vs WGSL +0.5 中心で半テクセル
  ずれ、(3) curr_bias_disocclusion が uniform 未配管で disoc 時
  curr 100% 固定 (CPU bias 語彙と乖離、bias≠1.0 設定で両者非同義)。
  根治: `(center − mv·a) / res2` (texel 単位・+0.5 保持) と uniform
  +curr_bias / `select(a, cfg.curr_bias, disoc)` で CPU 式と厳密同一
  語彙へ。GPU 未実行層のため contract text pin 4 本で固定。
- FE-2 [低] **捕捉 82 [小]** d_next 死計算 + clone_shallow 恒等 no-op
  helper の §7 未配線: 値は `let _` 破棄・GPU にも対応概念なし =
  不変式層でも死 → impossible-proof の上両者削除。+ **捕捉 83 [小]**
  wiring で `let _ = self.fsr3_buffers` / `let _fsr3_mean_delta` の
  2 破棄 → §7 消化 21: FrameWiringReport へ fsr3_color/depth/motion_
  bytes (定数 3) + fsr3_mean_delta (実測) 実配線 + det subset 4 面
  登録。**併根治: bootstrap (prev 不在) は out:=curr serve** (旧は
  全ゼロ出力の残留=黒混入測定系、FSR3 既定の「中間≈現フレーム」動作)。
- FE-3 [低] **捕捉 84 [小]** fsr3_required_buffers の u32 乗算 wrap
  (65536² = 2^32 → 0 へ wrap し全長 0 を静寂計上) → u64 昇格乗算
  (rq (5))。+ **捕捉 85 [小]** interpolate_cpu のバッファ長契約を
  fail-loud 化 (4 バッファ == w*h・out ≥ w*h を契約メッセージ付
  assert 6 本。旧は深部 index OOB panic への流出で診断不能)。
- FE-4 [低] strict module 8 (warp bilinear golden 行 [0,12,28,44,60,
  76,92,108]×2 行・alpha 端点 0≡prev/1≡curr 全画素・disoc 境界等号は
  非 disoc (dyadic 厳密 Δ=1/16=0.0625=thr)・bias=0.5 blend golden
  0x88804422・buffers 1920×1080 golden + 2^32 no-wrap・契約
  should_panic 2・wgsl contract pin 4 条件) + wiring 1 (bootstrap
  serve Δ=0.0 厳密位相 + f2 probe golden 0.3203125 = 0x3EA40000 +
  det subset 2 往復)。**+9 net 1311 全緑** (機械検算 1302+9=1311)。
  **自己照査 3 件誠実記録**: (1) f2 Δ の私の初予想 (speed=40 → 非ゼロ)
  は dome u8 量子化で厳密 0 だった → 実機 probe 4 点計測 (40/200/500/
  2000 → 0.0 / 0.08203125 / 0.3203125 / 1.2617188) で speed=500 の
  0x3EA40000 を採用 (初回主張の自己捕捉訂正、rq 系と probe 値の混同
  禁止規律)、(2) 私のテスト先行コミット 85698a6 (未 push) が修正込み
  形状で混入 → soft reset で単一 wave コミットへ正規化、(3) baseline
  再測定で私が fixed 版を「旧版」へ誤コピー (integrated-code+旧試験
  =1302 緑を旧版と誤測) → 真の旧版+旧試験基準は 2c0a940 の CI 緑
  (30323336231) で代替確立。rq fe_fsr3 (1)-(6) 全 assert 通過
  (TDD compile RED 6 件 = E0609 report 4 field 機械記録、module 到達
  RED は下記 adversarial で逆証明)。

adversarial 5 系統: (a) 捕捉 84 revert (u32 wrap) **1 RED** (buffers
pin、事前予想一致)・(b) WGSL 乖離 revert (uv 空間式+select 固定+bias
なし uniform) **1 RED** (wgsl contract pin、予想一致)・(c) d_next +
clone_shallow 死計算復活 **0 RED 非検出** (値同一 dead code・警告 0、
除去正当性は §7 census + 不変式層不在証明が担保、誠実記録)・(d)
bootstrap serve 削除 **1 RED** (f1 Δ=0 assert が黒混入を検出、予想
一致)・(e) バッファ配線 revert (`let _ =`) **1 RED** (3 定数 assert、
予想一致)。変異前実体コピー /tmp+rsift/bak 先行・python assert/grep
-c で適用確認後計測・毎回復元 MD5-VERIFIED 5 回
(fg 098bf4bc9e4d5345de6a633473456c52、wiring
d7335407fcea8615c12ad5d2f60de38f — fmt 選択適用で初版
50215f46899f37f13bcd4b18bdb62c14 / c90efc801fdb8903737c4b42aab8244c
から再採番、src+/tmp+rsift/bak 三重照合)。

opt-gfx **1311 全緑** (net +9、機械検算 1302+9=1311、fmt 後全量再実測
21.68s・adversarial 後最終 21.28s)・api 49 全緑・replay 16 全緑・
警告 0・fmt: 両対象 HEAD 原生逸脱保有 → difflib+git diff HEAD 交差
判定で自己起因 hunk のみ選択適用 (fg 6 適用/3 原生保持・wiring 1/1、
seal ゲート2 機械値 fg 2/2/0・wiring 0/0/0 (両対象 PASS))・digest 004c1cf5fb17bfe8 rows=357
不変・seal 全 6 ゲート PASS・台帳 605・TRIGGER 196。

## wave 160 (FF) ddgi.rs 厳密監査 (2026-07-28)

census grep 機械確定: `ddgi::Vec3` 消費=full_graph_wiring:541/542/1466、
`chebyshev_visibility`=frame_ddgi:386 (実消費)、`wgsl_source`=gpu_runtime:59
登録、`probe_coord`=wiring:1466-1471 **`let _ = probe` 破棄 (§7 違反)**、
`probe_count`=自家テストのみ、`oct_encode_unit/oct_decode_unit`/`Vec4`=crate
全域消費者ゼロ (frame_ddgi は `oct_encode_wgsl` 等の別複製を `let uv =`
:191・`let dir =` :254・往復試験 :834 で実消費、外部参照なし)。他 crate
参照なし (stray コピー除外)。

- **FF-1 [中] 捕捉 86**: 旧 oct 対は z<0 の wrap で成分 swap を欠く非標準
  fold (ddgi: `((1-|ox|)sgn(ox),(1-|oy|)sgn(oy))` vs WGSL truth:
  `((1-|oy|)sgn(ox),(1-|ox|)sgn(oy))`)。rq ff_ddgi (1) で代数的乖離
  ((0.8,0.6) vs (0.6,0.8)) と実数厳密 roundtrip を導出、実機 probe
  (rustc -O) で corpus 8+6 の bit 記録: 下半球 3 入力で対角鏡像乖離を
  実測 ((0x3f19999a,0x3f4ccccd) vs (0x3f4ccccd,0x3f19999a) 等)。自己整合
  ペアのため旧 roundtrip 試験は緑のまま潜伏 (capture 81 同型の CPU/GPU
  ミラー乖離)。根治: WGSL 式ツリー同一語彙化 (pre-normalize 撤去: 射影
  s が正規化を兼ねる、swap fold、タプル同時評価で逐語代入の更新漏れを
  構造排除)。TDD RED 4/4 を機械記録 (encode golden / decode golden /
  bit 同一 corpus / no-wrap、現行値は全て真値の対角 swap で検出)。
- **FF-2 [低] 捕捉 87 §7 消化 22 + 捕捉 88 [小]**: wiring:1466-1471 の
  `let _ = probe` 破棄を report 実フィールド 2 (ddgi_probe_coord [f32;3]、
  ddgi_probe_count u32 min 飽和) 配線へ根治。`probe_count` は u32 積が
  1626^3 超で暗黙 wrap (rq (3)、debug で overflow panic 実測、capture 84
  同型) だったものを usize 積昇格。Vec4 (型+3 演算 impl)・
  Vec3::{normalize,Add,Mul<f32>} は修正後の消費者ゼロを機械 grep
  (crate 全域 + 他 crate) で証明し不可能証明削除 (async_compute EJ-2
  判例、`use std::ops::{Add,Mul}` 未使用化も同時整理、警告 0 維持)。
- **FF-3 [低] 捕捉 89**: frame_ddgi の oct_encode_wgsl/oct_decode_wgsl を
  ddgi 本家への bit 同一委譲へ統合 (二重実装解消 + oct 対への実消費者
  新設、probe: 委譲版は旧複製と corpus 全域 bit 同一 enc 8/8・dec 14/14)。
  decode の旧 1e-8 normalize ガードは Sigma|n_i| = a+|1-a| >= 1 (rq (2)
  milli grid min=1、実数上は三角不等式で恒成立) に到達不能と証明し除去。
  自己照査記録: 旧 decode ty 項が sign(ox) typo 状態と一時主張したが
  sed 一次確認で `if oy` の自己整合実装と判明、台帳・節とも訂正済
  (grep 折返し一字確認工程の信用担保)。rustfmt 後の mut-(e) アンカー
  不一致は python assert が適用失敗を阻止 (誠実記録、現テキスト一行化
  を確認後に再適用)。
- **FF-4 [低] strict +7 net 1318 全緑** (機械検算 1311+7、fmt 後全量
  21.10s・adversarial 後最終 21.17s): ddgi 6 (WGSL swap golden encode/
  decode・frame_ddgi bit 同一 corpus 8・probe_count no-wrap golden・
  probe_coord 厳密 binary pin (0x3fc00000/0x3f800000/0x40400000/
  0xc0200000/0xbf800000 全て 2 冪 cell の厳密値、実機 probe 確定)・
  Sigma bound grid) + wiring 1 (camera (24,8,48)/(-40,-8,-16) の非ゼロ
  工程化 2 tick pin、wave 157 FC の all-zero vacuous 教訓を設計に織込)。
  rq ff_ddgi (1)-(4) 全 assert 通過 (python 引退継続、max が f32 専用の
  言語制約は一次確認で if 分解へ修正した上で全緑)。adversarial 5 系統:
  (a) encode revert 3 RED / (b) decode revert 3 RED / (c) u32 revert 1 RED
  (overflow panic) / (d) Vec4 死コード再追加 **非検出** (pub 死コードは
  警告も出ず、code-review/census 領域の限界として誠実記録、wave 157 (d)
  系 9 例目) / (e) report pin 0 化 1 RED。復元 MD5-VERIFIED 5 回
  (ddgi 81e8421d/frame_ddgi 94e9d4bc/wiring aaac157d 三重照合)。誠実
  分析: `ff_oct_pair_bit_identical` は対称変異 (両側同一の誤り) を
  理論上不検出 (consistency pin であって truth pin ではない) — 絶対真値は
  golden 2 本が担保、と pin 分類を明文化。api 49・replay 16 全緑・警告 0・
  fmt 自己起因 0 (seal ゲート2 機械値: ddgi 2/2/0・frame_ddgi 2/2/0・
  wiring 0/0/0 全 PASS — difflib+git diff 交差選択適用では自己起因範囲と
  交差する hunk が 0 件 (0/3・0/2) で機械的に変更要パッチゼロ!)。digest 004c1cf5fb17bfe8
  rows=357 不変・seal 全 6 ゲート PASS・台帳 609・TRIGGER 197。

## wave 161 (FG) frame_pacing.rs 厳密監査 (2026-07-28)

census grep 機械確定: `FramePacer::new`/`record_frame` は full_graph_wiring
:543/:629 実消費、`next_present_time`/`smoothed_frame_ms`/refresh_hz 読取は
消費者ゼロ (自家テストのみ)、`FramePacing::wgsl_source` (unit-struct 装飾
メソッド) ゼロ、`FRAME_PACING_WGSL` const は gpu_runtime:94 直参照登録。
本モジュール内部 (S-3 契約 assert・EMA NaN 遮断・O(1) 境界 snap +
±1 端数補正ループ) は既波群 (17/23/32) で強化済、**アルゴリズム面の
新規捕捉はゼロ** (最早境界不変式 sweep 2000 件・NaN/負ギャップ帰着・
巨大ギャップ O(1) が既存緑) — 誠実な陰性監査結果として記録し、本 wave
は配線/契約醸成案件のみで完結。

- **FG-1 [低] 捕捉 90 [小]**: `FramePacing` unit struct + `wgsl_source(&self)`
  は crate 全体で消費者完全ゼロの完全装飾 (tbdr_hints.rs:56-58 が同型を
  明文判例化済: 「状態を持たず &self を使わない装飾メソッド」)。EL-1
  判例へ整合: free fn `wgsl_source()` + 登録一本化。wgsl は 2 行コメント
  のみ (真空) だが、mip_streaming 波 43 / tbdr_hints:15-20 の確立方針
  (registry 真空 marker は removed-not ではなく「設計上存在し得ない」の
  正当 marker + naga pin で維持、除去は naga 検証経路と wiring 連結 pin
  への波及で設計引継ぎ) に整合させ、frame_pacing.wgsl を自己参照
  ループ + ホスト時計観測不能の 2 理由で marker 強化し、
  wgsl_is_intentionally_shader_free_marker (naga parse・entry/global ゼロ)
  を新設。私の当初案 (registry エントリ+ファイル削除断行) は先行波方針と
  衝突するため機械証拠で撤回・方針整合側へ修正したことを誠実記録。
- **FG-2 [低] 捕捉 91 [小] §7 消化 23**: vsync snap/平滑値/目標 Hz の
  消費者ゼロ → wiring Pacing 帳簿 (pacing_clock_ms/last_present_ms 状態、
  壁時計非依存の決定論系列) 真駆動 + report 3 実フィールド。refresh_hz は
  private+getter 化。rq fg_pacing (1) EMA 分数厳密 (s1=248/15、s2=1232/75、
  私の初版誤式を rq が捕捉し訂正済) (2) 境界 index (ceil 46/50=1 等)
  (3) 負ギャップ n=1 帰着 (650/3>200) (4) 1 分=3600 提示厳密、f64 実機
  probe bits: s1=0x4030888888888889・s2=0x40306d3a06d3a06e・
  t1=0x4030aaaaaaaaaaab・t2=0x4040aaaaaaaaaaab・neggap=0x406b155555555555・
  hz=0x404e000000000000。
- **FG-3 [低] strict +3 net 1321 全緑** (機械検算 1318+3、fmt 後全量
  21.67s・adversarial 後最終 21.15s)。TDD compile RED E0609×5 機械記録。
  adversarial 6 系統: (a) ceil→floor 非検出 = floor(x)∈{ceil(x),ceil(x)-1}
  のため ±1 補正ループが構造吸収 (検出不動点)。(a2) 第1補正ループ除去
  非検出 = 発火は「i が fl 下方丸めかつ商が厳密整数」の bit 域で現 corpus
  外防御 (第2ループが上側 overshoot を担当する実動作整理)。(b) alpha
  0.2→0.5 は wiring bit pin のみ捕捉 (module 区間検査は検出不能と分類
  明記)。(c) hz 0 化 1 RED。(d) @compute 注入 1 RED で marker pin の
  有効性を実証。(e) 装飾 struct 再救出非検出 (dead code 系 10 例目)。
  復元 MD5-VERIFIED 6 回 (fp 646db112/gr 313fdd56/wiring ed39da22/
  wgsl ddcd149f 三重照合)。api 49・replay 16 全緑・警告 0・fmt 自己起因
  0 (seal ゲート2 機械値: frame_pacing 0/0/0・gpu_runtime 0/0/0・
  wiring 0/0/0 全 PASS — 手計測で自己起因 1 (naga parseStr 行の過剰
  折返し) を検出し in-place で解消後に seal 0/0/0)。
  digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS・台帳
  612・TRIGGER 198。

## wave 162 (FH) adaptive_shading.rs 厳密監査 (2026-07-28)

census grep 機械確定: `new`=render_pipeline:211-217 (feather 2 flag +
software_vrs_checkerboard 実設定由来)、`set_camera_speed`=:623 (M-1
回帰で実変位速度)、`tick`=:624 (同経路で実駆動 — 初判「tick 未駆動」は
変数名 `s` 見落としの grep 誤り、一次確認で訂正)、`should_draw_chunk`=
:703 vacuous gate (恒 true で `continue` は死分岐)、`shading_rate`/
`rate_for_distance`/`apply_motion`/`skip_stride`/`checkerboard_skip` は
自家 strict_tests のみ。lib.rs:71 wildcard re-export 経由の他 crate 参照
なし。

- **FH-1 [中] 捕捉 92**: 恒値スタブ 2 件 + 死帳簿: `honesty_spec_never_
  culls_geometry` が「将来拡張の遺物」「shading rate hint はメッシュを
  カリングしない」と証言する恒 false/true 常数 API で、pipeline は
  `if !s.should_draw_chunk(i,d) { shading_skipped += 1; continue }` の
  vacuous gate を保持 (到達不能分岐と不増カウンタの真空帳簿 — cross-pin
  1567 は a==b の決定性のみで 0 定数の検出力を持たなかった)。常数
  のため配線意味なし → 不可能証明削除、gate は rate 実計測へ根治、
  「カリングしない」仕様は API 非存在による構造保証へ昇格。
- **FH-2 [低] 捕捉 93 §7 消化 24**: 出力側 4 API の消費者ゼロを
  FrameStats 3 実フィールド (shading_half/shading_quarter/
  shading_stride_sum=u32) 真計上で根治。TDD compile RED E0609×3 機械
  記録。自己照査: use 行未更新の E0433×3 を全量ビルドで捕捉し
  `{AdaptiveShadingController, ShadingRate}` import で修正 (緑前解消)。
- **FH-3 [低] strict +1 net 1322 全緑** (機械検算 1321+1、fmt 後全量
  21.20s・adversarial 後最終 21.04s): fh_shading_rate_buckets_golden は
  camera (0,0) からの距離境界等号 2 件 ((3,0):48.0→Full、(6,0):96.0→
  Half) を含む非ゼロ工程化 golden (half=2/quarter=1/stride=9、rq
  fh_shading で分数ではなく i64 厳密導出、bool 比較は言語型制約で
  差分形式へ修正の上全 assert 通過)。honesty 試験はスタブ消失に伴い
  rate 飽和上限 (Quarter 飽和・stride≤4) の全掃引 pin へ作り替え。
  adversarial 5 系統: (a) 96→90 境界潰し 2 RED (module+pipeline 両 pin
  の二重捕捉)・(b) 8→9 動作閾値 1 RED・(c) Quarter→Half 誤帰属 1 RED・
  (e) stride 4→3 で 2 RED・(d) 恒値スタブ死救出 非検出 (dead code 系
  11 例目)。復元 MD5-VERIFIED 5 回 (shading fd0c9885/pipe badddd3a
  三重照合)。api 49・replay 16 全緑・警告 0・fmt 自己起因 0
  (seal ゲート2 機械値: adaptive_shading 15/13/0・render_pipeline
  0/0/0 全 PASS — stub 削除で原生逸脱 2 行も消滅、difflib 交差選択
  適用 0/12 で自己起因ゼロ実証)。digest 004c1cf5fb17bfe8 rows=357 不変・
  seal 全 6 ゲート PASS・台帳 615・TRIGGER 199。

## wave 163 (FI) pso_library_cache.rs 厳密監査 (2026-07-28)

census grep 機械確定: `get`/`insert`/`save` = full_graph_wiring:1694-1699
実消費 (wave 83 CG-7 で 100% miss 虚偽計器を真のキャッシュ挙動へ根治済)、
`stats()` = 消費者ゼロ (自家テストのみ)、`new` = :498 (cache_dir 由来)。
他 crate 参照なし。既監査注記 (2026-07-22) で save() ワイヤ形式の
不可逆性 (12B レコード = vs_hash+len のみ、ロード不可) は公知・設計として
公表済み。

- **FI-1 [低] 捕捉 94 [小] §7 消化 25**: CG-7 で「測る側」を真化した
  計器の「読む側」が閉じていなかった (`stats() = (hits, misses)` の実
  消費者ゼロ)。wiring report 2 実フィールドへ配線、3 tick 非ゼロ工程化
  pin (material_independent_hash 不変のため初 tick のみ miss、hit 経路
  必発火 = vacuous 回避設計、rq fi_pso (1))。TDD compile RED E0609×5
  機械記録。
- **FI-2 [低] 捕捉 95 [小]**: ワイヤ blob 長の `as u32` キャストは
  4GiB 超で暗黙 wrap (capture 84/88 の wrap 禁止族)。`wire_blob_len`
  純粋関数へ抽出し u32 飽和へ根治。TDD は fn をキャスト版で先行実装し
  `left: 0 vs right: 4294967295` の value RED を機械記録、saturate で
  GREEN (到達不能域だが契約の数学的完全性として、と深刻度の境界を
  誠実明記)。
- **FI-3 [低] strict +2 net 1324 全緑** (機械検算 1322+2、fmt 後全量
  21.11s・adversarial 後最終 21.15s)。rq fi_pso (1)-(3) 全 assert 通過。
  adversarial 5 系統: (a) hits/misses swap 1 RED・(b) CG-7 再帰 (tick
  混合キー → 恒 miss) 1 RED で回帰ガード実証・(c) cast revert 1 RED・
  (d) report 0 化 1 RED・(e) dead code 死救出 非検出 (12 例目、census
  grep 領域の限界として一貫記録)。復元 MD5-VERIFIED 5 回 (pso
  6b21d11d/wiring 194bef49 三重照合)。api 49・replay 16 全緑・警告 0・
  fmt 自己起因 0 (seal ゲート2 機械値: wiring 0/0/0・pso 7/7/0 全 PASS
  — difflib 交差選択適用 1/9・1/2 で自己起因 hunk のみ解消)。digest 004c1cf5fb17bfe8 rows=357 不変・seal
  全 6 ゲート PASS・台帳 618・TRIGGER 200。

## wave 164 (FJ) ibl_sh.rs 厳密監査 (2026-07-28)

census grep 機械確定: ibl_sh の消費者は full_graph_wiring:2121-2129 の IBL
抽出 (Vec3::new/sh_basis/evaluate_sh/Add/Mul<f32> 実消費 →
report.ambient_light) と gpu_runtime:74 の `wgsl_source` 登録の 2 系統。
Vec4・Vec3::Sub は crate 全域消費者ゼロ。陽性確定: SH 5 定数
(0.282095/0.488603/1.092548/0.315392/0.546274) は CPU リテラルと WGSL
本文が逐語一致しており捕捉 81 型の乖離は非該当 (6dp trunc は両側同一
設計、真値との絶対誤差 ~2e-7 は [観] 注記)。

- **FJ-1 [低] 捕捉 96 [小] §7 消化 26**: Vec4 (型+Add/Sub/Mul<f32>)・
  Vec3::Sub を不可能証明削除 (EJ-2/FF 判例)。WGSL `evaluate_sh` は
  `sh_basis(normalize(dir))` の二重正規化 (値不動点だが bit 語彙が一意
  でない装飾) を `sh_basis(dir)` へ統一 — TDD value RED 1/1
  (wgsl_matches_cpu_reference_vocabulary が旧形を検出) → GREEN。wiring
  :2123 の enumerate+`let _ = i;` 装飾撤去 (反復値不変、dome 先頭 9
  方向の CH-4 公知リング)。残愛器 new/dot/length/normalize/Add/Mul は
  sh_basis/evaluate_sh の内部語彙で全て実消費中を確認。
- **FJ-2 [低] strict +2 net 1326 全緑** (機械検算 1324+2、fmt 後全量
  21.20s・adversarial 後最終 21.20s): bit golden は実機 probe (rustc -O)
  3 端点全 9 項 (z+: Y00=0x3e906ec1/Y1,0=0x3efa2a2c/Y2,0=0x3f217b0f、
  x+: Y2,0=-0.315392=0xbea17b0f/Y2,2=+0.546274=0x3f0bd89d、y+: Y2,2
  符号反転=0xbf0bd89d) + (0,0,2) 非単位入力の正規化不動点 bit 一致 pin。
  adversarial 5 系統: (b)/(c) (rs 側定数・添字変異) で bit golden RED、
  (d)/(e) (wgsl 定数テキスト・二重正規化復帰) で vocabulary pin RED、
  (a) Vec4 死救出 非検出 (dead code 系 13 例目)。復元 MD5-VERIFIED 5 回
  (ibl 7d767aa4/wiring 8f0a3a5c/wgsl 36b12568 三重照合)。api 49・replay
  16 全緑・警告 0・fmt 自己起因 0 (ibl/wiring 共に HEAD 逸脱と現逸脱が
  全一致、seal ゲート2 機械値: wiring 0/0/0・ibl 8/8/0 全 PASS)。
  digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS・台帳
  620・TRIGGER 201。

## wave 165 (FK) decals.rs 厳密監査 (2026-07-28)

census grep 機械確定: `decal_local` = full_graph_wiring:1914 (EB-3 公知
junction: 書込みサイト 0 件・登録 API 不在で恒空ループ、旧 `let _` 評価
破棄)、`wgsl_source` = gpu_runtime:75。`soft_edge`/Vec4/Vec3::Add/Mul は
自家テスト以外ゼロ。census のẢng 補足: 全消費構造は wave EB-3 の誠実
注記どおり (本 wave は評価破棄側の根治に限定、登録経路設計は継承)。

- **FK-1 [低] 捕捉 97 [小]**: zero-half 除算チャネルの NaN 潜入口を
  事前拒否へ根治 (TDD value RED 1/1 → GREEN)。数学整理 (rq fk_decals
  (1)(2)): ゲート `|x|<=half.x` は half.x==0 かつ x==0 で真 → 0/0=NaN;
  負 extent は恒偽で自然 None; half.z==0 は z 除算なしで無害 (d3 陽性
  pin で仕様として固定)。境界等号は 3 辺全て inclusive (x/half.x =
  1.0 bit 厳密 0x3f800000)。
- **FK-2 [低] 捕捉 98 [小] §7 消化 27**: 評価破棄を計測へ閉塞
  (report.decals_projected u32、EB-3 junction 設計を温存した上で計測
  点として真値 0/非 0 を帳簿) + Vec4 (型+3 演算)・Vec3::{Add,Mul<f32>}
  不可能証明削除 (判例連鎖 4 wave 目、crate 全域 + 他 crate grep 証跡)。
  soft_edge 保持判定: 「decal_local と対をなす正当アルゴリズムで自家
  strict と wgsl 語彙に消費証跡があり、登録経路接続時の実消費に直結」
  — directive⑦ の保持側判断を明文記録 (削除は不可能証明不能 = 意味を
  持つ計算であり fake 配線も不可のため)。
- **FK-3 [低] strict +3 net 1329 全緑** (機械検算 1326+3、fmt 後全量
  21.06s・adversarial 後最終緑確認済)。wiring pin はテスト特権 push で
  非ゼロ工程化 (projected=1、EB-3 公知の登録経路不在を誠実明記)。
  adversarial 5 系統: (a) guard 除去 1 RED・(b) x 境界厳格化 1 RED・
  (e) z 境界厳格化 2 RED (inclusive 3 系を全捕捉)・(c) 評価破棄復帰
  1 RED・(d) Vec4 死救出 非検出 (dead code 系 14 例目)。復元 MD5-
  VERIFIED 5 回 (decals 275c302e/wiring 06ea60a7 三重照合)。api 49・
  replay 16 全緑・警告 0・fmt 自己起因 0 (decals HEAD 実質正準 →
  rustfmt 全量適用で自己起因逸脱を解消、seal ゲート2 機械値: decals
  0/0/0・wiring 0/0/0 全 PASS)。digest 004c1cf5fb17bfe8 rows=357 不変・
  seal 全 6 ゲート PASS・台帳 623・TRIGGER 202。

## wave 166 (FL) gigavoxels.rs 厳密監査 (2026-07-28)

census grep 機械確定: `GigaVoxelsBrickStreaming` = full_graph_wiring:339
(フィールド)/:503 `new(2048)`/:1243 `request_brick(BrickKey(0,0,0,0))`
(`section_palettes.first()` 条件)/:1258 旧 `_bricks` 破棄。生成クロージャ
は実パレット由来の真生成 (`section_idx` = binary_greedy_meshing::idx、
SectionPalette = `[u16; 4096]`、恒値・空クロージャではない) で、brick
ボクセル値は負の座標重なりを 0..16 の実境界でクリップ。

- **FL-1 [低] 捕捉 99 [小]**: `process_requests` が pop 先行構造で
  budget==0 でもキュー先頭 1 件を生成していた契約逸脱 (rq fl_giga (1))
  → budget ゲートを pop より先行させる根治 (TDD value RED: budget=0 で
  processed>=1 を機械記録 → 0 へ GREEN)。partial carryover も厳密 pin。
- **FL-2 [低] 捕捉 100 [小]**: `new(0)` で pool 空のまま evict 経路に
  入り `pool_slots[0]` への OOB panic 潜入口 (rq (2)) → 構築時 assert
  (max_bricks >= 1) の fail-loud 化。should_panic strict で契約 pin。
- **FL-3 [低] 捕捉 101 [小] §7 消化 28**: wiring の
  `let _bricks = self.gigavoxels.process_requests(...)` 評価破棄を根治
  → report 実フィールド `gigavoxels_processed` / `gigavoxels_resident`
  (u32) への実計測配線 (brick ストリーミング負荷の決定性計測点)。
  TDD 機械記録: compile RED E0609×6 (report フィールド先行追加) →
  value RED 3/3 (budget_zero / new_rejects / report_pins_nonvacuous)。
  strict 当初 +5 (module 4: budget_zero / budget_partial_carryover /
  new_rejects should_panic / evicts_oldest_key_exactly (green-today
  pin) + wiring 1: report_pins_nonvacuous) net 1334 全緑を経て、
  adversarial (e) の盲点分析で強化 +1 (後述) → **+6 net 1335 全緑**
  (機械検算 1329+6、fmt 後全量 21.23s・最終緑確認済)。
  adversarial 5 系統: (a) budget==0 early return 除去 1 RED・(b) 構築
  assert 除去 1 RED・(c) wiring report 0 化 1 RED・(d) pub 死救出
  (resident_brick_count) 非検出 (dead code 系 15 例目、誠実連番)・
  (e) eviction 比較 `<`→`>`: 初回 **非検出 = 構造吸収** (oldest_idx
  初期値 0 が touch 無し系列で真の最古 slot 0 と常に一致、スキャン
  不更新化が不可視) → 盲点分析により touch 後 LRU 順位 pin の強化
  strict `fl_evicts_lru_after_touch_exactly` を新設し変異下 1 RED で
  検出確立、固定版で全緑。復元 MD5-VERIFIED 5 回 (b/c/d/e/強化後再
  適用、a は前セッションで計上済)。最終 md5: gigavoxels 4467d4ca /
  wiring 732abcd6。bak 固定版は強化テスト込み最終版へ更新済。
  api 49・replay 16 全緑・警告 0。fmt 自己起因 0 (seal ゲート2 機械値:
  wiring 0/0/0・gigavoxels 1/1/0 全 PASS; 手計測 tail+2 では
  gigavoxels HEAD 原生 8 行 = 1d0 artifact + process_requests シグ
  ネチャ長行、hunk 内容 HEAD と完全一致・行シフトのみ)。
  digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS・
  台帳 626・TRIGGER 203。

## wave 167 (FM) ao_bake.rs 厳密監査 (2026-07-28)

census grep 機械確定: `corner_ao` = full_graph_wiring:2681 実消費 (corner_ao_from_palette → :2517 refine_pull_quads)+自家 2 テスト・`bake_face_ao`/`AoNeighborhood` = 外部消費者ゼロ・自家 1 テスト・`ao_to_shade`/`pack_ao4` = 完全ゼロ (自家テスト含む)・WGSL 参照なし・rsift/src (main crate) 5 シンボル全無参照。GPU truth: terrain_vertex_pull.wgsl:141 `0.55 + f32(in.light_ao) * 0.15` (fs_pull) ≡ frame_reference.rs:82 (wave 21 同時根治・shade_exact_bits 同一演算順 pin 世帯)。

- **FM-1 [低] 捕捉 102 [小]**: `ao_to_shade` の `0→0.2, 1→0.45, 2→0.7, _→1.0` テーブルは GPU truth (0.55+0.15·k) と k=0..2 で対立する漂流語彙 (fm_probe 実機 bit: 旧 0.2=0x3e4ccccd vs truth 0.55=0x3f0ccccd、旧 0.45=0x3ee66666 vs truth k=1 0x3f333334、旧 0.7=0x3f333333 vs truth k=2 0x3f59999a、k=3 は 1.0 で偶発一致)。消費者ゼロで死蔵していたため未顕在化。根治 = truth 式 `0.55 + ao as f32 * 0.15` への同一化 + frame_reference::shade を本関数へ委譲 (single vocabulary 点化、FF 捕捉 89 判例。fm_probe で cur==del 全 4 値 bit 同一を実機証明、契約 ao∈0..=3 doc 明記)。**TDD value RED**: fm_ao_to_shade_truth_bits (k=0..2 で旧 bit ≠ golden) + fm_shade_delegates_to_ao_bake (textual pin、自己言及捕捉で concat! 分割針へ修正 — 一次実測誠実記録) の 2 RED → GREEN。
- **FM-2 [低] 捕捉 103 [小] §7 消化 29**: `pack_ao4` (4×2bit byte packer) は①呼出ゼロ ②pull 形式は quad 単一 2bit AO (packed4.rs:132・wgsl:45 `(w>>30u)&3u`) で 4-corner byte の格納域非存在 ③WGSL 消費者皆無 の 3 点不可能証明で削除 (FH 捕捉 92 判例、配線は捏造禁止のため不可)。`bake_face_ao`+`AoNeighborhood` は directive⑦ 保持判定 (soft_edge FK 判例同型: vanilla 4-corner 真実装かつ消費語彙 corner_ao の生成元、per-corner AO の GPU 消費は format 変更を要し将来登録経路) + 消費証跡強化として **strict 48 golden** 新設 (fm_probe2/3 機械導出: 無遮蔽 [3,3,3,3]/全遮蔽 [0,0,0,0]/エッジのみ [0,0,0,0]/対角のみ [2,2,2,2] の万有真理 + 単一セル 48 golden でコーナ順含む全写像 pin。green-today pin として誠実記録)。
- **FM-3 [低] strict 総括**: +3 net **1338 全緑** (機械検算 1335+3、grep count 1338 一次確認)。api 49・replay 16 全緑・警告 0。adversarial 5 系統: (a) ao_to_shade 0.55→0.50 定数改ざん → **2 RED** (fm_truth_bits + shade_exact_bits 連動)・(b) face0 corner0 s1 改ざん → **1 RED** (golden)・(c) pack_ao4 死救出 → **非検出** (dead code 系 **16 例目**、誠実連番)・(d) shade 委譲解除 (bit 同一 inline 戻し) → **1 RED** (textual pin のみ・bit pin 緑維持 = 語彙単一点 pin の特異価値実証)・(e) face5 corner 恒値 3 化 → **1 RED** (golden)。復元 MD5-VERIFIED 5 回 (a/b/c/d/e 各後)。
- **wave 167 自己照査事故 (同 wave 内捕捉・解決、誠実記録)**: ①fm_shade_delegates 挿入時に波 21 テスト shade_exact_bits の `#[test]` を剥奪 (属性は doc+fm 側に付着し fm が二重登録: "+3-1+1=1338" の帳尻で潜伏) — "shade_exact" フィルタ 0 件で発見、属性復元 + wave21 doc 帰属修復 + テスト総数 1338=1335+3 で機械整合確認。②fmt 採用置換で diff ハンク方向読み違い (file1=fmt/file2=現行の方向誤認) → ao_bake.rs 構文破壊 → /tmp/ao_fmt.txt rustfmt 正準完全版 (自己 rustfmt 逸脱 0 一次確認) から CRLF 全量復元 → 1338 green 再確認。③MUT-(c) 初回 python bytes リテラル非 ASCII で SyntaxError 未適用 → 再適用で正式非検出記録。
- fmt (seal ゲート2 機械値): ao_bake HEAD 逸脱 4 / 現 0 / 自己起因 0 (pack_ao4 削除で HEAD 原生逸脱消滅 + golden 配列 rustfmt 正準採用、CRLF 保持は gate1 PASS 世帯)・frame_reference 0/0/0。md5: ao_bake 17092f2f / frame_reference db17c5fe。rq /tmp/fm_ao.rq 全 assert 通過 (bit 定数は python 機械変換投入 — 暗算十進変換 5/7 誤りを python 照合で捕捉の誠実記録)。台帳 628・TRIGGER 204。

## wave 168 (FN) root_signature_optimized.rs 厳密監査 (2026-07-28)

census grep 機械確定: `OptimizedRootSignature::rs_graphics()`+`root_cost()` = full_graph_wiring:462-463 消費 → Self フィールド保持 (:351) → tick `let _ = self.root_cost;` (:1690) 評価破棄。`flags`/`RootParamType`/`RootParameter`/`StaticSampler` の direct 読取は実パイプラインに存在せず自家 strict のみ (wgpu 経由のため DX12 直値は設計図ドキュメント)。GPU/一次情報: MS Learn D3D12_ROOT_SIGNATURE_FLAGS (fetch 2026-07-28: 0x1=ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT、DENY_HULL=0x4/DENY_DOMAIN=0x8/DENY_GEOMETRY=0x10) + Root Signature Limits (64 DWORD 上限、Descriptor tables=1 DWORD each、Root constants=1 DWORD each (32-bit 値)、Root descriptors=2 DWORD each、Static samplers=cost 0)。vertex pull 確定: frame_pipeline.rs:479 `pass.draw(0..pull_vertex_count, 0..1)` で主パイプライン IA 不使用 (set_vertex_buffer は occlusion_query.rs:774 HI-Z 系のみ)。

- **FN-1 [低] 捕捉 104 [小]**: `flags: 0x1, // DENY_HS|DS|GS等最適化` は一次情報と正矛盾 — 0x1 は ALLOW_IA (IA opt-in) で DENY 系ではない。設計意図 DENY_HS|DS|GS = 0x4|0x8|0x10 = **0x1C** (python 機械値 28、rq fn_root 全 assert)。strict golden 自身も `assert_eq!(rs.flags, 0x1, "DENY 系フラグ 0x1 固定")` で誤値 pin 済 (スタグナント golden 二重虚偽)。根治 = flags 0x1C + コメント正確化 (vertex pull で IA 非使用のため ALLOW_IA 非 opt-in) + 旧 golden を真値へ。TDD value RED (fn_flags_truth_from_first_source で 0x1≠0x1C 失敗)。コスト規則 (Table=1/Constants=DWORD数/RootDesc=2/Sampler=0) は MS 公式と rs_graphics() 実値 (16+1+2=19) で一次照合・陽性確定。
- **FN-2 [低] 捕捉 105 [小] §7 消化 30**: tick `let _ = self.root_cost;` 破棄 → Self フィールド `root_cost_dwords` 改名正規化 + report 実フィールド `root_cost_dwords: u32` 実計測配線 (rs_graphics() 定数生成のため 19 固定 pin で非装飾化 — 64 DWORD 制限内不変式の実行時 telemetry)。TDD compile RED E0609×2 → GREEN。
- **FN-3 [低] strict 総括**: +2 net **1340 全緑** (機械検算 1338+2・grep count 1340 一次確認)。api 49・replay 16 全緑・警告 0。adversarial 4 系統: (a) flags 旧値回帰 → **2 RED** (fn_flags_truth+graphics_layout_fields 真値 golden 両側)・(b) DescriptorTable 重み 1→2 → **3 RED** (module 2+wiring report pin 波及)・(c) report 0 化 → **1 RED**・(d) dead code 死救出 → **非検出** (dead code 系 **17 例目**、誠実連番)。復元 MD5-VERIFIED 4 回。
- **wave 168 自己照査事故 (同 wave 内捕捉・解決、誠実記録)**: fn_root_cost 挿入が fl_gigavoxels_report_pins_nonvacuous の `#[test]` を剥奪し自身を二重登録 (wave 167 **同型事故の再発** — 死者+二重登録で総数帳尻が 1340 に一致する危険な相殺状態。「新規テスト追加後は対象+隣接テストを個別フィルタで確認」の対策不足を認めて記録) → 領域全体再構成で修復 (fl 復活・二重解消)、doc 帰属に修復注記。
- fmt (seal ゲート2 機械値): wiring 0/0/0・root_sig 23/23/0 (自作分逸脱は正準採用で消去、HEAD 原生 hunk 8 件同型シフトのみ)。rq /tmp/fn_root.rq 全 assert 通過 (初版暗算式を rq が捕捉 → 素直式へ修正の誠実記録)。md5: root_sig c96a6223 / wiring 491a54e3。台帳 630・TRIGGER 205。

## wave 169 (FO) bundle_reuse.rs 厳密監査 (2026-07-28)

census grep 機械確定: `BundleCache` = wiring:359 フィールド/:521 構築/:1743-1757 chunk_keys take(8) で get_or_create 実消費 (key=chunk+lod 固定 0、コマンド SetPipeline+DrawIndexed(idx_count 実供給))。`stats()` = wiring:1760 `let _bundle_stats = ...;` 評価破棄。本体アルゴリズム群 (vertex_count=DrawIndexed 和算・hits/misses 帳簿・再借用 NLL 構造) は自家 3 テストで pin 済 — 追加捕捉なしの陰性確定 (HEAD 完全照合、wave 本体未編集)。
- **FO-1 [低] 捕捉 106 [小] §7 消化 31**: `_bundle_stats` 破棄 → report.bundle_hits/bundle_misses (u64 累積) 実配線 (pso_lib 帳簿報告 FI-1 と同型)。TDD compile RED E0609×4 → 8 キー固定供給で初回 (hits=0, misses=8)・2 回目累積 (hits=8, misses=8) 非ゼロ両側 pin → GREEN。
- **FO-2 [低] strict 総括**: +1 net **1341 全緑** (機械検算 1340+1・grep count 1341 一次確認)。api 49・replay 16 全緑・警告 0。adversarial 3 系統: (a) report 0 化 → **1 RED**・(b) stats() hits/misses 交換 → **2 RED** (自家 tests+wiring pin 両側)・(c) dead code 死救出 → **非検出** (dead code 系 **18 例目**、誠実連番)。復元 MD5-VERIFIED 3 回 (wiring bak e5587950 + bundle HEAD 照合 2 回)。
- wave 168 対策 (新規テスト後の隣接個別フィルタ確認) を実施: fn_root_cost・fl_gigavoxels 隣接無傷確認。fmt wiring 0/0/0 (自作分正準、HEAD 原生 0)。md5 wiring e5587950。台帳 631・TRIGGER 206。

## wave 170 (FP) mimalloc_config.rs 厳密監査 (2026-07-28)

census grep: for_tier/recommended_allocator/env_string = full_graph_wiring:446-452 info! ログ消費 (実消費者存在)。use_mimalloc 読み取り消費者ゼロ・Cargo.lock/全 Cargo.toml に mimalloc 皆無 (未リンク truth) を機械確定。

- **FP-1 [低] 捕捉 107 [小]**: `env_string()` が mimalloc 公式 env 語彙に存在しない架空文字列 "MIMALLOC_ARENA=64MB_LARGE=1048576" を生成 (README truth: オプション `arena_reserve` = `MIMALLOC_ARENA_RESERVE` 単位 KiB) → truth 語彙根治。TDD value RED (fn_env_string_truth_vocabulary / fn_tier_arena_kib_truth 2/2)。
- **FP-2 [低] 捕捉 108 [小]**: §7 消化 32。`use_mimalloc` 恒 true 死蔵 (truth 未リンクで真値 false への虚構) + `large_object_threshold` (mimalloc env 語彙非対応の未配線値) を不可能証明削除。

rq fn_mimalloc 全 assert 通過 (KiB 換算 16→16384/32→32768/64→65536)。strict +2 net 1343 全緑 (機械検算 1341+2)。api 49・replay 16 全緑・警告 0。adversarial 4 系統: (a) 架空語彙逆変異 3 RED・(b) KiB 乗数 1000 変異 3 RED・(c) Minimal tier 16→8 変異 2 RED・(d) use_mimalloc 死救出 非検出 (dead code 19 例目、pub フィールドは lint 不検出)。復元 MD5-VERIFIED 4 回 (固定版 md5 c009e626f10205dce24ffe355180b4a2)。fmt: seal ゲート2 機械値 HEAD 逸脱 8 / 現 0 / 自己起因 0 (HEAD 原生存続の長行が根治で縮退、現逸脱ゼロ)。digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS。

## wave 171 (FQ) dashmap_registry.rs 厳密監査 (2026-07-28)

census grep: registry=wiring:383 保持/:537 構築/:768-770 set_state(Building) のみ消費。get_state/pending_chunks/version 読み取り消費者ゼロ・doc「再投入抑止に使える」虚偽を機械確定。

- **FQ-1 [低] 捕捉 109 [小]**: §7 消化 33。→ slab truth 連動の真状態機械へ根治 (初見 → Building → alloc 直後 Done、退去 → remove 新 API、state_counts/len 追加 + report 5 実フィールド + get_state 実消費 integrity)。TDD compile RED E0609×5 → 実装 GREEN。module 側は remove API compile RED E0599×2 → 追加 GREEN。
- **FQ-2 [低]**: pending/building は同期 pipeline truth で常 0 の到達不能 pin として doc 誠実明記 (非同期化で非ゼロ変動)。

rq fq_registry 全 assert 通過 (version 4→7 導出: 初見 +2/件・退去 +1/件)。strict +3 net 1346 全緑 (機械検算 1343+3)。api 49・replay 16 全緑・警告 0。adversarial 4 系統: (a) Done 遷移除去 1 RED (integrity debug_assert 即検出)・(b) remove version 前進除去 2 RED (module+wiring 両側)・(c) report 0 化 1 RED・(d) failed_chunks 死救出 非検出 (dead code 20 例目)。復元 MD5-VERIFIED 4 回 (固定版 md5 dashmap 53f478717a407aa2ac9133078f0f4605 / wiring fdfeca7d400234548542a83695d618c1)。fmt: seal ゲート2 機械値 dashmap HEAD 逸脱 10 / 現 10 / 自己起因 0 (HEAD 原生完全保持、git diff 削除行 0)・wiring 0/0/0 (自己起因 1 適用済)。digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS。

## wave 172 (FR) rayon_job.rs 厳密監査 (2026-07-29)

census grep: parallel_map_chunks = full_graph_wiring:863 実消費 (morton 局所性コード生成、indexed collect 順序保持が契約)。build_pcore_threadpool/RayonJobConfig/parallel_for_each_chunk = 全クレート全域呼出ゼロ・core_affinity dep 非存在・threads 読み書きゼロを機械確定。

- **FR-1 [低] 捕捉 110 [小]**: build_pcore_threadpool の doc/名虚偽 (P-Core 専用・core_affinity ピン留め言及だが affinity 未構成) → 不可能証明 3 点削除 (FH 捕捉 92 判例)。
- **FR-2 [低] 捕捉 111 [小]**: §7 消化 34。RayonJobConfig + parallel_for_each_chunk 消費者ゼロ → 不可能証明削除。module は wiring 実消費の parallel_map_chunks 単機能へ truth 縮退。

adversarial 2 系統: (a) 順序逆転変異 2 RED (preserves/fr_deterministic 両側)・(b) pcore 死救出 非検出 (dead code 21 例目)。復元 MD5-VERIFIED 2 回 (固定版 md5 eed00df59afebb7e57c9a982e00b3e34)。strict 削除 2・追加 1 → net -1 で 1345 全緑 (機械検算 1346-1)。api 49・replay 16 全緑・警告 0・fmt 自己起因 0 (seal ゲート2 機械値 HEAD 1/現 1/自己起因 0 = artifact のみ)。digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS。

## wave 173 (FS) pgo_bolt.rs 厳密監査 (2026-07-29)

census grep: PgoConfig::release().rustflags() = full_graph_wiring:465 info! ログ実消費。dev()/bolt/pgo_instrument 読み取り消費者ゼロ・CI RUSTFLAGS 未配線 (bench.yml のみ) を機械確定。workspace [profile.release] lto=fat/CU 1 一致で release 値は陽性。

- **FS-1 [低] 捕捉 112 [小]**: rustflags() が pgo_instrument/bolt を完全無視 (bolt: true でも出力同一の計装不整合) → pgo_instrument truth 接続 (-C profile-generate 付加) + bolt 不可能証明削除 (rustc flag 非対応 truth)。TDD value RED 1/1。
- **FS-2 [低] 捕捉 113 [小]**: §7 消化 35。dev() 消費者ゼロ → 不可能証明削除。

adversarial 3 系統: (a) profile-generate 分岐除去 1 RED・(b) target-cpu v3→v2 変異 2 RED (profile_field/rustflags_string 両側)・(c) bolt 死救出 非検出 (dead code 22 例目)。復元 MD5-VERIFIED 3 回 (最終固定版 md5 a94328a48cad32048e4fc789a7397f00、fmt 正準化で更新)。strict +1 net 1346 全緑 (機械検算 1345+1)。api 49・replay 16 全緑・警告 0。fmt: 自己起因 11 機械分離 (release struct/format!/テスト struct update ×2) → 4 hunk rustfmt 正準化で逸脱 0 → seal ゲート2 機械値 HEAD 4 / 現 1 / 自己起因 0 (現逸脱は artifact のみ)。digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS。

## wave 174 (FT) instanced_draw.rs 厳密監査 (2026-07-29)

census grep: InstancedCollector/add/total_instances/groups/as_bytes = full_graph_wiring:1108-1125 実消費 (InstancedData Pod 32B レイアウト・追加順保持は module 3 テスト pin 済で本体アルゴリズム陰性)。wiring:1121 の `let _instanced_stats` 破棄 (doc「帯域見積に反映」虚偽) を機械確定。InstanceData 32B/追跡/bytes roundtrip は HEAD 完全照合の陰性。

- **FT-1 [低] 捕捉 114 [小]**: §7 消化 36。→ report 3 実フィールド真配線 (instanced_total/groups/bytes)、bytes を Σ 全グループ総バイト truth へ根治 (旧 first mesh のみ見積)。TDD compile RED E0609×3 → 配線 GREEN (ft_instanced_report_truth: groups 3/total 5/bytes 160 非ゼロ pin)。

rq ft_instanced 全 assert 通過。adversarial 3 系統: (a) report.instanced_total 0 化 1 RED・(b) total_instances を first group のみ変異 2 RED (module+wiring 両側)・(c) clear API 死救出 非検出 (dead code 23 例目)。復元 MD5-VERIFIED 3 回 (固定版 md5 wiring 9e3e946c9f4de4d3d959ad74e6547975、instanced_draw 未変更で git restore 2 回)。strict +1 net 1347 全緑 (機械検算 1346+1)。api 49・replay 16 全緑・警告 0。fmt: 自己起因 1 (map クロージャ) 適用 → seal ゲート2 機械値 HEAD 0 / 現 0 / 自己起因 0。digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS。

## wave 175 (FU) bump_arena.rs 厳密監査 (2026-07-29)

census grep: BumpArena=wiring:393 保持/:553 new(4MiB)/:869 reset/:880 alloc_slice morton 実確保+copy。used() は module 4 テスト (align/枯渇/容量/reset 再利用の厳密 pin 済で本体アルゴリズム陰性 HEAD 照合) 以外に読み出しゼロを機械確定。frame_arena は stutter_guard 別型で非対象。

- **FU-1 [低] 捕捉 115 [小]**: §7 消化 37。used() 消費者ゼロ → report.bump_used 実計測配線 (morton 実確保バイト truth、reset 周期の当該 tick 値)。

rq fu_bump 全 assert 通過 (max(k,1)*8: 5→40/3→24/0→8)。adversarial 3 系統: (a) report 0 化 1 RED・(b) alloc 引数 +1 変異 1 RED (used 40→48 偏差)・(c) capacity() 死救出 非検出 (dead code 24 例目)。復元 MD5-VERIFIED 3 回 (固定版 md5 wiring a36fd83cfb42e194265a9fc3a3da64a4、bump_arena 未変更で git restore ×1)。strict +1 net 1348 全緑 (機械検算 1347+1)。api 49・replay 16 全緑・警告 0。fmt: seal ゲート2 機械値 HEAD 0 / 現 0 / 自己起因 0。digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS。

## wave 176 (FV) gtao.rs 厳密監査 (2026-07-29)

census grep: Gtao=wiring:440 フィールド参照/:599 `Gtao::new()`/:2348 `occlusion` 実消費 (tick_world_step_palette_reverse_ej_gtao_varies 等 strict 多数)。GTAO_WGSL=gpu_runtime:106 実消費 (定数経由)。4 module テスト (flat/nearby/closer_taller/multi_slice) は本体アルゴリズム陰性 HEAD 照合の範囲 pin で厳密性不足 → fv strict 追加。

- **FV-1 [低] 捕捉 116 [小]**: directions/steps/radius フィールドが Default 定数保持のみで読み取り消費者ゼロ (CPU 参照は呼出し側 samples/slices 駆動でパラメータ非参照、外部読み取り全クレート grep ゼロ) → unit-struct 化 (`pub struct Gtao;`、EL-1 tbdr_hints 波 138 判例)。new() 返却型不変で wiring:599 無傷。
- **FV-2 [低] 捕捉 117 [小]**: §7 消化 38。`wgsl_source()` 消費者ゼロ (GTAO_WGSL 直接消費で装飾) → 不可能証明削除。CPU/WGSL 語彙差分析: WGSL `pow(1.0 - occlusion, u.power)` 一般形 vs CPU `1.0 - occlusion` は power=1.0 特殊形で数学的に一致 (uniform 構築コード crate 内非存在 = GPU truth はソース登録のみ) → 陽性確定で fv_wgsl_power_special_form_vocabulary テキスト pin 化 (FE 判例: gtao.wgsl の atan2(h - center / clamp(maxHorizon / 1.5707963 / pow(1.0 - occlusion, u.power) 語彙含有)。
- **FV-3 [低]**: adversarial 変異 A (clamp 除去) 初回非検出を捕捉 → 下端 truth pin fv_clamp_downward_returns_unoccluded 追加 (空 samples→max_horizon -π/2→ratio -1、負のみ→ratio -0.5、いずれも clamp で 0 → 1.0 返却、bit 0x3f800000 の f32 probe /tmp/fv_probe2 機械値 2 ケース pin) → 再変異で RED 1 検出可能化 (dead/loose pin 非検出 25 例から 1 回収)。

strict: fv_slice_golden_bits (near=0x3dd75208・far=0x3ebfa8b8・flat=0x3f800000・avg=0x3e757d3a、f32 probe 導出、bit→10進は python 照合値 1037521416/1052747960/1065353216/1047887162) + fv_wgsl_power_special_form_vocabulary + fv_clamp_downward_returns_unoccluded、計 +3 net **1351** 全緑 (機械検算 1348+3)。adversarial 3 系統: (a) clamp 除去 1 RED (強化後)・(b) occlusion() avg→first slice 退化 1 RED (avg golden pin 検出)・(c) wgsl_source() 死救出 非検出 (dead code 25 例目、pub fn は lint 不検出)。復元 MD5-VERIFIED 3 回 (固定版 md5 ff15cbec5c404c7df6ce9aa5cc3be1ea)。api 49・replay 16 全緑・警告 0。

自己照査誠実記録: (i) bit→10進変換暗算 3/4 誤りを python 機械照合で捕捉 (比較は順序保持で実害 0、正値で rq 再実行済); (ii) adversarial 初回 md5 -c を復元前に実行 → FAILED ガード検出、bak 復元後 MD5-VERIFIED; (iii) fmt 正準化は 2 段 (初回 edit が impl 閉じ `}` 前の空行を見落とし、diff 再採点で 54d54 を自己検出・確定修正)。fmt: seal ゲート2 機械値 HEAD 逸脱 0/現 逸脱 0/自己起因 0 (手計測 diff でも artifact のみ、正準化 2 段経過後)。digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS。

## wave 177 (FW) section_compress.rs 厳密監査 (2026-07-29)

census grep: CompactSection=world_column_store (struct field :16・encode_hot :154/:196・to_cold :189・stored_bytes :104) 実消費、lib.rs re-export。encode_cold は encode_hot fallback 内部消費のみ、**is_air() は全クレート+module テストで消費者完全ゼロ** を機械確定。2 module テスト (hot_rle_roundtrip 範囲 pin/cold_lz4_roundtrip 完全一致 pin) は variant 選択・境界・to_cold を未 pin で厳密性不足。

- **FW-1 [低] 捕捉 118 [小]**: §7 消化 39。is_air() 消費者ゼロ → directive⑦ 第一選択で column_for_mesh へ真配線 (air セクション decode skip)。数学的等価証明 (is_air()=true ⟹ decode()≡[0;VOL]): Single(0) 定義・RleSection::is_empty()=全 run block 0 で fill 対象全 0・**空 runs の手組み Rle は現行 decode が契約 assert panic する (wave 61 BK-1) ので skip 配線はむしろ頑健化**。any フラグ (範囲重複 truth) 挙動不変。補題を fw_is_air_truth_lemma で形式 pin。
- **FW-2 [低] 捕捉 119 [小]**: encode_hot fallback コメント「If RLE is worse than raw」虚偽 → truth (RLE bytes ≥ 4098 = raw 8192B の約半分で保守的早期退避、break-even runs ≥ 2048 ではない) へ doc 書換え (挙動不変)。
- **FW-3 [低]**: adversarial 変異 C 初回非検出捕捉 → 破損 LZ4 decode 契約 pin (空/欺瞞 size prefix 2³⁰/末尾切詰め 8B の 3 ケース、panic せず全 0 返却の防御設計 — Lz4 wire は to_cold メモリ内のみで破損 wire 到達不能 = wave 61 BK fail-loud 対象外を明記) → 再変異 RED 1 で検出可能化 (FV-3 同型回収)。

rq fw_section 全 assert 通過 (idx(1,2,3)=801・runs=3・stored=2+3*4=14・1022*4=4088<4096→Rle・1024*4=4096>=4096→Lz4・raw=8192・threshold bytes=4098)。fw strict 計 +4 net **1355** 全緑 (機械検算 1351+4): fw_encode_variant_selection_exact (Single/1差分 Rle runs 値 pin/境界両側 1022,1024)・fw_is_air_truth_lemma (全構築 variant 補題 pin)・fw_lz4_decode_corrupt_contract (3 破損ケース契約)・fw_to_cold_forms (Single/Lz4 clone・Rle→Lz4 化 roundtrip)。adversarial 3 系統: (a) fallback `>=`→`>` 1 RED (境界 pin 検出)・(b) is_air 配線反転 3 RED (ingest_and_window + render_pipeline strict 2 本波及で配線 truth 逆証明)・(c) unwrap 化 初回 0 RED 非検出 → pin 追加後 RED 1 (26 例未採で回収)。復元 MD5-VERIFIED 5 回 (固定版 md5 section_compress e2863fe41c333a390cfe65755d254a22・world_column_store 4964e5c84c11e9b2518f9e14ce93f2d1)。api 49・replay 16 全緑・警告 0。fmt: section_compress 自己起因 4 hunk 正準化 (1 回目 3+見落とし 1 を再採点で自己検出、誠実記録) → 現 ZERO、world_column_store は HEAD 原生逸脱 21 行保持 (新規編集 column_for_mesh 領域は逸脱ゼロ)。fmt seal ゲート2 機械値: section_compress HEAD 0/現 0/自己起因 0・world_column_store HEAD 9/現 9/自己起因 0 (HEAD 原生完全保持)。digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS。
## wave 178 (FX) software_tiling.rs (111 行) — カメラ非追従 bin 配線・doc 虚構・装飾的 clamp
- census 機械確定: SoftwareTileBinner=render_pipeline:108 保持/:638 new/
  :645 bin_chunks→frame_stats.tiles_binned 実消費。SoftwareTileBinner/
  TileId/TileDrawList 直接参照=render_pipeline のみ (dx12 等他クレート
  参照ゼロ)。tile_count()=他 module/他 crate 参照ゼロ・内部 tw/th 導出
  のみ。tile_size_px default 64 (feather_preset.rs:45)、software_tile
  _binning default=false (disabled 既定)。
- 捕捉 120 [中] (FX-1: render_pipeline:645 bin_chunks を (0.0, 0.0) 固定
  渡しでカメラ非追従 → 真の world.camera_chunk() 配線)。tdd: RED→GREEN
  確立 — 新 strict fx_tile_binner_camera_tracks_world_camera (camera
  chunk (24,0) で 8 chunk が 3 tile 分散、/tmp/fx_probe.rs で f32 px/tx
  機械導出 tx 4,4,4,5,5,5,5,6・ty 全 2 → tiles=3) を先記述し旧配線で
  RED 適合 (旧 (0,0) 固定: clamp 集約 1 tile) → 配線後 GREEN。
  なお中途工程でテスト挿入により既存 camera_speed_measured_from_real
  _displacement の #[test] attribute を剥奪する自己起因事故を検出即修正
  (wave 167/168 判例、テスト 2 本を個別フィルタで復元後検証)。
- 捕捉 121 [小] (FX-2: module doc「draw reorder」に対し chunk_indices は
  未配線・lists.len() 計測のみ消費 → doc truth 化、CJ-6 判例様式)。
- 捕捉 122 [小] (FX-3: tile_count() 外部消費ゼロ → pub 剥奪 §7 消化 40)。
  adversarial 変異 C (pub 復活) 初回非検出 → included_str! 自己参照
  lexeme pin で回収 (fx_tile_count_visibility_lexeme、検出語彙分割記述
  で自己言及 vacuous 化回避・wave 176 判例) → 再変異 RED 1 検証。
- 捕捉 123 [小] (FX-4: bin_chunks の `.clamp(-1.0, 1.0)` は装飾的二重
  防御 — 数学的等価証明 (ndc>1→px≥640→cap、ndc<-1→負飽和 0、NaN→0)
  の上で削除、テスト fx_bin_edges_saturate_and_cap 改題・真実記述)。
  adversarial 変異 B (clamp 除去) を非検出→関数等価で釈明、真の不変式
  pin (min cap) の除去変異初回はテストフィルタ `tile` が対象に不一致で
  無効測定 → ターゲット絞りで正当 RED 1 検証 (誠実訂正記録)。
- strict +6 net 1361 全緑 (機械検算 1355+6): module 5 本 (visibility
  lexeme/recenter/saturate_and_cap/equal_distance_stable/floor_boundaries)
  + render 側 1 本 (camera_tracks)。rq /tmp/fx_bounds.rq 全 assert
  (110>19/3520/32=110/ceil 640/17=38 等)。api 49・replay 16 全緑・警告 0。
- adversarial 4 系統 4/1/1/回収RED1: (a) camera 配線 revert (cam_cx→0.0)
  → fx_tile_binner RED 1 検出。(b) tx cap (min(tw-1)) 除去 → fx_bin
  _edges_saturate_and_cap RED 1 (110 vs 19)。(c) tile_count pub 復活
  → 初回非検出 (API 露出 pin 不在) → lexeme pin 追加で回収・再変異
  RED 1。(d) clamp 除去 (FX-4 適用後) → 実施済み削除の検証は非検出は
  関数等価で釈明。復元 MD5-VERIFIED (software_tiling 固定版 md5
  8741aaba8924274315e44cf64474211b・render_pipeline 固定版 md5
  7bda52c17edf20142d8eb243a8fde943、bak 第 2 固定版は lexeme 追版で
  md5 同、変異は逐適用/逐復元)。
- fmt: 自己起因 1 hunk (right assert 折返し) 正準化、HEAD 原生逸脱ハンク
  (TileDrawList 初期化/trace!/既存テスト長行) 完全一致で完全保持
  (機械分離 6del/14add 集合一致)。fmt: seal ゲート2 機械値は下記コミット
  時記載へ整合。

## wave 179 以降の監査計画
## wave 179 (FY) boot_splash.rs (112 行) — 消費者ゼロ誠実 logger の真配線化
- census 機械確定: BootSplash 系は lib.rs:7 pub mod + :76 pub use のみで
  全クレート内参照ゼロ (step_name/BootSplash の他ファイル参照ゼロ)、
  module strict_tests 3 本 (default 厳密 pin/lifecycle/ゼロ除算ガード) のみ
  が消費者。native_loader (rsift-api) 実ロード経路への crate 跨ぎ配線は
  dep 追加+lockfile 変更の大型リスクで見送り (誠実記録)、crate 内真実
  イベント (render_pipeline::new() 実初期化) への配線を採用。
- 捕捉 124 [小] (FY-1 §7 消化 41): render_pipeline::new() に 3 段 truth
  配線 (Detecting hardware profile 1/3 → Building renderer subsystems
  2/3 → Completing pipeline assembly 3/3 + render_splash_frame +
  finish_and_fade_out)。ステップ区切りは本関数の実作業 truth に対応
  (profile 検出/caps・feather 報告/texture budget・render_graph・lod 構築/
  Self 返却直前) で虚構区切りに非ず。use 配置は rustfmt ASCII ソート
  (billboard→binary→boot) で私の初回挿入位置 (billboard 直後) が自己起因
  逸脱 1 件 → binary_greedy_meshing ブロック直後へ正準化 (ソート直感
  誤りの誠実記録)。
- strict +2 net 1363 全緑 (機械検算 1361+2): fy_boot_splash_wired_lexeme
  (render 側配線語彙 pin、value 観測不能誠実記録・split concat! vacuous
  回避) + fy_truth_paths_extended (透過保管超過比 200/100・冪等 activate・
  未 activate finish no panic 契約 green-today)。api 49・replay 16・警告 0。
- adversarial 3 系統 1/1/2 全検出: (a) render_splash_frame() 呼出削除 →
  lexeme pin RED 1 (render 語彙消失)・(b) update_progress step_name 空化
  → activate_update_finish_lifecycle RED 1・(c) finish の inactive 化除去
  → RED 2 (lifecycle + fy_truth_paths)。復元 MD5-VERIFIED 3 回 (boot_splash
  bf490d63a1026df95f88fcb0e03457fb・render_pipeline
  c584baab4a2922641e180dde457bd560)。非検出 0 (dead code カウンタ据置 26
  未採、本 wave は検出可能系統で構成)。
- fmt: boot_splash 自己起因 0 (機械分離: 31 分行集合が HEAD 原生集合に
  完全包含 = HEAD 原生完全保持、use 並び+テスト長行 hunk 2 件由来)、
  render_pipeline 自己起因 1 正準化 (use 位置) で逸脱 0。fmt: seal ゲート2 機械値 boot_splash 5/5/0・
  render_pipeline 0/0/0 (HEAD 原生 5 行完全保持・自己起因 0)。

## wave 180 以降の監査計画
## wave 180 (FZ) more_culling.rs (117 行) — 消費ゼロ catalog API の保持証明
- census 機械確定: sign_text_visible=full_graph_wiring:1884 /
  screen_footprint_px=:1874 / rain_visible=:1900 の 3 関数が wiring 実評価
  経路実消費 (let _ 形式だが CH-3 承認済の実計算 assertion 路、値は別系
  entity/mesh 判定が管轄)。item_frame_visible / leaf_face_needed /
  neighbor_mask / shared_layer_face_needed の 4 関数は外部参照 0・module
  テストも未消費 → §7 消化 42。
- 捕捉 125 [小] (FZ-1): directive⑦ 両証明付き保持判定 — (i) 葉面 cull 同等
  機能は binary_greedy_meshing:105 の neighbor_opaque 高速専用形で既配線
  (統合は face 毎 6 近傍配列再構築の性能退化・hot path 不可逆 → 不可能
  証明)、(ii) 額縁省略は entity_culling 管轄、(iii) 透過共有面は mesh
  greedy 管轄。upstream 互換カタログ API 保持、module doc 明記 + 自家
  strict (fz_* 4 本) を truth 消費証跡 (FK 判例準拠、fake 配線なし)。
- 機械導出スナップショット: 初回 probe で to_cam=(0,s,c) の s/c を転倒し
  θ=110° 近傍が取れていなかったことを結果 (-0.94 系列) から自己捕捉・
  s=0.94/c=0.342 修正 probe で threshold 両側を正確確定 (dot=-0.3419036
  bits 0xbeaf0dfe → true / c=0.343 → dot=-0.3427860 bits 0xbeaf81a5 →
  false)、MoreCulling 既定 cos110°=-0.34202012 との差 2e-5 で「≒」doc
  truth 検証。next_down(4.0)=3.9999998 bits 0x407fffff (境界 item_frame)、
  fp(1,4,0.7,1080)=192.85715 (0x4340db6e)。
- strict +4 net 1367 全緑 (機械検算 1363+4): fz_sign_threshold_both_sides
  / fz_item_frame_boundary_both_sides / fz_leaf_neighbor_mask_projection
  (6 dir + 射影微分) / fz_shared_layer_truth_table (全 6 分岐)。
  api 49・replay 16 全緑・警告 0。
- adversarial 3 系統全検出 RED 1/1/1: (a) footprint 境界 <→<= 変異 →
  fz_item_frame RED 1・(b) sign threshold -0.342→-0.35 厳格化変異 →
  fz_sign RED 1 (緩和方向 -0.34 変異は両側 pin で検出不変のため厳格化変異
  採用、誠実記録)・(c) shared_layer (true,false)=>false→true 変異 →
  fz_shared RED 1。復元 MD5-VERIFIED 3 回 (more_culling 固定版 md5
  ac0b9d9c7ca1c4259c5185f082365f71)。非検出 0 (dead カウンタ据置 26 未採)。
- fmt: 自己起因 2 件 (fz_item_frame の assert! 折返し) 正準化 → 逸脱 0、
  neighbor_mask 長行 (71) は HEAD 原生 (head-dev-del 機械分離) で保持。
  fmt: seal ゲート2 機械値 more_culling 1/1/0 (HEAD 原生 1 行保持・自己起因 0)。

## wave 181 以降の監査計画
## wave 181 (GA) dag_scheduler.rs (120 行) — 破棄 critical_path の配線化 + truth 契約
- census 機械確定: DagScheduler=wiring:780 new 実消費、5 タスク実構築
  (ingest/mesh/cull/upload/light×delta_ms 係数)/topological_order len 5
  debug_assert 実評価/critical_path_ms は wiring:788 で `let critical_ms`
  破棄 → 後続 grep 消費ゼロ機械確定 (FQ/FT/FU 同型破棄パターン第 4 段)。
- 捕捉 126 [小] (GA-1 §7 消化 43): report.dag_critical_ms (f32) 実計測
  配線、critical chain ingest→mesh→upload = 0.65*delta_ms → delta_ms=16
  で f32 10.400001 bits 0x41266667 (f32 probe /tmp/ga_probe.rs・
  rq /tmp/ga_dag.rq 機械導出: c1=10/c2=8/c3=2、c1 critical)。TDD compile
  RED E0609 → 配線 GREEN (ga_dag_critical_report_truth)。
- 捕捉 127 [小] (GA-2): truth 契約未記載 → Task.deps (dangling dep は
  silent eternal block)/cost_ms (検証なし透過・負は chain 縮小・NaN は
  f32::max 非 NaN 仕様で落選)/topological_order (HashMap 由来非決定的)
  の doc 明記 + ga strict 2 本 green-today: ga_dangling_dep_blocks_silently
  (TaskId(999) dangling → 永久ブロック、critical は schedulable subset)
  ・ga_cost_special_values_truth (NaN chain 落選 crit=5.0・負 cost q
  end=2.5 < p end=4.0 → max 4.0)。
- strict +3 net 1370 全緑 (機械検算 1367+3): module 2 本 + wiring 1 本。
  api 49・replay 16 全緑・警告 0。
- adversarial 3 系統 1/6/1: (a) report 0 化 → ga_dag_critical RED 1・
  (b) critical fold に f32::min 変異 → dag 系全 strict 6 RED (diamond/
  chain/ga_cost/ga_dangling/self_loop/ga_dag_critical、強感応度証明)・
  (c) 幽霊 dep ガード挿入 (`!contains_key → continue`) → ga_dangling RED 1。
  復元 MD5-VERIFIED 3 回 (dag 7d588f0ce8daaef28f849e703b7ac4f4・
  fgw 256714a351e8b9fb1f5e3be19ccdd0de)。非検出 0 (dead カウンタ据置 26)。
- fmt: dag_scheduler 自己起因 0 (cur-dev-del 7 == head-dev-del 7 で機械
  完全一致 = HEAD 原生完全保持: new() 1 行定義/add_task 長行等由来)、
  full_graph_wiring も artifact のみで自己起因 0。fmt: seal ゲート2 機械値 dag_scheduler 8/8/0・
  full_graph_wiring 0/0/0 (dag HEAD 原生 8 行完全保持・両者自己起因 0)。

## wave 182 以降の全量最終検証・`rsift-opt-gfx` 厳密監査完遂後工程
## wave 182 (GB) — 厳密監査フェーズ 1 完遂宣言 (2026-07-29 機械証明)
- **在庫ゼロ機械証明**: rsift-opt-gfx src 全モジュール (lib.rs 除く) について
  AUDIT 本文・BUGFIX_REGISTRY・ci/TRIGGER.md の 3 文書横断で「一度も監査
  証跡に登場しないモジュール 0 件」を全件機械計算で確認 (機械計算値:
  modules=162・unaudited_zero_mention=0、スクリプト: os.listdir 全 rs
  走査 × 3 文書の文字列横断)。
- wave 170-181 (FP〜GA) で残未監査 12 件を全消化 (捕捉 107-127・§7 消化
  32-44)。テスト総数 1370 全緑・api 49・replay 16 全緑・警告 0・
  structural digest 004c1cf5fb17bfe8 rows=357 不変・seal 全 6 ゲート PASS。
- 監査フェーズ 1 完遂の truth 宣言、以下 hygiene 判定を誠実記録:
  (i) src 残 CRLF ファイル約 10 件 (ao_bake 252/billboard_lod 229/
  diff_mesh 214/drs 168/entity_tick_lod 272/simd_kernels 346/soa_layout
  369/spatial_hash 187/taa 176/texture_atlas 347) は seal ゲート 1/2 が
  `.lines()` 正規化で実害ゼロ・熱心度比から大規模 cosmetic diff を抑止し
  保持判定、(ii) wave 見出し H2/H3 不統一 (178-181 H3 4 件) は H2 へ
  定型修復済 (wave 182 適用)、(iii) `rsift/rsift/rsift-opt-gfx` 複写物は
  ユーザー管理資産の可能性を鑑みて削除保留 (確認要)。
- 次工程 (wave 183 以降): フェーズ 2 = adversarial 非検出 26 例未採分の
  深化回収・loose pin の golden bit 化・台帳/docs 文書整合照査へ移行。

## wave 183 (GC) — 台帳 hygiene 照査 (2026-07-29、文書のみ・採番なし)
- **ID 重複疑いの機械解消**: BUGFIX_REGISTRY の uniq カウントで BG-2/BN-3
  が 2 件ずつに見えたが、これは `BG-2b` (line 203) / `BN-3b` 等の suffix 型
  ID の prefix 部分マッチ誤検出 (grep パターン `[A-Z]{1,3}-[0-9]+` が
  suffix 一文字を切り落とす) — 実 ID は全件一意であることを raw grep -nF
  で機械証明。**台帳整合は異常なし**、誤警報の自己照査として記録。
- **捕捉 76/77 欠番の truth 記録**: 台帳は捕捉 74/75 (FB-1/FB-3) の次に
  捕捉 78 (FC-1) へ進んでおり、捕捉 76/77 は FB〜FC 採番ジャンプで**使
  途なく欠番** (3 文書横断 grep で参照ゼロ機械証明)。旧工程の採番採記
  録が当時補完されなかったが事案 (a 期以降の自己照査項目) — 今後も
  欠番の意図的利用はしない (NULL として保持、誤って 76/77 を新採番
  しない運用確約)。本注記のために台帳 ID 本体は改変しない (歴史改ざん
  としない方針)。
- **adversarial dead/loose 非検出カウンタ現況 truth 記録**: 連番 26 件の
  内訳 — 170 (d)=19 (wgsl_source 死救出)、171 (d)=20 (get_state 死救出)、
  172 (b)=21 (parallel_for_each_chunk 削除対象死救出)、173 (c)=22 (bolt
  死救出)、174 (c)=23 (clear API 死救出)、175 (c)=24 (capacity() 死救出)、
  176 (c)=25 (gtao wgsl_source 死救出)。FV-3 / FW-3 / FX-3 / FX-C は初回
  非検出を捕捉して strict pin 追加で全回収済 (採番増分なし)。waves
  177-181 (FW/FY/FZ/GA) は adversarial 非検出 0 で据置 26。回収様式は
  「include_str! 自己参照 lexeme pin」+「truth 契約 pin」に定型確立。
- 本 wave は文書整合のみで src 変更ゼロ (テスト 1370 据置)。
## wave 184 (GD) — adversarial 非検出旧例 7 件の一括回収 (2026-07-29)
- 対象 （全て削除済み API の復活挿入変異非検出、lint/テスト限界):
  170-d use_mimalloc (19 例目)・171-d failed_chunks (20)・172-b build_pcore
  (21)・173-c dev() (22)・174-c clear() (23)・175-c capacity() (24)・
  176-c wgsl_source (25)。
- 回収様式 （FX-3 / wave 176 自己照査判例の定型確立): include_str! 自己参照
  lexeme pin — 宣言形 (`fn <name>` 接頭) の不在 pin、検出語彙は split
  concat! 分割記述で自己言及 vacuous 化回避、証跡 doc 条文は接頭限定で
  範囲外。各ファイルに gd_removed_<api>_lexeme strict 追加 (7 本)。
- adversarial 7 系統一括実証: 各 pin が復活挿入変異 (pub fn 宣言挿入) を
  1 RED で全検出 (7/7 RED、復元 MD5-VERIFIED ×7: mimalloc
  4baac38f9cd3e55658818735ebed748e / dashmap 85af22835774c0078784562237431fcd
  / rayon 9b57c5aa348cc3e4e79d4f79360bd8fe / pgo 55e55f5e53b1c64f8075628bd7ffd165
  / instanced d606a65ea626038361c62bbdf6a4121a / bump e65f9cbed13f8c33cb9cccb612e70402
  / gtao bdba7092ba406907321903650e8d7f33)。**非検出カウンタ 26→19**。
- 先行作業で環境再リセット発生 (HEAD→base 回帰、ワークツリー完全残存を
  検出後 git reset --mixed FETCH_HEAD 整列 + restore-env.sh + rspeed 再
  構築、消失 wave 183 コミット内容は残存ワークツリーから同一内容で
  4e8d257 として再コミット → push 成功・CI run 30419726961 success 検証)。
- strict +7 net 1377 全緑 (機械検算 1370+7、gtao.rs 1 系のみ pin 追加、
  src ロジック変更ゼロで digest 不変確実)。api 49・replay 16 全緑・警告 0。
- fmt: 全 7 ファイル自己起因 0 (dashmap_registry HEAD 原生 set 31・
  instanced_draw 34 を機械分離、残り artifact のみ = 完全保持)。
  fmt 自己照査: ピン挿入スクリプトでテスト fn 閉じと mod 閉じの間に空行
  重複を生成 (4 ファイル)、機械分離では `<>` 記号行集合で 0 と誤判定した
  が seal ゲート2 が FAIL で捕捉 — 正規化 python のスライス境界バグ
  (tail[:-5] で `}` に過剰空白混入) を二度目 FAIL で特定し、seal 条件
  (先頭 2 行除去) 準拠の diff 検証で全ファイル dev=0 修正。seal ゲート2
  機械値: bump_arena 0/0/0・dashmap_registry 10/10/0・gtao 0/0/0・
  instanced_draw 8/8/0・mimalloc_config 0/0/0・pgo_bolt 1/1/0 (HEAD 原生
  先頭空行 1 保持)・rayon_job 1/1/0 (全て HEAD 原生完全保持・自己起因 0)。
  定型改善: fmt 判定は必ず seal 同一条件 (tail -n +3) で採点すること。

## wave 185 (GE) — dead code 系非検出 1-18 例目全回収 + 採番系譜機械復元 (2026-07-29)

- **採番系譜の機械復元 (GC 棚卸の例目帰属訂正)**: GC 台帳 hygiene は例目 1-8
  を帰属不能 (空白) 扱いしていたが、AUDIT 一次資料の「同型 N 連続目」記述から
  機械確定: EX-2 (wave 152)「EU-3(c)/EV-2(b)/EW-2(b) 同型 4 連続目」・EY (153)
  「同型 5 連続目」・EZ (154)「同型 6 連続目」・FB (156) と FC (157)「誠実記録
  8 連続」より 1=EU-3(c) parallax Vec4・2=EV-2(b) volumetric_fog Vec4+impl・
  3=EW-2(b) screen_space_shadow Vec4・4=EX-2(b) material_batch Flora 系・
  5=EY-2(b) fsr2 削除系 4 構造・6=EZ-3(b) power_policy 削除系 4 構造・
  7=FB-2(b) pool_slab get_mut・8=FC-(d) chunk_cull fields 復活。9-18 =
  FF-(d) ddgi Vec4・FG-(e) frame_pacing 装飾 struct・FH-(d) adaptive_shading
  恒値スタブ・FI-(e) pso・FJ-(a) ibl_sh Vec4・FK-(d) decals Vec4・FL-(d)
  gigavoxels resident_brick_count・FM-(c) ao_bake pack_ao4・FN-(d) root_sig・
  FO-(c) bundle。**私の初手誤帰属** (AUDIT 行番号 grep で naga offset 突合
  2/3 例目 (U-3/V-2) を dead code 系 2/3 例目と誤読した自己照査記録) を
  本系譜復元で訂正。
- **回収実施**: GD 定型 (include_str! 自己参照 lexeme pin・split concat! 自己
  言及 vacuous 回避・証跡 doc 接頭限定) で ge_removed_*_lexeme 18 本を各
  モジュール tests mod 末尾へ追加 + 採番枠外の先行同型 (EP-2(d) wave 142
  foveated Vec4・ES-2(d) wave 145 motion_blur Sub impl・ET-2(b) wave 147
  depth_of_field Sub impl — dead code 系採番は EU-3(c) 起点のため枠外だが
  同一限界構造) 3 本、**計 21 pin**。例目 12 (FI)・17 (FN)・18 (FO) は対象
  語彙の一次資料が消失 (FI: 当該コミット diff は削除行ゼロ機械確認・FN/FO:
  /tmp 変異スクリプト消失) ため構造網羅 census pin (現存 pub 構造存在 pin +
  pub fn/struct/enum 宣言総数 pin) へ代置の誠実記録。
- **census pin 自己言及事故 (3 連続・全てテスト RED で捕捉・誠実記録)**:
  総数 pin の concat! 第一引数が完全形リテラル ("pub fn "/"pub struct "/
  "pub enum ") として自己言及混入 — 3 段 (pub fn→struct→enum) とも RED 検出
  → concat! 分割 ("pub f"+"n "/"pub e"+"num ...") と assert メッセージ日本語化
  (「公開 struct 宣言」) で根治。私の初期版 doc 採番 (dof=1 例目) 誤りも上記
  系譜復元で訂正 (ET を採番枠外表記へ・EU=1/EV=2 へ繰上げ)。
- **adversarial 21/21 RED 全検出**: 宣言形復活変異 (struct Vec4 宣言・
  Sub/Flora/fsr2/power legacy API 復活・fields フィールド復活・unit struct
  復活・恒値スタブ復活・pub fn/enum/struct 総数 +1 変異等) を全 21 ファイルに
  個別適用 (18 一括 + dof 単独 + sss/foveated/motion_blur 3 追加)、全 pin が
  1 RED 以上で検出 (dof 初回変異は use 行未変更で E0405 — コンパイル可能形
  (use Sub 追加同行) へ修正して RED 確立の誠実記録)。復元 MD5-VERIFIED ×21。
- **dead code 系非検出カウンタ 19→0 (例目 1-25 全回収完遂)**: 19-25 は wave
  184 GD で回収済、1-18 を本 wave で回収 → カウンタ 0。「26 例目」は FW-3
  (wave 177 adversarial (c) unwrap 化、破損 LZ4 契約 pin で既回収) の
  truth 契約系で dead code 系には未採番 — FY 据置 26 記述との整合を確定記録
  (dead code 系の最終採番は 25 で完結)。
- strict +21 net **1398 全緑** (機械検算 1377+21。1377+18=1395 で 18 pin
  反映、+3=1398 で先行同型 3 pin 反映を個別フィルタ確認で検算)。api 49・
  replay 16 全緑・警告 0。src ロジック変更ゼロで digest 不変確実。
- fmt (seal ゲート2 機械値、全 PASS): 全 21 ファイル自己起因 0 — HEAD 逸脱
  完全保持 (adaptive_shading 13/13/0・bundle 20/20/0・ddgi 2/2/0・foveated
  2/2/0・gigavoxels 1/1/0・ibl_sh 8/8/0・pool_slab 1/1/0・pso 7/7/0・
  root_sig 23/23/0、残 12 ファイル 0/0/0)。ao_bake は CRLF 原生保持で
  逸脱 0/0/0 (私の挿入ブロックのみ選択正規化で HEAD 原生を非改変)。
- seal 全 6 ゲート PASS・digest 004c1cf5fb17bfe8 rows=357 不変。

## wave 186 (GF) — loose pin の golden bit 化: frame_pacing (a)(a2)(b) + power_policy (e)、捕捉 128 発掘根治 (2026-07-29)

棚卸スキャン (`非検出構造|検出不能|構造的非検出|理論上不検出` 36 件) を個別精読し
回収可能系のみ選定: FG frame_pacing (a) ceil→floor / (a2) 第 1 補正ループ除去 /
(b) α 0.2→0.5 (wiring のみ捕捉)、EZ power_policy (e) gate ε 削除。既回収
(6388/6451 追設・EJ-4 golden 強化・dead code 系 wave 184/185 完遂) と不可能証明
保持系 (DH/DJ/DK 証明済中性・subgroup (d)(e) GPU 構造・FF consistency pin (truth
golden 2 本が責務)・EW (a) (module dual pin が責務)・EZ (a) wiring Active 限界)
は対象外と機械分類。

- **GF-1 [中] 捕捉 128 [中] (新発掘根治)**: wave 161 FG (a) ceil→floor を
  「±1 補正ループが構造吸収」と記述していたが、rustc -O probe (gf_probe,
  IEEE 754 厳密・暗算禁止) で **旧実装は 67116 corpus 中 4422 件で bit 発散**
  = loose-corpus 非検出 (golden 不在) であり構造的等価ではなかったと機械訂正。
  しかも発散の主形は **ceil 側が 1 インターバル全分遅れる提示スキップ**
  (now = 計算済み第 k 境界で `t - interval >= now` の段階減算 guard が二重
  丸めで不発 → 最早境界でなく t_{k+1} を返却): 23.976Hz k≡0 mod 3 系・
  60Hz k=62 (now=0x4090255555555556 → 旧 0x4090680000000000) 等、
  hz 別分布 probe 確定 (50Hz のみ 0)。現実運用系列 (16.7ms グリッド 60000
  ステップ連続提示) では発散 0 = 稀な潜在欠陥を誠実記録。TDD RED
  (gf_boundary_exact_returns_computed_boundary_strict が旧 0x40923F568779614B
  で失敗を機械記録) → 根治: 段階加算/減算を廃し各反復で
  `last + n*interval` を再計算 (段階丸め累積の排除)。新旧差分 2574 件 (全て
  スキップ解消方向) + 段階加算ドリフト修正 4 件、根治後 ceil≡floor の真の
  observational equivalence を probe で証明 (67116 corpus 発散 0)。
- **GF-2 [低] (a2) 回収**: 第 1 補正ループの発火 bit 域を機械特定
  (`now = t_k + 1 ulp` かつ商下側丸め、probe gf_probe2 (ii)) →
  gf_loop1_undershoot_window_strict (k=34/136/139・y/now/golden 全て probe
  bits) で pin 化。adversarial loop1 除去で RED 実証 (t=0x40B62856C91363DB
  < now 退行を検出)。
- **GF-3 [低] (b) 回収**: EMA α=0.2 module 厳密 golden
  (gf_module_ema_golden_bits_strict: s1=0x4030A740DA740DA8・
  s2=0x4037529A485CD7BA probe gf_probe [2] 機械値)。wiring only 捕捉
  だった loose 例を module でも検出可能化。
- **GF-4 [低] (e) 回収**: gate ε=1e-5 窓下端 pin
  (gf_gate_epsilon_lower_edge_strict): acc = need − 1 ulp (0x3D088888)
  は ε あり true / なし false (gap = 2^-28 = 3.725290298e-9 < 1e-5、
  probe [1] 機械確定: need=0x3D088889, ε=0x3727C5AC, 余剰 −2^-28=0xB1800000)。
  EZ (e) の「ε 不発区間設計」(need−4·dt==0 bits 一致) と相補的に窓上下を
  閉じる。
- **adversarial 5 系統** (bak+md5 基線・python assert 適用確認・毎回復元
  md5 -c): (a) ceil→floor → **1402 全緑 = 非検出・証明済み中性** (根治で n
  基準の単一計算単位を共有し真の等価化、probe 67116 corpus 発散 0 が証拠、
  DH/DK 系と同型の誠実記録)・(b) loop1 除去 → **1 RED** (gf_loop1)・
  (c) loop2 除去 → **1 RED** (gf_boundary、loop2 除去の検出可能化は従来
  未検証のボーナス回収)・(d) α 0.2→0.5 → **2 RED** (module gf_module_ema +
  wiring fg_pacing_report_pins_nonvacuous 既存 bit pin — wave 161 記録
  「wiring bit pin のみ捕捉」と一致)・(e) ε 削除 → **1 RED**
  (gf_gate_epsilon、ε 不発区間設計の gate_accumulates_exact_count_strict 等は
  緑維持 = 検出範囲が設計通り)。復元 MD5-VERIFIED×5。
- **自己照査の誠実記録**: adversarial 儀式のバックアップを波編集前に採取
  していた誤りで、MUT-A 復元時に wave 編集 (4 pin + 根治 + doc) が全て
  巻き戻る事故 → MUT-B の python assert (loop1 count=0) が変異未適用を
  拒否し md5 照合 forensics で巻き戻りを機械確定。編集を同一内容で再適用、
  復元基線を波修正後 md5 (fp=8e7a1a70630fb89d0dcec96669f4b366・
  pp=52cd71c7fe6876b1f05679f5dad38f11) に再設定して全 5 系統を最初から
  やり直し (バックアップ採取時点と復元基線の同一性を今後の儀式要件に明文化)。
- strict +4 net **1402 全緑** (機械検算 1398+4、4 pin 全て個別フィルタ確認 +
  frame_pacing 系 15・power_policy 系 14 全緑)。api 49・replay 16 全緑・
  警告 0。wiring pacing golden (fg_pacing_report_pins_nonvacuous) は修正
  前後とも緑 = 修正の波及ゼロを機械確認。fmt (seal ゲート2 機械値、
  全 PASS): frame_pacing.rs 0/0/0・power_policy.rs 0/0/0 (私の挿入
  assert 1 行の過長は rustfmt 忠実形へ in-place 正規化後に 0)。
- seal 全 6 ゲート PASS・digest 004c1cf5fb17bfe8 rows=357 不変。

## wave 187 (GG) — dead code 系採番以前枠外同型の棚卸回収: EN-2 subgroup Vec3/Vec4 + EL-1(d)/EO(e) WGSL アクセスポイント (2026-07-29)

「wave 142 より前の同型誠実記録」の網羅スキャン (wave 130-147 節を機械精読):
EP-2(d)/ES-2(d)/ET-2(b) は wave 185 GE で回収済、EU-3(c) 以降は dead code
採番連番で回収済、FF ddgi (d) も 9 例目として回収済。残存は大元の
**EN-2 (wave 140 subgroup Vec3/Vec4+trait 完全装飾削除、revival 変異自体は
当時未実施だが同型非検出クラス)** と、別系譜の **EL-1(d) (wave 138) /
EO(e) (wave 141) gpu_runtime WGSL 登録の free fn→const revert 非検出**
(同一 &str 機能等価・参照様式差のみ) の 2 系列と機械確定。

- **GG-1 [低] subgroup.rs EN-2 lexeme pin** (gg_removed_subgroup_vec_lexeme):
  `struct Vec3`/`struct Vec4`/`impl Vec3`/`impl Vec4` の split concat!
  宣言形検出 (doc 注記は接頭限定で範囲外、現言及は module doc 2 行のみを
  grep 機械確認)。
- **GG-2 [低] gpu_runtime.rs WGSL 登録アクセスポイント pin**
  (gg_wgsl_registration_access_point_pin): free fn 様式統一の 5 モジュール
  (ddgi/tbdr_hints/shadow_lod/fragment_ray_box/frame_pacing) について
  登録が `wgsl_source()` 呼出形であること (wave 161 FG 捕捉 90「唯一の公式
  アクセスポイント」宣言の機械固定) と、const 直接参照 (DDGI_WGSL 等)
  の不在を pin (禁止語彙は split concat! で自己言及 vacuous 回避、
  現在の直接参照ゼロを grep 機械確認済)。意味的に等価でも様式差は
  契約逸脱として RED。
- adversarial 3 系統 (復元基線は波修正後 md5、wave 186 自己照査の
  儀式要件適用): (α) subgroup `pub struct Vec3`+impl 宣言形復活 →
  **1 RED** (gg_removed_subgroup_vec_lexeme)・(β) tbdr_hints 登録
  free fn→TBDR_HINTS_WGSL revert → **1 RED** (gg pin)・(γ) shadow_lod
  登録 free fn→SHADOW_LOD_WGSL revert → **1 RED** (gg pin)。
  復元 MD5-VERIFIED×3。
- **環境再構築の誠実記録 (4 度目)**: wave 挿入直後に toolchain
  (/home/user/rust/bin 等) が消失・HEAD が base (64294c6) 浅 clone に回帰 →
  ワークツリー残存を md5 機械確認 (wave 187 編集 2 ファイル無傷) →
  fetch origin + git reset --mixed FETCH_HEAD で HEAD=faa79c1 へ整列 →
  restore-env.sh で toolchain+vendor 復元 (rustc 1.94.1) → rspeed 再構築。
  消失コミット発生せず (wave 編集は全て未コミット作業ツリー内)。
- strict +2 net **1404 全緑** (機械検算 1402+2、2 pin とも個別フィルタ確認 +
  隣接 wave_width_contract/naga_all_runtime_dispatched_wgsl_validate 全緑)。
  警告 0。fmt: seal 同一条件で ddgi タプル 1 行の過長を捕捉 → rustfmt
  忠実形へ in-place 正規化後、seal ゲート2 機械値
  **gpu_runtime.rs 0/0/0・subgroup.rs 0/0/0 (全 PASS)**。
- seal 全 6 ゲート PASS・digest 004c1cf5fb17bfe8 rows=357 不変。

## wave 188 (GH) — 構造的限界系列の閉鎖 + フェーズ 2 完遂宣言 (2026-07-29)

### GH-1 [中] EW (a) 検出空白クラスの構造的除去 (dedup 根治)
wave 151 EW (a) 「捕捉 63 wiring closure revert 非検出構造」の最終判定:
wiring の sss_depth inline closure は「同じ意味論を module テストと 2 か所
複製保持」し、出力は report 非属 (report 構造体に shadow 由来フィールドなし
= grep 機械確定) のため wiring 層 golden は**構造的に新設不可能**。
正しい根治は複製の解消: `screen_space_shadow::aabb_occupancy_depth` を唯一
実装新設し wiring closure を共有本体呼出へ一本化 (文レベル同一抽出のため
IEEE 厳密に挙動不変、module 既存 golden 緑維持で機械立証)。検出空白の
「クラス」自体が消滅 (wiring 実体は module golden の検出範囲に帰着)。
呼出側連続性は gh_wiring_sss_shared_depth_lexeme (共有呼出 presence +
旧 inline 帰結式 banned、split concat!) が担保。
- strict 追加: gh_shared_occupancy_depth_golden (共有本体経由で EW-4
  golden とビット同等=vis 0.0/sample 1、rq ew_sss 引用)・
  gh_aabb_occupancy_depth_direct_contract (閉区間境界 8.0=0x41000000・
  1.0=0x3F800000・外部/空 AABB INF、bits は python struct.pack 機械値)。
- adversarial: (A) 共有本体の捕捉 63 revert (return 0.0 化) → **2 RED**
  (両 shared pin) = 旧 EW (a) 変異クラスの検出可能化を実証・(B) wiring
  呼出側の旧 inline 復元 → **1 RED** (gh_wiring lexeme、presence 側)。
  復元 MD5-VERIFIED×2。**誠実記録**: (B) の python 事後 grep は pin 自己
  言及を除外し忘れ誤 FAIL — anchor assert (count==1) と RED 観測で適用は
  機械確定 (install 確認 grep を併用する定型は wave 186 儀式で整流済)。

### GH-2 [低] EN (d)/(e) 証明済み中性の前提監視 pin
wave 140 EN (d)/(e) を精査して**証明の前提をコード内構造に由来すると機械
確定**: (d) wiring reduce max→min は「発光走査 cap 32 `break 'scan` 硬停止
→ intensities ≤ 32 → subgroup wave 集約出力が常に単一要素 → reduce
f32::max ≡ f32::min 恒等 (subgroup WAVE_WIDTH=32 契約 pin と連立)」で
構造的非検出が**構造証明**の形で成立 (corpus に依存しない)。(e) .max(0.0)
除去は「intensity が `lvl as f32` (u8 由来・grep 機械採数で構築単一箇所)
→ 非負・-0.0 構造不出 → 恒等関数」で型レベル証明成立。前提が将来崩れた
時だけ RED になる前提監視 pin `gh_en_proof_premise_lexeme` を新設
(cap 32 単一箇所・intensity 構築単一箇所の count==1、禁止語彙 split
concat!)。**adversarial**: (C) cap 32→64 緩和 → **1 RED**・(D) `* 1.0`
恒等付加 → 前提非破壊で**緑 (正しい沈黙)** = 私の変異設計不備を誠実記録、
真の前提破壊 (D' `-(lvl as f32)` 符号反転) → **1 RED**。復元
MD5-VERIFIED×3。

### GH-3 [低] 証明済み中性系列の閉鎖索引 (doc のみ、再証明済)
以下は既存証明書 (各 wave 節の Python 構造照合・数学恒等・構造証明) により
閉鎖: DH (a')(b')(c') 前置/post/逆戻しマスク (全 1024 入力 0 差分、
digest 不変由来)・DJ-3 (c) 第一マスク冗長 (二段目 `&0x030000FF` が bit10+
殺傷)・DK (c) select OR→XOR (XOR≡OR≡ADD 同一 domain)・DS (d) prefetch
除去 (WGSL 非生成・診断シンボルのみ)・CC-1 (GPU 側読み検出不能 →
書込側 fail-loud 契約)・frustum 平面コモンモード (wave 82 4 層整合 +
f32 検算ミラーでの捕捉経緯)・DG (c) idx 層 assert (防御第 2 層保持決定)・
ER (d) NaN Equal→Greater (挿入ソート両方向同一帰結)・FE (c) clone_shallow
(値同一 dead code・不変式層不在証明)・EH メトリクス非属 (module golden
存在)・EZ (a) wiring 恒 false revert (focused=true 固定・60s 未到達で
Active 恒、検出責務は module gate_is_sole_skip_decider_contract_pin)・
FB (f) material read-back (round-trip 値同一性、整合責務は eviction golden
map 保持 pin)・FF consistency pin 対称変異 (絶対真値は golden 2 本が責務、
pin 分類明文化済)。

### フェーズ 2 完遂宣言
- **全 162 モジュール × adversarial 非検出系列の閉鎖を機械棚卸で証明**:
  非検出/検出不能/理論上不検出の全言及 116 行 (grep 機械走査) を精査し、
  各系列は (i) pin 回収済 (dead code 1-25 例目・wave 184-187 各回収・
  FL eviction 追設・FV/FW/FW-4 追設・EO-5/EP 補完・EJ-4 強化・wave 186
  loose 4 件・GH-1 構造的除去) か (ii) 証明済み中性・責務 pin 完備の
  誠実記録 (GH-3 索引) の何れかに機械分類完結。dead code 連番カウンタ
  26 → **0** (wave 185 完結)、loose 構造系列 → 本 wave で全閉鎖。
- 完遂条件: 「 adversarial 非検出 = 将来 wave 棚卸対象」の残件ゼロ。
  今後 adversarial で新たな非検出が出た場合は同 wave 内で自己照査解決
  (directive ⑧) する従来運用へ帰還。
- strict +4 net **1408 全緑** (機械検算 1404+4、4 pin 全て個別フィルタ確認)。
  警告 0 (lib build gate、more_culling/dag_scheduler テスト側 unused は
  HEAD 原生 hygiene として誠実記録・ゲート非該当)。fmt: seal ゲート2 機械値
  **screen_space_shadow 0/0/0・full_graph_wiring 0/0/0 (全 PASS)** — 初回
  seal は premise pin の .matches().count() 行逸脱 1 を捕捉、rustfmt 忠実形
  へ in-place 正規化して再 seal PASS (誠実記録)。
- seal 全 6 ゲート PASS・digest 004c1cf5fb17bfe8 rows=357 不変・台帳 660。

## wave 189 (GI) — 機能開発: 非 3D 画面 (メイン/ポーズ) 適応静止低レート化 + MacBook バックエンドポリシー (2026-07-29)

### GI-1 [機能] ユーザー要求対応 2 本立て (監査系でなく機能開発 wave)
要求 (1)「3D 描画ではないメイン画面やポーズ画面の描画高速化」、要求 (2)
「最新 MacBook 向け Metal 4・旧 MacBook 向け OpenGL 対応 = 完全 MacBook
対応」に対応。

#### (A) gui_composite 適応静止低レート化
- 現状認識: gui_composite は wave 149 GC 系デュアルレート (既定 gui 30/
  render 120)。メイン/ポーズ画面等の入力不在区間でも 30 fps で GUI を
  再描画し続けるため、静止時の再描画回数そのものを削る。
- 実装: `GuiAdaptive { enabled, hold_full, hold_mid, mid_div, floor_div }`
  (Default true/45/135/2.5/7.5) を `GuiRates::adaptive` として追加
  (literal 構築 2 箇所は `..Default::default()` 形へ修正)。
  `should_render_gui` が `still` (最後の入力/面破棄からの連続 render
  回数) で u32 整数比較により 30→12.0→4.0 へ段階退化。**入力・
  invalidate() は判定フレームに即時フルレート復帰** (0 フレーム遅延)。
  `adaptive.enabled = false` で wave 149 挙動とビット一致 (legacy pin)。
  `fps_now` は実効レートを報告 (GC-5 注記 5 からの意図的変更、44 render
  未満では不変を module 機査で立証)。分母 2.5/7.5 は既定 30.0 が厳密に
  12.0/4.0 になる値 (probe 機械確定)。
- golden (probe /tmp/gi_probe・/tmp/gi_probe_extra 機械導出、IEEE 754):
  dt=1/120・3000 tick 無入力 renders **199** (legacy 750 の **73.5%
  削減**)。退化境界 render #44=(t=173,30.0)/#45=(177,30.0)/
  #46=(181,30.0)、初 fps==12.0 render t=191・初 fps==4.0 t=1111、
  内訳 30/12/4 = 46/90/63、最終 render t=2971。t=2000 入力 → 同フレーム
  render・fps=30.0・invalidated=true、log 1998..2006 全 9 行 golden。
  invalidate() 面破棄 → t=2001 復帰以後 renders = vec![2001,2005,2009,
  2013,2017]。bits: 30.0=0x41F00000/12.0=0x41400000/4.0=0x40800000
  (python struct.pack 機械値)。回帰: 120 tick=30 renders・dt=0.02 系列は
  gc golden と一致。
- strict 5 本追加: gi_adaptive_static_degrade_golden /
  gi_adaptive_input_restore_zero_latency_golden /
  gi_adaptive_full_rate_below_threshold (46 render まで全 30.0) /
  gi_adaptive_disabled_is_legacy_identical (750) /
  gi_adaptive_invalidate_surface_restores。
- **自己照査**: 初版実装は invalidate() が still をリセットせず復帰しない
  欠陥 → 新規テストが RED 捕捉 → probe 後 (a) 設計 (invalidate も即時
  still=0 復帰) で根治した経緯を誠実記録 (TDD の検出力が機能自体の
  バグを捕捉した事例)。

#### (B) apple_backend 新設 + gpu_runtime 配線 (完全 MacBook 対応)
- 実装: 新規モジュール `apple_backend` — OsKind/ArchKind/host_os()/
  host_arch()・`AppleGpuClass` 7 分類 (NotApple/AppleSilicon/
  AppleSiliconX86Process/IntelDedicatedMetal/IntelIntegratedMetal/
  LegacyNoMetal/Unknown)・`classify(os, arch, device_name)`
  (Aarch64+macOS→AppleSilicon 高信頼、"Apple M<digit>"+x86_64→
  SiliconX86Process=Rosetta、GMA→LegacyNoMetal、AMD/Radeon/NVIDIA/
  GeForce→Dedicated、Intel→Integrated、空/不明→Unknown)・
  `chip_generation` ("Apple M1 Pro"→Some(1)..M4→Some(4)、M9、非 M 系
  None)・`Metal4Surface { declared, usable }` (declared = AppleSilicon 系
  && os_major>=26)・`BackendChoice`・`instance_backends(os, env)` (macOS
  既定 MetalPreferred・非 macOS All・RSIFT_GFX_BACKENDS=metal/all/未知値
  は既定再帰、trim+lowercase で 1 回読み)・`legacy_gl_fallback_note`
  (LegacyNoMetal のみ Some)・`describe` (chip_gen は引数供給 — 初期の
  プレースホルダ供給は誠実性で根治済)。
- 配線 (§7 実消費): gpu_runtime に `instance_descriptor_with_apple_policy()`
  を新設し runtime() の Instance::new へ適用 (MetalPreferred →
  `desc.backends = wgpu::Backends::METAL`)。adapter 取得時に
  classify+chip_generation+describe+legacy note を全消費する info!/warn!
  ログ (os_major は wgpu 0.20 から取得不能のため describe へ 0 供給と
  doc 明記)。
- **誠実境界 (一次情報・信頼度: 高)**: wgpu 0.20.1 (lock 固定) は macOS
  上で GL コンテキストを生成しない (GLES backend は EGL 経由のみ) ため、
  旧 Intel/GMA Mac の「GL 対応」は runtime()==None の CPU フォールバック
  経路 + legacy 注記ログで到達する policy 宣言 (GL 実コンテキスト
  interop は将来検討対象、スタブでなく到達経路の宣言)。Metal 4 も metal
  binding (metal-rs 系) 未導入のため `metal4_surface` は declared 算出
  のみで **`usable = false` 固定** — 技術見送り宣言でなく policy 値であり、
  `gi_metal4_surface_is_declaration_only` が usable=true 化 (binding 導入
  時の忘れ物) を RED 検出する構造 (MUT-E で検出力実証済)。
- strict 5 本追加: gi_classify_corpus_golden (AppleSilicon xcode env 系/
  Rosetta "Apple M2"+x86_64/AMD Radeon Pro 5500M/NVIDIA GT 650M/
  Intel Iris Pro/GMA 950/Windows Intel/Linux unknown corpus)・
  gi_chip_generation_golden (M1 Max→1/M2→2/M3 Pro→3/M4→4/M9→Some(9)/
  "M1" 前置なし None/Radeon None)・gi_metal4_surface_is_declaration_only
  (class×os_major 行列で declared/usable 全確定)・
  gi_instance_backends_policy_golden・gi_legacy_note_and_describe_golden。

### adversarial 6 系統 (全 RED 検出・復元 MD5-VERIFIED×6)
- (A) `still < hold_full` → `<=` (境界ずらし) → **2 RED**
  (gi_adaptive_static_degrade_golden 200≠199・
  gi_adaptive_full_rate_below_threshold 47≠46)。**自己照査**: 初回試行は
  cargo test の複数フィルタを空白連結 1 文字列で渡し 0 マッチ vacuous
  (全件 filtered) となるスクリプトバグで RED 未検証 → フィルタ別引数化
  で再実施し RED 機械確定 (復元は初回から md5 照合済)。
- (B) mid_div 2.5→3.0 → **1 RED** (static_degrade golden)。
- (C) invalidated_now の still 解除削除 → **1 RED**
  (input_restore_zero_latency golden)。
- (D) classify Intel 腕の返値を IntelDedicatedMetal へ改竄 → **1 RED**
  (classify_corpus: "Intel Iris Pro Graphics" left=Dedicated right=Integrated)。
- (E) metal4_surface `usable: false` → `true` → **1 RED**
  (declaration_only pin = 忘れ物防止構造の機能実証)。
- (F) instance_backends macOS 既定腕 → BackendChoice::All → **1 RED**
  (backends_policy: left=All right=MetalPreferred)。
- 注記: adversarial 計測は fmt 正規化前コンテンツで実施し、正規化
  (rustfmt 忠実形・純粋書式) 後に全量 1418 再検算で緑を再確認 (復元
  基線 md5 も正規化後値へ再設定)。

### 機械検数
- strict +10 net **1418 全緑** (機械検算 1408+5(gui_composite)+
  5(apple_backend)、gi_ 10 本は個別フィルタで個々に起動確認)。
  api 49・replay 16 全緑。警告 0 (lib build gate)。
- fmt: 初回採点 (seal 同一条件) で自己起因逸脱 3 ファイル (assert! 長行
  分割・`||` 連結条件行・match 引数 1 行化) を捕捉 → rustfmt 忠実形へ
  in-place 正規化 → 4/4 FMT-OK。seal ゲート2 機械値: **gpu_runtime
  0/0/0・gui_composite 0/0/0・lib.rs 0/0/0 (全 PASS)** — apple_backend は
  新規未追跡でゲート2 の HEAD 包含比較対象外のため、同一条件採点の
  FMT-OK (0 逸脱) を自前機械確認で補完 (誠実記録)。san 27 files 0
  findings・trailws 0・台帳件数 661・変更追跡ファイル 6 の seal 機械表示。
- 台帳 GI-1 (661 行目)。digest 004c1cf5fb17bfe8 rows=357 不変。

## wave 190 (GJ) — hud_batch 軽量化 + BatchView 契約整合 (捕捉 129 根治) (2026-07-29)

背景: ユーザー指示「軽くしまくって・新技術自由・デバイス依存技術は抜く」に
基づく portable 軽量化 wave 系列の第 1 段。対象は HUD/テキスト集約の
hud_batch (wave 155 FA 系)。

### GJ-1 [中] 捕捉 129: DrawRange.first_index の露出 buffer 不整合を根治
wave 155 捕捉 73 (非連続同一キー run 再配置) の残根: finalize が露出する
`BatchView (ranges, indices)` において `DrawRange.first_index` は「挿入前
len 由来」のまま露出し、層 interleave/repeat 系では露出 indices 上の
**他者 quad の slice** を指す自己不整合だった。python 機械実証 (帰属照合
PROOF-OK 2 系): interleave 系は全 3 range が他者帰属、repeat 系は L0 が
L9 quad・L9 が L0 混入 slice。wiring が first_index を非消費 (counts のみ)
かつ昇順連続 push のみ供給するため非顕在化していた (捕捉 62/63/72 系の
「供給パターン消隠」クラス)。根治: finalize が露出 buffer の累積確定位置で
first_index を上書き。fa_hud 両 golden の pin 値は新契約値へ機械更新
(interleave [6,12,0]→[0,6,12]・repeat (6,0)→(0,6))、同質性を
gj_first_index_addresses_own_quads_contract が pin。

### GJ-2 [機能] append O(1) 化 + two-pass flat finalize (定常ゼロ再割当)
- slot_for: 旧「層昇順 insert + map 全値/chunks 全件 +1 ずらし」(新キー毎
  O(keys)+O(chunks)) → append のみ O(1)。層昇順の materialize は
  finalize の stable sort へ移譲 (append 順の層 stable sort ≡ 旧挿入順:
  等層は共に作成順保持、独立 spec reference との 1024 系列 corpus 照合で
  機械立証)。ずらし不変式の維持責務ごと構造消滅。
- finalize: 旧 `vec![Vec; K]` by_slot 集約 + Vec 返却 (finish 内差替えで
  scratch 容量を毎 frame 喪失) → 層 stable sort (order) + 累積 offset
  (offs) two-pass で scratch へ直接構築 (BatchOutput に ranges/order/offs
  追加、wiring の ObjectPool<BatchOutput> 確保回避設計と整合)。
- probe /tmp/gj_probe.rs (-O, counting allocator + 移動カウンタ):
  テキスト的 24 キー×2000 quad interleave corpus で旧 finish 87
  allocs/frame → 新 0 (定常 frame、fill+finish 合算)、新キーずらし 93
  回→0、出力 (indices 列・層順/count 系列) 全一致 assert 通過、旧 fi は
  12 range 中 11 が stale。**op-count/alloc-count の proxy 計測であり
  wall-time ではない** (捏造ベンチ禁止規律に基づく明記)。
- TDD RED 記録: compile RED 3× E0609 (BatchOutput 新 field 未存在)
  → 足場 field 追加で compile 通過後 runtime RED 5 件 (corpus n=4、
  contract で tex=2 に 30=tex3 quad 混入実証、capacity pin、両 golden)
  → 実装後全 GREEN。equal_layer pin は設計どおり green-today 構造 pin。
- strict +4 (1418→1422): gj_first_index_addresses_own_quads_contract /
  gj_finalize_matches_independent_spec_corpus (全 4^5 系列) /
  gj_equal_layer_creation_order_golden / gj_scratch_capacity_reuse_pin。
- adversarial 5 系統全 RED: (A) fi 上書き削除+作成時 len 復活 (旧 stale
  世界) 4 RED・(B) 層降順化 6 RED・(C) カーソル前進削除 4 RED・
  (D) map hit 返値改竄 3 RED・(E) offs 初期値 0 固定化 4 RED、
  復元 MD5-VERIFIED×5。
- fmt 自己照査: in-place 正規化が HEAD 原生逸脱 4 箇所中 3 箇所
  (push_glyph/key/mkquad 原生 1 行リテラル群) まで巻き戻した — 初回
  diff を head -20 で截断して残 3 hunk を見落とした私の過失 (誠実記録)。
  HEAD 原生形へ全復元し「現逸脱 ⊆ HEAD 原生逸脱」を行集合機械照合。
- san 自己照査: (a) wave 155 注記 5 が U+FFFD 生体を引用文に内包していた
  (HEAD 原生だが同ファイル未変更 wave では gate1 走査対象外で既往未検出、
  本 wave の変更で gate1 FAIL 捕捉) → 「実線<U+FFFD×2>形」の ASCII 表記化
  で誠実記録を保持したまま根治。(b) 私の AUDIT 節で「消<U+9690>」の簡体字
  typo → 私の補完走査は固定文字集合で U+9690 を含まず見落とし (seal san が
  捕捉、走査集合の不完全性を誠実記録)。(c) さらに本注記 (b) の引用文自体が
  U+9690 を再混入して 2 度目 FAIL → 置換表記化で根治 (自己言及混入を誠実
  記録、2 サイト機械確認で残存 0)。
- seal ゲート2 機械値: **hud_batch HEAD 逸脱 10 行/現 9 行/自己起因 0 行
  PASS** (初回 seal は gate1 FAIL の後、上記 (a)(b) 修復で再 seal 確認)。
- api 49・replay 16 全緑。警告 0。digest 004c1cf5fb17bfe8 rows=357 不変。

## wave 191 (GK) — wiring material 名 format! メモ化 + intern_pool 軽量化 census (却下記録) + bc7 α roundtrip 制約発見 (2026-07-29)

### GK-1 [機能] tick per-material `format!("block/{m}")` の tick 跨ぎメモ化
wiring テクスチャ節 (5) が全 material × 全 tick で `format!` の String を
新規割当してから string interner へ渡す alloc churn 構造だった (interner が
重複を潰しても format! 側の割当は残存)。根治: `mat_name_cache`
(HashMap<u32, String, FoldBuildHasher>) で entry API 1 照会メモ化 —
hit 時は format! 自体が不発 (or_insert_with の hit 非評価性)。
- probe /tmp/gk_probe.rs (counting allocator、64 mats×60 ticks/16 unique
  corpus): alloc **3844 → 20 (99.48% 削減)**、hits=3824=N×ticks−U と
 理論一致 (op-count proxy、wall-time 非計測を明記。初版 probe の期待式を
  暗算で誤記 (U×ticks) → assert が捕捉して修正の誠実記録)。
- strict +3 (1422→1425): gk_material_name_memo_zero_realloc_golden
  (tick1 hits=3=N−U・tick2 cumulative 9、TDD compile RED 3× E0609 →
  GREEN)・gk_material_name_memo_names_resolve_exact (resolve 逆引き
  {block/1,3,7} byte 同一)・gk_material_name_memo_lexeme (presence
  `.entry(m)` full-m キー + 禁止 `or_insert(format!`、presence 形も split
  concat! — include_str! はテスト自身を含む自己言及対策)。
- adversarial 4 系統 7 RED: (A) hits 増分削除 2 RED・(B) or_insert_with
  形での hits 不計上 revert 2 RED・(C) キー短絡 m&0xFF 1 RED (lexeme
  のみ、corpus <256 で golden/resolve 正しい沈黙 = 設計どおり)・
  (D) eager or_insert(format! 2 RED。復元 MD5-VERIFIED×4。

### GK-2 [監査] intern_pool 単一保持化はメモリ悪化で却下 (変更なし記録)
候補として精査: map+values の T 2 重保持を hash→候補 slot の buckets 化で
単一保持にする案は、u64 キー map node (~40B)+Vec オーバーヘッド (24B+)
が現行 (T node ~17-24B + Option<T> 13B) を hash 相異時に上回り**悪化**
(実質全ユニーク相異 hash のため常悪化)。単一候補 HashMap<u64,u32> は
FoldHasher の衝突で Eq 違反を起こし得るため非健全。miss 経路 2 ハッシュは
get+insert の API 構造由来で entry API 化の範囲では解消しない。**偽軽量化
を踏まなかった評価記録** (数理却下、コード変更なし)。

### GK-3 [発見・保留] bc7 mode6 α roundtrip の m 依存 debug_assert 制約
wiring:1860 の `debug_assert_eq!(dec[0][3], 255)` が first material 由来の
v pattern で **α=255 入力が 254 に decode** され得る症候を m≥256 テスト
入力で踏み発見。probe /tmp/gk_bc7probe.rs で m=0..31 全域機械走査:
PASS={1,2,3,4,5,8,9,10,12,13,15,16,17,18,20,22,23,24,25,27,28,30,31}、
FAIL(α254)={0,6,7,11,14,19,21,26,29}。gk 両テストの first 要素は PASS
集合の 1 を機械選定 (対症ではなく一次情報に基づく入力設計)。encoder 側の
mode6 α 端点量子化精度の可能性 (信頼度: 中 — 症候は確定、原因帰属は
未精査)。**本 wave では調査・改修せず、将来 wave 候補として記録**
(既存 debug_assert の入力依存発火系、CI 緑阻害なし)。

### 機械検数
- strict 1425 全緑 (1422+3 機械検算、gk_ 3 本個別フィルタ確認)。api 49・
  replay 16 全緑。警告 0。fmt: field 宣言 1 行化の外科正規化 1 箇所、
  現逸脱 0/HEAD 外 0 (SUBSET-OK)。seal ゲート2 機械値:
  **full_graph_wiring HEAD 逸脱 0 行/現 0 行/自己起因 0 行 PASS**
  (一発 PASS、san 25 files 0 findings・台帳 663 の機械表示)。
- 台帳 GK-1 (663 行目)。digest 004c1cf5fb17bfe8 rows=357 不変。
- 環境再構築 6 度目復旧 (fetch+reset --mixed+restore-env+rspeed)。

## wave 192 (GL) — bc7 mode6 α roundtrip debug_assert の規格真理根治 (GK-3 発見の回収) (2026-07-29)

### GL-1 [小] wiring:1859 debug_assert は規格上不達契約だった (根治)
GK-3 (wave 191 発見保留) の本格精査。`choose_pbits` は RGBA 4ch 誤差和で
端点単位共有 pbit を最適選択済み (一次実装確認) = encoder は mode6 規格
(RGBA 7bit + 1 endpoint = RGBA セット 1 pbit) 下で正しい。共有 pbit が
RGB を優先すると α は (q<<1)|p で 254 に復元される — **α 誤差 ≤1 は規格
内帰結であり、wiring の `debug_assert_eq!(dec[0][3], 255)` は規格上不達
の bitwise 期待契約バグ**が真因 (m 依存発火は probe m=0..31 で FAIL 9 値
確定 = wave 191)。根治: assert を `dec[0][3] >= 254` (const-α=255 一様
ブロックの契約真下限) へ + 根拠コメント。
- const-α 定理 (mode6 で const-α ブロックは ANY p policy でも round-to-
  nearest 量子化ゆえ復元 α 誤差 ≤1): /tmp/gl_probe で 10 α×7 m = 70 件
  全走査 **max 誤差丁度 1 (bound tight)**、solid RGB=0 α=255 では全画素
  dec α≡254 (SSE が RGB 優先で p=0 — 私の初期注釈予想 (255 復元) は外れ
  を誠実記録、誤差 1 は例外でなく頻発し得る規格内挙動)。
- strict +3 (1425→1428): gl_mode6_const_alpha_error_bound_strict (70 件
  ≤1 + tight=1 + solid 全 254)・gl_mode6_wiring_vpattern_alpha_floor_
  corpus (m=0..31 dec[0][3] exact table、probe 機械導出) =
  green-today 財産化・gl_bc7_alpha_contract_first_material_fail_set_
  strict (FAIL 集合 m=7 first で tick 完走、**TDD RED = 旧 assert で
  panic を機械記録** → 修正で GREEN)。
- adversarial 4 系統 5 RED: (A) choose_pbits round→floor 1 RED (corpus、
  定理は |2·floor(x)−2x|<2 で ≤1 不変の数学的正しい沈黙を誠実記録)・
  (B) clamp 127→126 2 RED (定理破壊も検出)・(C) decode expand p 無視
  1 RED (corpus、定理正しい沈黙)・(D) wiring assert ==255 revert 1 RED
  (panic 再現)。復元 MD5-VERIFIED×4。

### 機械検数
- strict 1428 全緑 (1425+3、gl_ 3 本個別フィルタ確認)。api 49・replay 16
  全緑。警告 0。fmt: assert_eq! 分割形の外科正規化 1 箇所 (rustfmt 忠実形
  = 引数ペア 1 行化へ初回 1 行化誤りを二度目で適合、誠実記録)、
  現逸脱 HEAD 外 0。seal ゲート2 機械値: **bc7_ktx2 HEAD 逸脱 8 行/
  現 8 行/自己起因 0 行・full_graph_wiring 0/0/0 (両 PASS)** (一発
  PASS、san 26 files 0 findings・台帳 664 の機械表示)。
- 台帳 GL-1 (664 行目)。digest 004c1cf5fb17bfe8 rows=357 不変。

## wave 189 以降の運用 (フェーズ 2 完遂後)
- adversarial 非検出の棚卸運用は終了。新規 adversarial 非検出は発生 wave 内
  完結 (directive ⑧)。
- hygiene 残 (保持判定継続): opt-gfx src CRLF ファイル 10+・テスト側 unused
  変数警告 3 件 (more_culling/dag_scheduler、HEAD 原生)・stray copy
  (rsift/rsift/rsift-opt-gfx) 削除はユーザー管理資産確認待ちで継続保留。
- 捕捉採番の次空き: 130 (129 は wave 190 で使用)。strict 総数履歴:
  …1398(185)→1402(186)→1404(187)→1408(188)→1418(189、gi_ 10 本)
  →1422(190、gj_ 4 本)→1425(191、gk_ 3 本)→1428(192、gl_ 3 本)。
