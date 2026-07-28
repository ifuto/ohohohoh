//! Rsift render pipeline — 必須/推奨 + Feather weak-PC (no resolution scaling).

use crate::adaptive_shading::AdaptiveShadingController;
use crate::billboard_lod::BillboardLodSelector;
use crate::binary_greedy_meshing::{
    demo_column_palettes, mesh_chunk_column, mesh_chunk_column_pull_world, SectionPalette,
    SECTION_SIZE,
};
use crate::chunk_cull::{ChunkCullPass, CullVerdict};
use crate::chunk_mesh::{BuiltChunkMesh, MultithreadedChunkBuilder};
use crate::cpu_occlusion::CpuMaskedOccluder;
use crate::depth_prepass::DepthPrepassPlanner;
use crate::diff_mesh::DiffMeshUpdater;
use crate::drs::DynamicResolutionScaler;
use crate::eco_render::{EcoRegionRenderer, SodiumComparison};
use crate::entity_tick_lod::EntityTickScheduler;
use crate::frame_reuse::{FrameReuseCache, ReuseEncoding};
use crate::full_graph_wiring::{FrameWiringInputs, FullGraphWiring};
use crate::gpu_culling::ChunkBoundingBox;
use crate::hzb_2d::CameraState;
use crate::leaf_fast_path::apply_leaf_fast_path;
use crate::light_cache::LightPropagationCache;
use crate::lod_hybrid::LodHybridSelector;
use crate::low_spec_stack::{
    adaptive_mesh_interval, apply_cheap_ao, apply_solid_interior_cull, emit_lod_box_quads,
    filter_quads_by_face_mask, flora_should_skip_detail, frustum_culled, section_is_empty_occ,
    section_occupancy, sort_nearest_first, sort_quads_by_material, truncate_quad_budget,
    FaceEmitMask, LowSpecPlan, PullGenerationCache,
};
use crate::mesh_cache::MeshDiskCache;
use crate::noise_upsample::{benchmark_upsample, column_palettes_upsampled, NoiseUpsampleConfig};
use crate::occlusion_complete::SoftwareOcclusion;
use crate::persistent_vbo_pool::PersistentVboPool;
use crate::pull_mesh::PullBuiltMesh;
use crate::render_graph::RenderGraphScheduler;
use crate::section_rle::occupied_section_indices;
use crate::section_rle::RleSection;
use crate::software_tiling::SoftwareTileBinner;
use crate::spatial_hash::SpatialHashGrid3D;
use crate::svo::SparseVoxelOctree;
use crate::taa::LightweightTaa;
use crate::texture_budget::TextureBudget;
use crate::tick_render_split::FixedTickClock;
use crate::triple_buffer::TripleBuffer;
use crate::vertex_pool::VertexPool;
use crate::world_column_store::{TerrainFrameConstants, WorldColumnStore};
use rsift_api::{AdaptivePerfEngine, AdaptiveRenderProfile, EngineCaps, FeatherRenderConfig};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tracing::{debug, info};

static PIPELINE: OnceLock<Mutex<RsiftRenderPipeline>> = OnceLock::new();

#[derive(Debug, Default, Clone)]
pub struct FrameStats {
    pub chunks_built: u32,
    pub cache_hits: u32,
    pub visible_chunks: u32,
    pub draw_calls: u32,
    pub cpu_culled: u64,
    pub tiles_binned: u32,
    pub shading_skipped: u32,
    pub empty_culled: u32,
    pub visgraph_culled: u32,
    pub range_culled: u32,
    pub rle_palette_bytes: u64,
    pub svo_nodes_built: u32,
    pub frame_reuse_hits: u32,
    pub frame_reuse_misses: u32,
    pub frame_reuse_mem_kb: u32,
    pub pull_quads_built: u32,
    pub pull_verts_drawn: u32,
    pub pull_ssbo_bytes: u64,
    pub pull_vram_ratio: f32,
    pub noise_speedup: f32,
    pub persistent_vbo_mb: u32,
    pub mdi_chunk_draws: u32,
    pub pull_cache_hits: u32,
    pub frustum_culled: u32,
    pub soft_occluded: u32,
    pub lod_boxes: u32,
    pub interior_culled_voxels: u32,
    /// FullGraphWiring が当該フレームに実実行したサブシステム数。
    pub wiring_subsystems: u32,
    /// AO 精緻で実際に書き換わったプルクアッド数 (視覚効果の実測)。
    pub wiring_ao_refined_quads: u32,
}

pub struct RsiftRenderPipeline {
    pub profile: AdaptiveRenderProfile,
    pub feather: FeatherRenderConfig,
    pub game_dir: PathBuf,
    pub builder: MultithreadedChunkBuilder,
    pub cache: MeshDiskCache,
    pub vertex_pool: Option<VertexPool>,
    pub persistent_pool: Option<PersistentVboPool>,
    pub eco: EcoRegionRenderer,
    pub cpu_occluder: Option<CpuMaskedOccluder>,
    pub tile_binner: Option<SoftwareTileBinner>,
    pub shading: Option<AdaptiveShadingController>,
    pub lod: LodHybridSelector,
    pub texture_budget: TextureBudget,
    pub render_graph: RenderGraphScheduler,
    pub last_comparison: Option<SodiumComparison>,
    pub frame_stats: FrameStats,
    pub last_camera_speed: f32,
    pub cull_pass: ChunkCullPass,
    /// 【wave 157 FC §7 消化 20】chunk_cull::CullStats 第 2 帳簿:
    /// 旧は消費者ゼロ孤立 (捕捉 79)。両 verdict call site で apply 蓄積し、
    /// frame 末端で frame_stats arms と cross-field 照合する (検出力は
    /// frame_cull_stats_cross_invariants_strict + frame 末端 debug_assert)。
    pub cull_stats: crate::chunk_cull::CullStats,
    pub camera: CameraState,
    pub svo_cache: HashMap<(i32, i32), SparseVoxelOctree>,
    pub frame_reuse: FrameReuseCache,
    pub pull_meshes: Vec<PullBuiltMesh>,
    /// Latest pull-SSBO bytes uploaded to DX12 each frame.
    pub gpu_quad_bytes: Vec<u8>,
    pub diff_mesh: DiffMeshUpdater,
    pub upload_ring: TripleBuffer<Vec<u8>>,
    pub drs: DynamicResolutionScaler,
    pub taa: LightweightTaa,
    pub soft_occlusion: Option<SoftwareOcclusion>,
    pub tick_clock: FixedTickClock,
    pub flora_lod: BillboardLodSelector,
    pub depth_plan: DepthPrepassPlanner,
    pub world: WorldColumnStore,
    /// World section Y of the last meshed window base (for pull packing).
    mesh_section_y0: i32,
    pub low_spec: LowSpecPlan,
    pub pull_gen_cache: PullGenerationCache,
    pub entity_scheduler: EntityTickScheduler,
    pub spatial_hash: SpatialHashGrid3D,
    pub light_cache: LightPropagationCache,
    pub full_wiring: FullGraphWiring,
    /// Governor 由来の動的 build 予算 (前フレームの wiring レポートが実適用)。
    pub dynamic_build_budget: Option<u32>,
    /// front-to-back 可視順の build 優先度マップ (次フレームの build 順に実効果)。
    pub wiring_priority: std::collections::HashMap<(i32, i32), usize>,
    /// PowerPolicy が「余分な仕事をスキップせよ」と判定した実状態 (次フレームへ適用)。
    pub wiring_power_skip_extra: bool,
    /// 前フレームの実カメラ位置 (motion adaptive shading の実速度計測用 — M-1)。
    prev_camera_xyz: Option<[f32; 3]>,
    last_build: Option<ChunkBuildArtifacts>,
    tick: u64,
}

#[derive(Debug, Default)]
struct ChunkBuildArtifacts {
    full_mesh: Option<BuiltChunkMesh>,
    svo: Option<SparseVoxelOctree>,
    encoding: ReuseEncoding,
}

impl RsiftRenderPipeline {
    pub fn new(game_dir: &Path) -> Self {
        let hw = AdaptivePerfEngine::hardware();
        let profile = AdaptivePerfEngine::render_profile(hw);
        let feather = profile.feather.clone();
        info!(
            "[RenderPipeline] {}",
            AdaptivePerfEngine::render_profile_summary(&profile)
        );
        if let Some(caps) = EngineCaps::from_jvm_props() {
            info!("[RenderPipeline] engine caps: {}", caps.summary());
        }
        info!("[RenderPipeline] {}", feather.summary());
        if feather.enabled {
            info!(
                "[Feather] tile={}px merged_subpass={} pseudo_vrs={} lod_3tier={} tex={}",
                feather.tile_size_px,
                feather.merged_subpasses,
                feather.software_vrs_checkerboard,
                feather.lod_hybrid_3tier,
                TextureBudget::from_profile(true, feather.compressed_textures, feather.mipmap_bias)
                    .label()
            );
        }
        let texture_budget = TextureBudget::from_profile(
            feather.enabled,
            feather.compressed_textures,
            feather.mipmap_bias,
        );
        let render_graph =
            RenderGraphScheduler::new(feather.merged_subpasses, feather.minimal_barriers);
        let lod = LodHybridSelector::new(
            feather.lod_hybrid_3tier,
            feather.flat_palette_priority,
            feather.svo_far_only,
        );
        Self {
            profile: profile.clone(),
            feather: feather.clone(),
            game_dir: game_dir.to_path_buf(),
            builder: MultithreadedChunkBuilder::adaptive(),
            cache: MeshDiskCache::adaptive(game_dir),
            vertex_pool: if profile.vertex_pool_enabled && !profile.persistent_vbo_pool {
                Some(VertexPool::adaptive())
            } else {
                None
            },
            persistent_pool: if profile.persistent_vbo_pool {
                Some(PersistentVboPool::adaptive())
            } else {
                None
            },
            eco: EcoRegionRenderer::new(),
            cpu_occluder: None,
            tile_binner: None,
            shading: if feather.enabled {
                Some(AdaptiveShadingController::new(
                    feather.distance_adaptive_shading,
                    feather.motion_adaptive_shading,
                    feather.software_vrs_checkerboard,
                ))
            } else {
                None
            },
            lod,
            texture_budget,
            render_graph,
            last_comparison: None,
            frame_stats: FrameStats::default(),
            last_camera_speed: 0.0,
            cull_pass: ChunkCullPass::from_profile(&profile),
            cull_stats: crate::chunk_cull::CullStats::default(),
            camera: CameraState {
                y: 64.0,
                fov_y: 70.0_f32.to_radians(),
                aspect: 16.0 / 9.0,
                ..Default::default()
            },
            svo_cache: HashMap::new(),
            frame_reuse: FrameReuseCache::adaptive(feather.enabled),
            pull_meshes: Vec::new(),
            gpu_quad_bytes: Vec::new(),
            diff_mesh: DiffMeshUpdater::new(),
            upload_ring: TripleBuffer::new(),
            drs: DynamicResolutionScaler::for_tier_fps(profile.target_fps_cap),
            taa: if profile.lightweight_taa {
                LightweightTaa::default()
            } else {
                LightweightTaa {
                    enabled: false,
                    ..Default::default()
                }
            },
            soft_occlusion: if profile.software_occlusion {
                Some(SoftwareOcclusion::new(512, 512))
            } else {
                None
            },
            tick_clock: FixedTickClock::new(),
            flora_lod: BillboardLodSelector::for_tier_scale(match profile.tier {
                rsift_api::PerformanceTier::Minimal => 0.6,
                rsift_api::PerformanceTier::Low => 0.8,
                _ => 1.0,
            }),
            depth_plan: if matches!(profile.tier, rsift_api::PerformanceTier::High) {
                DepthPrepassPlanner::for_high_spec()
            } else {
                DepthPrepassPlanner::for_low_spec()
            },
            world: WorldColumnStore::new(),
            mesh_section_y0: 0,
            low_spec: {
                let plan = LowSpecPlan::for_profile(&profile);
                info!("[LowSpecStack] {}", plan.summary());
                plan
            },
            pull_gen_cache: PullGenerationCache::default(),
            entity_scheduler: EntityTickScheduler::default(),
            spatial_hash: SpatialHashGrid3D::new(16.0),
            light_cache: LightPropagationCache::new(),
            full_wiring: FullGraphWiring::new(game_dir),
            dynamic_build_budget: None,
            wiring_priority: std::collections::HashMap::new(),
            wiring_power_skip_extra: false,
            prev_camera_xyz: None,
            last_build: None,
            tick: 0,
        }
    }

