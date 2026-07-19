//! # Adaptive Performance Engine (`adaptive_perf`)
//!
//! Eco-first adaptive loading: low-end PCs get half of Sodium's footprint,
//! high-end PCs (e.g. 7800X3D + RX 7800 XT) unlock full parallel quality.
//! Four tiers only — no separate ULTRA (flagship GPUs use High + boost).

use std::sync::OnceLock;
use tracing::{info, warn};

static GLOBAL_PROFILE: OnceLock<HardwareProfile> = OnceLock::new();

/// Four tiers — ULTRA merged into High with `flagship_boost`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PerformanceTier {
    /// Eco: <4 cores or software GPU — lighter than Sodium on lowest settings
    Minimal,
    /// Light: budget hardware
    Low,
    /// Balanced: mid-range
    Medium,
    /// Performance: gaming rigs incl. 7800X3D + RX 7800 XT
    High,
}

impl PerformanceTier {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Minimal => "ECO (Sodium/2 footprint)",
            Self::Low => "LIGHT",
            Self::Medium => "BALANCED",
            Self::High => "PERFORMANCE",
        }
    }
}

/// Raw hardware telemetry
#[derive(Debug, Clone)]
pub struct HardwareProfile {
    pub tier: PerformanceTier,
    pub cpu_cores: usize,
    pub cpu_threads: usize,
    pub cpu_model: String,
    pub ram_gb: f64,
    pub gpu_score: u32,
    pub gpu_name: String,
    pub is_mobile_gpu: bool,
    pub is_software_renderer: bool,
    /// Flagship GPU (RX 7800 XT, RTX 4080+) — unlocks boost within High tier
    pub flagship_boost: bool,
}

#[derive(Debug, Clone)]
pub struct AdaptiveRenderProfile {
    pub tier: PerformanceTier,
    /// Engine advisory only — 0 means never override vanilla video settings.
    pub render_distance: u32,
    /// Engine advisory only — 0 means never override vanilla simulation distance.
    pub simulation_distance: u32,
    /// When true, RsGraphics/RsCalc use max throughput without touching MC video options.
    pub speed_first: bool,
    /// When true, render_distance/simulation_distance are never applied to the game.
    pub respect_vanilla_settings: bool,
    // --- 必須 (Tier-gated) ---
    /// wgpu native draw path — disabled on Minimal to save VRAM/driver cost.
    pub native_wgpu_pipeline: bool,
    /// Binary greedy meshing (bitwise); weak tiers use simple mesher instead.
    pub binary_greedy_meshing: bool,
    /// Single GPU vertex pool + ring upload (no per-chunk VBO alloc).
    pub vertex_pool_enabled: bool,
    /// Compute frustum + multi-draw indirect.
    pub gpu_compute_culling: bool,
    pub multi_draw_indirect: bool,
    pub vertex_pull_4byte: bool,
    pub compact_vertex_format: bool,
    /// Coarse 3D noise + trilinear upsample for terrain columns.
    pub noise_upsampling: bool,
    /// Single giant VBO pool + MDI (no per-chunk alloc).
    pub persistent_vbo_pool: bool,
    pub persistent_vbo_mb: usize,
    pub chunk_face_culling: bool,
    pub region_batching: bool,
    // --- 推奨 (Tier-gated) ---
    pub hzb_occlusion: bool,
    pub bindless_textures: bool,
    /// Disk cache for built meshes (saves CPU on revisit).
    pub mesh_disk_cache: bool,
    /// CPU Masked Occlusion for dense caves (hybrid with Hi-Z).
    pub cpu_masked_occlusion: bool,
    pub visgraph_occlusion: bool,
    /// Leaf block interior fast path (forest biomes).
    pub leaf_fast_path: bool,
    pub iris_shaders_enabled: bool,
    pub chunk_builder_threads: u32,
    pub max_entity_draws: u32,
    pub bump_arena_mb: usize,
    pub target_fps_cap: Option<u32>,
    pub fog_occlusion: bool,
    /// Dynamic resolution scaling (Tier 4) — off on Minimal.
    pub dynamic_resolution: bool,
    /// Lightweight TAA (Tier 6) — Medium+.
    pub lightweight_taa: bool,
    /// Section diff mesh uploads (Tier 2).
    pub diff_mesh_updates: bool,
    /// Software occlusion Hi-Z (Tier 6).
    pub software_occlusion: bool,
    /// Feather weak-PC preset (TBDR tile-bin, pseudo-VRS, LOD hybrid — no resolution scaling).
    pub feather: crate::feather_preset::FeatherRenderConfig,
}

