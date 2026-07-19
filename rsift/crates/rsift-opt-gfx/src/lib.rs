//! # Rsift Optimized Graphics Engine (`rsift-opt-gfx`)
//!
//! Tier 1–6 rendering / CPU optimizations — adaptive per hardware tier.

pub mod chunk_mesh;
pub mod gpu_culling;
pub mod iris_pipeline;
pub mod gui_settings;
pub mod boot_splash;
pub mod eco_render;
pub mod binary_greedy_meshing;
pub mod vertex_pool;
pub mod mesh_cache;
pub mod cpu_occlusion;
pub mod leaf_fast_path;
pub mod render_pipeline;
pub mod software_tiling;
pub mod adaptive_shading;
pub mod lod_hybrid;
pub mod texture_budget;
pub mod render_graph;
pub mod section_rle;
pub mod hzb_2d;
pub mod chunk_cull;
pub mod noise_upsample;
pub mod persistent_vbo_pool;
pub mod packed4;
pub mod pull_mesh;
pub mod gpu_vertex_pull;
pub mod frame_reuse;
pub mod branchless_dda;
pub mod svo;
// Tier 1
pub mod material_batch;
pub mod texture_atlas;
pub mod gl33_compat;
pub mod ao_bake;
// Tier 2
pub mod diff_mesh;
pub mod triple_buffer;
// Tier 3
pub mod simd_kernels;
pub mod async_chunk_io;
pub mod soa_layout;
pub mod tick_render_split;
// Tier 4
pub mod drs;
pub mod depth_prepass;
pub mod billboard_lod;
// Tier 5
pub mod spatial_hash;
pub mod entity_tick_lod;
// Tier 6
pub mod light_cache;
pub mod taa;
pub mod occlusion_complete;
pub mod world_column_store;
pub mod low_spec_stack;
pub mod section_compress;

pub use chunk_mesh::*;
pub use gpu_culling::*;
pub use iris_pipeline::*;
pub use gui_settings::*;
pub use boot_splash::*;
pub use eco_render::*;
pub use binary_greedy_meshing::*;
pub use vertex_pool::*;
pub use mesh_cache::*;
pub use cpu_occlusion::*;
pub use leaf_fast_path::*;
pub use render_pipeline::*;
pub use software_tiling::*;
pub use adaptive_shading::*;
pub use lod_hybrid::*;
pub use texture_budget::*;
pub use render_graph::*;
pub use section_rle::*;
pub use chunk_cull::*;
pub use hzb_2d::*;
pub use noise_upsample::*;
pub use persistent_vbo_pool::*;
pub use packed4::*;
pub use pull_mesh::*;
pub use gpu_vertex_pull::*;
pub use frame_reuse::*;
pub use branchless_dda::*;
pub use svo::*;
pub use material_batch::*;
pub use texture_atlas::*;
pub use gl33_compat::*;
pub use ao_bake::*;
pub use diff_mesh::{DiffMeshUpdater, MeshPatch, SECTIONS_Y};
pub use triple_buffer::*;
pub use simd_kernels::*;
pub use async_chunk_io::*;
pub use soa_layout::*;
pub use tick_render_split::*;
pub use drs::*;
pub use depth_prepass::*;
pub use billboard_lod::*;
pub use spatial_hash::*;
pub use entity_tick_lod::*;
pub use light_cache::*;
pub use taa::*;
pub use occlusion_complete::*;
pub use world_column_store::*;
pub use low_spec_stack::*;
pub use section_compress::*;

// ---- New low-spec / integrated-GPU modules ----
// Registered with `pub mod` ONLY (no glob `pub use`): each module defines its
// own Vec3/Vec4 to avoid name collisions when re-exported.
pub mod aces_tonemap;
pub mod overdraw_sort;
pub mod half_vertex;
pub mod checkerboard;
pub mod vertex_cache_opt;
pub mod frame_pacing;
pub mod mip_streaming;
pub mod tbdr_hints;
pub mod simd_frustum;
pub mod fsr1;
pub mod fsr2;
pub mod fxaa;
pub mod smaa;
pub mod vrs;
pub mod gtao;
pub mod shadow_lod;
pub mod meshlet_cone;
pub mod clustered_lighting;
pub mod sparse_texture;
pub mod visibility_buffer;
pub mod wboit;

