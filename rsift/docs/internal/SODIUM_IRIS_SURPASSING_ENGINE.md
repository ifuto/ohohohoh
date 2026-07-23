# Rsift Optimized Graphics (`rsift-opt-gfx`) — 実装・検証ステータスと設計

**Target Minecraft Version: 1.21.11 Edition**
**Architecture: Rust native graphics pipeline (wgpu / Rayon / compute-shader culling)**

---

## 1. Executive Summary — 検証済みステータス (2026-07-21 監査で改訂)

> **改訂理由**: 旧版は「Sodium と Nvidium のレンダリング性能を上回る」「Iris Shaders /
> OptiFine シェーダーパックと 100% 互換」と記載していたが、2026-07-21 のコード監査で
> **実装と一致しない過剰主張**と判明したため、以下の検証済み事実に置き換える。

### ✅ 実装済み・検証済み (ソース・実測で裏付け可能)

| 項目 | 実体 | 検証方法 |
|---|---|---|
| 超圧縮頂点フォーマット | `Quantized12ByteVertex` (**12 bytes/vertex**。`crates/rsift-opt-gfx/src/chunk_mesh.rs:33`、`size_of == 12` をコンパイル時 assert) | 静的 assert + ベンチ実測 (メッシュ C で 303,895 verts / 3,646,740 B = 12.0 B/vertex) |
| マルチスレッド・チャンクメッシング | `MultithreadedChunkBuilder` (`chunk_mesh.rs:86`) + Rayon + `BumpArena` 再使用 | 疑似 MC ベンチで実行・計測 (`docs/BENCH_SODIUM_VS_RSIFT.md`) |
| GPU ドリブン・カリング | `GpuDrivenCullingEngine` (`gpu_culling.rs:102`)。wgpu compute カリング + `draw_indexed_indirect` | WGSL は naga 検証 pass (`gpu_runtime.rs` の恒久ガード 51→50 ファイル)。GPU 実行は examples 経路 |
| Sodium 式アルゴリズムとの比較計測 | Sodium 公式ソース (LGPL-3.0, commit `9d11e9cf`) を読解し **file:line 引用つきで Rust 準拠再現**したモデルとの 8 分野計測対決 | `docs/BENCH_SODIUM_VS_RSIFT.md` (例: ACMR 1.669 → 0.929, 頂点キャッシュヒット率 90.9%) |
| Sodium 式設定 UI + シェーダーパック選択画面 | `SodiumVideoSettingsGui` (`gui_settings.rs:235`)。タブ/トグル/プリセットの状態管理と **box-drawing ログレンダリング** | launcher ライフサイクルから「preview」として実呼び出し (`rsift-launcher/src/lifecycle.rs:212`) |
| シェーダーパックの発見・zip 展開 | `IrisShaderEngine::discover_shaderpacks` / `extract_zip_shaders` (`iris_pipeline.rs:162, 522`)。展開制限つき flate2 ベース | inline テスト + コード監査 |

### ❌ 未実装 (旧版の誤記。現状は fail-loud で安全側に倒す)

| 旧版の主張 | 実際の状態 |
|---|---|
| 「Sodium / Nvidium のレンダリング性能を上回る」 | 実バイナリとの比較計測は**未実施**(本 sandbox に JDK/Mojang 成果物が無く Sodium 実実行は不可能 — `BENCH_SODIUM_VS_RSIFT.md` §2)。あくまで「Sodium 本物ソース準拠の再現モデル」との計測比較で、領域限定の優位が出ているに留まる。**Nvidium 比較は実施していない**(Nvidium は NVIDIA 専用 mesh shader 依存。Rsift は低スペック方針で mesh shader を設計対象外とするため代替不能) |
| 「Iris / OptiFine シェーダーパックと 100% 互換」 | **パック GLSL の実行互換は未実装**。読み込んだ GLSL は WGSL 変換経路が未配線のため GPU プログラムとして拒否し、警告を出して内蔵 **Eco WGSL** パスへフォールバックする (`iris_pipeline.rs:284`)。「zip 発見・展開・選択 UI」は実装済みだが、シェーダーパックの効果は反映されない |
| 「完全な MRT パイプライン (shadow/gbuffers/composite) を構築」 | pass 計画 (`plan_frame`) と Eco 内蔵 pass の dispatch はあるが、Eco パスは **identity composite**(bloom/DoF/shadow 無し — `iris_pipeline.rs:490` 付近) |
| 「設定 GUI をゲーム内 UI レイヤーへ配備」 | ゲーム内スクリーンへの注入は無し。**launcher ライフサイクルから tracing ログへ box-drawing 描画する preview** が現行の出力形態 |
| 「旧 §4 の起動ログは検証済み実行ログ」 | **フィクションのサンプルログだった**。実コードは `ComplementaryReimagined` 等のパック zip を MRT ロードしない (fail-loud で Eco へフォールバック)。旧ログの "Successfully loaded 6 shader passes ... MRT pipeline ready" という出力は現行コードには存在しない |

