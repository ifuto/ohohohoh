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
| C Rsift | **285.6 ms** (内訳下記) | 不要 (独自 DDA 経路) | **285.6 ms** | **303,895** | **3,646,740** | 12 |

C 内訳 (ステージ実測): mesh-loop (encode + dedup) **96.6 ms** / Tipsify 頂点
キャッシュ最適化 **189.1 ms** / ACMR 計測器 (品質レポート用, 非計上) 42.1 ms。

> **wave 213 HJ による Tipsify 再計測 (2026-08-02 同一砂場)**: 同一の貪欲ヒープ
> 意味論 (BV-3 ピンで厳密系列を固定) を保ったまま、(a) ソートキーの単一 u64
> 畳み込み (比較が total_cmp×2 → u64 1 命令) (b) 無変化再スコアの version/push
> 省略 (LRU 追放のスタレネスを仕様として含む遅延更新は厳密維持 — 一括化すると
> bit 不一致になることを optimality ピンでの RED 再現で確認し採用却下) (c)
> cache_pos 更新の差分区間化 (空枠ガード込み) により **Tipsify 507.3 →
> 189.1 ms (2.7x)**、ACMR 列は bit 同一 (1.669→0.929 両版で一致)。旧 547.2 ms
> の経緯は下段落を参照。

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

### 5.3 実体カリング (600 entities)  **wave 214 HK-1 再計測 (2026-08-03)**

| pipe | 時間 | 描画対象 |
|---|---|---|
| A Vanilla系 (無カリング) | 16.1 µs | 600 |
| B Sodium系 (距離+錐台) | 11.3 µs | 422 |
| C Rsift (FOV+5レイ DDA 遮蔽 V2, 実モジュール `FastEntityCuller`) | **528.9 µs** | **125** (occluded 475 / far+fov 178 / rays 2,110) |

凍結 (wave 213): `EntityCuller` (legacy 27 レイ, FOV なし) 2.54-2.57 ms,
175 (rays 10,500)。wave 214 で C のエンジンを `FastEntityCuller` (V2) に
統一し **4.9x 高速化・レイ数 80% 減**。**V2 可視集合 ⊆ legacy 可視集合**
(v2_only=0 = 虚偽可視混入なし)。

機械的な膝点根拠 (dense 5³=125 レイを真値計測器とする eval_set=422 の
サンプル点数スイープ、同一ワールド・同一プロセス):

| サンプラー | 可視 | 時間 | pops (真値可視の見落とし) |
|---|---|---|---|
| V2 5 レイ  | 125 | 519 µs | 4 |
| 9 レイ      | 126 | 926 µs | 3 |
| legacy 27 レイ | 127 | 2,037 µs | 2 |
| dense 125 レイ (真値) | 129 | 8,257 µs | 0 |

- 全サンプラー waste 0 (壁抜けの虚偽可視はどれも発生しない = 保守側)。
- pops は有限サンプリングの近似誤差であり **legacy 27 レイも 2 件欠落**。
  27→5 の粗化で +2 件 (4/422 = 0.9%) 増えるが、コスト 3.9x を正当化する
  精度差ではない = 膝点は 5-9 レイ帯 (移動再評価周期 10 tick が統計的に
  更に縮める)。可視数変更 175→125 の内訳: FOV 整合 −48、サンプリ −2。
- 【既知欠陥の根治記録】V2 には ChunkBucketGate が存在したが、(a) skip
  対象の last_eval_tick を更新しないため新規スロットの既定 visible=true
  を 0 レイのまま永久保持 (岩盤内実体の虚偽可視 — 旧 CL-3 pin がバグ
  を仕様化していた) (b) skip 判定が opaque 3 probe + 1 レイを毎 tick
  要求し skip 節約を恒に上回るデッドコード → wave 214 で撤去 (出力は
  虚偽可視分のみ変化、抗変異 RED x4 で番犬確認)。
- production (`full_graph_wiring` tick_world) も同エンジンに統一し、CL-4
  で「将来課題」だった実カメラ FOV 真値配線を完了 (対角半角余弦 +
  0.15 rad 余裕)。boot banner の "FastEntityCuller V2" 表記と実装が
  初めて一致した。

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
- C Rsift (固有設計, **wave 212 HI 門番化済**): `sq_sort_plan` のしきい値で
  topo attempt を門番 (Sodium STATIC_TOPO_SORT_ATTEMPT_LIMITS と同一表で、
  limit 超過セクションは**試行自体をしない** = Cycle セクションの内部順は
  大域マージに流れるため試行結果を使わない設計。既往版は全セクションに
  O(n²) 全ペア試行し 99.7% が失敗 = 試行成果を全捨てしていた欠陥があった)。
  関係 = カメラ依存の古典分離平面。サイクル/Dynamic セクションは
  セクション単位で閉じず、**クアッド単位ユニットに分解して非サイクル
  ブロックと共に全クアッド大域の投影視深でマージ** する (セクション境界の
  ハードな順序不連続 = 境界フリップ誤順の系統的原因を解消するのが狙い)。