    /// ingest された実セクション帯域を diff 追跡へダーティ登録 (wave 86 CJ-3)。
    /// 旧実装はカメラ帯 `mid_y-16..=mid_y+16` をマークしており、カメラと異なる
    /// 帯域のインジェスト更新が diff 追跡から抜け落ち (変更列が再メッシュ
    /// されない)、代わりに不要なカメラ帯が誤ダーティ化されていた。
    /// セクション中心ブロック (sy*16+8) で正確に 1 セクションのみをマーク
    /// (端ブロックでは mark_block_dirty の境界伝播が隣へ及ぶため)。
    fn note_ingested_sections(
        &mut self,
        cx: i32,
        cz: i32,
        base_section_y: i32,
        section_count: usize,
    ) {
        for s in 0..section_count {
            let sy = base_section_y + s as i32;
            self.diff_mesh.mark_block_dirty(cx, cz, sy * 16 + 8);
        }
    }

    /// CK-1 (wave 87): ワールド列 prune に追随する派生キャッシュ整合。
    /// 語彙は world_column_store::prune_outside と同一 (Chebyshev 半径、
    /// 境界含む)。旧実装は svo_cache が未 prune で、除去済み列の stale SVO
    /// が VCT プローブへ永久に供給され続け、世代キャッシュも滞留していた。
    fn prune_derived_caches(&mut self, center_cx: i32, center_cz: i32, radius: i32) {
        self.svo_cache.retain(|(cx, cz), _| {
            (*cx - center_cx).abs() <= radius && (*cz - center_cz).abs() <= radius
        });
        self.pull_gen_cache
            .prune_outside(center_cx, center_cz, radius);
    }

    /// CK-1 (wave 87): 列内容の再インジェスト置換に伴う派生無効化。
    /// 旧実装は disk cache/pull gen のみ無効化で、SVO キャッシュは stale
    /// のまま VCT プローブへ供給され続けた (次の SVO 再ビルドまで旧地形を
    /// 参照し続ける)。
    fn invalidate_derived_for_column(&mut self, cx: i32, cz: i32) {
        self.svo_cache.remove(&(cx, cz));
    }

    /// wiring へ供給する SVO を決定論的に 1 つ選択 (wave 86 CJ-4)。
    /// `HashMap::values().next()` は RandomState のプロセス毎ランダム順であり
    /// 「意味のある選択を任意要素に委ねる」CI-1 と同型の潜在非決定性だった
    /// (現行消費は VCT メトリクスで読み捨てのため実害は未発生)。最小キー固定。
    fn svo_for_wiring(&self) -> Option<&SparseVoxelOctree> {
        self.svo_cache
            .iter()
            .min_by_key(|(k, _)| **k)
            .map(|(_, v)| v)
    }

    /// CJ-4 の借用移譲版: 選択と同時に所有クローン。
    /// (frame() 後段の inputs ライフタイムと svo_cache 借用を切るため)
    fn svo_for_wiring_owned(&self) -> Option<SparseVoxelOctree> {
        self.svo_for_wiring().cloned()
    }

    fn prepare_column(&self, cx: i32, cz: i32) -> (Vec<SectionPalette>, Vec<RleSection>, u64, i32) {
        if let Some((mut sections, section_y0)) = self.world.column_for_mesh(cx, cz) {
            for palette in &mut sections {
                if self.low_spec.occupancy_bitmask {
                    let occ = section_occupancy(palette);
                    if section_is_empty_occ(&occ) {
                        *palette = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
                        continue;
                    }
                }
                if self.low_spec.solid_interior_cull {
                    apply_solid_interior_cull(palette);
                }
                // 【注】live 路は `low_spec.leaf_fast_path` のみを見て常時 true 適用
                // (`profile.leaf_fast_path` は見ない)。フォールバック (demo/ノイズ) 路は
                // 両フラグの OR を有効条件にする — 非対称の記録のみ (監査 2026-07-22 M-2)。
                if self.low_spec.leaf_fast_path {
                    apply_leaf_fast_path(palette, true);
                }
            }
            let rle: Vec<RleSection> = sections.iter().map(RleSection::encode).collect();
            return (sections, rle, 0, section_y0);
        }
        // Liveデータ無し時のフォールバック: 従来は空セクションを返すスタブだったが、スタブ禁止方針により常に実際に描画可能なデモ/ノイズ地形を生成する。
        // デモ環境変数RSIFT_ENABLE_DEMO_RENDERの有無に関わらず、少なくとも見えるメッシュを保証し、ベンチや低スペPCのワールド無し起動でもレンダーパスが完全に配線される。
        let mut sections = if self.profile.noise_upsampling {
            let cfg = NoiseUpsampleConfig::for_chunk(cx, cz);
            column_palettes_upsampled(cx, cz, &cfg)
        } else {
            demo_column_palettes(cx, cz)
        };
        for palette in &mut sections {
            if self.low_spec.solid_interior_cull {
                apply_solid_interior_cull(palette);
            }
            apply_leaf_fast_path(
                palette,
                self.low_spec.leaf_fast_path || self.profile.leaf_fast_path,
            );
        }
        let rle: Vec<RleSection> = sections.iter().map(RleSection::encode).collect();
        let raw_bytes = (sections.len() * 4096 * 2) as u64;
        let rle_bytes: u64 = rle.iter().map(|s| s.to_bytes().len() as u64).sum();
        let saved = raw_bytes.saturating_sub(rle_bytes);
        (sections, rle, saved, 0)
    }

    pub fn build_chunk(
        &mut self,
        cx: i32,
        cz: i32,
        section_y: i32,
        dist_blocks: f32,
        sections: Option<&[SectionPalette]>,
        rle: Option<&[RleSection]>,
    ) -> BuiltChunkMesh {
        if let Some(cached) = self.cache.get(cx, cz, section_y) {
            self.frame_stats.cache_hits += 1;
            return self
                .lod
                .simplify_mesh(cached, self.lod.tier_for_distance(dist_blocks));
        }
        let (sections, rle, saved, section_y0) = match (sections, rle) {
            (Some(s), Some(r)) => (s.to_vec(), r.to_vec(), 0u64, self.mesh_section_y0),
            _ => self.prepare_column(cx, cz),
        };
        self.mesh_section_y0 = section_y0;
        if saved > 0 {
            self.frame_stats.rle_palette_bytes += saved;
        }

        let tier = self.lod.tier_for_distance(dist_blocks);

        if self.lod.use_svo_encoding(tier) {
            let svo = SparseVoxelOctree::from_column(&sections);
            self.frame_stats.svo_nodes_built += svo.node_count;
            self.svo_cache.insert((cx, cz), svo.clone());
            self.last_build = Some(ChunkBuildArtifacts {
                full_mesh: None,
                svo: Some(svo),
                encoding: ReuseEncoding::Svo,
            });
            return BuiltChunkMesh {
                chunk_x: cx,
                chunk_z: cz,
                vertices: vec![],
                indices: vec![],
            };
        }

        let mesh = if self.profile.binary_greedy_meshing {
            mesh_chunk_column(&sections, cx, cz, self.profile.chunk_face_culling)
        } else {
            self.builder_simple(cx, cz)
        };
        let want_pull = self.profile.vertex_pull_4byte
            || self.world.has_live_data
            || std::env::var("rsift.render.backend")
                .map(|s| s.contains("dx12"))
                .unwrap_or(false);
        if want_pull {
            let origin = self.world.mesh_origin;
            let mut pull = mesh_chunk_column_pull_world(
                &sections,
                cx,
                cz,
                section_y0,
                origin[0],
                origin[1],
                origin[2],
                self.profile.chunk_face_culling,
            );
            if self.low_spec.camera_face_mask {
                let mask = FaceEmitMask::from_camera_yaw_pitch(self.camera.yaw, self.camera.pitch);
                filter_quads_by_face_mask(&mut pull.quads, mask);
            }
            if self.low_spec.cheap_directional_ao {
                apply_cheap_ao(&mut pull.quads);
            }
            // 実効果: ao_bake コーナー AO を実パレット近傍から計算し light_ao を精細化
            // (cheap_face_ao 未満にはしない — 視覚差のある実配線)。
            let ao_changed = self.full_wiring.ao_refine_quads(&mut pull.quads, &sections);
            self.frame_stats.wiring_ao_refined_quads += ao_changed;
            if self.low_spec.material_sort {
                sort_quads_by_material(&mut pull.quads);
            }
            // wave 56 BF-1: is_empty フィールドは廃止 (PullBuiltMesh::is_empty() が
            // quads から常時導出) — AO refine / material sort による quads 変化後の
            // 手動再同期は不要になった (旧設計ではこの行自体が同期漏れハザードだった)。
            self.frame_stats.pull_quads_built += pull.quads.len() as u32;
            self.frame_stats.pull_verts_drawn += pull.pull_vertex_count();
            self.frame_stats.pull_ssbo_bytes += pull.ssbo_bytes() as u64;
            if !pull.quads.is_empty() {
                let bytes = crate::zerocopy_cast::cast_slice_to_bytes(&pull.quads).to_vec();
                if self.low_spec.pull_generation_cache {
                    if let Some(gen) = self.world.column_generation(cx, cz) {
                        self.pull_gen_cache.put(cx, cz, gen, bytes.clone());
                    }
                }
                self.gpu_quad_bytes.extend_from_slice(&bytes);
            }
            self.pull_meshes.push(pull);
        }
        // (wave 86 CJ-5: 旧 debug_assert は型レベル const assert
        //  chunk_mesh.rs の `size_of::<Quantized12ByteVertex>() == VERTEX_STRIDE_BYTES`
        //  により完全に包含され、頂点数 O(n) の定数比較だったため除去。)
        let full_mesh = mesh.clone();
        let mesh = self.lod.simplify_mesh(mesh, tier);
        self.last_build = Some(ChunkBuildArtifacts {
            full_mesh: Some(full_mesh),
            svo: None,
            encoding: ReuseEncoding::Mesh,
        });
        if !mesh.is_empty() {
            self.cache.put(&mesh, &rle, section_y);
        }
        self.frame_stats.chunks_built += 1;
        mesh
    }

