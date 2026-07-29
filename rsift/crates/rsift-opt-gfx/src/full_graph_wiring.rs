//! Full Graph Wiring — 全モジュールを実データで毎フレーム実実行するオーケストレーター。
//!
//! 監査 (AUDIT_STUB_WIRING.md B1/B2) 指摘を完全解消:
//!  - `size_of` リンク保証の廃止 → 各サブシステムを保持し **実入力で実実行**。
//!  - WGSL 文字列の `let _ =` 破棄 → `gpu_runtime` で実デバイス上に実コンパイル検証。
//!  - ダミー定数呼び出しの破棄 → 実チャンク AABB・実パレット・実クアッド・実ライト
//!    を入力し、結果 (カリング集合・露出・VRAM メーター・ガバナ予算) をパイプラインに
//!    **実効果として還元** する。
//!
//! 実効果の例:
//!  - QualityGovernor の連続オーバー検知が次フレームの chunk build 予算を縮小。
//!  - ao_bake コーナー AO が実パレット近傍からプル型クアッドの AO を上書き (視覚差)。
//!  - temporal_diff + generation cache で未変更列の再メッシュをスキップ (既存効果の実測器)。
//!  - OverdrawSorter の front-to-back 順序が upload イテレーション順に反映。

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use crate::binary_greedy_meshing::{idx as section_idx, SectionPalette};
use crate::packed4::PackedPullQuad;
use crate::svo::SparseVoxelOctree;
use tracing::{debug, info};

/// パイプラインから配線へ渡す実フレーム入力。
pub struct FrameWiringInputs<'a> {
    pub delta_ms: f32,
    pub frame_us_measured: u32,
    pub frame_index: u64,
    pub screen_w: u32,
    pub screen_h: u32,
    pub camera_pos: [f32; 3],
    pub camera_dir: [f32; 3],
    pub view_proj: [[f32; 4]; 4],
    /// このフレームに描画対象だったチャンク (build/hit 両方)。
    pub chunk_keys: Vec<(i32, i32)>,
    /// chunk_keys に 1:1 の支配マテリアル (pull クアッド先頭の tex id。無ければ 0)。
    pub chunk_materials: Vec<u32>,
    /// chunk_keys に 1:1 の AABB (min, max)。
    pub chunk_aabbs: Vec<([f32; 3], [f32; 3])>,
    /// chunk_keys に 1:1 のカメラ距離 (blocks)。
    pub chunk_dists: Vec<f32>,
    /// chunk_keys に 1:1 の実メッシュ index 数。
    pub draw_index_counts: Vec<u32>,
    /// このフレームに手続きした実 section パレット (最大 4 列分)。
    pub section_palettes: Vec<SectionPalette>,
    /// 実クアッド位置サンプル (最大 256)。
    pub quad_positions: Vec<[f32; 3]>,
    pub quad_materials: Vec<u32>,
    /// アップロード済みクアッド SSBO 総バイト数。
    pub quad_bytes: usize,
    /// カメラ速度 (blocks/frame — pipeline の motion adaptive 値)。
    pub camera_speed: f32,
    /// カメラ垂直 FOV (radian)。nanite proj_factor / more_culling の
    /// fov_tan_half で使用 (wave 84 CH-3: 旧 70° ハードコード近似から
    /// render_pipeline の実カメラ値への配線)。
    pub camera_fov_y: f32,
    /// SVO エンコードされた列があれば借用 (VCT が実走査する)。
    pub svo: Option<&'a SparseVoxelOctree>,
}

/// 配線がパイプラインへ還元する実効果集合。
#[derive(Debug, Default, Clone)]
pub struct FrameWiringReport {
    /// Governor 判定の次フレーム chunk build 予算 (None = 変更なし)。
    pub next_build_budget: Option<u32>,
    /// front-to-back の描画順 (inputs.chunk_keys の index 順序)。
    pub overdraw_order: Vec<usize>,
    /// GPU 間接描画コマンド数 (AZDO 圧縮後)。
    pub draw_command_count: u32,
    pub lockfree_cache_hits: u32,
    pub lbvh_culled: u32,
    pub aokana_visible_regions: u32,
    pub visgraph_reachable: u32,
    pub nanite_meshlets: u32,
    pub nanite_meshlets_culled: u32,
    pub vram_used_bytes: u64,
    pub post_exposure: f32,
    pub ambient_light: f32,
    /// GPU 光伝搬の実ディスパッチ結果: 16³ セクション内の点灯ボクセル割合 (0.0〜1.0)。
    pub clp_lit_fraction: f32,
    /// FRB ビルボードの本数計測 (実クアッドを (center, half, color) タプルの
    /// 形状へ組立てた個数)。【誠実注記 wave 83 CG-6】組立物は
    /// FragmentRayBoxIntersect に CPU 側入力 API が存在しないため送達されず
    /// 破棄される。「GPU 入力への実変換数」ではない (旧 doc を訂正)。
    pub frb_billboards: u32,
    /// 旧 vanilla render バイトコードフック (transpiler HEAD 挿入) の今フレーム実デルタ。
    /// 【非決定】プロセス全域の共有カウンタ由来のため、テスト並行実行では他テストの
    /// vanilla 呼出と混ざり得る。決定性検証の比較対象からは除外する (監査 K-4)。
    pub vanilla_hook_hits_delta: u32,
    /// 【非決定】電源モード判定が壁時計 (`init_time.elapsed()`) を要求する仕様のため
    /// フレーム間・実行間で揺らぐ。決定性検証の比較対象からは除外する (監査 K-4)。
    pub power_skip_extra: bool,
    /// Clustered 付きライトの最大クラスタ荷重 (実 index リストからの集計)。
    /// 【wave 135 EI-1】旧 `_max_cluster_load` 破棄から実フィールドへ配線
    /// (新指令 §7 消費者なし禁止の消化)。
    pub cluster_max_load: u32,
    /// DDGI probe 体積に対するカメラ位置の fractional probe 座標
    /// (実 `DdgiVolume::probe_coord` 由来、セル内補間格子位置)。
    /// 【wave 160 FF 捕捉 87】旧 `let probe = ...; let _ = probe;` 評価破棄
    /// (新指令 §7「評価実効・消費なし」) を report 実フィールドへ根治。
    pub ddgi_probe_coord: [f32; 3],
    /// 同体積のプローブ総数 (dims 積、`probe_count` usize 積の u32 min 飽和)。
    /// 【wave 160 FF 捕捉 87】ddgi::probe_count の実消費者配線。
    pub ddgi_probe_count: u32,
    /// この tick で確定した次回提示境界 (vsync snap 済み、累積 delta 時計上)。
    /// 【wave 161 FG 捕捉 91】next_present_time の実消費者配線 (Pacing 帳簿)。
    pub frame_next_present_ms: f64,
    /// EMA 平滑されたフレーム時間 (ms)。低スペックのスパイク耐性監視。
    /// 【wave 161 FG 捕捉 91】smoothed_frame_ms の実消費者配線。
    pub frame_smoothed_ms: f64,
    /// Pacer の目標リフレッシュレート (Hz、構築契約不変値)。
    /// 【wave 161 FG 捕捉 91】target_refresh_hz (private 化済み) の実消費者配線。
    pub frame_target_hz: f64,
    /// PSO library の累計ヒット数 (get 実測、wave 83 CG-7 で真のキャッシュ化
    /// 済みの計器)。【wave 163 FI 捕捉 94】stats() 消費者ゼロの実配線。
    pub pso_lib_hits: u64,
    /// 同、累計ミス数 (初回プロファイル遭遇数)。
    pub pso_lib_misses: u64,
    /// この tick にカメラ位置がデカール箱へ投影成立した個数 (実評価計測)。
    /// 【wave 165 FK 捕捉 98】旧 `let _ = decal_local(...)` 評価破棄を
    /// 実消費へ閉塞。EB-3 公知の登録経路不在では恒 0、登録時に真値を返す
    /// 計測点 (junction 保持設計は wiring 側注記参照)。
    pub decals_projected: u32,
    /// この tick に GigaVoxels 要求キューから実生成・アップロード扱いと
    /// なった brick 数 (process_requests の戻り値実測)。
    /// 【wave 166 FL 捕捉 101】旧 `_bricks` 破棄の実消費配線。
    pub gigavoxels_processed: u32,
    /// 処理後のプール常駐 brick 数 (BrickPool.resident_bricks.len() 実測)。
    pub gigavoxels_resident: u32,
    /// 【wave 168 FN 捕捉 105 §7 消化 30】rs_graphics() の root cost
    /// (DWORD 単位、定数 19 = 16 Camera + 1 bindless Table + 2 Material)。
    /// D3D12 64 DWORD 制限内の実行時 telemetry 化。
    pub root_cost_dwords: u32,
    /// 【wave 169 FO 捕捉 106 §7 消化 31】bundle_cache 帳簿 (累積値) の
    /// 実配線。hits=再利用回数・misses=初回生成回数 (u64、wiring 単位累積)。
    pub bundle_hits: u64,
    pub bundle_misses: u64,
    /// 1 灯以上帰属したクラスタ数 (同上、正規化後の実供給から集計)。
    pub cluster_lit_clusters: u32,
    /// GTAO オクルージョン (実パレット高さ場の中央断面スライス由来、[0,1]、
    /// 1=遮蔽なし)。【wave 136 EJ-1】旧実装は全サンプル高 ≤ center の合成
    /// スライスで定数 1.0 に退化 + `_ = gtao_occ` 破棄の中間構造 (EH-1 同型)
    /// だったものを、真の高さ場断面化+実フィールド書出しへ根治 (新指令 §7
    /// 配線実用化の消化)。パレットなしも 1.0 (遮蔽評価対象なし)。
    pub gtao_occ: f32,
    /// deinterleave 半解像度 AO パイプライン (デフォルト 1/4 コスト) の
    /// フル解像度再合成後の平均 AO。低スペック AO 半解像度化の品質監視。
    /// 【wave 136 EJ-1】モジュール全消費者ゼロだった deinterleave_ao の
    /// 実消費者配線 (新指令 §7 「接続か削除か」の前者を選択)。
    pub ao_halfres_mean: f32,
    /// 同、最小 AO (そのフレームで最も遮蔽が効いた観測点)。
    pub ao_halfres_min: f32,
    /// async compute 経済モデルの overlap 見積 (pass 作業量 proxy 写像、
    /// **実測 ms ではない**絶対値解釈不可の proxy、`async_saved_pct` の
    /// 比率のみ意味を持つ)。【wave 136 EJ-2】planner 全消費者ゼロ
    /// (wgsl_source 経由の WGSL 登録のみ) だった async_compute への
    /// 実消費者配線。係数表は PROXY_* 定数に固定文書化 (低スペック実機
    /// 計測後に校正する設計の仮係数、誠実注記)。
    pub async_overlap_proxy_ms: f32,
    /// 同、pipelined スケジュールの見積。
    pub async_pipelined_proxy_ms: f32,
    /// 同、(naive-pipelined)/naive の見積削減率 [%] (比率のため意味あり)。
    pub async_saved_pct: f32,
    /// LEO ring (最大 4096 ノード、pop_front 窓) の tag 8 スロット分布。
    /// Σ==ring len の不変式で wiring が maintain する実消費値。【wave 137
    /// EK-1】旧は allocate のみで ring 内容も payload も未消費の中間構造
    /// だったものを、分布集計と decode mirror で実消費者に接続 (§7 消化)。
    pub leo_tag_dist: [u32; 8],
    /// LEO ring 最後に登録されたノードの payload (= tick 値) を実読出し
    /// したもの (`get_payload` Option 版の真の消費地)。
    pub leo_latest_tick: u64,
    /// EN-1】旧 `_subgroup_mask` は評価後 `_` 破棄の中間構造だったものを
    /// 実フィールド化 (§7 消化): intensity > 8.0 の emissive light の
    /// ballot mask (bit j = lane j 充足、subgroup_ballot は 64 lane 超を
    /// 静寂切捨て — subgroup.rs 誠実注記 EN-3 参照)。
    pub emissive_high_mask: u64,
    /// EN-1】旧 `_subgroup_reduced` 同様: emissive intensity の wave
    /// (32 lane) 集約 sum の最大。空入力は None → +0.0、全 -0.0 経路も
    /// +0.0 正規化 (捕捉 55 同型の f32 .max(0.0) 正規化)。
    pub subgroup_wave_sum_max: f32,
    /// 【wave 141 EO-1】旧 `_caster` 固定引数 24.0 の `_` 破棄中間構造は根治済 (§7
    /// 消化 8): 全クアッドを仮想 caster として (x,z) 平面ノルム proxy を
    /// `caster_lod` に供給し lod 0..3 バケット件数を集計した分布
    /// (leo_tag_dist 同型の [u32; N] 分布契約)。
    pub shadow_caster_lod_dist: [u32; 4],
    /// 【wave 141 EO-1】同 proxy で `casts_shadow` が false (= culled) だった
    /// クアッド件数。【誠実注記】proxy は真のスクリーン投影寸法ではなく
    /// ワールド (x,z) ノルム (ビュー投影未接続)。
    pub shadow_casters_culled: u32,
    /// 【wave 142 EP-1】旧 `let _ = fos` で評価後 `_` 破棄の中間構造を根治 (§7
    /// 消化 9): 画面中央 uv=(0.5,0.5) の foveated shading rate。
    /// gaze は camera_dir (x,z) の ×0.5+0.5 NDC→uv 写像 (定数
    /// radius=0.2・min_rate=0.5 供給)。
    pub foveated_center_rate: f32,
    /// 【wave 143 EQ-1】`cull_lights_for_tiles` は wiring:1145 で実実行される
    /// が結果の消費者ゼロだった中間構造を根治 (§7 消化 10): タイルあたり
    /// 最大光源数 (ホットスポット指標、cluster_max_load 同型)。
    pub tdl_max_tile_load: u32,
    /// 【wave 143 EQ-1】同上: 非空 (1 灯以上割当) タイル数
    /// (cluster_lit_clusters 同型)。
    pub tdl_lit_tiles: u32,
    /// 【wave 144 ER-1】render_pipeline は depth_plan を保持・初期化
    /// するのみで plan() 未消費だった状態 (§7 消化 11) から、wiring 側
    /// に実消費者を追加: `DepthPrepassPlanner::for_low_spec().plan()`
    /// の pass 数。【選択誠実注記】render_pipeline は tier≥High で
    /// high_spec を選ぶが、wiring はプロジェクトの低スペック PC 制約
    /// (UserSystemPrompt §2) に整合する low_spec 構成を固定選択。
    pub depth_pass_count: u32,
    /// 【wave 144 ER-1】同上: plan() の total_cost (静的推定定数、
    /// 実測非接続 = module doc 注記 1)。
    pub depth_pass_cost: u32,
    /// 【wave 144 ER-2】`sort_translucent_indices` 消費者ゼロ (§7 消化
    /// 12) からの実配線: 素材判定 `mat%7==5` (material_batcher 731 行
    /// 既存規則と同一) の半透明クワッド総数。build_draws の
    /// translucent_draws 件数と同一判定から再計算した cross 不変式。
    pub translucent_total: u32,
    /// 【wave 144 ER-2】同上: 半透明クワッドをクアッド中心の距離²降順
    /// (back-to-front) にソートした先頭 = 最遠クワッドの元 index
    /// (空 → 0、実消費の観測値)。
    pub translucent_back_first: u32,
    /// 【wave 148 EU-2】`parallax_occlusion` は wiring 内で実呼出されていた
    /// ものの結果消費者ゼロ (`let _parallax_hit` 評価後破棄、§7 消化 13) の
    /// 中間構造だったものを実フィールド化: 監視点 uv=(0.5,0.5)・view は
    /// camera_dir 写像 (x→x, z→y, |y|.max(0.2)→z) で衝突した POM 層深度
    /// (パレット高さ場の層格子 [0,1]、パレットなし → 0.0)。
    pub parallax_layer_depth: f32,
    /// 【wave 148 EU-2】同上: POM 最終 uv の y オフセット (final_uv.y - 0.5、
    /// 負 = 高さ場の奥方向シフト)。golden は rq eu_pom 全導出 (bit 一致 pin)。
    pub parallax_uv_offset_y: f32,
    /// 【wave 149 GC-2】`should_render_gui` の判定は wiring:542 で実呼出
    /// されていたものの `let _gui_decision` で全結果消費者ゼロ (§7 消化 14)
    /// だったものを実フィールド化: この tick で GUI オフスクリーン面を
    /// 再描画すべきか (デュアルレート・スケジューラの実効判定)。
    pub gui_rendered_surface: bool,
    /// 【wave 149 GC-2】同上: 判定時点の目標 GUI FPS (anim 経路撤去後は
    /// GuiRates::gui_fps 定数、既定 30.0)。
    pub gui_target_fps: f32,
    /// 【wave 149 GC-2/GC-3】同上: この tick に入力イベント (wiring では
    /// camera_dir 変化を視点操作=入力駆動として実検出) で即時無効化が
    /// 発火したか (due 非依存の再描画トリガ)。
    pub gui_invalidated: bool,
    /// 【wave 149 GC-2】同上: 3D シーンは前回キャッシュ再利用か
    /// (module 仕様では常に true、契約 pin)。
    pub gui_reuse_cached_scene: bool,
    /// 【wave 149 GC-2/GC-4】Tick 終了時点の GUI 面 valid (screen_w/h
    /// 変化で invalidate() が実駆動される経路の観測面、
    /// `GuiCompositeClock::surface_valid()` の真の消費地)。
    pub gui_surface_valid: bool,
    /// 【wave 152 EX-1】`build_draws` の opaque draw 件数を実フィールド化:
    /// 旧実装は `let _ = (opaque_draws.len(), translucent_draws.len());` で
    /// draw 構造を丸ごと破棄していた (§7 消化 15 の中間構造破棄)。1 draw =
    /// 同一 material の連続 quad run (BatchedDraw、texture bind 削減単位)。
    pub material_opaque_draws: u32,
    /// 【wave 152 EX-1】同上: translucent draw 件数。
    pub material_translucent_draws: u32,
    /// 【wave 152 EX-1】両リストの Σquad_count = batcher に bin された quad
    /// 総数。cross 不変式 == quad_materials.len() を debug_assert で検算
    /// (全 quad が一意に bin される完全性)。捕捉 65 根治 assert の観測面。
    pub material_binned_quads: u32,
    /// 【wave 153 EY-2】FSR2 reproject 結果 uv (`let _ = reprojected` 破棄
    /// を根治、§7 消化 16)。監視点 uv=(0.5,0.5)、mv は jitter_uv (dims
    ///   消費の px→uv 正規化、旧 0.002 ハードコード撲滅)。
    pub fsr2_reproj_uv: [f32; 2],
    /// 【wave 153 EY-2】FSR2 アップスケール率 (output/input、GPU Uniforms
    /// inRes/outRes と同一次元の監視値 — 捕捉 67 の output dims 消費者)。
    pub fsr2_scale: [f32; 2],
    /// 【wave 154 EZ-1】mode_tick 判決の frame_dt (1/target_fps [s]、
    /// target=0=無制限 → 0.0 契約、捕捉 68 no-op 根治の観測面)。wall-clock
    /// 由来 mode 経路の値のため det subset 非登録 (Active 内 pin で代替)。
    pub power_frame_dt: f32,
    /// 【wave 154 EZ-3】frame_gate 呼出毎の実 dt EMA (α=0.1、秒契約、
    /// 捕捉 71 系 dead state 根治)。全値 delta_ms 系列のみ由来 = 完全
    /// 決定的 → det subset 登録が正当 (GC-2 同型)。
    pub power_smoothed_dt: f32,
    /// 【wave 154 EZ-1】PowerMode の u8 符号 (Active=0/Unfocused=1/
    /// Minimized=2/Idle=3)。mode() getter 冗長削除の代替観測面。
    /// wall-clock 由来のため det subset 非登録 (grep 587 注記と同根拠)。
    pub power_mode: u8,
    /// 【wave 155 FA-3】HUD batch の quad 総数 (§7 消化 18: 旧 drop 破棄
    /// 根治)。wiring は layer 4 相異キーの統計バー 4 本固定 → 常に 4。
    pub hud_quads: u32,
    /// 【wave 155 FA-3】同上: index_count>0 の描画 range 数 ( ImmediatelyFast
    /// 本質の merge 後 draw call 数、4 相異キーで常に 4)。
    pub hud_draw_ranges: u32,
    /// 【wave 155 FA-3】同上: quads − ranges の draw call 削減量 (同一キー
    /// の複数 quad が 1 draw に merge された節約、4 相異キーで常に 0)。
    pub hud_draw_calls_saved: u32,
    /// 【wave 156 FB-3】pool_slab の占有 slot 数 (key ライフサイクル駆動:
    /// 初見 chunk alloc・退去 chunk free、§7 消化 19 で旧無限 append を
    /// 根治)。det subset 登録 (chunk_keys 系列のみ由来=完全決定)。
    pub slab_occupied: u32,
    /// 同上: 当該 tick の初見 chunk による alloc 実行数。
    pub slab_new_allocs: u32,
    /// 同上: 当該 tick の退去 chunk による free 実行数。
    pub slab_freed: u32,
    /// 【wave 171 FQ 捕捉 109】DashMap registry の状態別計測 (Building)。
    /// 現 pipeline は同期的で初見 tick 内に即 Done 遷移するため常 0 pin =
    /// 非同期ビルド未導入の truth pin (導入時に非ゼロへ動く実計測面)。
    pub registry_building: u32,
    /// 同上 (Done): 現存 chunk 総数で slab_occupied と常時一致 (truth pin)。
    pub registry_done: u32,
    /// 同上 (Pending): 非同期投入キュー未導入で到達不能 = 常 0 pin。
    pub registry_pending: u32,
    /// 同上: 追跡中 chunk 総数 (map.len)。
    pub registry_total: u32,
    /// 同上: set_state+remove の累計操作回数 (単調増加、初見 +2/件・退去 +1/件)。
    pub registry_version: u64,
    /// 【wave 174 FT 捕捉 114】InstancedCollector 実計測: 総インスタンス数
    /// (旧 `let _instanced_stats` 破棄 + doc「帯域見積に反映」虚偽を根治)。
    pub instanced_total: u32,
    /// 同上: 相異 mesh_id のグループ数 (mesh_id = mat.min(63))。
    pub instanced_groups: u32,
    /// 同上: Σ 全グループの InstanceData 総バイト (旧 first mesh のみの弱い
    /// 見積から truth へ、32B/instance、rq ft_instanced)。
    pub instanced_bytes: u32,
    /// 【wave 175 FU 捕捉 115】BumpArena::used() (morton codes 実確保後の
    /// 当該 tick アリーナ使用バイト、旧消費者ゼロ → §7 消化 37 実計測配線、
    /// max(k,1)*8 の truth、rq fu_bump)。
    pub bump_used: u64,
    /// 【wave 181 GA 捕捉 126 §7 消化 43】tick_world 実構築 DAG の critical
    /// path (ms)。旧実装は wiring:788 で計算後 `let critical_ms` で破棄
    /// (消費者ゼロ)。5 タスク (ingest/mesh/cull/upload/light、係数係0.05/
    /// 0.4/0.1/0.2/0.1 × delta_ms) の critical = ingest→mesh→upload =
    /// 0.65*delta_ms (/tmp/ga_probe.rs 機械導出)。
    pub dag_critical_ms: f32,
    /// 同上: ObjectPool<BatchOutput> の HUD scratch acquire 直後
    /// available (cap 2・1 outstanding で常に 1、capacity 保持
    /// リサイクルの稼働証跡)。det subset 登録 (定数構造値)。
    pub object_pool_available: u32,
    /// 配線サブシステム仕様数 (固定 60)。【誠実注記 wave 83 CG-3】本値は
    /// 実数え上げではなく固定の仕様値 — tick_world 内で起動される系の
    /// 実計数ではなく、決定性ピンのために定数で供給する。
    pub subsystems_active: u32,
    /// FSR3 FG 用バッファ: color 2 面分 [bytes] (1920×1080 契約、定数)。
    /// 【wave 159 FE 捕捉 83 §7 消化 21】旧実装は fsr3_buffers タプルを
    /// `let _ =` で破棄していた中間構造 — 実報告フィールドへ配線。
    pub fsr3_color_bytes: u64,
    /// 同上: depth 2 面分 [bytes]。
    pub fsr3_depth_bytes: u64,
    /// 同上: motion [bytes]。
    pub fsr3_motion_bytes: u64,
    /// FSR3 FG アルファ中間フレームの R チャンネル平均絶対差
    /// (0.0〜255.0、bootstrap (prev 不在) は out:=curr serve で厳密 0.0)。
    /// 【wave 159 FE 捕捉 83 §7 消化 21】旧実装は `_fsr3_mean_delta` 捨ての
    /// 検証メトリクスだった — 「serve した中間フレームが現フレームとどれ
    /// ほど違うか」の品質観測として実配線。det subset 登録 (scene 固定で
    /// 完全決定、dome/inputs 由来のみ)。
    pub fsr3_mean_delta: f32,
}

/// 全サブシステムを保持・駆動する配線オーケストレーター。
pub struct FullGraphWiring {
    tick: u64,
    // 注: 旧 `game_dir: PathBuf` フィールドは cache_dir 導出後に一度も読まれない
    // デッド状態だったため削除 (2026-07-21 監査)。`new(game_dir)` 引数は維持。
    init_time: std::time::Instant,

    // ---- block / section 基礎 ----
    block_lut: crate::branchless_block::BlockLut,
    intern_pool: crate::intern_pool::InternPool<u32>,
    intern_strings: crate::string_intern::CompactSymbolTable,

    // ---- 2025 cutting-edge suite (全て実データ駆動) ----
    svdag: Option<crate::svdag::SparseVoxelDag>,
    t_svdag: crate::transform_svdag::TransformAwareSvdag,
    aokana: crate::aokana::AokanaFramework,
    azdo: crate::azdo::AzdoOrchestrator,
    gigabuffer: crate::gigabuffer::GigaBufferSuballocator,
    gb_handles: HashMap<(i32, i32), crate::gpu_arena::ArenaHandle>,
    /// gigabuffer 圧迫時の FIFO 被害者選択用の挿入順キュー
    /// (wave 85 CI-1。「最古」の真の定義 = 挿入順。stale エントリは
    /// 置換時に重複登録しない設計で 1 キー 1 順位を保証)。
    gb_order: VecDeque<(i32, i32)>,
    vram_cache: crate::lockfree_vram_cache::LockFreeVramMeshCache,
    paging: Option<crate::out_of_core_paging::OutOfCoreMmapPaging>,
    page_handles: HashMap<(i32, i32), crate::out_of_core_paging::PageHandle>,
    leo: crate::location_encoded_occupancy::LocationEncodedOccupancy,
    leo_nodes: VecDeque<usize>,
    /// 【wave 137 EK-1】ring と一致を保証する tag の 8 スロット集計
    /// (push/pop の decode で対称加減、Σ==ring len の不変式)。
    leo_tag_dist: [u32; 8],
    frb: crate::fragment_ray_box::FragmentRayBoxIntersect,
    gigavoxels: crate::gigavoxels::GigaVoxelsBrickStreaming,
    tdl: crate::tiled_deferred::TiledDeferredLighting,
    clp: crate::compute_light_prop::ComputeLightPropagation,

    // ---- D3D12 相当の CPU モデル群 (実バイト/実コマンドを生成) ----
    desc_ring: crate::descriptor_heap_ring::DualHeapRing,
    /// 【wave 168 FN】rs_graphics() の root cost (定数 19、D3D12 64 DWORD
    /// 制限内の実行時不変式)。§7 消化 30 で tick 破棄を report 配線へ根治。
    root_cost_dwords: u32,
    pso_lib: crate::pso_library_cache::PsoLibrary,
    bundle_cache: crate::bundle_reuse::BundleCache,
    barriers: crate::enhanced_barriers::BarrierBatch,
    virtual_atlas: crate::texture_atlas_virtual::VirtualAtlas,
    sparse_table: crate::sparse_texture::SparsePageTable,
    tex_streamer: crate::mip_streaming::TextureStreamer,
    temporal_diff: crate::temporal_mesh_diff::TemporalDiff,
    vis_graph: crate::visibility_graph::VisibilityGraph,
    indirect_scratch: Vec<crate::mesh_compactor::IndirectDrawCmd>,

    // ---- memory / scheduling ----
    bump: crate::bump_arena::BumpArena,
    slab: crate::pool_slab::Slab<u32>,
    /// 【wave 156 FB §7 消化 19】chunk key → slab slot の真ライフサイクル
    /// 写像 (初見 key のみ alloc・退去 key を free)。旧は毎 tick 全 chunk
    /// を無限 append (free 消費者ゼロ) + `slot == usize::MAX` vacuous
    /// 満杯分岐 (捕捉 74)。
    slab_key_to_slot: std::collections::BTreeMap<(i32, i32), usize>,
    /// 【wave 156 FB §7 消化 19】HUD scratch の ObjectPool リサイクル
    /// (BatchOutput の Vec 容量を跨 tick で保持 = pool 本来の確保回避)。
    scratch_pool: crate::pool_slab::ObjectPool<crate::hud_batch::BatchOutput>,
    registry: crate::dashmap_registry::ChunkRegistry,
    time_slice: crate::stutter_guard::TimeSlice<u32>,
    frame_arena: crate::stutter_guard::FrameArena,
    job_sys: crate::job_system::JobSystem,
    gpu_arena: crate::gpu_arena::GpuArena,
    gpu_alloc_queue: VecDeque<(crate::gpu_arena::ArenaHandle, u64)>,
    hazard: crate::gpu_arena::HazardQueue,
    frame_ring: crate::gpu_arena::SharedRingBuffer<u64, 1024>,
    region_codec: crate::region_zstd::RegionCodec,
    bobby: Option<crate::bobby_cache::BobbyCache>,
    material_batcher: crate::material_batch::MaterialBatcher,
    atlas_packer: crate::texture_atlas::TextureAtlasPacker,
    entity_culler: crate::entity_culling::EntityCuller,

    // ---- quality / post reference chain ----
    governor: crate::quality_governor::QualityGovernor,
    power: crate::power_policy::PowerPolicy,
    /// 【wave 154 EZ-1】frame_gate 貯金法 accumulator (wiring 保持契約、
    /// 捕捉 70 根治で tick_world 実駆動へ移管)。
    power_gate_acc: f32,
    gui_clock: crate::gui_composite::GuiCompositeClock,
    // 【wave 149 GC-3/GC-4】GUI 時計への実駆動イベント検出用 prev 値。
    // camera_dir 変化 = 視点操作入力 (on_input_event 実駆動)、
    // screen_w/h 変化 = GUI 面破棄事象 (invalidate 実駆動)。
    prev_gui_camera_dir: Option<[f32; 3]>,
    prev_gui_screen: Option<(u32, u32)>,
    particles: crate::particle_control::ParticleController,
    be_policy: crate::static_be::BeStaticPolicy,
    be_entries: HashMap<u64, crate::static_be::BePromotionEntry>,
    hud: crate::hud_batch::HudBatch,
    decals: Vec<crate::decals::Decal>,
    vrs_inst: crate::vrs::Vrs,
    wboit: crate::wboit::Wboit,
    gtao_inst: crate::gtao::Gtao,
    shadow_lod_inst: crate::shadow_lod::ShadowLod,
    fxaa_inst: crate::fxaa::Fxaa,
    smaa_inst: crate::smaa::Smaa,
    fsr1_inst: crate::fsr1::Fsr1,
    fsr2_inst: crate::fsr2::Fsr2,
    aces_inst: crate::aces_tonemap::AcesTonemap,
    vco: crate::vertex_cache_opt::VertexCacheOptimizer,
    pacer: crate::frame_pacing::FramePacer,
    /// Pacing 帳簿 (wave 161 FG 捕捉 91): 決定論の入力由来時計 (累積
    /// delta_ms)。壁時計非依存で tick 列に対し再生可能な提示境界列を得る。
    pacing_clock_ms: f64,
    /// 直近に確定した提示境界 (strictly-after-last 語彙の last)。
    last_present_ms: f64,
    fsr3_interp: crate::fsr3_fg::FrameInterpolator,
    fsr3_buffers: (u64, u64, u64),
    fsr3_prev: Option<crate::fsr3_fg::FrameInput>,
    fsr3_out: Vec<u32>,
    ddgi: crate::ddgi::DdgiVolume,
    prev_exposure: f32,
    prev_frame_color: [f32; 3],
    /// 旧 vanilla render フック実測値 (デルタ計算用)。
    prev_getquads_hits: u64,
    prev_chunklayer_hits: u64,
}

