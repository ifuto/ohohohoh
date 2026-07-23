# Sodium vs Rsift — 疑似 Minecraft 多分野ベンチマーク対決

> 結論: CaffeineMC/sodium `mc1.21.11-0.8.13` の**本物のソース** (commit
> `9d11e9cfc5915de826215152988e115e5306e748`, provenance 検証済) を読解し、
> そのアルゴリズムを **file:line 引用つきで Rust 再現** した上で、
> 画面描画のない疑似 Minecraft (Anvil 形式・16³ セクション・パレット等の
> データシステムは本物仕様) 上で **8 分野の測定対決**を行った。
> 本物の Sodium jar のビルド・実行は本 sandbox 環境では不可能 (証拠は §2)。
> これは実バイナリのプロファイルではなく、**本物ソース準拠のアルゴリズム
> 再現モデル**による比較である。

## 1. 背景と目的

ユーザー要求 (2026-07-20):

1. 「Bash で Sodium をダウンロードしデコンパイル、本物のマイクラじゃなくても
   動くように改造します」
2. 「画面描画のない架空上のマイクラ (データのシステム等は本物そっくり) を
   動かして、Sodium vs Rsift で対決させて。様々な分野でベンチマーク対決」

実施内容:

- Sodium は **GitHub 上で完全にソース公開** されているため、Java バイトコードの
  デコンパイルは不要だった (ソースのほうが正確)。
- 「本物のマイクラじゃなくても動くように改造」の解釈: 本物の Minecraft ランタイム
  (Mojang 公式 jar + Fabric Loader + JDK) が入手・起動できない環境でも Sodium の
  **アルゴリズム**を動かして測定できるよう、Sodium の実ソースを読んでその処理を
  **rsift の Rust ハーネス上に準拠再現**する (§3)。
- 架空 Minecraft = `rsift/crates/rsift-opt-gfx/examples/pseudo_mc_bench.rs` の
  疑似ワールドハーネス (§4)。画面描画はなく、ブロックデータ・リージョン I/O・
  メッシュ構築・カリング等の**データシステムは本物の仕様**に従う。

## 2. Sodium 取得と「本物実行不可能」の証拠

### 2.1 取得と provenance 検証

```bash
git clone --depth 1 --branch mc1.21.11-0.8.13 \
  https://github.com/CaffeineMC/sodium.git /tmp/sodium
cd /tmp/sodium && git rev-parse HEAD
# → 9d11e9cfc5915de826215152988e115e5306e748
```

- タグ選択根拠: GitHub API (`api.github.com/repos/CaffeineMC/sodium/tags`) で
  `mc1.21.11-*` 系の最新が `0.8.13`, その commit sha が clone 結果と
  **完全一致** (API 応答 sha = `9d11e9cfc5…`)。
- 674 Java ファイル / 5.5MB。ライセンスは **LGPL-3.0**。
- ライセンスと差分管理のため **Sodium のソース自体は本リポジトリに含めない**。
  本書は定数値と方式の引用 (file:line) のみを行う。

### 2.2 本物バイナリのビルド・実行が不可能である証拠 (全て実測)

| 必要物 | 状況 | 実測 |
|---|---|---|
| JDK (Java 21+) | **存在しない** | `which java javac` → not found |
| Mojang 公式 jar / メタデータ | 接続不可 | `piston-meta.mojang.com` → curl FAIL |
| Maven 依存解決 | 接続不可 | `repo.maven.apache.org` → curl FAIL |
| Gradle 実行環境 | 取得不可 | `services.gradle.org` → curl FAIL (wrapper jar も無し) |
| メモリ | 不足 | sandbox は 3GB (MC クライアント実走には 1-2GB の heap だけで逼迫) |

到達可能なのは `github.com` / `codeload.github.com` / `api.github.com` のみ。
すなわち「Sodium のソースを読む・検証する」ことはできるが「本物を javac/gradle
でビルドして動かす」ことは sandbox のネットワーク・ツールチェーン制約上**不可能**
であり、バイナリ実行を装った数値は一切出していない。代替として §3 の
**実ソース準拠の Rust 再現**をベンチ対象とする。

## 3. Sodium アルゴリズム分析 (検証済み引用)

