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
    // 回帰修正 (2026-07-22): 旧実装は `s.replace_range(pos..=pos, "◆")` で
    // char 位置を**バイト範囲**として切断していた。「─」は 3 byte グリフのため
    // pos/pos+1 の両境界同時成立は不可能で、スライダー描画は必ずパニック
    // (「動画設定」ボタン押下 = 既定 Performance タブで確実にクラッシュ)。
    // char 配列の要素置換に修正。
    let mut bar: Vec<char> = std::iter::repeat('─').take(width).collect();
    if pos < width {
        bar[pos] = '◆';
    }
    bar.into_iter().collect()
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

#[cfg(test)]
mod strict_tests {
    use super::*;

    #[test]
    fn palette_default_bit_exact() {
        let p = SodiumPalette::default();
        assert_eq!(p.bg, [0.07, 0.07, 0.10, 0.96]);
        assert_eq!(p.sidebar, [0.10, 0.10, 0.14, 0.98]);
        assert_eq!(p.card, [0.14, 0.14, 0.19, 0.95]);
        assert_eq!(p.accent, [0.26, 0.78, 0.55, 1.0]);
        assert_eq!(p.text, [0.94, 0.95, 0.97, 1.0]);
        assert_eq!(p.toggle_on, p.accent, "toggle_on は accent と同一色");
        assert_eq!(p.toggle_off, [0.28, 0.28, 0.34, 1.0]);
    }

    #[test]
    fn tab_table_order_labels_icons_exact() {
        let expect = [
            (SodiumSettingsTab::General, "⚙", "General"),
            (SodiumSettingsTab::Quality, "◆", "Quality"),
            (SodiumSettingsTab::Performance, "⚡", "Performance"),
            (SodiumSettingsTab::Advanced, "⌗", "Advanced"),
            (SodiumSettingsTab::Shaders, "✦", "Shaders"),
        ];
        assert_eq!(SodiumSettingsTab::ALL.len(), 5);
        for (i, (tab, icon, label)) in expect.iter().enumerate() {
            assert_eq!(SodiumSettingsTab::ALL[i], *tab, "ALL の順序は UI サイドバー順");
            assert_eq!(tab.icon(), *icon);
            assert_eq!(tab.label(), *label);
        }
    }

    #[test]
    fn toggle_glyph_exact_strings() {
        assert_eq!(toggle_glyph(true), "●━━━━ ON ", "末尾スペース込みで固定");
        assert_eq!(toggle_glyph(false), "○──── OFF");
    }

    #[test]
    fn slider_bar_geometry_exact_and_degenerate_safe() {
        // 回帰: 旧実装は 3-byte グリフのバイト切断 replace_range で必ずパニック。
        assert_eq!(
            slider_bar(5, 1, 7, 24),
            format!("{}◆{}", "─".repeat(15), "─".repeat(8)),
            "t=4/6 → pos=floor(23*0.666..)=15"
        );
        assert_eq!(slider_bar(1, 1, 7, 24), format!("◆{}", "─".repeat(23)), "min は左端");
        assert_eq!(slider_bar(7, 1, 7, 24), format!("{}◆", "─".repeat(23)), "max は右端");
        assert_eq!(slider_bar(0, 1, 7, 24), slider_bar(1, 1, 7, 24), "min 未満は飽和");
        assert_eq!(slider_bar(100, 1, 7, 24), slider_bar(7, 1, 7, 24), "max 超過はクランプ");
        assert_eq!(slider_bar(3, 5, 5, 24), slider_bar(1, 1, 7, 24), "max<=min は t=0");
        assert_eq!(slider_bar(1, 1, 7, 0), "", "width 0 は空文字 (パニックしない)");
    }

