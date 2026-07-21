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