    /// Returns None when cull pass rejects the chunk (no mesh work).
    pub fn build_chunk_if_visible(
        &mut self,
        cx: i32,
        cz: i32,
        camera_x: f32,
        camera_z: f32,
        dist_blocks: f32,
    ) -> Option<BuiltChunkMesh> {
        let (sections, rle, saved, section_y0) = self.prepare_column(cx, cz);
        self.mesh_section_y0 = section_y0;
        self.frame_stats.rle_palette_bytes += saved;
        let verdict = self
            .cull_pass
            .verdict_column(cx, cz, camera_x, camera_z, &rle);
        self.cull_pass.apply(verdict, &mut self.cull_stats); // FC §7 消化 20: 第 2 帳簿蓄積 (apply は pass 保有 API)
        match verdict {
            CullVerdict::Visible => {
                Some(self.build_chunk(cx, cz, 0, dist_blocks, Some(&sections), Some(&rle)))
            }
            CullVerdict::EmptyColumn => {
                self.frame_stats.empty_culled += 1;
                None
            }
            CullVerdict::Occluded => {
                self.frame_stats.visgraph_culled += 1;
                None
            }
            CullVerdict::OutOfRange => {
                self.frame_stats.range_culled += 1;
                None
            }
        }
    }

    fn builder_simple(&self, cx: i32, cz: i32) -> BuiltChunkMesh {
        self.builder
            .build_chunks_parallel(&[(cx, cz)])
            .into_iter()
            .next()
            .unwrap_or(BuiltChunkMesh {
                chunk_x: cx,
                chunk_z: cz,
                vertices: vec![],
                indices: vec![],
            })
    }

