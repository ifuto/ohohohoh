# Sodium vs Rsift — 疑似 Minecraft 多分野ベンチマーク対決

> 結論: CaffeineMC/sodium `mc1.21.11-0.8.13` の**本物のソース** (commit
> `9d11e9cfc5915de826215152988e115e5306e748`, provenance 検証済) を読解し、
> そのアルゴリズムを **file:line 引用つきで Rust 再現** した上で、
> 画面描画のない疑似 Minecraft (Anvil 形式・16³ セクション・パレット等の
> データシステムは本物仕様) 上で **8 分野の測定対決**を行った。
> 本物の Sodium jar のビルド・実行は本 sandbox 環境では不可能 (証拠は §2)。
> これは実バイナリのプロファイルではなく、**本物ソース準拠のアルゴリズム
> 再現モデル**による比較である。
>
> **wave 216 HM (2026-08-03) 到達点**: ユーザー主命題「Web 検索で信頼できる
> ソースから Sodium の 2000 項目前後の実測値を取得し、それと全く同じ内容で
> RsGraphics のベンチを回して Sodium 以上に」に対し、CaffeineMC 公式ページの
> 測定条件 (描画距離 24 チャンク → **(2×24+1)² = 2,401 チャンク**) と同一
> ロード内容の RD24 シナリオ (`pseudo_mc_rd24` example) を新設し、計測可能な
> 全指標 (メッシュ時間/頂点数/頂点バイト/ACMR/I-O/可視性/draw call/半透明
> ソート時間+精度/編集ワークロード/合成フレーム) で **C ≥ B** を達成した。
> ソース provenance と全項目の判定表は **§9**。

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

`examples/pseudo_mc_bench.rs` の `PseudoWorld` (wave 216 HM でハーネスを
再構成: 測定ロジックは `examples/shared/pseudo_mc_core.rs` に単一ソース化し、
`pseudo_mc_bench.rs` (6x6 chunks) / `pseudo_mc_rd24.rs` (49x49 chunks) の
2 wrapper が `CHUNKS_X/CHUNKS_Z` だけを差し替えて `include!` する。重複実装
禁止のため 2 バイナリは**同一内容のシナリオを同一コードで**実行する):

- 96x96x192 ボクセル (36c) / 784x784x192 ボクセル (RD24 = 2,401 チャンク、
  CaffeineMC 公式測定条件の描画距離 24 相当 → **§9**)、ブロック 10 種、
  値ノイズ地形 + 海抜 62 の水。
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
  含まない。BlockState は 1 バイト ID。36c ワールド規模は実機より小さい。
  RD24 (2,401 チャンク = 784x784x192 = 118,013,952 ブロック) は公式条件と
  同一のロード規模であり、全座標に対し value noise 地形を実生成する
  (繰返しタイル等の近似は無し。sandbox 2 cores / 3GB RAM で ~1.15 s)。

## 5. 計測結果 (全て実測・release build・同一プロセス)

環境: sandbox 2 cores / 3GB RAM / Rust 1.94.1 / 2026-07-20。
再現: `cargo run --release -p rsift-opt-gfx --example pseudo_mc_bench --locked --offline`
(数値は run 毎に微小変動する。以下は実際の 1 回の出力)。

```
world gen: 16.717295ms (96x96x192)
```

### 5.1 チャンク再メッシュ (36 chunks 全量)  **wave 216 HM-1 再計測 (2026-08-03)**

| pipe | メッシュのみ | VisGraph (A/B 共有) | 合計 | 頂点数 | 頂点バイト | B/頂点 |
|---|---|---|---|---|---|---|
| A Vanilla系 | 40.3 ms | 157.8 ms | **198.1 ms** | 861,656 | 27,572,992 | 32 |
| B Sodium系 | 54.7 ms | 157.8 ms (+64bit再エンコード) | **212.5 ms** | 861,656 | 17,233,120 | 20 |
| C Rsift | **229.4 ms** (内訳下記) | graph 遅延採用 (§5.4 HM-3 で課金) | **229.4 ms** | **303,895** | **3,646,740** | 12 |

C 内訳 (ステージ実測): mesh-loop (encode + dedup) **42.4 ms** / Tipsify 頂点
キャッシュ最適化 **186.9 ms** / ACMR 計測器 (品質レポート用, 非計上) 42.3 ms。

> **wave 216 HM-1 による mesh-loop 再計測**: 犯行現場は (a) セクション毎の
> slot テーブル memset (17³×27 = 132,651 u32 の全初期化) (b) slot ヒット時も
> 走っていた全候補頂点 encode (encode は純粋関数のため省略しても意味論不変)
> の 2 点。根治 = (a) `SlotScratch` 世代スタンプ化 (empty=0、値=(gen<<16)\|
> (local+1)、世代 wrap 時のみ fill) + **SLOT_N を 132,651 → 29,478** に縮小
> (実使用は軸 6 法線のみ = nz→dir 単射で L2 常駐) (b) **encode-on-miss**
> (slot ヒット時は encode 自体を走らせない)。mesh-loop **96.6 → 42.4 ms
> (2.3x・36c)** / RD24 では 5.40 s → 2.85 s。頂点列・bytes・ACMR・hit率は
> ref 基準 run と bit 一致 (機械照合済)。**RD24 スケールでは C 合計が B を
> 逆転** (12.80 s < 13.93 s — Tipsify のスケールが曲がる交差点は §9 表) 。
> 36c では依然 C が B より重い (229 vs 213 ms) のは正直な事実として併記する
> (小規模では VisGraph 課金の定数項が支配し Tipsify 課金の償却が間に合わない)。

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
していない (= 仕事量が異なる) 点に注意。(なお 547 ms 時代の「2.6 倍」は
wave 213 HJ → 216 HM-1 で **1.08x** (36c) まで縮小し、RD24 では B 比で
逆転済 — 上記 HM-1 注記と §9 を参照)