```
+---------------------------------------------------------------------------------------------------+
|                        RSIFT GRAPHICS PIPELINE — 検証済み構成要素                                |
|                                                                                                   |
|  [ 1. Compact Vertex ]          [ 2. Rayon Meshing ]              [ 3. wgpu GPU Culling ]         |
|  * 12-byte quantized vertex     * Rayon 並列メッシュ構築          * compute カリング              |
|  * (Quantized12ByteVertex,      * BumpArena 再使用                * draw_indexed_indirect         |
|    静的 assert 済)              * 疑似 MC ベンチ実測あり          * mesh shader 不不使用          |
|                                                                                                   |
|  +---------------------------------------------------------------------------------------------+  |
|  |  SHADER PACK 現状: zip 発見/展開/選択 UI = 実装済, GLSL 実行互換 = 未実装                   |  |
|  |  → 読み込み時に警告を出して内蔵 Eco WGSL (identity composite) へフォールバック              |  |
|  +---------------------------------------------------------------------------------------------+  |
|                                                                                                   |
|  +---------------------------------------------------------------------------------------------+  |
|  |  SETTINGS UI 現状: タブ/トグル/プリセット状態は実装済。出力は launcher ログへの preview     |  |
|  +---------------------------------------------------------------------------------------------+  |
+---------------------------------------------------------------------------------------------------+
```

---

## 2. Sodium / Iris / Nvidium アーキテクチャの調査メモ (外部実装の分析)

> 本節は **外部プロジェクトの分析結果**であり、Rsift 側の実装完了を意味しない。
> Rsift 側の対応状態は §1 の表を参照。

### 2.1. バニラの描画が遅い理由 (一般論)
バニラ Minecraft は旧来の immediate-mode OpenGL 系の描画で、チャンクジオメトリを
単一 CPU スレッドで構築し、非圧縮の頂点フォーマットと大量の個別 draw call を使う。

### 2.2. Sodium の最適化柱 (分析) と Rsift 側の対応実装
1. **Compact Vertex Format**: Sodium は頂点を 28 bytes → 20 bytes へ圧縮。
   * Rsift 側: `Quantized12ByteVertex` (12 bytes、静的 assert 済)。ベンチ実測は
     `BENCH_SODIUM_VS_RSIFT.md` 参照。
2. **Multithreaded Chunk Meshing**: Sodium はバックグラウンドスレッドでメッシュ生成。
   * Rsift 側: `MultithreadedChunkBuilder` (Rayon work-stealing + `BumpArena`)。
3. **Frustum & Occlusion Culling**: Sodium は不可視チャンク/面をスキップ。
   * Rsift 側: `GpuDrivenCullingEngine` (wgpu compute カリング + `draw_indexed_indirect`)。
     Nvidium 式の NVIDIA 専用 mesh shader は**採用しない**(低スペック/携帯性方針。
     meshlet カリングは `frame_worldgen::GpuMeshletCull` が compute エミュレーションで
     実 dispatch する設計)。
4. **比較計測の性質**: Sodium 実バイナリのビルド・実行は本環境では不可能
   (JDK 無し・Mojang/Maven 接続不可 — `BENCH_SODIUM_VS_RSIFT.md` §2 の実測証拠)。
   そのため計測は **Sodium 本物ソース (LGPL-3.0) のアルゴリズムを file:line 引用つきで
   Rust 準拠再現した比較モデル**との対決であり、「実製品 Sodium との総合性能比較」
   ではない。

