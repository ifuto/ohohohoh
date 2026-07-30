//! # Rsift Optimized Graphics Engine (`rsift-opt-gfx`)
//!
//! Tier 1–6 rendering / CPU optimizations — adaptive per hardware tier.

pub mod adaptive_shading;
pub mod apple_backend;
pub mod apple_canon;
pub mod binary_greedy_meshing;
pub mod boot_splash;
pub mod branchless_dda;
pub mod chunk_cull;
pub mod chunk_mesh;
pub mod cpu_occlusion;
pub mod eco_render;
pub mod frame_ddgi;
pub mod frame_fsr1;
pub mod frame_hiz;
pub mod frame_pipeline;
pub mod frame_postfx;
pub mod frame_reference;
pub mod frame_reuse;
pub mod frame_vct;
pub mod frame_worldgen;
pub mod gpu_culling;
pub mod gpu_vertex_pull;
pub mod gui_settings;
pub mod hzb_2d;
pub mod iris_pipeline;
pub mod leaf_fast_path;
pub mod lod_hybrid;
pub mod mesh_cache;
pub mod noise_upsample;
pub mod packed4;
pub mod persistent_vbo_pool;
pub mod pull_mesh;
pub mod render_graph;
pub mod render_pipeline;
pub mod section_rle;
pub mod software_tiling;
pub mod svo;
pub mod texture_budget;
pub mod vertex_pool;
// Tier 1
pub mod ao_bake;
pub mod cpu_saver;
pub mod gl33_compat;
pub mod material_batch;
pub mod texture_atlas;
pub use cpu_saver::*;
// Tier 2
pub mod diff_mesh;
pub mod triple_buffer;
// Tier 3
pub mod async_chunk_io;
pub mod simd_kernels;
pub mod soa_layout;
pub mod tick_render_split;
// Tier 4
pub mod billboard_lod;
pub mod depth_prepass;
pub mod drs;
// Tier 5
pub mod entity_tick_lod;
pub mod spatial_hash;
// Tier 6
pub mod light_cache;
pub mod low_spec_stack;
pub mod occlusion_complete;
pub mod section_compress;
pub mod taa;
pub mod world_column_store;

pub use adaptive_shading::*;
pub use ao_bake::*;
pub use async_chunk_io::*;
pub use billboard_lod::*;
pub use binary_greedy_meshing::*;
pub use boot_splash::*;
pub use branchless_dda::*;
pub use chunk_cull::*;
pub use chunk_mesh::*;
pub use cpu_occlusion::*;
pub use depth_prepass::*;
pub use diff_mesh::{DiffMeshUpdater, MeshPatch, SECTIONS_Y};
pub use drs::*;
pub use eco_render::*;
pub use entity_tick_lod::*;
pub use frame_reuse::*;
pub use gl33_compat::*;
pub use gpu_culling::*;
pub use gpu_vertex_pull::*;
pub use gui_settings::*;
pub use hzb_2d::*;
pub use iris_pipeline::*;
pub use leaf_fast_path::*;
pub use light_cache::*;
pub use lod_hybrid::*;
pub use low_spec_stack::*;
pub use material_batch::*;
pub use mesh_cache::*;
pub use noise_upsample::*;
pub use occlusion_complete::*;
pub use packed4::*;
pub use persistent_vbo_pool::*;
pub use pull_mesh::*;
pub use render_graph::*;
pub use render_pipeline::*;
pub use section_compress::*;
pub use section_rle::*;
pub use simd_kernels::*;
pub use soa_layout::*;
pub use software_tiling::*;
pub use spatial_hash::*;
pub use svo::*;
pub use taa::*;
pub use texture_atlas::*;
pub use texture_budget::*;
pub use tick_render_split::*;
pub use triple_buffer::*;
pub use vertex_pool::*;
pub use world_column_store::*;

// ---- New low-spec / integrated-GPU modules ----
// Registered with `pub mod` ONLY (no glob `pub use`): each module defines its
// own Vec3/Vec4 to avoid name collisions when re-exported.
pub mod aces_tonemap;
pub mod bitpacked_section;
pub mod checkerboard;
pub mod clustered_lighting;
pub mod frame_pacing;
pub mod fsr1;
pub mod fsr2;
pub mod fxaa;
pub mod gtao;
pub mod half_vertex;
pub mod meshlet_cone;
pub mod mip_streaming;
pub mod overdraw_sort;
pub mod shadow_lod;
pub mod simd_frustum;
pub mod smaa;
pub mod sparse_texture;
pub mod string_intern;
pub mod tbdr_hints;
pub mod vertex_cache_opt;
pub mod visibility_buffer;
pub mod vrs;
pub mod wboit;