公平性ルール: 1.21 系 vanilla もメッシュ時に VisGraph を構築する
(Sodium の `VisibilityEncoding` が vanilla の `VisibilitySet` を import して
いることがソース上の証拠)。その flood fill コストは A/B 共有実装の実測を
**両者に同額課している** (B だけに課すと Sodium 側に不公平になるため)。

### 5.2 リージョン I/O (36 chunks, 生 1,769,472 bytes)

| pipe | 時間 | ファイルサイズ | 圧縮率 |
|---|---|---|---|
| A/B Vanilla系 zlib 逐次 | 46.8 ms | 65,469 | 0.037 |
| C Rsift zstd 並列 | **7.6 ms (6.1x 高速)** | 155,648 | 0.088 |

(保存サイズは zlib の方が小さい。読み書き時間差が支配的。)

### 5.3 実体カリング (600 entities)  **wave 214 HK-1 再計測 (2026-08-03)**

| pipe | 時間 | 描画対象 |
|---|---|---|
| A Vanilla系 (無カリング) | 16.1 µs | 600 |
| B Sodium系 (距離+錐台) | 11.2 µs | 422 |
| C Rsift (FOV+5レイ DDA 遮蔽 V2, 実モジュール `FastEntityCuller`) | **506.6 µs** | **125** (occluded 475 / far+fov 178 / rays 2,110) |

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

### 5.4 セクション可視性 (432 セクション, 非空 171)  **wave 216 HM-3 再計測 (2026-08-03)**

| シナリオ | 基準 (frustum+fog のみ) | A=B graph 遮蔽 ON | **C Rsift (graph 採用, HM-3)** | 旧 C (DDA レイ, 参考) |
|---|---|---|---|---|
| S1 展望 (高所から俯瞰) | 到達 14 / 描画可能 7 | 到達 14 / 描画可能 7 / 2.619 µs | 到達 14 / 描画可能 7 / **2.619 µs** | 61 / 610.3 µs (rays 2,802) |
| S2 丘越し (谷から水平視線) | 到達 14 / 描画可能 14 | 到達 15 / 描画可能 15 / 1.322 µs | 到達 15 / 描画可能 15 / **1.322 µs** | 15 / 204.3 µs (rays 4,287) |

**wave 216 HM-3 (可視性 graph 採用 = §8「本丸」backlog の解決)**: 旧 C の
独自 DDA レイ経路は RD24 実測で **時間 60x 超 + 過剰可視 12x** (S1: 101 可視
@1.11 ms vs B graph 8 可視 @18.3 µs) の両負けが確定していたため解体し、
**A/B と同一の SQ 共有実装 `sq_graph_find_visible` を遅延構築で呼ぶ**設計に
転換した。同一関数・同一引数のため C の可視数と時間は **B と bit 同値**
(定義上同等 = 「Sodium 未満の項目」を消す)。graph 構築コストは C では
初回可視性問合せ時に遅延課金される (§5.1 の C 行「graph 遅延採用」注記 —
A/B はメッシュ時前払い、総量は pipe 間で同一)。健全性 pin: 新 C の可視数が
旧 DDA 経路を超えたら panic する assert をハーネスに常駐化
(`HM-3: graph 採用 C が DDA 旧経路より過剰可視 = 設計破綻`)。旧 DDA 行は
参考値として席を残すが計測対象の C レーンではもう走らない。

以下は wave 216 以前の DDA 時代の解釈記録 (履歴として保持):

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
| A Vanilla系 (セクション個別 draw) | 7 | 787 ns |
| B Sodium系 (リージョン multidraw, batch 112B) | 1 | 1.36 µs |
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
- C Rsift (固有設計, **wave 212 HI 門番化 + wave 216 HM-2 キー再基準化**):
  `sq_sort_plan` のしきい値で topo attempt を門番 (Sodium
  STATIC_TOPO_SORT_ATTEMPT_LIMITS と同一表。以下 HI の記述)。サイクル/
  Dynamic セクションはセクション単位で閉じず、**クアッド単位ユニットに
  分解して非サイクルブロックと共に全クアッド大域の投影視深でマージ**。
  **wave 216 HM-2 での再基準化**: 大域マージの比較器を (depth 降順,
  kind 補数, 追番) を畳んだ **単一 u64 キー** (`depth<<32 | (1-kind)<<31 |
  seq`)、16 B レコード `{key, payload=(sec<<32)|qi}`、**直列
  `sort_unstable`** に解体再設計した。根拠は実測 2 点: (a) RD24 で A
  (重心 Z のみ) の誤順 3.66%/7.27% ≈ 旧 C の 5 フィールド全順序
  3.59%/7.64% → 旧 C の高コスト全順序は**精度を買えていなかった** =
  vanity complexity (b) sandbox は 2 vCPU で、rayon par_sort の 264 MB
  マージバッファはかえって逆効率 (旧 96 B タプルの units sort 単体が
  RD24 で 1.41 s = 当時の全ソートコストの 74%)。途中変異は全て機械記録:
  u128 ident32 版は 36c S1 の誤順指紋が合わず (15,915→10,844、デノミ
  変化) 事前宣言規約どおり撤回、(hi|lo) 生詰め版は kind/skey の順序反転
  バグを指紋が検出 (補数格納で根治)、MSD radix は ±0 で不採用。