impl FullGraphWiring {
    pub fn new(game_dir: &Path) -> Self {
        let cache_dir = game_dir.join(".rsift_cache");
        let _ = std::fs::create_dir_all(&cache_dir);
        let alloc_cfg = crate::mimalloc_config::AllocConfig::for_tier(
            rsift_api::AdaptivePerfEngine::hardware().tier,
        );
        info!(
            "[FullGraphWiring] 実実行配線 v5: 全サブシステム実データ駆動 / allocator={} {} rustflags={}",
            crate::mimalloc_config::recommended_allocator(),
            alloc_cfg.env_string(),
            crate::pgo_bolt::PgoConfig::release().rustflags()
        );

        // TBDR アタッチメント推奨 (初期化時実評価)
        let tbdr_shadow = crate::tbdr_hints::TbdrPass {
            writes: true,
            reads_outside_pass: false,
        };
        let tbdr_scene = crate::tbdr_hints::TbdrPass {
            writes: true,
            reads_outside_pass: true,
        };
        info!(
            "[FullGraphWiring] TBDR: shadow={:?}(lazy={}) scene={:?}",
            tbdr_shadow.recommended_usage(),
            tbdr_shadow.lazy_allocated(),
            tbdr_scene.recommended_usage()
        );

        let root_sig = crate::root_signature_optimized::OptimizedRootSignature::rs_graphics();
        let root_cost = root_sig.root_cost();
        let paging = crate::out_of_core_paging::OutOfCoreMmapPaging::new(
            cache_dir.join("chunk_pages.bin"),
            4096,
        )
        .map_err(|e| tracing::warn!("[FullGraphWiring] mmap paging disabled: {}", e))
        .ok();
        let bobby = crate::bobby_cache::BobbyCache::new(&cache_dir, "local", 256 << 20)
            .map_err(|e| tracing::warn!("[FullGraphWiring] bobby cache disabled: {}", e))
            .ok();

        // GPU ランタイム: 実デバイス生成 + 全 WGSL 実コンパイル (失敗は詳細ログ)。
        if let Some(rt) = crate::gpu_runtime::runtime() {
            info!(
                "[FullGraphWiring] wgpu: {} ({}) shaders compiled={} failed={}",
                rt.adapter_name,
                rt.backend,
                rt.shaders_compiled,
                rt.shaders_failed.len()
            );
        } else {
            info!("[FullGraphWiring] wgpu unavailable — WGSL は CPU 参照実装で代替実実行");
        }
        Self {
            tick: 0,
            init_time: std::time::Instant::now(),
            block_lut: crate::branchless_block::BlockLut::new(),
            intern_pool: crate::intern_pool::InternPool::new(),
            intern_strings: crate::string_intern::CompactSymbolTable::new(),
            svdag: None,
            t_svdag: crate::transform_svdag::TransformAwareSvdag::new(),
            aokana: crate::aokana::AokanaFramework::new(1920, 1080),
            // 第2引数は MiB (`PersistentVboPool` 仕様)。以前の `64 << 20` は
            // 67108864 MiB (=64 TiB) プール要求で、本経路が実行されると
            // 即座に異常アロケーションだった (作者意図は 64 MiB)。
            azdo: crate::azdo::AzdoOrchestrator::new(8192, 64),
            gigabuffer: crate::gigabuffer::GigaBufferSuballocator::new(512),
            gb_handles: HashMap::new(),
            gb_order: VecDeque::new(),
            vram_cache: crate::lockfree_vram_cache::LockFreeVramMeshCache::new(1 << 16),
            paging,
            page_handles: HashMap::new(),
            leo: crate::location_encoded_occupancy::LocationEncodedOccupancy::new(),
            leo_nodes: VecDeque::new(),
            leo_tag_dist: [0; 8],
            frb: crate::fragment_ray_box::FragmentRayBoxIntersect::new(1 << 20),
            gigavoxels: crate::gigavoxels::GigaVoxelsBrickStreaming::new(2048),
            tdl: crate::tiled_deferred::TiledDeferredLighting::new(1920, 1080),
            clp: crate::compute_light_prop::ComputeLightPropagation::new(4096),
            desc_ring: crate::descriptor_heap_ring::DualHeapRing::new(4096, 256, 64, 64),
            root_cost_dwords: root_cost,
            pso_lib: crate::pso_library_cache::PsoLibrary::new(&cache_dir),
            bundle_cache: crate::bundle_reuse::BundleCache::new(),
            barriers: crate::enhanced_barriers::BarrierBatch::new(),
            virtual_atlas: crate::texture_atlas_virtual::VirtualAtlas::new(4096, 4096, 128, 2048),
            sparse_table: crate::sparse_texture::SparsePageTable::new(1024),
            tex_streamer: crate::mip_streaming::TextureStreamer::new(512 << 20),
            temporal_diff: crate::temporal_mesh_diff::TemporalDiff::new(),
            vis_graph: crate::visibility_graph::VisibilityGraph::new(),
            indirect_scratch: Vec::new(),
            bump: crate::bump_arena::BumpArena::new(4 << 20),
            slab: crate::pool_slab::Slab::with_capacity(1024),
            slab_key_to_slot: std::collections::BTreeMap::new(),
            scratch_pool: crate::pool_slab::ObjectPool::new(2),
            registry: crate::dashmap_registry::ChunkRegistry::new(),
            time_slice: crate::stutter_guard::TimeSlice::new(800),
            frame_arena: crate::stutter_guard::FrameArena::new(1 << 20),
            job_sys: crate::job_system::JobSystem::new(2),
            gpu_arena: crate::gpu_arena::GpuArena::new(512 << 20, 256),
            gpu_alloc_queue: VecDeque::new(),
            hazard: crate::gpu_arena::HazardQueue::new(),
            frame_ring: crate::gpu_arena::SharedRingBuffer::new(),
            region_codec: crate::region_zstd::RegionCodec::new(
                crate::region_zstd::CodecChoice::auto(num_cpus_or(4), false),
            ),
            bobby,
            material_batcher: crate::material_batch::MaterialBatcher::new(),
            atlas_packer: crate::texture_atlas::TextureAtlasPacker::new(4096, 4096),
            entity_culler: crate::entity_culling::EntityCuller::new(16, 64.0),
            governor: crate::quality_governor::QualityGovernor::new(
                crate::quality_governor::GovernorConfig::default(),
            ),
            power: crate::power_policy::PowerPolicy::new(
                crate::power_policy::PowerLimits::default(),
            ),
            power_gate_acc: 0.0,
            gui_clock: crate::gui_composite::GuiCompositeClock::new(
                crate::gui_composite::GuiRates::default(),
            ),
            // GC-3/GC-4: camera_dir/screen 変化の実検出用 prev (初回 None =
            // 非発火、rq gc_gui の t1 系列前提と一致)。
            prev_gui_camera_dir: None,
            prev_gui_screen: None,
            particles: crate::particle_control::ParticleController::new(
                crate::particle_control::ParticleBudget::default(),
            ),
            be_policy: crate::static_be::BeStaticPolicy::default(),
            be_entries: HashMap::new(),
            hud: crate::hud_batch::HudBatch::new(1024),
            decals: Vec::new(),
            vrs_inst: crate::vrs::Vrs::new(),
            wboit: crate::wboit::Wboit::new(),
            gtao_inst: crate::gtao::Gtao::new(),
            shadow_lod_inst: crate::shadow_lod::ShadowLod::new(),
            fxaa_inst: crate::fxaa::Fxaa::new(),
            smaa_inst: crate::smaa::Smaa::new(),
            fsr1_inst: crate::fsr1::Fsr1::default(),
            fsr2_inst: crate::fsr2::Fsr2::new(640, 360, 1280, 720),
            aces_inst: crate::aces_tonemap::AcesTonemap::new(),
            vco: crate::vertex_cache_opt::VertexCacheOptimizer::new(16),
            pacer: crate::frame_pacing::FramePacer::new(60.0),
            pacing_clock_ms: 0.0,
            last_present_ms: 0.0,
            fsr3_interp: crate::fsr3_fg::FrameInterpolator::default(),
            fsr3_buffers: crate::fsr3_fg::fsr3_required_buffers(1920, 1080),
            fsr3_prev: None,
            fsr3_out: Vec::new(),
            ddgi: crate::ddgi::DdgiVolume::new(
                crate::ddgi::Vec3::new(0.0, 0.0, 0.0),
                crate::ddgi::Vec3::new(16.0, 8.0, 16.0),
                (16, 4, 16),
            ),
            prev_exposure: 1.0,
            prev_frame_color: [0.0; 3],
            prev_getquads_hits: 0,
            prev_chunklayer_hits: 0,
        }
    }

    /// Governor 提案の chunk build 予算 (render_pipeline の max_builds 計算が参照)。
    pub fn suggested_build_budget(&self) -> u32 {
        let score = self.governor.quality_score(); // 0..100 程度
        if score >= 80 {
            20
        } else if score >= 55 {
            10
        } else if score >= 30 {
            6
        } else {
            4
        }
    }

    /// gigabuffer へのハンドル登録 (FIFO 順位を維持し、旧ハンドルは即解放)。
    fn gb_store(&mut self, k: (i32, i32), handle: crate::gpu_arena::ArenaHandle) {
        if !self.gb_handles.contains_key(&k) {
            self.gb_order.push_back(k);
        }
        if let Some(old) = self.gb_handles.insert(k, handle) {
            self.gigabuffer.free_mesh_slice(old);
        }
    }

    /// gigabuffer の実確保。【wave 85 CI-1】容量超過 (None) 時は
    /// **FIFO (挿入順最古) の被害者を 1 つだけ実解放して単一 retry**。
    /// 旧実装は `HashMap::keys().next()` (ハッシュ順の任意要素) を
    /// 「最古エントリ」と偽り、かつ「実解放して再試行」のコメントに
    /// **再試行コードが存在しなかった** (当該確保が静寂に欠落)。
    /// retry も失敗した場合は潔く None (過剰退避で既存メッシュを
    /// 壊滅させない bounded 挙動、自然退役に委譲)。
    fn gb_alloc_or_evict(
        &mut self,
        k: (i32, i32),
        bytes: u64,
    ) -> Option<crate::gpu_arena::ArenaHandle> {
        if let Some(h) = self.gigabuffer.allocate_mesh_slice(bytes) {
            self.gb_store(k, h);
            return Some(h);
        }
        while let Some(victim) = self.gb_order.pop_front() {
            if let Some(h) = self.gb_handles.remove(&victim) {
                self.gigabuffer.free_mesh_slice(h);
                break;
            }
            // 置換/退避で stale 化した順序エントリは読み飛ばす。
        }
        let h = self.gigabuffer.allocate_mesh_slice(bytes)?;
        self.gb_store(k, h);
        Some(h)
    }