    pub fn frame(
        &mut self,
        chunk_coords: &[(i32, i32)],
        screen_w: u32,
        screen_h: u32,
        delta_time: f32,
    ) -> FrameStats {
        self.tick += 1;
        // 注: FullGraphWiring の駆動はフレーム末端で「このフレームの実データ」を
        //   揃えて `tick_world` を呼ぶ (実メッシュ/実クアッド/実パレットを実入力)。
        self.frame_stats = FrameStats::default();
        self.cull_stats = crate::chunk_cull::CullStats::default();
        self.frame_reuse.begin_frame(self.tick);
        self.pull_meshes.clear();
        self.gpu_quad_bytes.clear();
        self.world.recompute_mesh_origin();

        // Fixed 20 TPS simulation clock (CPU budget: never spiral of death).
        let sim_ticks = self.tick_clock.consume_ticks(delta_time as f64);
        for _ in 0..sim_ticks {
            self.entity_scheduler.advance();
        }
        let _render_alpha = self.tick_clock.render_alpha();
        // DRS: feed frame time so scale can adapt (used by internal_size callers / stats).
        self.drs.push_frame_ms(delta_time * 1000.0);
        // 【誠実注記 wave 129 EC-2】internal_size の評価結果は読み捨て —
        // 内部解像度の変更はどこにも還元されず、ヘッダ行「(no resolution
        // scaling)」の現行効果と整合する計測実演。DRS 自体は将来の解像度
        // スケーリング接続用の結合点として保持 (directive⑦)。
        let _internal = self.drs.internal_size(screen_w, screen_h);

        // Prefer live camera from Minecraft ingest; fall back to stable origin for demo.
        if self.world.has_live_data {
            self.camera = self.world.camera;
        } else {
            // 【誠実注記 wave 87 CK-4】live→デモのモード切替ではカメラが
            // 原点へ跳び、1 フレームだけ実速度スパイクが出る (min(40) クランプ
            // で有界、motion adaptive shading の品質が 1 フレーム低下)。
            // 現挙動を固定 (補正の可否は実機検証待ち — BR-1 判断)。
            self.camera.x = 0.0;
            self.camera.y = 32.0;
            self.camera.z = 0.0;
            self.camera.yaw = 0.0;
            self.camera.pitch = 0.0;
        }
        self.camera.aspect = if screen_h > 0 {
            screen_w as f32 / screen_h as f32
        } else {
            16.0 / 9.0
        };
        if self.camera.fov_y <= 0.0 {
            self.camera.fov_y = 70.0_f32.to_radians();
        }

        if self.feather.enabled && self.feather.motion_adaptive_shading {
            // 実カメラ速度: 前フレームとの実変位 / delta_time (blocks/sec)。
            // 旧実装は「6.0 / delta_time」という実変位と無関係の一定式で、
            // 60fps では常時 ~375 → min(40) クランプにより motion adaptive
            // shading が**恒に最高速判定**され品質低下が常態化していた
            // (監査 2026-07-22 M-1)。
            let cur = [self.camera.x, self.camera.y, self.camera.z];
            let prev = self.prev_camera_xyz.replace(cur);
            let speed = match (prev, delta_time > 0.0) {
                (Some(p), true) => {
                    let dx = cur[0] - p[0];
                    let dy = cur[1] - p[1];
                    let dz = cur[2] - p[2];
                    (dx * dx + dy * dy + dz * dz).sqrt() / delta_time
                }
                _ => 0.0,
            };
            self.last_camera_speed = speed;
            if let Some(s) = self.shading.as_mut() {
                s.set_camera_speed(speed.min(40.0));
                s.tick();
            }
        }

        if self.feather.enabled && self.feather.software_tile_binning {
            if self.tile_binner.is_none() {
                self.tile_binner = Some(SoftwareTileBinner::new(
                    screen_w,
                    screen_h,
                    self.feather.tile_size_px,
                ));
            }
            if let Some(b) = self.tile_binner.as_mut() {
                let lists = b.bin_chunks(chunk_coords, 0.0, 0.0);
                self.frame_stats.tiles_binned = lists.len() as u32;
            }
        }

        self.render_graph.log_schedule();

        let mut coords: Vec<(i32, i32)> = chunk_coords.to_vec();
        let (cam_cx, cam_cz) = self.world.camera_chunk();
        if self.low_spec.nearest_first {
            sort_nearest_first(&mut coords, cam_cx, cam_cz);
        }
        // 実効果: 前フレームの OverdrawSorter front-to-back 順を build 優先度に反映
        // (安定ソート — nearest_first 内の等距離タイブレークのみ変更)。
        if !self.wiring_priority.is_empty() {
            coords.sort_by_key(|c| self.wiring_priority.get(c).copied().unwrap_or(usize::MAX));
        }

        let view_proj =
            TerrainFrameConstants::from_camera(&self.camera, self.world.mesh_origin).view_proj;
        if self.low_spec.pre_mesh_occlusion && self.soft_occlusion.is_none() {
            self.soft_occlusion = Some(SoftwareOcclusion::new(256, 256));
        }
        // Collect occluder AABBs this frame; Hi-Z tests use last frame's pyramid.
        let mut occluder_aabbs: Vec<([f32; 3], [f32; 3])> = Vec::new();

        let mut meshes = Vec::new();
        let mut occupied_per_chunk: Vec<(i32, i32, Vec<u32>)> = Vec::new();
        // 実効果: QualityGovernor の連続フレームオーバー検知が build 予算を実縮小。
        // 【誠実注記 wave 86 CJ-6】予算は「処理列数」で pull キャッシュヒットも
        // 1 消費する (バイト展開+draw コストのフレーム時間経済として意図的)。
        // 一方 chunks_built は build_chunk 到達のみカウント (ヒットは含まない) —
        // この語彙の非対称は仕様 (wave 86 で現挙動を固定)。
        let base_builds = if self.profile.speed_first {
            4
        } else if self.feather.enabled {
            10
        } else {
            20
        };
        let max_builds = self
            .dynamic_build_budget
            .unwrap_or(base_builds)
            .min(base_builds);
        // wiring へ渡す実パレット (描画可視と判定された列のみ、上限 4)。
        let mut wired_palettes: Vec<SectionPalette> = Vec::new();
        // 【wave 86 CJ-1】wiring 入力専用の「描画したがメッシュ再構築しない」列。
        // pull キャッシュヒット / flora LOD box 経路は meshes/pull_meshes に
        // 到達せず、旧実装ではフレーム末端の wiring 入力 (chunk_keys/overdraw
        // 順位づけ → 次フレーム build 順フィードバック) から抜け落ちていた
        // (cache-warm な近接列が unwrap_or(usize::MAX) で常時最劣後化)。
        // (key, dist, aabb, draw_index_count)。
        let mut wiring_only: Vec<((i32, i32), f32, ([f32; 3], [f32; 3]), u32)> = Vec::new();
        let mut builds_this_frame = 0u32;
        let pull_live = self.profile.vertex_pull_4byte
            || self.world.has_live_data
            || std::env::var("rsift.render.backend")
                .map(|s| s.contains("dx12"))
                .unwrap_or(false);

        for (i, &(cx, cz)) in coords.iter().enumerate() {
            if builds_this_frame >= max_builds {
                break;
            }
            let dist = ((cx - cam_cx) as f32).hypot((cz - cam_cz) as f32) * 16.0;
            if let Some(s) = self.shading.as_ref() {
                if !s.should_draw_chunk(i, dist) {
                    self.frame_stats.shading_skipped += 1;
                    continue;
                }
            }

            if self.low_spec.frustum_cull
                && frustum_culled(&self.camera, self.world.mesh_origin, cx, cz)
            {
                self.frame_stats.frustum_culled += 1;
                continue;
            }

            // Soft Hi-Z: after a few near occluders, skip hidden far columns.
            if self.low_spec.pre_mesh_occlusion && builds_this_frame >= 2 {
                if let Some(occ) = self.soft_occlusion.as_mut() {
                    let y0 = self.world.mesh_origin[1] as f32;
                    let aabb_min = [cx as f32 * 16.0, y0, cz as f32 * 16.0];
                    let aabb_max = [(cx + 1) as f32 * 16.0, y0 + 64.0, (cz + 1) as f32 * 16.0];
                    if occ.is_occluded_hysteresis((cx, cz), aabb_min, aabb_max, &view_proj) {
                        self.frame_stats.soft_occluded += 1;
                        continue;
                    }
                }
            }

            // Generation-keyed pull cache (skip remesh when column unchanged).
            if pull_live && self.low_spec.pull_generation_cache {
                if let Some(gen) = self.world.column_generation(cx, cz) {
                    let cached = self.pull_gen_cache.get(cx, cz, gen).map(|b| b.to_vec());
                    if let Some(bytes) = cached {
                        self.frame_stats.pull_cache_hits += 1;
                        self.gpu_quad_bytes.extend_from_slice(&bytes);
                        builds_this_frame += 1;
                        // CJ-1: キャッシュヒット列も実描画対象として wiring へ。
                        // 1 クアッド = 8B 固定 (packed4.rs 型レベル assert)。
                        let y0 = self.world.mesh_origin[1] as f32;
                        wiring_only.push((
                            (cx, cz),
                            dist,
                            (
                                [cx as f32 * 16.0, y0, cz as f32 * 16.0],
                                [(cx + 1) as f32 * 16.0, y0 + 64.0, (cz + 1) as f32 * 16.0],
                            ),
                            (bytes.len() / 8 * 6) as u32,
                        ));
                        if self.low_spec.pre_mesh_occlusion {
                            let y0 = self.world.mesh_origin[1] as f32;
                            occluder_aabbs.push((
                                [cx as f32 * 16.0, y0, cz as f32 * 16.0],
                                [(cx + 1) as f32 * 16.0, y0 + 64.0, (cz + 1) as f32 * 16.0],
                            ));
                        }
                        continue;
                    }
                }
            }

            if !pull_live {
                if let Some(reused) = self.frame_reuse.try_reuse(cx, cz, dist, &self.lod) {
                    occupied_per_chunk.push((cx, cz, reused.occupied));
                    if !reused.mesh.is_empty() {
                        if let Some(pool) = self.persistent_pool.as_mut() {
                            pool.upload_mesh(&reused.mesh);
                        } else if let Some(pool) = self.vertex_pool.as_mut() {
                            pool.upload_mesh(&reused.mesh);
                        }
                        meshes.push(reused.mesh);
                    }
                    continue;
                }
                // ミス計上は try_reuse 内部の契約 (hits+misses==試行数) に
                // 一本化。ここで record_miss を併呼すると二重計上になる
                // (wave 18 で除去)。
            }

            // Far LOD box — skip full greedy when flora LOD says billboard/culled.
            if pull_live
                && self.low_spec.flora_lod
                && flora_should_skip_detail(&self.flora_lod, dist)
            {
                let origin = self.world.mesh_origin;
                let sy0 = self.world.camera_section_y() - 1;
                let mut box_quads =
                    emit_lod_box_quads(cx, cz, sy0, origin[0], origin[1], origin[2], 64, 1);
                if self.low_spec.camera_face_mask {
                    let mask =
                        FaceEmitMask::from_camera_yaw_pitch(self.camera.yaw, self.camera.pitch);
                    filter_quads_by_face_mask(&mut box_quads, mask);
                }
                let bytes = crate::zerocopy_cast::cast_slice_to_bytes(&box_quads);
                self.gpu_quad_bytes.extend_from_slice(bytes);
                self.frame_stats.lod_boxes += 1;
                self.frame_stats.pull_quads_built += box_quads.len() as u32;
                builds_this_frame += 1;
                // CJ-1: LOD box 列も実描画対象として wiring へ。
                {
                    let y0 = origin[1] as f32;
                    wiring_only.push((
                        (cx, cz),
                        dist,
                        (
                            [cx as f32 * 16.0, y0, cz as f32 * 16.0],
                            [(cx + 1) as f32 * 16.0, y0 + 64.0, (cz + 1) as f32 * 16.0],
                        ),
                        (box_quads.len() * 6) as u32,
                    ));
                }
                continue;
            }

            builds_this_frame += 1;
            let (sections, rle, saved, section_y0) = self.prepare_column(cx, cz);
            self.mesh_section_y0 = section_y0;
            self.frame_stats.rle_palette_bytes += saved;
            let occupied = occupied_section_indices(&rle);
            let verdict = self
                .cull_pass
                .verdict_column(cx, cz, self.camera.x, self.camera.z, &rle);
            self.cull_pass.apply(verdict, &mut self.cull_stats); // FC §7 消化 20: 第 2 帳簿蓄積 (apply は pass 保有 API)
            match verdict {
                CullVerdict::EmptyColumn => {
                    self.frame_stats.empty_culled += 1;
                    continue;
                }
                CullVerdict::OutOfRange => {
                    self.frame_stats.range_culled += 1;
                    continue;
                }
                // 【wave 86 CJ-2】build_chunk_if_visible と整合: 現行
                // verdict_column は Occluded を送出しない (wave 61 BK 設計 —
                // 隣接データ無しの全列 occluded 判定は透過ホール障害を招く) が、
                // 旧実装はここで Visible|Occluded を同一視しており、仮に将来
                // producer が現れた場合に frame 路と build_chunk_if_visible 路で
                // 真逆の挙動となる潜在乖離があった → skip 側に統一 (現挙動不変)。
                CullVerdict::Occluded => {
                    self.frame_stats.visgraph_culled += 1;
                    continue;
                }
                CullVerdict::Visible => {}
            }
            occupied_per_chunk.push((cx, cz, occupied.clone()));
            if wired_palettes.len() < 4 {
                if let Some(p0) = sections.first() {
                    wired_palettes.push(p0.clone());
                }
            }
            let mesh = self.build_chunk(cx, cz, 0, dist, Some(&sections), Some(&rle));
            if let Some(artifacts) = self.last_build.take() {
                self.frame_reuse.store(
                    cx,
                    cz,
                    &rle,
                    &occupied,
                    artifacts.full_mesh,
                    artifacts.svo,
                    artifacts.encoding,
                );
            }
            // 【誠実注記 wave 87 CK-5】pull モードでも greedy メッシュは
            // 構築・プール供給される (wgpu/ネイティブ後方互換の二重経路設計)。
            // DX12 実経路の描画実体は gpu_quad_bytes (SSBO) 側であり、
            // この pool 供給は DX12 では消費されない (ベンチ経路の実演)。
            if !mesh.is_empty() {
                if let Some(pool) = self.persistent_pool.as_mut() {
                    pool.upload_mesh(&mesh);
                } else if let Some(pool) = self.vertex_pool.as_mut() {
                    pool.upload_mesh(&mesh);
                }
                meshes.push(mesh);
            }

            if self.low_spec.pre_mesh_occlusion {
                let y0 = self.world.mesh_origin[1] as f32;
                occluder_aabbs.push((
                    [cx as f32 * 16.0, y0, cz as f32 * 16.0],
                    [(cx + 1) as f32 * 16.0, y0 + 64.0, (cz + 1) as f32 * 16.0],
                ));
            }
        }

        if self.low_spec.pre_mesh_occlusion {
            if let Some(occ) = self.soft_occlusion.as_mut() {
                occ.clear_far();
                for (mn, mx) in &occluder_aabbs {
                    occ.rasterize_aabb(*mn, *mx, &view_proj);
                }
                occ.build_pyramid();
            }
        }

        if self.low_spec.quad_budget > 0 && !self.gpu_quad_bytes.is_empty() {
            // 【wave 129 EC-1 / BA-3 解消】旧実装は cast 結果に
            // `unwrap_or_default()` を被せ、cast 失敗 (ラギッド/非整列) 時に
            // **全 quad を静寂空化して書き戻す** データ損失パスを持っていた
            // (棚卸し BA-3)。失敗時は budget 適用を諦め bytes を無変更保持
            // + fail-loud warn へ根治。None は gpu_quad_bytes の生成規約上
            // ほぼ到達不能だが、到達不能を理由に損失を許容しない。
            if !apply_quad_budget_bytes(&mut self.gpu_quad_bytes, self.low_spec.quad_budget) {
                tracing::warn!(
                    "[render_pipeline] quad_budget: quad bytes の cast 失敗 (ラギッド/非整列) — budget 適用をスキップし元バイトを保持 (BA-3)"
                );
            }
        }

        if self.low_spec.triple_buffer_upload && !self.gpu_quad_bytes.is_empty() {
            let bytes = self.gpu_quad_bytes.clone();
            self.upload_ring.with_cpu_write(|slot| {
                *slot = bytes;
            });
        }

        // Continue with eco / HZB stats using built meshes (legacy path).
        // 【誠実注記 wave 87 CK-3】verts_per は先頭メッシュのみの代表値
        // (eco 地域比較メトリクスの近似であり、全メッシュ加重ではない)。
        let verts_per = meshes.first().map(|m| m.vertices.len() as u64).unwrap_or(0);
        let cmp = self
            .eco
            .build_regions_with_occupancy(&coords, &occupied_per_chunk, verts_per);
        self.last_comparison = Some(cmp.clone());
        self.frame_stats.draw_calls = self.eco.regions.iter().map(|r| r.draw_calls).sum();

        let hzb_active = self.profile.hzb_occlusion || self.profile.cpu_masked_occlusion;
        if hzb_active {
            if self.cpu_occluder.is_none() {
                self.cpu_occluder = Some(CpuMaskedOccluder::adaptive(screen_w, screen_h));
            }
            if let Some(occ) = self.cpu_occluder.as_mut() {
                // CK-2 (wave 87): y 帯をメッシュウィンドウ (origin.y..+64) に整合
                // (旧固定 0..64 は live 帯と 48 ブロックずれ、HZB 統計の系統誤り)。
                let boxes = hzb_boxes_for(&meshes, self.world.mesh_origin[1] as f32);
                let vis = occ.cull_boxes_with_camera(&boxes, self.camera);
                self.frame_stats.visible_chunks = vis.len() as u32;
                self.frame_stats.cpu_culled = occ.stats().0;
            }
        } else {
            self.frame_stats.visible_chunks = meshes.len() as u32;
        }

        self.frame_reuse.end_frame();
        self.frame_stats.frame_reuse_hits = self.frame_reuse.stats.hits;
        self.frame_stats.frame_reuse_misses = self.frame_reuse.stats.misses;
        self.frame_stats.frame_reuse_mem_kb = (self.frame_reuse.stats.memory_bytes / 1024) as u32;

        if self.tick % 120 == 0 && self.profile.vertex_pull_4byte {
            let ratio = if self.frame_stats.pull_ssbo_bytes > 0 {
                let legacy = self.frame_stats.pull_quads_built as f64 * 72.0;
                (legacy / self.frame_stats.pull_ssbo_bytes as f64) as f32
            } else {
                0.0
            };
            self.frame_stats.pull_vram_ratio = ratio;
            debug!(
                "[PullMesh] quads={} pull_verts={} ssbo={}KB vram_ratio={:.1}x ibo=0 path=vertex_pull",
                self.frame_stats.pull_quads_built,
                self.frame_stats.pull_verts_drawn,
                self.frame_stats.pull_ssbo_bytes / 1024,
                ratio
            );
        }

        if self.profile.multi_draw_indirect {
            if let Some(pool) = self.persistent_pool.as_mut() {
                pool.rebuild_mdi();
                self.frame_stats.mdi_chunk_draws = pool.mdi_commands.len() as u32;
                self.frame_stats.draw_calls = if pool.mdi_commands.len() > 1 {
                    1
                } else {
                    pool.mdi_commands.len() as u32
                };
            }
        }

        if self.tick % 120 == 0 && self.profile.noise_upsampling {
            let stats = benchmark_upsample(0, 0, &NoiseUpsampleConfig::for_chunk(0, 0));
            self.frame_stats.noise_speedup = stats.speedup;
            stats.log();
        }

        if self.tick % 120 == 0 && self.profile.persistent_vbo_pool {
            if let Some(pool) = self.persistent_pool.as_ref() {
                self.frame_stats.persistent_vbo_mb = (pool.pool_bytes / (1024 * 1024)) as u32;
                debug!(
                    "[PersistentVbo] chunks={} util={:.0}% uploads={} bucket_reuse={} mdi_cmds={}",
                    pool.active_chunks(),
                    pool.utilization() * 100.0,
                    pool.uploads,
                    pool.reuses,
                    pool.mdi_commands.len()
                );
            }
        }

        if self.tick % 120 == 0 {
            self.frame_reuse.log_report(self.tick);
        }

        if self.tick % 300 == 1 && !self.wiring_power_skip_extra {
            cmp.log_report();
            let (hits, misses) = self.cache.stats();
            debug!(
                "[RenderPipeline] cache hits={} misses={} culled empty={} vis={} range={} rle_saved={}KB tex_bw={:.0}%",
                hits,
                misses,
                self.frame_stats.empty_culled,
                self.frame_stats.visgraph_culled,
                self.frame_stats.range_culled,
                self.frame_stats.rle_palette_bytes / 1024,
                self.texture_budget.bandwidth_factor() * 100.0,
            );
        }

        // ============================================================
        // Full Graph Wiring: このフレームの実データで全サブシステムを実実行。
        // (実メッシュ/実プルクアッド/実パレット/実カメラ行列を入力として供給。)
        // ============================================================
        {
            let (sin_y, cos_y) = self.camera.yaw.sin_cos();
            let (sin_p, cos_p) = self.camera.pitch.sin_cos();
            let camera_dir = [-sin_y * cos_p, -sin_p, cos_y * cos_p];
            let y0 = self.world.mesh_origin[1] as f32;

            let mut chunk_keys: Vec<(i32, i32)> = Vec::with_capacity(meshes.len());
            let mut chunk_dists: Vec<f32> = Vec::with_capacity(meshes.len());
            let mut chunk_aabbs: Vec<([f32; 3], [f32; 3])> = Vec::with_capacity(meshes.len());
            let mut draw_index_counts: Vec<u32> = Vec::with_capacity(meshes.len());
            for m in &meshes {
                chunk_keys.push((m.chunk_x, m.chunk_z));
                chunk_dists
                    .push(((m.chunk_x - cam_cx) as f32).hypot((m.chunk_z - cam_cz) as f32) * 16.0);
                chunk_aabbs.push((
                    [m.chunk_x as f32 * 16.0, y0, m.chunk_z as f32 * 16.0],
                    [
                        (m.chunk_x + 1) as f32 * 16.0,
                        y0 + 64.0,
                        (m.chunk_z + 1) as f32 * 16.0,
                    ],
                ));
                draw_index_counts.push(m.indices.len() as u32);
            }
            // プル専用の空メッシュ列 (SVO エンコード路) も実描画対象として追加。
            for p in &self.pull_meshes {
                if chunk_keys.contains(&(p.chunk_x, p.chunk_z)) {
                    continue;
                }
                chunk_keys.push((p.chunk_x, p.chunk_z));
                chunk_dists
                    .push(((p.chunk_x - cam_cx) as f32).hypot((p.chunk_z - cam_cz) as f32) * 16.0);
                chunk_aabbs.push((
                    [p.chunk_x as f32 * 16.0, y0, p.chunk_z as f32 * 16.0],
                    [
                        (p.chunk_x + 1) as f32 * 16.0,
                        y0 + 64.0,
                        (p.chunk_z + 1) as f32 * 16.0,
                    ],
                ));
                draw_index_counts.push((p.quads.len() * 6) as u32);
            }
            // 【wave 86 CJ-1】cache ヒット / LOD box 経路の列も併合
            // (構造上 meshes/pull_meshes とキーは衝突しない — それらの経路は
            //  当該フレームでメッシュ未構築の列 — だが安全側で重複ガード)。
            for (key, d, aabb, icount) in wiring_only {
                if chunk_keys.contains(&key) {
                    continue;
                }
                chunk_keys.push(key);
                chunk_dists.push(d);
                chunk_aabbs.push(aabb);
                draw_index_counts.push(icount);
            }
            let chunk_materials: Vec<u32> = chunk_keys
                .iter()
                .map(|k| {
                    self.pull_meshes
                        .iter()
                        .find(|p| p.chunk_x == k.0 && p.chunk_z == k.1)
                        .and_then(|p| p.quads.first())
                        .map(|q| crate::packed4::PackedPullQuad::unpack_tex(q.word0))
                        .unwrap_or(0)
                })
                .collect();
            // 【wave 129 EC-3 棚卸し】上の材料引き当ては chunk_keys ×
            // pull_meshes の線形 find で O(n·m)。両者とも数百スケールで
            // 現害は小さい (支配 tex の決定用途) — HashMap 化は冗長メモリ
            // との実効見合いを要検討とし現状維持、本節で公表する。
            let mut quad_positions: Vec<[f32; 3]> = Vec::new();
            let mut quad_materials: Vec<u32> = Vec::new();
            'quads: for p in &self.pull_meshes {
                for q in p.quads.iter() {
                    if quad_positions.len() >= 256 {
                        break 'quads;
                    }
                    quad_positions.push([
                        crate::packed4::PackedPullQuad::unpack_x(q.word0) as f32
                            + p.chunk_x as f32 * 16.0,
                        crate::packed4::PackedPullQuad::unpack_y(q.word0) as f32 + y0,
                        crate::packed4::PackedPullQuad::unpack_z(q.word0) as f32
                            + p.chunk_z as f32 * 16.0,
                    ]);
                    quad_materials.push(crate::packed4::PackedPullQuad::unpack_tex(q.word0));
                }
            }
            // 【wave 86 CJ-4 借用移譲】svo 参照は svo_cache (&self) を借用する。
            // フレーム末尾の inputs 消費 (chunk_keys クロージャ) まで生かすと
            // tick_world (&mut self) と衝突するため、所有クローンで借用を即終了
            // させる (旧実装の values().next() も参照保持が故に借用が長命だった)。
            let wiring_svo = self.svo_for_wiring_owned();
            let inputs = FrameWiringInputs {
                delta_ms: delta_time * 1000.0,
                frame_us_measured: (delta_time * 1_000_000.0) as u32,
                frame_index: self.tick,
                screen_w,
                screen_h,
                camera_pos: [self.camera.x, self.camera.y, self.camera.z],
                camera_dir,
                view_proj,
                chunk_keys,
                chunk_materials,
                chunk_aabbs,
                chunk_dists,
                draw_index_counts,
                section_palettes: wired_palettes,
                quad_positions,
                quad_materials,
                quad_bytes: self.gpu_quad_bytes.len(),
                camera_speed: self.last_camera_speed,
                camera_fov_y: self.camera.fov_y,
                svo: wiring_svo.as_ref(),
            };
            let report = self.full_wiring.tick_world(&inputs);
            // 実効果の適用: wiring レポートが次フレームの実入力へフィードバック。
            self.dynamic_build_budget = report.next_build_budget;
            self.wiring_power_skip_extra = report.power_skip_extra;
            self.wiring_priority = report
                .overdraw_order
                .iter()
                .enumerate()
                .filter_map(|(rank, idx)| inputs.chunk_keys.get(*idx).map(|k| (*k, rank)))
                .collect();
            self.frame_stats.wiring_subsystems = report.subsystems_active;
        }