#### S1 展望 (北端の高所から南を俯瞰)  **wave 212 再計測 (2026-08-02)**

| pipe | 時間 | 誤順 (古典関係) | 誤順 (Sodium関係) | 誤順 (合併) |
|---|---|---|---|---|
| A Vanilla系 | 2.57 ms | 20,368/272,654 (7.47%) | 22,211/296,195 (7.50%) | 27,024/312,568 (8.65%) |
| B Sodium系 | 12.72 ms | 23,069/261,664 (8.82%) | 27,329/285,801 (9.56%) | 28,968/300,918 (9.63%) |
| C Rsift | **25.23 ms** (旧 3.6 s) | **15,915/259,562 (6.13%)** | **18,440/284,620 (6.48%)** | **22,508/298,970 (7.53%)** |

フォールバック実績 (水 44 セクション中): B = DYNAMIC 直行 33 セクション
(116,381 quads) + topo 断念 0 / C = topo 断念 0 + **試行省略 (Dynamic 門番)
33 セクション** (116,381 quads → 大域マージへ解放)。wave 212 で時間は
**143x 改善** (3.6 s → 25 ms) したうえ誤順も **改善** (7.81% → 7.53%):
旧版で topo が「まれに成功」した limit 超過セクションをブロックユニットと
して扱っていた境界フリップ誤順が、全大域クアッドマージ一貫化で解消したため。

#### S2 丘越し (谷の地表+2.5 m から水平視線)  **wave 212 再計測 (2026-08-02)**

| pipe | 時間 | 誤順 (古典関係) | 誤順 (Sodium関係) | 誤順 (合併) |
|---|---|---|---|---|
| A Vanilla系 | 2.09 ms | 9,268/191,638 (4.84%) | 12,622/217,652 (5.80%) | 13,723/224,027 (6.13%) |
| B Sodium系 | 11.37 ms | 9,490/192,441 (4.93%) | 13,068/218,729 (5.97%) | 13,505/225,239 (6.00%) |
| C Rsift | **21.38 ms** (旧 3.8 s) | **7,744/197,467 (3.92%)** | **11,045/223,702 (4.94%)** | **11,959/230,460 (5.19%)** |

フォールバック実績: B = DYNAMIC 直行 33 セクション / C = topo 断念 0 +
試行省略 (Dynamic 門番) 33 セクション (116,381 quads → 大域マージ)。
(wave 213 後の再計測でも誤順カウントは S1/S2 とも上表と bit 一致
= Tipsify 線形化の半透明ソートへの非干渉を決定性で確認)

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

### 5.7 編集ワークロード (編集 120 / 汚染セクション延べ 787 = touched 実セクション 136 / 全 pipe 同一並列度)

**wave 215 HL 再計測 (2026-08-03、独立 2 run 可再現)**

| pipe | 総時間 | 再構築バイト |
|---|---|---|
| A Vanilla系 (32B + smooth AO) | 85.5 ms | 168,887,936 |
| B Sodium系 (20B + 遮蔽再計算) | 309.9 ms | 105,554,960 |
| C Rsift (12B + dedup + Tipsify) | **840.0 ms** (旧 1.266 s → wave 213 HJ) | **24,511,704** |
| C Rsift 差分 (12B diff-mesh + Tipsify 償却, wave 215) | **84.7 ms** | **1,560,744** |

犯行現場の確定 (temp 計測、後に撤収): 全量経路 C のコスト ≈ 全て Tipsify
(CPU 累積 967 ms / 787 calls ≈ 1.23 ms/セクション) + セクション毎の
slot 配列 (17³×27 = 132,651 u32 = 0.53 MB) 初期化 (全編集で ≈417 MB の
メモリ書込)。**1 ブロックの編集のたびにセクション全面を再構築する全量
再描画こそが構造的浪費**であり、差分経路は編集ブロック+6 近傍の ≤42 面のみ
再評価する (実測 4,986 面 vs 全量はセクション当たり最大 24,576 面)。