    // 【wave 154 EZ-4】旧 `tick_frame` legacy wrapper は削除 (消費者ゼロ
    // census 全域確定 + `let _` 3 連・ms 誤供給=捕捉 62 同型の恒害構造、
    // GC-4 判例の不可能証明削除。pacer/governor 経路は tick_world で実消費
    // 持続、frame_gate は EZ-1 で tick_world に正規配線)。
    /// メイン配線: 1 フレーム分の全サブシステム実実行。
    pub fn tick_world(&mut self, inputs: &FrameWiringInputs<'_>) -> FrameWiringReport {
        self.tick += 1;
        let mut report = FrameWiringReport {
            post_exposure: self.prev_exposure,
            ..Default::default()
        };
        let now_secs = self.init_time.elapsed().as_secs_f64();
        let n_chunks = inputs.chunk_keys.len();

        // ============================================================
        // 1. 計測・スケジューリング (実 frame 計測値で動作)
        // ============================================================
        self.pacer.record_frame(inputs.delta_ms as f64);
        // 【wave 161 FG 捕捉 91 §7 消化 23】Pacing 帳簿の真駆動: 旧来は
        // EMA 観測のみで vsync snap (next_present_time) / 平滑値
        // (smoothed_frame_ms) / 目標 Hz に消費者が存在しなかった。入力由来
        // の決定論時計 (累積 delta_ms) で提示境界列を生成し report 実
        // フィールドへ配線 (低スペックの提示浪費・スパイク耐性の監視)。
        self.pacing_clock_ms += inputs.delta_ms as f64;
        let next_present = self
            .pacer
            .next_present_time(self.last_present_ms, self.pacing_clock_ms);
        self.last_present_ms = next_present;
        report.frame_next_present_ms = next_present;
        report.frame_smoothed_ms = self.pacer.smoothed_frame_ms();
        report.frame_target_hz = self.pacer.target_refresh_hz();
        // bytecode_transpiler (HEAD 挿入済 vanilla メソッド) の実測到達デルタ。
        // Governor の実入力: vanilla 描画ループの過負荷時にダウンシフト圧を与える。
        let (gq, cl) = crate::render_pipeline::vanilla_render_hook_hits();
        let gq_delta = gq.saturating_sub(self.prev_getquads_hits);
        let cl_delta = cl.saturating_sub(self.prev_chunklayer_hits);
        self.prev_getquads_hits = gq;
        self.prev_chunklayer_hits = cl;
        report.vanilla_hook_hits_delta = (gq_delta + cl_delta).min(u32::MAX as u64) as u32;
        if gq_delta > 4096 && inputs.delta_ms > 0.0 {
            // vanilla バンド幅飽和 → 実績からガバナへ追加観測 (実効果パス)。
            let _ = self
                .governor
                .observe((inputs.frame_us_measured as u64 * 11 / 10) as u32);
        }
        let gov_action = self.governor.observe(inputs.frame_us_measured.max(1));
        if let crate::quality_governor::GovernorAction::Downshift(knob, level) = gov_action {
            debug!("[Wiring] governor downshift {:?}→{}", knob, level);
        }
        report.next_build_budget = Some(self.suggested_build_budget());
        let power_decision = self.power.mode_tick(now_secs, true, false);
        // EZ-1 (wave 154) 捕捉 70 [中] 根治: 旧 `report.power_skip_extra =
        // !power_decision.render` は mode_tick が全経路 render:true 固定返却
        // で **恒 false の定数退化** (全沈黙観測面) だった。skip の真判定は
        // frame_gate (貯金法) — 秒化 (delta_ms/1000.0、捕捉 62 同型 ms
        // 誤供給の構造的撲滅) で tick_world に実駆動して一本化。
        let gate_render = self.power.frame_gate(
            &mut self.power_gate_acc,
            inputs.delta_ms / 1000.0,
            power_decision.target_fps,
        );
        report.power_skip_extra = !gate_render;
        report.power_frame_dt = power_decision.frame_dt;
        report.power_smoothed_dt = self.power.smoothed_dt();
        report.power_mode = power_decision.mode as u8;
        // GC-1 (捕捉 62 [高]): `real_dt` は秒契約 (need=1/fps [s]・module
        // test 一次情報) なのに旧来は ms の `inputs.delta_ms` (16.0) を誤
        // 供給 → gui_accum が 480 倍速蓄積で due 常時真 = 30fps デュアル
        // レート機構の構造的全沈黙 (旧 `_gui_decision` 破棄で観測経路が
        // 無く潜在化。§7 配線と同時根治しないと省電力機構が有効化直後に
        // 無効化状態で実害化する二重構造)。/1000.0 で秒化に根治。
        // GC-3: camera_dir 変化 (視点操作=入力駆動) を prev 照合で実検出し
        // on_input_event を実駆動 (GUI 入力遅延ゼロ設計の真の消費者)。
        if let Some(prev_dir) = self.prev_gui_camera_dir {
            if prev_dir != inputs.camera_dir {
                self.gui_clock.on_input_event();
                // EZ-1 (wave 154): power_policy.on_input も同一検出点に真接続
                // (消費者ゼロだった §7 消化 — Idle 解除が実入力駆動になる)。
                self.power.on_input(now_secs);
            }
        }
        self.prev_gui_camera_dir = Some(inputs.camera_dir);
        // GC-4: screen_w/h 変化 (= GUI オフスクリーン面の実破棄事象) で
        // invalidate() を実駆動。
        let cur_screen = (inputs.screen_w, inputs.screen_h);
        if let Some(prev_screen) = self.prev_gui_screen {
            if prev_screen != cur_screen {
                self.gui_clock.invalidate();
            }
        }
        self.prev_gui_screen = Some(cur_screen);
        let gui_decision = self.gui_clock.should_render_gui(inputs.delta_ms / 1000.0);
        // GC-2 (§7 消化 14): 旧 `_gui_decision` 全消費者ゼロ破棄から実
        // フィールド化。全値 inputs 系列のみ由来 (module 側で wall-clock
        // 依存を撤去済、GC-5 注記 2) → det subset 登録が正当。
        report.gui_rendered_surface = gui_decision.render_gui_surface;
        report.gui_target_fps = gui_decision.fps_now;
        report.gui_invalidated = gui_decision.invalidated;
        report.gui_reuse_cached_scene = gui_decision.reuse_cached_scene;
        report.gui_surface_valid = self.gui_clock.surface_valid();

        // DAG: 実フレームのパス依存グラフを構築・評価。
        let mut dag = crate::dag_scheduler::DagScheduler::new();
        let t_ingest = dag.add_task("chunk_ingest", &[], 0.05 * inputs.delta_ms);
        let t_mesh = dag.add_task("mesh_build", &[t_ingest], 0.4 * inputs.delta_ms);
        let t_cull = dag.add_task("cull", &[t_ingest], 0.1 * inputs.delta_ms);
        let _t_upload = dag.add_task("gpu_upload", &[t_mesh, t_cull], 0.2 * inputs.delta_ms);
        let _t_light = dag.add_task("light", &[t_ingest], 0.1 * inputs.delta_ms);
        let exec_order = dag.topological_order();
        debug_assert_eq!(exec_order.len(), 5);
        let critical_ms = dag.critical_path_ms();
        // 【wave 181 GA 捕捉 126 §7 消化 43】旧実装は critical_ms を破棄
        // (消費者ゼロ) → report 実計測配線。
        report.dag_critical_ms = critical_ms;

        // Registry: chunk key ライフサイクル駆動の真状態機械 (wave 171 FQ
        // 捕捉 109)。slab truth と連動: 初見 → Building → (alloc 直後) Done、
        // 退去 → remove。旧実装は全 chunk を毎 tick Building に一色上書きする
        // のみで get_state/pending_chunks/version の読み取り消費者ゼロ、
        // doc「再投入抑止に使える」は虚偽だった。
        // RenderSection slab: chunk key ライフサイクル駆動の真実装 (wave 156
        // FB 捕捉 74 根治 + §7 消化 19)。旧実装は毎 tick 全 chunk を無限
        // append するのみ (free 消費者ゼロ) で、`slot == usize::MAX` の
        // 満杯分岐は 64-bit 到達不能 (alloc は無制限成長で MAX sentinel を
        // 返しえない) の vacuous check、「満杯判定自体が目的」のコメントは
        // 虚偽構文だった。真契約: 初見 key のみ alloc (世代スラブへ mat
        // 格納)、退去 key は free で mat 回収、map は module 側 len と
        // 常時一致 (integrity debug_assert)。
        {
            let cur: std::collections::BTreeSet<(i32, i32)> =
                inputs.chunk_keys.iter().copied().collect();
            let evicted: Vec<(i32, i32)> = self
                .slab_key_to_slot
                .keys()
                .copied()
                .filter(|k| !cur.contains(k))
                .collect();
            let mut slab_freed = 0u32;
            for k in evicted {
                let slot = self
                    .slab_key_to_slot
                    .remove(&k)
                    .expect("evict key は map に在る");
                self.slab
                    .free(slot)
                    .expect("evict slot は map 一意で必ず在る (1 key = 1 slot 不変式)");
                self.registry.remove(k); // FQ 捕捉 109: 退去で registry からも除去
                slab_freed += 1;
            }
            let mut slab_new_allocs = 0u32;
            for (i, k) in inputs.chunk_keys.iter().enumerate() {
                if !self.slab_key_to_slot.contains_key(k) {
                    let mat = inputs.chunk_materials.get(i).copied().unwrap_or(0);
                    let slot = self.slab.alloc(mat);
                    debug_assert!(slot != usize::MAX, "alloc は無制限 (捕捉 74)");
                    self.slab_key_to_slot.insert(*k, slot);
                    // FQ 捕捉 109: 初見 → Building → (slab alloc 直後) Done
                    // (同期 pipeline の truth: tick 内即時遷移)。
                    self.registry
                        .set_state(*k, crate::dashmap_registry::ChunkBuildState::Building);
                    self.registry
                        .set_state(*k, crate::dashmap_registry::ChunkBuildState::Done);
                    slab_new_allocs += 1;
                }
            }
            debug_assert_eq!(
                self.slab_key_to_slot.len(),
                self.slab.len(),
                "map と slab 占有は常時一致 (integrity)"
            );
            debug_assert_eq!(
                self.slab_key_to_slot.is_empty(),
                self.slab.is_empty(),
                "is_empty 一致 (integrity)"
            );
            report.slab_freed = slab_freed;
            report.slab_new_allocs = slab_new_allocs;
            report.slab_occupied = self.slab.len() as u32;
            // FQ 捕捉 109 §7 消化 33: registry 読み取り面の実計測
            // (state_counts/pending_chunks/version/len の実消費者配線)。
            let counts = self.registry.state_counts();
            report.registry_pending = self.registry.pending_chunks().len() as u32;
            report.registry_building = counts[1];
            report.registry_done = counts[2];
            report.registry_total = self.registry.len() as u32;
            report.registry_version = self.registry.version();
            debug_assert_eq!(
                report.registry_done, report.slab_occupied,
                "registry Done と slab 占有は slab truth 連動で常時一致"
            );
            debug_assert!(
                inputs.chunk_keys.iter().all(|k| self.registry.get_state(*k)
                    == Some(crate::dashmap_registry::ChunkBuildState::Done)),
                "全現存 chunk key は registry で Done (get_state 実消費 integrity)"
            );
        }

        // BumpArena: morton コード列を実確保 (ヒープ不使用) して局所性 sort を評価。
        self.bump.reset();
        let codes: Vec<u64> =
            crate::rayon_job::parallel_map_chunks(inputs.chunk_keys.clone(), |(cx, cz)| {
                // DH-1: morton_encode_3d は 10 bit/成分で静寂切捨て (wave 108 契約公表)。
                // `& 0xF_FFFF` (21 bit) マスクの上位 11 bit は従来も到達していなかった
                // (split_by_3 の magic チェーン第 1 段で消失 = 切捨ては中央強制。)
                // — 挙動完全一致のまま意図を明示に置換 (局所性は「低 10 bit 域の
                // Z-curve」として解釈する契約)。くわえて adversarial (c) で本行を
                // 旧マスクに逆戻ししても digest 不変 (= 中立性) を実測確認済。
                crate::morton_order::morton_encode_3d((cx as u32) & 1023, 0, (cz as u32) & 1023)
            });
        if let Some(ptr) = self.bump.alloc_slice::<u64>(codes.len().max(1)) {
            unsafe {
                std::ptr::copy_nonoverlapping(codes.as_ptr(), ptr, codes.len());
            }
        }
        // FU 捕捉 115 §7 消化 37: bump used() 消費者ゼロ → report 実計測
        // (morton 実確保の当該 tick 使用バイト truth telemetry)。
        report.bump_used = self.bump.used() as u64;
        let mut order_morton: Vec<usize> = (0..n_chunks).collect();
        order_morton.sort_by_key(|&i| codes.get(i).copied().unwrap_or(0));
        let _ = order_morton;

        // JobSystem: 実クアッド数に比例したチャンク化でワーカーへ負荷を
        // 実分散する実演 (wave 83 CG-5: クロージャは no-op で、メトリクスの
        // 集計は行わない — 旧コメントの「軽量メトリクスを集計」は虚偽)。
        {
            let quads = inputs.quad_positions.len();
            if quads > 0 {
                self.job_sys.parallel_for(
                    quads,
                    64,
                    crate::job_system::Priority::Normal,
                    |_i, _j| {},
                );
                self.job_sys.wait_idle();
            }
        }

        // FrameArena: 当該フレームのスクラッチを実確保・実利用 (used 計測が実効果確認)。
        self.frame_arena.reset();
        if let Some(scratch) = self
            .frame_arena
            .alloc_bytes(4096.min(self.frame_arena.capacity()))
        {
            unsafe {
                std::ptr::write_bytes(
                    scratch.as_ptr(),
                    self.tick as u8,
                    4096.min(self.frame_arena.capacity()),
                );
                let _sample = std::ptr::read_volatile(scratch.as_ptr());
            }
        }
        let frame_scratch_used = self.frame_arena.used();

        // ============================================================
        // 2. チャンク実データ系: カリング/間接描画/VRAM 台帳 (全て実入力)
        // ============================================================

        // -- 視錐台抽出 (view_proj 実行列から 6 面)。
        let planes6 = extract_frustum_planes(&inputs.view_proj);
        let lbvh_planes: Vec<crate::lbvh::Plane> = planes6
            .iter()
            .map(|p| crate::lbvh::Plane::new(p[0], p[1], p[2], p[3]))
            .collect();
        let simd_planes: [crate::simd_frustum::Plane; 6] =
            std::array::from_fn(|i| crate::simd_frustum::Plane {
                a: planes6[i][0],
                b: planes6[i][1],
                c: planes6[i][2],
                d: planes6[i][3],
            });

        // -- LBVH: 実チャンク中心・半径で BVH 構築 → 実フラスタムカリング。
        let mut lbvh = None;
        if !inputs.chunk_aabbs.is_empty() {
            let centers: Vec<crate::lbvh::Vec3> = inputs
                .chunk_aabbs
                .iter()
                .map(|(mn, mx)| {
                    crate::lbvh::Vec3::new(
                        (mn[0] + mx[0]) * 0.5,
                        (mn[1] + mx[1]) * 0.5,
                        (mn[2] + mx[2]) * 0.5,
                    )
                })
                .collect();
            let radii: Vec<crate::lbvh::Vec3> = inputs
                .chunk_aabbs
                .iter()
                .map(|(mn, mx)| {
                    crate::lbvh::Vec3::new(
                        (mx[0] - mn[0]) * 0.5,
                        (mx[1] - mn[1]) * 0.5,
                        (mx[2] - mn[2]) * 0.5,
                    )
                })
                .collect();
            let (mut mn, mut mx) = (
                crate::lbvh::Vec3::new(f32::MAX, f32::MAX, f32::MAX),
                crate::lbvh::Vec3::new(f32::MIN, f32::MIN, f32::MIN),
            );
            for c in &centers {
                mn = crate::lbvh::Vec3::new(mn.x.min(c.x), mn.y.min(c.y), mn.z.min(c.z));
                mx = crate::lbvh::Vec3::new(mx.x.max(c.x), mx.y.max(c.y), mx.z.max(c.z));
            }
            let tree = crate::lbvh::Lbvh::build(&centers, &radii, mn, mx);
            let visible = tree.cull(&lbvh_planes);
            report.lbvh_culled = (inputs.chunk_aabbs.len().saturating_sub(visible.len())) as u32;
            lbvh = Some((tree, visible));
        }

        // -- SIMD frustum: SoA 並列テスト (LBVH の相互検証)。
        let simd_boxes: Vec<crate::simd_frustum::Aabb> = inputs
            .chunk_aabbs
            .iter()
            .map(|(mn, mx)| crate::simd_frustum::Aabb { min: *mn, max: *mx })
            .collect();
        let frustum = crate::simd_frustum::SimdFrustum::new(simd_planes);
        let soa = crate::simd_frustum::SoaAabbs::from_aabbs(&simd_boxes);
        let simd_visible = frustum.cull_soa_portable(&soa);

        // -- mesh_compactor + AZDO: 実 DrawCandidate → 実 indirect コマンド生成。
        let candidates: Vec<crate::mesh_compactor::DrawCandidate> = inputs
            .chunk_aabbs
            .iter()
            .enumerate()
            .map(|(i, (mn, mx))| crate::mesh_compactor::DrawCandidate {
                center: [
                    (mn[0] + mx[0]) * 0.5,
                    (mn[1] + mx[1]) * 0.5,
                    (mn[2] + mx[2]) * 0.5,
                ],
                half_extents: [
                    (mx[0] - mn[0]) * 0.5,
                    (mx[1] - mn[1]) * 0.5,
                    (mx[2] - mn[2]) * 0.5,
                ],
                vertex_count: inputs.draw_index_counts.get(i).copied().unwrap_or(0),
                first_vertex: i as u32,
                visible_prev: simd_visible.get(i).copied().unwrap_or(true),
                pass_key: (inputs.chunk_materials.get(i).copied().unwrap_or(0) & 0xFF) as u16,
            })
            .collect();
        let compact_policy = crate::mesh_compactor::CompactPolicy::default();
        let frustum_planes_mc = crate::mesh_compactor::FrustumPlanes::from_view_proj(
            &view_proj_to_m16(&inputs.view_proj),
        );
        let _alive = crate::mesh_compactor::compact_draws(
            &candidates,
            &frustum_planes_mc,
            inputs.camera_pos,
            &compact_policy,
            &mut self.indirect_scratch,
        );

        let cmds: Vec<crate::execute_indirect::ChunkDrawCommand> = inputs
            .chunk_keys
            .iter()
            .enumerate()
            .map(|(i, _)| crate::execute_indirect::ChunkDrawCommand {
                args: crate::execute_indirect::DrawIndexedIndirectArgs {
                    index_count_per_instance: inputs.draw_index_counts.get(i).copied().unwrap_or(0),
                    instance_count: 1,
                    start_index_location: 0,
                    base_vertex_location: 0,
                    start_instance_location: i as u32,
                },
                chunk_id: i as u32,
                // §7 消化 19 (wave 156): material を世代スラブから真の
                // read-back 供給 (slab を RenderSection の真の material
                // store とする本来アーキテクチャ。key → slot → get の
                // 丸ごと経路で、値は inputs 由来と round-trip bit 同一)。
                material_id: self
                    .slab_key_to_slot
                    .get(&inputs.chunk_keys[i])
                    .and_then(|&slot| self.slab.get(slot))
                    .copied()
                    .unwrap_or(0),
                _pad: [0; 2],
            })
            .collect();
        let vis_mask: Vec<bool> = (0..cmds.len())
            .map(|i| {
                simd_visible.get(i).copied().unwrap_or(true)
                    && lbvh.as_ref().map(|(_, v)| v.contains(&i)).unwrap_or(true)
            })
            .collect();
        report.draw_command_count = self.azdo.execute_azdo_pass(&cmds, &vis_mask);

        // -- Material batch & instancing: 実マテリアルで再バッチ。
        self.material_batcher.clear();
        for (i, mat) in inputs.quad_materials.iter().enumerate() {
            let translucent = (*mat as u16) % 7 == 5; // 素材 id 由来の決定論的 translucent 代表
            self.material_batcher
                .push_quad(*mat as u16, i as u32, translucent);
        }
        let (opaque_draws, translucent_draws) = self.material_batcher.build_draws();
        // EX-1 (wave 152): `let _ = (…)` 破棄を根治 (§7 消化 15) — draw 件数系と
        // bin 済 quad 総数を report に実配線し、完全性を cross 不変式で検算する。
        report.material_opaque_draws = opaque_draws.len() as u32;
        report.material_translucent_draws = translucent_draws.len() as u32;
        report.material_binned_quads = opaque_draws
            .iter()
            .chain(translucent_draws.iter())
            .map(|d| d.quad_count)
            .sum::<u32>();
        debug_assert_eq!(
            report.material_binned_quads as usize,
            inputs.quad_materials.len(),
            "全 quad は batcher で一意に bin される (Σquad_count 完全性)"
        );
        // ER-2 (wave 144): sort_translucent_indices の実消費者追加
        // (§7 消化 12)。translucent 判定は 731 行と同一規則で再収集し、
        // build_draws 件数との cross 不変式でも検算。
        let translucent_centers: Vec<[f32; 3]> = inputs
            .quad_materials
            .iter()
            .enumerate()
            .filter(|(_, mat)| (**mat as u16) % 7 == 5)
            .map(|(i, _)| inputs.quad_positions.get(i).copied().unwrap_or([0.0; 3]))
            .collect();
        let mut translucent_order: Vec<u32> = (0..translucent_centers.len() as u32).collect();
        crate::depth_prepass::sort_translucent_indices(
            &translucent_centers,
            inputs.camera_pos,
            &mut translucent_order,
        );
        report.translucent_total = translucent_centers.len() as u32;
        report.translucent_back_first = translucent_order.first().copied().unwrap_or(0);
        // EX-1 (wave 152) 捕捉 65 [中] 根治: 旧 assert は translucent **quads**
        // 件数 (translucent_total) と draw **ranges** 件数 (translucent_draws
        // .len()) を誤等置し、同一 mat の連続 translucent quad 2 個で debug
        // 誤爆していた (TDD RED 機械記録: left=2, right=1)。正しい不変式は
        // quads 件数 == Σ translucent quad_count (ranges≠quads の区別)。
        debug_assert_eq!(
            report.translucent_total,
            translucent_draws.iter().map(|d| d.quad_count).sum::<u32>(),
            "translucent quads 件数 == batcher translucent bin の Σquad_count (同一規則一致)"
        );

        // ER-1 (wave 144): DepthPrepassPlanner::plan() の実消費者追加
        // (§7 消化 11)。for_low_spec 固定選択 (低スペック制約整合、
        // report フィールド doc 記載)。
        let depth_plan = crate::depth_prepass::DepthPrepassPlanner::for_low_spec().plan();
        report.depth_pass_count = depth_plan.len() as u32;
        report.depth_pass_cost = depth_plan.iter().map(|p| p.estimated_cost).sum();

        // Instancing: フレームごとに実クアッド座標から再集積 (clear API 不在のためローカル生成)。
        let mut instanced = crate::instanced_draw::InstancedCollector::new();
        for (i, pos) in inputs.quad_positions.iter().enumerate() {
            let mat = inputs.quad_materials.get(i).copied().unwrap_or(0);
            instanced.add(mat.min(63), *pos, mat, 3);
        }
        // 【wave 174 FT 捕捉 114 §7 消化 36】旧 `let _instanced_stats` 破棄
        // (doc「帯域見積に反映」は反映先ゼロの虚偽) → report 3 実フィールド
        // 真配線。bytes は旧 first mesh のみの弱い見積から Σ 全グループ総
        // バイトの truth へ根治 (as_bytes/total_instances/groups 実消費)。
        report.instanced_total = instanced.total_instances() as u32;
        report.instanced_groups = instanced.groups().count() as u32;
        report.instanced_bytes = instanced
            .groups()
            .map(|g| instanced.as_bytes(g.mesh_id).map(|b| b.len()).unwrap_or(0) as u32)
            .sum();

        // -- OverdrawSorter: front-to-back 実順序 → upload 順に反映。
        let draw_items: Vec<crate::overdraw_sort::DrawItem> = inputs
            .chunk_aabbs
            .iter()
            .enumerate()
            .map(|(i, (mn, mx))| crate::overdraw_sort::DrawItem {
                center: [
                    (mn[0] + mx[0]) * 0.5,
                    (mn[1] + mx[1]) * 0.5,
                    (mn[2] + mx[2]) * 0.5,
                ],
                radius: ((mx[0] - mn[0]).powi(2)
                    + (mx[1] - mn[1]).powi(2)
                    + (mx[2] - mn[2]).powi(2))
                .sqrt()
                    * 0.5,
                id: i as u32,
            })
            .collect();
        report.overdraw_order = crate::overdraw_sort::OverdrawSorter::sort_front_to_back(
            &draw_items,
            inputs.camera_pos,
        );

        // -- VRAM 台帳: GigaBuffer(再配置) + LockFree cache + GpuArena(リタイア管理)。
        let arena_epoch = self.hazard.advance_epoch();
        for (i, k) in inputs.chunk_keys.iter().enumerate() {
            let (_h, existed) = self.vram_cache.lookup_or_insert(k.0, k.1);
            if existed {
                report.lockfree_cache_hits += 1;
            }
            let bytes = inputs.draw_index_counts.get(i).copied().unwrap_or(0) as u64 * 4 + 4096;
            let _giga = self.gb_alloc_or_evict(*k, bytes);
            if let Some(h) = self.gpu_arena.alloc(bytes) {
                self.gpu_alloc_queue.push_back((h, arena_epoch));
            }
        }
        // 2 エポック以上前の確保を退役→回収 (実世代管理)。
        while let Some((h, ep)) = self.gpu_alloc_queue.pop_front() {
            if arena_epoch.saturating_sub(ep) >= 2 {
                self.hazard.defer_free(h, ep);
            } else {
                self.gpu_alloc_queue.push_front((h, ep));
                break;
            }
        }
        let reclaimed = self
            .hazard
            .reclaim(arena_epoch.saturating_sub(1), &mut self.gpu_arena);
        let _ = reclaimed;
        report.vram_used_bytes = self
            .gigabuffer
            .used_bytes()
            .max(self.gpu_arena.used_bytes());
        let _ = self.frame_ring.try_push(self.tick);
        let _drained = self.frame_ring.try_pop();

        // -- Out-of-core ページング & Bobby cache & RegionCodec (実ディスク I/O、予算制)。
        if !inputs.chunk_keys.is_empty() && self.tick % 4 == 0 {
            let k = inputs.chunk_keys[0];
            if let std::collections::hash_map::Entry::Vacant(e) = self.page_handles.entry(k) {
                // insert は write_chunk_page 成功時のみ (失敗でゴミエントリを残さない)。
                if let Some(p) = self.paging.as_mut() {
                    if let Ok(h) = p.write_chunk_page(k.0, k.1, &self.tick.to_le_bytes()) {
                        e.insert(h);
                    }
                }
            }
            if let Some(b) = self.bobby.as_ref() {
                if self.tick % 16 == 0 {
                    let chunk = crate::bobby_cache::CachedChunk {
                        dim: 0,
                        cx: k.0,
                        cz: k.1,
                        // 【注】BobbyCache の time_ms 欄は eviction 順序付けにだけ
                        // 使われるため、壁時計非依存の単調フレーム tick を擬似時刻と
                        // して格納し決定性を維持する (監査 2026-07-22 K-2。旧コードは
                        // delta_ms x 1000 という単位不整合かつ毎回近似一定の値を
                        // time_ms 欄へ入れており順序情報を持たなかった)。
                        time_ms: self.tick,
                        payload: vec![],
                    };
                    let _ = b.store(&chunk);
                }
            }
            // ペイロードは合成データだが、キー混合は i32 の符号拡張で上位 32bit が
            // 汚染される XOR (`k.0 as u64` は負座標で 0xFFFF_FFFF_xxxx_xxxx) ではなく、
            // 零拡張同士の単射的なビット配置にする (監査 2026-07-22 K-3)。
            let key_mix = (k.0 as u32 as u64) | ((k.1 as u32 as u64) << 32);
            self.region_codec.put_chunk(
                (k.0.rem_euclid(32)) as usize,
                (k.1.rem_euclid(32)) as usize,
                &key_mix.to_le_bytes(),
            );
        }
        let _region_stats = self.region_codec.stats();

        // -- TemporalDiff / LEO / GigaVoxels (実セクションデータ)。
        let mut dirty_sections = 0u32;
        for (i, k) in inputs.chunk_keys.iter().enumerate() {
            self.temporal_diff
                .mark_dirty(crate::temporal_mesh_diff::SectionKey {
                    cx: k.0,
                    cz: k.1,
                    sy: i as i32 % 4,
                });
            dirty_sections += 1;
        }
        let taken = self.temporal_diff.take_dirty();
        // TimeSlice: dirty section のメッシュパッチ生成を時分割で実処理 (budget 超過分は翌フレーム繰越)。
        for key in &taken {
            let packed_key = (((key.cx as u32) & 0x3FF) << 20)
                | (((key.cz as u32) & 0x3FF) << 10)
                | ((key.sy as u32) & 0x3FF);
            self.time_slice.push(packed_key);
        }
        let mut patch_quads_removed = 0usize;
        let processed_this_frame = self.time_slice.run_frame(|packed| {
            // 【誠実注記 wave 85 CI-2】patch 駆動キーは `packed & 0xFFF`
            // = cz 下位 2bit と sy 10bit の混在。セクションの真の per-block
            // 差分ではなく時分割シミュレーションの決定的駆動値
            // (patch_for_block の契約 <4096 は構造的に常時満たされる)。
            let patch =
                crate::temporal_mesh_diff::TemporalDiff::patch_for_block((packed & 0xFFF) as usize);
            patch_quads_removed += patch.removed_quads.len();
            180 // 実処理コスト見積 (μs)
        });
        let _ = (dirty_sections, processed_this_frame, patch_quads_removed);

        // LEO: 3bit ロケーションエンコードに収まるよう tag を 1..=7 に丸めて実登録。
        let occupancy_tag = leo_occupancy_tag(inputs.section_palettes.len());
        let idx_leo = self.leo.allocate_tagged_node(occupancy_tag, self.tick);
        self.leo_nodes.push_back(idx_leo);
        // 【wave 137 EK-1】実消費者配線: ring 維持と同期して tag を 8 スロット
        // 集計 (pop は decode した旧 tag をデクリメント) → report 実フィールド化。
        self.leo_tag_dist[occupancy_tag as usize] += 1;
        if self.leo_nodes.len() > 4096 {
            let old_idx = self
                .leo_nodes
                .pop_front()
                .expect("len > 4096 checked above");
            let old_tag = crate::location_encoded_occupancy::LocationEncodedOccupancy::decode_occupancy_from_location(old_idx)
                as usize;
            debug_assert!(
                self.leo_tag_dist[old_tag] > 0,
                "ring 不変式: pop した tag のカウント正定"
            );
            self.leo_tag_dist[old_tag] -= 1;
        }
        debug_assert_eq!(
            self.leo_tag_dist.iter().sum::<u32>() as usize,
            self.leo_nodes.len(),
            "Σ leo_tag_dist == ring len (ring maintain 不変式)"
        );
        debug_assert_eq!(
            crate::location_encoded_occupancy::LocationEncodedOccupancy::decode_occupancy_from_location(idx_leo),
            occupancy_tag
        );
        report.leo_tag_dist = self.leo_tag_dist;
        // 【wave 137 EK-2】payload (=tick) を get_payload(Option 版) で実読出し
        // する真の消費地 — 割当直後 index は範囲内なので expect 恒真保持。
        report.leo_latest_tick = self
            .leo
            .get_payload(idx_leo)
            .expect("wave 137 EK-2: 割当直後 index は pool 範囲内 (範囲外は None/昨日 0 静寂返却を Option 化して根治)");

        if let Some(palette) = inputs.section_palettes.first() {
            let pallet = *palette;
            self.gigavoxels
                .request_brick(crate::gigavoxels::BrickKey(0, 0, 0, 0));
            let gen = move |key: crate::gigavoxels::BrickKey| {
                let mut brick = [0u16; crate::gigavoxels::BRICK_VOL];
                if key.3 == 0 {
                    for i in 0..crate::gigavoxels::BRICK_VOL {
                        let x = (key.0 * 8 + (i % 8) as i32) as usize;
                        let y = (key.1 * 8 + ((i / 8) % 8) as i32) as usize;
                        let z = (key.2 * 8 + (i / 64) as i32) as usize;
                        if x < 16 && y < 16 && z < 16 {
                            brick[i] = pallet[section_idx(x, y, z)];
                        }
                    }
                }
                brick
            };
            // 【wave 166 FL 捕捉 101 §7 消化 28】旧 `_bricks` 破棄を根治:
            // 生成件数と常駐数を report 実フィールドへ閉じる (brick
            // ストリーミング負荷の決定性計測点)。
            let bricks_processed = self.gigavoxels.process_requests(8, self.tick, gen) as u32;
            report.gigavoxels_processed = bricks_processed;
            report.gigavoxels_resident = self
                .gigavoxels
                .pool
                .lock()
                .map(|p| p.resident_bricks.len() as u32)
                .unwrap_or(0);
        }
        // -- CLP: 実パレットの不透明ビットボード + 実発光種で GPU に実ディスパッチ。
        //    (デバイス非搭載時は安全にスキップ — CPU フォールバックの AO/Light 系で代替。)
        if let Some(palette) = inputs.section_palettes.first() {
            if self.tick % 120 == 0 {
                let mut opacity = [0u32; 128];
                let mut seeds: Vec<(u32, u8)> = Vec::new();
                for z in 0..16usize {
                    for y in 0..16usize {
                        for x in 0..16usize {
                            let id = palette[section_idx(x, y, z)];
                            // WGSL と同一線形規約: idx = x | (y << 4) | (z << 8)
                            let idx = (x + y * 16 + z * 256) as u32;
                            if self.block_lut.is_opaque_branchless(id) {
                                opacity[(idx >> 5) as usize] |= 1u32 << (idx & 31);
                            }
                            let lvl = self.block_lut.light_branchless(id);
                            if lvl > 0 && seeds.len() < 256 {
                                seeds.push((idx, lvl));
                            }
                        }
                    }
                }
                if let Some(light) =
                    crate::gpu_runtime::dispatch_light_propagation(&opacity, &seeds, 16)
                {
                    let lit = light.iter().filter(|v| **v & 15 > 0).count() as f32;
                    report.clp_lit_fraction = lit / self.clp.chunk_volume.max(1) as f32;
                }
            }
        }
        // -- FRB: 実クアッドのビルボード形状組立 (center, half, color) の実演
        //    と本数計測 (wave 83 CG-6: FragmentRayBoxIntersect に CPU 入力 API
        //    はなく、組立タプルは GPU へ送達されず破棄される。旧コメントの
        //    「GPU 入力へ実変換」は虚偽だったため訂正。count 自体は実測)。
        let mut frb_count = 0u32;
        for (i, pos) in inputs
            .quad_positions
            .iter()
            .enumerate()
            .take((self.frb.max_dynamic_voxels as usize).min(128))
        {
            let mat = inputs.quad_materials.get(i).copied().unwrap_or(0);
            let _billboard = (*pos, 0.5f32, mat); // (center, half_size, color)
            frb_count += 1;
            // EB-5 (2026-07-26): 旧 frb_above (camera_pos[1]-32 未満との高さ
            // 比較カウント) は集計後に let _ 破棄されるのみの消費者不在
            // メトリクスだったため削除 (slab_slots 削除と同型、挙動中立)。
        }
        report.frb_billboards = frb_count;

        // -- SVDAG / Transform-Aware / Aokana (実パレットから実構築 — 初回 & 定期更新)。
        if let Some(palette) = inputs.section_palettes.first() {
            let mut volume = [[[false; 16]; 16]; 16];
            for y in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        volume[y][z][x] = self
                            .block_lut
                            .is_opaque_branchless(palette[section_idx(x, y, z)]);
                    }
                }
            }
            if self.svdag.is_none() || self.tick % 600 == 0 {
                let mut dag = crate::svdag::SparseVoxelDag::new();
                let root = dag.build_from_volume(&volume);
                let _ = root;
                // Transform-Aware SVDAG: 実ノード列を transform タグ付きで挿入。
                for node in dag.nodes.iter().take(16) {
                    let _ = self.t_svdag.insert_transform_aware(node.clone());
                }
                // Aokana: 実セクション由来の DAG をシャローリージョンとして登録し
                // 実リージョンカリングを駆動する。リージョン座標は aokana 規約
                // (region_size_blocks=64、すなわち 4x4 チャンク区画) に合わせて、
                // パレット供給チャンクの実座標から導出する。
                // 【注】section_palettes はチャンク内 y 帯を保持しないため ry=0 区画
                // への登録となる (監査 2026-07-22 K-1)。旧コードは供給元に関わらず
                // 無条件 (0,0,0) 登録で、さらに self.svdag.take() で実構築物を aokana
                // へ移し self.svdag には空 DAG を残していた — 本実装では svdag に
                // 実 DAG を保持し aokana には clone を登録する。
                let (rx, rz) = inputs
                    .chunk_keys
                    .first()
                    .map(|&(cx, cz)| (cx.div_euclid(4), cz.div_euclid(4)))
                    .unwrap_or((0, 0));
                self.aokana.insert_shallow_region(rx, 0, rz, dag.clone(), 0);
                self.svdag = Some(dag);
            }
        }
        let visible_regions = self
            .aokana
            .evaluate_visible_regions(inputs.camera_pos, &planes6);
        report.aokana_visible_regions = visible_regions.len() as u32;

        // -- Voxel Cone Tracing (実 SVO があるフレームで実走査)。
        // 【誠実注記 wave 84 CH-5】dir.y = |camera_dir.y| の絶対値化は
        // 天頂方向 sky-visibility プローブの近似で、GI 遮蔽の真の方向性
        // ではない。出力は現行読み捨て (メトリクス実演)。
        let mut vct_occlusion = 1.0f32;
        if let Some(svo) = inputs.svo {
            let cone = crate::voxel_cone_tracing::ConeRay {
                origin: inputs.camera_pos,
                dir: [
                    inputs.camera_dir[0],
                    inputs.camera_dir[1].abs(),
                    inputs.camera_dir[2],
                ],
                aperture: 0.577,
                max_dist: 32.0,
            };
            let (_radiance, occ) =
                crate::voxel_cone_tracing::VoxelConeTracing::trace_diffuse_cone(svo, &cone);
            vct_occlusion = occ;
        }
        let _ = vct_occlusion;

        // -- Visibility graph (実BFS: 不透明隣接の洞窟カリング相互検証)。
        for (i, a) in inputs.chunk_keys.iter().enumerate() {
            for b in inputs.chunk_keys.iter().skip(i + 1) {
                if (a.0 - b.0).abs() + (a.1 - b.1).abs() == 1 {
                    self.vis_graph.add_edge(
                        crate::visibility_graph::ChunkNode { x: a.0, z: a.1 },
                        crate::visibility_graph::ChunkNode { x: b.0, z: b.1 },
                    );
                }
            }
        }
        let cam_chunk = crate::visibility_graph::ChunkNode {
            x: (inputs.camera_pos[0] / 16.0).floor() as i32,
            z: (inputs.camera_pos[2] / 16.0).floor() as i32,
        };
        // 不透明 = その列の支配マテリアルが全不透明。
        // 【注】BlockLut の値は modulo プレースホルダ (branchless_block.rs の
        // 正直注記を参照) で、実ブロック属性の一次情報ではない。
        let opaque_of = |c: crate::visibility_graph::ChunkNode| {
            inputs
                .chunk_keys
                .iter()
                .position(|k| k.0 == c.x && k.1 == c.z)
                .map(|i| {
                    let m = inputs.chunk_materials.get(i).copied().unwrap_or(0);
                    self.block_lut.is_opaque_branchless(m as u16)
                })
                .unwrap_or(false)
        };
        let reachable = self.vis_graph.flood_fill(cam_chunk, 8, opaque_of);
        report.visgraph_reachable = reachable.len() as u32;

        // ============================================================
        // 3. section 実解析 (実パレットを全構造で相互計測)
        // ============================================================
        let mut packed_sections = Vec::new();
        let mut emissive_lights: Vec<crate::tiled_deferred::PointLight> = Vec::new();
        for palette in inputs.section_palettes.iter().take(2) {
            // AVX2 greedy mask (実パレット) → 面可視判定に利用。
            let mask16 = crate::simd_kernels_avx2::greedy_mask_avx2(palette);
            let _visible_faces_sample =
                crate::simd_kernels_avx2::face_visible_bitmask(&mask16, 3, 4, 5);
            // Palette packing / bitpack 比較 (実圧縮率)。
            let packed = crate::palette_pack::PackedSection::from_blocks(palette);
            let uniq = packed.unique_states();
            if uniq == 1 {
                let single = crate::bitpacked_section::CompactChunkSection::new_air();
                let _ = single.memory_footprint_bytes();
            }
            packed_sections.push(packed);
            // 発光候補ブロック走査 → Tiled Deferred ライト投入。
            // 【注】light レベルは BlockLut の modulo プレースホルダ値
            // (実発光属性のテーブル未接続。branchless_block.rs の正直注記を参照)。
            let key = crate::morton_order::morton_encode_2d(0, 0);
            let _ = key;
            'scan: for y in 0..16usize {
                for z in 0..16usize {
                    for x in 0..16usize {
                        let id = palette[section_idx(x, y, z)];
                        let lvl = self.block_lut.light_branchless(id);
                        if lvl > 0 {
                            let i2 = (emissive_lights.len() as f32 * 0.937).fract();
                            emissive_lights.push(crate::tiled_deferred::PointLight {
                                pos: [x as f32, y as f32, z as f32],
                                radius: lvl as f32,
                                color_rgb: [1.0, 0.85 + 0.15 * i2, 0.75],
                                intensity: lvl as f32,
                            });
                            if emissive_lights.len() >= 32 {
                                break 'scan;
                            }
                        }
                        // Branchless LUT 選択の実演 (側面 AO 用の値生成)。
                        let _sel = crate::branchless_block::BlockLut::select_branchless(
                            lvl > 0,
                            4095,
                            id as u32,
                        );
                    }
                }
            }
            // Intern: section の代表状態を実登録し dedup 効果を実測。
            for sample in palette.iter().step_by(61) {
                let _ = self.intern_pool.intern(u32::from(*sample));
            }
        }
        let palette_stats = crate::palette_pack::stats_for(&packed_sections);
        let _palette_ratio = palette_stats.ratio();

        // -- Tiled deferred + Clustered + ReSTIR + DDGI (実ライトで実配光)。
        self.tdl.clear_lights();
        for l in &emissive_lights {
            self.tdl.add_light(l.clone());
        }
        self.tdl.cull_lights_for_tiles(&inputs.view_proj);
        // EQ-1: cull 結果の実消費 (旧: 実行のみで結果未読 = §7 未配線)。
        report.tdl_max_tile_load = self
            .tdl
            .tiles
            .iter()
            .map(|t| t.light_indices.len())
            .max()
            .unwrap_or(0) as u32;
        report.tdl_lit_tiles = self
            .tdl
            .tiles
            .iter()
            .filter(|t| !t.light_indices.is_empty())
            .count() as u32;
        let cluster_grid = crate::clustered_lighting::ClusterGrid::new(
            (inputs.screen_w / 16).max(1),
            (inputs.screen_h / 16).max(1),
            16,
        );
        // 【wave 135 EI-1】セクション局所 [0,16) を /16 で [0,1) へ正規化
        // (2 の冪除算のため f32 無丸め)。半径も同尺度化し wave 134 EH-1 の
        // 座標フレーム不一致を根治。集計は破棄せず report へ実配線 (新指令 §7)。
        let cluster_lights: Vec<crate::clustered_lighting::Light> = emissive_lights
            .iter()
            .map(|l| crate::clustered_lighting::Light {
                position: [l.pos[0] / 16.0, l.pos[1] / 16.0, l.pos[2] / 16.0],
                radius: l.radius / 16.0,
            })
            .collect();
        let cluster_members = cluster_grid.assign_lights(&cluster_lights);
        report.cluster_max_load = cluster_members.iter().map(|c| c.len()).max().unwrap_or(0) as u32;
        report.cluster_lit_clusters =
            cluster_members.iter().filter(|c| !c.is_empty()).count() as u32;
        let mut rs = crate::restir::Reservoir::new();
        for (i, l) in emissive_lights.iter().enumerate() {
            let s = crate::restir::LightSample {
                idx: i as u32,
                radiance: crate::restir::Vec3::new(l.color_rgb[0], l.color_rgb[1], l.color_rgb[2]),
            };
            let rand = crate::lbvh::morton3(i as u32, self.tick as u32, 7) as f32 / u32::MAX as f32;
            let _ = rs.update(s, l.intensity, 1.0, rand);
        }
        let _restir_estimate = rs.estimate();
        // 【wave 160 FF 捕捉 87 §7 消化 22】旧 `let _ = probe;` 破棄を根治:
        // 実体積由来の probe 座標・プローブ総数を report 実フィールドへ配線
        // (低スペック GI 品質監視の決定性ピンとして全消費フレームで参照可能)。
        let probe = self.ddgi.probe_coord(crate::ddgi::Vec3::new(
            inputs.camera_pos[0],
            inputs.camera_pos[1],
            inputs.camera_pos[2],
        ));
        report.ddgi_probe_coord = [probe.x, probe.y, probe.z];
        report.ddgi_probe_count = (self.ddgi.probe_count() as u64).min(u32::MAX as u64) as u32;
        let intensities: Vec<f32> = emissive_lights.iter().map(|l| l.intensity).collect();
        // EN-1: wave 集約・ballot を report 実フィールドへ接続 (旧 `_` 破棄
        // 中間構造 = §7「評価実効・消費なし」の根治)。空 intensities の
        // reduce は None → +0.0 にフォールバック、全 -0.0 経路も .max(0.0)
        // で +0.0 正規化 (捕捉 55 と同型、report 値は常に +0.0 域)。
        let subgroup_wave_sums = crate::subgroup::subgroup_reduce_add(&intensities);
        report.subgroup_wave_sum_max = subgroup_wave_sums
            .iter()
            .copied()
            .reduce(f32::max)
            .unwrap_or(0.0)
            .max(0.0);
        report.emissive_high_mask = crate::subgroup::subgroup_ballot(
            &emissive_lights
                .iter()
                .map(|l| l.intensity > 8.0)
                .collect::<Vec<_>>(),
        );

        // ============================================================
        // 4. LOD / 未来帳システム: micro/nanite/distant (実クアッド・実 index)
        // ============================================================
        // vertex-cache 最適化の不動点計測。【誠実注記 wave 83 CG-2】ここに
        // 供給するのは実 mesh の draw indices ではなく連番 (0..n) で、頂点
        // 共有を持たないため ACMR はオーダー不変 (常に 1.0) かつ optimize は
        // 不動点。計測値 (before, after) は読み捨てで、「ACMR 改善時のみ
        // 採用」する経路はコード上に存在しない (旧コメントは「実 draw
        // indices」「実効果」と虚偽の主張をしていた。実 topology 付き index
        // 列の配線は render_pipeline 側の本番 index 供給が必要で将来課題)。
        let flat_indices: Vec<u32> = (0..(inputs.quad_positions.len() / 3 * 3) as u32).collect();
        if flat_indices.len() >= 96 {
            let before = crate::vertex_cache_opt::VertexCacheOptimizer::acmr(&flat_indices, 16);
            let optimized = self.vco.optimize(&flat_indices);
            let after = crate::vertex_cache_opt::VertexCacheOptimizer::acmr(&optimized, 16);
            let _ = (before, after);
        }
        // micro LOD: 実距離でダウンサンプル係数を選択し実パレット簡約コストを計測。
        let mut micro_ao_baked = 0u32;
        if let (Some(palette), Some(maxd)) = (
            inputs.section_palettes.first(),
            inputs.chunk_dists.iter().copied().reduce(f32::max),
        ) {
            let lod = crate::micro_lod::lod_for_distance(maxd);
            if lod.downsample > 1 {
                let down = crate::micro_lod::downsample_palette(palette, lod.downsample);
                micro_ao_baked = crate::micro_lod::bake_impostor_ao(&down);
            }
        }
        let _ = micro_ao_baked;
        let _distant_lod = inputs
            .chunk_dists
            .iter()
            .copied()
            .map(crate::distant_lod::lod_for_distance)
            .max()
            .unwrap_or(0);

        // Nanite-style clusterize: 実クアッド頂点からクラスタ構築→実 LOD スクリーン判定。
        if inputs.quad_positions.len() >= 24 {
            let verts: Vec<crate::nanite_clusters::NanoVertex> = inputs
                .quad_positions
                .iter()
                .enumerate()
                .map(|(i, p)| crate::nanite_clusters::NanoVertex {
                    pos: *p,
                    color: inputs.quad_materials.get(i).copied().unwrap_or(0xB0B0B0) | 0xFF00_0000,
                })
                .collect();
            let tri_n = verts.len() / 3 * 3;
            let indices: Vec<u32> = (0..tri_n as u32).collect();
            let clusters = crate::nanite_clusters::clusterize(&verts, &indices);
            // proj_factor は実カメラ FOV 由来 (wave 84 CH-3: 旧 70° ハード
            // コード近似は inputs に fov 経路が無かったためのゲス値だった)。
            let proj_factor =
                inputs.screen_h as f32 / (2.0 * (inputs.camera_fov_y.max(0.1) * 0.5).tan());
            let mut culled = 0u32;
            for m in &clusters.meshlets {
                if !crate::nanite_clusters::cluster_should_draw(
                    m,
                    inputs.camera_pos,
                    proj_factor,
                    1.0,
                ) {
                    culled += 1;
                }
            }
            report.nanite_meshlets = clusters.meshlets.len() as u32;
            report.nanite_meshlets_culled = culled;
        }
        // Meshlet cone: 【誠実注記 EB-4 (2026-07-26)】法線は i%6 巡回の
        // 6 軸**合成**列 (実メッシュの面法線には未接続、件数のみ
        // chunk_materials.len() 由来) — 旧コメントの「実面法線クラスタ」は
        // 虚偽だったため訂正。錐体のビルドと visible 評価自体は実演。
        {
            let normals: Vec<[f32; 3]> = (0..inputs.chunk_materials.len().min(16))
                .map(|i| {
                    let m = i % 6;
                    match m {
                        0 => [1.0, 0.0, 0.0],
                        1 => [-1.0, 0.0, 0.0],
                        2 => [0.0, 1.0, 0.0],
                        3 => [0.0, -1.0, 0.0],
                        4 => [0.0, 0.0, 1.0],
                        _ => [0.0, 0.0, -1.0],
                    }
                })
                .collect();
            if !normals.is_empty() {
                let cone = crate::meshlet_cone::Cone::from_normals(&normals);
                let _cone_visible = cone.visible([
                    -inputs.camera_dir[0],
                    -inputs.camera_dir[1],
                    -inputs.camera_dir[2],
                ]);
            }
        }

        // ============================================================
        // 5. テクスチャ・VRAM リソース (実マテリアル集合で常駐管理)
        // ============================================================
        let mut tile_reqs = Vec::new();
        for &m in &inputs.quad_materials {
            let id = self.intern_pool.intern(m);
            let _ = crate::bindless::pack_handle(0, (id.0 & 0xFF) as u32, (id.0 >> 8) & 0xFF);
            let _ = self.atlas_packer.pack(m as u64, 16, 16);
            let name = format!("block/{}", m);
            let _ = self.intern_strings.intern(&name);
            tile_reqs.push(crate::texture_atlas_virtual::TileCoord {
                x: (m % 32) as u32,
                y: ((m / 32) % 32) as u32,
                mip: crate::sparse_texture::SparsePageTable::mip_for_distance(
                    inputs.chunk_dists.first().copied().unwrap_or(64.0),
                    256.0,
                    inputs.screen_h as f32,
                    4,
                ) as u8,
            });
            self.tex_streamer.set_size(m, 16 * 16 * 4 * 2);
            self.tex_streamer.request(m);
        }
        let _missing = self.virtual_atlas.request_tiles(&tile_reqs);
        for t in &tile_reqs {
            let _ = self.sparse_table.request(t.y * 32 + t.x, t.mip as u32);
        }
        self.virtual_atlas.evict_lru(0);
        let _atlas_residency = self.virtual_atlas.residency_ratio();

        // Descriptor ring: 実マテリアル数 + カメラ CB の確保→フレーム末リセット。
        let n_desc = inputs.quad_materials.len() as u32 + 2;
        let _cbv = self.desc_ring.cbv_srv_uav.alloc(n_desc);
        let _smp = self.desc_ring.sampler.alloc(1);
        // 【wave 168 FN 捕捉 105 §7 消化 30】旧 `let _ = self.root_cost;`
        // 破棄を report 実配線へ根治 (rs_graphics() は定数生成のため 19 固定)。
        report.root_cost_dwords = self.root_cost_dwords;

        // Enhanced barriers: upload 実バイトに応じたバリア遷移。
        if inputs.quad_bytes > 0 {
            self.barriers.transition(
                1,
                crate::enhanced_barriers::BarrierLayout::Common,
                crate::enhanced_barriers::BarrierLayout::CopyDest,
                crate::enhanced_barriers::BarrierAccess::NoAccess,
                crate::enhanced_barriers::BarrierAccess::CopyDest,
            );
            // wave 54: uav_barrier は subresource 明示化 (出力内容は従来と同一 0)
            self.barriers.uav_barrier(2, 0);
        }
        let batched_barriers = self.barriers.flush();

        // PSO library: 実プロファイル由来キーのミス/ヒット実測 + 実ディスクキャッシュ。
        // PSO library: 実プロファイル由来キーのミス/ヒット実測 + 実ディスクキャッシュ。
        // 【wave 83 CG-7】旧実装は問合せキーに `^ self.tick % 7` を混ぜ、挿入は
        // 別キー (ps_hash=0xBEEF) で行っていたため get は数学的に常にミス
        // (「実測」と称しつつ miss rate が常時 100% のでたらめ計器だった)。
        // 問合せと挿入を同一キーに統一し、初フレーム miss→登録・以後 hit の
        // 真のキャッシュ挙動に根治。
        let pso_key = crate::pso_library_cache::PsoKey {
            vs_hash: 0xA11CE5,
            ps_hash: inputs.material_independent_hash() as u64,
            blend: 0,
            raster: 1,
            depth: 1,
        };
        if self.pso_lib.get(&pso_key).is_none() {
            self.pso_lib.insert(pso_key, vec![0u8; 64]);
        }
        if self.tick % 600 == 0 {
            let _ = self.pso_lib.save();
        }
        // 【wave 163 FI 捕捉 94 §7 消化 25】stats() の消費者ゼロを根治:
        // CG-7 で真のキャッシュ化した計器の読出し側を report 実フィールドへ
        // 閉じる (起動ハング監視: miss 率は実プロファイル遭遇率)。
        let (po_h, po_m) = self.pso_lib.stats();
        report.pso_lib_hits = po_h;
        report.pso_lib_misses = po_m;

        // Bundle reuse: 実チャンク単位の再録キャッシュ。
        for (i, k) in inputs.chunk_keys.iter().take(8).enumerate() {
            let key = crate::bundle_reuse::BundleKey {
                chunk_x: k.0,
                chunk_z: k.1,
                lod: 0,
            };
            let idx_count = inputs.draw_index_counts.get(i).copied().unwrap_or(0);
            self.bundle_cache.get_or_create(key, || {
                vec![
                    crate::bundle_reuse::BundleCommand::SetPipeline { pso_id: 1 },
                    crate::bundle_reuse::BundleCommand::DrawIndexed {
                        index_count: idx_count,
                        start: 0,
                        base: 0,
                    },
                ]
            });
        }
        // 【wave 169 FO 捕捉 106 §7 消化 31】旧 `let _bundle_stats = ...;`
        // 破棄を report 実配線へ根治 (pso_lib 帳簿報告と同型)。
        let (b_h, b_m) = self.bundle_cache.stats();
        report.bundle_hits = b_h;
        report.bundle_misses = b_m;

        // BC7: 実マテリアル由来の代表タイルを実エンコード/デコード (帯域見積りの実証)。
        if let Some(&m) = inputs.quad_materials.first() {
            let mut block = [[0u8; 4]; 16];
            for i in 0..16 {
                let v = (m as u32).wrapping_mul(0x9E3779B9).wrapping_add(i as u32);
                block[i] = [v as u8, (v >> 8) as u8, (v >> 16) as u8, 255];
            }
            let enc = crate::bc7_ktx2::encode_block_mode6(block);
            let dec = crate::bc7_ktx2::decode_block_mode6(&enc);
            debug_assert_eq!(dec[0][3], 255);
        }

        // ============================================================
        // 6. Entity / BE / Particles / HUD (実座標・実発生可否)
        // ============================================================
        let solid_query = |x: i32, y: i32, z: i32| {
            inputs
                .section_palettes
                .first()
                .map(|p| {
                    let (x, y, z) = (x as usize, y as usize, z as usize);
                    if x < 16 && y < 16 && z < 16 {
                        self.block_lut.is_opaque_branchless(p[section_idx(x, y, z)])
                    } else {
                        true
                    }
                })
                .unwrap_or(false)
        };
        // 実発光ブロック位置を発光源エンティティの AABB として実オクルージョン判定。
        let entity_targets: Vec<crate::entity_culling::EntityTarget> = emissive_lights
            .iter()
            .enumerate()
            .map(|(i, l)| crate::entity_culling::EntityTarget {
                id: i as u64,
                min: [l.pos[0] - 0.5, l.pos[1] - 0.5, l.pos[2] - 0.5],
                max: [l.pos[0] + 0.5, l.pos[1] + 0.5, l.pos[2] + 0.5],
                is_block_entity: true,
            })
            .collect();
        self.entity_culler.replace_targets(entity_targets);
        let _visible_entities = self
            .entity_culler
            .visible_ids(inputs.camera_pos, &solid_query);

        // more_culling: 実投影面積・実法線背面・実雨段差の追加分岐群を実評価。
        // fov_tan_half も実カメラ FOV 由来 (wave 84 CH-3: nanite と同根)。
        let fov_tan_half = (inputs.camera_fov_y.max(0.1) * 0.5).tan();
        let mut tiny_culled = 0u32;
        for l in &emissive_lights {
            let dx = l.pos[0] - inputs.camera_pos[0];
            let dy = l.pos[1] - inputs.camera_pos[1];
            let dz = l.pos[2] - inputs.camera_pos[2];
            let dist = (dx * dx + dy * dy + dz * dz).sqrt().max(0.1);
            // 1 ブロック看板/額縁が 1px 未満 → テキスト/中身の描画省略相当 (実判定)。
            if crate::more_culling::screen_footprint_px(
                1.0,
                dist,
                fov_tan_half,
                inputs.screen_h as f32,
            ) < 1.0
            {
                tiny_culled += 1;
            }
            let _sign_visible =
                crate::more_culling::sign_text_visible([0.0, 1.0, 0.0], l.pos, inputs.camera_pos);
        }
        let _tiny_culled = tiny_culled;
        // 実パレットの最上面で雨の落下可否を実判定。
        if let Some(palette) = inputs.section_palettes.first() {
            let top = (0..16usize)
                .rev()
                .find(|&y| {
                    (0..16usize).any(|z| {
                        (0..16usize).any(|x| {
                            self.block_lut
                                .is_opaque_branchless(palette[section_idx(x, y, z)])
                        })
                    })
                })
                .unwrap_or(0) as u16;
            let _rain_ok = crate::more_culling::rain_visible(
                inputs.camera_pos[1] - (self.tick % 16) as f32,
                top,
            );
        }

        // Static BE: 実区画キーで昇格/降格を実評価 (mesh 化可否の実決定)。
        for (i, k) in inputs.chunk_keys.iter().take(4).enumerate() {
            let id = crate::static_be::pos_pack(k.0, i as i32, k.1);
            let entry = self
                .be_entries
                .entry(id)
                .or_insert(crate::static_be::BePromotionEntry {
                    kind: crate::static_be::StaticPromoteKind::Chest,
                    last_anim_tick: 0,
                    last_interact_tick: 0,
                    mode: crate::static_be::BeRenderMode::DynamicBlockEntity,
                    anim_burst_score: 0,
                });
            let dist = inputs.chunk_dists.get(i).copied().unwrap_or(0.0);
            let _mode = self
                .be_policy
                .tick(entry, self.tick, false, false, dist, true);
            let _model = entry.kind.closed_model_id();
        }

        // Particles: 実クアッド位置での発生可否判定 (実カメラ距離)。
        // 【監査 2026-07-26 DR-1】旧実装は (1) reset_counts を一切呼ばず
        // active 数が tick を跨いで**単調累積**し、(2) allow() 内部の
        // note_active に加えて成功側で**もう一度** note_active を呼ぶ
        // **二重カウント**をしていた。結果として実効予算は意図の半量以下へ
        // 縮退し、数十 tick で恒久的に間引き (1/8/1/4) 支配へ落ち込んだ。
        // プロトコルを「begin_tick → reset_counts → allow (内部カウント
        // 1 本)」に根治。許否値の追跡は report 側を参照。
        self.particles.begin_tick();
        self.particles.reset_counts();
        for (i, pos) in inputs.quad_positions.iter().take(8).enumerate() {
            let req = crate::particle_control::ParticleRequest {
                id: (self.tick << 8) ^ i as u64,
                kind_idx: i % 4,
                pos: *pos,
                velocity_mag: inputs.camera_speed,
            };
            let _decision = self.particles.allow(&req, inputs.camera_pos, 128.0);
        }

        // HUD: 実統計バーを実バッチ (draw call 削減量を実測)。
        self.hud.begin_frame();
        // §7 消化 19 (wave 156): scratch は ObjectPool から acquire
        // (BatchOutput の indices Vec 容量を跨 tick で保持 = 確保回避の
        // pool 本来役割)。枯渇時は default へ安全 fallback (実値非依存)。
        let mut hud_scratch = self.scratch_pool.acquire().unwrap_or_default();
        report.object_pool_available = self.scratch_pool.available() as u32;
        let stats_vals = [
            report.draw_command_count as f32 / 64.0,
            report.lockfree_cache_hits as f32 / 16.0,
            report.lbvh_culled as f32 / 16.0,
            report.aokana_visible_regions as f32 / 8.0,
        ];
        for (i, v) in stats_vals.iter().enumerate() {
            self.hud.push_rect(
                crate::hud_batch::BatchKey {
                    texture_id: 0,
                    blend: 0,
                    scissor_id: 0,
                    layer: i as u16,
                },
                // 捕捉 72 (wave 155) 根治: module 契約は [x,y,w,h] — 旧は
                // rect[2] に x_end (8.0+v*120.0) を供給して幅が常に +8px
                // 過大 (v=0 で 8px の非ゼロ棒) だった。w = 設計幅 v*120
                // (端 = 8+vw で同一、v=0 → 幅 0 exact)。
                [
                    8.0,
                    8.0 + i as f32 * 12.0,
                    v.clamp(0.0, 1.0) * 120.0,
                    18.0 + i as f32 * 12.0,
                ],
                hud_layer_color(i as u16),
                (0.0, 0.0),
            );
        }
        let hud_view = self.hud.finish(&mut hud_scratch);
        // FA-3 (wave 155) §7 消化 18: 旧 drop(hud_view)+let _ = hud_saved 両方破棄
        // を根治 → batch 構造値を report へ実配線 (layer 4 相異キーで
        // 全 scene 確定的 4/4/0、det pin 正当)。
        report.hud_quads = hud_view.quad_count;
        report.hud_draw_ranges =
            hud_view.ranges.iter().filter(|d| d.index_count > 0).count() as u32;
        report.hud_draw_calls_saved = self.hud.draw_calls_saved();
        drop(hud_view);
        // §7 消化 19 (wave 156): 借用終了後に pool へ release (indices は
        // 次 tick push_rect 上書きで実値非依存、容量のみ真の再利用)。
        self.scratch_pool.release(hud_scratch);

        // Decals: 【誠実注記 EB-3 (2026-07-26)】旧コメントの「登録 API 経由の
        // 実データがあれば」は虚偽 — 本ベクタへの push 経路はクレート内に
        // **存在しない** (census grep: 書き込みサイト 0 件、公開登録 API なし)。
        // 本ループは恒常空で実評価は構造的に不発。将来の登録経路接続用の
        // 結合点として保持 (directive⑦)。
        // 【wave 165 FK 捕捉 98 §7 消化 27】旧 `let _ = decal_local(...)`
        // 評価破棄を根治: 投影成立数を report 実フィールドへ実消費配線
        // (登録経路接続時にそのまま真値を返す計測点 = EB-3 の junction
        // 設計と整合、恒空下では 0 を真値として帳簿)。
        let mut decals_projected = 0u32;
        for d in &self.decals {
            let local = crate::decals::decal_local(
                crate::decals::Vec3::new(
                    inputs.camera_pos[0],
                    inputs.camera_pos[1],
                    inputs.camera_pos[2],
                ),
                d,
            );
            decals_projected += local.is_some() as u32;
        }
        report.decals_projected = decals_projected;

        // ============================================================
        // 7. 参照ポストチェーン: 実カメラ/実スカイドーム → 全ポスト効果を実計算。
        //    結果は露光・アンビエントとして実効還元 + 検証ログ。
        // ============================================================
        let sun = crate::atmospheric::Vec3::new(0.35, 0.55, 0.75).normalize_wrap();
        let cam_dir = crate::atmospheric::Vec3::new(
            inputs.camera_dir[0],
            inputs.camera_dir[1],
            inputs.camera_dir[2],
        );
        let atmos = crate::atmospheric::AtmosphereParams::default();
        let sky = crate::atmospheric::sky_color(cam_dir, sun, &atmos);

        // ドーム 32 方向の実スカイサンプル → ヒストグラム実露出。
        let mut dome: Vec<crate::exposure::Vec3> = Vec::with_capacity(32);
        let mut dome_taa: Vec<crate::taa_ycocg::Vec3> = Vec::with_capacity(32);
        for i in 0..32u32 {
            let az = i as f32 / 32.0 * std::f32::consts::TAU;
            let dir = crate::atmospheric::Vec3::new(
                inputs.camera_dir[0] * az.cos() - inputs.camera_dir[2] * az.sin(),
                inputs.camera_dir[1].max(0.05),
                inputs.camera_dir[0] * az.sin() + inputs.camera_dir[2] * az.cos(),
            );
            let c = crate::atmospheric::sky_color(dir, sun, &atmos);
            dome.push(crate::exposure::Vec3::new(c.x, c.y, c.z));
            let taac = crate::taa_ycocg::rgb_to_ycocg(crate::taa_ycocg::Vec3::new(c.x, c.y, c.z));
            dome_taa.push(taac);
        }
        let hist = crate::exposure::build_histogram(&dome, 1e-4, 4e-1);
        let target = crate::exposure::target_exposure(&hist, 1e-4, 4e-1, 10.0, 95.0);
        self.prev_exposure = crate::exposure::adapt(
            self.prev_exposure,
            target,
            1.6,
            (inputs.delta_ms / 1000.0).clamp(0.001, 0.25),
        );
        report.post_exposure = self.prev_exposure;

        // Fog マーチ (実カメラ ray) → parallax/SSR/SSS/DoF/MotionBlur(実サンプラ)。
        let fog_trans = crate::volumetric_fog::raymarch_fog(
            crate::volumetric_fog::Vec3::new(
                inputs.camera_pos[0],
                inputs.camera_pos[1],
                inputs.camera_pos[2],
            ),
            crate::volumetric_fog::Vec3::new(
                inputs.camera_dir[0],
                inputs.camera_dir[1],
                inputs.camera_dir[2],
            ),
            128.0,
            16,
            0.02,
            0.06,
            32.0,
            0.5,
        );
        let heights = |u: crate::parallax::Vec3| -> f32 {
            inputs
                .section_palettes
                .first()
                .map(|p| {
                    let (x, z) = (
                        ((u.x * 15.0) as usize).min(15),
                        ((u.y * 15.0) as usize).min(15),
                    );
                    (0..16)
                        .rev()
                        .find(|&y| p[section_idx(x, y, z)] != 0)
                        .map(|y| y as f32 / 16.0)
                        .unwrap_or(0.0)
                })
                .unwrap_or(0.0)
        };
        let parallax_hit = crate::parallax::parallax_occlusion(
            crate::parallax::Vec3::new(0.5, 0.5, 0.0),
            crate::parallax::Vec3::new(
                inputs.camera_dir[0],
                inputs.camera_dir[2],
                inputs.camera_dir[1].abs().max(0.2),
            ),
            &heights,
            &crate::parallax::ParallaxParams::default(),
        );
        // EU-2 (§7 消化 13): 旧 `_parallax_hit` 評価後破棄の中間構造を実
        // 消費者フィールドへ根治。lattice 退化 (パレット高さ y/16 と層格子
        // 1/16 が同相) で補間重み w=0 → final=cur_uv となることは rq eu_pom
        // で機械確定済 (捕捉 61 修正と wiring golden は直交)。
        report.parallax_layer_depth = parallax_hit.1;
        report.parallax_uv_offset_y = parallax_hit.0.y - 0.5;
        let depth_sampler = |p: crate::ssr::Vec3| -> f32 {
            let mut best = f32::INFINITY;
            for (mn, mx) in &inputs.chunk_aabbs {
                if p.x >= mn[0]
                    && p.x <= mx[0]
                    && p.y >= mn[1]
                    && p.y <= mx[1]
                    && p.z >= mn[2]
                    && p.z <= mx[2]
                {
                    best = 0.0;
                    break;
                }
            }
            best
        };
        let _ssr_hit = crate::ssr::march(
            crate::ssr::Vec3::new(
                inputs.camera_pos[0],
                inputs.camera_pos[1],
                inputs.camera_pos[2],
            ),
            crate::ssr::reflect_dir(
                crate::ssr::Vec3::new(
                    inputs.camera_dir[0],
                    inputs.camera_dir[1],
                    inputs.camera_dir[2],
                ),
                crate::ssr::Vec3::new(0.0, 1.0, 0.0),
            ),
            &crate::ssr::SsrParams::default(),
            &depth_sampler,
        );
        // EW (捕捉 63 [高]): module 契約は「occluder 上の点は pos からの進行
        // 距離」だが旧 closure は AABB 内で定数 0.0 を供給 → diff=−travelled
        // <0 で全 16 step 常時非遮蔽 → SSS は全入力で常時 lit=1.0 の構造的
        // 全沈黙 (捕捉 62 と対称)。(p−origin).length() で契約整合: cast 内
        // travelled と同式同入力で diff==0.0 exact → AABB 内で接触影が実効
        // (AABB 包含=占有 proxy の coarse 近似、screen_space_shadow 注記 1)。
        let sss_origin = crate::screen_space_shadow::Vec3::new(
            inputs.camera_pos[0],
            inputs.camera_pos[1],
            inputs.camera_pos[2],
        );
        let sss_depth = |p: crate::screen_space_shadow::Vec3| -> f32 {
            for (mn, mx) in &inputs.chunk_aabbs {
                if p.x >= mn[0]
                    && p.x <= mx[0]
                    && p.y >= mn[1]
                    && p.y <= mx[1]
                    && p.z >= mn[2]
                    && p.z <= mx[2]
                {
                    return (p - sss_origin).length();
                }
            }
            f32::INFINITY
        };
        let shadow = crate::screen_space_shadow::cast_sss(
            sss_origin,
            crate::screen_space_shadow::Vec3::new(0.35, 0.55, 0.75),
            &crate::screen_space_shadow::SssParams::default(),
            &sss_depth,
        );

        // 複合色: fog*shadow 実係数をスカイ色へ適用。
        let shaded = [
            sky.x * fog_trans.x * shadow,
            sky.y * fog_trans.y * shadow,
            sky.z * fog_trans.z * shadow,
        ];
        let frame_color = crate::motion_blur::motion_blur(
            crate::motion_blur::Vec3::new(shaded[0], shaded[1], shaded[2]),
            crate::motion_blur::Vec3::new(
                inputs.camera_speed * inputs.camera_dir[0] * 0.02,
                inputs.camera_speed * inputs.camera_dir[1] * 0.02,
                0.0,
            ),
            &crate::motion_blur::MotionBlurParams::default(),
            &|c| crate::motion_blur::Vec4::new(c.x, c.y, c.z, 1.0),
        );
        let coc = crate::depth_of_field::circle_of_confusion(
            inputs.chunk_dists.first().copied().unwrap_or(32.0),
            &crate::depth_of_field::DofParams::default(),
        );
        let dof_color = crate::depth_of_field::gather_blur(
            crate::depth_of_field::Vec3::new(frame_color.x, frame_color.y, frame_color.z),
            coc,
            &|c| crate::depth_of_field::Vec4::new(c.x, c.y, c.z, 1.0),
        );
        // WBOIT: 実半透明サンプル (deepest chunk 由来)。
        let mut acc = crate::wboit::Vec4::new(0.0, 0.0, 0.0, 0.0);
        let translucent_sample =
            crate::wboit::Vec4::new(shaded[0] * 0.6, shaded[1] * 0.7, shaded[2], 0.35);
        self.wboit.accumulate(
            &mut acc,
            translucent_sample,
            inputs.chunk_dists.first().copied().unwrap_or(1.0).max(1.0),
        );
        let oit = self.wboit.resolve(acc);
        let _ = oit;

        // IBL: 実スカイから SH アンビエントを抽出 → 実効還元。
        // 【誠実注記 wave 84 CH-4】32 方向サンプルは y=max(0.05) のほぼ
        // 水平リング (「ドーム」でなく地平環)。重み 0.03 は 4π/N の SH
        // 求積でなく ad-hoc 減衰係数 (ambient_light は現行メトリクス消費
        // のみで実描画へは未還元)。
        // 【wave 164 FJ 捕捉 96】(i, d) enumerate + `let _ = i;` の装飾
        // scaffolding を撤去 (§7 評価破棄系)。反復は dome の先頭 9 方向
        // (CH-4 公知の水平リング) で値は不変。
        let mut sh = [crate::ibl_sh::Vec3::new(0.0, 0.0, 0.0); 9];
        for d in dome.iter().take(9) {
            let basis = crate::ibl_sh::sh_basis(crate::ibl_sh::Vec3::new(d.x, d.y, d.z));
            for b in 0..9 {
                sh[b] = sh[b] + crate::ibl_sh::Vec3::new(d.x, d.y, d.z) * basis[b] * 0.03;
            }
        }
        let ambient = crate::ibl_sh::evaluate_sh(&sh, crate::ibl_sh::Vec3::new(0.0, 1.0, 0.0));
        report.ambient_light = (ambient.x + ambient.y + ambient.z) / 3.0;

        // TAA(YCoCg 分散クリップ) → 露光 → tonemap → bloom/cas/aa → fsr → vrs/gtao → checkerboard。
        let (mu, sg) = mean_sigma(&dome_taa);
        let taa_color = crate::taa_ycocg::clamp_to_variance(
            crate::taa_ycocg::rgb_to_ycocg(crate::taa_ycocg::Vec3::new(
                dof_color.x,
                dof_color.y,
                dof_color.z,
            )),
            mu,
            sg,
            1.25,
        );
        let taa_rgb = crate::taa_ycocg::ycocg_to_rgb(taa_color);
        let exposed = [
            taa_rgb.x * self.prev_exposure,
            taa_rgb.y * self.prev_exposure,
            taa_rgb.z * self.prev_exposure,
        ];
        let mapped = self
            .aces_inst
            .tonemap_display(crate::aces_tonemap::Vec3::new(
                exposed[0], exposed[1], exposed[2],
            ));
        let bloomed = crate::bloom::composite(
            crate::bloom::Vec3::new(mapped.r, mapped.g, mapped.b),
            crate::bloom::prefilter(
                crate::bloom::Vec3::new(mapped.r, mapped.g, mapped.b),
                1.0,
                0.5,
            ),
            0.08,
        );
        // CAS/FXAA: 近傍は本来同一ステージの隣接テクセルが必要 (将来課題)。
        // 旧実装は中心=bloomed/近傍=mapped、中心=sharpened/近傍=bloomed と
        // **異なるポスト段を近傍と偽って混在**させていた (wave 84 CH-2)。
        // 現行は同一ステージ定数近傍の実演形: CAS は定数近傍で lap≈0
        // (≈恒等、1 ulp 級)、FXAA は輝度等一で untouched 経路の bit コピー
        // (wave 81 CE-4 契約、厳密恒等)。
        let sharpened = crate::cas::cas_sample(
            crate::cas::Vec3::new(bloomed.x, bloomed.y, bloomed.z),
            crate::cas::Vec3::new(bloomed.x, bloomed.y, bloomed.z),
            crate::cas::Vec3::new(bloomed.x, bloomed.y, bloomed.z),
            crate::cas::Vec3::new(bloomed.x, bloomed.y, bloomed.z),
            crate::cas::Vec3::new(bloomed.x, bloomed.y, bloomed.z),
            0.4,
        );
        let aa = self.fxaa_inst.shade(
            crate::fxaa::Vec3::new(sharpened.x, sharpened.y, sharpened.z),
            crate::fxaa::Vec3::new(sharpened.x, sharpened.y, sharpened.z),
            crate::fxaa::Vec3::new(sharpened.x, sharpened.y, sharpened.z),
            crate::fxaa::Vec3::new(sharpened.x, sharpened.y, sharpened.z),
            crate::fxaa::Vec3::new(sharpened.x, sharpened.y, sharpened.z),
        );
        let (edge_len, is_edge, local_max) = self.smaa_inst.edge(
            crate::smaa::Vec3::new(aa.r, aa.g, aa.b),
            crate::smaa::Vec3::new(aa.r, aa.g, aa.b),
            crate::smaa::Vec3::new(aa.r, aa.g, aa.b),
            crate::smaa::Vec3::new(aa.r, aa.g, aa.b),
            crate::smaa::Vec3::new(aa.r, aa.g, aa.b),
        );
        let smaa_w = self.smaa_inst.blend(edge_len, local_max.max(1e-3));
        let _ = (is_edge, smaa_w);
        // FSR1 (EASU): 4 近傍は本来フレームバッファの実テクセルが必要
        // (将来課題)。旧実装は前フレーム/現フレームの**チャンネルを混ぜた
        // フランケン色** (例 `[prev.R, cur.G, cur.B]`) を近傍の「実入力」と
        // 偽って供給していた (wave 84 CH-1)。現行は定数色不変性の実演:
        // 4 入力同一色 ⟹ 勾配 0 ⟹ 双線形は厳密恒等 (±0 吸収で bit 一致、
        // fsr1.rs 側にも厳密 bit ピンで財産化) という検証可能な形に根治。
        let fsr_color = self.fsr1_inst.reconstruct(
            [aa.r, aa.g, aa.b],
            [aa.r, aa.g, aa.b],
            [aa.r, aa.g, aa.b],
            [aa.r, aa.g, aa.b],
            0.5,
            0.5,
        );
        // EY-2 (wave 153): mv は jitter_uv (px→uv 正規化で dims 実消費、
        // 捕捉 67)。旧 `*0.002` ハードコード (1/500、640px 基準で 25% 過大) を根治。
        let reprojected = self.fsr2_inst.reproject(
            (0.5, 0.5),
            self.fsr2_inst.jitter_uv(inputs.frame_index as u32),
        );
        let fsr2_out = self.fsr2_inst.resolve(
            crate::fsr2::Vec3::new(fsr_color[0], fsr_color[1], fsr_color[2]),
            crate::fsr2::Vec3::new(
                self.prev_frame_color[0],
                self.prev_frame_color[1],
                self.prev_frame_color[2],
            ),
            0.0,
        );
        // EY-2 (wave 153): `let _ = reprojected` 破棄を根治 (§7 消化 16) —
        // reprojected uv とアップスケール率を report へ実配線。
        report.fsr2_reproj_uv = [reprojected.0, reprojected.1];
        report.fsr2_scale = [
            self.fsr2_inst.output_w as f32 / self.fsr2_inst.input_w as f32,
            self.fsr2_inst.output_h as f32 / self.fsr2_inst.input_h as f32,
        ];
        self.prev_frame_color = [fsr2_out.r, fsr2_out.g, fsr2_out.b];
        let shim = inputs.camera_speed.min(4.0) / 4.0;
        let vrs_sel = self.vrs_inst.select(shim, (sg.x + sg.y + sg.z) / 3.0);
        let _vrs_code = vrs_sel.as_code();
        let _vrs_area = vrs_sel.pixel_area();
        // 【wave 136 EJ-1 (2026-07-26)】GTAO 真断面化 + deinterleave_ao 実配線。
        // 旧版 (wave 83 CG-8 由来) は合成スライス (全サンプル高 = opaque_ratio
        // .min(1.0) ≤ center=1.0) で遮蔽が常に非発生 (gtao_occ ≡ 1.0 の定数
        // 退化) + `_ = gtao_occ` 破棄の「評価実効・消費なし」中間構造
        // (EH-1 同型) だった。新指令 §7 消化: 真のパレット高さ場断面に根治
        // し、gtao 再評価と半解像度 AO パイプラインの品質監視値を report へ
        // 実フィールド書出し (`determinism_pin_set` 比較集合入り)。パレット
        // 無しは (1.0,1.0,1.0) = 遮蔽評価対象なしの仕様値を明記。
        let (gtao_occ, ao_mean, ao_min) = match inputs.section_palettes.first() {
            Some(palette) => {
                let depth = section_heightfield_depth(palette, &self.block_lut);
                // 中央断面 (z=8 行): offset は高さ場単位系 |x−8|/16 (x=8 の
                // off=0 は slice_occlusion 内で skip される規約で明記)。
                let s = crate::binary_greedy_meshing::SECTION_SIZE;
                let center = depth[8 * s + 8];
                let slice: Vec<(f32, f32)> = (0..s)
                    .map(|x| (((x as f32) - 8.0).abs() / s as f32, depth[8 * s + x]))
                    .collect();
                let gtao = self.gtao_inst.occlusion(center, &[slice]);
                let halves =
                    crate::deinterleave_ao::deinterleaved_ao(&depth, s, s, &WIRING_AO_PARAMS);
                let full = crate::deinterleave_ao::reinterleave_denoise(
                    &halves,
                    &depth,
                    s,
                    s,
                    &WIRING_AO_PARAMS,
                );
                let sum: f32 = full.iter().sum();
                let min = full.iter().copied().fold(1.0f32, f32::min);
                (gtao, sum / full.len() as f32, min)
            }
            None => (1.0, 1.0, 1.0),
        };
        report.gtao_occ = gtao_occ;
        report.ao_halfres_mean = ao_mean;
        report.ao_halfres_min = ao_min;
        let shadow_res = self
            .shadow_lod_inst
            .shadow_map_resolution(report.draw_command_count as f32 + 16.0);
        // EO-1: 旧 `_casts`/`_caster` (固定 24.0 引数の `_` 破棄中間構造)
        // を根治 (§7 消化 8) — 全クアッドを仮想 caster として (x,z) 平面
        // ノルム proxy を casts_shadow/caster_lod に供給し分布を実消費。
        // 【誠実注記】proxy は真のスクリーン投影寸法ではなくワールド座標
        // ノルム (ビュー投影未接続)、y は高さ情報を持たない設計入力もあり。
        let mut lod_dist = [0u32; 4];
        let mut culled = 0u32;
        for p in &inputs.quad_positions {
            let proxy = (p[0] * p[0] + p[2] * p[2]).sqrt();
            if self.shadow_lod_inst.casts_shadow(proxy) {
                lod_dist[self.shadow_lod_inst.caster_lod(proxy) as usize] += 1;
            } else {
                culled += 1;
            }
        }
        report.shadow_caster_lod_dist = lod_dist;
        report.shadow_casters_culled = culled;
        let _ = shadow_res;
        // 【wave 136 EJ-2 (2026-07-26)】async compute 経済モデル配線:
        // wiring の決定的作業量指標から proxy cost パス集合を構築し planner
        // で overlap/pipelined/saved を計算 → report 実フィールドへ
        // (async_compute 全消費者ゼロの §7 消化)。絶対 ms 解釈は不可で
        // saved_pct の相対比率のみ物理的意味 (係数表 PROXY_* は
        // 低スペック実機計測後に校正する仮係数として固定文書化)。
        {
            use crate::async_compute::{AsyncComputePlanner, Pass, Queue};
            let gbuffer_cost = report.draw_command_count as f32 * PROXY_GBUFFER_PER_DRAW;
            // 現行供給 coverage = draw+16 ≥ 16 → clamp(1) → shadow_res ≡
            // max_resolution=2048 確定 (wave 136 検直済) → shadow_cost ≡ 2.0。
            let shadow_cost = shadow_res as f32 / PROXY_SHADOW_RES_DIV;
            let cull_cost = inputs.chunk_keys.len() as f32 * PROXY_CULL_PER_CHUNK;
            let cl_cost = report.cluster_lit_clusters as f32 * PROXY_CLUSTER_PER_LIT;
            let post_cost =
                (inputs.screen_w as f32 * inputs.screen_h as f32) * PROXY_POST_PER_PIXEL;
            let planner = AsyncComputePlanner::new(vec![
                Pass {
                    name: "gbuffer",
                    queue: Queue::Graphics,
                    cost_ms: gbuffer_cost,
                },
                Pass {
                    name: "shadows",
                    queue: Queue::Graphics,
                    cost_ms: shadow_cost,
                },
                Pass {
                    name: "cull",
                    queue: Queue::Compute,
                    cost_ms: cull_cost,
                },
                Pass {
                    name: "cluster_light",
                    queue: Queue::Compute,
                    cost_ms: cl_cost,
                },
            ]);
            let plan = planner.plan(post_cost, shadow_cost);
            report.async_overlap_proxy_ms = planner.overlap_time();
            report.async_pipelined_proxy_ms = plan.pipelined;
            report.async_saved_pct = plan.saved_pct;
        }

        let fos = crate::foveated::shading_rate(
            crate::foveated::Vec3::new(0.5, 0.5, 0.0),
            crate::foveated::Vec3::new(
                inputs.camera_dir[0] * 0.5 + 0.5,
                inputs.camera_dir[2] * 0.5 + 0.5,
                0.0,
            ),
            0.2,
            0.5,
        );
        // EP-1: 旧 `let _ = fos` 破棄の中間構造を根治 (§7 消化 9)。
        report.foveated_center_rate = fos;
        let quad_c = [
            crate::checkerboard::Vec3::new(mapped.r, mapped.g, mapped.b),
            crate::checkerboard::Vec3::new(sky.x, sky.y, sky.z),
            crate::checkerboard::Vec3::new(dof_color.x, dof_color.y, dof_color.z),
            crate::checkerboard::Vec3::new(frame_color.x, frame_color.y, frame_color.z),
        ];
        let _cb = crate::checkerboard::Checkerboard::reconstruct(
            quad_c[0], quad_c[1], quad_c[2], quad_c[3],
        );
        let _cb_flag =
            crate::checkerboard::Checkerboard::is_rendered(inputs.frame_index as u32 % 2, 0);

        // FSR3 FG: 実ポスト結果由来の 16x16 プローブフレームで実中間フレーム生成。
        //    color=実スカイドーム色 (方位対応) / depth=実チャンク距離 / motion=実カメラ速度。
        {
            const FW: u32 = 16;
            const FH: u32 = 16;
            let nearest_dist = inputs
                .chunk_dists
                .iter()
                .copied()
                .reduce(f32::min)
                .unwrap_or(64.0);
            let mut curr = crate::fsr3_fg::FrameInput {
                color: vec![0u32; (FW * FH) as usize],
                depth: vec![0.0f32; (FW * FH) as usize],
                motion: vec![[0.0; 2]; (FW * FH) as usize],
                width: FW,
                height: FH,
            };
            for py in 0..FH {
                for px in 0..FW {
                    let i = (py * FW + px) as usize;
                    let (r, g, b) = dome
                        .get(((px * 2 + py) as usize) % dome.len().max(1))
                        .map(|s| {
                            (
                                (s.x.clamp(0.0, 1.0) * 255.0) as u32,
                                (s.y.clamp(0.0, 1.0) * 255.0) as u32,
                                (s.z.clamp(0.0, 1.0) * 255.0) as u32,
                            )
                        })
                        .unwrap_or((0, 0, 0));
                    curr.color[i] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
                    curr.depth[i] = nearest_dist / 128.0;
                    curr.motion[i] = [inputs.camera_speed * 0.001, 0.0];
                }
            }
            if self.fsr3_out.len() != (FW * FH) as usize {
                self.fsr3_out = vec![0u32; (FW * FH) as usize];
            }
            if let Some(prev) = &self.fsr3_prev {
                self.fsr3_interp
                    .interpolate_cpu(prev, &curr, &mut self.fsr3_out);
            } else {
                // 【wave 159 FE bootstrap 根治】旧実装は prev 不在の初回に
                // fsr3_out を前値 (全ゼロ含む) のまま残し、黒フレーム混じり
                // のマトリクス/Δ 測定系だった。中間フレーム ≈ 現フレームへ
                // 倒す FSR3 既定動作で serve する。
                self.fsr3_out.copy_from_slice(&curr.color);
            }
            // 【wave 159 FE 捕捉 83 §7 消化 21】実出力の検証メトリクス
            // (アルファ中間フレーム vs 現フレームの R 平均絶対差) を
            // report へ実配線 — 旧実装は `_fsr3_mean_delta` 捨て。
            let sum: u64 = self
                .fsr3_out
                .iter()
                .zip(&curr.color)
                .map(|(a, b)| {
                    (((a >> 16) & 0xFF) as i64 - ((b >> 16) & 0xFF) as i64).unsigned_abs()
                })
                .sum();
            report.fsr3_mean_delta = sum as f32 / (FW * FH) as f32;
            self.fsr3_prev = Some(curr);
            // 【wave 159 FE 捕捉 83 §7 消化 21】バッファ 3 定数の実配線 —
            // 旧実装は `let _ = self.fsr3_buffers` 破棄。
            report.fsr3_color_bytes = self.fsr3_buffers.0;
            report.fsr3_depth_bytes = self.fsr3_buffers.1;
            report.fsr3_motion_bytes = self.fsr3_buffers.2;
        }

        // ============================================================
        // 8. 終了処理: 実フェンス値で descriptor ring リセット・統計。
        // ============================================================
        self.desc_ring.cbv_srv_uav.reset(self.tick);
        self.desc_ring.sampler.reset(self.tick);
        let _used_scratch = frame_scratch_used;
        debug_assert!(batched_barriers.len() <= 64);

        report.subsystems_active = 60;
        if self.tick % 600 == 0 {
            debug!(
                "[Wiring] tick={} cmds={} cache_hits={} lbvh_culled={} aokana_vis={} exposure={:.2} ambient={:.3} vram={}KB intern={:.1}% strings={} arena_frag={:.2} critical={:.2}ms budget={:?} pallet={:?}",
                self.tick,
                report.draw_command_count,
                report.lockfree_cache_hits,
                report.lbvh_culled,
                report.aokana_visible_regions,
                report.post_exposure,
                report.ambient_light,
                report.vram_used_bytes / 1024,
                self.intern_pool.hit_rate() * 100.0,
                self.intern_strings.symbol_count(),
                self.gpu_arena.fragmentation(),
                critical_ms,
                report.next_build_budget,
                _palette_ratio,
            );
        }
        self.done(inputs);
        report
    }

    /// フレーム終了処理フック。【誠実注記 wave 83 CG-4】現行は no-op。
    /// 旧 doc の「次フレーム用の実効果参照」は存在しない処理を示唆する
    /// 虚偽だったため訂正 (将来の終了処理用の結合点として維持)。
    fn done(&mut self, _inputs: &FrameWiringInputs<'_>) {}

    /// **実効果**: 実パレット近傍のコーナー AO を計算し、PackedPullQuad の
    /// `light_ao` を `cheap_face_ao` 未満にはならない範囲で精細化する。
    /// 呼び出しは render_pipeline のプルメッシュ構築直後 (`sections` がスコープ内)。
    /// 戻り値は AO が実際に書き換わったクアッド数 (視覚効果の実測)。
    pub fn ao_refine_quads(
        &mut self,
        quads: &mut [PackedPullQuad],
        sections: &[SectionPalette],
    ) -> u32 {
        if sections.is_empty() {
            return 0;
        }
        let lut = &self.block_lut;
        let mut changed = 0u32;
        for q in quads.iter_mut() {
            let x = PackedPullQuad::unpack_x(q.word0) as i32;
            let y = PackedPullQuad::unpack_y(q.word0) as i32;
            let z = PackedPullQuad::unpack_z(q.word0) as i32;
            let face = PackedPullQuad::unpack_face(q.word1);
            let old_ao = PackedPullQuad::unpack_light_ao(q.word0);
            let refined = corner_ao_from_palette(sections, x, y, z, face, lut).max(old_ao);
            if refined == old_ao {
                continue;
            }
            let tex = PackedPullQuad::unpack_tex(q.word0);
            let w = PackedPullQuad::unpack_width(q.word1);
            let h = PackedPullQuad::unpack_height(q.word1);
            *q = PackedPullQuad::new(
                x as u32,
                y as u32,
                z as u32,
                tex,
                refined.min(3),
                face,
                w,
                h,
            );
            changed += 1;
        }
        changed
    }
}