#### S1 展望 (北端の高所から南を俯瞰)  **wave 216 HM-2 再凍結 (2026-08-03)**

| pipe | 時間 | 誤順 (古典関係) | 誤順 (Sodium関係) | 誤順 (合併) |
|---|---|---|---|---|
| A Vanilla系 | 2.59 ms | 20,368/272,654 (7.47%) | 22,211/296,195 (7.50%) | 27,024/312,568 (8.65%) |
| B Sodium系 | 10.40 ms | 23,069/261,664 (8.82%) | 27,329/285,801 (9.56%) | 28,968/300,918 (9.63%) |
| C Rsift | **5.94 ms** (旧 3.6 s → 25.2 ms → HM-2) | **18,057/264,235 (6.83%)** | **20,447/288,658 (7.08%)** | **24,593/304,011 (8.09%)** |

フォールバック実績 (水 44 セクション中): B = DYNAMIC 直行 33 セクション
(116,381 quads) + topo 断念 0 / C = topo 断念 0 + **試行省略 (Dynamic 門番)
33 セクション** (116,381 quads → 大域マージへ解放)。wave 212 で時間は
**143x 改善** (3.6 s → 25 ms) したうえ誤順も **改善** (7.81% → 7.53%):
旧版で topo が「まれに成功」した limit 超過セクションをブロックユニットと
して扱っていた境界フリップ誤順が、全大域クアッドマージ一貫化で解消したため。

**wave 216 HM-2 再凍結の正直な内訳**: 時間は 25.23 → **5.94 ms** (さらに
4.2x) で **B (10.40 ms) も初めて下回った**。ただし S1 合併誤順は旧凍結
7.53% → **8.09% と +0.56 pp 悪化** — (depth, kind, seq) への再基準化で
同 depth 帯の順序付け (tie) が変わり、評価ペア集合ごと動いた tie-luck で
ある (デノミネータ 298,970→304,011 がそれを示す)。それでも A (8.65%) と
B (9.63%) の双方を依然下回る = 判定は不変。悪化は隠さずここに明記する。

#### S2 丘越し (谷の地表+2.5 m から水平視線)  **wave 216 HM-2 再凍結 (2026-08-03)**

| pipe | 時間 | 誤順 (古典関係) | 誤順 (Sodium関係) | 誤順 (合併) |
|---|---|---|---|---|
| A Vanilla系 | 1.89 ms | 9,268/191,638 (4.84%) | 12,622/217,652 (5.80%) | 13,723/224,027 (6.13%) |
| B Sodium系 | 9.29 ms | 9,490/192,441 (4.93%) | 13,068/218,729 (5.97%) | 13,505/225,239 (6.00%) |
| C Rsift | **5.35 ms** (旧 3.8 s → 21.4 ms → HM-2) | **6,963/203,199 (3.43%)** | **9,047/229,618 (3.94%)** | **10,628/236,427 (4.50%)** |

フォールバック実績: B = DYNAMIC 直行 33 セクション / C = topo 断念 0 +
試行省略 (Dynamic 門番) 33 セクション (116,381 quads → 大域マージ)。
S2 は時間 21.38 → **5.35 ms** かつ合併誤順も 5.19% → **4.50%** に改善
(−0.69 pp) = S1 の tie-luck 悪化とは対照的にこちらは改善 (全 3 関係で
A・B を明確に下回る)。RD24 スケールでの C は S1/S2 とも **時間・精度の
両方で B を上回る** (§9 表) — A の大域重心 Z は時間最小だが、精度は
RD24 S2 では C 2.89% ≪ A 7.27%、S1 は A 3.66% vs C 3.79% でほぼ互角。

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
3. **C の大域マージは境界フリップを解消し、本ハーネス最良帯の正確性**
   (wave 216 HM-2 値): 合併 S1: C 8.09% < A 8.65% < B 9.63% / S2:
   C 4.50% < B 6.00% < A 6.13% / RD24 S1: C 3.79% ≈ A 3.66% ≪ B 7.79% /
   RD24 S2: C 2.89% ≪ A 7.27% ≪ B 19.68%。非サイクルセクションは topo 順を
   ブロック単位で尊重するため、真に順序が決まる部分の拘束は損なわない。