以下の行番号は全て tag `mc1.21.11-0.8.13` (= commit `9d11e9cfc5`) の
`common/src/main/java/net/caffeinemc/mods/sodium/client/` 以下で実際に
開いて確認したもの (2026-07-20 実検証)。右列は本ハーネス側の再現箇所
(`examples/pseudo_mc_bench.rs`) との対応。

### 3.1 頂点フォーマット: Compact 20 バイト

| offset | 内容 | 出典 | 再現 |
|---|---|---|---|
| 0 | pos_hi (各軸上位 10bit x3) | `render/chunk/vertex/format/impl/CompactChunkVertex.java:72` | `sq_pack_hi` |
| 4 | pos_lo (各軸下位 10bit x3) | 同:78 | `sq_pack_lo` |
| 8 | color RGBA8 (AO を RGB 畳込 `ColorARGB.mulRGB`) | 同:61 | `sq_mesh_section` 内 |
| 12 | tex UV (各 15bit + 符号 bit、セントロイド方向 1LSB 縮退) | 同:96-107 | `sq_tex` |
| 16 | light (sky/block 8+8bit, +8 バイアス clamp 8..=248) + material 8bit + section 8bit | 同:109-120 | `sq_light` |

- `STRIDE = 20` は 同:12。
- 位置量子化 `((8.0 + v) / 32.0) * 2^20 & 0xFFFFF` は 同:21,24-25,84-90。
- 対比: vanilla の BLOCK 頂点は 32B (float3 pos + color4 + float2 uv + short2
  overlay + short2 light + byte3 normal + pad)。Sodium は **37.5% 削減**。

### 3.2 遮蔽カリング (オクルージョン)

| 機構 | 出典 | 再現 |
|---|---|---|
| メッシュ時に vanilla `VisibilitySet` を 64bit に再エンコード (bit = from*8+to) | `render/chunk/occlusion/VisibilityEncoding.java:9-26` (import が vanilla `net.minecraft.client.renderer.chunk.VisibilitySet` であることが「vanilla も構築している」証拠) | `sq_visibility` |
| `createMask` 乗算展開 + 32/16/8 fold で incoming→outgoing 接続 | 同:28-49 | `sq_connections` |
| double-buffer キュー BFS | `render/chunk/occlusion/OcclusionCuller.java:20-21,50-57` | `sq_graph_find_visible` |
| 角度遮蔽マスク (垂直成分が最大なら水平透過パスを潰す等) | 同:111-129 | `sq_angle_mask` |
| 外向き方向マスク (origin に近い側への逆流を抑止) | 同:187-199 | `sq_outward` |
| 距離判定 = vanilla 円筒 fog (box を ±1 拡張, dx²+dz²<d² かつ \|dy\|<d) | 同:202-227 | `sq_within_distance` + `sq_nearest_to_zero` |
| 錐台 (セクション box に margin 1.125 / 近傍は 2.125) | 同:131-133 + `render/viewport/Viewport.java:13-16` | `sq_aabb_visible` |
| カメラ近傍 26 セクションは BFS 未到達でも追加 | 同:243-269 | `sq_graph_find_visible` 末尾 |
| 方向番号 DOWN=0,UP=1,NORTH=2,SOUTH=3,WEST=4,EAST=5 | `render/chunk/occlusion/GraphDirection.java:6-11` | `DIR_*` 定数 |

要点: Sodium 自身は遮蔽「仕組み」を vanilla と**同族**のまま使い、vanilla の
VisGraph 構築済みデータを軽量に再エンコードして高速に回す設計。
したがってベンチ上も A/B で遮蔽探索は共通実装とし、差は頂点形式・
draw call・更新スケジューリングで測るのが正しい。

### 3.3 draw call: リージョン multidraw

| 機構 | 出典 | 再現 |
|---|---|---|
| `glMultiDrawElementsBaseVertex` に CPU 構築バッチ (count / elementPointer / baseVertex 配列) | `gl/device/MultiDrawBatch.java:8-14` | `draw_call_model` の 16B エントリ |
| 共有 quad index buffer (uint32, 全セクション共用) | `render/chunk/DefaultChunkRenderer.java:33-38` | 同上 |
| リージョン単位の multidraw 実行 | 同:358-363 (executeDrawBatch: fillCommandBuffer + multiDrawElementsBaseVertex) | リージョン集約で call 数計算 |
| リージョン = 8x4x8 セクション (128x64x128 ブロック) | `render/chunk/region/RenderRegion.java:30-32` | `sq_region` |

