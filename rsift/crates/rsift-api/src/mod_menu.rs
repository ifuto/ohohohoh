//! # Mod Menu Screen & Icon Catalog (`mod_menu`)
//!
//! Opens a real Minecraft host screen populated with mod entries via the platform bridge.

use crate::mod_api::ModManifest;
use crate::cloth_config::ClothConfigBuilder;
use crate::platform::{mark_dirty, request_open_screen};
use crate::os_integ::OsIntegration;
use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use tracing::{info, debug};

/// Mod リストエントリー
#[derive(Clone)]
pub struct ModMenuEntry {
    pub manifest: ModManifest,
    pub icon_gpu_texture_id: Option<u32>,
    pub homepage_url: Option<String>,
    pub cloth_config_screen: Option<ClothConfigBuilder>,
}

/// Mod Menu 管理・描画スクリーン
#[derive(Default, Clone)]
pub struct RsiftModMenuScreen {
    pub entries: Arc<RwLock<HashMap<String, ModMenuEntry>>>,
    pub selected_mod_id: Arc<RwLock<Option<String>>>,
    pub is_open: Arc<RwLock<bool>>,
}

impl RsiftModMenuScreen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_mod(
        &self,
        manifest: ModManifest,
        icon_id: Option<u32>,
        url: Option<&str>,
        config: Option<ClothConfigBuilder>,
    ) {
        debug!(
            "[ModMenu] Registering mod to catalog: {} (ID: {})",
            manifest.name, manifest.id
        );
        if let Ok(mut map) = self.entries.write() {
            map.insert(
                manifest.id.clone(),
                ModMenuEntry {
                    manifest,
                    icon_gpu_texture_id: icon_id,
                    homepage_url: url.map(|s| s.to_string()),
                    cloth_config_screen: config,
                },
            );
        }
        mark_dirty();
    }

    /// Open the Minecraft-hosted Mod Menu screen (buttons injected by platform bridge).
    pub fn open_screen(&self) {
        if let Ok(mut guard) = self.is_open.write() {
            *guard = true;
        }
        info!("[ModMenu] Opening Minecraft Mod Menu screen");
        request_open_screen("mod_menu");
    }

    pub fn close_screen(&self) {
        if let Ok(mut guard) = self.is_open.write() {
            *guard = false;
        }
    }

    pub fn select_mod(&self, id: &str) {
        *self.selected_mod_id.write().unwrap() = Some(id.to_string());
    }

    pub fn open_selected_config(&self) {
        let id = self.selected_mod_id.read().unwrap().clone();
        let Some(id) = id else { return };
        if let Ok(map) = self.entries.read() {
            if let Some(entry) = map.get(&id) {
                if let Some(cfg) = &entry.cloth_config_screen {
                    cfg.open_screen();
                }
            }
        }
    }

    pub fn open_selected_homepage(&self) {
        let id = self.selected_mod_id.read().unwrap().clone();
        let Some(id) = id else { return };
        if let Ok(map) = self.entries.read() {
            if let Some(entry) = map.get(&id) {
                if let Some(url) = &entry.homepage_url {
                    let _ = OsIntegration::open_url_in_browser(url);
                }
            }
        }
    }

    pub fn render_catalog_frame(&self) {
        // Kept for logging diagnostics; real UI is the Minecraft screen.
        if let Ok(map) = self.entries.read() {
            info!("[ModMenu] catalog entries={}", map.len());
        }
    }

    /// Fabric Mod Menu 参考の一覧行プロトコルを構築する。
    /// 各行は `mod|<id>|<name> (<version>)` で、Java ホスト画面が行を
    /// ボタン化し、クリックは `parse_row` で Select{id} と解釈されて
    /// 詳細画面 (mod_menu_detail) へ遷移する。名前順 (id タイブレーク) で
    /// 安定ソート (旧実装は HashMap 走査で行順序が実行毎に揺れた)。
    pub fn catalog_lines(&self) -> Vec<String> {
        let Ok(map) = self.entries.read() else {
            return Vec::new();
        };
        let mut es: Vec<(&String, &ModMenuEntry)> = map.iter().collect();
        es.sort_by(|a, b| {
            a.1.manifest
                .name
                .to_lowercase()
                .cmp(&b.1.manifest.name.to_lowercase())
                .then_with(|| a.0.cmp(b.0))
        });
        es.iter()
            .map(|(id, e)| {
                format!(
                    "mod|{}|{} ({})",
                    id,
                    e.manifest.name.replace('|', "/"),
                    e.manifest.version.replace('|', "/")
                )
            })
            .collect()
    }

    /// 選択中 mod の詳細行を構築する (mod_menu_detail 画面の内容)。
    /// `info|<text>` は押下無反応の表示行、`act:*|<text>` はアクション行。
    /// Fabric Mod Menu の「選択 mod の詳細 + Config/Homepage ボタン」の簡約版。
    /// 未選択 (または抹消済み id 選択) 時は空。
    pub fn detail_lines(&self) -> Vec<String> {
        let Ok(sel) = self.selected_mod_id.read() else {
            return Vec::new();
        };
        let Some(sel) = sel.clone() else {
            return Vec::new();
        };
        let Ok(map) = self.entries.read() else {
            return Vec::new();
        };
        let Some(entry) = map.get(&sel) else {
            return Vec::new();
        };
        let m = &entry.manifest;
        let mut rows = vec![
            format!("info|{}", m.name.replace('|', "/")),
            format!("info|ID: {}", m.id.replace('|', "/")),
            format!("info|Version: {}", m.version.replace('|', "/")),
            format!("info|Author: {}", m.author.replace('|', "/")),
        ];
        if !m.description.is_empty() {
            rows.push("info|".into());
            rows.push(format!("info|{}", m.description.replace('|', "/")));
        }
        if entry.cloth_config_screen.is_some() {
            rows.push("act:config|Config".into());
        }
        if entry.homepage_url.is_some() {
            rows.push("act:home|Open Homepage".into());
        }
        rows.push("act:back|< Back to mod list".into());
        rows
    }
}