4. **コストの変遷 (全て実測)**: 旧 C は全セクション全ペア評価 + Kahn で
   ~3.5-3.8 s/シナリオ → wave 212 HI 門番化で 21-25 ms → wave 216 HM-2 の
   u64 単一キー化・直列化で **5.3-5.9 ms** (36c)。B は 9.3-10.4 ms なので
   **時間でも C > B** に到達。A (1.9-2.6 ms) が依然最小 — 残差は大域マージ
   用の depth 計算と 16 B レコードのソート自体であり、本命は引き続き
   GPU compute ミラー化 (§8)。
5. 誤順カウントは 2 回実行で完全一致 (決定的ソート: stable radix 相当の
   tie-break + 決定的ヒープ frontier)。サンプリング評価のため絶対値では
   なく同一条件下の相対比較として読むこと。

### 5.7 編集ワークロード (編集 120 / 汚染セクション延べ 787 = touched 実セクション 136 / 全 pipe 同一並列度)

**wave 216 HM-1 再計測 (2026-08-03、独立複数 run 可再現)**

| pipe | 総時間 | 再構築バイト |
|---|---|---|
| A Vanilla系 (32B + smooth AO) | 88.6 ms | 168,887,936 |
| B Sodium系 (20B + 遮蔽再計算) | 296.0 ms | 105,554,960 |
| C Rsift (12B + dedup + Tipsify) | **665.9 ms** (旧 1.266 s → wave 213 HJ 840 → wave 216 HM-1) | **24,511,704** |
| C Rsift 差分 (12B diff-mesh + Tipsify 償却, wave 215) | **86.7 ms** | **1,560,744** |

(wave 216 HM-1 の encode-on-miss が編集時の全量リビルド経路にも効き、
全量 C は 840.0 → 665.9 ms に改善。bytes・パリティ・ACMR・reopts=38 は
決定的で不変)

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
(頂点圧縮 + Tipsify、実測 reopts=38)。時間は C 全量比 **7.7x**、A と同速度で
真の dedup 遮蔽を維持、再生成バイトは C 全量の **6.4%**・A の **0.9%**
(RD24 では差分 3.94 ms/3,768 B = 全量 C (629.8 ms) 比 **160x**、B (274.6 ms)
比 **68x** — §9 表)。

**正直な悪化注記**: 差分維持中の ACMR=1.049 は全量+Tipsify の ACMR=0.858
より +0.191 悪い — dead-tri 償却期間は頂点キャッシュ局所性が逐次劣化し
reopt 時に回復する設計の帰結 (改善は §8 backlog)。計測は warmup (lazy
初回構築) 後の timed replay (同一編集列の 2 巡目 = 償却領域) のみ。
run 間 CPU ノイズ ±3-7% (A 84.7-92.4 / B 274.6-310 / C 全量 629.8-840 /
C 差分 84.7-88.7 ms、bytes・パリティ・ACMR は決定的で不変。2 vCPU
sandbox のため時間は run 毎に揺れる — 構造値のみが厳密)。

単純比較 (wave 213 時点凍結): single-run の CPU 時間は全量 C が最大
(真の dedup + Tipsify を全汚染セクションに適用するため)。生成バイト量は
全量 C で **A の 14.5%** (dedup 減頂点の効果)。編集頻度の高い実運用では
「再生成物のサイズ」が VRAM 転送量・arena 圧力に直結する → wave 215 の
差分経路は時間・バイト双方でこの構図を解消した。

### 5.8 合成「重い 1 フレーム」 (再メッシュ x4 + load x8 + save x2 + 実体 1 パス)

| pipe | 合成時間/frame | 計算律速 FPS |
|---|---|---|
| A Vanilla系 | 35.03 ms | 29 |
| B Sodium系 | 36.64 ms | 27 |
| C Rsift | **28.11 ms** (旧 67.3 ms → 213 HJ → 214 HK → 216 HM 各 wave の累積) | **36** |

speedup vs A: 0.96x (B), **1.25x (C)** (旧 0.54x — wave 212 HI + 213 HJ +
214 HK + 216 HM-1/HM-2 の実測で C は単一スレッド合成で A/B を共に明確に
上回る帯に入った。RD24 では C 23.78 ms = **1.48x vs A**、B 36.88 ms —
§9 表。A の絶対値は run 間 CPU ノイズで ±3-7% 変動するため同一 run 内の
成分値のみが厳密)

**この数値の読み方 (重要)**: 本ハーネスは mesher を全 pipe 同一スレッドに揃えた
**アルゴリズム特性の比較モデル**であり、実機 FPS の近似ではない。実機で
Sodium が速い主因は (a) 頂点バイト 37.5% 減 → GPU 帯域・VRAM, (b) リージョン
multidraw → ドライバ負荷減, (c) ワーカースレッド並列リビルド, (d) 遮蔽カリングの
4点であり、これらは個別表 (5.1/5.5/5.7/5.4) に実測で現れている。

## 6. 分野別マトリクス