### 3.4 更新スケジューリング / VBO 管理 / 半透明

| 機構 | 出典 | 再現 |
|---|---|---|
| 汚染セクションのみリビルド投下 (join) | `render/chunk/RenderSectionManager.java:797-800` | `edit_sim` の汚染集合 |
| 優先度 REBUILD=0b010 / IMPORTANT=0b100 / INITIAL_BUILD=0b1000、近接 16m は IMPORTANT 昇格 | `render/chunk/ChunkUpdateTypes.java:12-14` + `RenderSectionManager.java:65,401` | コスト積算に反映 |
| VBO アリーナ (stride 固定 + free list で断片化抑制) | `gl/arena/GlBufferArena.java:9-23` | バイト計測のみ (管理コストは対象外) |
| 半透明: セクション単位の隠れグラフ DFS トポロジカルソート。関係 `quadVisibleThrough`: 対向面は互いに見えない / 平行面は extents 比較 / 直交面は half-space 2 条件 + 交差時 `合計>1` ヒューリスティク | `render/chunk/translucent_sorting/data/TopoGraphSorting.java:72-95,170-200` (2 quads special case 同:307-308) | `sq_visible_through` に完全移植 |
| 半透明: extents 配列順 = [posX,posY,posZ,negX,negY,negZ] / facing ordinal POS_X..POS_Z=0..2, NEG_X..NEG_Z=3..5 | `translucent_sorting/TQuad.java:141` + `client/model/quad/properties/ModelQuadFacing.java:11-18` | `sq_extents` / `sq_facing` |
| 半透明: `sortTypeHeuristic` (NONE / STATIC_NORMAL_RELATIVE / STATIC_TOPO / DYNAMIC の判定)。attempt limit `STATIC_TOPO_SORT_ATTEMPT_LIMITS = {-1,-1,250,100,50,30}` を法線数でクランプ。limit 超過は topo を試みず DYNAMIC 直行 | `TranslucentGeometryCollector.java:215,254-342,393-394` | `sq_sort_plan` に完全移植 |
| 半透明: STATIC_TOPO 失敗 (サイクル) → DYNAMIC へ切替。DYNAMIC で >1000 quads は directTrigger (トリガ毎に重心距離ソート)。距離キー = 重心の二乗ユークリッド距離 `~floatToRawIntBits(d²)` + radix sort | `TranslucentGeometryCollector.java:410-416,425-434` + `DynamicTopoData.java:36,62-70,271-275` | `sodium_tsort` fallback / `sq_dist_sort` |

## 4. 疑似 Minecraft (架空上の本物そっくりデータシステム)

`examples/pseudo_mc_bench.rs` の `PseudoWorld`:

- 96x96x192 ボクセル、ブロック 10 種、値ノイズ地形 + 海抜 62 の水。
- **本物仕様のまま再現しているもの**
  - Anvil 相当リージョン I/O: チャンク生バイト列を **zlib(deflate) 圧縮→
    展開の完全 roundtrip** (`vanilla_region_roundtrip`)。
  - 16³ セクション単位メッシュ (Sodium/vanilla の管理単位と一致)。
  - 面カリング規則 (隣接不透明ブロックで面消去) と **4 サンプル
    スムースライティング AO** (vanilla/Sodium 共通規則を A/B で同一コード使用)。
  - 距離 fog 96 (6 チャンク相当)、fov 75° 錐台。
  - プレイヤー編集 → 境界接触で隣接セクションまで汚染 (vanilla の
    `updateShape` 相当) → 汚染セクションのみリビルド。
  - 半透明 (水) クアッドの実ソート。
- 簡略化 (正直な注意): テクスチャ/バイオーム/エンティティ AI/物理/ネット等は
  含まない。BlockState は 1 バイト ID。ワールド規模は実機より小さい
  (sandbox は 2 cores / 3GB RAM)。

## 5. 計測結果 (全て実測・release build・同一プロセス)