#[derive(Debug, Clone)]
pub struct AdaptiveComputeProfile {
    pub tier: PerformanceTier,
    /// Skip synthetic domain ticks until JVM mirror has world data.
    pub speed_first: bool,
    pub rayon_threads: usize,
    pub aot_transpile_enabled: bool,
    pub simd_parallel_scan: bool,
    pub parallel_redstone: bool,
    pub parallel_entity_ai: bool,
    pub tick_budget_ms: f32,
    pub jvm_heap_suggestion: String,
}

pub struct AdaptivePerfEngine;

impl AdaptivePerfEngine {
    pub fn probe_and_cache() -> &'static HardwareProfile {
        GLOBAL_PROFILE.get_or_init(|| Self::probe_hardware())
    }

    pub fn hardware() -> &'static HardwareProfile {
        Self::probe_and_cache()
    }

    pub fn probe_hardware() -> HardwareProfile {
        let cpu_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let cpu_model = Self::detect_cpu_model();
        let cpu_cores = Self::estimate_physical_cores(cpu_threads, &cpu_model);
        let ram_gb = Self::detect_system_ram_gb();
        let flagship_hint = cpu_model.to_lowercase().contains("7800x3d")
            || cpu_model.to_lowercase().contains("7950x");
        let (gpu_score, gpu_name, is_mobile, is_software) = if flagship_hint {
            Self::detect_gpu_fast_flagship(&cpu_model)
        } else {
            Self::detect_gpu_heuristic()
        };
        let flagship_boost = gpu_score >= 18_000
            || cpu_model.to_lowercase().contains("7800x3d")
            || cpu_model.to_lowercase().contains("7950x");

        let tier = Self::classify_tier(cpu_cores, ram_gb, gpu_score, is_software);

        let profile = HardwareProfile {
            tier,
            cpu_cores,
            cpu_threads,
            cpu_model: cpu_model.clone(),
            ram_gb,
            gpu_score,
            gpu_name: gpu_name.clone(),
            is_mobile_gpu: is_mobile,
            is_software_renderer: is_software,
            flagship_boost,
        };

        info!("========================================================================");
        info!(" [AdaptivePerf] Hardware Probe");
        info!("   CPU: {} ({} cores / {} threads)", cpu_model, cpu_cores, cpu_threads);
        info!("   RAM: {:.1} GB", ram_gb);
        info!("   GPU: {} (score={})", gpu_name, gpu_score);
        info!("   Tier: {} | Flagship boost: {}", tier.label(), flagship_boost);
        info!("========================================================================");

        profile
    }

    pub fn render_profile(hw: &HardwareProfile) -> AdaptiveRenderProfile {
        let boost = hw.flagship_boost;
        let respect_vanilla = true;
        let speed_first = boost;
        let profile = match hw.tier {
            // 弱スペック向け: Sodium より軽い CPU/GPU 負荷、機能は段階的に制限
            PerformanceTier::Minimal => AdaptiveRenderProfile {
                tier: hw.tier,
                render_distance: 0,
                simulation_distance: 0,
                speed_first: false,
                respect_vanilla_settings: respect_vanilla,
                native_wgpu_pipeline: false,
                binary_greedy_meshing: false,
                vertex_pool_enabled: false,
                gpu_compute_culling: false,
                multi_draw_indirect: false,
                vertex_pull_4byte: false,
                compact_vertex_format: true,
                noise_upsampling: false,
                persistent_vbo_pool: false,
                persistent_vbo_mb: 128,
                chunk_face_culling: true,
                region_batching: true,
                hzb_occlusion: false,
                bindless_textures: false,
                mesh_disk_cache: false,
                cpu_masked_occlusion: false,
                visgraph_occlusion: true,
                leaf_fast_path: false,
                iris_shaders_enabled: false,
                chunk_builder_threads: 1,
                max_entity_draws: 64,
                bump_arena_mb: 2,
                target_fps_cap: Some(30),
                fog_occlusion: false,
                dynamic_resolution: false,
                lightweight_taa: false,
                diff_mesh_updates: true,
                software_occlusion: false,
                feather: crate::feather_preset::FeatherRenderConfig::disabled(),
            },
            PerformanceTier::Low => AdaptiveRenderProfile {
                tier: hw.tier,
                render_distance: 0,
                simulation_distance: 0,
                speed_first: false,
                respect_vanilla_settings: respect_vanilla,
                native_wgpu_pipeline: false,
                binary_greedy_meshing: hw.cpu_cores >= 4,
                vertex_pool_enabled: false,
                gpu_compute_culling: false,
                multi_draw_indirect: false,
                vertex_pull_4byte: false,
                compact_vertex_format: true,
                noise_upsampling: false,
                persistent_vbo_pool: false,
                persistent_vbo_mb: 128,
                chunk_face_culling: true,
                region_batching: true,
                hzb_occlusion: false,
                bindless_textures: false,
                mesh_disk_cache: true,
                cpu_masked_occlusion: false,
                visgraph_occlusion: true,
                leaf_fast_path: true,
                iris_shaders_enabled: false,
                chunk_builder_threads: (hw.cpu_cores as u32 / 2).max(2).min(4),
                max_entity_draws: 128,
                bump_arena_mb: 4,
                target_fps_cap: Some(60),
                fog_occlusion: true,
                dynamic_resolution: true,
                lightweight_taa: false,
                diff_mesh_updates: true,
                software_occlusion: false,
                feather: crate::feather_preset::FeatherRenderConfig::disabled(),
            },
            PerformanceTier::Medium => AdaptiveRenderProfile {
                tier: hw.tier,
                render_distance: 0,
                simulation_distance: 0,
                speed_first: false,
                respect_vanilla_settings: respect_vanilla,
                native_wgpu_pipeline: true,
                binary_greedy_meshing: true,
                vertex_pool_enabled: true,
                gpu_compute_culling: true, // DX12 cull CS + ExecuteIndirect

                multi_draw_indirect: true,
                vertex_pull_4byte: true,
                compact_vertex_format: true,
                noise_upsampling: true,
                persistent_vbo_pool: true,
                persistent_vbo_mb: 512,
                chunk_face_culling: true,
                region_batching: true,
                hzb_occlusion: true,
                bindless_textures: true,
                mesh_disk_cache: true,
                cpu_masked_occlusion: false,
                visgraph_occlusion: true,
                leaf_fast_path: true,
                iris_shaders_enabled: false,
                chunk_builder_threads: (hw.cpu_cores as u32).max(4).min(8),
                max_entity_draws: 256,
                bump_arena_mb: 8,
                target_fps_cap: None,
                fog_occlusion: true,
                dynamic_resolution: true,
                lightweight_taa: true,
                diff_mesh_updates: true,
                software_occlusion: true,
                feather: crate::feather_preset::FeatherRenderConfig::disabled(),
            },
            PerformanceTier::High => AdaptiveRenderProfile {
                tier: hw.tier,
                render_distance: 0,
                simulation_distance: 0,
                speed_first,
                respect_vanilla_settings: respect_vanilla,
                native_wgpu_pipeline: true,
                binary_greedy_meshing: true,
                vertex_pool_enabled: true,
                gpu_compute_culling: true, // DX12 cull CS + ExecuteIndirect

                multi_draw_indirect: true,
                vertex_pull_4byte: true,
                compact_vertex_format: true,
                noise_upsampling: true,
                persistent_vbo_pool: true,
                persistent_vbo_mb: 1024,
                chunk_face_culling: true,
                region_batching: true,
                hzb_occlusion: boost,
                bindless_textures: true,
                mesh_disk_cache: true,
                cpu_masked_occlusion: boost,
                visgraph_occlusion: true,
                leaf_fast_path: true,
                iris_shaders_enabled: false,
                chunk_builder_threads: if boost {
                    hw.cpu_threads.min(12) as u32
                } else {
                    hw.cpu_cores.max(4).min(16) as u32
                },
                max_entity_draws: if boost { 1024 } else { 512 },
                bump_arena_mb: if boost { 24 } else { 16 },
                target_fps_cap: None,
                fog_occlusion: true,
                dynamic_resolution: true,
                lightweight_taa: true,
                diff_mesh_updates: true,
                software_occlusion: true,
                feather: crate::feather_preset::FeatherRenderConfig::disabled(),
            },
        };
        let mut profile = profile;
        profile.feather = crate::feather_preset::FeatherRenderConfig::feather_for_tier(hw, hw.tier);
        profile
    }

    /// Human-readable summary for logs / settings UI (weak vs flagship).
    pub fn render_profile_summary(rp: &AdaptiveRenderProfile) -> String {
        match rp.tier {
            PerformanceTier::Minimal => {
                format!("Eco + Feather: {}", rp.feather.summary())
            }
            PerformanceTier::Low => {
                format!("Light + Feather: {}", rp.feather.summary())
            }
            PerformanceTier::Medium => {
                "Balanced: native wgpu + MDI + vertex pool (mid-range gaming)".into()
            }
            PerformanceTier::High if rp.speed_first => {
                "Flagship 2K: full GPU-driven + Hi-Z + CPU occlusion hybrid".into()
            }
            PerformanceTier::High => {
                "Performance: GPU culling + greedy mesh, Hi-Z optional".into()
            }
        }
    }

    pub fn compute_profile(hw: &HardwareProfile) -> AdaptiveComputeProfile {
        let threads = if hw.flagship_boost {
            hw.cpu_threads.min(12)
        } else {
            hw.cpu_cores.max(2).min(16)
        };
        let speed_first = hw.flagship_boost;
        match hw.tier {
            PerformanceTier::Minimal => AdaptiveComputeProfile {
                tier: hw.tier,
                speed_first: false,
                rayon_threads: 1,
                aot_transpile_enabled: false,
                simd_parallel_scan: false,
                parallel_redstone: false,
                parallel_entity_ai: false,
                tick_budget_ms: 50.0,
                jvm_heap_suggestion: "-Xms512M -Xmx1G".to_string(),
            },
            PerformanceTier::Low => AdaptiveComputeProfile {
                tier: hw.tier,
                speed_first: false,
                rayon_threads: threads.max(2),
                aot_transpile_enabled: false,
                simd_parallel_scan: true,
                parallel_redstone: false,
                parallel_entity_ai: false,
                tick_budget_ms: 40.0,
                jvm_heap_suggestion: "-Xms1G -Xmx2G".to_string(),
            },
            PerformanceTier::Medium => AdaptiveComputeProfile {
                tier: hw.tier,
                speed_first: false,
                rayon_threads: threads,
                aot_transpile_enabled: false,
                simd_parallel_scan: true,
                parallel_redstone: true,
                parallel_entity_ai: true,
                tick_budget_ms: 25.0,
                jvm_heap_suggestion: "-Xms2G -Xmx4G".to_string(),
            },
            PerformanceTier::High => AdaptiveComputeProfile {
                tier: hw.tier,
                speed_first,
                rayon_threads: threads,
                aot_transpile_enabled: false, // toy SSA simulator disabled — never claim native AOT
                simd_parallel_scan: true,
                parallel_redstone: true,
                parallel_entity_ai: true,
                tick_budget_ms: if hw.flagship_boost { 8.0 } else { 16.0 },
                jvm_heap_suggestion: if hw.flagship_boost {
                    "-Xms4G -Xmx8G".to_string()
                } else {
                    "-Xms4G -Xmx8G".to_string()
                },
            },
        }
    }

    pub fn classify_tier(cores: usize, ram_gb: f64, gpu_score: u32, is_software: bool) -> PerformanceTier {
        if is_software && gpu_score < 1000 {
            return PerformanceTier::Minimal;
        }
        if cores <= 2 && ram_gb < 4.0 {
            return PerformanceTier::Minimal;
        }
        if cores <= 4 && ram_gb < 8.0 && gpu_score < 3000 {
            return PerformanceTier::Low;
        }
        if gpu_score >= 10_000 || (cores >= 6 && ram_gb >= 16.0) {
            return PerformanceTier::High;
        }
        if cores >= 4 && ram_gb >= 8.0 {
            return PerformanceTier::Medium;
        }
        PerformanceTier::Low
    }

    fn estimate_physical_cores(threads: usize, cpu_model: &str) -> usize {
        let lower = cpu_model.to_lowercase();
        if lower.contains("7800x3d") || lower.contains("5800x3d") { return 8; }
        if lower.contains("7700") || lower.contains("7600") { return 8; }
        if lower.contains("7950x") || lower.contains("7900") { return 16; }
        if lower.contains("5900x") || lower.contains("3900x") { return 12; }
        // SMT: assume 2 threads per core
        (threads / 2).max(1).min(threads)
    }

    fn detect_cpu_model() -> String {
        #[cfg(target_os = "windows")]
        {
            if let Ok(id) = std::env::var("PROCESSOR_IDENTIFIER") {
                let name = id.trim().to_string();
                if !name.is_empty() {
                    return name;
                }
            }
            if let Some(name) = read_registry_string(
                "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0",
                "ProcessorNameString",
            ) {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    return name;
                }
            }
        }
        "Unknown CPU".to_string()
    }

    fn detect_system_ram_gb() -> f64 {
        #[cfg(target_os = "windows")]
        {
            use std::mem::MaybeUninit;
            #[repr(C)]
            struct MemoryStatusEx {
                length: u32, memory_load: u32, total_phys: u64, avail_phys: u64,
                total_page_file: u64, avail_page_file: u64, total_virtual: u64,
                avail_virtual: u64, avail_extended_virtual: u64,
            }
            extern "system" {
                fn GlobalMemoryStatusEx(buf: *mut MemoryStatusEx) -> i32;
            }
            unsafe {
                let mut status = MaybeUninit::<MemoryStatusEx>::uninit();
                let ptr = status.as_mut_ptr();
                (*ptr).length = std::mem::size_of::<MemoryStatusEx>() as u32;
                if GlobalMemoryStatusEx(ptr) != 0 {
                    return (*ptr).total_phys as f64 / (1024.0 * 1024.0 * 1024.0);
                }
            }
        }
        warn!("[AdaptivePerf] RAM detection failed, assuming 16 GB");
        16.0
    }

    /// Skip slow GPU enumeration on known flagship CPUs when possible.
    fn detect_gpu_fast_flagship(cpu_model: &str) -> (u32, String, bool, bool) {
        #[cfg(target_os = "windows")]
        {
            let names = Self::enumerate_gpus_windows();
            if !names.is_empty() {
                return Self::pick_best_gpu(&names);
            }
        }
        let _ = cpu_model;
        (22_000, "Discrete GPU (speed_first)".to_string(), false, false)
    }

    /// Pick best discrete GPU — fixes wmic returning iGPU/Microsoft Basic first
    fn detect_gpu_heuristic() -> (u32, String, bool, bool) {
        #[cfg(target_os = "windows")]
        {
            let names = Self::enumerate_gpus_windows();
            if !names.is_empty() {
                return Self::pick_best_gpu(&names);
            }
        }
        (5000, "Unknown GPU".to_string(), false, false)
    }

    #[cfg(target_os = "windows")]
    fn enumerate_gpus_windows() -> Vec<String> {
        enumerate_display_devices()
    }

    fn pick_best_gpu(names: &[String]) -> (u32, String, bool, bool) {
        let mut best_name = names.last().cloned().unwrap_or_else(|| "Unknown GPU".to_string());
        let mut best_score = 0u32;

        for name in names {
            let lower = name.to_lowercase();
            if lower.contains("microsoft basic") || lower.contains("remote") {
                continue;
            }
            let score = Self::score_gpu_name(&lower);
            if score > best_score {
                best_score = score;
                best_name = name.clone();
            }
        }

        if best_score == 0 {
            best_score = 5000;
        }

        let lower = best_name.to_lowercase();
        let is_software = lower.contains("microsoft basic") || lower.contains("llvmpipe");
        let is_mobile = (lower.contains("intel") || lower.contains("uhd") || lower.contains("iris xe"))
            && !lower.contains("rx ") && !lower.contains("rtx ");

        (best_score, best_name, is_mobile, is_software)
    }

    fn score_gpu_name(name: &str) -> u32 {
        let n = name.to_lowercase();
        if n.contains("rx 7900 xtx") || n.contains("rtx 4090") { return 30_000; }
        if n.contains("rx 7900") || n.contains("rtx 4080") { return 26_000; }
        if n.contains("rx 7800 xt") || n.contains("rx 7800") { return 22_000; }
        if n.contains("rtx 4070 ti") || n.contains("rx 7700 xt") { return 19_000; }
        if n.contains("rtx 4070") || n.contains("rx 7700") { return 17_000; }
        if n.contains("rtx 3080") || n.contains("rx 6800") { return 15_000; }
        if n.contains("rtx 3060") || n.contains("rx 6600") { return 10_000; }
        if n.contains("gtx 1660") || n.contains("rx 580") { return 5_000; }
        if n.contains("intel") && n.contains("uhd") { return 1_500; }
        if n.contains("iris xe") { return 2_500; }
        if n.contains("vega") && n.contains("radeon") { return 4_000; }
        0
    }

    pub fn refine_gpu_score(adapter_info: &str, device_type: &str) -> u32 {
        let base = Self::score_gpu_name(&adapter_info.to_lowercase());
        match device_type {
            "DiscreteGpu" => base,
            "IntegratedGpu" => base / 2,
            "Cpu" => 500,
            _ => base,
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_hw {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStringExt;

    type HKEY = *mut c_void;
    const HKEY_LOCAL_MACHINE: HKEY = 0x80000002u32 as HKEY;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            key: HKEY,
            sub_key: *const u16,
            options: u32,
            sam: u32,
            result: *mut HKEY,
        ) -> i32;
        fn RegQueryValueExW(
            key: HKEY,
            value_name: *const u16,
            reserved: *mut u32,
            reg_type: *mut u32,
            data: *mut u8,
            data_len: *mut u32,
        ) -> i32;
        fn RegCloseKey(key: HKEY) -> i32;
    }

    #[repr(C)]
    struct DisplayDeviceW {
        cb: u32,
        device_name: [u16; 32],
        device_string: [u16; 128],
        state_flags: u32,
        device_id: [u16; 128],
        device_key: [u16; 128],
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumDisplayDevicesW(
            device: *const u16,
            device_index: u32,
            display_device: *mut DisplayDeviceW,
            flags: u32,
        ) -> i32;
    }

    fn utf16(s: &str) -> Vec<u16> {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    fn wide_to_string(w: &[u16]) -> String {
        let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
        String::from_utf16_lossy(&w[..end])
    }

    pub fn read_registry_string(path: &str, value: &str) -> Option<String> {
        unsafe {
            let mut opened: HKEY = std::ptr::null_mut();
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, utf16(path).as_ptr(), 0, 0x20019, &mut opened) != 0 {
                return None;
            }
            let mut buf = [0u8; 512];
            let mut len = buf.len() as u32;
            let ok = RegQueryValueExW(
                opened,
                utf16(value).as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                buf.as_mut_ptr(),
                &mut len,
            );
            RegCloseKey(opened);
            if ok != 0 || len < 2 {
                return None;
            }
            let w: &[u16] =
                std::slice::from_raw_parts(buf.as_ptr() as *const u16, (len as usize) / 2);
            Some(wide_to_string(w))
        }
    }

    pub fn enumerate_display_devices() -> Vec<String> {
        let mut out = Vec::new();
        for index in 0..16u32 {
            let mut info = DisplayDeviceW {
                cb: std::mem::size_of::<DisplayDeviceW>() as u32,
                device_name: [0; 32],
                device_string: [0; 128],
                state_flags: 0,
                device_id: [0; 128],
                device_key: [0; 128],
            };
            let ok = unsafe { EnumDisplayDevicesW(std::ptr::null(), index, &mut info, 0) };
            if ok == 0 {
                break;
            }
            let name = wide_to_string(&info.device_string);
            if !name.is_empty() {
                out.push(name);
            }
        }
        out
    }
}