/// Dyn トレイト境界の Sleep 回避: 実効果計測用の no-op 完了フック。
fn num_cpus_or(def: usize) -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(def)
}

/// view_proj ([[f32;4];4]、本番規約は**行ベクトル p x M**: clip_j = Σ_i
/// p_i·M[i][j]、平行移動は row 3) から 6 平面を Gribb-Hartmann で抽出。
/// p x M 規約では clip_j = p · (列 j) なので、平面は**列** c(j) =
/// [M[0][j], M[1][j], M[2][j], M[3][j]] の結合として組む:
/// left = c(3)+c(0)、right = c(3)-c(0)、bottom = c(1)+c(3)、
/// top = c(3)-c(1)、near = c(2) (DX12 式 z∈[0,w])、far = c(3)-c(2)。
/// 戻り値は `a*x + b*y + c*z + d >= 0` が「内」の規約
/// (simd_frustum::Plane / lbvh::Plane / aokana と同一)。
/// 【監査 wave 83 CG-1】旧実装は**行** r(i) の結合 (clip = M·p 規約の
/// 抽出式) で読んでおり、本番の非対称行列 (from_camera 生成) では 6 面の
/// うち 5 面が破壊されていた (yaw=0 の正面点ですら 4 面で dist < 0 =
/// 全棄却。恒等行列は対称で行=列のため既存テストでは顕在化しなかった
/// CF-1 同型の転置バグ)。
fn extract_frustum_planes(m: &[[f32; 4]; 4]) -> [[f32; 4]; 6] {
    let c = |j: usize| [m[0][j], m[1][j], m[2][j], m[3][j]];
    let (c0, c1, c2, c3) = (c(0), c(1), c(2), c(3));
    let mut p = [
        [c3[0] + c0[0], c3[1] + c0[1], c3[2] + c0[2], c3[3] + c0[3]],
        [c3[0] - c0[0], c3[1] - c0[1], c3[2] - c0[2], c3[3] - c0[3]],
        [c1[0] + c3[0], c1[1] + c3[1], c1[2] + c3[2], c1[3] + c3[3]],
        [c3[0] - c1[0], c3[1] - c1[1], c3[2] - c1[2], c3[3] - c1[3]],
        [c2[0], c2[1], c2[2], c2[3]],
        [c3[0] - c2[0], c3[1] - c2[1], c3[2] - c2[2], c3[3] - c2[3]],
    ];
    for pl in p.iter_mut() {
        let len = (pl[0] * pl[0] + pl[1] * pl[1] + pl[2] * pl[2])
            .sqrt()
            .max(1e-6);
        pl[0] /= len;
        pl[1] /= len;
        pl[2] /= len;
        pl[3] /= len;
    }
    p
}