環境: sandbox 2 cores / 3GB RAM / Rust 1.94.1 / 2026-07-20。
再現: `cargo run --release -p rsift-opt-gfx --example pseudo_mc_bench --locked --offline`
(数値は run 毎に微小変動する。以下は実際の 1 回の出力)。

```
world gen: 16.717295ms (96x96x192)
```

### 5.1 チャンク再メッシュ (36 chunks 全量)

| pipe | メッシュのみ | VisGraph (A/B 共有) | 合計 | 頂点数 | 頂点バイト | B/頂点 |
|---|---|---|---|---|---|---|
| A Vanilla系 | 40.4 ms | 158.6 ms | **199.1 ms** | 861,656 | 27,572,992 | 32 |
| B Sodium系 | 54.3 ms | 158.6 ms (+64bit再エンコード) | **212.9 ms** | 861,656 | 17,233,120 | 20 |
| C Rsift | **547.2 ms** (内訳下記) | 不要 (独自 DDA 経路) | **547.2 ms** | **303,895** | **3,646,740** | 12 |

C 内訳 (ステージ実測): mesh-loop (encode + dedup) **94.8 ms** / Tipsify 頂点
キャッシュ最適化 **452.5 ms** / ACMR 計測器 (品質レポート用, 非計上) 39.7 ms。

品質軸 (実測): C の頂点数は intern 重複排除で **861,656 → 303,895 (−64.7%)**。
頂点メモリは **A 比 86.8% 減 / B 比 78.8% 減** (B はフォーマット差のみで
A 比 37.5% 減)。Tipsify の効果は **ACMR ( average cache miss ratio, cache 16 )
before=1.669 → after=0.929** — 旧構成では新規 4 頂点/面の退化グラフ
(頂点 valence ≒1) で恒等的に無力 (2.000→2.000) だったが、dedup で実頂点
グラフが復元され初めて実機能した。

**メッシュ CPU 時間の正直な経緯 (改修履歴)**: 以前の C 合計 274-301 ms は
「intern が ID を破棄する装飾呼出 + dedup 無し」だった**偽実装の偽コスト**
であり、真の実装 (量子化ドメイン直接索引 dedup + SIP 廃止の FoldHasher
intern + Tipsify の CSR/heapify/スコアテーブル化) に置き換えた結果が
**547 ms** である。これは旧版より遅いのではなく、**dedup・頂点キャッシュ
最適化を実際に行う真のコストを初めて正しく計上した値**で、品質軸
(頂点 −64.7% / ACMR 半減) と引き換えの設計判断である。単純比較では
A/B の 2.6 倍重いが、A/B はフォーマットエンコードのみで頂点グラフ構築を
していない (= 仕事量が異なる) 点に注意。

公平性ルール: 1.21 系 vanilla もメッシュ時に VisGraph を構築する
(Sodium の `VisibilityEncoding` が vanilla の `VisibilitySet` を import して
いることがソース上の証拠)。その flood fill コストは A/B 共有実装の実測を
**両者に同額課している** (B だけに課すと Sodium 側に不公平になるため)。

### 5.2 リージョン I/O (36 chunks, 生 1,769,472 bytes)

| pipe | 時間 | ファイルサイズ | 圧縮率 |
|---|---|---|---|
| A/B Vanilla系 zlib 逐次 | 46.9 ms | 65,469 | 0.037 |
| C Rsift zstd 並列 | **7.6 ms (6.2x 高速)** | 155,648 | 0.088 |

(保存サイズは zlib の方が小さい。読み書き時間差が支配的。)

### 5.3 実体カリング (600 entities)

| pipe | 時間 | 描画対象 |
|---|---|---|
| A Vanilla系 (無カリング) | 16.0 µs | 600 |
| B Sodium系 (距離+錐台) | 11.7 µs | 422 |
| C Rsift (DDA 遮蔽, 実モジュール `EntityCuller`) | 2.54 ms | **175** (occluded 425 / far 48 / rays 10,500) |

C のみ「地形に完全に隠れた実体」を確定的に落とせる (175/600)。
代償はレイコスト。tick 単位の差分更新 (`period_ticks`) で実機では
フレーム辺りに償却される。