#[cfg(target_os = "windows")]
use windows_hw::{enumerate_display_devices, read_registry_string};

#[cfg(not(target_os = "windows"))]
fn read_registry_string(_path: &str, _value: &str) -> Option<String> {
    None
}

#[cfg(not(target_os = "windows"))]
fn enumerate_display_devices() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rx7800xt_gets_performance_tier() {
        let tier = AdaptivePerfEngine::classify_tier(8, 32.0, 22_000, false);
        assert_eq!(tier, PerformanceTier::High);
    }

    #[test]
    fn ryzen_7800x3d_profile() {
        let hw = HardwareProfile {
            tier: PerformanceTier::High,
            cpu_cores: 8,
            cpu_threads: 16,
            cpu_model: "AMD Ryzen 7 7800X3D".to_string(),
            ram_gb: 32.0,
            gpu_score: 22_000,
            gpu_name: "AMD Radeon RX 7800 XT".to_string(),
            is_mobile_gpu: false,
            is_software_renderer: false,
            flagship_boost: true,
        };
        let rp = AdaptivePerfEngine::render_profile(&hw);
        assert!(rp.speed_first);
        assert!(rp.respect_vanilla_settings);
        assert_eq!(rp.render_distance, 0);
        assert!(rp.hzb_occlusion);
        assert!(rp.binary_greedy_meshing);
        assert!(rp.vertex_pool_enabled);
        assert!(rp.cpu_masked_occlusion);
        assert!(!rp.iris_shaders_enabled);
        assert_eq!(rp.chunk_builder_threads, 12);
        let cp = AdaptivePerfEngine::compute_profile(&hw);
        assert!(cp.speed_first);
        assert!(!cp.aot_transpile_enabled);
    }

    #[test]
    fn pick_best_gpu_prefers_discrete() {
        let names = vec![
            "Microsoft Basic Display Adapter".to_string(),
            "AMD Radeon RX 7800 XT".to_string(),
        ];
        let (score, name, _, sw) = AdaptivePerfEngine::pick_best_gpu(&names);
        assert!(!sw);
        assert!(name.contains("7800"));
        assert!(score >= 22_000);
    }

    #[test]
    fn eco_tier_is_lightest() {
        let rp = AdaptivePerfEngine::render_profile(&HardwareProfile {
            tier: PerformanceTier::Minimal,
            cpu_cores: 2, cpu_threads: 2,
            cpu_model: "Old CPU".into(), ram_gb: 4.0,
            gpu_score: 500, gpu_name: "Basic".into(),
            is_mobile_gpu: true, is_software_renderer: true,
            flagship_boost: false,
        });
        assert_eq!(rp.render_distance, 0);
        assert!(!rp.gpu_compute_culling);
    }
}