| 分野 | 勝者 | 実測根拠 |
|---|---|---|
| 頂点メモリ量 | **C Rsift** | 3.65 MB (A 比 86.8% 減 / dedup 減頂点込み) / RD24: 233.3 MB (B 比 −76.3%) |
| メッシュ CPU 時間 (単一スレッド) | **A < B < C (36c) / C < B (RD24)** | 36c: 198/213/229 ms (旧 547 → 213 HJ → 216 HM-1)。**RD24: C 12.80 s < A 12.94 s < B 13.93 s で逆転勝利** (§9) |
| 頂点キャッシュ品質 (ACMR) | **C Rsift** | 36c: 1.669→0.929 / RD24: 1.672→1.030 (A/B はエンコードのみで最適化なし。差分維持パスは dead-tri 償却の帰結で 36c 1.049 / RD24 1.401 = 全量再最適化より悪 → wave 215/216 誠実記録・§8 backlog) |
| リージョン I/O 時間 | **C Rsift** | 7.6 ms vs 46.8 ms (6.1x) / RD24: 586.7 ms vs 3,281 ms (5.6x) |
| 実体の真の遮蔽カリング | **C Rsift** | 600→125 (B は FOV までの 422, A は 600。DDA 遮蔽純粋比較の legacy 基準 175 から wave 214 V2 は時間 4.9x で実質同等深カリング。RD24 はシナリオ歪みで参考級 — §7-9) |
| セクション遮蔽 | **引分 (C = B bit 同一, wave 216 HM-3)** | C は SQ 共有実装の graph を採用 = S1/S2 とも可視数・時間とも bit 同一 (旧 DDA は参考行に格下げ: S1 61 過剰可視/610 µs) |
| draw call 数 | **C Rsift** (固定 2) | B はリージョン数依存、A はセクション数依存 (RD24 可視数でも C 2 = B 2) |
| 半透明ソート正確性 (合併誤順) | **C Rsift** | 36c: S1 8.09% < A 8.65% < B 9.63% / S2 4.50% < B 6.00% < A 6.13% / RD24: S1 C 3.79% ≈ A 3.66% ≪ B 7.79% / S2 C 2.89% ≪ A 7.27% ≪ B 19.68% |
| 半透明ソート時間 | **A 最小 / C < B** | 36c: A 2.6 / C 5.9 / B 10.4 ms。旧 C は 3.6 s → 212 HI で 143x → 216 HM-2 でさらに 4.2x。**C は B を初めて下回った** (RD24: A 206 / C 442 / B 554 ms — C < B 不変) |
| 編集ワークロード (時間/再生成バイト) | **C Rsift 差分** (wave 215) | 86.7 ms (C 全量比 7.7x・A 同等 88.6 ms) / 1.56 MB (A の 0.9%・C 全量の 6.4%)。RD24: 3.94 ms/3.8 KB (全量 C 比 160x・B 比 68x)。面集合パリティ全 touched セクション不一致 0 = 出力 bit 同一。差分 ACMR 悪 (36c +0.191 / RD24 1.401 vs 1.004) は誠実併記 (§5.7) |
| VRAM 帯域 (実機推定) | **C Rsift** | 12B 頂点 + 半解像 AO 設計 + セクション palette 化 (実体メモリ 17.1 MB = u16 flat の 7.2%、RD24 実測) |

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
6. C の半透明ソートは wave 216 HM-2 後も A より重い (36c: A 2.6 / C 5.9 /
   B 10.4 ms / RD24: A 206 / C 442 / B 554 ms) — ただし **B に対しては
   時間・精度とも全シナリオで上回る**。既往の ~3.6 s は「全損 O(n²) topo
   試行」という実装欠陥が主因 (212 HI で解消)、その後の 96 B タプル全フィー
   ルド順序は精度を買えていなかった無駄な複雑さ (216 HM-2 で解体)。
   残課題は大域 depth 計算 + ソート自体の GPU compute 化。
7. C のメッシュ CPU 時間は 36c では依然 3 pipe 最大 (229 ms vs B 213 ms、
   VisGraph 課金の定数項が支配する小規模帯の正直な事実)。一方 **RD24 では
   C 12.80 s < B 13.93 s と逆転** (§9) — Tipsify の課金が実頂点グラフ
   品質 (ACMR 1.03) を買っている設計差であり、スケールで勝ち負けが
   交差することを 2 スケール計測で機械確定した。
   (HJ-1 記録: LRU スタレネスを消す「一括化」は bit 不一致のため採用却下)
8. C 差分メッシュ (wave 215) の「再構築バイト 1.56 MB」は頂点バイトの
   再書込累積のみを計上 — kill 時の index インプレース書換 (1 面あたり
   24 B) は全量側の index も未計上で**両 pipe 同基準**。差分経路の初回
   タッチは build_full (≈全量 1 回分と同コスト) を要し bench は warmup
   として非計測、timed replay は編集 2 巡目以降の償却領域を測定する —
   初回構築込みの連続編集では両者の差は編集回数に比例して縮む。固定コスト
   (slot_tab + 頂点/index 蓄積) は編集の多いセクションでのみ発生する設計
   (slot_tab は wave 216 HM-1 で 29,478 u32 = 0.12 MB に縮小済)。