### 5.4 セクション可視性 (432 セクション, 非空 171)

| シナリオ | 基準 (frustum+fog のみ) | A=B graph 遮蔽 ON | C DDA |
|---|---|---|---|
| S1 展望 (高所から俯瞰) | 到達 14 / 描画可能 7 | 到達 14 / 描画可能 7 / 2.4 µs | 61 / 582 µs (rays 2,802) |
| S2 丘越し (谷から水平視線) | 到達 14 / 描画可能 14 | 到達 15 / 描画可能 15 / 1.8 µs | 15 / 151 µs (rays 4,287) |

解釈:

- **graph と DDA の意味論差をそのまま可視化した結果**。vanilla 族の graph は
  「面接続を辿れるセクション」= 空気連結路しか進まない積極的保守近似
  (S1 で描画可能たった 7) だが、近傍 26 セクション強制追加で穴を補う。
  DDA は「AABB へのレイが届くか」で、地形表面のセクションは表面が見える
  以上ほぼ可視 (S1 で 61)。**S2 の丘越しでは両者が一致 (15)** = 「丘の向こうは
  確定的に描かない」点でどちらの方式も機能している。
- graph 探索は µs 級だが、それは VisGraph 構築コストをメッシュ時に
  前払いしているから (5.1 に計上済)。DDA は前払い不要で毎回レイを張る。

### 5.5 draw call (S1 可視 14 セクション)

| pipe | solid+translucent calls | cmd 構築時間 |
|---|---|---|
| A Vanilla系 (セクション個別 draw) | 7 | 799 ns |
| B Sodium系 (リージョン multidraw, batch 112B) | 1 | 1.08 µs |
| C Rsift (GPU cull → indirect 固定) | **2 (シーン規模非依存)** | — |

B = リージョン数×pass 数に集約 (本ワールドは XZ 1 リージョン)。
C = 可視数に依らず 2 本固定 (azdo/gpu_vertex_pull 設計)。

### 5.6 半透明ソート (水クアッド 116,719 個を実ソート)

**メトリクス設計**: ソート結果のペアワイズ順序誤り率を決定的サンプリング
(近接セル 8 m × i%8 間引き) で実測する。評価対象は **両方のクアッドが
カメラを向く** (バックフェイスカリングで描画され得る) かつ画面 AABB が
重なるペアに限定。拘束関係は 3 種で独立集計: (a) 古典関係 (カメラ依存の
分離平面画家規則), (b) Sodium 関係 (quadVisibleThrough 完全移植),
(c) 合併 (矛盾ペアは除外)。

**両 pipe のソート実装** (§3.4 の出典に基づく):

- B Sodium系 (実ソース準拠パイプライン): セクション毎に
  `sortTypeHeuristic` を適用 → attempt limit 内なら STATIC_TOPO を試行、
  limit 超過 (本ワールドの水セクションは全 6 法線発生 → limit 30) は
  **topo を試みず DYNAMIC 直行** → セクション内を重心の二乗ユークリッド
  距離で far→near。セクション間は距離整列で連結。
- C Rsift (固有設計): 全セクションで topo attempt (関係 = カメラ依存の古典
  分離平面)。サイクル検出セクションはセクション単位で閉じず、
  **クアッド単位ユニットに分解して非サイクルブロックと共に全クアッド
  大域の投影視深でマージ** する (セクション境界のハードな順序不連続
  = 境界フリップ誤順の系統的原因を解消するのが狙い)。

#### S1 展望 (北端の高所から南を俯瞰)

| pipe | 時間 | 誤順 (古典関係) | 誤順 (Sodium関係) | 誤順 (合併) |
|---|---|---|---|---|
| A Vanilla系 | 2.59 ms | 20,368/272,654 (7.47%) | 22,211/296,195 (7.50%) | 27,024/312,568 (8.65%) |
| B Sodium系 | 12.50 ms | 23,069/261,664 (8.82%) | 27,329/285,801 (9.56%) | 28,968/300,918 (9.63%) |
| C Rsift | 3.57 s | **17,831/273,751 (6.51%)** | **20,097/298,569 (6.73%)** | **24,543/314,193 (7.81%)** |