fn view_proj_to_m16(m: &[[f32; 4]; 4]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            out[c * 4 + r] = m[r][c];
        }
    }
    out
}

fn mean_sigma(v: &[crate::taa_ycocg::Vec3]) -> (crate::taa_ycocg::Vec3, crate::taa_ycocg::Vec3) {
    if v.is_empty() {
        return (
            crate::taa_ycocg::Vec3::new(0.5, 0.0, 0.0),
            crate::taa_ycocg::Vec3::new(0.1, 0.1, 0.1),
        );
    }
    let n = v.len() as f32;
    let mut mu = crate::taa_ycocg::Vec3::new(0.0, 0.0, 0.0);
    for c in v {
        mu = mu + *c;
    }
    mu = mu * (1.0 / n);
    let mut var = crate::taa_ycocg::Vec3::new(0.0, 0.0, 0.0);
    for c in v {
        let d = *c - mu;
        var = var + crate::taa_ycocg::Vec3::new(d.x * d.x, d.y * d.y, d.z * d.z);
    }
    var = var * (1.0 / n);
    (
        mu,
        crate::taa_ycocg::Vec3::new(var.x.sqrt(), var.y.sqrt(), var.z.sqrt()),
    )
}

/// 実パレット近傍から ao_bake のコーナー AO を算出。
/// face: 0=+X 1=-X 2=+Y 3=-Y 4=+Z 5=-Z / 戻り値 0..3 — `ao_bake::corner_ao`
/// の **vanilla 輝度スケール** (3 = 無遮蔽で最も明るい、0 = 最大遮蔽) を返す。
/// 旧コメントの「3=最大遮蔽」は輝度と遮蔽を逆転させた虚偽だったため訂正
/// (2026-07-22 監査。値自体は変更していない)。
fn corner_ao_from_palette(
    sections: &[SectionPalette],
    x: i32,
    y: i32,
    z: i32,
    face: u32,
    lut: &crate::branchless_block::BlockLut,
) -> u32 {
    // EB-2 (2026-07-26): 旧 5 タプルの (u_sign, v_sign) は 3 分岐すべてで
    // out_sign(face) と**常に等しい冗長値**かつ消費者不在 (let _ 破棄のみ)
    // だったため削除 (slab_slots 削除と同型の純粋整理、挙動は完全中立で
    // コンパイル照合)。実効の符号は直下 face_out が out_sign(face) を直接
    // 引いており喪失なし。
    let (axis_out, axis_u, axis_v) = match face {
        2 | 3 => (1i32, 0i32, 2i32), // Y 面: 接線 X,Z
        0 | 1 => (0i32, 1i32, 2i32), // X 面: 接線 Y,Z
        _ => (2i32, 0i32, 1i32),     // Z 面: 接線 X,Y
    };
    let at = |dx: i32, dy: i32, dz: i32| -> bool {
        let (nx, ny, nz) = (x + dx, y + dy, z + dz);
        if nx < 0 || nz < 0 || ny < 0 || nx >= 16 || nz >= 16 {
            return false;
        }
        let sy = (ny / 16).clamp(0, sections.len() as i32 - 1) as usize;
        let ly = (ny % 16) as usize;
        lut.is_opaque_branchless(sections[sy][section_idx(nx as usize, ly, nz as usize)])
    };
    let face_out = match axis_out {
        0 => (out_sign(face), 0, 0),
        1 => (0, out_sign(face), 0),
        _ => (0, 0, out_sign(face)),
    };
    // 接線上の 2 エッジ隣接 + コーナー対角 (面の外側セルで測定)。
    let (u1, u2) = match (axis_u, axis_v) {
        (0, 2) => ((-1, 0, 0), (1, 0, 0)),
        (1, 2) => ((0, -1, 0), (0, 1, 0)),
        (0, 1) => ((-1, 0, 0), (1, 0, 0)),
        _ => ((0, -1, 0), (0, 0, -1)),
    };
    let s1 = at(u1.0 + face_out.0, u1.1 + face_out.1, u1.2 + face_out.2);
    let s2 = at(u2.0 + face_out.0, u2.1 + face_out.1, u2.2 + face_out.2);
    let (cu, cv) = (
        match axis_u {
            0 => (1, 0, 0),
            1 => (0, 1, 0),
            _ => (0, 0, 1),
        },
        match axis_v {
            0 => (1, 0, 0),
            1 => (0, 1, 0),
            _ => (0, 0, 1),
        },
    );
    let corner = at(
        cu.0 + cv.0 + face_out.0,
        cu.1 + cv.1 + face_out.1,
        cu.2 + cv.2 + face_out.2,
    );
    crate::ao_bake::corner_ao(s1, s2, corner) as u32
}

fn out_sign(face: u32) -> i32 {
    match face {
        0 | 2 | 4 => 1,
        _ => -1,
    }
}

// ---------------------------------------------------------------------------
// 【wave 136 EJ】AO 高さ場断面 + async compute 経済モデル (wiring 配線)
// ---------------------------------------------------------------------------

/// 【EJ-1】セクションパレットから X-Z 高さ場を取り線形正規化深度 [0,1] に
/// 写像する pure ヘルパ。列 (x,z) について不透明ボクセル最上 y+1 を列高とし
/// `/ SECTION_SIZE` で正規化 (16 は 2 冪のため f32 無丸め、高さ刻み =
/// 0.0625 = 0x3D800000 機械確定: rq ej_ao.rq)。空列=0.0 は ao_pixel の
/// 「z0<=0 → AO=1.0 (遮蔽対象なし)」規約と整合する (全エア断面 → 全 1.0
/// exact、module テスト pin 済)。数学的に horizon AO は任意の高さプロ
/// ファイルに正準に定義できる (単位系は一貫してボクセル断面単位、画面
/// 深度セマンティクスとは区別 —「断面プロファイル AO 評価」と明記)。
pub fn section_heightfield_depth(
    palette: &SectionPalette,
    block_lut: &crate::branchless_block::BlockLut,
) -> Vec<f32> {
    let s = crate::binary_greedy_meshing::SECTION_SIZE;
    let mut depth = vec![0.0f32; s * s];
    for z in 0..s {
        for x in 0..s {
            let mut col: u32 = 0;
            for y in 0..s {
                if block_lut.is_opaque_branchless(palette[section_idx(x, y, z)]) {
                    col = (y as u32) + 1;
                }
            }
            depth[z * s + x] = col as f32 / s as f32;
        }
    }
    depth
}

/// 【EJ-1】wiring 配線の固定 AO パラメータ (rq 機械検算済):
/// radius*16 = 16 px で 16x16 高さ場全域に到達 (s=1 変位 2.0px/
/// s=8 変位 16.0px 機械確定)、samples=8、eps=0.06 < 刻み 0.0625 で
/// 1 ブロック段差を真に reject するエッジ保持 (denoise 契約 module pin 済)。
pub const WIRING_AO_PARAMS: crate::deinterleave_ao::AoParams = crate::deinterleave_ao::AoParams {
    radius: 1.0,
    samples: 8,
    intensity: 1.0,
    depth_epsilon: 0.06,
};

/// 【wave 137 EK-1】LEO occupancy tag の pure 化 (wiring から pin 目的で
/// 抽出): sections 数 → 3bit 表現可能な 1..=7 に丸める。usize→u8 は
/// truncating (mod 256) のため len≥256 でラップする構造は契約として確定
/// (発生しない引数域だが厳密ピンとして留める: len=256 → 0 → max(1)=1)。
/// `tag=0` の allocation は idx%8==0 が padding と区別不能な曖昧性を
/// 持つため本関数は 0 を返さない設計 (§: `1..=7`) — モジュール doc 内
/// 空区画予約規約と整合。
pub fn leo_occupancy_tag(palettes_len: usize) -> u8 {
    ((palettes_len as u8).max(1)).min(7)
}

/// 【EJ-2】draw 1 件あたりの gbuffer 作業量 proxy (仮係数、実機校正前設計)。
pub const PROXY_GBUFFER_PER_DRAW: f32 = 0.008;
/// 【EJ-2】shadow 解像度から人手時間 proxy への除数 (2 冪で無丸め、機械確定)。
pub const PROXY_SHADOW_RES_DIV: f32 = 1024.0;
/// 【EJ-2】chunk 1 件あたりのカリング調査 proxy 作業量 (列挙基数基準、
/// カリング「排除数」ではなく「対象総数」に比例させるのが真の仕事量契約)。
pub const PROXY_CULL_PER_CHUNK: f32 = 0.002;
/// 【EJ-2】点灯クラスタ 1 件あたりの clustered light 集約 proxy 作業量。
pub const PROXY_CLUSTER_PER_LIT: f32 = 0.01;
/// 【EJ-2】ピクセル 1 個あたりの post-FX chain proxy 作業量 (ワックス
/// パス群を画素単位に一括正規化した名目見積)。
pub const PROXY_POST_PER_PIXEL: f32 = 0.000001;

/// HUD 統計バーのレイヤー色。【EB-1 (2026-07-26 修正)】旧式は
/// `0xFF30_8040 + (i as u32) << 4` — Rust は `<<` より `+` が強く結合
/// するため **`(base + i) << 4`** と評価され、alpha が意図の 0xFF
/// (不透明) から 0xF3 へ化け、base 上位 nibble も捨てていた (rq 導出
/// 機械値: i=0 で 0xF3080400)。不透明意図 (base 定数に 0xFF alpha を
/// 据える書き方) に沿って `base + (i<<4)` に根治。ピン可能化のため
/// 純粋関数として抽出 (tick_world 呼び出し点は layer=i<4)。
fn hud_layer_color(layer: u16) -> u32 {
    0xFF30_8040 + ((layer as u32) << 4)
}

/// 全 WGSL を収集 (GPU ランタイム検証用・後方互換)。
pub fn collect_all_wgsl() -> String {
    let mut out = String::new();
    for (_name, src) in crate::gpu_runtime::all_wgsl_sources() {
        out.push_str(src);
        out.push('\n');
    }
    out
}

impl<'a> FrameWiringInputs<'a> {
    /// 実マテリアル集合から独立したハッシュ (PSO キャッシュキー成分)。
    fn material_independent_hash(&self) -> u32 {
        let mut h = 0x811C_9DC5u32;
        for m in &self.chunk_materials {
            h = (h ^ m).wrapping_mul(16_777_619);
        }
        h
    }
}

/// 旧バージョン互換: モジュール WGSL を列挙する静的 API。
impl FullGraphWiring {
    pub fn collect_all_wgsl() -> String {
        collect_all_wgsl()
    }
}

trait NormalizeWrap {
    fn normalize_wrap(self) -> Self;
}
impl NormalizeWrap for crate::atmospheric::Vec3 {
    fn normalize_wrap(self) -> Self {
        self.normalize()
    }
}

#[cfg(test)]
mod strict_tests {
    use super::*;