機械保証 (bench 停止ガード): **面集合パリティ touched 136 セクション全てで
不一致 0** (= 差分維持メッシュと FULL rescan の面集合が bit 一致、
全 key 空間 24,576 × 全 touched セクション照合)。差分の除去面は dead-tri
(零面積三角形) 退避し、dead 率 12.5% または 16 連続編集で一次再最適化
(頂点圧縮 + Tipsify、実測 reopts=38)。時間は C 全量比 **9.9x**、A と同速度で
真の dedup 遮蔽を維持、再生成バイトは C 全量の **6.4%**・A の **0.9%**。

**正直な悪化注記**: 差分維持中の ACMR=1.049 は全量+Tipsify の ACMR=0.858
より +0.191 悪い — dead-tri 償却期間は頂点キャッシュ局所性が逐次劣化し
reopt 時に回復する設計の帰結 (改善は §8 backlog)。計測は warmup (lazy
初回構築) 後の timed replay (同一編集列の 2 巡目 = 償却領域) のみ。
run 間 CPU ノイズ ±3-5% (A 84.7-85.5 / B 275-310 / C 全量 820-840 /
C 差分 84.7-88.7 ms、bytes・パリティ・ACMR は決定的で不変)。

単純比較 (wave 213 時点凍結): single-run の CPU 時間は全量 C が最大
(真の dedup + Tipsify を全汚染セクションに適用するため)。生成バイト量は
全量 C で **A の 14.5%** (dedup 減頂点の効果)。編集頻度の高い実運用では
「再生成物のサイズ」が VRAM 転送量・arena 圧力に直結する → wave 215 の
差分経路は時間・バイト双方でこの構図を解消した。

### 5.8 合成「重い 1 フレーム」 (再メッシュ x4 + load x8 + save x2 + 実体 1 パス)

| pipe | 合成時間/frame | 計算律速 FPS |
|---|---|---|
| A Vanilla系 | 35.11 ms | 28 |
| B Sodium系 | 36.60 ms | 27 |
| C Rsift | **33.34 ms** (旧 67.3 ms → wave 213 HJ → wave 214 HK-1) | 30 |

speedup vs A: 0.96x (B), **1.05x (C)** (旧 0.54x — wave 212 HI + 213 HJ +
214 HK-1 の実測で C は単一スレッド合成でバニラ系を初めて上回る範囲に入った。
実体カリング成分 2.57 → 0.53 ms が寄与。なお A の絶対値は run 間 CPU
ノイズで ±3-5% 変動するため 1.00-1.05x は同位置 — 同一 run 内の成分値のみ
が厳密)

**この数値の読み方 (重要)**: 本ハーネスは mesher を全 pipe 同一スレッドに揃えた
**アルゴリズム特性の比較モデル**であり、実機 FPS の近似ではない。実機で
Sodium が速い主因は (a) 頂点バイト 37.5% 減 → GPU 帯域・VRAM, (b) リージョン
multidraw → ドライバ負荷減, (c) ワーカースレッド並列リビルド, (d) 遮蔽カリングの
4点であり、これらは個別表 (5.1/5.5/5.7/5.4) に実測で現れている。

## 6. 分野別マトリクス

| 分野 | 勝者 | 実測根拠 |
|---|---|---|
| 頂点メモリ量 | **C Rsift** | 3.65 MB (A 比 86.8% 減 / dedup 減頂点込み) |
| メッシュ CPU 時間 (単一スレッド) | **A/B < C** (差は縮小) | 199/214/286 ms (VisGraph は A/B 同額。C は真の dedup+Tipsify の真コスト。旧 547 → wave 213 HJ で 286) |
| 頂点キャッシュ品質 (ACMR) | **C Rsift** | 1.669→0.929 (A/B はエンコードのみで最適化なし。差分維持パスは dead-tri 償却の帰結で 1.049 = 全量再最適化より +0.191 悪 → wave 215 誠実記録・§8 backlog) |
| リージョン I/O 時間 | **C Rsift** | 7.6 ms vs 46.9 ms (6.2x) |
| 実体の真の遮蔽カリング | **C Rsift** | 600→125 (B は FOV までの 422, A は 600。DDA 遮蔽純粋比較の legacy 基準 175 から wave 214 V2 は時間 4.9x で実質同等深カリング) |
| セクション遮蔽 (丘越し) | 引分 (A/B=C 一致) | S2 で 15=15 |
| draw call 数 | **C Rsift** (固定 2) | B はリージョン数依存、A はセクション数依存 |
| 半透明ソート正確性 | **C Rsift** | 合併誤順 S1: 7.53% < A 8.65% < B 9.63% / S2: 5.19% < B 6.00% < A 6.13% |
| 半透明ソート時間 | **A 最小** | A 2.6 / B 12.5 / C 24.2 ms (旧 C は 3.6 s → wave 212 HI で 143x。A 比では依然 ~10x だが、正確性 1.1 pp 向上との引換) |
| 編集ワークロード (時間/再生成バイト) | **C Rsift 差分** (wave 215) | 84.7-88.7 ms (C 全量比 9.9x・A 同等 84.7-85.5 ms) / 1.56 MB (A の 0.9%・C 全量の 6.4%)。面集合パリティ全 touched セクション不一致 0 = 出力 bit 同一。差分 ACMR +0.191 悪は誠実併記 (§5.7) |
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
4. C の DDA 実体カリング 529 µs (wave 214 V2) は償却前提なし
   (`period_ticks=1`) の全件測定。実機では period=10 の差分
   スケジューリングで ≈53 µs/tick 平均に償却される (実体 600 基準)。