// ---- Additional new-tech modules (wave 2) ----
// Registered with `pub mod` ONLY (no glob `pub use`): each module defines its
// own Vec3/Vec4 to avoid name collisions when re-exported.
// Wave-2a: GPU-driven / sampling / post upgrades (prior turn, not yet wired).
pub mod async_compute;
pub mod bloom;
pub mod cas;
pub mod ddgi;
pub mod exposure;
pub mod lbvh;
pub mod restir;
pub mod ssr;
// Wave-2b: quality-neutral / low-spec-friendly new technologies.
pub mod atmospheric;
pub mod bindless;
pub mod decals;
pub mod depth_of_field;
pub mod foveated;
pub mod ibl_sh;
pub mod motion_blur;
pub mod parallax;
pub mod screen_space_shadow;
pub mod subgroup;
pub mod taa_ycocg;
pub mod volumetric_fog;

// ---- Wave-3: world-proven techniques from the MC mod ecosystem & modern
// engines (EntityCulling / MoreCulling / ImmediatelyFast / DynamicFPS /
// Exordium / Sodium Extra / FastChest / Bobby / Distant Horizons / FSR3 FG /
// Nanite-style / BC7+KTX2 / hardware-style occlusion queries).
// Registered with `pub mod` ONLY — internal names may overlap with other
// waves, so access via the module path.
pub mod bc7_ktx2;
pub mod bobby_cache;
pub mod distant_lod;
pub mod entity_culling;
pub mod fsr3_fg;
pub mod gui_composite;
pub mod hud_batch;
pub mod more_culling;
pub mod nanite_clusters;
pub mod occlusion_query;
pub mod particle_control;
pub mod power_policy;
pub mod static_be;

// ---- Wave-5: low-spec CPU/GPU/memory savers researched 2026-07 (sources
// documented per-module: Sodium GlBufferArena sizing, Frostbite indirect
// compaction, FerriteCore interning, MC palette bit-packing, work-stealing
// job systems, UE DynamicRes governor, zstd region codec, XeGTAO half-res
// AO, HdrHistogram bench methodology).
pub mod bench_harness;
pub mod deinterleave_ao;
pub mod gpu_arena;
pub mod intern_pool;
pub mod job_system;
pub mod mesh_compactor;
pub mod palette_pack;
pub mod quality_governor;
pub mod region_zstd;
pub mod stutter_guard;

// ---- Category 1: Graphics API & Draw Optimization (DirectX12) - First Proposal Full Implementation
pub mod bundle_reuse;
pub mod descriptor_heap_ring;
pub mod enhanced_barriers;
pub mod execute_indirect;
pub mod instanced_draw;
pub mod micro_lod;
pub mod pso_library_cache;
pub mod root_signature_optimized;
pub mod temporal_mesh_diff;
pub mod texture_atlas_virtual;
pub mod vertex_compression_r10g10;
pub mod visibility_graph;

// ---- Category 2: Memory & Data Structure
pub mod bump_arena;
pub mod dag_scheduler;
pub mod dashmap_registry;
pub mod mimalloc_config;
pub mod morton_order;
pub mod pool_slab;
pub mod rayon_job;
pub mod zerocopy_cast;

// ---- Category 6: Compile-time / SIMD / Branchless
pub mod branchless_block;
pub mod pgo_bolt;
pub mod simd_kernels_avx2;

// ---- 45-Technique & 2025 Cutting-Edge Implementation Suite ----
pub mod aokana;
pub mod azdo;
pub mod compute_light_prop;
pub mod fragment_ray_box;
pub mod gigabuffer;
pub mod gigavoxels;
pub mod location_encoded_occupancy;
pub mod lockfree_vram_cache;
pub mod out_of_core_paging;
pub mod svdag;
pub mod tiled_deferred;
pub mod transform_svdag;
pub mod voxel_cone_tracing;

// ---- GPU Runtime (wgpu Device/Queue を実生成し WGSL を実コンパイル検証)
pub mod gpu_runtime;

// ---- Native Apple direct binding (objc dispatcher 注入 + canon 監査)
pub mod apple_ffi_audit;
pub mod metal_direct;
pub mod objc_rt;

// ---- Complete Wiring Orchestrator (スタブ禁止: 全モジュールを本番フレームに配線)
pub mod full_graph_wiring;