### 2.3. Iris Shaders パイプライン (分析)
* Iris は `/shaderpacks/` 配下の `.zip` シェーダーパック (BSL, Complementary Reimagined,
  Sildur's Vibrant, SEUS PTGI 等) をロードする。
* パイプライン概念: `gbuffers_*` (地形/エンティティ/水面を複数バッファへ) →
  `shadow` (日照シャドウマップ) → `composite0..15` (Bloom, SSAO, 被写界深度等の
  スクリーンスペース処理) → `final` (トーンマップ/FXAA 出力)。
* **Rsift 側の対応実装 (`IrisShaderEngine`, `iris_pipeline.rs:134`)**:
  - 実装済: パック発見 (`discover_shaderpacks`)、zip 展開 (展開制限つき
    `extract_zip_shaders`)、GLSL ソースの検出、pass 計画 (`plan_frame`)、
    Iris uniform バッファ構造 (`IrisUniformBuffer`)。
  - **未実装: 任意パック GLSL の WGSL 変換・GPU 実行**。
    検出した GLSL は警告を発して拒否し、内蔵 Eco WGSL (低コスト、
    identity composite) へフォールバックする (`iris_pipeline.rs:284`)。
    `dispatch_frame_passes` は `ShaderSourceKind::EcoWgsl` の pass のみを
    GPU-ready として流通させる (`iris_pipeline.rs:345` 付近)。

---

## 3. 設定 UI (`rsift_opt_gfx::gui_settings`) の現状

### 3.1. 実装されているもの
* `SodiumVideoSettingsGui` (`gui_settings.rs:235`): タブ (General / Quality /
  Performance / Advanced / Shaders)・トグル・スライダー・プリセット
  (`feather_preset` / `eco_preset` / `flagship_2k_preset`) の**状態と描画ロジック**。
* 描画形態: tracing `info!` ログへの box-drawing テキストレンダリング
  (`render_gui_frame`)。launcher ライフサイクルが「設定 UI preview」として
  実呼び出しする (`rsift-launcher/src/lifecycle.rs:212`)。
* Shaders タブのパック一覧は `available_shaderpacks` フィールドを列挙。
  2026-07-21 監査で **ディスクに存在しないサンプル名のハードコードを除去**し、
  実一覧は `IrisShaderEngine::discover_shaderpacks` の走査結果を注入する
  契約に変更 (未注入時は空表示)。

### 3.2. 実装されていないもの (旧版の誤解を招く表現への訂正)
* Minecraft のゲーム内 `Screen` への注入描画は無い (`rsift-jvm` の画面注入系は
  未検証の Windows/JNI 契約面として保留中)。
* シェーダーパック選択は状態を保持するが、GLSL 実行互換が未実装のため
  **選択しても見た目は Eco 内蔵パスのまま** (fail-loud の警告付き)。

---

## 4. 起動時に実際に発行されるログ (コード引用に基づく現行動作)

旧版 §4 は実在しないログを「verified execution logs」と称していたため削除し、
現行コードが実際に発行するログに置き換える。GPU が無い CI/開発環境では
preview ログのみが観測できる。

```
[INFO] [RsGraphics] Window title → Minecraft 1.21.11 — RsGraphics
[INFO] [RsGraphics HUD] <PerformanceTier ラベル> | rd=<render_distance>
[INFO] ╔══════════… (SodiumVideoSettingsGui の box-drawing preview) …══════════╗
[INFO] ║  RsGraphics Video Settings                                          ║
[INFO] ║  <tier> · <gpu_name>                                                 ║
[INFO] ║  Profile: <shader_profile>                                           ║
 ...   (Performance タブのトグル行: Compact Vertex / GPU Culling / Meshing / Bindless 等)
[INFO] ║  Active: None                    ← パック未選択 (GLSL 実行互換は未実装)
```

シェーダーパック zip を実際にロードした場合の現行挙動 (`iris_pipeline.rs` の
実コードメッセージを引用):

```
[WARN] [Iris] Pack GLSL found for <pass> but GLSL→HLSL/WGSL transpile is not wired — refusing uncompiled GPU program (falling back to Eco WGSL)
[INFO] [Iris] Disabled → Eco/Sodium path      ← disable_shaders 時
```

---

## 5. Conclusion (検証可能な範囲のみ)

- `rsift-opt-gfx` により、Sodium 系の主要アルゴリズム (頂点圧縮・並列メッシング・
  GPU カリング) を Rust/wgpu で実装し、そのうち CPU 側分野については
  Sodium 本物ソース準拠の再現モデルとの 8 分野計測で領域限定的な優位を確認した
  (詳細と全数値: `docs/BENCH_SODIUM_VS_RSIFT.md`。実バイナリ比較ではない)。
- シェーダーパックまわりは「発見・展開・選択 UI」までが実装済みで、
  **GLSL 実行互換は未実装** (fail-loud + Eco フォールバックで安全側に倒す設計)。
- 旧版の「Sodium/Nvidium を上回る」「Iris/OptiFine と 100% 互換」の記述は
  実装・計測の裏付けが無かったため本改訂で撤回した。今後これらを主張する場合は
  実測エビデンス (再現手順つき) を本書に添付すること。