        // FC §7 消化 20: frame 末端で第 2 帳簿と第 1 帳簿を cross 照合
        // (Σ 完全性 + 3 面一致、不変式違反は即 panic で検出)。
        debug_assert_eq!(
            self.cull_stats.tested,
            self.cull_stats.visible
                + self.cull_stats.empty_skipped
                + self.cull_stats.occluded_skipped
                + self.cull_stats.range_skipped,
            "cull Σ 完全性 (tested == Σ4 verdict)"
        );
        debug_assert_eq!(
            self.cull_stats.empty_skipped, self.frame_stats.empty_culled,
            "帳簿 cross: empty"
        );
        debug_assert_eq!(
            self.cull_stats.occluded_skipped, self.frame_stats.visgraph_culled,
            "帳簿 cross: visgraph"
        );
        debug_assert_eq!(
            self.cull_stats.range_skipped, self.frame_stats.range_culled,
            "帳簿 cross: range"
        );

        self.frame_stats.clone()
    }

    pub fn is_feather_mode(&self) -> bool {
        self.feather.enabled
    }
}

/// CK-2 (wave 87): HZB/CPU occluder AABB — y 帯をメッシュウィンドウ
/// (mesh_origin[1] 基点の 64 ブロック帯) に整合させる。旧実装の固定 0..64 は
/// live ワールド帯 (例: origin.y=48) と 48 ブロックずれで、HZB 統計
/// (visible_chunks/cpu_culled) が系統的に誤帯域で計測されていた。
/// BA-3 (wave 129 EC-1): quad_budget をバイト列へ適用する pure 部。
/// cast 成功時は切詰めて `true`、cast 失敗 (ラギッド/非整列) 時は **bytes
/// を無変更で保持** して `false` — 呼出側が warn して継続する。旧実装の
/// `unwrap_or_default()` は None を空 Vec に倒して全 quad を静寂空化
/// したうえ書き戻していた (実害: 到達時にフレーム全描画内容が無警告で
/// 消失)。
fn apply_quad_budget_bytes(bytes: &mut Vec<u8>, budget: usize) -> bool {
    match crate::zerocopy_cast::cast_bytes_to_slice::<crate::packed4::PackedPullQuad>(bytes) {
        Some(q) => {
            let mut quads = q.to_vec();
            truncate_quad_budget(&mut quads, budget);
            *bytes = crate::zerocopy_cast::cast_slice_to_bytes(&quads).to_vec();
            true
        }
        None => false,
    }
}

fn hzb_boxes_for(meshes: &[BuiltChunkMesh], y0: f32) -> Vec<ChunkBoundingBox> {
    meshes
        .iter()
        .enumerate()
        .map(|(i, m)| ChunkBoundingBox {
            min_xyz: [m.chunk_x as f32 * 16.0, y0, m.chunk_z as f32 * 16.0],
            is_visible: 1,
            max_xyz: [
                m.chunk_x as f32 * 16.0 + 16.0,
                y0 + 64.0,
                m.chunk_z as f32 * 16.0 + 16.0,
            ],
            chunk_index: i as u32,
            bindless_texture_id: 0,
            _pad: [0; 3],
        })
        .collect()
}

pub fn global_pipeline() -> &'static Mutex<RsiftRenderPipeline> {
    PIPELINE.get_or_init(|| {
        let dir = std::env::var("APPDATA")
            .map(|a| PathBuf::from(a).join(".minecraft"))
            .unwrap_or_else(|_| PathBuf::from("."));
        Mutex::new(RsiftRenderPipeline::new(&dir))
    })
}

pub fn init_pipeline(game_dir: &Path) {
    let _ = PIPELINE.set(Mutex::new(RsiftRenderPipeline::new(game_dir)));
}