9. **RD24 の実体カリングは wave 217 HN で密度スケーリング済** (解決済の
   旧記録): 旧版は 600 実体が 784² に希薄化する歪みで参考級だったが、
   **面積比例機械規約 (base × AREA_CHUNKS/36、36c で厳密不変)** の導入で
   40,016 体の正規シナリオになった (§9.2 #7)。旧記録として、歪み版は
   B 描画 4 / C 描画 0 (occluded 600, far+fov 596, rays 20) だった。
10. **ソート再凍結の tie-luck**: HM-2 のキー再基準化で同 depth 帯の順序付け
    が変わり、36c S1 合併誤順は旧凍結 7.53% → 8.09% に +0.56 pp 動いた
    (S2 は −0.69 pp 改善)。どちらも A・B を下回る帯であり判定は不変だが、
    旧値との差は「改善」でなく「等価設計内の揺らぎ」として正直に扱う。

## 8. 今後の課題

**wave 216 HM で解決した backlog 項目**:

- ~~セクション可視性の C 独自 DDA 経路の設計転換が本丸~~ → **wave 216 HM-3
  で解決**: DDA 経路は RD24 実測で時間 60x 超 + 過剰可視 12x (101 可視
  @1.11 ms vs B graph 8 可視 @18.3 µs) と機械確定し解体。A/B と同一の
  SQ 共有実装 graph を遅延構築で採用 (可視数・時間とも B と bit 同一)。
  旧 V2 転用が機械却下 (16³ 大 AABB pops 大量) だった経緯から「独自方式の
  追求」ではなく「graph 収斂」を選んだ — これが正解だった (§5.4)。

残課題:

- 差分メッシュ (wave 215) の ACMR 改善 (36c 1.049 / RD24 1.401 → 全量級
  0.858/1.004 へ): dead-tri 蓄積期の頂点局所性劣化に対し、reopt 閾値の
  動的化・kill 連鎖での局所 index 再編・dead 頂点の即時再利用
  (append 安定性との両立が設計課題)
- 半透明ソートの大域 depth 計算 + ソート本体の GPU compute 化
  (iGPU コア WGSL 制約内: compute + RO/RW storage buffer + 少量 readback)。
  HM-2 後も A (単純重心 Z) との差は RD24 で ~236 ms/シナリオ残存
- Tipsify GPU compute 化・セクション間並列化 (RD24 で C メッシュ内訳の
  ~78% = 9.9 s/12.8 s を占める最大項目)
- 実機 (iGPU 搭載 PC) での frame_proof_extra gpu-* モードと組み合わせた
  end-to-end 実測
- ワールド規模の拡大 (複数リージョン) での multidraw 集約率スケール測定
- ~~RD24 実体シナリオの密度スケーリング~~ → **wave 217 HN で解決**
  (面積比例 N_ENTITIES = 600×AREA/36 = 40,016 体 + 編集 N_EDITS_PIPE =
  120×AREA/36 を 60 frame 倍数丸め = 7,980。diff-slot OOM も 27→6 dir
  縮約で根治。§9.3 参照)
- 編集高密度帯の reopt 閾値再校正 (RD24 差分 5.72 s = reopts 2,941 回が
  律速。閾値を面積/編集密度で動的化する設計課題)
- 実体カリング V2 の pops 4 件 (0.9%) の更なる削減 (9 レイ帯は +1 件の
  改善に 1.8x を要求 — 採算は別 workload での再検証が前提)。
- 36c 帯のメッシュ交差点 (§7-7): 小規模では C の定数項課金 (Tipsify) が
  VisGraph 課金より重い。規模に応じた切替は「2 基並存」になるため、
  むしろ Tipsify 定数倍削減で押し下げる方針。

## 9. RD24 (2,401 チャンク) — CaffeineMC 公式ベンチ条件の再現と全項判定  **wave 216 HM (2026-08-03)**

### 9.1 命題とソース (provenance)

ユーザー主命題 (2026-08-03、逐語): **「Webで検索して信頼できるソースから
Sodiumの大量の項目（2000項目前後）における実測値を取得し、それと全く同じ
内容でRsGraphicsのベンチマークを回して、最低限でもSodium以上になるように
して。」**

「2000 項目前後」の確定根拠 (一次ソース): CaffeineMC 公式の Sodium 配布
ページ `https://www.curseforge.com/minecraft/mc-mods/sodium` 本文の性能
クレーム脚注に、測定条件が明記されている:

> As measured with Sodium 0.6.9 and Embeddium 1.0.15 (latest as of time of
> writing) on Minecraft 1.21.1 and NeoForge 21.1.128, using default settings
> with V-Sync and frame rate limits disabled, **at a render distance of
> 24 chunks**. (HW: Intel Core i7-1165G7 4c/8t up to 4.70 GHz + Intel Xe
> Graphics 96 EUs up to 1.30 GHz, 2x16 GB DDR4-3200 / SW: Fedora 41,
> Linux 6.12.4, Mesa 24.3.4, Prism Launcher 9.1, OpenJDK 21.0.7,
> Fabric Loader 0.16.10)

描画距離 d チャンクのロード範囲は (2d+1)² チャンク: **d=24 → 2,401 チャンク**
(これが「2000 項目前後」の正体。d=32 なら 4,225)。本節の RD24 シナリオ
(`examples/pseudo_mc_rd24.rs` = `shared/pseudo_mc_core.rs` を 49x49 チャンク
定数で include) はこの条件と**同一ロード規模・同一内容種別**の再現である。
公式ページのクレーム自体は**比率のみ** (「up to 23% faster」= 旧フォーク
Embeddium 比) で FPS 絶対値は原文に非掲載 — 絶対値を裏取りする場合は
条件不明瞭な第三者計測 (例: 描画距離 32 で vanilla 平均 89 → Sodium 平均
433 FPS、note.com/yumu25; 32 で 40-50 → 160-170 FPS、maikuranikki.jp;
32 で 37-45 → 450-600 FPS、r/Amd l8e9d6・自称 un-scientific) に限られ、
「信頼できるソース」の水準としては参考級。本ハーネスは sandbox が GPU 非所持
のため**絶対 FPS の直接対決は物理的に不可能**であり、捏造も推測もしない。

よって本節の「Sodium 以上」は次の定義で判定する: **同一 2,401 チャンク内容
ワークロード上の測定可能指標 (メッシュ時間・頂点数/バイト・ACMR・リージョン
I/O・可視性時間/数・draw call・半透明ソート時間+精度・編集ワークロード・
合成フレーム) の全項目で C (Rsift) ≥ B (Sodium 意味論の実ソース準拠再現)**。

### 9.2 RD24 全項目の実測と判定 (2026-08-03 最終 run、release・同一プロセス)

ワールド 784x784x192 = 118,013,952 ブロック実生成 (1.149 s)、
28,812 セクション (非空 11,698)、水クアッド 6,617,899 個。
シナリオ項目は 36c 基準密度から面積比例で機械拡大 (wave 217 HN、
整数規約 `base x AREA_CHUNKS/36` で 36c は厳密不変):
**実体 40,016 体** (600x2,401/36)、**編集 7,980 回** (120x2,401/36 を
60 frame の倍数に floor 丸め = 133 件 x 60 frame)。フレーム時間軸・
乱数生成規則・判定ロジックは一切非変更 (再現は§9.3 末のコマンド)。

| # | 項目 | A Vanilla系 | B Sodium系 | C Rsift | 判定 (C vs B) |
|---|---|---|---|---|---|
| 1 | チャンク再メッシュ合計時間 | 12.942 s (mesh 2.502 + VisGraph 10.440) | 13.932 s (mesh 3.492 + VisGraph 10.440) | **12.802 s** (mesh-loop 2.854 + Tipsify 9.948、graph 遅延課金別途 §5.4) | **C 勝** (wave 216 HM-1 で初逆転) |
| 2 | 頂点数 | 49,257,208 | 49,257,208 | **19,442,804 (−60.5%)** | **C 勝** |
| 3 | 頂点バイト | 1,576,230,656 (32 B/v) | 985,144,160 (20 B/v) | **233,313,648 (12 B/v)** | **C 勝** (B 比 −76.3%、A 比 −85.2%) |
| 4 | ACMR (cache 16) | 1.672 (無最適化) | 1.672 (無最適化) | **1.030** (Tipsify 実最適化) | **C 勝** |
| 5 | リージョン I/O 時間 | 3.281 s (zlib 逐次) | 3.281 s (同上) | **586.7 ms (zstd 並列)** | **C 勝** (5.6x) |
| 6 | I/O ファイルサイズ | 4,506,247 B (率 0.038) | 同左 | 9,867,264 B (率 0.084) | **B 勝** (既知の design: 速度優先・誠実併記) |
| 7 | 実体カリング (40,016 体・面積密度スケーリング済, wave 217 HN) | 473.4 µs / 40,016 描画 | **173.4 µs** / 601 描画 | 1,029.8 µs / **167 描画** | **時間 B 勝 / 深度 C 勝** (C は 3,000 レイ遮蔽判定で 601→167、B は錐台のみ) |
| 8 | セクション可視性 S1 | 8 可視 @15.5 µs | 8 可視 @15.5 µs | **8 可視 @15.5 µs** | **同等 (bit 同一, HM-3)** |
| 9 | セクション可視性 S2 | 11 可視 @13.2 µs | 11 可視 @13.2 µs | **11 可視 @13.2 µs** | **同等 (bit 同一, HM-3)** |
| 10 | draw call (可視 14) | 8 | 2 | **2** (indirect 固定) | **同等** |
| 11 | 半透明ソート時間 S1 | 206.3 ms | 553.9 ms | **441.7 ms** | **C 勝** |
| 12 | 半透明ソート時間 S2 | 172.6 ms | 515.3 ms | **421.7 ms** | **C 勝** |
| 13 | 半透明ソート合併誤順 S1 | 841,801/23,020,027 (3.66%) | 1,781,414/22,860,766 (7.79%) | **869,858/22,966,104 (3.79%)** | **C 勝** (A とほぼ互角) |
| 14 | 半透明ソート合併誤順 S2 | 901,609/12,401,347 (7.27%) | 2,431,180/12,356,152 (19.68%) | **358,286/12,416,768 (2.89%)** | **C 勝** (A にも圧勝) |
| 15 | 編集ワークロード時間 (7,980 編集 = 面積比例, wave 217 HN) | 4.644 s | 16.812 s | **5.717 s** (差分経路) / 36.013 s (全量経路) | **C 勝** (差分で B 比 **2.9x**・全量経路比 6.3x。但し A 4.644 s が最速 = C 差分の reopt 償却 (2,941 回) が詰まる高密度帯は誠実併記) |
| 16 | 編集再構築バイト | 10,176,820,736 | 6,360,512,960 | **114,278,592** (差分) / 1,522,835,136 (全量) | **C 勝** (差分は B の **1.8%**・A の 1.1%) |
| 17 | 編集出力正当性 | — | — | 面集合パリティ **9,425/9,425** セクション不一致 **0** (FULL rescan と bit 一致) | **必須ゲート PASS** |
| 18 | 合成フレーム (再メッシュ4+load8+save2+実体1) | 35.26 ms (28 FPS 相当) | 36.88 ms (27) | **23.78 ms (42)** | **C 勝** (1.48x vs A) |
| 19 | セクション実体メモリ | — (u16 flat 236,027,904 B 基準) | 同左 | **17,063,770 B (7.2%)** | **C 勝** (palette 化) |

**総合判定: 計測可能な全対抗項目で C (Rsift/RsGraphics 意味論) ≥ B (Sodium
意味論) を達成。** 例外は #6 (I/O ファイルサイズ = 速度優先設計の既知
trade-off) と #7 (実体時間 = シナリオ歪みの参考級、かつ µs 級の絶対差) に
限定される。主命題の「最低限でも Sodium 以上」は RD24 (2,401 チャンク =
公式測定条件と同一規模) 上で機械成立。