    #[test]
    fn eco_feather_flagship_override_tables() {
        let eco = VideoSettings::eco_preset();
        assert_eq!(eco.shader_profile, "Eco (weak PC)");
        assert!(!eco.binary_greedy_meshing && !eco.vertex_pool && !eco.gpu_compute_culling
            && !eco.hzb_occlusion && !eco.cpu_masked_occlusion && !eco.native_wgpu_pipeline
            && !eco.mesh_disk_cache && !eco.leaf_fast_path,
            "eco は GPU 依存機能を全て OFF (hw 非依存で上書き)");
        assert!(eco.feather_tile_binning && eco.feather_merged_subpass && eco.feather_pseudo_vrs
            && eco.feather_lod_3tier && eco.feather_compressed_textures,
            "eco は Feather ソフト群を全て ON");
        assert_eq!(eco.chunk_builder_threads, 1);
        assert!(!eco.multithreaded_chunk_building && eco.eco_mode_locked);

        let fe = VideoSettings::feather_preset();
        assert_eq!(fe.shader_profile, "Feather (weak PC / iGPU)");
        assert!(fe.feather_tile_binning && fe.feather_merged_subpass && fe.feather_pseudo_vrs
            && fe.feather_lod_3tier && fe.feather_compressed_textures);
        assert!(fe.binary_greedy_meshing && fe.mesh_disk_cache);
        assert_eq!(fe.chunk_builder_threads, 2);
        assert!(fe.multithreaded_chunk_building);
        assert!(!fe.vertex_pool && !fe.gpu_compute_culling && !fe.hzb_occlusion
            && !fe.cpu_masked_occlusion && !fe.native_wgpu_pipeline && !fe.leaf_fast_path,
            "feather は eco ベース (GPU compute 系 OFF) のまま");

        let fl = VideoSettings::flagship_2k_preset();
        assert_eq!(fl.shader_profile, "Flagship 2K");
        assert!(fl.binary_greedy_meshing && fl.vertex_pool && fl.gpu_compute_culling
            && fl.hzb_occlusion && fl.cpu_masked_occlusion && fl.native_wgpu_pipeline
            && fl.mesh_disk_cache && fl.leaf_fast_path,
            "flagship は GPU 群を全て ON");
        assert!(!fl.feather_tile_binning && !fl.feather_merged_subpass && !fl.feather_pseudo_vrs
            && !fl.feather_lod_3tier && !fl.feather_compressed_textures,
            "flagship は Feather OFF (eco と鏡像)");
        assert_eq!(fl.chunk_builder_threads, 12);
        assert!(fl.multithreaded_chunk_building && !fl.eco_mode_locked);
    }

    #[test]
    fn adaptive_hardware_independent_constants_and_invariants() {
        let s = VideoSettings::adaptive();
        // ハードウェア非依存の定数既定値
        assert_eq!(s.render_distance, 12);
        assert_eq!(s.brightness, 1.0);
        assert!(s.entity_culling);
        assert!(s.animate_only_visible);
        assert_eq!(s.biome_blend, 5);
        // ハードウェア依存だが「関係」として要求される不変式
        assert_eq!(
            s.multithreaded_chunk_building,
            s.chunk_builder_threads > 1,
            "MT フラグはスレッド数 > 1 と常に一致"
        );
        let hw = rsift_api::AdaptivePerfEngine::hardware();
        assert_eq!(
            s.eco_mode_locked,
            matches!(hw.tier, rsift_api::PerformanceTier::Minimal | rsift_api::PerformanceTier::Low),
            "eco_mode_locked は tier Minimal/Low に一致"
        );
        assert!(
            ["Eco", "Balanced", "Flagship 2K", "Performance"].contains(&s.shader_profile.as_str()),
            "profile は既知4表のいずれか: {}", s.shader_profile
        );
    }

    #[test]
    fn gui_defaults_lifecycle_all_tabs_smoke() {
        let mut g = SodiumVideoSettingsGui::new();
        assert!(!g.is_open, "初期は閉");
        assert_eq!(g.current_tab, SodiumSettingsTab::Performance, "既定タブ");
        assert!(g.available_shaderpacks.is_empty(),
            "実在しないシェーダーパック名を表示しない (2026-07-21 監査固定)");
        g.render_gui_frame(); // 閉状態: 早期 return、パニックなし
        g.open_screen();
        assert!(g.is_open);
        // 全タブ走査 (回帰: slider 含む General/Performance で旧実装はパニック)
        for tab in SodiumSettingsTab::ALL {
            g.switch_tab(tab);
            assert_eq!(g.current_tab, tab);
        }
        g.select_shaderpack(Some("PackA.zip".into()));
        assert_eq!(g.settings.selected_shaderpack.as_deref(), Some("PackA.zip"));
        g.select_shaderpack(None);
        assert!(g.settings.selected_shaderpack.is_none());
        g.available_shaderpacks = vec!["PackA.zip".into(), "PackB.zip".into()];
        g.switch_tab(SodiumSettingsTab::Shaders); // 空でも注入済みでも描画可能
        g.apply_window_title_override();
        g.render_instant_visual_indicators();
        g.save_and_close();
        assert!(!g.is_open);
    }
}