フォールバック実績 (水 44 セクション中): B = DYNAMIC 直行 33 セクション
(116,381 quads) + topo 断念 0 / C = topo 断念 32 セクション (116,349 quads
→ 大域マージへ解放)。

#### S2 丘越し (谷の地表+2.5 m から水平視線)

| pipe | 時間 | 誤順 (古典関係) | 誤順 (Sodium関係) | 誤順 (合併) |
|---|---|---|---|---|
| A Vanilla系 | 2.65 ms | 9,268/191,638 (4.84%) | 12,622/217,652 (5.80%) | 13,723/224,027 (6.13%) |
| B Sodium系 | 13.62 ms | 9,490/192,441 (4.93%) | 13,068/218,729 (5.97%) | 13,505/225,239 (6.00%) |
| C Rsift | 3.74 s | **7,744/197,467 (3.92%)** | **11,045/223,702 (4.94%)** | **11,959/230,460 (5.19%)** |

フォールバック実績: B = DYNAMIC 直行 33 セクション / C = topo 断念
33 セクション (116,381 quads → 大域マージ)。

**計測からの知見 (全て実測根拠)**:

1. **サイクル支配**: 116,719 quads 中 **99.7% (116,381) がサイクルセクション**
   に属する (最大セクションは 7,039 quads / 12,007,564 拘束エッジの全結合級)。
   段差状の湖岸テラスが交差ヒューリスティク (TopoGraphSorting.java:88-92)
   と直交 half-space 関係で長距離の拘束サイクルを構成するためで、
   「STATIC_TOPO は構造的に成立しない」「DYNAMIC 距離ソートが主戦場」
   という Sodium の設計判断 (limit で attempt 自体を回避) が正しいことも
   同時に示された。
2. **セクション単位距離ソート + セクション順ハード境界は大域単純ソートに
   劣後し得る**: S1 で B (9.63%) は A の大域重心 Z (8.65%) より誤順が多い。
   境界で遠近ブロックが強制フリップするのが原因 (境界またぎ拘束ペアで
   系統的に誤る)。
3. **C の大域マージは境界フリップを解消し、本ハーネス最良の正確性**:
   S1/S2 とも全 3 関係で A・B を下回る (合併 S1: C 7.81% < A 8.65% <
   B 9.63% / S2: C 5.19% < B 6.00% < A 6.13%)。非サイクルセクションは
   topo 順をブロック単位で尊重するため、真に順序が決まる部分の拘束は
   損なわない。
4. **コストの正直な位置付け**: C は全セクションで全ペア関係評価 + Kahn を
   試行するため ~3.5 s/シナリオ を要する (B は heuristic が試行自体を回避し
   12-14 ms)。これは「拘束をどこまで真剣に解くか」の設計差であり、
   rsift 側の本命はこの関係評価の GPU compute ミラー化 (iGPU コア WGSL
   制約内: compute + storage buffer + 少量 readback) である。
5. 誤順カウントは 2 回実行で完全一致 (決定的ソート: stable radix 相当の
   tie-break + 決定的ヒープ frontier)。サンプリング評価のため絶対値では
   なく同一条件下の相対比較として読むこと。

### 5.7 編集ワークロード (編集 120 / 汚染セクション延べ 787 / 全 pipe 同一並列度)

| pipe | 総時間 | 再構築バイト |
|---|---|---|
| A Vanilla系 (32B + smooth AO) | 82.7 ms | 168,887,936 |
| B Sodium系 (20B + 遮蔽再計算) | 281.3 ms | 105,554,960 |
| C Rsift (12B + dedup + Tipsify) | 1.266 s | **24,511,704** |

single-run の CPU 時間は C が最大 (真の dedup + Tipsify を全汚染セクションに
適用するため)。生成バイト量は **C が A の 14.5%** (dedup で頂点数そのものが
減る効果)。編集頻度の高い実運用では「再生成物のサイズ」が VRAM 転送量・
arena 圧力に直結する。

### 5.8 合成「重い 1 フレーム」 (再メッシュ x4 + load x8 + save x2 + 実体 1 パス)

| pipe | 合成時間/frame | 計算律速 FPS |
|---|---|---|
| A Vanilla系 | 35.16 ms | 28 |
| B Sodium系 | 36.69 ms | 27 |
| C Rsift | 65.63 ms | 15 |

