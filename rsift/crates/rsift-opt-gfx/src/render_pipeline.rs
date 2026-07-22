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
use crate::gpu_vertex_pull::PullSsboPool;
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
    pub camera: CameraState,
    pub svo_cache: HashMap<(i32, i32), SparseVoxelOctree>,
    pub frame_reuse: FrameReuseCache,
    pub pull_pool: Option<PullSsboPool>,
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
            camera: CameraState {
                y: 64.0,
                fov_y: 70.0_f32.to_radians(),
                aspect: 16.0 / 9.0,
                ..Default::default()
            },
            svo_cache: HashMap::new(),
            frame_reuse: FrameReuseCache::adaptive(feather.enabled),
            pull_pool: if profile.vertex_pull_4byte {
                Some(PullSsboPool::adaptive())
            } else {
                None
            },
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
                is_empty: true,
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
            pull.is_empty = pull.quads.is_empty();
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
            if let Some(pool) = self.pull_pool.as_mut() {
                pool.upload_pull_mesh(&pull);
            }
            self.pull_meshes.push(pull);
        }
        debug_assert!(
            mesh.vertices
                .iter()
                .all(|_| std::mem::size_of_val(&mesh.vertices[0])
                    == crate::chunk_mesh::VERTEX_STRIDE_BYTES)
                || mesh.vertices.is_empty()
        );
        let full_mesh = mesh.clone();
        let mesh = self.lod.simplify_mesh(mesh, tier);
        self.last_build = Some(ChunkBuildArtifacts {
            full_mesh: Some(full_mesh),
            svo: None,
            encoding: ReuseEncoding::Mesh,
        });
        if !mesh.is_empty {
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
                is_empty: true,
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
        let _internal = self.drs.internal_size(screen_w, screen_h);

        // Prefer live camera from Minecraft ingest; fall back to stable origin for demo.
        if self.world.has_live_data {
            self.camera = self.world.camera;
        } else {
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
                    if !reused.mesh.is_empty {
                        if let Some(pool) = self.persistent_pool.as_mut() {
                            pool.upload_mesh(&reused.mesh);
                        } else if let Some(pool) = self.vertex_pool.as_mut() {
                            pool.upload_mesh(&reused.mesh);
                        }
                        meshes.push(reused.mesh);
                    }
                    continue;
                }
                self.frame_reuse.record_miss();
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
            match verdict {
                CullVerdict::EmptyColumn => {
                    self.frame_stats.empty_culled += 1;
                    continue;
                }
                CullVerdict::OutOfRange => {
                    self.frame_stats.range_culled += 1;
                    continue;
                }
                CullVerdict::Visible | CullVerdict::Occluded => {}
            }
            occupied_per_chunk.push((cx, cz, occupied.clone()));
            if wired_palettes.len() < 4 {
                if let Some(p0) = sections.first() {
                    wired_palettes.push(*p0);
                }
            }
            let mesh = self.build_chunk(cx, cz, 0, dist, Some(&sections), Some(&rle));
            if let Some(artifacts) = self.last_build.take() {
                self.frame_reuse.store(
                    cx,
                    cz,
                    &sections,
                    &rle,
                    &occupied,
                    artifacts.full_mesh,
                    artifacts.svo,
                    artifacts.encoding,
                );
            }
            if !mesh.is_empty {
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

        if self.low_spec.quad_budget > 0 {
            let mut quads: Vec<crate::packed4::PackedPullQuad> =
                crate::zerocopy_cast::cast_bytes_to_slice::<crate::packed4::PackedPullQuad>(
                    &self.gpu_quad_bytes,
                )
                .map(|q| q.to_vec())
                .unwrap_or_default();
            truncate_quad_budget(&mut quads, self.low_spec.quad_budget);
            self.gpu_quad_bytes = crate::zerocopy_cast::cast_slice_to_bytes(&quads).to_vec();
        }

        if self.low_spec.triple_buffer_upload && !self.gpu_quad_bytes.is_empty() {
            let bytes = self.gpu_quad_bytes.clone();
            self.upload_ring.with_cpu_write(|slot| {
                *slot = bytes;
            });
        }

        // Continue with eco / HZB stats using built meshes (legacy path).
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
                let boxes: Vec<ChunkBoundingBox> = meshes
                    .iter()
                    .enumerate()
                    .map(|(i, m)| ChunkBoundingBox {
                        min_xyz: [m.chunk_x as f32 * 16.0, 0.0, m.chunk_z as f32 * 16.0],
                        is_visible: 1,
                        max_xyz: [
                            m.chunk_x as f32 * 16.0 + 16.0,
                            64.0,
                            m.chunk_z as f32 * 16.0 + 16.0,
                        ],
                        chunk_index: i as u32,
                        bindless_texture_id: 0,
                        _pad: [0; 3],
                    })
                    .collect();
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
                svo: self.svo_cache.values().next(),
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

        self.frame_stats.clone()
    }

    pub fn is_feather_mode(&self) -> bool {
        self.feather.enabled
    }
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
        // Mark mid-height sections dirty for section-level diff tracking.
        let mid_y = pipe.world.camera.y as i32;
        pipe.diff_mesh.mark_block_dirty(cx, cz, mid_y);
        pipe.diff_mesh.mark_block_dirty(cx, cz, mid_y + 16);
        pipe.diff_mesh.mark_block_dirty(cx, cz, mid_y - 16);
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

    /// frame() 2 フレームの実統計が、同一機械上の新鮮 2 インスタンスで
    /// 厳密一致すること (期間起動 %120 系を除く全カウンタ)。
    /// frame() 2 フレームの実統計が、同一機械上の新鮮 2 インスタンスで
    /// 厳密一致すること。コア系 (診断 W11-B3 で半分分割)。
    // 診断 W11-B6: CI ログ取得不能のため、フィールド不一致時にフィールド番号を
    // 終了コード化して 1bit チャネルから byte チャネルへ拡張する (恒久ではない。
    // 最終形では通常の assert_eq! へ戻す)。
    macro_rules! field_code {
        ($cond:expr, $code:expr) => {
            if !$cond {
                eprintln!("[W11-B6] mismatch code={}", $code);
                std::process::exit($code);
            }
        };
    }

    // 診断 W11-B7: exit 101 (自前コード 61..79 未到達) に対し、frame()/new() の
    // パニックを catch_unwind で独立コード化する (恒久ではない)。
    fn guarded_new(tag: &str, code: i32) -> (std::path::PathBuf, RsiftRenderPipeline) {
        match std::panic::catch_unwind(|| unique_pipeline(tag)) {
            Ok(v) => v,
            Err(_) => std::process::exit(code),
        }
    }

    fn guarded_frame(
        p: &mut RsiftRenderPipeline,
        coords: &[(i32, i32)],
        code: i32,
    ) -> FrameStats {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            p.frame(coords, 640, 360, 0.016)
        })) {
            Ok(s) => s,
            Err(_) => std::process::exit(code),
        }
    }

    // 診断 W11-B10: マルチチャンク条件要素の切り分け (恒久ではない)。
    #[cfg(any())] // 診断 W11-B11: occ ON/OFF 対決のみ解放 (恒久撤去ではない)
    #[test]
    fn probe_frame_two_pos() {
        let (_d, mut p) = guarded_new("probe_2p", 39);
        let _ = guarded_frame(&mut p, &[(0, 0), (1, 0)], 81);
    }

    #[cfg(any())] // 診断 W11-B12: シングル完結への通信譲渡 (恒久撤去ではない)
    #[test]
    fn probe_frame_multi_no_occ() {
        let (_d, mut p) = guarded_new("probe_noocc", 39);
        p.low_spec.pre_mesh_occlusion = false;
        let _ = guarded_frame(&mut p, &[(0, 0), (1, 0), (2, 0)], 83);
    }

    // 診断 W11-B14: 条件付きサブシステムの選択切断 (恒久ではない)。
    // 診断 W11-B15: 同一 matrix を逐次直列化 (恒久ではない)。
    #[test]
    fn probe_matrix_sequential() {
        {
            let (_d, mut p) = guarded_new("m1", 39);
            p.feather.enabled = false;
            p.profile.hzb_occlusion = false;
            p.profile.cpu_masked_occlusion = false;
            p.profile.multi_draw_indirect = false;
            p.profile.vertex_pull_4byte = false;
            p.profile.noise_upsampling = false;
            p.low_spec.pre_mesh_occlusion = false;
            p.low_spec.quad_budget = 0;
            let _ = guarded_frame(&mut p, &[(0, 0)], 201);
        }
        {
            let (_d, mut p) = guarded_new("m3", 39);
            p.profile.hzb_occlusion = false;
            p.profile.cpu_masked_occlusion = false;
            p.feather.enabled = false;
            let _ = guarded_frame(&mut p, &[(0, 0)], 207);
        }
        {
            let (_d, mut p) = guarded_new("m4", 39);
            p.profile.hzb_occlusion = false;
            p.profile.cpu_masked_occlusion = false;
            p.profile.multi_draw_indirect = false;
            let _ = guarded_frame(&mut p, &[(0, 0)], 209);
        }
        {
            let (_d, mut p) = guarded_new("m5", 39);
            p.profile.hzb_occlusion = false;
            p.profile.cpu_masked_occlusion = false;
            p.profile.vertex_pull_4byte = false;
            let _ = guarded_frame(&mut p, &[(0, 0)], 211);
        }
        {
            let (_d, mut p) = guarded_new("m6", 39);
            p.profile.hzb_occlusion = false;
            p.profile.cpu_masked_occlusion = false;
            p.profile.noise_upsampling = false;
            let _ = guarded_frame(&mut p, &[(0, 0)], 213);
        }
        {
            let (_d, mut p) = guarded_new("m7", 39);
            p.profile.hzb_occlusion = false;
            p.profile.cpu_masked_occlusion = false;
            p.low_spec.pre_mesh_occlusion = false;
            let _ = guarded_frame(&mut p, &[(0, 0)], 215);
        }
        {
            let (_d, mut p) = guarded_new("m8", 39);
            p.profile.hzb_occlusion = false;
            p.profile.cpu_masked_occlusion = false;
            p.low_spec.quad_budget = 0;
            let _ = guarded_frame(&mut p, &[(0, 0)], 217);
        }
    }

    #[cfg(any())] // 診断 W11-B15: 逐次 matrix への譲渡 (恒久撤去ではない)
    #[test]
    fn probe_frame_full_strip() {
        let (_d, mut p) = guarded_new("probe_strip", 39);
        p.feather.enabled = false;
        p.profile.hzb_occlusion = false;
        p.profile.cpu_masked_occlusion = false;
        p.profile.multi_draw_indirect = false;
        p.profile.vertex_pull_4byte = false;
        p.profile.noise_upsampling = false;
        p.low_spec.pre_mesh_occlusion = false;
        p.low_spec.quad_budget = 0;
        let _ = guarded_frame(&mut p, &[(0, 0)], 101);
    }

    #[cfg(any())] // 診断 W11-B15: 逐次 matrix への譲渡 (恒久撤去ではない)
    #[test]
    fn probe_frame_no_hzb() {
        let (_d, mut p) = guarded_new("probe_nohzb", 39);
        p.profile.hzb_occlusion = false;
        p.profile.cpu_masked_occlusion = false;
        let _ = guarded_frame(&mut p, &[(0, 0)], 103);
    }

    #[cfg(any())] // 診断 W11-B15: 逐次 matrix への譲渡 (恒久撤去ではない)
    #[test]
    fn probe_frame_no_feather() {
        let (_d, mut p) = guarded_new("probe_nofeather", 39);
        p.feather.enabled = false;
        let _ = guarded_frame(&mut p, &[(0, 0)], 105);
    }

    #[cfg(any())] // 診断 W11-B15: 逐次 matrix への譲渡 (恒久撤去ではない)
    #[test]
    fn probe_frame_no_mdi_pull() {
        let (_d, mut p) = guarded_new("probe_nomdi", 39);
        p.profile.multi_draw_indirect = false;
        p.profile.vertex_pull_4byte = false;
        let _ = guarded_frame(&mut p, &[(0, 0)], 107);
    }

    // 診断 W11-B13: 単一テスト内で逐次実行 (並列レース排除) し、
    // frame() を構成ステージごとに catch_unwind で被覆 (恒久ではない)。
    #[cfg(any())] // 診断 W11-B14: 選択切断プローブへの譲渡 (恒久撤去ではない)
    #[test]
    fn probe_stage_by_stage_sequential() {
        let (_d, mut p) = guarded_new("probe_stage", 39);
        // stage 1: デモ列生成
        let r1 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.prepare_column(2, 0)));
        let (sections, rle, _saved, _sy0) = match r1 {
            Ok(v) => v,
            Err(_) => std::process::exit(91),
        };
        // stage 2: 単チャンク build
        let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            p.build_chunk(2, 0, 0, 32.0, Some(&sections), Some(&rle))
        }));
        if r2.is_err() {
            std::process::exit(93);
        }
        // stage 3: (0,0) 単独のデモフレーム
        let _ = guarded_frame(&mut p, &[(0, 0)], 95);
        // stage 4: 2 チャンク最小マルチ
        let _ = guarded_frame(&mut p, &[(0, 0), (1, 0)], 97);
    }

    #[cfg(any())] // 診断 W11-B13: 逐次ステージプローブへの譲渡 (恒久撤去ではない)
    #[test]
    fn probe_frame_single_far() {
        let (_d, mut p) = guarded_new("probe_far", 39);
        let _ = guarded_frame(&mut p, &[(2, 0)], 85);
    }

    #[cfg(any())] // 診断 W11-B11: occ ON/OFF 対決のみ解放 (恒久撤去ではない)
    #[test]
    fn probe_frame_multi_no_frustum() {
        let (_d, mut p) = guarded_new("probe_nofr", 39);
        p.low_spec.frustum_cull = false;
        let _ = guarded_frame(&mut p, &[(0, 0), (1, 0), (2, 0)], 87);
    }

    #[cfg(any())] // 診断 W11-B9: probes への通信譲渡 (恒久撤去ではない)
    #[test]
    fn frame_demo_stats_det_core_x() {
        let (dir_a, mut a) = guarded_new("det_corex_a", 39);
        let (dir_b, mut b) = guarded_new("det_corex_b", 40);
        let coords = [(0, 0), (1, 0), (-1, 0)];
        for _ in 0..2 {
            let sa = guarded_frame(&mut a, &coords, 41);
            let sb = guarded_frame(&mut b, &coords, 42);
            field_code!(sa.chunks_built == sb.chunks_built, 61);
            field_code!(sa.cache_hits == sb.cache_hits, 62);
            field_code!(sa.visible_chunks == sb.visible_chunks, 63);
            field_code!(sa.draw_calls == sb.draw_calls, 64);
            field_code!(sa.cpu_culled == sb.cpu_culled, 65);
            field_code!(sa.tiles_binned == sb.tiles_binned, 66);
        }
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    // 診断 W11-B8: パニックのコンテンツ依存性プローブ (恒久ではない。
    // パニック時は frame 呼出地点のコードで終了する)。
    #[cfg(any())] // 診断 W11-B13: 逐次ステージプローブへの譲渡 (恒久撤去ではない)
    #[test]
    fn probe_frame_single_origin() {
        let (_d, mut p) = guarded_new("probe_o", 39);
        let _ = guarded_frame(&mut p, &[(0, 0)], 51);
        let _ = std::fs::remove_dir_all(_d);
    }

    #[cfg(any())] // 診断 W11-B13: 逐次ステージプローブへの譲渡 (恒久撤去ではない)
    #[test]
    fn probe_frame_single_neg() {
        let (_d, mut p) = guarded_new("probe_n", 39);
        let _ = guarded_frame(&mut p, &[(-1, 0)], 53);
    }

    #[cfg(any())] // 診断 W11-B13: 逐次ステージプローブへの譲渡 (恒久撤去ではない)
    #[test]
    fn probe_frame_single_pos() {
        let (_d, mut p) = guarded_new("probe_p", 39);
        let _ = guarded_frame(&mut p, &[(1, 0)], 55);
    }

    #[cfg(any())] // 診断 W11-B12: シングル完結への通信譲渡 (恒久撤去ではない)
    #[test]
    fn probe_frame_multi_pos() {
        let (_d, mut p) = guarded_new("probe_m", 39);
        let _ = guarded_frame(&mut p, &[(0, 0), (1, 0), (2, 0)], 57);
    }

    #[cfg(any())] // 診断 W11-B9: probes への通信譲渡 (恒久撤去ではない)
    #[test]
    fn frame_demo_stats_det_core_y() {
        let (dir_a, mut a) = guarded_new("det_corey_a", 39);
        let (dir_b, mut b) = guarded_new("det_corey_b", 40);
        let coords = [(0, 0), (1, 0), (-1, 0)];
        for _ in 0..2 {
            let sa = guarded_frame(&mut a, &coords, 41);
            let sb = guarded_frame(&mut b, &coords, 42);
            field_code!(sa.shading_skipped == sb.shading_skipped, 71);
            field_code!(sa.empty_culled == sb.empty_culled, 72);
            field_code!(sa.visgraph_culled == sb.visgraph_culled, 73);
            field_code!(sa.range_culled == sb.range_culled, 74);
            field_code!(sa.rle_palette_bytes == sb.rle_palette_bytes, 75);
            field_code!(sa.svo_nodes_built == sb.svo_nodes_built, 76);
            field_code!(sa.wiring_subsystems == sb.wiring_subsystems, 77);
            field_code!(sa.wiring_subsystems == 60, 78);
            field_code!(a.last_camera_speed.to_bits() == 0.0f32.to_bits(), 79);
        }
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    /// frame() 実統計の厳密一致。pull/キャッシュ系 (診断 W11-B3 で半分分割)。
    #[cfg(any())] // 診断 W11-B4: det_core 単独切り分け中 (恒久撤去ではない)
    #[test]
    fn frame_demo_stats_det_pull() {
        let (dir_a, mut a) = unique_pipeline("det_pull_a");
        let (dir_b, mut b) = unique_pipeline("det_pull_b");
        let coords = [(0, 0), (1, 0), (-1, 0)];
        for _ in 0..2 {
            let sa = a.frame(&coords, 640, 360, 0.016);
            let sb = b.frame(&coords, 640, 360, 0.016);
            assert_eq!(sa.frame_reuse_hits, sb.frame_reuse_hits);
            assert_eq!(sa.frame_reuse_misses, sb.frame_reuse_misses);
            assert_eq!(sa.pull_quads_built, sb.pull_quads_built);
            assert_eq!(sa.pull_verts_drawn, sb.pull_verts_drawn);
            assert_eq!(sa.pull_ssbo_bytes, sb.pull_ssbo_bytes);
            assert_eq!(sa.pull_cache_hits, sb.pull_cache_hits);
            assert_eq!(sa.frustum_culled, sb.frustum_culled);
            assert_eq!(sa.soft_occluded, sb.soft_occluded);
            assert_eq!(sa.lod_boxes, sb.lod_boxes);
            assert_eq!(sa.interior_culled_voxels, sb.interior_culled_voxels);
            assert_eq!(sa.wiring_ao_refined_quads, sb.wiring_ao_refined_quads);
        }
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }
}