    fn empty_inputs() -> FrameWiringInputs<'static> {
        FrameWiringInputs {
            delta_ms: 16.0,
            frame_us_measured: 16_000,
            frame_index: 0,
            screen_w: 1920,
            screen_h: 1080,
            camera_pos: [0.0; 3],
            camera_dir: [0.0, 0.0, 1.0],
            view_proj: [[0.0; 4]; 4],
            chunk_keys: Vec::new(),
            chunk_materials: Vec::new(),
            chunk_aabbs: Vec::new(),
            chunk_dists: Vec::new(),
            draw_index_counts: Vec::new(),
            section_palettes: Vec::new(),
            quad_positions: Vec::new(),
            quad_materials: Vec::new(),
            quad_bytes: 0,
            camera_speed: 0.0,
            camera_fov_y: 1.0, // 本番既定 (from_camera と一致する安定ピン用)
            svo: None,
        }
    }

    #[test]
    fn out_sign_table_and_fallback() {
        // face: 0=+X 1=-X 2=+Y 3=-Y 4=+Z 5=-Z — 偶数=正方向
        let got: Vec<i32> = (0..6).map(out_sign).collect();
        assert_eq!(got, vec![1, -1, 1, -1, 1, -1]);
        for f in [6u32, 7, 255, u32::MAX] {
            assert_eq!(out_sign(f), -1, "未知 face は負方向扱い (フォールバック)");
        }
    }

    #[test]
    fn hud_layer_color_opaque_exact_goldens() {
        // EB-1: 優先順位根治後の厳密値 (rq eb_hud.rq 導出機械値)。
        // 旧式 `(base + i) << 4` は i=0 で 0xF3080400 (alpha 0xF3、base
        // 上位 nibble 欠落) — 旧式への回帰は本 pin で即 RED。
        let expect = [0xFF30_8040u32, 0xFF30_8050, 0xFF30_8060, 0xFF30_8070];
        for (i, e) in expect.iter().enumerate() {
            assert_eq!(
                hud_layer_color(i as u16),
                *e,
                "layer {i}: base + (i<<4) の bit 厳密値"
            );
        }
        for l in 0..16u16 {
            assert_eq!(
                hud_layer_color(l) >> 24,
                0xFF,
                "layer {l} の alpha は常時 0xFF (不透明)"
            );
        }
    }

    #[test]
    fn view_proj_to_m16_is_column_major_transpose() {
        let mut m = [[0.0f32; 4]; 4];
        for r in 0..4 {
            for c in 0..4 {
                m[r][c] = (r * 4 + c) as f32;
            }
        }
        let out = view_proj_to_m16(&m);
        assert_eq!(
            out,
            [
                0.0, 4.0, 8.0, 12.0, 1.0, 5.0, 9.0, 13.0, 2.0, 6.0, 10.0, 14.0, 3.0, 7.0, 11.0,
                15.0
            ],
            "out[c*4+r] = m[r][c] の列優先並べ替え"
        );
    }

    #[test]
    fn extract_frustum_planes_identity_table() {
        let id = [
            [1.0, 0.0, 0.0, 0.0f32],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let p = extract_frustum_planes(&id);
        // 恒等行列は対称 (行 i == 列 i) で新旧どちらの規約でも同一テーブル —
        // だからこそ本テストでは CG-1 転置を検出**できない**。検出器は
        // 下の production 系 2 テスト (非対称行列) が担う。
        assert_eq!(
            p[0],
            [1.0, 0.0, 0.0, 1.0],
            "left = c3+c0 (恒等では row 同値)"
        );
        assert_eq!(p[1], [-1.0, 0.0, 0.0, 1.0], "right = c3-c0");
        assert_eq!(p[2], [0.0, 1.0, 0.0, 1.0], "bottom = c1+c3");
        assert_eq!(p[3], [0.0, -1.0, 0.0, 1.0], "top = c3-c1");
        assert_eq!(p[4], [0.0, 0.0, 1.0, 0.0], "near = c2 (D3D 0..w 型)");
        assert_eq!(p[5], [0.0, 0.0, -1.0, 1.0], "far = c3-c2");
        // 再正規化チェック: 行を 5 倍しても単位法線化で同一テーブル
        let mut scaled = id;
        for r in scaled.iter_mut() {
            for x in r.iter_mut() {
                *x *= 5.0;
            }
        }
        assert_eq!(extract_frustum_planes(&scaled), p);
    }

    #[test]
    fn extract_frustum_planes_degenerate_no_nan() {
        let zero = [[0.0f32; 4]; 4];
        let p = extract_frustum_planes(&zero);
        assert_eq!(p, [[0.0f32; 4]; 6], "len.max(1e-6) ガードで NaN を出さない");
    }

    /// CG-1: 本番行列 (from_camera 生成、p x M 規約・非対称) に対する厳密
    /// 平面テーブル (f32 bits)。値は検算ミラー (struct 往復、列ベースの
    /// Rust 演算順逐語再現) 確定。旧行ベース規約では left が
    /// (-0.0156, -0.9999, ...) に化けていたため即座に検出できる。
    #[test]
    fn extract_frustum_planes_production_exact_table() {
        let cam = crate::hzb_2d::CameraState {
            x: 0.0,
            y: 64.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 1.0,
            aspect: 1.0,
        };
        let vp = crate::world_column_store::TerrainFrameConstants::from_camera(&cam, [0, 0, 0])
            .view_proj;
        let p = extract_frustum_planes(&vp);
        let expect: [[u32; 4]; 6] = [
            [0xBF60A941, 0x00000000, 0x3EF57745, 0x00000000], // left
            [0x3F60A941, 0x00000000, 0x3EF57745, 0x00000000], // right
            [0x00000000, 0x3F60A941, 0x3EF57745, 0xC260A941], // bottom
            [0x00000000, 0xBF60A941, 0x3EF57745, 0x4260A941], // top
            [0x00000000, 0x00000000, 0x3F800000, 0xBD4CCCCE], // near (D3D 0..w)
            [0x00000000, 0x00000000, 0xBF800000, 0x44000B34], // far
        ];
        for (i, e) in expect.iter().enumerate() {
            let got = [
                p[i][0].to_bits(),
                p[i][1].to_bits(),
                p[i][2].to_bits(),
                p[i][3].to_bits(),
            ];
            assert_eq!(&got, e, "plane {i} bits (列ベース Gribb-Hartmann 厳密値)");
        }
        // 鏡像対称性 (本カメラでは解析的に成立: left/right は x 反転のみ、
        // bottom/top は y,d 反転のみ) も構造的にピン。
        assert_eq!(p[0][0], -p[1][0], "left/right x 鏡像");
        assert_eq!(p[0][2], p[1][2], "left/right z 一致");
        assert_eq!(p[2][3], -p[3][3], "bottom/top d 鏡像");
        assert_eq!(p[2][1], -p[3][1], "bottom/top y 鏡像");
    }

    /// CG-1: 抽出平面の内外判定と厳密射影 (p x M) の内外判定の全点一致。
    /// 検算ミラーで 14 点 (yaw=0 12 点 + 回転カメラ 2 点) 全一致を確認済。
    /// 旧行ベース規約では yaw=0 の正面点 (0,64,16) すら 4 面で dist<0 に
    /// なる全棄却だった (検算距離実測: [-64.0, -64.0, -64.0, -64.0, +17.0,
    /// -64.1])。
    #[test]
    fn extract_frustum_planes_matches_projection_semantics() {
        let proj4 = |p: [f32; 3], vp: &[[f32; 4]; 4]| -> [f32; 4] {
            let mut o = [0.0f32; 4];
            for (j, oj) in o.iter_mut().enumerate() {
                *oj = vp[0][j] * p[0] + vp[1][j] * p[1] + vp[2][j] * p[2] + vp[3][j];
            }
            o
        };
        let frustum_inside = |p: [f32; 3], vp: &[[f32; 4]; 4]| -> bool {
            let c = proj4(p, vp);
            c[3] > 0.0
                && c[0] >= -c[3]
                && c[0] <= c[3]
                && c[1] >= -c[3]
                && c[1] <= c[3]
                && c[2] >= 0.0
                && c[2] <= c[3]
        };
        let planes_inside = |p: [f32; 3], planes: &[[f32; 4]; 6]| -> bool {
            planes
                .iter()
                .all(|pl| pl[0] * p[0] + pl[1] * p[1] + pl[2] * p[2] + pl[3] >= 0.0)
        };
        let cam = crate::hzb_2d::CameraState {
            x: 0.0,
            y: 64.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 1.0,
            aspect: 1.0,
        };
        let vp = crate::world_column_store::TerrainFrameConstants::from_camera(&cam, [0, 0, 0])
            .view_proj;
        let planes = extract_frustum_planes(&vp);
        let cases: [([f32; 3], bool); 12] = [
            ([0.0, 64.0, 16.0], true),
            ([8.0, 68.0, 24.0], true),
            ([0.0, 64.0, 0.06], true),  // near 境界のすぐ内側 (0.05 超)
            ([-8.0, 60.0, 8.0], false), // |ndc_x|=1.83 で視錐台外 (画面に見えない)
            ([40.0, 70.0, 60.0], false),
            ([0.0, 64.0, -10.0], false), // 背面
            ([2000.0, 64.0, 16.0], false),
            ([0.0, 400.0, 16.0], false),
            ([0.0, 64.0, 600.0], false), // far 超過
            ([300.0, 64.0, 100.0], false),
            ([0.0, -200.0, 50.0], false),
            ([1.5, 63.5, 1.0], false),
        ];
        for (pt, expect) in cases {
            let fr = frustum_inside(pt, &vp);
            assert_eq!(fr, expect, "射影の内外 (前提の検算照合) {pt:?}");
            assert_eq!(
                planes_inside(pt, &planes),
                fr,
                "抽出平面の内外は厳密射影と一致 {pt:?}"
            );
        }
        // 回転カメラ (yaw=0.7, pitch=0.15、転置規約との乖離が最大の形)。
        // 前方点の構成は from_camera の fw と同一演算子列 (sin_cos ペア)。
        let (sy, cy) = 0.7f32.sin_cos();
        let (sp, cp) = 0.15f32.sin_cos();
        let fw = [-sy * cp, -sp, cy * cp];
        let cam2 = crate::hzb_2d::CameraState {
            x: 10.0,
            y: 70.0,
            z: -5.0,
            yaw: 0.7,
            pitch: 0.15,
            fov_y: 1.0,
            aspect: 1.0,
        };
        let vp2 = crate::world_column_store::TerrainFrameConstants::from_camera(&cam2, [0, 0, 0])
            .view_proj;
        let planes2 = extract_frustum_planes(&vp2);
        let fwd = [
            10.0 + fw[0] * 20.0,
            70.0 + fw[1] * 20.0,
            -5.0 + fw[2] * 20.0,
        ];
        let back = [
            10.0 - fw[0] * 20.0,
            70.0 - fw[1] * 20.0,
            -5.0 - fw[2] * 20.0,
        ];
        for (pt, expect) in [(fwd, true), (back, false)] {
            let fr = frustum_inside(pt, &vp2);
            assert_eq!(fr, expect, "回転カメラ射影の内外 {pt:?}");
            assert_eq!(
                planes_inside(pt, &planes2),
                fr,
                "回転カメラ抽出平面の一致 {pt:?}"
            );
        }
    }

    #[test]
    fn mean_sigma_empty_fallback_and_exact_pairs() {
        let (mu, sig) = mean_sigma(&[]);
        assert_eq!(
            (mu.x, mu.y, mu.z),
            (0.5, 0.0, 0.0),
            "空入力の既定 mean (仕様固定)"
        );
        assert_eq!((sig.x, sig.y, sig.z), (0.1, 0.1, 0.1), "空入力の既定 sigma");
        let one = [crate::taa_ycocg::Vec3::new(2.0, 4.0, 8.0)];
        let (mu, sig) = mean_sigma(&one);
        assert_eq!((mu.x, mu.y, mu.z), (2.0, 4.0, 8.0));
        assert_eq!(
            (sig.x, sig.y, sig.z),
            (0.0, 0.0, 0.0),
            "単一要素の sigma は 0"
        );
        let two = [
            crate::taa_ycocg::Vec3::new(1.0, 2.0, 3.0),
            crate::taa_ycocg::Vec3::new(3.0, 2.0, 1.0),
        ];
        let (mu, sig) = mean_sigma(&two);
        assert_eq!((mu.x, mu.y, mu.z), (2.0, 2.0, 2.0));
        assert_eq!(
            (sig.x, sig.y, sig.z),
            (1.0, 0.0, 1.0),
            "population sigma (1/n 分散)"
        );
    }

    #[test]
    fn material_independent_hash_goldens() {
        let mut i = empty_inputs();
        assert_eq!(
            i.material_independent_hash(),
            0x811C_9DC5,
            "空は FNV 種値そのまま"
        );
        i.chunk_materials = vec![0];
        assert_eq!(i.material_independent_hash(), 0x050C_5D1F);
        i.chunk_materials = vec![1];
        assert_eq!(i.material_independent_hash(), 0x040C_5B8C);
        i.chunk_materials = vec![1, 2, 3];
        assert_eq!(i.material_independent_hash(), 0x56CF_37AB);
        i.chunk_materials = vec![7, 0];
        assert_eq!(i.material_independent_hash(), 0x9F6F_2892);
        // 順序に依存すること (xor 畳み込みではない)
        let mut j = empty_inputs();
        j.chunk_materials = vec![1, 2];
        let mut k = empty_inputs();
        k.chunk_materials = vec![2, 1];
        assert_ne!(j.material_independent_hash(), k.material_independent_hash());
    }

    #[test]
    fn corner_ao_axes_brightness_and_occlusion() {
        let lut = crate::branchless_block::BlockLut::new();
        // id 1 は opaque (i%3!=0 プレースホルダ — branchless_block 監査注記参照)
        let air = [0u16; 4096];
        // --- ケース A: 近傍なし → 3 (無遮蔽で最も明るい) ---
        assert_eq!(
            corner_ao_from_palette(std::slice::from_ref(&air), 4, 4, 4, 2, &lut),
            3,
            "全 air 近傍は 3 (=無遮蔽)"
        );
        // --- s1 のみ → 2 (1 段減光) / 両側 → 0 (完全遮蔽) ---
        let mut sec_b = air;
        sec_b[section_idx(3, 5, 4)] = 1; // face 2(+Y) @ (4,4,4): s1 = (3,5,4)
        assert_eq!(corner_ao_from_palette(&[sec_b], 4, 4, 4, 2, &lut), 2);
        let mut sec_c = sec_b;
        sec_c[section_idx(5, 5, 4)] = 1; // s2
        assert_eq!(
            corner_ao_from_palette(&[sec_c], 4, 4, 4, 2, &lut),
            0,
            "両側隣接は corner_ao が 0 (最大遮蔽) を返す規約"
        );
        // --- corner のみ → 2 ---
        let mut sec_d = air;
        sec_d[section_idx(5, 5, 5)] = 1; // face 2 の corner = (x+1, y+1, z+1)
        assert_eq!(corner_ao_from_palette(&[sec_d], 4, 4, 4, 2, &lut), 2);
        // --- face 0 (+X): s1 = (x+1, y-1, z) ---
        let mut sec_e = air;
        sec_e[section_idx(5, 3, 4)] = 1;
        assert_eq!(corner_ao_from_palette(&[sec_e], 4, 4, 4, 0, &lut), 2);
        // --- face 5 (-Z): s1 = (x-1, y, z-1) ---
        let mut sec_f = air;
        sec_f[section_idx(3, 4, 3)] = 1;
        assert_eq!(corner_ao_from_palette(&[sec_f], 4, 4, 4, 5, &lut), 2);
        // --- OOB は非不透明: face 3 (-Y) を y=0 で呼んでも ny=-1 → 3 ---
        assert_eq!(corner_ao_from_palette(&[sec_c], 4, 0, 4, 3, &lut), 3);
        // --- multisection: y=16 で 2 枚目セクションを参照 ---
        // face 2 (+Y) @ (4,15,4): s1 = (3,16,4) → sections[1][idx(3,0,4)]
        let mut sec_h = [air, air];
        sec_h[1][section_idx(3, 0, 4)] = 1;
        assert_eq!(
            corner_ao_from_palette(&sec_h, 4, 15, 4, 2, &lut),
            2,
            "y>=16 は sy=ny/16 のセクションを参照 (1 枚構成)"
        );
    }

    #[test]
    fn collect_all_wgsl_matches_sources_and_assoc_delegate() {
        let expect: String = crate::gpu_runtime::all_wgsl_sources()
            .into_iter()
            .map(|(_, src)| format!("{src}\n"))
            .collect();
        let free = collect_all_wgsl();
        assert_eq!(free, expect, "全ソースを順序通りに連結 (欠落/改竄を検出)");
        assert_eq!(
            FullGraphWiring::collect_all_wgsl(),
            expect,
            "assoc は free への純粋委譲"
        );
        assert!(
            !expect.is_empty(),
            "gpu_runtime に少なくとも 1 ソースが登録"
        );
    }

    #[test]
    fn ao_refine_quads_rewrites_and_counts() {
        // sections 空: early return で 0、クアッド不変
        let dir = std::env::temp_dir().join(format!(
            "rsift_fgw_ao_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut w = FullGraphWiring::new(&dir);
        let mut quads = vec![PackedPullQuad::new(4, 4, 4, 7, 0, 2, 3, 2)];
        let q0 = quads[0];
        assert_eq!(w.ao_refine_quads(&mut quads, &[]), 0, "sections 空は no-op");
        assert_eq!(quads[0], q0, "bits 不変");

        // 1 セクション: face 2 (+Y) @ (4,4,4)、隣接 s1 のみ → corner_ao=2
        let mut sec = [0u16; 4096];
        sec[section_idx(3, 5, 4)] = 1; // s1
        let sections = [sec];
        let mut quads = vec![
            PackedPullQuad::new(4, 4, 4, 7, 0, 2, 3, 2), // refined = max(2,0)=2 → 変更
            PackedPullQuad::new(4, 4, 4, 7, 3, 2, 3, 2), // refined = max(2,3)=3 → 不変
            PackedPullQuad::new(8, 8, 8, 9, 1, 2, 1, 1), // 近傍なし: max(3,1)=3 → 変更
            PackedPullQuad::new(8, 8, 8, 9, 3, 2, 1, 1), // max(3,3)=3 → 不変
        ];
        let changed = w.ao_refine_quads(&mut quads, &sections);
        assert_eq!(changed, 2, "変更クアッド数 = 実視覚効果の実測");
        assert_eq!(PackedPullQuad::unpack_light_ao(quads[0].word0), 2);
        assert_eq!(
            PackedPullQuad::new(4, 4, 4, 7, 2, 2, 3, 2),
            quads[0],
            "AO 以外の packed フィールド (tex/face/w/h) は厳密保存"
        );
        assert_eq!(
            PackedPullQuad::unpack_light_ao(quads[1].word0),
            3,
            "max() 取れる側は不変"
        );
        assert_eq!(PackedPullQuad::unpack_light_ao(quads[2].word0), 3);
        assert_eq!(PackedPullQuad::unpack_light_ao(quads[3].word0), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // =====================================================================
    // 第 9 波: tick_world 統合テスト (監査 2026-07-22 に基づく決定性仕様)
    // =====================================================================

    /// 【wave 160 FF 捕捉 87 §7 消化 22】DDGI probe 座標の旧 `let _ = probe`
    /// 破棄を report 実フィールド pin へ根治したことの非ゼロ工程化 pin:
    /// 全ゼロ帳簿 vacuous 化 (wave 157 FC の教訓) を避けるため camera_pos は
    /// 非自明値。cell は全て 2 の冪で除算厳密 (rq ff_ddgi (4) + 実機 probe:
    /// 1.5=0x3fc00000/1.0=0x3f800000/3.0=0x40400000/-2.5=0xc0200000)。
    /// tick 2 回で pin 不変 (体積設定は静的契約) も併せて検証。
    #[test]
    fn ff_ddgi_probe_report_pins_nonvacuous() {
        let (dir, mut w) = unique_wiring("ddgi_probe_pins");
        let mut inp = empty_inputs();
        inp.camera_pos = [24.0, 8.0, 48.0];
        let r1 = w.tick_world(&inp);
        assert_eq!(
            r1.ddgi_probe_coord.map(f32::to_bits),
            [0x3fc00000, 0x3f800000, 0x40400000],
            "camera (24,8,48) -> probe coord (1.5,1.0,3.0) exact bits"
        );
        assert_eq!(r1.ddgi_probe_count, 1024, "dims 16*4*16");
        inp.camera_pos = [-40.0, -8.0, -16.0];
        let r2 = w.tick_world(&inp);
        assert_eq!(
            r2.ddgi_probe_coord.map(f32::to_bits),
            [0xc0200000, 0xbf800000, 0xbf800000],
            "camera (-40,-8,-16) -> (-2.5,-1.0,-1.0) exact bits"
        );
        assert_eq!(r2.ddgi_probe_count, 1024);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 【wave 175 FU 捕捉 115】BumpArena used() 消費者ゼロ → report.bump_used
    /// 真配線 pin (rq fu_bump): morton codes 実確保の used = max(k,1)*8。
    /// tick1: keys=5 → 40・tick2: keys=3 → reset 後 24 (u64 align 8)。
    #[test]
    fn fu_bump_used_truth() {
        let (dir, mut w) = unique_wiring("bump_used_pins");
        let mut i1 = empty_inputs();
        i1.chunk_keys = vec![(0, 0), (1, 1), (2, 2), (3, 3), (4, 4)];
        let r1 = w.tick_world(&i1);
        assert_eq!(r1.bump_used, 40, "keys=5 → 5*8 (rq)");
        let mut i2 = empty_inputs();
        i2.chunk_keys = vec![(0, 0), (1, 1), (5, 5)];
        let r2 = w.tick_world(&i2);
        assert_eq!(r2.bump_used, 24, "reset 後 keys=3 → 3*8 (rq)");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 【wave 181 GA 捕捉 126 §7 消化 43】tick_world DAG critical path の
    /// report.dag_critical_ms 真配線 pin (旧実装は計算結果を `let critical_ms`
    /// で破棄、消費者ゼロ = FQ/FT/FU 判例の report 化)。
    /// 5 タスク実構築 (ingest/mesh/cull/upload/light、係数 0.05/0.4/0.1/0.2/
    /// 0.1 × delta_ms)、critical chain = ingest→mesh→upload = 0.65*delta_ms
    /// = 0.65*16.0 → f32 10.400001 bits 0x41266667 (/tmp/ga_probe.rs・
    /// rq /tmp/ga_dag.rq 機械導出)。
    #[test]
    fn ga_dag_critical_report_truth() {
        let (dir, mut w) = unique_wiring("dag_crit_pins");
        let i = empty_inputs();
        let r = w.tick_world(&i);
        assert_eq!(
            r.dag_critical_ms.to_bits(),
            0x41266667,
            "critical = 0.65*16.0 = 10.400001 (f32 probe/rq 機械値)"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 【wave 174 FT 捕捉 114】InstancedCollector 実計測 pin (rq ft_instanced):
    /// mats=[1,1,2,2,3] → mesh_id=mat.min(63) で groups=3、total=5、
    /// bytes=Σ全グループ総バイト=5*32=160。旧 `_instanced_stats` 破棄
    /// (doc「帯域見積に反映」虚偽) から report 3 実フィールド真配線。
    #[test]
    fn ft_instanced_report_truth() {
        let (dir, mut w) = unique_wiring("instanced_pins");
        let mut i1 = empty_inputs();
        i1.quad_positions = vec![
            [0.0, 64.0, 0.0],
            [4.0, 64.0, 0.0],
            [8.0, 70.0, 8.0],
            [12.0, 70.0, 8.0],
            [16.0, 70.0, 8.0],
        ];
        i1.quad_materials = vec![1, 1, 2, 2, 3];
        let r1 = w.tick_world(&i1);
        assert_eq!(r1.instanced_total, 5);
        assert_eq!(r1.instanced_groups, 3);
        assert_eq!(r1.instanced_bytes, 160, "Σ 全グループ 5*32 (rq)");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 【wave 171 FQ 捕捉 109】DashMap registry 真 lifecycle pin (rq fq_registry):
    /// 初見 → Building → (slab alloc 直後) Done、退去 → remove。tick1: 初見
    /// 2 件で version=4 (set 2 回/件)、done=2、building=0 (即時遷移の truth)。
    /// tick2: (0,0) 退去 remove +1、(2,2) 初見 set +2 → version=7、total=2。
    /// pending は非同期投入キュー未導入で到達不能（常 0 pin）。report 4 実
    /// フィールドの非ゼロ工程化 pin 兼ねる (version/done は非ゼロ変動)。
    #[test]
    fn fq_registry_lifecycle_truth() {
        let (dir, mut w) = unique_wiring("registry_lifecycle");
        let mut i1 = empty_inputs();
        i1.chunk_keys = vec![(0, 0), (1, 1)];
        let r1 = w.tick_world(&i1);
        assert_eq!(
            r1.registry_version, 4,
            "初見 2 件 × set 2 回 (Building+Done)"
        );
        assert_eq!(r1.registry_done, 2);
        assert_eq!(r1.registry_building, 0, "Building は tick 内即時 Done 遷移");
        assert_eq!(r1.registry_pending, 0, "非同期キュー未導入で到達不能");
        assert_eq!(r1.registry_total, 2);
        let mut i2 = empty_inputs();
        i2.chunk_keys = vec![(1, 1), (2, 2)];
        let r2 = w.tick_world(&i2);
        assert_eq!(r2.registry_version, 7, "(0,0) 退去 +1、(2,2) 初見 +2");
        assert_eq!(r2.registry_total, 2);
        assert_eq!(r2.registry_done, 2);
        assert!(r2.registry_version > r1.registry_version, "単調増加");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 【wave 161 FG 捕捉 91 §7 消化 23】Pacing 帳簿の非ゼロ工程化 pin:
    /// 60Hz (interval 50/3ms) + delta 16ms x2 tick。rq fg_pacing (1)(2) の
    /// 分数厳密値 (s1=248/15、s2=1232/75、t1=50/3、t2=100/3) を f64 実機
    /// probe bits で固定。smoothed は 16 でも interval でもない (nonvacuous)。
    #[test]
    fn fg_pacing_report_pins_nonvacuous() {
        let (dir, mut w) = unique_wiring("pacing_pins");
        let inp = empty_inputs(); // delta_ms = 16.0
        let r1 = w.tick_world(&inp);
        assert_eq!(
            r1.frame_smoothed_ms.to_bits(),
            0x4030888888888889,
            "s1 = 248/15 (probe bits)"
        );
        assert_eq!(
            r1.frame_next_present_ms.to_bits(),
            0x4030aaaaaaaaaaab,
            "t1 = 50/3 (probe bits)"
        );
        assert_eq!(
            r1.frame_target_hz.to_bits(),
            0x404e000000000000,
            "60.0 (probe bits)"
        );
        let r2 = w.tick_world(&inp);
        assert_eq!(
            r2.frame_smoothed_ms.to_bits(),
            0x40306d3a06d3a06e,
            "s2 = 1232/75 (probe bits)"
        );
        assert_eq!(
            r2.frame_next_present_ms.to_bits(),
            0x4040aaaaaaaaaaab,
            "t2 = 100/3 (probe bits)"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 【wave 163 FI 捕捉 94 §7 消化 25】PsoLibrary::stats() の消費者ゼロを
    /// report 実配線したことの非ゼロ工程化 pin: 同一 inputs (material hash
    /// 不変) で tick 系列は (misses=1, hits=N-1) — rq fi_pso (1)。CG-7 で
    /// 真のキャッシュ化した計器の読出し側が閉じていなかったことの根治で、
    /// hit 経路が必ず発火する (vacuous 回避)。
    #[test]
    fn fi_pso_stats_report_pins_nonvacuous() {
        let (dir, mut w) = unique_wiring("pso_stats");
        let inp = empty_inputs();
        let r1 = w.tick_world(&inp);
        assert_eq!(r1.pso_lib_misses, 1, "初 tick は get=miss + insert");
        assert_eq!(r1.pso_lib_hits, 0);
        let r2 = w.tick_world(&inp);
        assert_eq!(
            r2.pso_lib_misses, 1,
            "material hash 不変 → miss は 1 のまま"
        );
        assert_eq!(r2.pso_lib_hits, 1, "2 tick 目以降 hit 単調 (hits=N-1)");
        let r3 = w.tick_world(&inp);
        assert_eq!(r3.pso_lib_hits, 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 【wave 165 FK 捕捉 98 §7 消化 27】decal_local 評価の `let _` 破棄を
    /// report 実フィールドへ閉じたことの非ゼロ工程化 pin。EB-3 公知: 本
    /// クレートに登録 API は存在しないため wiring フィールドへの直接 push
    /// (テスト特権) で工程化する。原点箱 (カメラ内) + 遠方箱 (100,0,0) で
    /// 投影成立は 1/2 (rq fk_decals (3))。
    #[test]
    fn fk_decals_projected_report_pin_nonvacuous() {
        let (dir, mut w) = unique_wiring("decals_pin");
        let box_at = |x: f32| crate::decals::Decal {
            center: crate::decals::Vec3::new(x, 0.0, 0.0),
            right: crate::decals::Vec3::new(1.0, 0.0, 0.0),
            up: crate::decals::Vec3::new(0.0, 1.0, 0.0),
            forward: crate::decals::Vec3::new(0.0, 0.0, 1.0),
            half: crate::decals::Vec3::new(2.0, 2.0, 2.0),
        };
        w.decals.push(box_at(0.0));
        w.decals.push(box_at(100.0));
        let inp = empty_inputs(); // camera [0,0,0]
        let r = w.tick_world(&inp);
        assert_eq!(
            r.decals_projected, 1,
            "カメラ原点箱のみ投影成立 (非ゼロ工程化、far は None)"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 【wave 168 FN 捕捉 105】§7 消化 30: tick の `let _ = self.root_cost;`
    /// 破棄を report.root_cost_dwords 実配線へ根治。rs_graphics() は定数生成の
    /// ため 19 固定 pin (D3D12 64 DWORD 制限内である不変式を実行時 telemetry 化)。
    #[test]
    fn fn_root_cost_report_field_pin() {
        let (_dir, mut w) = unique_wiring("fn_root_cost");
        let inp = empty_inputs();
        let r = w.tick_world(&inp);
        assert_eq!(
            r.root_cost_dwords, 19,
            "16 (Camera RootConstants) + 1 (bindless Table) + 2 (Material RootDescriptor)"
        );
        assert!(r.root_cost_dwords <= 64, "D3D12 64 DWORD 制限内");
    }

    /// 【wave 169 FO 捕捉 106】§7 消化 31: 旧 `let _bundle_stats = ...;` 破棄を
    /// report.bundle_hits/bundle_misses 実配線へ根治。8 キー固定供給で初回 8 miss、
    /// 2 回目 tick で 8 hit 累積 (BundleCache 帳簿は wiring 単位累積) の非ゼロ両側 pin。
    #[test]
    fn fo_bundle_report_pins_nonvacuous() {
        let (_dir, mut w) = unique_wiring("fo_bundle");
        let mut inp = empty_inputs();
        inp.chunk_keys = (0..8).map(|i| (i, i * 7)).collect();
        inp.draw_index_counts = (0..8).map(|i| 36 + i as u32).collect();
        let r1 = w.tick_world(&inp);
        assert_eq!(r1.bundle_misses, 8, "初回 8 キー全 miss");
        assert_eq!(r1.bundle_hits, 0, "初回 hit 0");
        let r2 = w.tick_world(&inp);
        assert_eq!(r2.bundle_hits, 8, "2 回目は 8 キー全 hit (累積 8)");
        assert_eq!(r2.bundle_misses, 8, "miss 累積は 8 のまま");
    }

    /// 【wave 166 FL 捕捉 101 §7 消化 28】`_bricks` 破棄を report 実配線
    /// したことの非ゼロ工程化 pin: palette ありで tick 毎に (0,0,0,0) が
    /// request→process され processed=1・resident=1 定常 (rq fl_giga (3))。
    /// palette なしでは両者 0 (分岐両側を pin)。
    /// (wave 168: fn テスト挿入が一時的に本 fn の #[test] を剥奪 — wave 167
    /// 同型事故の再発。同 wave 内自己照査で捕捉・修復、誠実記録)
    #[test]
    fn fl_gigavoxels_report_pins_nonvacuous() {
        let (dir, mut w) = unique_wiring("giga_stats");
        let mut inp = empty_inputs();
        let mut palette: crate::binary_greedy_meshing::SectionPalette = [0u16; 4096];
        palette[0] = 7;
        inp.section_palettes = vec![palette];
        let r1 = w.tick_world(&inp);
        assert_eq!(r1.gigavoxels_processed, 1, "palette あり: 1 件処理");
        assert_eq!(r1.gigavoxels_resident, 1, "brick 常駐 1");
        let r2 = w.tick_world(&inp);
        assert_eq!(
            r2.gigavoxels_processed, 1,
            "tick 毎に再生成 (動的 voxel 経路)"
        );
        assert_eq!(
            r2.gigavoxels_resident, 1,
            "pool は (0,0,0,0) の 1 brick 定常"
        );
        let (dir2, mut w2) = unique_wiring("giga_stats_empty");
        let inp2 = empty_inputs(); // palette なし
        let r3 = w2.tick_world(&inp2);
        assert_eq!(r3.gigavoxels_processed, 0, "palette なし: 処理 0");
        assert_eq!(r3.gigavoxels_resident, 0);
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(dir2);
    }

    fn unique_wiring(tag: &str) -> (std::path::PathBuf, FullGraphWiring) {
        let dir = std::env::temp_dir().join(format!(
            "rsift_fgw_tick_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (dir.clone(), FullGraphWiring::new(&dir))
    }

    /// 【wave 159 FE 捕捉 83 §7 消化 21 + bootstrap 根治】fsr3 報告の実配線:
    /// (a) fsr3_*_bytes は計算→破棄 (`let _ =`) だったバッファ 3 定数、
    /// (b) fsr3_mean_delta は `let _fsr3_mean_delta` 捨ての検証メトリクス。
    /// f1 (prev 不在) は out := curr serve で Δ=0.0、f2 以降は実補間 Δ。
    #[test]
    fn fsr3_bootstrap_serve_and_report_wired() {
        let (dir_a, mut a) = unique_wiring("fe_fsr3_a");
        let (dir_b, mut b) = unique_wiring("fe_fsr3_b");
        let mut inputs = empty_inputs();
        // speed=40 では dome 量子化で補間差分が厳密 0 となる (実機 probe
        // 4 点 (40/200/500/2000) → (0.0, 0.08203125, 0.3203125, 1.2617188)
        // で初回 40 前提の非ゼロ予想を自己捕捉訂正) — 500 を採用。
        inputs.camera_speed = 500.0;
        let r1a = a.tick_world(&inputs);
        let r1b = b.tick_world(&inputs);
        assert_eq!(
            r1a.fsr3_mean_delta.to_bits(),
            0.0f32.to_bits(),
            "f1 bootstrap: out:=curr serve で Δ=0 (旧実装は全ゼロ out から測る擬似 Δ)"
        );
        assert_eq!(
            (
                r1a.fsr3_color_bytes,
                r1a.fsr3_depth_bytes,
                r1a.fsr3_motion_bytes
            ),
            (8_294_400, 8_294_400, 16_588_800),
            "1920x1080 バッファ 3 定数 (rq fe_fsr3 (4))"
        );
        let r2a = a.tick_world(&inputs);
        let r2b = b.tick_world(&inputs);
        assert!(
            r2a.fsr3_mean_delta > 0.0,
            "f2: camera_speed=500 の motion warp で補間差分は非ゼロ"
        );
        assert_eq!(
            r2a.fsr3_mean_delta.to_bits(),
            0x3EA4_0000u32,
            "f2: Δ 実機 probe golden 0.3203125 (waves の実測値 pin、rq 系ではなく probe 由来であることを誠実明記)"
        );
        assert_report_det_subset(&r1a, &r1b, "fsr3 f1");
        assert_report_det_subset(&r2a, &r2b, "fsr3 f2");
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    /// 監査 2026-07-22 K-4/K-5 の決定性比較集合: FrameWiringReport の pub
    /// フィールドのうち、非決定と doc 明記されたもの — vanilla_hook_hits_delta
    /// (プロセス全域カウンタ由来)・power_skip_extra (壁時計由来 mode/gate
    /// 経路)・【wave 154 EZ-1 追加】power_frame_dt/power_mode (同じく wall
    /// 由来 mode 経路) — を除く全決定フィールドを、f32 は IEEE-754 ビット
    /// 同一として全比較する (field 数の絶対記述は wave 毎に増えるため
    /// 「群」で規定、wave 151 時点の「全 17/15」記述を誠実訂正)。
    fn assert_report_det_subset(a: &FrameWiringReport, b: &FrameWiringReport, ctx: &str) {
        assert_eq!(
            a.next_build_budget, b.next_build_budget,
            "{ctx}: next_build_budget"
        );
        assert_eq!(a.overdraw_order, b.overdraw_order, "{ctx}: overdraw_order");
        assert_eq!(
            a.draw_command_count, b.draw_command_count,
            "{ctx}: draw_command_count"
        );
        assert_eq!(
            a.lockfree_cache_hits, b.lockfree_cache_hits,
            "{ctx}: lockfree_cache_hits"
        );
        assert_eq!(a.lbvh_culled, b.lbvh_culled, "{ctx}: lbvh_culled");
        assert_eq!(
            a.cluster_max_load, b.cluster_max_load,
            "{ctx}: cluster_max_load"
        );
        assert_eq!(
            a.cluster_lit_clusters, b.cluster_lit_clusters,
            "{ctx}: cluster_lit_clusters"
        );
        assert_eq!(
            a.gtao_occ.to_bits(),
            b.gtao_occ.to_bits(),
            "{ctx}: gtao_occ (bit)"
        );
        assert_eq!(
            a.ao_halfres_mean.to_bits(),
            b.ao_halfres_mean.to_bits(),
            "{ctx}: ao_halfres_mean (bit)"
        );
        assert_eq!(
            a.ao_halfres_min.to_bits(),
            b.ao_halfres_min.to_bits(),
            "{ctx}: ao_halfres_min (bit)"
        );
        assert_eq!(
            a.async_overlap_proxy_ms.to_bits(),
            b.async_overlap_proxy_ms.to_bits(),
            "{ctx}: async_overlap_proxy_ms (bit)"
        );
        assert_eq!(
            a.async_pipelined_proxy_ms.to_bits(),
            b.async_pipelined_proxy_ms.to_bits(),
            "{ctx}: async_pipelined_proxy_ms (bit)"
        );
        assert_eq!(
            a.async_saved_pct.to_bits(),
            b.async_saved_pct.to_bits(),
            "{ctx}: async_saved_pct (bit)"
        );
        assert_eq!(a.leo_tag_dist, b.leo_tag_dist, "{ctx}: leo_tag_dist");
        assert_eq!(
            a.leo_latest_tick, b.leo_latest_tick,
            "{ctx}: leo_latest_tick"
        );
        assert_eq!(
            a.emissive_high_mask, b.emissive_high_mask,
            "{ctx}: emissive_high_mask"
        );
        assert_eq!(
            a.subgroup_wave_sum_max.to_bits(),
            b.subgroup_wave_sum_max.to_bits(),
            "{ctx}: subgroup_wave_sum_max (bit)"
        );
        assert_eq!(
            a.shadow_caster_lod_dist, b.shadow_caster_lod_dist,
            "{ctx}: shadow_caster_lod_dist"
        );
        assert_eq!(
            a.shadow_casters_culled, b.shadow_casters_culled,
            "{ctx}: shadow_casters_culled"
        );
        assert_eq!(
            a.foveated_center_rate.to_bits(),
            b.foveated_center_rate.to_bits(),
            "{ctx}: foveated_center_rate (bit)"
        );
        assert_eq!(
            a.tdl_max_tile_load, b.tdl_max_tile_load,
            "{ctx}: tdl_max_tile_load"
        );
        assert_eq!(a.tdl_lit_tiles, b.tdl_lit_tiles, "{ctx}: tdl_lit_tiles");
        assert_eq!(
            a.depth_pass_count, b.depth_pass_count,
            "{ctx}: depth_pass_count"
        );
        assert_eq!(
            a.depth_pass_cost, b.depth_pass_cost,
            "{ctx}: depth_pass_cost"
        );
        assert_eq!(
            a.translucent_total, b.translucent_total,
            "{ctx}: translucent_total"
        );
        assert_eq!(
            a.translucent_back_first, b.translucent_back_first,
            "{ctx}: translucent_back_first"
        );
        // EU-2: POM 実消費フィールドの cross-instance 決定性 pin。
        assert_eq!(
            a.parallax_layer_depth.to_bits(),
            b.parallax_layer_depth.to_bits(),
            "{ctx}: parallax_layer_depth"
        );
        assert_eq!(
            a.parallax_uv_offset_y.to_bits(),
            b.parallax_uv_offset_y.to_bits(),
            "{ctx}: parallax_uv_offset_y"
        );
        // GC-2: GUI デュアルレート実消費フィールド (全値 inputs のみ由来の
        // 完全決定的機構、GC-5 注記 2 — wall-clock 非依存)。
        assert_eq!(
            a.gui_rendered_surface, b.gui_rendered_surface,
            "{ctx}: gui_rendered_surface"
        );
        assert_eq!(
            a.gui_target_fps.to_bits(),
            b.gui_target_fps.to_bits(),
            "{ctx}: gui_target_fps"
        );
        assert_eq!(
            a.gui_invalidated, b.gui_invalidated,
            "{ctx}: gui_invalidated"
        );
        assert_eq!(
            a.gui_reuse_cached_scene, b.gui_reuse_cached_scene,
            "{ctx}: gui_reuse_cached_scene"
        );
        assert_eq!(
            a.gui_surface_valid, b.gui_surface_valid,
            "{ctx}: gui_surface_valid"
        );
        assert_eq!(
            a.aokana_visible_regions, b.aokana_visible_regions,
            "{ctx}: aokana_visible_regions"
        );
        assert_eq!(
            a.visgraph_reachable, b.visgraph_reachable,
            "{ctx}: visgraph_reachable"
        );
        assert_eq!(
            a.nanite_meshlets, b.nanite_meshlets,
            "{ctx}: nanite_meshlets"
        );
        assert_eq!(
            a.nanite_meshlets_culled, b.nanite_meshlets_culled,
            "{ctx}: nanite_meshlets_culled"
        );
        assert_eq!(
            a.vram_used_bytes, b.vram_used_bytes,
            "{ctx}: vram_used_bytes"
        );
        assert_eq!(
            a.post_exposure.to_bits(),
            b.post_exposure.to_bits(),
            "{ctx}: post_exposure (bit)"
        );
        assert_eq!(
            a.ambient_light.to_bits(),
            b.ambient_light.to_bits(),
            "{ctx}: ambient_light (bit)"
        );
        assert_eq!(
            a.clp_lit_fraction.to_bits(),
            b.clp_lit_fraction.to_bits(),
            "{ctx}: clp_lit_fraction (bit)"
        );
        assert_eq!(a.frb_billboards, b.frb_billboards, "{ctx}: frb_billboards");
        // EY-2 (wave 153): fsr2 実配線の det pin (f32 は bit 比較)
        assert_eq!(
            a.fsr2_reproj_uv[0].to_bits(),
            b.fsr2_reproj_uv[0].to_bits(),
            "{ctx}: fsr2_reproj_uv[0]"
        );
        assert_eq!(
            a.fsr2_reproj_uv[1].to_bits(),
            b.fsr2_reproj_uv[1].to_bits(),
            "{ctx}: fsr2_reproj_uv[1]"
        );
        assert_eq!(
            a.fsr2_scale[0].to_bits(),
            b.fsr2_scale[0].to_bits(),
            "{ctx}: fsr2_scale[0]"
        );
        assert_eq!(
            a.fsr2_scale[1].to_bits(),
            b.fsr2_scale[1].to_bits(),
            "{ctx}: fsr2_scale[1]"
        );
        // EX-1 (wave 152): material draw 件数系の det pin (§7 消化 15 配線の観測面)
        assert_eq!(
            a.material_opaque_draws, b.material_opaque_draws,
            "{ctx}: material_opaque_draws"
        );
        assert_eq!(
            a.material_translucent_draws, b.material_translucent_draws,
            "{ctx}: material_translucent_draws"
        );
        assert_eq!(
            a.material_binned_quads, b.material_binned_quads,
            "{ctx}: material_binned_quads"
        );
        // EZ-3 (wave 154): EMA は delta_ms 系列のみ由来の完全決定値
        // (wall-clock 非依存) → det pin 正当。power_frame_dt/power_mode は
        // wall 由来 mode 経路のため非登録 (power_skip_extra と同グループ)。
        assert_eq!(
            a.power_smoothed_dt.to_bits(),
            b.power_smoothed_dt.to_bits(),
            "{ctx}: power_smoothed_dt (bit)"
        );
        // FA-3 (wave 155): HUD batch 構造 3 値 (全 scene 確定的)
        assert_eq!(a.hud_quads, b.hud_quads, "{ctx}: hud_quads");
        assert_eq!(
            a.hud_draw_ranges, b.hud_draw_ranges,
            "{ctx}: hud_draw_ranges"
        );
        assert_eq!(
            a.hud_draw_calls_saved, b.hud_draw_calls_saved,
            "{ctx}: hud_draw_calls_saved"
        );
        // FB-3 (wave 156): pool_slab 構造 4 値 (chunk_keys 系列のみ由来の
        // 完全決定値、wall-clock 非依存) → det pin 正当。
        assert_eq!(a.slab_occupied, b.slab_occupied, "{ctx}: slab_occupied");
        assert_eq!(
            a.slab_new_allocs, b.slab_new_allocs,
            "{ctx}: slab_new_allocs"
        );
        assert_eq!(a.slab_freed, b.slab_freed, "{ctx}: slab_freed");
        assert_eq!(
            a.object_pool_available, b.object_pool_available,
            "{ctx}: object_pool_available"
        );
        assert_eq!(
            a.subsystems_active, b.subsystems_active,
            "{ctx}: subsystems_active"
        );
        // 【wave 159 FE 捕捉 83】§7 消化 21 で実配線した fsr3 報告 4 面を
        // det subset 登録 (定数 3 + scene 固定で完全決定の実測 ratio)。
        assert_eq!(
            a.fsr3_color_bytes, b.fsr3_color_bytes,
            "{ctx}: fsr3_color_bytes"
        );
        assert_eq!(
            a.fsr3_depth_bytes, b.fsr3_depth_bytes,
            "{ctx}: fsr3_depth_bytes"
        );
        assert_eq!(
            a.fsr3_motion_bytes, b.fsr3_motion_bytes,
            "{ctx}: fsr3_motion_bytes"
        );
        assert_eq!(
            a.fsr3_mean_delta.to_bits(),
            b.fsr3_mean_delta.to_bits(),
            "{ctx}: fsr3_mean_delta"
        );
    }

    const IDENTITY_VP: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];

    /// 1 チャンク (0,0) + 全 1 パレット + 24 クアッドの実入力。
    /// 恒等フラスタムに対しチャンク AABB [0,16)³ と Aokana 区画 [0,64)³ は
    /// 6 面全てで冠点テストを通過する (extract_frustum_planes_identity_table の表)。
    fn chunked_inputs() -> FrameWiringInputs<'static> {
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        inputs.camera_pos = [8.0, 8.0, 8.0];
        inputs.camera_speed = 0.5;
        inputs.chunk_keys = vec![(0, 0)];
        inputs.chunk_materials = vec![1];
        inputs.chunk_aabbs = vec![([0.0, 0.0, 0.0], [16.0, 16.0, 16.0])];
        inputs.chunk_dists = vec![8.0];
        inputs.draw_index_counts = vec![36];
        inputs.section_palettes = vec![[1u16; 4096]];
        inputs.quad_positions = (0..24u32)
            .map(|i| [i as f32 * 0.5, 1.0, (i % 6) as f32 * 0.5])
            .collect();
        inputs.quad_materials = (0..24u32).map(|i| i * 7 + 1).collect();
        inputs.quad_bytes = 24 * 64;
        inputs
    }

    #[test]
    fn tick_world_empty_inputs_wellformed() {
        let (dir, mut w) = unique_wiring("empty");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        for t in 1..=4u64 {
            inputs.frame_index = t;
            let r = w.tick_world(&inputs);
            assert_eq!(
                r.aokana_visible_regions, 0,
                "tick {t}: パレットなし → リージョン未登録"
            );
            assert_eq!(
                r.visgraph_reachable, 1,
                "tick {t}: グラフ無辺でも flood_fill は始点 (cam_chunk=(0,0), dist=0) を常時含む \
                 (visibility_graph 設計: dist=0 は opaqueness 非適用)"
            );
            assert_eq!(r.lbvh_culled, 0, "tick {t}: AABB なし → LBVH カリングなし");
            assert_eq!(
                r.nanite_meshlets, 0,
                "tick {t}: クアッドなし → メッシュレットなし"
            );
            assert_eq!(r.nanite_meshlets_culled, 0, "tick {t}");
            assert_eq!(
                r.frb_billboards, 0,
                "tick {t}: クアッドなし → ビルボード変換なし"
            );
            assert_eq!(
                r.clp_lit_fraction.to_bits(),
                0.0f32.to_bits(),
                "tick {t}: CLP 未ディスパッチ → 既定 0.0"
            );
            assert_eq!(
                r.draw_command_count, 0,
                "tick {t}: コマンドなし → 0 (compact_and_filter 空走査)"
            );
            assert_eq!(
                r.lockfree_cache_hits, 0,
                "tick {t}: VRAM キャッシュ走査なし"
            );
            assert_eq!(
                r.shadow_caster_lod_dist,
                [0, 0, 0, 0],
                "tick {t}: クアッドなし → caster 評価なし (EO-1)"
            );
            assert_eq!(r.shadow_casters_culled, 0, "tick {t}: culled 0");
            assert_eq!(
                r.foveated_center_rate.to_bits(),
                0x3F00_0000,
                "tick {t}: dir=[0,0,1] → 0.5 (rq ep_foveated)"
            );
            // EQ-1 golden (rq eq_tiled): palettes 空 → lights 0 → 0/0。
            assert_eq!(r.tdl_max_tile_load, 0, "tick {t}: lights 空 → 0");
            assert_eq!(r.tdl_lit_tiles, 0, "tick {t}: lights 空 → 0");
            // ER-1 golden (rq er_dp): for_low_spec 固定 → 5/15 (定数)。
            assert_eq!(r.depth_pass_count, 5, "tick {t}: low 5 passes");
            assert_eq!(r.depth_pass_cost, 15, "tick {t}: low cost 15");
            // ER-2 golden: quads 空 → translucent 0/0。
            assert_eq!(r.translucent_total, 0, "tick {t}: translucent 0");
            assert_eq!(r.translucent_back_first, 0, "tick {t}: 空 → 0");
            // EU-2 golden (rq eu_pom): palettes 空 → heights≡0 → 層前進なし
            // cur_layer=0、final_uv=(0.5,0.5) 不動 → offset_y +0.0 (rq 導出)。
            assert_eq!(
                r.parallax_layer_depth.to_bits(),
                0x0000_0000,
                "tick {t}: 空 → 層深度 0 (rq eu_pom)"
            );
            assert_eq!(
                r.parallax_uv_offset_y.to_bits(),
                0x0000_0000,
                "tick {t}: 空 → オフセット +0.0 (rq eu_pom)"
            );
            // GC-1/GC-2 golden (rq gc_gui): dt=0.016・need=1/30 の系列で
            // render は t∈{1,4} (t1=未 valid・t4=due)。camera/screen 不変で
            // invalidated 非発火、surface valid は t1 以降維持。
            assert_eq!(
                r.gui_rendered_surface,
                t == 1 || t == 4,
                "tick {t}: GUI デュアルレート系列 (rq gc_gui)"
            );
            assert_eq!(r.gui_target_fps.to_bits(), 0x41F0_0000, "tick {t}: 30.0");
            assert!(!r.gui_invalidated, "tick {t}: 非発火");
            assert!(r.gui_reuse_cached_scene, "tick {t}: 契約");
            assert!(r.gui_surface_valid, "tick {t}: t1 render 後維持");
            assert_eq!(r.vram_used_bytes, 0, "tick {t}: 実アロケーションなし");
            assert_eq!(
                r.subsystems_active, 60,
                "tick {t}: 配線サブシステム総数は仕様値"
            );
            assert!(r.overdraw_order.is_empty(), "tick {t}");
            assert!(
                (0.05..=20.0).contains(&r.post_exposure),
                "tick {t}: 露光は adapt() クランプ域内: {}",
                r.post_exposure
            );
            assert!(
                r.ambient_light.is_finite(),
                "tick {t}: IBL アンビエントは有限"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 141 EO-5】EO-1 proxy の検出空白補完: chunked_inputs では
    /// |x| 変異 (z 成分除去) と真ノルムで culled 判定が 24 点全一致
    /// (rq eo_adv_b 機械列挙 same=24/diff=0) し golden 非検出の構造
    /// だった → z 寄与が判定を反転する点 ([3.9,1.0,1.5]:
    /// |x|=3.9<4 だが sqrt(17.46)≈4.178≥4、rq bits 0x4085B668) で
    /// proxy 式の成分構成を golden 化。
    #[test]
    fn tick_world_shadow_proxy_z_component_decides() {
        let (dir, mut w) = unique_wiring("eo_z_proxy");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        inputs.quad_positions = vec![[3.9f32, 1.0, 1.5]];
        inputs.quad_materials = vec![1];
        inputs.quad_bytes = 64;
        for t in 1..=2u64 {
            inputs.frame_index = t;
            let r = w.tick_world(&inputs);
            assert_eq!(
                r.shadow_casters_culled, 0,
                "tick {t}: z 寄与でノルム≥4.0 → cast (rq 導出)"
            );
            assert_eq!(
                r.shadow_caster_lod_dist,
                [0, 0, 0, 1],
                "tick {t}: cast 1 件・ノルム<16 → lod 3"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 142 EP-5】EP-1 の検出空白補完: 既定 camera_dir=[0,0,1]
    /// では t≥1 で rate≡min_rate=0.5 の**下限退化**となり radius/
    /// min_rate 変異が wiring golden 非検出の構造 → camera_dir を振り
    /// t<1 域の変動値を golden 化 (rq ep_foveated 導出、f32 逐次丸め)。
    #[test]
    fn tick_world_foveated_rate_varies_with_camera_dir() {
        let (dir, mut w) = unique_wiring("ep_fov_step");
        let cases: [(f32, u32); 4] = [
            (0.0, 0x3F80_0000),  // gaze=uv 一致 → 1.0
            (0.2, 0x3F3F_FFFF),  // d=0.099999994 → 0.75 ではなく 1 ulp 下
            (0.4, 0x3F00_0000),  // t=1 境界 → min_rate
            (-0.4, 0x3F00_0000), // 対称
        ];
        for (dz, want) in cases {
            let mut inputs = empty_inputs();
            inputs.view_proj = IDENTITY_VP;
            inputs.camera_dir = [0.0, 0.0, dz];
            inputs.frame_index = 1;
            let r = w.tick_world(&inputs);
            assert_eq!(
                r.foveated_center_rate.to_bits(),
                want,
                "camera_dir=[0,0,{dz}] → 0x{want:08X} (rq 導出)"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 144 ER-2】ER-2 の検出空白補完: empty/chunked とも
    /// translucent=0 (chunked は mat%7=1 全員非半透明) の**下限退化**
    /// で golden が sort 経路変異を検出しない構造 (EP-5 同型) →
    /// 半透明素材を供給する strict で golden 化 (rq er_dp 導出)。
    #[test]
    fn tick_world_translucent_sort_varies_with_materials() {
        let (dir, mut w) = unique_wiring("er_translucent");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        inputs.camera_pos = [0.0, 0.0, -1.0];
        inputs.quad_materials = vec![5, 12, 30]; // %7: 5 T, 5 T, 2 F
        inputs.quad_positions = vec![[0.0, 0.0, 0.0], [0.0, 0.0, 10.0], [1.0, 1.0, 1.0]];
        inputs.frame_index = 1;
        let r = w.tick_world(&inputs);
        assert_eq!(r.translucent_total, 2, "mat%7==5 の 2 個のみ");
        assert_eq!(
            r.translucent_back_first, 1,
            "dist2: idx1=121 > idx0=1 → back-to-front 先頭は 1 (rq 導出)"
        );
        assert_eq!(r.depth_pass_count, 5);
        assert_eq!(r.depth_pass_cost, 15);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 152 EX-1 捕捉 65】同一 translucent mat の連続 quad 2 個で
    /// 旧 debug_assert (translucent **quads** 件数 == translucent **draw ranges**
    /// 件数という誤った等置) が debug ビルドで誤爆する構造を再現 → assert を
    /// Σquad_count 不変式へ根治し draw 件数系を report 実配線 (§7 消化 15)。
    /// golden は rq ex_mb 全導出 (整数厳密)。
    #[test]
    fn tick_world_material_translucent_same_mat_runs_capture65() {
        let (dir, mut w) = unique_wiring("ex_capture65");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        inputs.quad_materials = vec![5, 5]; // %7==5 → 両方 translucent、同 mat・連続 index
        inputs.quad_positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        inputs.frame_index = 1;
        let r = w.tick_world(&inputs);
        assert_eq!(
            r.translucent_total, 2,
            "translucent quads は 2 (ER-2 規則 pin、rq ex_mb)"
        );
        // 根治後 golden (rq ex_mb (4)): opaque 0 件 / translucent 1 run
        // (first=0, count=2) / binned 2。「quads 2 個 = draw 1 件」の区別が
        // report 観測面で厳密に分離されていることを pin する。
        assert_eq!(r.material_opaque_draws, 0, "capture65: opaque 0");
        assert_eq!(
            r.material_translucent_draws, 1,
            "capture65: translucent は 1 run (≠quads 2、捕捉 65 の区別 pin)"
        );
        assert_eq!(r.material_binned_quads, 2, "capture65: Σquad_count=2");
        // 等値継続: 同一 inputs の再 tick で draw 構造は決定的同一。
        // (det_subset 全体比較はしない — leo_tag_dist 等の蓄積系は設計上
        // 同一インスタンス連続帧で増分するため、先行例 EU-2 同型の別インス
        // タンス比較文脈専用。ここでは material 系 4 値のみ直接 pin。)
        inputs.frame_index = 2;
        let r2 = w.tick_world(&inputs);
        assert_eq!(
            r2.translucent_total, 2,
            "frame2: translucent quads 等値継続"
        );
        assert_eq!(r2.material_opaque_draws, 0, "frame2: opaque 等値継続");
        assert_eq!(
            r2.material_translucent_draws, 1,
            "frame2: translucent run 等値継続"
        );
        assert_eq!(r2.material_binned_quads, 2, "frame2: binned 等値継続");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 152 EX-1】chunked 素材 (0..24).map(i*7+1) の実配線 golden:
    /// 全員 %7==1 → opaque 24 runs (全 mat 相異・各 1 quad) / translucent 0 /
    /// binned 24 == quad_materials.len() (rq ex_mb (1) 導出、整数厳密)。
    #[test]
    fn tick_world_material_draws_chunked_golden_strict() {
        let (dir, mut w) = unique_wiring("ex_chunked_draws");
        let mut inputs = chunked_inputs();
        for t in 1..=2u64 {
            inputs.frame_index = t;
            let r = w.tick_world(&inputs);
            assert_eq!(r.material_opaque_draws, 24, "tick {t}: opaque 24 runs");
            assert_eq!(
                r.material_translucent_draws, 0,
                "tick {t}: mat%7==1 全員非半透明 (mat=i*7+1 ⇒ %7==1)"
            );
            assert_eq!(r.material_binned_quads, 24, "tick {t}: Σquad_count=24");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 152 EX-1】empty 入力 golden: (0, 0, 0) — draw なしでも
    /// batcher 配線は well-formed (rq ex_mb (2))。
    #[test]
    fn tick_world_material_draws_empty_golden_strict() {
        let (dir, mut w) = unique_wiring("ex_empty_draws");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        inputs.frame_index = 1;
        let r = w.tick_world(&inputs);
        assert_eq!(r.material_opaque_draws, 0, "empty: opaque 0");
        assert_eq!(r.material_translucent_draws, 0, "empty: translucent 0");
        assert_eq!(r.material_binned_quads, 0, "empty: binned 0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 153 EY-2】chunked の fsr2_reproj_uv bit golden (rq ey_fsr2):
    /// jitter_uv (dims 正規化 640×360) → reproject(0.5,0.5)。
    /// frame1: (0x3EFFCCCD, 0x3F001E57)、frame2: (0x3F00199A, 0x3EFF7269)。
    /// fsr2_scale は (1280/640, 720/360) = (2.0, 2.0) exact。
    #[test]
    fn tick_world_fsr2_reproj_chunked_golden_strict() {
        let (dir, mut w) = unique_wiring("ey_fsr2_chunked");
        let mut inputs = chunked_inputs();
        inputs.frame_index = 1;
        let r1 = w.tick_world(&inputs);
        assert_eq!(
            (
                r1.fsr2_reproj_uv[0].to_bits(),
                r1.fsr2_reproj_uv[1].to_bits()
            ),
            (0x3EFF_CCCDu32, 0x3F00_1E57u32),
            "frame1 uv (rq ey_fsr2)"
        );
        assert_eq!(
            (r1.fsr2_scale[0].to_bits(), r1.fsr2_scale[1].to_bits()),
            (0x4000_0000u32, 0x4000_0000u32),
            "scale 2.0×2.0 exact"
        );
        inputs.frame_index = 2;
        let r2 = w.tick_world(&inputs);
        assert_eq!(
            (
                r2.fsr2_reproj_uv[0].to_bits(),
                r2.fsr2_reproj_uv[1].to_bits()
            ),
            (0x3F00_199Au32, 0x3EFF_7269u32),
            "frame2 uv (halton 系列が実駆動、rq ey_fsr2)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 153 EY-2】empty frame0 golden: jx=0 → u=0.5 exact
    /// (0x3F000000)、jy=−0.16666666 → v=0x3EFFC352 (rq ey_fsr2)。
    /// frame1 は chunked と同値 (uv は入力内容非依存、det 構造の pin)。
    #[test]
    fn tick_world_fsr2_reproj_empty_golden_strict() {
        let (dir, mut w) = unique_wiring("ey_fsr2_empty");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        inputs.frame_index = 0;
        let r0 = w.tick_world(&inputs);
        assert_eq!(
            (
                r0.fsr2_reproj_uv[0].to_bits(),
                r0.fsr2_reproj_uv[1].to_bits()
            ),
            (0x3F00_0000u32, 0x3EFF_C352u32),
            "frame0 uv (rq ey_fsr2)"
        );
        assert_eq!(
            r0.fsr2_scale[0].to_bits(),
            0x4000_0000u32,
            "scale は empty でも dims 由来 2.0 exact"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 154 EZ-1/EZ-3】power gate 実駆動の Active 経路 golden:
    /// Active (unlimited=0) → skip=false 恒 (真判定、旧恒 false 退化とは
    /// 別物 = gate true 由来)、frame_dt=0.0 契約、mode=Active(0)。
    /// smoothed_dt は EMA golden s1=0x3C87FCBA → s2=0x3C877EE6 (rq ez_pp、
    /// delta_ms=16 → real_dt=0.016 秒化系列)。
    #[test]
    fn tick_world_power_gate_active_golden_strict() {
        let (dir, mut w) = unique_wiring("ez_power_chunked");
        let mut inputs = chunked_inputs();
        inputs.frame_index = 1;
        let r1 = w.tick_world(&inputs);
        assert!(!r1.power_skip_extra, "Active → gate true (skip=false)");
        assert_eq!(
            r1.power_frame_dt.to_bits(),
            0u32,
            "target=0 → frame_dt 0.0 契約"
        );
        assert_eq!(r1.power_mode, 0, "Active=0");
        assert_eq!(
            r1.power_smoothed_dt.to_bits(),
            0x3C87_FCBAu32,
            "tick1 EMA s1 (rq ez_pp)"
        );
        inputs.frame_index = 2;
        let r2 = w.tick_world(&inputs);
        assert_eq!(
            r2.power_smoothed_dt.to_bits(),
            0x3C87_7EE6u32,
            "tick2 EMA s2 (rq ez_pp)"
        );
        assert!(!r2.power_skip_extra, "tick2 も Active → skip=false");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 154 EZ-1】empty でも EMA 同系列 (real_dt は delta_ms 由来で
    /// シーン非依存 = 完全決定の傍証、det pin の根拠)。
    #[test]
    fn tick_world_power_smoothed_empty_series_strict() {
        let (dir, mut w) = unique_wiring("ez_power_empty");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        inputs.frame_index = 1;
        let r = w.tick_world(&inputs);
        assert_eq!(
            r.power_smoothed_dt.to_bits(),
            0x3C87_FCBAu32,
            "empty tick1 EMA s1 (rq ez_pp、シーン非依存)"
        );
        assert_eq!(r.power_mode, 0, "empty も初期 Active");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 155 FA-3】HUD batch 構造値 golden: layer 4 相異キー →
    /// quads=4 / ranges=4 / saved=0 (全 scene 確定的、chunked/empty 両方
    /// で pin、rq fa_hud (1))。ImmediatelyFast merge の観測面実配線。
    #[test]
    fn tick_world_hud_batch_counts_golden_strict() {
        for (tag, chunked) in [("fa_hud_c", true), ("fa_hud_e", false)] {
            let (dir, mut w) = unique_wiring(tag);
            let mut inputs = if chunked {
                chunked_inputs()
            } else {
                empty_inputs()
            };
            inputs.view_proj = IDENTITY_VP;
            inputs.frame_index = 1;
            let r = w.tick_world(&inputs);
            assert_eq!(r.hud_quads, 4, "{tag}: 4 bar = 4 quads");
            assert_eq!(r.hud_draw_ranges, 4, "{tag}: 4 相異キー = 4 ranges");
            assert_eq!(r.hud_draw_calls_saved, 0, "{tag}: merge なし → saved 0");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// 【wave 155 FA-1 捕捉 72 TDD RED】HUD 棒グラフ rect 契約: module は
    /// `[x,y,w,h]` の w を受けるのに旧 wiring は x_end (8.0+v*120.0) を w
    /// として供給 → 幅が常に +8px 過大 (v=0 でも 8px の非ゼロ棒 = 捕捉
    /// 62/63 クラスの供給値契約不一致)。幅は wiring 内基準 x=8 起点で
    /// `x1-x0 == v*120` (端 8+v*120) が正しい。頂点は finish 後も
    /// begin_frame まで保持されるため経由して厳密照合する。
    #[test]
    fn tick_world_hud_bar_width_capture72_strict() {
        let (dir, mut w) = unique_wiring("fa_hud72");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        inputs.frame_index = 1;
        let r = w.tick_world(&inputs);
        let stats = [
            r.draw_command_count as f32 / 64.0,
            r.lockfree_cache_hits as f32 / 16.0,
            r.lbvh_culled as f32 / 16.0,
            r.aokana_visible_regions as f32 / 8.0,
        ];
        let mut scratch = crate::hud_batch::BatchOutput::default();
        let view = w.hud.finish(&mut scratch);
        for (i, &v) in stats.iter().enumerate() {
            let vw = v.clamp(0.0, 1.0) * 120.0; // 真の設計幅 (端 = 8+vw)
            let got = view.vertices[i * 4 + 1].pos[0] - view.vertices[i * 4].pos[0];
            assert_eq!(
                got.to_bits(),
                vw.to_bits(),
                "bar{i}: 幅は設計値 v*120 (v={vw}); 旧 x_end 供給は +8px 超過 (捕捉 72)"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 156 FB-1 TDD RED】pool_slab lifecycle 構造値 golden: 静的
    /// chunked keys=[(0,0)] は t1 で new=1/occupied=1、t2 は map hit で
    /// new=0/freed=0/occupied=1 不変 (rq fb_slab (5))。捕捉 74 根治で
    /// vacuous MAX 分岐は除去、ObjectPool mid available=1 も pin。
    #[test]
    fn tick_world_slab_lifecycle_counts_golden_strict() {
        let (dir, mut w) = unique_wiring("fb_slab_c");
        let mut inputs = chunked_inputs();
        inputs.frame_index = 1;
        let r1 = w.tick_world(&inputs);
        assert_eq!(r1.slab_new_allocs, 1, "t1: 初見 (0,0) のみ alloc");
        assert_eq!(r1.slab_freed, 0, "t1: 退去なし");
        assert_eq!(r1.slab_occupied, 1, "t1: occupied 1");
        assert_eq!(r1.object_pool_available, 1, "cap2-1=1 (acquire 稼働)");
        inputs.frame_index = 2;
        let r2 = w.tick_world(&inputs);
        assert_eq!(r2.slab_new_allocs, 0, "t2: 同 key → map hit で alloc 0");
        assert_eq!(r2.slab_freed, 0, "t2: 退去なし");
        assert_eq!(
            r2.slab_occupied, 1,
            "t2: occupied 不変 (旧無限 append 根治)"
        );
        assert_eq!(r2.object_pool_available, 1, "t2 も 1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 156 FB-1】empty scene golden + integrity: key ゼロでは
    /// 全 tick 0/0/0 (is_empty 一致)、ObjectPool は chunk 非依存で 1。
    #[test]
    fn tick_world_slab_empty_golden_strict() {
        let (dir, mut w) = unique_wiring("fb_slab_e");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        for t in 1..=2u64 {
            inputs.frame_index = t;
            let r = w.tick_world(&inputs);
            assert_eq!(r.slab_new_allocs, 0, "t{t}: alloc 0");
            assert_eq!(r.slab_freed, 0, "t{t}: free 0");
            assert_eq!(r.slab_occupied, 0, "t{t}: occupied 0 (is_empty 一致)");
            assert_eq!(r.object_pool_available, 1, "t{t}: pool は chunk 非依存");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 156 FB-1 捕捉 74/§7 消化 19 TDD RED】eviction 系列 strict
    /// (rq fb_slab (6)): t1 keys [(0,0),(1,0)] mats [7,9] → slots 0,1
    /// occupied 2。t2 keys [(1,0),(2,0)] mats [5,3] → 退去 (0,0) freed=1
    /// (mat 7 回収)、初見 (2,0) new=1 で slot0 LIFO 再利用 (gen+1)、
    /// occupied 2 追従。material read-back は (1,0)→9 / (2,0)→3 の厳密
    /// 一致 (全 tick chunk_keys 全要素を走査する chunk_id 0..last)。
    #[test]
    fn tick_world_slab_eviction_golden_strict() {
        let (dir, mut w) = unique_wiring("fb_slab_v");
        let mut inputs = chunked_inputs();
        inputs.chunk_keys = vec![(0, 0), (1, 0)];
        inputs.chunk_materials = vec![7, 9];
        inputs.chunk_aabbs = vec![
            ([0.0, 0.0, 0.0], [16.0, 16.0, 16.0]),
            ([16.0, 0.0, 0.0], [32.0, 16.0, 16.0]),
        ];
        inputs.chunk_dists = vec![8.0, 8.0];
        inputs.draw_index_counts = vec![36, 36];
        inputs.section_palettes = vec![[1u16; 4096], [2u16; 4096]];
        inputs.frame_index = 1;
        let r1 = w.tick_world(&inputs);
        assert_eq!(
            (r1.slab_new_allocs, r1.slab_freed, r1.slab_occupied),
            (2, 0, 2),
            "t1 構造値"
        );
        assert_eq!(
            w.slab.get(w.slab_key_to_slot[&(0, 0)]).copied(),
            Some(7),
            "t1: (0,0)→mat 7"
        );
        assert_eq!(
            w.slab.get(w.slab_key_to_slot[&(1, 0)]).copied(),
            Some(9),
            "t1: (1,0)→mat 9"
        );
        assert_eq!(w.slab_key_to_slot[&(0, 0)], 0, "(0,0)→slot0");
        assert_eq!(w.slab_key_to_slot[&(1, 0)], 1, "(1,0)→slot1");
        // t2: (0,0) 退去 → mat 7 回収、(2,0) 新規 → slot0 LIFO 再利用。
        inputs.chunk_keys = vec![(1, 0), (2, 0)];
        inputs.chunk_materials = vec![5, 3];
        inputs.frame_index = 2;
        let r2 = w.tick_world(&inputs);
        assert_eq!(
            (r2.slab_new_allocs, r2.slab_freed, r2.slab_occupied),
            (1, 1, 2),
            "t2 構造値"
        );
        assert_eq!(
            w.slab.get(w.slab_key_to_slot[&(1, 0)]).copied(),
            Some(9),
            "t2: (1,0) は mat 9 のまま (inputs mat 5 に上書きされない)"
        );
        assert_eq!(
            w.slab.get(w.slab_key_to_slot[&(2, 0)]).copied(),
            Some(3),
            "t2: (2,0)→mat 3"
        );
        assert_eq!(
            w.slab_key_to_slot[&(2, 0)],
            0,
            "(2,0) は free 済 slot0 を LIFO 再利用"
        );
        assert!(
            !w.slab_key_to_slot.contains_key(&(0, 0)),
            "退去 key は map から除去"
        );
        assert_eq!(w.slab.len(), 2, "module len と map 整合");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// EU-2 補完 strict: camera_dir を振ると POM 配線値が変化する (実経路
    /// 稼働 pin)。chunked golden は既定 dir=[0,0,1] の 1 点のみのため振動
    /// 供給。[0,1,1] → view=(0,1,1)、step_y=0x3BCCCCCD (rq eu_pom 導出)。
    #[test]
    fn tick_world_parallax_varies_with_camera_dir() {
        let (dir_a, mut a) = unique_wiring("eu_pom_a");
        let (dir_b, mut b) = unique_wiring("eu_pom_b");
        let mut ia = chunked_inputs();
        let mut ib = chunked_inputs();
        ia.camera_dir = [0.0, 0.0, 1.0]; // view=(0,1,0.2) → step_y 0.03125
        ib.camera_dir = [0.0, 1.0, 1.0]; // view=(0,1,1.0) → step_y 0x3BCCCCCD
        ia.frame_index = 1;
        ib.frame_index = 1;
        let ra = a.tick_world(&ia);
        let rb = b.tick_world(&ib);
        assert_eq!(
            ra.parallax_uv_offset_y.to_bits(),
            0xBEF0_0000,
            "dir=[0,0,1] → -0.46875 (rq eu_pom)"
        );
        assert_eq!(
            rb.parallax_uv_offset_y.to_bits(),
            0xBDBF_FFF4,
            "dir=[0,1,1] → 15 逐次減算後 -0.09374991 (rq eu_pom)"
        );
        assert_ne!(
            ra.parallax_uv_offset_y.to_bits(),
            rb.parallax_uv_offset_y.to_bits(),
            "camera_dir 振動で POM オフセットが変化 (実経路稼働)"
        );
        // 層深度は高さ場のみ決定 (camera 非依存、lattice 退化 0.9375)。
        assert_eq!(ra.parallax_layer_depth.to_bits(), 0x3F70_0000);
        assert_eq!(rb.parallax_layer_depth.to_bits(), 0x3F70_0000);
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn tick_world_chunked_inputs_exact_and_cross_instance_deterministic() {
        let (dir_a, mut a) = unique_wiring("chunk_a");
        let (dir_b, mut b) = unique_wiring("chunk_b");
        let (mut ia, mut ib) = (chunked_inputs(), chunked_inputs());
        for t in 1..=8u64 {
            ia.frame_index = t;
            ib.frame_index = t;
            let ra = a.tick_world(&ia);
            let rb = b.tick_world(&ib);
            assert_report_det_subset(&ra, &rb, &format!("tick {t}"));
            // K-1 回帰: パレット供給チャンク (0,0) → リージョン (0,0,0) (64³ 区画)。
            // 区画 AABB [0,64)³ は恒等フラスタム 6 面全合格 → 常時 1 区画可視。
            assert_eq!(
                ra.aokana_visible_regions, 1,
                "tick {t}: K-1 実登録 1 区画が可視"
            );
            // 3 層フィルタ (SIMD frustum / LBVH / compact mask) 全通過の静的検証済み。
            assert_eq!(
                ra.draw_command_count, 1,
                "tick {t}: 1 可視チャンク = 1 コマンド"
            );
            assert_eq!(ra.frb_billboards, 24, "tick {t}: 24 クアッドの実変換");
            // EP-1 golden (rq ep_foveated 導出): camera_dir=[0,0,1] →
            // gaze=(0.5,1.0), dy=-0.5 → t=2.5→clamp 1 → rate=0.5。
            assert_eq!(
                ra.foveated_center_rate.to_bits(),
                0x3F00_0000,
                "tick {t}: foveated = min_rate 0.5 (rq 導出)"
            );
            // EQ-1 golden (rq eq_tiled 導出): 全 1 パレット → z=0/1 平面
            // の 32 灯 (radius=1.0, sr=960px)。IDENTITY_VP 対称で新旧
            // 読み一致。x≥2 は min_tx≥120>119 逆転空ループで消失 (物理的
            // に正しい画面外排除) → x=0 (列 0..119 全行) + x=1 (列
            // 60..119) の 4 灯のみ → max=4 (列 60..119) / lit=8160 (x=0
            // 灯が全タイル被覆)。
            assert_eq!(
                ra.tdl_max_tile_load, 4,
                "tick {t}: 列 60..119 は x=0/1 の 4 灯 (rq eq_tiled)"
            );
            assert_eq!(
                ra.tdl_lit_tiles, 8160,
                "tick {t}: 120x68 全タイル被覆 (rq eq_tiled)"
            );
            // ER-1/ER-2 golden (rq er_dp): plan は定数 5/15。
            // quad_materials=(0..24).map(i*7+1) → mat%7=1 全員非半透明
            // → translucent 0/0 (下限退化、ER 補完 strict で振動供給)。
            assert_eq!(ra.depth_pass_count, 5, "tick {t}: low 5 passes");
            assert_eq!(ra.depth_pass_cost, 15, "tick {t}: low cost 15");
            assert_eq!(ra.translucent_total, 0, "tick {t}: mat%7=1 → 0");
            assert_eq!(ra.translucent_back_first, 0, "tick {t}: 空 → 0");
            // EU-2 golden (rq eu_pom): 全 1 パレット → heights≡15/16=0.9375。
            // 15 層前進で cur_layer=0.9375 (layer_depth=1/16 と高さ格子 y/16
            // が同相の lattice 退化 → 補間 w=0 → final=cur_uv)。camera_dir
            // =[0,0,1] → view=(0,1,0.2)、step_y=0.03125 → final_uv.y=0.03125
            // → offset_y=-0.46875。全値 rq 導出 bit pin。
            assert_eq!(
                ra.parallax_layer_depth.to_bits(),
                0x3F70_0000,
                "tick {t}: 層深度 0.9375 (rq eu_pom)"
            );
            assert_eq!(
                ra.parallax_uv_offset_y.to_bits(),
                0xBEF0_0000,
                "tick {t}: offset_y -0.46875 (rq eu_pom)"
            );
            // GC-1/GC-2 golden (rq gc_gui): t∈{1,4,6,8} (16ms 系列 8 tick)。
            assert_eq!(
                ra.gui_rendered_surface,
                t == 1 || t == 4 || t == 6 || t == 8,
                "tick {t}: GUI デュアルレート系列 (rq gc_gui)"
            );
            assert_eq!(ra.gui_target_fps.to_bits(), 0x41F0_0000, "tick {t}");
            assert!(!ra.gui_invalidated, "tick {t}: camera/screen 不変 → 非発火");
            assert!(ra.gui_surface_valid, "tick {t}");
            // EO-1 golden (rq eo_shadow 機械列挙): 24 クアッド (x,z) ノルム
            // proxy で culled=8 (i=0..7 ノルム<4.0)、cast=16 は全て
            // ノルム<16 (max sqrt(138.5)=0x413C4C32) → lod 3 バケット集中。
            assert_eq!(
                ra.shadow_caster_lod_dist,
                [0, 0, 0, 16],
                "tick {t}: cast 16 全 lod 3 (rq 導出)"
            );
            assert_eq!(ra.shadow_casters_culled, 8, "tick {t}: culled 8 (rq 導出)");
        }
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    /// GC-1/GC-2 golden: 16ms (62.5Hz) 駆動での GUI デュアルレート系列。
    /// rq gc_gui 確定: dt=0x3C83126F・need=0x3D088889、render は
    /// t ∈ {1,4,6,8,10,12,14,16} (f32 蓄積の非自明系列、16 tick で 8 回
    /// ≒ 30 GUI fps)。t1 は surface 未 valid、以降は due のみ駆動。
    #[test]
    fn tick_world_gui_dual_rate_cadence_golden() {
        const EXPECT: [bool; 16] = [
            true, false, false, true, false, true, false, true, false, true, false, true, false,
            true, false, true,
        ];
        let (dir, mut w) = unique_wiring("gc_gui_cadence");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        for i in 0..16usize {
            inputs.frame_index = i as u64 + 1;
            let r = w.tick_world(&inputs);
            assert_eq!(
                r.gui_rendered_surface,
                EXPECT[i],
                "tick {}: rq gc_gui 導出系列との一致 (捕捉 62 ms 誤供給は全 true 化で RED)",
                i + 1
            );
            assert_eq!(r.gui_target_fps.to_bits(), 0x41F0_0000, "30.0 定数");
            assert!(!r.gui_invalidated, "camera/screen 不変 → 非発火");
            assert!(r.gui_reuse_cached_scene, "契約: 常時キャッシュ再利用");
            assert!(r.gui_surface_valid, "t1 render 後は常に valid");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// GC-3 稼働 pin: camera_dir 変化 (視点操作=入力駆動) で on_input_event
    /// が実駆動 → due=false の tick でも即時 render + invalidated=true。
    #[test]
    fn tick_world_gui_camera_input_invalidates() {
        let (dir, mut w) = unique_wiring("gc_gui_cam");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        inputs.frame_index = 1;
        let r1 = w.tick_world(&inputs);
        assert!(r1.gui_rendered_surface);
        assert!(!r1.gui_invalidated, "初回 prev なし → 非発火 (rq)");
        // t2: accum=0.016 < need で due 非成立の tick に camera 変更。
        inputs.camera_dir = [0.0, 1.0, 1.0];
        inputs.frame_index = 2;
        let r2 = w.tick_world(&inputs);
        assert!(
            r2.gui_invalidated,
            "camera_dir 変化で on_input_event 実駆動"
        );
        assert!(
            r2.gui_rendered_surface,
            "due 非依存で即時再描画 (入力遅延ゼロ設計)"
        );
        // t3: camera 再び不変 → invalidated クリア、due 未成立 → render なし。
        inputs.frame_index = 3;
        let r3 = w.tick_world(&inputs);
        assert!(!r3.gui_invalidated);
        assert!(!r3.gui_rendered_surface, "accum=0 起点 → t3 非 due (rq)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// GC-4 稼働 pin: screen 寸法変化 (GUI 面の実破棄事象) で invalidate()
    /// が実駆動 → 次 tick render=true・surface_valid 再確立。
    #[test]
    fn tick_world_gui_screen_resize_invalidates() {
        let (dir, mut w) = unique_wiring("gc_gui_screen");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        inputs.frame_index = 1;
        let r1 = w.tick_world(&inputs);
        assert!(r1.gui_surface_valid);
        assert!(r1.gui_rendered_surface);
        // t2: 解像度変更 → invalidate() 実駆動 → この tick で即再描画。
        inputs.screen_w = 1280;
        inputs.frame_index = 2;
        let r2 = w.tick_world(&inputs);
        assert!(r2.gui_rendered_surface, "resize で GUI 面破棄 → 即再描画");
        assert!(r2.gui_surface_valid, "render 後に valid 再確立");
        // t3: 不変に戻す → due 未成立 → render なし。
        inputs.frame_index = 3;
        let r3 = w.tick_world(&inputs);
        assert!(!r3.gui_rendered_surface, "resize 後は通常系列へ復帰 (rq)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// GC-1 振動 pin: delta_ms=20 (50Hz) では cadence が真に変化する。
    /// rq gc_gui 確定: render t ∈ {1,3,5,7,8,10,12,13,15} (9 回/16 tick、
    /// 境界 gap 2^-27 で t8/t13 連続 render が発生する f32 非自明系列)。
    #[test]
    fn tick_world_gui_dt_varies_cadence() {
        const EXPECT20: [bool; 16] = [
            true, false, true, false, true, false, true, true, false, true, false, true, true,
            false, true, false,
        ];
        let (dir, mut w) = unique_wiring("gc_gui_dt");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        inputs.delta_ms = 20.0;
        let mut same_as_16ms = true;
        const EXPECT16: [bool; 16] = [
            true, false, false, true, false, true, false, true, false, true, false, true, false,
            true, false, true,
        ];
        for i in 0..16usize {
            inputs.frame_index = i as u64 + 1;
            let r = w.tick_world(&inputs);
            assert_eq!(
                r.gui_rendered_surface,
                EXPECT20[i],
                "tick {} (rq gc_gui)",
                i + 1
            );
            same_as_16ms &= r.gui_rendered_surface == EXPECT16[i];
        }
        assert!(
            !same_as_16ms,
            "delta_ms 振動で cadence が真に変化 (実経路稼働 pin)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// 単一 retry、retry 失敗時の潔い None + 被害者 1 件の bounded 挙動。
    #[test]
    fn gb_eviction_fifo_single_retry_and_stale_skip() {
        let (dir, mut w) = unique_wiring("gb_evict");
        assert!(w.gb_handles.is_empty() && w.gb_order.is_empty());
        assert!(w.gb_alloc_or_evict((0, 0), 1 << 20).is_some());
        assert!(w.gb_alloc_or_evict((1, 1), 2 << 20).is_some());
        assert!(w.gb_alloc_or_evict((0, 0), 1 << 20).is_some(), "置換も成功");
        let order: Vec<_> = w.gb_order.iter().copied().collect();
        assert_eq!(order, vec![(0, 0), (1, 1)], "置換は順位不変 (重複なし)");
        // 512MiB 容量に 600MiB oversize: 単一 retry でも不可 → None、
        // ただし FIFO 最古 (0,0) だけが実解放される。
        assert!(w.gb_alloc_or_evict((9, 9), 600 << 20).is_none());
        assert!(!w.gb_handles.contains_key(&(0, 0)), "FIFO 最古が被害者");
        assert!(w.gb_handles.contains_key(&(1, 1)), "次点は生存");
        assert!(!w.gb_handles.contains_key(&(9, 9)), "失敗確保は未登録");
        assert_eq!(w.gb_order.len(), 1, "被害者は 1 件のみ消費 (bounded)");
        // 空き回復後の小確保は追加退避なしで成功 (stale なし)。
        assert!(w.gb_alloc_or_evict((2, 2), 1 << 20).is_some());
        assert!(w.gb_handles.contains_key(&(1, 1)));
        assert!(w.gb_handles.contains_key(&(2, 2)));
        let _ = std::fs::remove_dir_all(&dir);

        // retry 成功径路: 250+250+11=511MiB 使用で空き 1MiB の状態に
        // 12MiB を要求 → 1 回目失敗、FIFO 最古 (10,10) の 250MiB 解放で
        // 単一 retry が成功 (旧実装は retry が無く静寂欠落していた径路)。
        let (dir2, mut w) = unique_wiring("gb_retry");
        assert!(w.gb_alloc_or_evict((10, 10), 250 << 20).is_some());
        assert!(w.gb_alloc_or_evict((11, 11), 250 << 20).is_some());
        assert!(w.gb_alloc_or_evict((12, 12), 11 << 20).is_some());
        assert!(w.gb_alloc_or_evict((13, 13), 12 << 20).is_some());
        assert!(!w.gb_handles.contains_key(&(10, 10)), "FIFO 最古を実解放");
        assert!(w.gb_handles.contains_key(&(11, 11)));
        assert!(w.gb_handles.contains_key(&(12, 12)));
        assert!(
            w.gb_handles.contains_key(&(13, 13)),
            "retry 成功で当該確保が救済 (旧実装では欠落)"
        );
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn tick_world_periodic_rebuild_cross_instance_deterministic() {
        // tick%600 の SVDAG 再構築 / pso_lib.save / tick%120 の CLP ディスパッチ周期を
        // 跨いでも報告の決定性集合が両インスタンスで一致すること。
        let (dir_a, mut a) = unique_wiring("period_a");
        let (dir_b, mut b) = unique_wiring("period_b");
        let (mut ia, mut ib) = (chunked_inputs(), chunked_inputs());
        const SAMPLE: [u64; 5] = [1, 120, 599, 600, 601];
        for t in 1..=601u64 {
            ia.frame_index = t;
            ib.frame_index = t;
            let ra = a.tick_world(&ia);
            let rb = b.tick_world(&ib);
            if SAMPLE.contains(&t) {
                assert_report_det_subset(&ra, &rb, &format!("tick {t}"));
                assert_eq!(
                    ra.aokana_visible_regions, 1,
                    "tick {t}: 再構築後も実座標登録を維持"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    /// 【wave 136 EJ】empty inputs golden — 全値を rq 事前導出で閉じて導出
    /// (ej_async.rq: PROXY bits・shadow_res≡2048→2.0・post=2.0736 (0x4004B5DD)・
    /// naive=post, pipelined=2.0, saved=3.549385 (0x40632920))。
    #[test]
    fn tick_world_empty_inputs_ej_golden() {
        let (dir, mut w) = unique_wiring("empty_ej");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        for t in 1..=4u64 {
            inputs.frame_index = t;
            let r = w.tick_world(&inputs);
            assert_eq!(
                r.gtao_occ.to_bits(),
                0x3F80_0000,
                "tick {t}: パレットなし → gtao=1.0"
            );
            assert_eq!(
                r.ao_halfres_mean.to_bits(),
                0x3F80_0000,
                "tick {t}: ao mean=1.0"
            );
            assert_eq!(
                r.ao_halfres_min.to_bits(),
                0x3F80_0000,
                "tick {t}: ao min=1.0"
            );
            assert_eq!(
                r.async_overlap_proxy_ms.to_bits(),
                0x4000_0000,
                "tick {t}: overlap = max(g=2.0, c=0) (rq ej_async)"
            );
            assert_eq!(
                r.async_pipelined_proxy_ms.to_bits(),
                0x4000_0000,
                "tick {t}: pipelined = g = 2.0 (rq ej_async)"
            );
            assert_eq!(
                r.async_saved_pct.to_bits(),
                0x4063_2920,
                "tick {t}: saved = (2.0736-2.0)/2.0736*100 bits = 0x40632920 (rq ej_async)"
            );
            // EN-1: lights 0 (パレット空) → ballot=0・reduce 空で +0.0
            // (None フォールバック + .max(0.0) 正規化の両経路を golden で固定)
            assert_eq!(r.emissive_high_mask, 0, "tick {t}: lights 0 → ballot 0");
            assert_eq!(
                r.subgroup_wave_sum_max.to_bits(),
                0x0000_0000,
                "tick {t}: lights 0 → reduce 空 → +0.0"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 136 EJ】chunked (全不透明パレット: 高さ場 16/16=1.0 全域)'
    /// フラット高さ場 → gtao/AO は 1.0 exact の pin + async 値 probe。
    #[test]
    fn tick_world_chunked_inputs_ej_golden() {
        let (dir, mut w) = unique_wiring("chunk_ej");
        let mut inputs = chunked_inputs();
        for t in 1..=3u64 {
            inputs.frame_index = t;
            let r = w.tick_world(&inputs);
            assert_eq!(
                r.gtao_occ.to_bits(),
                0x3F80_0000,
                "tick {t}: 全高 16 フラット高さ場 → gtao=1.0"
            );
            assert_eq!(
                r.ao_halfres_mean.to_bits(),
                0x3F80_0000,
                "tick {t}: ao mean=1.0"
            );
            assert_eq!(
                r.ao_halfres_min.to_bits(),
                0x3F80_0000,
                "tick {t}: ao min=1.0"
            );
            // EJ_CHUNK probe → rq ej_chunked.rq で再演 bit 一致を照合後 pin:
            // draw=1, lit=1149 → g=2.008 (0x40008312) / c=11.492 (0x4137DF3B)
            assert_eq!(
                r.async_overlap_proxy_ms.to_bits(),
                0x4137_DF3B,
                "tick {t}: overlap = max(2.008, 11.492) (rq 再演照合済 golden)"
            );
            assert_eq!(
                r.async_pipelined_proxy_ms.to_bits(),
                0x4139_0CB2,
                "tick {t}: pipelined = c+(post−shadow).max(0) (rq golden)"
            );
            assert_eq!(
                r.async_saved_pct.to_bits(),
                0x416B_E40B,
                "tick {t}: saved = (13.5656−11.5656)/13.5656*100 = 14.743175 (rq golden)"
            );
            // EN-1 golden: パレット全 id=1 → light=1%16=1>0 で全ボクセル
            // 発光、上限 32 灯 × intensity 1.0 → 単一 wave sum=32.0
            // (rq 導出 32×1.0=32.0=0x42000000)。lvl=1 ≤ 8.0 → ballot 全 0。
            assert_eq!(
                r.emissive_high_mask, 0,
                "tick {t}: lvl=1 ≤ 8.0 threshold → ballot 全 0"
            );
            assert_eq!(
                r.subgroup_wave_sum_max.to_bits(),
                0x4200_0000,
                "tick {t}: 32 灯×1.0 = wave sum 32.0 (rq 導出)"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 136 EJ】段差パレット (x<8: 列高 2、x>=8: 列高 4) で AO/GTAO が
    /// 真に変動する特性 pin + golden probe: フラットの定数 1.0 退化からの
    /// 脱却が存在する場に対して遮蔽が発生することを強制 (検出空白回避)。
    #[test]
    fn tick_world_step_palette_ej_ao_varies() {
        let (dir, mut w) = unique_wiring("step_ej");
        let mut sec = [0u16; 4096];
        for z in 0..16usize {
            for x in 0..8usize {
                for y in 0..2usize {
                    sec[section_idx(x, y, z)] = 1;
                }
            }
            for x in 8..16usize {
                for y in 0..4usize {
                    sec[section_idx(x, y, z)] = 1;
                }
            }
        }
        let mut inputs = chunked_inputs();
        inputs.section_palettes = vec![sec];
        for t in 1..=3u64 {
            inputs.frame_index = t;
            let r = w.tick_world(&inputs);
            // 順方向章の probe 実測 (3 tick 全同一) → golden pin:
            // gtao ≡ 1.0 (中央 x=8 が高側 z=0.25 で (h−c)≤0、GTAO 退化 = 構造明示)、
            // ao mean=0.90624988 (0x3F67FFBA) / min=0.47019 (0x3EF083B1)。
            assert_eq!(
                r.gtao_occ.to_bits(),
                0x3F80_0000,
                "tick {t}: 順方向は GTAO 退化 (中央高側で occ 非発生) = 構造 pin"
            );
            assert_eq!(
                r.ao_halfres_mean.to_bits(),
                0x3F67_FFBA,
                "tick {t}: 半解像度 AO 平均 golden (実測 probe 固定, 3 tick det 一致)"
            );
            assert_eq!(
                r.ao_halfres_min.to_bits(),
                0x3EF0_83B1,
                "tick {t}: 半解像度 AO 最小 golden (段差遮蔽の真値)"
            );
            assert!(
                r.ao_halfres_min.to_bits() != 0x3F80_0000 || r.gtao_occ.to_bits() != 0x3F80_0000,
                "tick {t}: 段差断面で AO/GTAO の少なくとも一方が 1.0 未満を観測せよ"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 136 EJ】段差逆 (x<8: 列高 4、x>=8: 列高 2) では GTAO 中央断面
    /// (x=8 = 低側 z=0.125) の左隣が高いため (h−c)/off > 0 → atan > 0 →
    /// occ > 0 → gtao_occ < 1.0 が真に発生することを pin (順方向の step
    /// では中央が高側にあり (h−c)≤0 で GTAO が退化=1.0 に留まる検出空白
    /// を本逆向きで構造排除)。golden は実測 probe → rq 閉形式 window 照合。
    #[test]
    fn tick_world_step_palette_reverse_ej_gtao_varies() {
        let (dir, mut w) = unique_wiring("step_rev_ej");
        let mut sec = [0u16; 4096];
        for z in 0..16usize {
            for x in 0..8usize {
                for y in 0..4usize {
                    sec[section_idx(x, y, z)] = 1;
                }
            }
            for x in 8..16usize {
                for y in 0..2usize {
                    sec[section_idx(x, y, z)] = 1;
                }
            }
        }
        let mut inputs = chunked_inputs();
        inputs.section_palettes = vec![sec];
        for t in 1..=3u64 {
            inputs.frame_index = t;
            let r = w.tick_world(&inputs);
            assert_eq!(
                r.gtao_occ.to_bits(),
                0x3E97_2028,
                "tick {t}: GTAO 閉形式 golden — 1−atan2(2,1)/(π/2) rq 導出 == 実測 bit 一致"
            );
            assert_eq!(
                r.ao_halfres_mean.to_bits(),
                0x3F67_F065,
                "tick {t}: 逆段差 AO 平均 golden (実測 probe 固定)"
            );
            assert_eq!(
                r.ao_halfres_min.to_bits(),
                0x3F17_FB8C,
                "tick {t}: 逆段差 AO 最小 golden (実測 probe 固定)"
            );
            assert!(
                r.gtao_occ < 1.0,
                "tick {t}: 逆向き段差で GTAO 真値変動 (occ > 0) を必須観測"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 136 EJ-1】高さ場ヘルパの直接 golden pin。
    /// adversarial (a) (全列 −1 uniform shift) は AO horizon が差分オペレータ
    /// であるため tick golden 系では構造的に非検出 (誠実記録) — 本テストで
    /// 高さ場出力レベル自体を pin して変異を RED 化する検出空白補完。
    #[test]
    fn section_heightfield_depth_golden_levels() {
        let lut = crate::branchless_block::BlockLut::new();
        // 段差: x<8 列高 4、x>=8 列高 2 (理由: 逆段差テストと対称底盤)
        let mut sec = [0u16; 4096];
        for z in 0..16usize {
            for x in 0..8usize {
                for y in 0..4usize {
                    sec[section_idx(x, y, z)] = 1;
                }
            }
            for x in 8..16usize {
                for y in 0..2usize {
                    sec[section_idx(x, y, z)] = 1;
                }
            }
        }
        let depth = section_heightfield_depth(&sec, &lut);
        let c4 = depth.iter().filter(|v| v.to_bits() == 0x3E80_0000).count();
        let c2 = depth.iter().filter(|v| v.to_bits() == 0x3E00_0000).count();
        assert_eq!(
            c4, 128,
            "列高 4/16=0.25 (0x3E800000) の列 census = 8*x * 16 z"
        );
        assert_eq!(c2, 128, "列高 2/16=0.125 (0x3E000000) の列 census");
        assert_eq!(
            depth[8 * 16 + 7].to_bits(),
            0x3E80_0000,
            "(x=7,z=8) 高 4 → 0.25 exact"
        );
        assert_eq!(
            depth[8 * 16 + 9].to_bits(),
            0x3E00_0000,
            "(x=9,z=8) 高 2 → 0.125 exact"
        );
        // 上端スパイク (y=15) → 16/16 = 1.0 exact (col=y+1 オフセットの検出点)
        let mut sec2 = [0u16; 4096];
        sec2[section_idx(5, 15, 12)] = 1;
        let depth2 = section_heightfield_depth(&sec2, &lut);
        assert_eq!(
            depth2[12 * 16 + 5].to_bits(),
            0x3F80_0000,
            "y=15 頂上 → 16/16 = 1.0 exact (off-by-one 捕捉点)"
        );
        // 全ゼロ → 空列 0.0 exact (z0<=0 early return 境界)
        assert_eq!(
            section_heightfield_depth(&[0u16; 4096], &lut)[10 * 16 + 10].to_bits(),
            0
        );
    }

    /// 【wave 137 EK-1】LEO ring maintain strict golden: palettes 空 → tag=1
    /// のみ累積し、payload(tick) を get_payload で実読出しする report 値。
    #[test]
    fn tick_world_leo_tag_dist_strict_golden() {
        let (dir, mut w) = unique_wiring("leo_ek");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        for t in 1..=6u64 {
            inputs.frame_index = t;
            let r = w.tick_world(&inputs);
            let want = [0, t as u32, 0, 0, 0, 0, 0, 0];
            assert_eq!(
                r.leo_tag_dist, want,
                "tick {t}: palettes 空 → tag=1 のみ累積"
            );
            assert_eq!(r.leo_latest_tick, t, "payload(=tick) の実読出し golden");
            assert_eq!(
                r.leo_tag_dist.iter().sum::<u32>() as usize,
                t as usize,
                "Σ dist == ring len 不変式"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 137 EK-1】palettes=3 → tag=3 の分布 pin (mixed 分布検出感度)。
    #[test]
    fn tick_world_leo_tag_dist_mixed_palettes_strict() {
        let (dir, mut w) = unique_wiring("leo_mixed");
        let sec = [1u16; 4096];
        let mut inputs = chunked_inputs();
        inputs.section_palettes = vec![sec, sec, sec];
        for t in 1..=3u64 {
            inputs.frame_index = t;
            let r = w.tick_world(&inputs);
            let want = [0, 0, 0, t as u32, 0, 0, 0, 0];
            assert_eq!(
                r.leo_tag_dist, want,
                "tick {t}: palettes 3 → tag=3 のみ累積"
            );
            assert_eq!(r.leo_latest_tick, t);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 137 EK-1】ring>4096 飽和: pop_front が走っても decode 対称
    /// 減算で分布は 4096 に飽和維持 (削除経路の不変式検証: 本経路のみでは
    /// 未検証だった機能を strict でカバー → 検出空白補完)。
    #[test]
    fn tick_world_leo_ring_4096_saturation_strict() {
        let (dir, mut w) = unique_wiring("leo_sat");
        let mut inputs = empty_inputs();
        inputs.view_proj = IDENTITY_VP;
        for t in 1..=4097u64 {
            inputs.frame_index = t;
            let r = w.tick_world(&inputs);
            if t == 4097 {
                assert_eq!(
                    r.leo_tag_dist,
                    [0, 4096, 0, 0, 0, 0, 0, 0],
                    "ring 満杯後の分布飽和 (pop の decode 対称減算)"
                );
                assert_eq!(r.leo_latest_tick, 4097);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 137 EK-1】`leo_occupancy_tag` の境界純粋 pin (u8 wrap 255/256
    /// 跨ぎの確定値: usize as u8 は mod 256 truncate 静 semantics)。
    #[test]
    fn leo_occupancy_tag_bounds_strict() {
        let cases: [(usize, u8); 10] = [
            (0, 1),
            (1, 1),
            (3, 3),
            (7, 7),
            (8, 7),
            (255, 7),
            (256, 1),        // 256 mod 256 = 0 → max(1) → 1 (wrap の確定文書化)
            (258, 2),        // 258 mod 256 = 2
            (65535, 7),      // 65535 mod 256 = 255 → min(7) = 7
            (usize::MAX, 7), // MAX mod 256 = 255 → 7
        ];
        for (len, want) in cases {
            assert_eq!(leo_occupancy_tag(len), want, "len={len} → tag {want}");
        }
    }
}