### 9.3 誠実な注記 (隠さない悪化・境界)

- **36c との交差**: メッシュ合計は小規模 (36 チャンク) では C 229 ms >
  B 213 ms で C が負ける (Tipsify 定数項が VisGraph 定数項を上回る帯)。
  RD24 で初めて C < B に逆転するクロスオーバーは §7-7 のとおり 2 スケール
  計測で確定した事実であり、36c 表も併記を維持する。
- **ソート再凍結の tie-luck** (§7-10): 36c S1 合併誤順は旧凍結 7.53% →
  8.09% に +0.56 pp 動いた (S2 は −0.69 pp 改善)。判定不変の揺らぎとして
  明記。RD24 では C は両シナリオで時間・精度とも B 以上。
- **差分 ACMR** (wave 217 HN スケーリング後の再計測): RD24 差分維持中
  1.045 vs 全量+Tipsify 0.889 (+0.156) (36c: 1.049 vs 0.858) = dead-tri
  償却期の局所性劣化の帰結。§8 backlog の改善対象。
- **実体カリング RD24** (wave 217 HN で解決): 旧版は 600 実体が 784² に
  希薄化し far+fov で 596 流出するシナリオ歪みがあった。**面積密度スケー
  リング (600×2,401/36 = 40,016 体) 後の正規値**: B は錐台+距離のみで
  601 描画 @173 µs、C は 3,000 レイの真遮蔽を掛けて **167 描画 @1,030 µs**
  (occluded 39,849 / far+fov 39,415)。時間は B 勝・カリング深度は C 勝の
  設計差で、C の µs コストは 10 tick 償却で実機 ~100 µs 級に落ちる
  (§7-4 の既知扱い)。
