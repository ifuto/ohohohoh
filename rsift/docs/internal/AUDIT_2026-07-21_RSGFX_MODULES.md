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
- 修復 (厳密): 8 隅を厳密 view 変換→透视除算 (凸体の射影 = 隅射影の凸包、
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
  全沉默して棄却 (背面跨ぎ)。
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
  全沉默してピラミッドが欠落する = 非保守。そのため射影を
  **project (test 専用: band 付き、遠く画面外の occludee は遮蔽不可 =
  保守的に可視扱い)** と **project_screen (raster 専用: band 無し)**
  に分割。raster 側の座標爆発は凸包 scanline の ±4 画面防御 clamp と
  書込み範囲の切詰めで処理 (画面外成分は自然に消える、捨てるのは保守)。
- AABB raster は 8 隅厳密射影 (p x M) → monotone chain 凸包 (f64、
  透视射影は w>0 半空間で凸性保存) → scanline で**完全内包 texel のみ**
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
- 編集誤字捕捉: 「解決必须」(中国語混入)・「隙間ゾロ」(ゼロ誤打) の
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