/// ホスト画面 1 行のプロトコル種別 (`mod_menu` / `mod_menu_detail`)。
/// Java は行をボタン化するだけで、意味解釈は Rust 側が一手に担う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModRowAction {
    /// 一覧行押下: id を選択して詳細画面へ遷移する。
    Select { id: String },
    /// 表示専用行 (押下 no-op)。
    Info,
    /// 選択中 mod の設定画面を開く。
    OpenConfig,
    /// 選択中 mod のホームページを開く。
    OpenHomepage,
    /// 一覧へ戻る。
    BackToList,
    /// 未知形式 (後方互換の逃がし; 押下 no-op)。
    Unknown,
}

/// `catalog_lines` / `detail_lines` が生成した行を意味解釈する。
pub fn parse_row(line: &str) -> ModRowAction {
    if let Some(rest) = line.strip_prefix("mod|") {
        let id = rest.split('|').next().unwrap_or("").to_string();
        return ModRowAction::Select { id };
    }
    if line.starts_with("info|") {
        return ModRowAction::Info;
    }
    if line.starts_with("act:config|") {
        return ModRowAction::OpenConfig;
    }
    if line.starts_with("act:home|") {
        return ModRowAction::OpenHomepage;
    }
    if line.starts_with("act:back|") {
        return ModRowAction::BackToList;
    }
    ModRowAction::Unknown
}

/// 行プロトコルからボタン表示文言を取り出す。
pub fn row_label(line: &str) -> String {
    if let Some(rest) = line.strip_prefix("mod|") {
        return rest.splitn(2, '|').nth(1).unwrap_or("").to_string();
    }
    if let Some(rest) = line.strip_prefix("info|") {
        return rest.to_string();
    }
    for p in ["act:config|", "act:home|", "act:back|"] {
        if let Some(rest) = line.strip_prefix(p) {
            return rest.to_string();
        }
    }
    line.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_api::ModManifest;

    fn manifest(id: &str) -> ModManifest {
        ModManifest {
            id: id.into(),
            name: format!("Mod {id}"),
            version: "1.0.0".into(),
            author: "tester".into(),
            description: "test".into(),
            target_rsift_version: "1.21.11".into(),
        }
    }

    #[test]
    fn register_mod_counts_distinct_ids() {
        let m = RsiftModMenuScreen::new();
        assert_eq!(m.entries.read().unwrap().len(), 0);
        m.register_mod(manifest("a"), None, None, None);
        m.register_mod(manifest("b"), Some(50000), Some("https://example.com"), None);
        let map = m.entries.read().unwrap();
        assert_eq!(map.len(), 2);
        let b = map.get("b").unwrap();
        assert_eq!(b.icon_gpu_texture_id, Some(50000));
        assert_eq!(b.homepage_url.as_deref(), Some("https://example.com"));
        assert_eq!(b.manifest.name, "Mod b");
    }

    #[test]
    fn register_mod_same_id_overwrites_not_duplicates() {
        // タイトルの Mods (N) 表示が参照するカタログ数は排他的 id 数で
        // なければならない (同一 id の再登録 = バージョン更新)。
        let m = RsiftModMenuScreen::new();
        let mut v1 = manifest("a");
        v1.version = "1.0.0".into();
        m.register_mod(v1, None, None, None);
        let mut v2 = manifest("a");
        v2.version = "2.0.0".into();
        m.register_mod(v2, None, None, None);
        let map = m.entries.read().unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("a").unwrap().manifest.version, "2.0.0");
    }

    #[test]
    fn open_close_selection_state_machine_is_noop_safe() {
        let m = RsiftModMenuScreen::new();
        assert!(!*m.is_open.read().unwrap());
        m.open_screen();
        assert!(*m.is_open.read().unwrap());
        m.close_screen();
        assert!(!*m.is_open.read().unwrap());
        // 未選択・未登録 id 選択の両方で config/homepage は no-op (panic しない)。
        m.open_selected_config();
        m.open_selected_homepage();
        m.select_mod("missing");
        m.open_selected_config();
        m.open_selected_homepage();
        assert_eq!(m.selected_mod_id.read().unwrap().as_deref(), Some("missing"));
    }
}

