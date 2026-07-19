//! Rsift render pipeline — 必須/推奨 + Feather weak-PC (no resolution scaling).

use crate::adaptive_shading::AdaptiveShadingController;
use crate::binary_greedy_meshing::{
    demo_column_palettes, mesh_chunk_column, mesh_chunk_column_pull_world, SectionPalette,
    SECTION_SIZE, SECTIONS_PER_COLUMN,
};
use crate::gpu_vertex_pull::PullSsboPool;
use crate::pull_mesh::PullBuiltMesh;
use crate::chunk_cull::{ChunkCullPass, CullVerdict};
use crate::chunk_mesh::{BuiltChunkMesh, MultithreadedChunkBuilder};
use crate::cpu_occlusion::CpuMaskedOccluder;
use crate::eco_render::{EcoRegionRenderer, SodiumComparison};
use crate::gpu_culling::ChunkBoundingBox;
use crate::hzb_2d::CameraState;
use crate::leaf_fast_path::apply_leaf_fast_path;
use crate::lod_hybrid::LodHybridSelector;
use crate::mesh_cache::MeshDiskCache;
use crate::section_rle::RleSection;
use crate::section_rle::occupied_section_indices;
use crate::frame_reuse::{FrameReuseCache, ReuseEncoding};
use crate::svo::SparseVoxelOctree;
use crate::render_graph::RenderGraphScheduler;
use crate::software_tiling::SoftwareTileBinner;
use crate::texture_budget::TextureBudget;
use crate::noise_upsample::{column_palettes_upsampled, benchmark_upsample, NoiseUpsampleConfig};
use crate::persistent_vbo_pool::PersistentVboPool;
use crate::vertex_pool::VertexPool;
use crate::diff_mesh::DiffMeshUpdater;
use crate::triple_buffer::TripleBuffer;
use crate::drs::DynamicResolutionScaler;
use crate::taa::LightweightTaa;
use crate::occlusion_complete::SoftwareOcclusion;
use crate::tick_render_split::FixedTickClock;
use crate::billboard_lod::BillboardLodSelector;
use crate::depth_prepass::DepthPrepassPlanner;
use crate::world_column_store::{TerrainFrameConstants, WorldColumnStore};
use crate::entity_tick_lod::EntityTickScheduler;
use crate::spatial_hash::SpatialHashGrid3D;
use crate::light_cache::LightPropagationCache;
use crate::low_spec_stack::{
    adaptive_mesh_interval, apply_cheap_ao, apply_solid_interior_cull, emit_lod_box_quads,
    filter_quads_by_face_mask, flora_should_skip_detail, frustum_culled, section_occupancy,
    section_is_empty_occ, sort_nearest_first, sort_quads_by_material, truncate_quad_budget,
    FaceEmitMask, LowSpecPlan, PullGenerationCache,
};
use rsift_api::{AdaptivePerfEngine, AdaptiveRenderProfile, EngineCaps, FeatherRenderConfig};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tracing::{info, debug};

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
        info!("[RenderPipeline] {}", AdaptivePerfEngine::render_profile_summary(&profile));
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
                TextureBudget::from_profile(true, feather.compressed_textures, feather.mipmap_bias).label()
            );
        }
        let texture_budget =
            TextureBudget::from_profile(feather.enabled, feather.compressed_textures, feather.mipmap_bias);
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
            depth_plan: if matches!(
                profile.tier,
                rsift_api::PerformanceTier::High
            ) {
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
            last_build: None,
            tick: 0,
        }
    }

    fn demo_mode_enabled() -> bool {
        std::env::var("RSIFT_ENABLE_DEMO_RENDER")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "on" | "ON"))
            .unwrap_or(false)
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
                if self.low_spec.leaf_fast_path {
                    apply_leaf_fast_path(palette, true);
                }
            }
            let rle: Vec<RleSection> = sections.iter().map(RleSection::encode).collect();
            return (sections, rle, 0, section_y0);
        }
        if !Self::demo_mode_enabled() {
            let sections = vec![[0u16; SECTION_SIZE * SECTION_SIZE * SECTION_SIZE]; SECTIONS_PER_COLUMN];
            let rle: Vec<RleSection> = sections.iter().map(RleSection::encode).collect();
            return (sections, rle, 0, 0);
        }
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
            apply_leaf_fast_path(palette, self.low_spec.leaf_fast_path || self.profile.leaf_fast_path);
        }
        let rle: Vec<RleSection> = sections.iter().map(RleSection::encode).collect();
        let raw_bytes = (sections.len() * 4096 * 2) as u64;
        let rle_bytes: u64 = rle.iter().map(|s| s.to_bytes().len() as u64).sum();
        let saved = raw_bytes.saturating_sub(rle_bytes);
        (sections, rle, saved, 0)
    }

    pub fn build_chunk(&mut self, cx: i32, cz: i32, section_y: i32, dist_blocks: f32, sections: Option<&[SectionPalette]>, rle: Option<&[RleSection]>) -> BuiltChunkMesh {
        if let Some(cached) = self.cache.get(cx, cz, section_y) {
            self.frame_stats.cache_hits += 1;
            return self.lod.simplify_mesh(cached, self.lod.tier_for_distance(dist_blocks));
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
            if self.low_spec.material_sort {
                sort_quads_by_material(&mut pull.quads);
            }
            pull.is_empty = pull.quads.is_empty();
            self.frame_stats.pull_quads_built += pull.quads.len() as u32;
            self.frame_stats.pull_verts_drawn += pull.pull_vertex_count();
            self.frame_stats.pull_ssbo_bytes += pull.ssbo_bytes() as u64;
            if !pull.quads.is_empty() {
                let bytes = bytemuck::cast_slice(&pull.quads).to_vec();
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
        debug_assert!(mesh.vertices.iter().all(|_| std::mem::size_of_val(&mesh.vertices[0]) == crate::chunk_mesh::VERTEX_STRIDE_BYTES) || mesh.vertices.is_empty());
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
        let verdict = self.cull_pass.verdict_column(cx, cz, camera_x, camera_z, &rle);
        match verdict {
            CullVerdict::Visible => Some(self.build_chunk(cx, cz, 0, dist_blocks, Some(&sections), Some(&rle))),
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
            let speed = if delta_time > 0.0 { 6.0 / delta_time } else { 0.0 };
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

        let view_proj = TerrainFrameConstants::from_camera(&self.camera, self.world.mesh_origin).view_proj;
        if self.low_spec.pre_mesh_occlusion && self.soft_occlusion.is_none() {
            self.soft_occlusion = Some(SoftwareOcclusion::new(256, 256));
        }
        // Collect occluder AABBs this frame; Hi-Z tests use last frame's pyramid.
        let mut occluder_aabbs: Vec<([f32; 3], [f32; 3])> = Vec::new();

        let mut meshes = Vec::new();
        let mut occupied_per_chunk: Vec<(i32, i32, Vec<u32>)> = Vec::new();
        let max_builds = if self.profile.speed_first {
            4
        } else if self.feather.enabled {
            10
        } else {
            20
        };
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
                    let cached = self
                        .pull_gen_cache
                        .get(cx, cz, gen)
                        .map(|b| b.to_vec());
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
                let mut box_quads = emit_lod_box_quads(
                    cx, cz, sy0, origin[0], origin[1], origin[2], 64, 1,
                );
                if self.low_spec.camera_face_mask {
                    let mask =
                        FaceEmitMask::from_camera_yaw_pitch(self.camera.yaw, self.camera.pitch);
                    filter_quads_by_face_mask(&mut box_quads, mask);
                }
                let bytes = bytemuck::cast_slice(&box_quads);
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
            let verdict = self.cull_pass.verdict_column(cx, cz, self.camera.x, self.camera.z, &rle);
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
                bytemuck::cast_slice(&self.gpu_quad_bytes).to_vec();
            truncate_quad_budget(&mut quads, self.low_spec.quad_budget);
            self.gpu_quad_bytes = bytemuck::cast_slice(&quads).to_vec();
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
                        min_xyz: [
                            m.chunk_x as f32 * 16.0,
                            0.0,
                            m.chunk_z as f32 * 16.0,
                        ],
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
        self.frame_stats.frame_reuse_mem_kb =
            (self.frame_reuse.stats.memory_bytes / 1024) as u32;

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

        if self.tick % 300 == 1 {
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
pub fn ingest_world_column(cx: i32, cz: i32, base_section_y: i32, blocks: &[u16], section_count: usize) {
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
    global_pipeline()
        .lock()
        .ok()
        .and_then(|p| {
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
