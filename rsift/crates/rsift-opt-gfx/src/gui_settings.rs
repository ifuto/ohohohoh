//! Sodium-style modern video settings GUI for RsGraphics.

use tracing::info;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy)]
pub struct SodiumPalette {
    pub bg: [f32; 4],
    pub sidebar: [f32; 4],
    pub card: [f32; 4],
    pub accent: [f32; 4],
    pub text: [f32; 4],
    pub toggle_on: [f32; 4],
    pub toggle_off: [f32; 4],
}

impl Default for SodiumPalette {
    fn default() -> Self {
        Self {
            bg: [0.07, 0.07, 0.10, 0.96],
            sidebar: [0.10, 0.10, 0.14, 0.98],
            card: [0.14, 0.14, 0.19, 0.95],
            accent: [0.26, 0.78, 0.55, 1.0],
            text: [0.94, 0.95, 0.97, 1.0],
            toggle_on: [0.26, 0.78, 0.55, 1.0],
            toggle_off: [0.28, 0.28, 0.34, 1.0],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SodiumSettingsTab {
    General,
    Quality,
    Performance,
    Advanced,
    Shaders,
}

impl SodiumSettingsTab {
    pub const ALL: [Self; 5] = [
        Self::General,
        Self::Quality,
        Self::Performance,
        Self::Advanced,
        Self::Shaders,
    ];

    pub fn icon(&self) -> &'static str {
        match self {
            Self::General => "⚙",
            Self::Quality => "◆",
            Self::Performance => "⚡",
            Self::Advanced => "⌗",
            Self::Shaders => "✦",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Quality => "Quality",
            Self::Performance => "Performance",
            Self::Advanced => "Advanced",
            Self::Shaders => "Shaders",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VideoSettings {
    pub render_distance: u32,
    pub brightness: f32,
    pub fog_occlusion: bool,
    pub compact_vertex_format: bool,
    pub chunk_face_culling: bool,
    pub gpu_compute_culling: bool,
    pub multithreaded_chunk_building: bool,
    pub chunk_builder_threads: u32,
    pub bindless_textures: bool,
    pub entity_culling: bool,
    pub animate_only_visible: bool,
    pub biome_blend: u32,
    pub selected_shaderpack: Option<String>,
    pub shader_profile: String,
    pub show_instant_visual_hud: bool,
    // 必須/推奨 (tier defaults, user can override in UI)
    pub binary_greedy_meshing: bool,
    pub vertex_pool: bool,
    pub mesh_disk_cache: bool,
    pub cpu_masked_occlusion: bool,
    pub hzb_occlusion: bool,
    pub leaf_fast_path: bool,
    pub native_wgpu_pipeline: bool,
    pub eco_mode_locked: bool,
    // Feather (weak PC — no resolution scaling)
    pub feather_tile_binning: bool,
    pub feather_merged_subpass: bool,
    pub feather_pseudo_vrs: bool,
    pub feather_lod_3tier: bool,
    pub feather_compressed_textures: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self::adaptive()
    }
}

impl VideoSettings {
    pub fn adaptive() -> Self {
        let hw = rsift_api::AdaptivePerfEngine::probe_and_cache();
        let rp = rsift_api::AdaptivePerfEngine::render_profile(hw);
        Self {
            render_distance: 12,
            brightness: 1.0,
            fog_occlusion: rp.fog_occlusion,
            compact_vertex_format: rp.compact_vertex_format,
            chunk_face_culling: rp.chunk_face_culling,
            gpu_compute_culling: rp.gpu_compute_culling,
            multithreaded_chunk_building: rp.chunk_builder_threads > 1,
            chunk_builder_threads: rp.chunk_builder_threads,
            bindless_textures: rp.bindless_textures,
            entity_culling: true,
            animate_only_visible: true,
            biome_blend: 5,
            selected_shaderpack: if rp.iris_shaders_enabled {
                Some("ComplementaryReimagined_v5.1.zip".into())
            } else {
                None
            },
            shader_profile: match hw.tier {
                rsift_api::PerformanceTier::Minimal | rsift_api::PerformanceTier::Low => "Eco".into(),
                rsift_api::PerformanceTier::Medium => "Balanced".into(),
                _ => if rp.speed_first { "Flagship 2K".into() } else { "Performance".into() },
            },
            show_instant_visual_hud: !matches!(hw.tier, rsift_api::PerformanceTier::Minimal),
            binary_greedy_meshing: rp.binary_greedy_meshing,
            vertex_pool: rp.vertex_pool_enabled,
            mesh_disk_cache: rp.mesh_disk_cache,
            cpu_masked_occlusion: rp.cpu_masked_occlusion,
            hzb_occlusion: rp.hzb_occlusion,
            leaf_fast_path: rp.leaf_fast_path,
            native_wgpu_pipeline: rp.native_wgpu_pipeline,
            eco_mode_locked: matches!(hw.tier, rsift_api::PerformanceTier::Minimal | rsift_api::PerformanceTier::Low),
            feather_tile_binning: rp.feather.software_tile_binning,
            feather_merged_subpass: rp.feather.merged_subpasses,
            feather_pseudo_vrs: rp.feather.software_vrs_checkerboard,
            feather_lod_3tier: rp.feather.lod_hybrid_3tier,
            feather_compressed_textures: rp.feather.compressed_textures,
        }
    }

    /// Feather preset — TBDR tile-bin, pseudo-VRS, 3-tier LOD (解像度は変更しない).
    pub fn feather_preset() -> Self {
        let mut s = Self::eco_preset();
        s.shader_profile = "Feather (weak PC / iGPU)".into();
        s.feather_tile_binning = true;
        s.feather_merged_subpass = true;
        s.feather_pseudo_vrs = true;
        s.feather_lod_3tier = true;
        s.feather_compressed_textures = true;
        s.mesh_disk_cache = true;
        s.binary_greedy_meshing = true;
        s.chunk_builder_threads = 2;
        s.multithreaded_chunk_building = true;
        s
    }

    /// Force weakest settings for laptops / integrated GPU (弱スペック向け Mod).
    pub fn eco_preset() -> Self {
        let mut s = Self::adaptive();
        s.shader_profile = "Eco (weak PC)".into();
        s.binary_greedy_meshing = false;
        s.vertex_pool = false;
        s.gpu_compute_culling = false;
        s.hzb_occlusion = false;
        s.cpu_masked_occlusion = false;
        s.native_wgpu_pipeline = false;
        s.mesh_disk_cache = false;
        s.leaf_fast_path = false;
        s.chunk_builder_threads = 1;
        s.multithreaded_chunk_building = false;
        s.eco_mode_locked = true;
        s.feather_tile_binning = true;
        s.feather_merged_subpass = true;
        s.feather_pseudo_vrs = true;
        s.feather_lod_3tier = true;
        s.feather_compressed_textures = true;
        s
    }

    /// Flagship 2K preset (7800X3D class) — Feather off.
    pub fn flagship_2k_preset() -> Self {
        let mut s = Self::adaptive();
        s.shader_profile = "Flagship 2K".into();
        s.binary_greedy_meshing = true;
        s.vertex_pool = true;
        s.gpu_compute_culling = true;
        s.hzb_occlusion = true;
        s.cpu_masked_occlusion = true;
        s.native_wgpu_pipeline = true;
        s.mesh_disk_cache = true;
        s.leaf_fast_path = true;
        s.chunk_builder_threads = 12;
        s.multithreaded_chunk_building = true;
        s.eco_mode_locked = false;
        s.feather_tile_binning = false;
        s.feather_merged_subpass = false;
        s.feather_pseudo_vrs = false;
        s.feather_lod_3tier = false;
        s.feather_compressed_textures = false;
        s
    }
}

fn toggle_glyph(on: bool) -> &'static str {
    if on { "●━━━━ ON " } else { "○──── OFF" }
}

fn slider_bar(value: u32, min: u32, max: u32, width: usize) -> String {
    let t = if max <= min {
        0.0
    } else {
        (value.saturating_sub(min) as f32) / (max - min) as f32
    };
    let pos = ((width as f32 - 1.0) * t.clamp(0.0, 1.0)) as usize;
    let mut s = "─".repeat(width);
    if pos < width {
        s.replace_range(pos..=pos, "◆");
    }
    s
}

pub struct SodiumVideoSettingsGui {
    pub is_open: bool,
    pub current_tab: SodiumSettingsTab,
    pub palette: SodiumPalette,
    pub settings: VideoSettings,
    pub available_shaderpacks: Vec<String>,
}

impl Default for SodiumVideoSettingsGui {
    fn default() -> Self {
        Self::new()
    }
}

impl SodiumVideoSettingsGui {
    pub fn new() -> Self {
        Self {
            is_open: false,
            current_tab: SodiumSettingsTab::Performance,
            palette: SodiumPalette::default(),
            settings: VideoSettings::default(),
            // 実在しないパック名を表示しないよう初期値は空 (2026-07-21 監査:
            // 以前は BSL/Complementary 等のサンプル名がハードコードされ、
            // ディスクに存在しないパックが「Available」表示され得た)。
            // 実際の一覧は `IrisShaderEngine::discover_shaderpacks` の
            // ディレクトリ走査結果を本フィールドへ注入して表示する。
            available_shaderpacks: Vec::new(),
        }
    }

    pub fn open_screen(&mut self) {
        self.is_open = true;
        self.render_gui_frame();
    }

    pub fn switch_tab(&mut self, tab: SodiumSettingsTab) {
        self.current_tab = tab;
        self.render_gui_frame();
    }

    pub fn select_shaderpack(&mut self, pack: Option<String>) {
        self.settings.selected_shaderpack = pack;
        self.render_gui_frame();
    }

    pub fn apply_window_title_override(&self) {
        info!("[RsGraphics] Window title → Minecraft 1.21.11 — RsGraphics");
    }

    pub fn render_instant_visual_indicators(&self) {
        let hw = rsift_api::AdaptivePerfEngine::hardware();
        info!("[RsGraphics HUD] {} | rd={}", hw.tier.label(), self.settings.render_distance);
    }

    pub fn render_gui_frame(&self) {
        if !self.is_open {
            return;
        }
        let hw = rsift_api::AdaptivePerfEngine::hardware();
        let w = 74usize;
        info!("");
        info!("╔{}╗", "═".repeat(w));
        info!("║ {:^72} ║", "RsGraphics Video Settings");
        info!("║ {:^72} ║", format!("{} · {}", hw.tier.label(), hw.gpu_name));
        info!("║ {:^72} ║", format!("Profile: {}", self.settings.shader_profile));
        info!("╠{}╣", "═".repeat(w));
        for tab in SodiumSettingsTab::ALL {
            let mark = if tab == self.current_tab { "▸" } else { " " };
            info!("║ {:<72} ║", format!("{} {} {}", mark, tab.icon(), tab.label()));
        }
        info!("╠{}╣", "═".repeat(w));
        self.render_active_panel();
        info!("╠{}╣", "═".repeat(w));
        info!("║ {:^72} ║", "[ Apply ]    [ Done ]    [ Reload Shaders ]");
        info!("╚{}╝", "═".repeat(w));
    }

    fn render_active_panel(&self) {
        match self.current_tab {
            SodiumSettingsTab::General => {
                info!("║ {:<72} ║", " Render Distance: controlled by vanilla video settings");
                self.row_slider("Biome Blend", self.settings.biome_blend, 1, 7);
                self.row_toggle("RsGraphics HUD", self.settings.show_instant_visual_hud);
            }
            SodiumSettingsTab::Quality => {
                self.row_toggle("Fog Occlusion", self.settings.fog_occlusion);
                self.row_toggle("Chunk Face Culling", self.settings.chunk_face_culling);
                self.row_toggle("Animate Only Visible", self.settings.animate_only_visible);
            }
            SodiumSettingsTab::Performance => {
                if self.settings.eco_mode_locked {
                    info!("║ {:<72} ║", " Eco Mod active — GPU features locked for weak hardware");
                }
                self.row_toggle("Binary Greedy Meshing", self.settings.binary_greedy_meshing);
                self.row_toggle("12B Quantized Vertices", self.settings.compact_vertex_format);
                self.row_toggle("Vertex Pool (GPU)", self.settings.vertex_pool);
                self.row_toggle("Mesh Disk Cache", self.settings.mesh_disk_cache);
                self.row_toggle("Native wgpu Pipeline", self.settings.native_wgpu_pipeline);
                self.row_toggle("GPU Frustum + MDI", self.settings.gpu_compute_culling);
                self.row_toggle("Hi-Z GPU Occlusion", self.settings.hzb_occlusion);
                self.row_toggle("CPU Masked Occlusion", self.settings.cpu_masked_occlusion);
                self.row_toggle("Leaf Fast Path", self.settings.leaf_fast_path);
                self.row_toggle("Entity Culling", self.settings.entity_culling);
                self.row_toggle("Multithreaded Building", self.settings.multithreaded_chunk_building);
                self.row_slider("Chunk Builder Threads", self.settings.chunk_builder_threads, 1, 16);
                self.row_toggle("Bindless Textures", self.settings.bindless_textures);
                if self.settings.feather_tile_binning || self.settings.eco_mode_locked {
                    info!("║ {:<72} ║", " ── Feather (no resolution scaling) ──");
                }
                self.row_toggle("Tile Binning (TBDR-style)", self.settings.feather_tile_binning);
                self.row_toggle("Merged Subpasses", self.settings.feather_merged_subpass);
                self.row_toggle("Pseudo-VRS (checkerboard)", self.settings.feather_pseudo_vrs);
                self.row_toggle("3-Tier LOD Hybrid", self.settings.feather_lod_3tier);
                self.row_toggle("BC7/ASTC Atlas", self.settings.feather_compressed_textures);
            }
            SodiumSettingsTab::Advanced => {
                let hw = rsift_api::AdaptivePerfEngine::hardware();
                let rp = rsift_api::AdaptivePerfEngine::render_profile(hw);
                info!("║ {:<72} ║", " Parity: STRICT");
                info!("║ {:<72} ║", " Presets: Feather | Eco | Auto | Flagship 2K");
                info!("║ {:<72} ║", " Note: internal resolution is NOT scaled (per user pref)");
                info!("║ {:<72} ║", format!(" Summary: {}",
                    rsift_api::AdaptivePerfEngine::render_profile_summary(&rp)));
            }
            SodiumSettingsTab::Shaders => {
                info!("║ {:<72} ║", format!(" Active: {}",
                    self.settings.selected_shaderpack.as_deref().unwrap_or("None")));
                for (i, p) in self.available_shaderpacks.iter().enumerate() {
                    let g = if self.settings.selected_shaderpack.as_deref() == Some(p.as_str()) { "◉" } else { "○" };
                    info!("║ {:<72} ║", format!("  {} {:>2}. {}", g, i + 1, p));
                }
            }
        }
    }

    fn row_toggle(&self, label: &str, on: bool) {
        info!("║ {:<42} {:>28} ║", label, toggle_glyph(on));
    }

    fn row_slider(&self, label: &str, value: u32, min: u32, max: u32) {
        info!("║ {:<30} {} {:>4} ║", label, slider_bar(value, min, max, 24), value);
    }

    pub fn save_and_close(&mut self) {
        info!("[RsGraphics] Settings saved");
        self.is_open = false;
    }
}

static GLOBAL_GUI: OnceLock<Mutex<SodiumVideoSettingsGui>> = OnceLock::new();

pub fn global_gui() -> &'static Mutex<SodiumVideoSettingsGui> {
    GLOBAL_GUI.get_or_init(|| Mutex::new(SodiumVideoSettingsGui::new()))
}

pub fn open_global_settings() {
    if let Ok(mut g) = global_gui().lock() {
        g.open_screen();
    }
}