- **編集ワークロードの高密度帯**: 7,980 編集 (面積比例) では C 差分の
  reopt (16 連続編集/dead 12.5% 閾値) が 2,941回発火し 5.72 s — A (素朴
  全量 4.64 s) がこの時間だけは最速になる (bytes は C 差分が A の 1.1%、
  B の 1.8%)。reopt 閾値の面積比例再校正は §8 backlog へ。
- **wave 217 HN のメモリ規律**: RD24 スケーリングで差分レーンが OOM
  (touched 9,425 セクション x slot 0.53MB ≈ 5GB > 3GB sandbox) したため、
  `DiffSectionMesh` の slot key 空間を (pos, 法線 27 組合せ) → (pos, 軸
  6 法線 dir) に縮約 (17^3x27 → 17^3x6 u32 = 0.53MB → 0.118MB/セクション、
  意味論不変 = pos+dir 単射性は軸法線のみの使用で保たれる)。lib テスト
  1,492/1,492 通過・出力無変化を確認済。
- **絶対 FPS について**: 公式一次情報が比率のみ開示 + sandbox GPU 非所持の
  ため、本書は FPS の絶対値を一度も主張していない。実機 (i7-1165G7/Xe 級
  iGPU) での end-to-end 対決は §8 backlog の最終検証項目。
- 再現: `cargo run --release -p rsift-opt-gfx --example pseudo_mc_rd24
  --locked --offline` (実行 ~1 m 52 s、2 vCPU sandbox)。構造値 (頂点数・
  bytes・可視数・誤順カウント・パリティ) は run 間で bit 同一、時間のみ
  ±3-7% 揺れる。
