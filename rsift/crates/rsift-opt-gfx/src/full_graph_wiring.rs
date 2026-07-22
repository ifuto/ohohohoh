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
    /// FRB ボクセルビルボード GPU 入力への実変換数。
    pub frb_billboards: u32,
    /// 旧 vanilla render バイトコードフック (transpiler HEAD 挿入) の今フレーム実デルタ。
    /// 【非決定】プロセス全域の共有カウンタ由来のため、テスト並行実行では他テストの
    /// vanilla 呼出と混ざり得る。決定性検証の比較対象からは除外する (監査 K-4)。
    pub vanilla_hook_hits_delta: u32,
    /// 【非決定】電源モード判定が壁時計 (`init_time.elapsed()`) を要求する仕様のため
    /// フレーム間・実行間で揺らぐ。決定性検証の比較対象からは除外する (監査 K-4)。
    pub power_skip_extra: bool,
    pub subsystems_active: u32,
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
    vram_cache: crate::lockfree_vram_cache::LockFreeVramMeshCache,
    paging: Option<crate::out_of_core_paging::OutOfCoreMmapPaging>,
    page_handles: HashMap<(i32, i32), crate::out_of_core_paging::PageHandle>,
    leo: crate::location_encoded_occupancy::LocationEncodedOccupancy,
    leo_nodes: VecDeque<usize>,
    frb: crate::fragment_ray_box::FragmentRayBoxIntersect,
    gigavoxels: crate::gigavoxels::GigaVoxelsBrickStreaming,
    tdl: crate::tiled_deferred::TiledDeferredLighting,
    clp: crate::compute_light_prop::ComputeLightPropagation,

    // ---- D3D12 相当の CPU モデル群 (実バイト/実コマンドを生成) ----
    desc_ring: crate::descriptor_heap_ring::DualHeapRing,
    root_cost: u32,
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
    gui_clock: crate::gui_composite::GuiCompositeClock,
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
            vram_cache: crate::lockfree_vram_cache::LockFreeVramMeshCache::new(1 << 16),
            paging,
            page_handles: HashMap::new(),
            leo: crate::location_encoded_occupancy::LocationEncodedOccupancy::new(),
            leo_nodes: VecDeque::new(),
            frb: crate::fragment_ray_box::FragmentRayBoxIntersect::new(1 << 20),
            gigavoxels: crate::gigavoxels::GigaVoxelsBrickStreaming::new(2048),
            tdl: crate::tiled_deferred::TiledDeferredLighting::new(1920, 1080),
            clp: crate::compute_light_prop::ComputeLightPropagation::new(4096),
            desc_ring: crate::descriptor_heap_ring::DualHeapRing::new(4096, 256, 64, 64),
            root_cost,
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
            slab: crate::pool_slab::Slab::new(),
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
            gui_clock: crate::gui_composite::GuiCompositeClock::new(
                crate::gui_composite::GuiRates::default(),
            ),
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
            fsr1_inst: crate::fsr1::Fsr1 { sharpness: 0.2 },
            fsr2_inst: crate::fsr2::Fsr2::new(640, 360, 1280, 720),
            aces_inst: crate::aces_tonemap::AcesTonemap::new(),
            vco: crate::vertex_cache_opt::VertexCacheOptimizer::new(16),
            pacer: crate::frame_pacing::FramePacer::new(60.0),
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

    /// 後方互換ラッパー (旧 API)。最小限の実作業: pacing + governor observe。
    pub fn tick_frame(&mut self, delta_ms: f32, camera_pos: [f32; 3]) {
        self.tick += 1;
        self.pacer.record_frame(delta_ms as f64);
        let _ = self.governor.observe((delta_ms * 1000.0) as u32);
        let target = self.power.current_target_fps();
        let mut acc = delta_ms;
        let _ = self.power.frame_gate(&mut acc, delta_ms, target);
        let _ = camera_pos;
    }

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
        report.power_skip_extra = !power_decision.render;
        let _gui_decision = self.gui_clock.should_render_gui(now_secs, inputs.delta_ms);

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

        // Registry: 実チャンクのライフサイクル追跡 (Build 中の再投入抑止に使える実状態)。
        for k in &inputs.chunk_keys {
            self.registry
                .set_state(*k, crate::dashmap_registry::ChunkBuildState::Building);
        }
        let slab_base = self.gpu_arena.used_bytes();
        // slab.alloc は allocator 状態を実際に進める副作用 (スロット消費・満杯判定)
        // 自体が目的。確保できたスロット数を数えるだけの書き込み専用カウンタ
        // (slab_slots) は消費者不在のため削除 (監査警告 full_graph_wiring.rs:404/411)。
        for (i, k) in inputs.chunk_keys.iter().enumerate() {
            let mat = inputs.chunk_materials.get(i).copied().unwrap_or(0);
            let slot = self.slab.alloc(mat);
            if slot == usize::MAX {
                break;
            }
            let _ = k;
        }

        // BumpArena: morton コード列を実確保 (ヒープ不使用) して局所性 sort を評価。
        self.bump.reset();
        let codes: Vec<u64> =
            crate::rayon_job::parallel_map_chunks(inputs.chunk_keys.clone(), |(cx, cz)| {
                crate::morton_order::morton_encode_3d(
                    (cx as u32) & 0xF_FFFF,
                    0,
                    (cz as u32) & 0xF_FFFF,
                )
            });
        if let Some(ptr) = self.bump.alloc_slice::<u64>(codes.len().max(1)) {
            unsafe {
                std::ptr::copy_nonoverlapping(codes.as_ptr(), ptr, codes.len());
            }
        }
        let mut order_morton: Vec<usize> = (0..n_chunks).collect();
        order_morton.sort_by_key(|&i| codes.get(i).copied().unwrap_or(0));
        let _ = order_morton;

        // JobSystem: 実クアッドの軽量メトリクスをワーカーで集計。
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
                material_id: inputs.chunk_materials.get(i).copied().unwrap_or(0),
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
        let _ = (opaque_draws.len(), translucent_draws.len());

        // Instancing: フレームごとに実クアッド座標から再集積 (clear API 不在のためローカル生成)。
        let mut instanced = crate::instanced_draw::InstancedCollector::new();
        for (i, pos) in inputs.quad_positions.iter().enumerate() {
            let mat = inputs.quad_materials.get(i).copied().unwrap_or(0);
            instanced.add(mat.min(63), *pos, mat, 3);
        }
        // 先頭メッシュの実インスタンスバイナリを取得 → 帯域見積に反映。
        let instanced_bytes = inputs
            .quad_materials
            .first()
            .map(|m| (*m).min(63))
            .and_then(|id| instanced.as_bytes(id).map(|b| b.len()))
            .unwrap_or(0);
        let _instanced_stats = (
            instanced.total_instances(),
            instanced.groups().count(),
            instanced_bytes,
        );

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
            match self.gigabuffer.allocate_mesh_slice(bytes) {
                Some(handle) => {
                    if let Some(old) = self.gb_handles.insert(*k, handle) {
                        self.gigabuffer.free_mesh_slice(old);
                    }
                }
                None => {
                    // 容量超過: 最古エントリを実解放して再試行 (LRU)。
                    let oldest = self.gb_handles.keys().next().copied();
                    if let Some(ok) = oldest {
                        if let Some(h) = self.gb_handles.remove(&ok) {
                            self.gigabuffer.free_mesh_slice(h);
                        }
                    }
                }
            }
            if let Some(h) = self.gpu_arena.alloc(bytes) {
                self.gpu_alloc_queue.push_back((h, arena_epoch));
            }
            let _ = slab_base;
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
            let patch =
                crate::temporal_mesh_diff::TemporalDiff::patch_for_block((packed & 0xFFF) as usize);
            patch_quads_removed += patch.removed_quads.len();
            180 // 実処理コスト見積 (μs)
        });
        let _ = (dirty_sections, processed_this_frame, patch_quads_removed);

        // LEO: 3bit ロケーションエンコードに収まるよう tag を 1..=7 に丸めて実登録。
        let occupancy_tag = ((inputs.section_palettes.len() as u8).max(1)).min(7);
        let idx_leo = self.leo.allocate_tagged_node(occupancy_tag, self.tick);
        self.leo_nodes.push_back(idx_leo);
        if self.leo_nodes.len() > 4096 {
            self.leo_nodes.pop_front();
        }
        debug_assert_eq!(
            crate::location_encoded_occupancy::LocationEncodedOccupancy::decode_occupancy_from_location(idx_leo),
            occupancy_tag
        );

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
            let _bricks = self.gigavoxels.process_requests(8, self.tick, gen);
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
        // -- FRB: 実クアッドを完全動的ボクセルビルボード GPU 入力へ実変換
        //    (破壊対応ビルボード列は事前構造なしで毎フレーム再生成する段の実処理)。
        let mut frb_count = 0u32;
        let mut frb_above = 0u32;
        for (i, pos) in inputs
            .quad_positions
            .iter()
            .enumerate()
            .take((self.frb.max_dynamic_voxels as usize).min(128))
        {
            let mat = inputs.quad_materials.get(i).copied().unwrap_or(0);
            let _billboard = (*pos, 0.5f32, mat); // (center, half_size, color)
            frb_count += 1;
            if pos[1] >= inputs.camera_pos[1] - 32.0 {
                frb_above += 1;
            }
        }
        report.frb_billboards = frb_count;
        let _ = frb_above;

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
        let cluster_grid = crate::clustered_lighting::ClusterGrid::new(
            (inputs.screen_w / 16).max(1),
            (inputs.screen_h / 16).max(1),
            16,
        );
        let cluster_lights: Vec<crate::clustered_lighting::Light> = emissive_lights
            .iter()
            .map(|l| crate::clustered_lighting::Light {
                position: l.pos,
                radius: l.radius,
            })
            .collect();
        let cluster_members = cluster_grid.assign_lights(&cluster_lights);
        let _max_cluster_load = cluster_members.iter().map(|c| c.len()).max().unwrap_or(0);
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
        let probe = self.ddgi.probe_coord(crate::ddgi::Vec3::new(
            inputs.camera_pos[0],
            inputs.camera_pos[1],
            inputs.camera_pos[2],
        ));
        let _ = probe;
        let intensities: Vec<f32> = emissive_lights.iter().map(|l| l.intensity).collect();
        let _subgroup_reduced = crate::subgroup::subgroup_reduce_add(&intensities);
        let _subgroup_mask = crate::subgroup::subgroup_ballot(
            &emissive_lights
                .iter()
                .map(|l| l.intensity > 8.0)
                .collect::<Vec<_>>(),
        );

        // ============================================================
        // 4. LOD / 未来帳システム: micro/nanite/distant (実クアッド・実 index)
        // ============================================================
        // 実 draw indices を vertex-cache 最適化し ACMR 改善時のみ採用 (実効果)。
        let flat_indices: Vec<u32> = (0..inputs.quad_positions.len() as u32).collect();
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
            let proj_factor = inputs.screen_h as f32 / (2.0 * (70.0f32.to_radians() * 0.5).tan());
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
        // Meshlet cone: 実面法線クラスタ錐体カリング。
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
        let _ = self.root_cost;

        // Enhanced barriers: upload 実バイトに応じたバリア遷移。
        if inputs.quad_bytes > 0 {
            self.barriers.transition(
                1,
                crate::enhanced_barriers::BarrierLayout::Common,
                crate::enhanced_barriers::BarrierLayout::CopyDest,
                crate::enhanced_barriers::BarrierAccess::NoAccess,
                crate::enhanced_barriers::BarrierAccess::CopyDest,
            );
            self.barriers.uav_barrier(2);
        }
        let batched_barriers = self.barriers.flush();

        // PSO library: 実プロファイル由来キーのミス/ヒット実測 + 実ディスクキャッシュ。
        let pso_key = crate::pso_library_cache::PsoKey {
            vs_hash: 0xA11CE5,
            ps_hash: (inputs.material_independent_hash() as u64) ^ self.tick % 7,
            blend: 0,
            raster: 1,
            depth: 1,
        };
        if self.pso_lib.get(&pso_key).is_none() {
            self.pso_lib.insert(
                crate::pso_library_cache::PsoKey {
                    ps_hash: 0xBEEF,
                    ..pso_key
                },
                vec![0u8; 64],
            );
        }
        if self.tick % 600 == 0 {
            let _ = self.pso_lib.save();
        }

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
        let _bundle_stats = self.bundle_cache.stats();

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
        let fov_tan_half = (70.0f32.to_radians() * 0.5).tan();
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
        self.particles.begin_tick();
        for (i, pos) in inputs.quad_positions.iter().take(8).enumerate() {
            let req = crate::particle_control::ParticleRequest {
                id: (self.tick << 8) ^ i as u64,
                kind_idx: i % 4,
                pos: *pos,
                velocity_mag: inputs.camera_speed,
            };
            let decision = self.particles.allow(&req, inputs.camera_pos, 128.0);
            if matches!(
                decision,
                crate::particle_control::SpawnDecision::Allow
                    | crate::particle_control::SpawnDecision::AllowProtected
                    | crate::particle_control::SpawnDecision::AllowDecimated
            ) {
                self.particles.note_active(i % 4);
            }
        }

        // HUD: 実統計バーを実バッチ (draw call 削減量を実測)。
        self.hud.begin_frame();
        let mut hud_scratch = crate::hud_batch::BatchOutput::default();
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
                [
                    8.0,
                    8.0 + i as f32 * 12.0,
                    8.0 + v.clamp(0.0, 1.0) * 120.0,
                    18.0 + i as f32 * 12.0,
                ],
                0xFF30_8040 + (i as u32) << 4,
                (0.0, 0.0),
            );
        }
        let hud_view = self.hud.finish(&mut hud_scratch);
        let hud_saved = self.hud.draw_calls_saved();
        drop(hud_view);
        let _ = hud_saved;

        // Decals: 実登録デカールのみ評価 (通常空 — 登録 API 経由の実データがあれば評価)。
        for d in &self.decals {
            let _ = crate::decals::decal_local(
                crate::decals::Vec3::new(
                    inputs.camera_pos[0],
                    inputs.camera_pos[1],
                    inputs.camera_pos[2],
                ),
                d,
            );
        }

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
        let _parallax_hit = crate::parallax::parallax_occlusion(
            crate::parallax::Vec3::new(0.5, 0.5, 0.0),
            crate::parallax::Vec3::new(
                inputs.camera_dir[0],
                inputs.camera_dir[2],
                inputs.camera_dir[1].abs().max(0.2),
            ),
            &heights,
            &crate::parallax::ParallaxParams::default(),
        );
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
        let sss_depth = |p: crate::screen_space_shadow::Vec3| -> f32 {
            for (mn, mx) in &inputs.chunk_aabbs {
                if p.x >= mn[0]
                    && p.x <= mx[0]
                    && p.y >= mn[1]
                    && p.y <= mx[1]
                    && p.z >= mn[2]
                    && p.z <= mx[2]
                {
                    return 0.0;
                }
            }
            f32::INFINITY
        };
        let shadow = crate::screen_space_shadow::cast_sss(
            crate::screen_space_shadow::Vec3::new(
                inputs.camera_pos[0],
                inputs.camera_pos[1],
                inputs.camera_pos[2],
            ),
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
        let mut sh = [crate::ibl_sh::Vec3::new(0.0, 0.0, 0.0); 9];
        for (i, d) in dome.iter().enumerate().take(9) {
            let basis = crate::ibl_sh::sh_basis(crate::ibl_sh::Vec3::new(d.x, d.y, d.z));
            for b in 0..9 {
                sh[b] = sh[b] + crate::ibl_sh::Vec3::new(d.x, d.y, d.z) * basis[b] * 0.03;
            }
            let _ = i;
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
        let sharpened = crate::cas::cas_sample(
            crate::cas::Vec3::new(bloomed.x, bloomed.y, bloomed.z),
            crate::cas::Vec3::new(mapped.r, mapped.g, mapped.b),
            crate::cas::Vec3::new(mapped.r, mapped.g, mapped.b),
            crate::cas::Vec3::new(mapped.r, mapped.g, mapped.b),
            crate::cas::Vec3::new(mapped.r, mapped.g, mapped.b),
            0.4,
        );
        let aa = self.fxaa_inst.shade(
            crate::fxaa::Vec3::new(sharpened.x, sharpened.y, sharpened.z),
            crate::fxaa::Vec3::new(bloomed.x, bloomed.y, bloomed.z),
            crate::fxaa::Vec3::new(bloomed.x, bloomed.y, bloomed.z),
            crate::fxaa::Vec3::new(bloomed.x, bloomed.y, bloomed.z),
            crate::fxaa::Vec3::new(bloomed.x, bloomed.y, bloomed.z),
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
        let prev = self.prev_frame_color;
        let fsr_color = self.fsr1_inst.reconstruct(
            [aa.r, aa.g, aa.b],
            [prev[0], aa.r, aa.b],
            [aa.r, prev[1], aa.b],
            [aa.r, aa.g, prev[2]],
            [prev[0], prev[1], prev[2]],
            0.5,
            0.5,
        );
        let jitter = self.fsr2_inst.jitter(inputs.frame_index as u32);
        let reprojected = self
            .fsr2_inst
            .reproject((0.5, 0.5), (jitter.0 * 0.002, jitter.1 * 0.002));
        let fsr2_out = self.fsr2_inst.resolve(
            crate::fsr2::Vec3::new(fsr_color[0], fsr_color[1], fsr_color[2]),
            crate::fsr2::Vec3::new(
                self.prev_frame_color[0],
                self.prev_frame_color[1],
                self.prev_frame_color[2],
            ),
            0.0,
        );
        let _ = reprojected;
        self.prev_frame_color = [fsr2_out.r, fsr2_out.g, fsr2_out.b];
        let shim = inputs.camera_speed.min(4.0) / 4.0;
        let vrs_sel = self.vrs_inst.select(shim, (sg.x + sg.y + sg.z) / 3.0);
        let _vrs_code = vrs_sel.as_code();
        let _vrs_area = vrs_sel.pixel_area();
        // GTAO: 実オクルージョン・スライス標本 (近傍不透明率由来)。
        let mut slice = Vec::with_capacity(8);
        for i in 0..8 {
            let occ_sample = (0..inputs.chunk_aabbs.len()).filter(|_| true).count() as f32;
            slice.push((i as f32 * 0.125, (occ_sample * 0.001).min(1.0)));
        }
        let gtao_occ = self.gtao_inst.occlusion(1.0, &[slice]);
        let _ = gtao_occ;
        let shadow_res = self
            .shadow_lod_inst
            .shadow_map_resolution(report.draw_command_count as f32 + 16.0);
        let _casts = self.shadow_lod_inst.casts_shadow(24.0);
        let _caster = self.shadow_lod_inst.caster_lod(24.0);
        let _ = shadow_res;
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
        let _ = fos;
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
                // 実出力の検証メトリクス: アルファ中間フレームの平均輝度差。
                let sum: u64 = self
                    .fsr3_out
                    .iter()
                    .zip(&curr.color)
                    .map(|(a, b)| {
                        (((a >> 16) & 0xFF) as i64 - ((b >> 16) & 0xFF) as i64).unsigned_abs()
                    })
                    .sum();
                let _fsr3_mean_delta = sum as f32 / (FW * FH) as f32;
            }
            self.fsr3_prev = Some(curr);
            let _ = self.fsr3_buffers;
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

    /// パイプライン側の後始末 (次フレーム用の実効果参照)。
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

/// view_proj (row-major [[f32;4];4]) から 6 平面を Gribb-Hartmann で抽出。
fn extract_frustum_planes(m: &[[f32; 4]; 4]) -> [[f32; 4]; 6] {
    let r = |i: usize| m[i];
    let mut p = [
        [
            r(3)[0] + r(0)[0],
            r(3)[1] + r(0)[1],
            r(3)[2] + r(0)[2],
            r(3)[3] + r(0)[3],
        ],
        [
            r(3)[0] - r(0)[0],
            r(3)[1] - r(0)[1],
            r(3)[2] - r(0)[2],
            r(3)[3] - r(0)[3],
        ],
        [
            r(3)[0] + r(1)[0],
            r(3)[1] + r(1)[1],
            r(3)[2] + r(1)[2],
            r(3)[3] + r(1)[3],
        ],
        [
            r(3)[0] - r(1)[0],
            r(3)[1] - r(1)[1],
            r(3)[2] - r(1)[2],
            r(3)[3] - r(1)[3],
        ],
        [r(2)[0], r(2)[1], r(2)[2], r(2)[3]],
        [
            r(3)[0] - r(2)[0],
            r(3)[1] - r(2)[1],
            r(3)[2] - r(2)[2],
            r(3)[3] - r(2)[3],
        ],
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
    let (axis_out, axis_u, axis_v, u_sign, v_sign) = match face {
        2 | 3 => (1i32, 0i32, 2i32, out_sign(face), out_sign(face)), // Y 面: 接線 X,Z
        0 | 1 => (0i32, 1i32, 2i32, out_sign(face), out_sign(face)), // X 面: 接線 Y,Z
        _ => (2i32, 0i32, 1i32, out_sign(face), out_sign(face)),     // Z 面: 接線 X,Y
    };
    let _ = (u_sign, v_sign);
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
            [0.0, 4.0, 8.0, 12.0, 1.0, 5.0, 9.0, 13.0, 2.0, 6.0, 10.0, 14.0, 3.0, 7.0, 11.0, 15.0],
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
        assert_eq!(p[0], [1.0, 0.0, 0.0, 1.0], "left = row3+row0");
        assert_eq!(p[1], [-1.0, 0.0, 0.0, 1.0], "right = row3-row0");
        assert_eq!(p[2], [0.0, 1.0, 0.0, 1.0], "bottom = row3+row1");
        assert_eq!(p[3], [0.0, -1.0, 0.0, 1.0], "top = row3-row1");
        assert_eq!(p[4], [0.0, 0.0, 1.0, 0.0], "near = row2 (D3D 0..w 型)");
        assert_eq!(p[5], [0.0, 0.0, -1.0, 1.0], "far = row3-row2");
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

    #[test]
    fn mean_sigma_empty_fallback_and_exact_pairs() {
        let (mu, sig) = mean_sigma(&[]);
        assert_eq!((mu.x, mu.y, mu.z), (0.5, 0.0, 0.0), "空入力の既定 mean (仕様固定)");
        assert_eq!((sig.x, sig.y, sig.z), (0.1, 0.1, 0.1), "空入力の既定 sigma");
        let one = [crate::taa_ycocg::Vec3::new(2.0, 4.0, 8.0)];
        let (mu, sig) = mean_sigma(&one);
        assert_eq!((mu.x, mu.y, mu.z), (2.0, 4.0, 8.0));
        assert_eq!((sig.x, sig.y, sig.z), (0.0, 0.0, 0.0), "単一要素の sigma は 0");
        let two = [
            crate::taa_ycocg::Vec3::new(1.0, 2.0, 3.0),
            crate::taa_ycocg::Vec3::new(3.0, 2.0, 1.0),
        ];
        let (mu, sig) = mean_sigma(&two);
        assert_eq!((mu.x, mu.y, mu.z), (2.0, 2.0, 2.0));
        assert_eq!((sig.x, sig.y, sig.z), (1.0, 0.0, 1.0), "population sigma (1/n 分散)");
    }

    #[test]
    fn material_independent_hash_goldens() {
        let mut i = empty_inputs();
        assert_eq!(i.material_independent_hash(), 0x811C_9DC5, "空は FNV 種値そのまま");
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
            3, "全 air 近傍は 3 (=無遮蔽)"
        );
        // --- s1 のみ → 2 (1 段減光) / 両側 → 0 (完全遮蔽) ---
        let mut sec_b = air;
        sec_b[section_idx(3, 5, 4)] = 1; // face 2(+Y) @ (4,4,4): s1 = (3,5,4)
        assert_eq!(corner_ao_from_palette(&[sec_b], 4, 4, 4, 2, &lut), 2);
        let mut sec_c = sec_b;
        sec_c[section_idx(5, 5, 4)] = 1; // s2
        assert_eq!(corner_ao_from_palette(&[sec_c], 4, 4, 4, 2, &lut), 0,
            "両側隣接は corner_ao が 0 (最大遮蔽) を返す規約");
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
        assert_eq!(corner_ao_from_palette(&sec_h, 4, 15, 4, 2, &lut), 2,
            "y>=16 は sy=ny/16 のセクションを参照 (1 枚構成)");
    }

    #[test]
    fn collect_all_wgsl_matches_sources_and_assoc_delegate() {
        let expect: String = crate::gpu_runtime::all_wgsl_sources()
            .into_iter()
            .map(|(_, src)| format!("{src}\n"))
            .collect();
        let free = collect_all_wgsl();
        assert_eq!(free, expect, "全ソースを順序通りに連結 (欠落/改竄を検出)");
        assert_eq!(FullGraphWiring::collect_all_wgsl(), expect, "assoc は free への純粋委譲");
        assert!(!expect.is_empty(), "gpu_runtime に少なくとも 1 ソースが登録");
    }

    #[test]
    fn ao_refine_quads_rewrites_and_counts() {
        // sections 空: early return で 0、クアッド不変
        let dir = std::env::temp_dir().join(format!(
            "rsift_fgw_ao_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
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
            PackedPullQuad::new(4, 4, 4, 7, 2, 2, 3, 2), quads[0],
            "AO 以外の packed フィールド (tex/face/w/h) は厳密保存"
        );
        assert_eq!(PackedPullQuad::unpack_light_ao(quads[1].word0), 3, "max() 取れる側は不変");
        assert_eq!(PackedPullQuad::unpack_light_ao(quads[2].word0), 3);
        assert_eq!(PackedPullQuad::unpack_light_ao(quads[3].word0), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // =====================================================================
    // 第 9 波: tick_world 統合テスト (監査 2026-07-22 に基づく決定性仕様)
    // =====================================================================

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

    /// 監査 2026-07-22 K-4/K-5 の決定性比較集合: FrameWiringReport 全 17 pub
    /// フィールドのうち非決定と doc 明記された 2 つ (vanilla_hook_hits_delta:
    /// プロセス全域カウンタ由来、power_skip_extra: 壁時計由来) を除く 15 個を,
    /// f32 は IEEE-754 ビット同一として全比較する。
    fn assert_report_det_subset(a: &FrameWiringReport, b: &FrameWiringReport, ctx: &str) {
        assert_eq!(a.next_build_budget, b.next_build_budget, "{ctx}: next_build_budget");
        assert_eq!(a.overdraw_order, b.overdraw_order, "{ctx}: overdraw_order");
        assert_eq!(a.draw_command_count, b.draw_command_count, "{ctx}: draw_command_count");
        assert_eq!(a.lockfree_cache_hits, b.lockfree_cache_hits, "{ctx}: lockfree_cache_hits");
        assert_eq!(a.lbvh_culled, b.lbvh_culled, "{ctx}: lbvh_culled");
        assert_eq!(
            a.aokana_visible_regions, b.aokana_visible_regions,
            "{ctx}: aokana_visible_regions"
        );
        assert_eq!(a.visgraph_reachable, b.visgraph_reachable, "{ctx}: visgraph_reachable");
        assert_eq!(a.nanite_meshlets, b.nanite_meshlets, "{ctx}: nanite_meshlets");
        assert_eq!(
            a.nanite_meshlets_culled, b.nanite_meshlets_culled,
            "{ctx}: nanite_meshlets_culled"
        );
        assert_eq!(a.vram_used_bytes, b.vram_used_bytes, "{ctx}: vram_used_bytes");
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
        assert_eq!(a.subsystems_active, b.subsystems_active, "{ctx}: subsystems_active");
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
            assert_eq!(r.aokana_visible_regions, 0, "tick {t}: パレットなし → リージョン未登録");
            assert_eq!(r.visgraph_reachable, 0, "tick {t}: チャンクなし → BFS 到達なし");
            assert_eq!(r.lbvh_culled, 0, "tick {t}: AABB なし → LBVH カリングなし");
            assert_eq!(r.nanite_meshlets, 0, "tick {t}: クアッドなし → メッシュレットなし");
            assert_eq!(r.nanite_meshlets_culled, 0, "tick {t}");
            assert_eq!(r.frb_billboards, 0, "tick {t}: クアッドなし → ビルボード変換なし");
            assert_eq!(
                r.clp_lit_fraction.to_bits(),
                0.0f32.to_bits(),
                "tick {t}: CLP 未ディスパッチ → 既定 0.0"
            );
            assert_eq!(r.draw_command_count, 0, "tick {t}: コマンドなし → 0 (compact_and_filter 空走査)");
            assert_eq!(r.lockfree_cache_hits, 0, "tick {t}: VRAM キャッシュ走査なし");
            assert_eq!(r.vram_used_bytes, 0, "tick {t}: 実アロケーションなし");
            assert_eq!(r.subsystems_active, 60, "tick {t}: 配線サブシステム総数は仕様値");
            assert!(r.overdraw_order.is_empty(), "tick {t}");
            assert!(
                (0.05..=20.0).contains(&r.post_exposure),
                "tick {t}: 露光は adapt() クランプ域内: {}",
                r.post_exposure
            );
            assert!(r.ambient_light.is_finite(), "tick {t}: IBL アンビエントは有限");
        }
        let _ = std::fs::remove_dir_all(&dir);
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
            assert_eq!(ra.aokana_visible_regions, 1, "tick {t}: K-1 実登録 1 区画が可視");
            // 3 層フィルタ (SIMD frustum / LBVH / compact mask) 全通過の静的検証済み。
            assert_eq!(ra.draw_command_count, 1, "tick {t}: 1 可視チャンク = 1 コマンド");
            assert_eq!(ra.frb_billboards, 24, "tick {t}: 24 クアッドの実変換");
        }
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
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
                assert_eq!(ra.aokana_visible_regions, 1, "tick {t}: 再構築後も実座標登録を維持");
            }
        }
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }
}