// ---- Additional new-tech modules (wave 2) ----
// Registered with `pub mod` ONLY (no glob `pub use`): each module defines its
// own Vec3/Vec4 to avoid name collisions when re-exported.
// Wave-2a: GPU-driven / sampling / post upgrades (prior turn, not yet wired).
pub mod async_compute;
pub mod lbvh;
pub mod restir;
pub mod ddgi;
pub mod ssr;
pub mod bloom;
pub mod cas;
pub mod exposure;
// Wave-2b: quality-neutral / low-spec-friendly new technologies.
pub mod atmospheric;
pub mod volumetric_fog;
pub mod taa_ycocg;
pub mod screen_space_shadow;
pub mod motion_blur;
pub mod depth_of_field;
pub mod parallax;
pub mod ibl_sh;
pub mod decals;
pub mod subgroup;
pub mod bindless;
pub mod foveated;

// ---- Wave-3: world-proven techniques from the MC mod ecosystem & modern
// engines (EntityCulling / MoreCulling / ImmediatelyFast / DynamicFPS /
// Exordium / Sodium Extra / FastChest / Bobby / Distant Horizons / FSR3 FG /
// Nanite-style / BC7+KTX2 / hardware-style occlusion queries).
// Registered with `pub mod` ONLY — internal names may overlap with other
// waves, so access via the module path.
pub mod entity_culling;
pub mod more_culling;
pub mod hud_batch;
pub mod power_policy;
pub mod gui_composite;
pub mod particle_control;
pub mod static_be;
pub mod bobby_cache;
pub mod distant_lod;
pub mod fsr3_fg;
pub mod nanite_clusters;
pub mod bc7_ktx2;
pub mod occlusion_query;

// ---- Wave-5: low-spec CPU/GPU/memory savers researched 2026-07 (sources
// documented per-module: Sodium GlBufferArena sizing, Frostbite indirect
// compaction, FerriteCore interning, MC palette bit-packing, work-stealing
// job systems, UE DynamicRes governor, zstd region codec, XeGTAO half-res
// AO, HdrHistogram bench methodology).
pub mod gpu_arena;
pub mod mesh_compactor;
pub mod intern_pool;
pub mod palette_pack;
pub mod job_system;
pub mod stutter_guard;
pub mod quality_governor;
pub mod region_zstd;
pub mod deinterleave_ao;
pub mod bench_harness;

// ---- Category 1: Graphics API & Draw Optimization (DirectX12) - First Proposal Full Implementation
pub mod descriptor_heap_ring;
pub mod root_signature_optimized;
pub mod pso_library_cache;
pub mod execute_indirect;
pub mod bundle_reuse;
pub mod enhanced_barriers;
pub mod texture_atlas_virtual;
pub mod vertex_compression_r10g10;
pub mod temporal_mesh_diff;
pub mod visibility_graph;
pub mod micro_lod;
pub mod instanced_draw;

// ---- Category 2: Memory & Data Structure
pub mod mimalloc_config;
pub mod bump_arena;
pub mod pool_slab;
pub mod morton_order;
pub mod zerocopy_cast;
pub mod rayon_job;
pub mod dag_scheduler;
pub mod dashmap_registry;

// ---- Category 6: Compile-time / SIMD / Branchless
pub mod simd_kernels_avx2;
pub mod pgo_bolt;
pub mod branchless_block;

// ---- Complete Wiring Orchestrator (スタブ禁止: 全モジュールを本番フレームに配線)
pub mod full_graph_wiring;