pub fn on_render_frame(width: u32, height: u32, delta_time: f32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static FRAME_COUNTER: AtomicU32 = AtomicU32::new(0);
    let frame_n = FRAME_COUNTER.fetch_add(1, Ordering::Relaxed);

    let rp = AdaptivePerfEngine::render_profile(AdaptivePerfEngine::hardware());
    let dx12 = std::env::var("rsift.render.backend")
        .map(|s| s.contains("dx12"))
        .unwrap_or(false);
    if !rp.native_wgpu_pipeline && !rp.feather.enabled && !dx12 {
        return;
    }

    let has_live = global_pipeline()
        .lock()
        .ok()
        .map(|p| p.world.has_live_data)
        .unwrap_or(false);
    let mesh_interval = adaptive_mesh_interval(delta_time, rp.speed_first, has_live);
    if frame_n % mesh_interval != 0 {
        return;
    }

    if let Ok(mut pipe) = global_pipeline().lock() {
        // Pull window is 64 blocks (≈ radius 1 around the player chunk).
        let radius = if pipe.world.has_live_data {
            1
        } else if rp.speed_first {
            2
        } else if pipe.is_feather_mode() {
            4
        } else {
            5
        };
        let (ocx, ocz) = if pipe.world.has_live_data {
            pipe.world.camera_chunk()
        } else {
            (0, 0)
        };
        let coords: Vec<(i32, i32)> = (-radius..=radius)
            .flat_map(|x| (-radius..=radius).map(move |z| (ocx + x, ocz + z)))
            .collect();
        let stats = pipe.frame(&coords, width, height, delta_time);
        if pipe.tick % 120 == 0 {
            debug!(
                "[RenderPipeline] {:.0}ms chunks={} live={} pullCache={} frustum={} softOcc={} lodBox={} quads={}B lowSpec={}",
                delta_time * 1000.0,
                stats.chunks_built,
                pipe.world.has_live_data,
                stats.pull_cache_hits,
                stats.frustum_culled,
                stats.soft_occluded,
                stats.lod_boxes,
                pipe.gpu_quad_bytes.len(),
                pipe.low_spec.summary()
            );
        }
    }
}

/// Ingest one Minecraft chunk column (flat `section_count * 4096` block ids, 0 = air).
pub fn ingest_world_column(
    cx: i32,
    cz: i32,
    base_section_y: i32,
    blocks: &[u16],
    section_count: usize,
) {
    if let Ok(mut pipe) = global_pipeline().lock() {
        pipe.world
            .ingest(cx, cz, base_section_y, blocks, section_count);
        pipe.cache.invalidate_chunk(cx, cz);
        pipe.pull_gen_cache.invalidate(cx, cz);
        // CK-1: stale SVO 退避 (再ビルドまで VCT が旧地形を見続ける実害)。
        pipe.invalidate_derived_for_column(cx, cz);
        // インジェスト帯域をダーティ登録 (旧: カメラ帯誤り — 実装は
        // note_ingested_sections 側のコメント参照、wave 86 CJ-3)。
        pipe.note_ingested_sections(cx, cz, base_section_y, section_count);
        // Light cache: dirty ingested sections (BFS only runs when queried).
        for s in 0..section_count {
            let sy = base_section_y + s as i32;
            pipe.light_cache.mark_dirty(cx * 16, sy * 16, cz * 16);
        }
    }
}

pub fn set_world_camera(x: f32, y: f32, z: f32, yaw_deg: f32, pitch_deg: f32) {
    if let Ok(mut pipe) = global_pipeline().lock() {
        pipe.world.set_camera(x, y, z, yaw_deg, pitch_deg);
        pipe.camera = pipe.world.camera;
    }
}

/// 現在の実カメラ (live ingest 優先、無ければ安定デフォルト)。
/// (x, y, z, yaw, pitch) — C ABI VTable の自己検証や外部ブリッジ用。
pub fn world_camera() -> (f32, f32, f32, f32, f32) {
    global_pipeline()
        .lock()
        .ok()
        .map(|p| {
            let cam = if p.world.has_live_data {
                p.world.camera
            } else {
                p.camera
            };
            (cam.x, cam.y, cam.z, cam.yaw, cam.pitch)
        })
        .unwrap_or((0.0, 32.0, 0.0, 0.0, 0.0))
}

pub fn prune_world_columns(center_cx: i32, center_cz: i32, radius: i32) {
    if let Ok(mut pipe) = global_pipeline().lock() {
        pipe.world.prune_outside(center_cx, center_cz, radius);
        // CK-1: 派生キャッシュも等語彙で追随 (stale SVO 供給・世代滞留の根絶)。
        pipe.prune_derived_caches(center_cx, center_cz, radius);
    }
}

/// FrameCB for DX12 present (view-proj + chunk origin).
pub fn production_frame_constants() -> Option<TerrainFrameConstants> {
    global_pipeline().lock().ok().map(|p| {
        let cam = if p.world.has_live_data {
            p.world.camera
        } else {
            p.camera
        };
        TerrainFrameConstants::from_camera(&cam, p.world.mesh_origin)
    })
}

/// Zero-copy quad bytes for DX12 present (avoids cloning each frame).
pub fn with_gpu_quad_bytes<R>(f: impl FnOnce(&[u8]) -> R) -> Option<R> {
    global_pipeline().lock().ok().and_then(|p| {
        // Prefer triple-buffer published slot when enabled.
        if p.low_spec.triple_buffer_upload {
            let published = p.upload_ring.begin_gpu_read();
            if !published.is_empty() {
                return Some(f(&published));
            }
        }
        if p.gpu_quad_bytes.is_empty() {
            None
        } else {
            Some(f(&p.gpu_quad_bytes))
        }
    })
}

/// Bytes for DX12 terrain SSBO upload (pull quads from last CPU mesh pass).
pub fn last_gpu_quad_bytes() -> Option<Vec<u8>> {
    with_gpu_quad_bytes(|b| b.to_vec())
}

// ============================================================
// Vanilla render hook counters (bytecode_transpiler が HEAD 挿入した
// Java フックから JNI 経由で実到達する実測カウンタ)。
// FullGraphWiring がデルタを読み、QualityGovernor の実入力に還元する。
// ============================================================