#[cfg(test)]
mod catalog_protocol_tests {
    use super::*;
    use crate::mod_api::ModManifest;
    use crate::cloth_config::ClothConfigBuilder;

    fn manifest_full(id: &str, name: &str, desc: &str) -> ModManifest {
        ModManifest {
            id: id.into(),
            name: name.into(),
            version: "1.2.3".into(),
            author: "tester".into(),
            description: desc.into(),
            target_rsift_version: "1.21.11".into(),
        }
    }

    #[test]
    fn catalog_lines_sorted_stable_with_display_format() {
        let m = RsiftModMenuScreen::new();
        // HashMap 投入順に依らず、name (case-insensitive) 昇順 + id タイブレーク。
        m.register_mod(manifest_full("zeta", "Zeta Engine", ""), None, None, None);
        m.register_mod(manifest_full("alpha", "alpha tools", ""), None, None, None);
        m.register_mod(manifest_full("beta", "Beta Pack", ""), None, None, None);
        assert_eq!(
            m.catalog_lines(),
            vec![
                "mod|alpha|alpha tools (1.2.3)".to_string(),
                "mod|beta|Beta Pack (1.2.3)".to_string(),
                "mod|zeta|Zeta Engine (1.2.3)".to_string(),
            ]
        );
    }

    #[test]
    fn catalog_lines_sanitizes_pipes_in_display_fields() {
        let m = RsiftModMenuScreen::new();
        let mut man = manifest_full("weird", "A|B", "");
        man.version = "9|9".into();
        m.register_mod(man, None, None, None);
        let lines = m.catalog_lines();
        assert_eq!(lines, vec!["mod|weird|A/B (9/9)".to_string()]);
        // プロトコル破壊不能化: 表示側に '|' が残らない。
        assert_eq!(lines[0].matches('|').count(), 2);
    }

    #[test]
    fn detail_lines_full_entry_has_all_sections_and_actions() {
        let m = RsiftModMenuScreen::new();
        let cfg = ClothConfigBuilder::default();
        m.register_mod(
            manifest_full("gfx", "RsGraphics Engine", "Sodium-surpassing renderer."),
            Some(50000),
            Some("https://example.com"),
            Some(cfg),
        );
        m.select_mod("gfx");
        let expected: Vec<String> = [
            "info|RsGraphics Engine",
            "info|ID: gfx",
            "info|Version: 1.2.3",
            "info|Author: tester",
            "info|",
            "info|Sodium-surpassing renderer.",
            "act:config|Config",
            "act:home|Open Homepage",
            "act:back|< Back to mod list",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(m.detail_lines(), expected);
    }

    #[test]
    fn detail_lines_omit_optional_actions_and_blank_desc() {
        let m = RsiftModMenuScreen::new();
        let mut man = manifest_full("bare", "Bare Mod", "");
        man.description = "".into();
        m.register_mod(man, None, None, None);
        m.select_mod("bare");
        let rows = m.detail_lines();
        // config/url 無し → act:config/act:home 無し、説明空 → 空行無し。
        assert!(!rows.iter().any(|r| r.starts_with("act:config|")));
        assert!(!rows.iter().any(|r| r.starts_with("act:home|")));
        assert!(!rows.iter().any(|r| r == "info|"));
        assert_eq!(rows.last().map(String::as_str), Some("act:back|< Back to mod list"));
        // 未選択・抹消済み id 選択は空。
        let empty = RsiftModMenuScreen::new();
        assert!(empty.detail_lines().is_empty());
        empty.select_mod("ghost");
        assert!(empty.detail_lines().is_empty());
    }

    #[test]
    fn parse_row_decodes_protocol() {
        assert_eq!(
            parse_row("mod|rsgraphics|RsGraphics Engine (0.1.0)"),
            ModRowAction::Select { id: "rsgraphics".into() }
        );
        assert_eq!(parse_row("info|hello"), ModRowAction::Info);
        assert_eq!(parse_row("act:config|Config"), ModRowAction::OpenConfig);
        assert_eq!(parse_row("act:home|Open Homepage"), ModRowAction::OpenHomepage);
        assert_eq!(parse_row("act:back|< Back"), ModRowAction::BackToList);
        // 旧 raw 形式など未知行は no-op 扱い。
        assert_eq!(parse_row("id|name|1.0|me|desc|http://x"), ModRowAction::Unknown);
    }

    #[test]
    fn row_label_extracts_display_text() {
        assert_eq!(row_label("mod|gfx|RsGraphics (1.0)"), "RsGraphics (1.0)");
        assert_eq!(row_label("info|Author: tester"), "Author: tester");
        assert_eq!(row_label("info|"), "");
        assert_eq!(row_label("act:back|< Back to mod list"), "< Back to mod list");
        assert_eq!(row_label("raw legacy line"), "raw legacy line");
    }
}