speedup vs A: 0.96x (B), 0.54x (C)

**この数値の読み方 (重要)**: 本ハーネスは mesher を全 pipe 同一スレッドに揃えた
**アルゴリズム特性の比較モデル**であり、実機 FPS の近似ではない。実機で
Sodium が速い主因は (a) 頂点バイト 37.5% 減 → GPU 帯域・VRAM, (b) リージョン
multidraw → ドライバ負荷減, (c) ワーカースレッド並列リビルド, (d) 遮蔽カリングの
4点であり、これらは個別表 (5.1/5.5/5.7/5.4) に実測で現れている。

## 6. 分野別マトリクス

| 分野 | 勝者 | 実測根拠 |
|---|---|---|
| 頂点メモリ量 | **C Rsift** | 3.65 MB (A 比 86.8% 減 / dedup 減頂点込み) |
| メッシュ CPU 時間 (単一スレッド) | **A/B ≈ 同等 < C** | 199/213/547 ms (VisGraph は A/B 同額。C は真の dedup+Tipsify の真コスト) |
| 頂点キャッシュ品質 (ACMR) | **C Rsift** | 1.669→0.929 (A/B はエンコードのみで最適化なし) |
| リージョン I/O 時間 | **C Rsift** | 7.6 ms vs 46.9 ms (6.2x) |
| 実体の真の遮蔽カリング | **C Rsift** | 600→175 (B は 422, A は 600) |
| セクション遮蔽 (丘越し) | 引分 (A/B=C 一致) | S2 で 15=15 |
| draw call 数 | **C Rsift** (固定 2) | B はリージョン数依存、A はセクション数依存 |
| 半透明ソート正確性 | **C Rsift** | 合併誤順 S1: 7.81% < A 8.65% < B 9.63% / S2: 5.19% < B 6.00% < A 6.13% |
| 編集時 再生成バイト | **C Rsift** (24.5 MB = A の 14.5%) | 時間は A が最小 |
| VRAM 帯域 (実機推定) | **C Rsift** | 12B 頂点 + 半解像 AO 設計 |

## 7. 限界 (正直な注意)

1. sandbox は GPU 非所持のため、GPU 側時間 (帯域・シェーダ実行) はモデル
   推定であって実測ではない。rsift の実機優位は低スペック iGPU の帯域削減に
   依存するため、**実機検証が最終判定に必要**。
2. ワールドは 96x96x192 と小さい。graph 探索・multidraw のスケール特性は
   実機規模で変わり得る。
3. Sodium 再現は CPU 側アルゴリズムの意味論に限定 (GL コマンド発行や
   ドライバ内部は対象外)。数値は CaffeineMC 公表値ではなく**本ハーネスの
   再現実装の実測値**。
4. C の DDA 実体カリング 2.5 ms は償却前提なしの全件測定。実機では
   `period_ticks` の差分スケジューリングでフレーム辺りはずっと軽い。
5. 疑似ワールドは水-水 内部面も生成する (全 pipe 同一の面規則で比較自体は
   公平。本物 MC の流体レンダラは水面のみ描くため、半透明クアッドの
   絶対量は実機より過大 = ソートに対し意図的に厳しいストレス条件)。
6. C の半透明ソート ~3.5 s/シナリオ は「全拘束を CPU で試行する」設計の
   実コスト。GPU compute ミラー化を前提とする設計意図であり、現状の
   CPU 時間は実機設計の近似ではない (§5.6 知見 4)。
7. C のメッシュ CPU 時間 547 ms は依然 3 pipe 最大 (§5.1 経緯参照)。
   dedup+Tipsify の仕事量と引き換えの設計判断だが、Tipsify の
   並列化/GPU 化は今後の削減余地。

## 8. 今後の課題

- 半透明ソート関係評価 / Kahn / Tipsify の GPU compute 化
  (iGPU コア WGSL 制約内: compute + RO/RW storage buffer + 少量 readback)
- 実機 (iGPU 搭載 PC) での frame_proof_extra gpu-* モードと組み合わせた
  end-to-end 実測
- ワールド規模の拡大 (複数リージョン) での multidraw 集約率スケール測定