5. 疑似ワールドは水-水 内部面も生成する (全 pipe 同一の面規則で比較自体は
   公平。本物 MC の流体レンダラは水面のみ描くため、半透明クアッドの
   絶対量は実機より過大 = ソートに対し意図的に厳しいストレス条件)。
6. C の半透明ソート (~24 ms/シナリオ, wave 212 HI 後) は依然 3 pipe 最遅
   (A 2.6 ms)。残課題は関係評価/Kahn の GPU compute 化。ただし既往の
   ~3.6 s は limit 超過セクションへの「全損 O(n²) topo 試行」という実装
   欠陥が主因であり、門番化で解消済 (試行省略でも正確性はむしろ向上)。
7. C のメッシュ CPU 時間 285.6 ms は依然 3 pipe 最大 (§5.1 経緯参照)。
   Tipsify 自体は wave 213 HJ (同一意味論の線形化: u64 単調キー + 無変化
   再スコア省略 + cache_pos 差分更新) で 507 → 189 ms まで縮小。残課題は
   貪欲ヒープの定数倍の更なる削減と、セクション間並列化/GPU 化。
   (HJ-1 記録: LRU スタレネスを消す「一括化」は bit 不一致のため採用却下)
8. C 差分メッシュ (wave 215) の「再構築バイト 1.56 MB」は頂点バイトの
   再書込累積のみを計上 — kill 時の index インプレース書換 (1 面あたり
   24 B) は全量側の index も未計上で**両 pipe 同基準**。差分経路の初回
   タッチは build_full (≈全量 1 回分と同コスト) を要し bench は warmup
   として非計測、timed replay は編集 2 巡目以降の償却領域を測定する —
   初回構築込みの連続編集では両者の差は編集回数に比例して縮む。固定コスト
   (slot_tab 132,651 u32 = 0.53 MB + 頂点/index 蓄積) は編集の多い
   セクションでのみ発生する設計。

## 8. 今後の課題

- 差分メッシュ (wave 215) の ACMR 改善 (1.049 → 全量級 0.858 へ):
  dead-tri 蓄積期の頂点局所性劣化に対し、reopt 閾値の動的化・kill 連鎖での
  局所 index 再編・dead 頂点の即時再利用 (append 安定性との両立が設計課題)
- 半透明ソート関係評価 / Kahn / Tipsify の GPU compute 化
  (iGPU コア WGSL 制約内: compute + RO/RW storage buffer + 少量 readback)
- 実機 (iGPU 搭載 PC) での frame_proof_extra gpu-* モードと組み合わせた
  end-to-end 実測
- ワールド規模の拡大 (複数リージョン) での multidraw 集約率スケール測定
- **セクション可視性の C 独自 DDA 経路は速度・精度の双方で A/B graph に
  劣後** (S1: 600 µs/61 可視 vs 2.3 µs/7 可視 — wave 214 実測。61 > 14
  (基準 frustum+fog) は FOV 非考慮の球カリング語彙の違いを含むが、
  FOV を強制した V2 実験でも pop 大量 (16³ 大 AABB に 5 レイは粗すぎ、
  S2 で 15→3) かつ過剰可視残留 (S1 39) で代替不可と機械判定)。
  horizons/heightmap 方式か graph 由来の厳密方式への設計転換が本丸。
- 実体カリング V2 の pops 4 件 (0.9%) の更なる削減 (9 レイ帯は +1 件の
  改善に 1.8x を要求 — 採算は別 workload での再検証が前提)。