/// BakedModel.getQuads HEAD フックの実到達回数。
static VANILLA_GETQUADS_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// LevelRenderer.renderChunkLayer HEAD フックの実到達回数。
static VANILLA_CHUNKLAYER_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Java `RsiftRenderHooks.getQuadsHeadHook()` → JNI からの実記録 (戻り値 = 累計)。
pub fn note_vanilla_get_quads_hook() -> u64 {
    VANILLA_GETQUADS_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// Java `RsiftRenderHooks.chunkLayerHeadHook()` → JNI からの実記録 (戻り値 = 累計)。
pub fn note_vanilla_chunk_layer_hook() -> u64 {
    VANILLA_CHUNKLAYER_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// 実測値スナップショット (FullGraphWiring が実デルタ計算に使用)。
pub fn vanilla_render_hook_hits() -> (u64, u64) {
    (
        VANILLA_GETQUADS_HITS.load(std::sync::atomic::Ordering::Relaxed),
        VANILLA_CHUNKLAYER_HITS.load(std::sync::atomic::Ordering::Relaxed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// カメラ/フレーム定数テストはグローバルパイプライン経由のため、
    /// テストスレッド並列での相互汚染を防ぐ直列化ロック。
    /// (poison 耐性: 片方のテストが失敗しても他方へ伝播させない)
    static CAMERA_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn camera_test_guard() -> std::sync::MutexGuard<'static, ()> {
        CAMERA_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn apply_quad_budget_bytes_truncates_and_preserves_on_cast_failure() {
        use crate::packed4::PackedPullQuad;
        // 4 クアッド (32 bytes) → budget 2 で先頭 2 クアッド (16 bytes) へ
        // 切詰め + 順序保持、返り値 true。
        let quads: Vec<PackedPullQuad> = (0..4u32)
            .map(|i| PackedPullQuad::new(i, 0, 0, 7, 3, 2, 1, 1))
            .collect();
        let mut bytes = crate::zerocopy_cast::cast_slice_to_bytes(&quads).to_vec();
        assert!(apply_quad_budget_bytes(&mut bytes, 2));
        assert_eq!(bytes.len(), 16, "2 quads * 8 bytes");
        let back: &[PackedPullQuad] = crate::zerocopy_cast::cast_bytes_to_slice(&bytes).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(PackedPullQuad::unpack_x(back[0].word0), 0);
        assert_eq!(PackedPullQuad::unpack_x(back[1].word0), 1, "先頭順序保持");
        // budget 超過なし → バイト列不変で true。
        let mut unchanged = bytes.clone();
        assert!(apply_quad_budget_bytes(&mut unchanged, 8));
        assert_eq!(unchanged, bytes, "budget 内は不変");
        // BA-3 核心: ラギッド (8n+1 bytes) で cast None → false かつ
        // **バイト列無変更** (旧 unwrap_or_default なら空化して本 assert で RED)。
        let mut ragged = vec![0xABu8; 9];
        assert!(!apply_quad_budget_bytes(&mut ragged, 0));
        assert_eq!(ragged, vec![0xABu8; 9], "cast 失敗でバイト列は無変更保持");
    }

    #[test]
    fn vanilla_hook_counters_are_monotonic_deltas() {
        // 絶対値ではなくデルタで検証 (他テストや将来のフック経路と並列安全)。
        let (g0, c0) = vanilla_render_hook_hits();
        let r1 = note_vanilla_get_quads_hook();
        let r2 = note_vanilla_get_quads_hook();
        let r3 = note_vanilla_chunk_layer_hook();
        let (g1, c1) = vanilla_render_hook_hits();
        assert_eq!(g1 - g0, 2);
        assert_eq!(c1 - c0, 1);
        assert_eq!(r2, r1 + 1, "戻り値は加算後の累計");
        assert!(r3 > c0);
    }

    #[test]
    fn world_camera_roundtrip_degrees_to_radians() {
        let _g = camera_test_guard();
        set_world_camera(10.0, 70.0, -25.0, 90.0, 45.0);
        let (x, y, z, yaw, pitch) = world_camera();
        assert_eq!((x, y, z), (10.0, 70.0, -25.0));
        // yaw_deg/pitch_deg はセット時にラジアン化されて保持される。
        assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((pitch - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
    }

    #[test]
    fn production_frame_constants_track_camera_and_mesh_origin() {
        let _g = camera_test_guard();
        set_world_camera(10.0, 70.0, -25.0, 30.0, -10.0);
        let fc = production_frame_constants().expect("pipeline must init lazily");
        // mesh_origin の語彙はブロック座標で (chunk - 1) * 16
        // (world_column_store::recompute_mesh_origin の実仕様。
        //  カメラ (10,70,-25) → chunk (0,4,-2) → origin (-16,48,-48))。
        assert_eq!(fc.chunk_origin, [-16.0, 48.0, -48.0, 0.0]);
        for row in &fc.view_proj {
            for v in row {
                assert!(v.is_finite(), "view_proj must be finite, got {v}");
            }
        }
    }

    #[test]
    fn gpu_quad_bytes_absent_before_any_cpu_mesh_pass() {
        // 新鮮状態 (frame()/CPU mesh 未実行) では publish も quad bytes も無く None。
        // DX12 present 側の「無ければ描かない」前提を固定する。
        assert!(last_gpu_quad_bytes().is_none());
    }

    // =================================================================
    // 第 11 波: frame 実パイプライン (監査 2026-07-22 M 節)
    // =================================================================

    /// ローカルインスタンス (グローバル PIPELINE を触らず並列安全)。
    fn unique_pipeline(tag: &str) -> (std::path::PathBuf, RsiftRenderPipeline) {
        let dir = std::env::temp_dir().join(format!(
            "rsift_pipe_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let p = RsiftRenderPipeline::new(&dir);
        (dir, p)
    }

    /// M-1 回帰: カメラ速度は実変位計測 (旧実装は 6.0/delta の虚偽一定式で
    /// motion adaptive shading が恒に最高速判定となっていた)。
    #[test]
    fn camera_speed_measured_from_real_displacement() {
        let (dir, mut p) = unique_pipeline("speed");
        // hw 検出由来の不確定性を排除して該当分岐を強制有効化。
        p.feather.enabled = true;
        p.feather.motion_adaptive_shading = true;
        p.shading = Some(AdaptiveShadingController::new(true, true, false));
        // live データで world カメラを支配して実変位を作る。
        p.world.ingest(0, 0, 0, &[1u16; 4096], 1);
        p.world.set_camera(0.0, 70.0, 0.0, 0.0, 0.0);
        let _ = p.frame(&[(0, 0)], 640, 360, 0.016);
        assert_eq!(
            p.last_camera_speed.to_bits(),
            0.0f32.to_bits(),
            "初フレームは前回位置なし → 厳密 0.0"
        );
        // 16 blocks 移動 → 16 / 0.016 = 1000 blocks/s。
        p.world.set_camera(16.0, 70.0, 0.0, 0.0, 0.0);
        let _ = p.frame(&[(0, 0)], 640, 360, 0.016);
        let expect = 16.0f32 / 0.016f32;
        assert_eq!(
            p.last_camera_speed.to_bits(),
            expect.to_bits(),
            "実変位 / delta_time の厳密ビット値"
        );
        // 同一位置の次フレームは厳密 0.0 (静止 = 旧実装の ~375 ではない)。
        let _ = p.frame(&[(0, 0)], 640, 360, 0.016);
        assert_eq!(p.last_camera_speed.to_bits(), 0.0f32.to_bits());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M-4 回帰検証を兼ねた frame() 実統計の厳密一致: コア系
    /// (同一機械上の新鮮 2 インスタンス、2 フレーム、期間起動 %120 系を除く)。
    #[test]
    fn frame_demo_stats_det_core() {
        let (dir_a, mut a) = unique_pipeline("det_core_a");
        let (dir_b, mut b) = unique_pipeline("det_core_b");
        let coords = [(0, 0), (1, 0), (-1, 0)];
        for _ in 0..2 {
            let sa = a.frame(&coords, 640, 360, 0.016);
            let sb = b.frame(&coords, 640, 360, 0.016);
            assert_eq!(sa.chunks_built, sb.chunks_built, "chunks_built");
            assert_eq!(sa.cache_hits, sb.cache_hits, "cache_hits");
            assert_eq!(sa.visible_chunks, sb.visible_chunks, "visible_chunks");
            assert_eq!(sa.draw_calls, sb.draw_calls, "draw_calls");
            assert_eq!(sa.cpu_culled, sb.cpu_culled, "cpu_culled");
            assert_eq!(sa.tiles_binned, sb.tiles_binned, "tiles_binned");
            assert_eq!(sa.shading_skipped, sb.shading_skipped, "shading_skipped");
            assert_eq!(sa.empty_culled, sb.empty_culled, "empty_culled");
            assert_eq!(sa.visgraph_culled, sb.visgraph_culled, "visgraph_culled");
            assert_eq!(sa.range_culled, sb.range_culled, "range_culled");
            assert_eq!(
                sa.rle_palette_bytes, sb.rle_palette_bytes,
                "rle_palette_bytes"
            );
            assert_eq!(sa.svo_nodes_built, sb.svo_nodes_built, "svo_nodes_built");
            assert_eq!(
                sa.wiring_subsystems, sb.wiring_subsystems,
                "wiring_subsystems"
            );
            // 仕様値の固定: 60 サブシステム配線。
            assert_eq!(sa.wiring_subsystems, 60, "wiring_subsystems spec");
            // M-1: デモ静止カメラでは実速度は厳密 0.0 (旧実装 ~375 固定ではない)。
            assert_eq!(
                a.last_camera_speed.to_bits(),
                0.0f32.to_bits(),
                "M-1 static cam"
            );
        }
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    /// frame() 実統計の厳密一致: pull/キャッシュ系 (det_core と分割)。
    /// M-4 修正後はデモ (0,0),(1,0),(-1,0) が pull バイト空でもパニックしない。
    #[test]
    fn frame_demo_stats_det_pull() {
        let (dir_a, mut a) = unique_pipeline("det_pull_a");
        let (dir_b, mut b) = unique_pipeline("det_pull_b");
        let coords = [(0, 0), (1, 0), (-1, 0)];
        for _ in 0..2 {
            let sa = a.frame(&coords, 640, 360, 0.016);
            let sb = b.frame(&coords, 640, 360, 0.016);
            assert_eq!(sa.frame_reuse_hits, sb.frame_reuse_hits, "frame_reuse_hits");
            assert_eq!(
                sa.frame_reuse_misses, sb.frame_reuse_misses,
                "frame_reuse_misses"
            );
            assert_eq!(sa.pull_quads_built, sb.pull_quads_built, "pull_quads_built");
            assert_eq!(sa.pull_verts_drawn, sb.pull_verts_drawn, "pull_verts_drawn");
            assert_eq!(sa.pull_ssbo_bytes, sb.pull_ssbo_bytes, "pull_ssbo_bytes");
            assert_eq!(sa.pull_cache_hits, sb.pull_cache_hits, "pull_cache_hits");
            assert_eq!(sa.frustum_culled, sb.frustum_culled, "frustum_culled");
            assert_eq!(sa.soft_occluded, sb.soft_occluded, "soft_occluded");
            assert_eq!(sa.lod_boxes, sb.lod_boxes, "lod_boxes");
            assert_eq!(
                sa.interior_culled_voxels, sb.interior_culled_voxels,
                "interior_culled_voxels"
            );
            assert_eq!(
                sa.wiring_ao_refined_quads, sb.wiring_ao_refined_quads,
                "wiring_ao_refined_quads"
            );
        }
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    /// 【wave 157 FC §7 消化 20 TDD RED】chunk_cull::CullStats を真の
    /// 第 2 帳簿として frame_stats arms と独立に apply 蓄積し、frame 毎に
    /// （a) Σ 完全性 (tested == visible+empty+occluded+range)、(b) 三面
    /// cross-field 一致 (empty_skipped==empty_culled 等 3 面) を検証する
    /// (rq fc_cull (1)(5)、wave 152 Σ==len 完全性の判例)。旧は統計機構が
    /// 消費者ゼロで孤立 (捕捉 79) し、二重帳簿のずれ検出手段がなかった。
    #[test]
    fn frame_cull_stats_cross_invariants_strict() {
        let (dir, mut p) = unique_pipeline("fc_xinv");
        // verdict 到達を確実化 (continue 前倒しゲートを本テスト内のみ無効化、
        // camera_speed 回帰テストの flag 制御と同型の harness 操作)。
        p.low_spec.frustum_cull = false;
        p.low_spec.flora_lod = false;
        p.low_spec.pull_generation_cache = false;
        p.low_spec.pre_mesh_occlusion = false;
        // 4 verdict 全てを 1 frame に生成 (rq fc_cull (6)): (0,0) デフォルト
        // 地形 Visible、(1,0) occupied ingest Visible、(5,0) 空 ingest
        // EmptyColumn (dist 88 < 128)、(20,0) occupied ingest OutOfRange
        // (center 328 > floor 半径 128)。all-zero 帳簿は vacuous になる
        // 構造を、非ゼロ 4 verdict scene で根治した (初版は全 V 帳簿のみで
        // adversarial (b) 非検出 → 本設計へ作り替え、誠実記録)。
        p.world.ingest(1, 0, 0, &[7u16; 4096], 1);
        p.world.ingest(5, 0, 0, &[0u16; 4096], 1);
        p.world.ingest(20, 0, 0, &[9u16; 4096], 1);
        let s = p.frame(&[(0, 0), (1, 0), (5, 0), (20, 0)], 640, 360, 0.016);
        let st = &p.cull_stats;
        assert_eq!(
            (
                st.tested,
                st.visible,
                st.empty_skipped,
                st.occluded_skipped,
                st.range_skipped
            ),
            (4, 2, 1, 0, 1),
            "f1: 4 verdict 構造値 golden (rq fc_cull (6))"
        );
        assert_eq!(
            st.tested,
            st.visible + st.empty_skipped + st.occluded_skipped + st.range_skipped,
            "f1: Σ 完全性"
        );
        assert_eq!(st.empty_skipped, s.empty_culled, "empty 帳簿一致");
        assert_eq!(
            st.occluded_skipped, s.visgraph_culled,
            "visgraph 帳簿一致 (恒 0)"
        );
        assert_eq!(st.range_skipped, s.range_culled, "range 帳簿一致");
        assert_eq!(
            st.visible, s.visible_chunks,
            "visible 帳簿一致 (fresh frame: 全 mesh が当該 verdict 経由)"
        );
        // 捕捉 80 (wave 158 FD で根治済): f2 暖機再読込路の回帰 pin は
        // frame_disk_cache_warm_reframe_strict へ分離 (旧実装は f2 の
        // mesh_cache::decode_mesh で bytemuck align panic、本 pin は
        // fresh f1 の構造値固定に特化)。
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 【wave 158 FD 捕捉 80 統合 pin】暖機フレーム (f2) の disk cache
    /// 再読込路: fc_xinv と同 scene で f1 が put したエントリを f2 の
    /// build_chunk が get→decode_mesh で復元する。旧実装は v2 wire 頂点
    /// オフセット ≡ 2 (mod 4) (SECTIONS_PER_COLUMN=4 偶数・rq fd_cache (3))
    /// のため全正当エントリが decode 時 bytemuck cast panic — f2 到達自体が
    /// 不可能だった。根治後は f2 も panic 無く帳簿が厳密維持されることを固定
    /// (cull verdict は cache 非依存でフレーム毎再評価、Σ+3 面 cross 有効)。
    #[test]
    fn frame_disk_cache_warm_reframe_strict() {
        let (dir, mut p) = unique_pipeline("fd_warm");
        p.low_spec.frustum_cull = false;
        p.low_spec.flora_lod = false;
        p.low_spec.pull_generation_cache = false;
        p.low_spec.pre_mesh_occlusion = false;
        p.world.ingest(1, 0, 0, &[7u16; 4096], 1);
        p.world.ingest(5, 0, 0, &[0u16; 4096], 1);
        p.world.ingest(20, 0, 0, &[9u16; 4096], 1);
        let coords = [(0, 0), (1, 0), (5, 0), (20, 0)];
        let s1 = p.frame(&coords, 640, 360, 0.016);
        assert_eq!(s1.cache_hits, 0, "f1: fresh (put 側)");
        assert_eq!(s1.chunks_built, 2, "f1: (0,0),(1,0) を構築→put");
        // 捕捉 80 の本来発火点: f2 は disk エントリの get→decode_mesh 経路。
        let s2 = p.frame(&coords, 640, 360, 0.016);
        assert_eq!(
            s2.cache_hits, 2,
            "f2: 両列 disk cache hit (decode 成功の証)"
        );
        assert_eq!(s2.chunks_built, 0, "f2: rebuild 無し (cache 供給)");
        let st = &p.cull_stats;
        assert_eq!(
            (
                st.tested,
                st.visible,
                st.empty_skipped,
                st.occluded_skipped,
                st.range_skipped
            ),
            (4, 2, 1, 0, 1),
            "f2: verdict golden は f1 と不変 (rq fc_cull (6) 同 scene)"
        );
        assert_eq!(
            st.tested,
            st.visible + st.empty_skipped + st.occluded_skipped + st.range_skipped,
            "f2: Σ 完全性"
        );
        assert_eq!(st.empty_skipped, s2.empty_culled, "f2: empty 帳簿一致");
        assert_eq!(
            st.occluded_skipped, s2.visgraph_culled,
            "f2: visgraph 帳簿一致 (恒 0)"
        );
        assert_eq!(st.range_skipped, s2.range_culled, "f2: range 帳簿一致");
        assert_eq!(
            st.visible, s2.visible_chunks,
            "f2: visible 帳簿一致 (両 mesh が cache hit 経由で描画列)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // =================================================================
    // wave 86 CJ 節
    // =================================================================

    /// CJ-1: pull キャッシュヒット / flora LOD box 経路の列も wiring 入力
    /// (overdraw 順位づけ → wiring_priority) に含まれる。
    /// 旧実装では cache-warm 列が wiring_priority から抜け落ち、
    /// `unwrap_or(usize::MAX)` により常時最劣後ソートされていた。
    #[test]
    fn wiring_priority_covers_cache_hit_and_flora_lod_columns() {
        let (dir, mut p) = unique_pipeline("wiring_cov");
        // HW プロファイル不確定性の排除: 該当経路のみ決定的に強制。
        p.profile.vertex_pull_4byte = true;
        p.shading = None;
        p.feather.enabled = false;
        p.low_spec.pull_generation_cache = true;
        p.low_spec.frustum_cull = false;
        p.low_spec.pre_mesh_occlusion = false;
        p.low_spec.flora_lod = true;
        p.low_spec.nearest_first = false;
        // dist(20,0)=320 > 288 (=192*1.5) で flora 帯は tier 不問で Culled。
        p.flora_lod = BillboardLodSelector::for_tier_scale(1.5);
        // live 列 (0,0) を生成 (pull 経路と generation の実体化)。
        p.world.ingest(0, 0, 0, &[1u16; 4096], 1);
        p.world.set_camera(8.0, 72.0, 8.0, 0.0, 0.0);
        let coords = [(0, 0), (20, 0)];
        // フレーム 1: (0,0) は pull 構築、(20,0) は flora LOD box。
        let s1 = p.frame(&coords, 640, 360, 0.016);
        assert_eq!(s1.lod_boxes, 1, "flora LOD box 経路に入った証跡");
        assert!(
            p.wiring_priority.contains_key(&(0, 0)),
            "構築列は wiring_priority に存在"
        );
        assert!(
            p.wiring_priority.contains_key(&(20, 0)),
            "CJ-1: flora LOD box 列も wiring_priority に存在 (旧実装は欠落)"
        );
        // フレーム 2: generation 不変で (0,0) はキャッシュヒット径路。
        let s2 = p.frame(&coords, 640, 360, 0.016);
        assert_eq!(s2.pull_cache_hits, 1, "cache ヒット径路に入った証跡");
        assert_eq!(s2.lod_boxes, 1);
        assert!(
            p.wiring_priority.contains_key(&(0, 0)),
            "CJ-1: cache ヒット列も wiring_priority に存在 (旧実装は欠落)"
        );
        assert!(p.wiring_priority.contains_key(&(20, 0)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CJ-3: diff 追跡のダーティ帯域はインジェスト帯域 (旧実装はカメラ帯)。
    /// セクション中心ブロック mark で正確に 1 セクション/回。
    #[test]
    fn ingest_dirty_band_tracks_ingested_sections_not_camera_band() {
        let (dir, mut p) = unique_pipeline("dirty_band");
        // カメラ帯 (y=72 → セクション index 8) と全く異なる帯域 base=0,2 枚
        // (ワールドセクション 0,1 → diff index 4,5: block_to_section_y は +64 基準)。
        p.world.set_camera(8.0, 72.0, 8.0, 0.0, 0.0);
        p.note_ingested_sections(0, 0, 0, 2);
        assert_eq!(
            p.diff_mesh.dirty_sections(0, 0),
            vec![4, 5],
            "インジェスト帯域ちょうど 2 セクション (旧実装は 7,8,9 付近)"
        );
        // 端セクション (world y=-64 = index 0) も正確 (境界伝播なし)。
        p.note_ingested_sections(1, 1, -4, 1);
        assert_eq!(p.diff_mesh.dirty_sections(1, 1), vec![0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CJ-4: wiring へ供給する SVO は最小キー決定論選択
    /// (HashMap 反復順の任意要素ではない)。
    #[test]
    fn svo_for_wiring_selects_min_key_deterministically() {
        let (dir, mut p) = unique_pipeline("svo_minkey");
        let mut block_palette = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        block_palette[0] = 7u16;
        let empty = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let svo_a = SparseVoxelOctree::from_column(&[block_palette]);
        let svo_b = SparseVoxelOctree::from_column(&[empty]);
        p.svo_cache.insert((3, 3), svo_a);
        p.svo_cache.insert((-5, 2), svo_b);
        let sel = p.svo_for_wiring().expect("2 件挿入済");
        assert!(
            std::ptr::eq(sel, p.svo_cache.get(&(-5, 2)).unwrap()),
            "最小キー (-5,2) の SVO が選ばれる"
        );
        // キー 1 件のみの場合もその値。
        p.svo_cache.remove(&(-5, 2));
        let sel = p.svo_for_wiring().expect("1 件残存");
        assert!(std::ptr::eq(sel, p.svo_cache.get(&(3, 3)).unwrap()));
        // 空なら None。
        p.svo_cache.clear();
        assert!(p.svo_for_wiring().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CJ-2 の前提固定: 現行 verdict_column は Occluded を送出しない
    /// (wave 61 BK 設計: 隣接データ無しの全列 occluded 判定は透過ホール
    /// 障害を招くため)。全空 → EmptyColumn、occupied あり近距離 → Visible。
    #[test]
    fn cull_pass_currently_has_no_occluded_producer() {
        let (dir, p) = unique_pipeline("no_occluded");
        let mut palette = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let rle_empty: Vec<RleSection> = vec![RleSection::encode(&palette)];
        // 列中心 (8,8) から判定 → dist 0 (view_radius 不問)。
        let v = p.cull_pass.verdict_column(0, 0, 8.0, 8.0, &rle_empty);
        assert_eq!(v, CullVerdict::EmptyColumn);
        palette[0] = 1u16;
        let rle_occ: Vec<RleSection> = vec![RleSection::encode(&palette)];
        let v = p.cull_pass.verdict_column(0, 0, 8.0, 8.0, &rle_occ);
        assert_eq!(
            v,
            CullVerdict::Visible,
            "Occluded でないこと (= CJ-2 統一腕は現行非到達のまま安全的に封印)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // =================================================================
    // wave 87 CK 節 (render_pipeline 第 2 部)
    // =================================================================

    /// CK-1: 派生キャッシュの prune はワールド列 prune と同語彙
    /// (Chebyshev 半径、境界含む — world_column_store::prune_outside の
    ///  `<= radius` 述語と厳密一致)。
    #[test]
    fn derived_cache_prune_matches_world_radius_vocabulary() {
        let (dir, mut p) = unique_pipeline("cache_prune");
        let empty = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let svo = SparseVoxelOctree::from_column(&[empty]);
        p.svo_cache.insert((0, 0), svo.clone());
        p.svo_cache.insert((3, 0), svo.clone());
        p.pull_gen_cache.put(0, 0, 7, vec![1, 2, 3]);
        p.pull_gen_cache.put(0, 3, 7, vec![4, 5, 6]);
        p.prune_derived_caches(0, 0, 2);
        assert!(p.svo_cache.contains_key(&(0, 0)), "半径内は残留");
        assert!(
            !p.svo_cache.contains_key(&(3, 0)),
            "Chebyshev 半径外 (3) は prune"
        );
        assert!(p.pull_gen_cache.get(0, 0, 7).is_some(), "gen 7 で残留");
        assert!(
            p.pull_gen_cache.get(0, 3, 7).is_none(),
            "世代キャッシュも同語彙 prune"
        );
        // 境界含む: 半径ちょうど (2,2) は残る (= world prune の <= と一致)。
        p.svo_cache.insert((2, 2), svo);
        p.prune_derived_caches(0, 0, 2);
        assert!(p.svo_cache.contains_key(&(2, 2)), "半径ちょうどは境界含む");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CK-1: 列再インジェストで stale SVO が退避される
    /// (VCT プローブへの旧地形供給を断つ)。
    #[test]
    fn invalidate_derived_for_column_evicts_stale_svo() {
        let (dir, mut p) = unique_pipeline("svo_evict");
        let empty = [0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE];
        let svo = SparseVoxelOctree::from_column(&[empty]);
        p.svo_cache.insert((0, 0), svo.clone());
        p.svo_cache.insert((5, 5), svo);
        p.invalidate_derived_for_column(0, 0);
        assert!(
            !p.svo_cache.contains_key(&(0, 0)),
            "置換列の SVO は退避 (旧実装は残存)"
        );
        assert!(p.svo_cache.contains_key(&(5, 5)), "無関係列は保持");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CK-2: HZB AABB の y 帯はメッシュウィンドウ (origin.y, +64)。
    #[test]
    fn hzb_boxes_use_mesh_origin_band() {
        let meshes = vec![BuiltChunkMesh {
            chunk_x: 1,
            chunk_z: 2,
            vertices: vec![],
            indices: vec![],
        }];
        let boxes = hzb_boxes_for(&meshes, 48.0);
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].min_xyz, [16.0, 48.0, 32.0]);
        assert_eq!(boxes[0].max_xyz, [32.0, 112.0, 48.0]);
        assert_eq!(boxes[0].chunk_index, 0);
    }
}
