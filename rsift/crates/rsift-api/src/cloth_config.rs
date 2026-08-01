//! # Cloth Config Native Engine (`cloth_config`)
//!
//! Builds config UI descriptors and opens a real Minecraft screen via
//! `RsiftPlatformBridge` (buttons + labels injected on a host PauseScreen).

use crate::platform::{mark_dirty, request_open_cloth};
use std::sync::{Arc, OnceLock, RwLock};
use tracing::{info, debug};

#[derive(Debug, Clone)]
pub enum ConfigEntryType {
    IntSlider { min: i32, max: i32, current: Arc<RwLock<i32>> },
    BooleanToggle { current: Arc<RwLock<bool>> },
    StringField { current: Arc<RwLock<String>> },
    ColorPicker { current_rgba: Arc<RwLock<u32>> },
}

#[derive(Debug, Clone)]
pub struct ConfigEntry {
    pub label: String,
    pub tooltip: Option<String>,
    pub entry_type: ConfigEntryType,
    /// 表示用の値係数 (例: 内部 e10 整数 40 → 表示 4.0) と単位 ("x", "ms")。
    /// 既定 1.0 / なし (raw 整数表示)。wave 201 RsZoom のバニラ風ラベル用。
    pub display_scale: f32,
    pub display_unit: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct ConfigCategory {
    pub name: String,
    pub icon_symbol: Option<String>,
    pub entries: Vec<ConfigEntry>,
}

/// 値がホスト画面操作で変わった後に呼ばれる永続化フック (mod が cfg 保存を登録)。
pub type ClothChangeFn = std::sync::Arc<dyn Fn(&ClothConfigBuilder) + Send + Sync>;

#[derive(Default, Clone)]
pub struct ClothConfigBuilder {
    pub title: String,
    pub categories: Vec<ConfigCategory>,
    /// 値変化後の実消費者 (RsZoom は config/rszoom.cfg 保存)。未設定でも
    /// 画面内の値更新自体は行われる (横互換)。
    pub on_change: Option<ClothChangeFn>,
}

impl ClothConfigBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_title(&mut self, title: &str) -> &mut Self {
        self.title = title.to_string();
        self
    }

    pub fn set_on_change(&mut self, f: ClothChangeFn) -> &mut Self {
        self.on_change = Some(f);
        self
    }

    pub fn add_category(&mut self, name: &str) -> usize {
        let idx = self.categories.len();
        self.categories.push(ConfigCategory {
            name: name.to_string(),
            icon_symbol: None,
            entries: Vec::new(),
        });
        mark_dirty();
        idx
    }

    pub fn add_int_slider(
        &mut self,
        cat_idx: usize,
        label: &str,
        min: i32,
        max: i32,
        initial: i32,
        tooltip: Option<&str>,
    ) -> Arc<RwLock<i32>> {
        self.add_scaled_slider(cat_idx, label, min, max, initial, 1.0, None, tooltip)
    }

    /// IntSlider + 表示係数/単位 (wave 201)。内部は常に i32、表示のみ
    /// `cur * display_scale` + unit (例: cur=40 scale=0.1 unit="x" → "4.0x")。
    pub fn add_scaled_slider(
        &mut self,
        cat_idx: usize,
        label: &str,
        min: i32,
        max: i32,
        initial: i32,
        display_scale: f32,
        display_unit: Option<&str>,
        tooltip: Option<&str>,
    ) -> Arc<RwLock<i32>> {
        let current = Arc::new(RwLock::new(initial));
        if let Some(cat) = self.categories.get_mut(cat_idx) {
            debug!("[ClothConfig] IntSlider [{}] [{}..{}]", label, min, max);
            cat.entries.push(ConfigEntry {
                label: label.to_string(),
                tooltip: tooltip.map(|s| s.to_string()),
                entry_type: ConfigEntryType::IntSlider {
                    min,
                    max,
                    current: current.clone(),
                },
                display_scale,
                display_unit: display_unit.map(|u| u.to_string()),
            });
        }
        mark_dirty();
        current
    }

    pub fn add_bool_toggle(
        &mut self,
        cat_idx: usize,
        label: &str,
        initial: bool,
        tooltip: Option<&str>,
    ) -> Arc<RwLock<bool>> {
        let current = Arc::new(RwLock::new(initial));
        if let Some(cat) = self.categories.get_mut(cat_idx) {
            debug!("[ClothConfig] BooleanToggle [{}] default={}", label, initial);
            cat.entries.push(ConfigEntry {
                label: label.to_string(),
                tooltip: tooltip.map(|s| s.to_string()),
                entry_type: ConfigEntryType::BooleanToggle {
                    current: current.clone(),
                },
                display_scale: 1.0,
                display_unit: None,
            });
        }
        mark_dirty();
        current
    }

    pub fn add_string_field(
        &mut self,
        cat_idx: usize,
        label: &str,
        initial: &str,
        tooltip: Option<&str>,
    ) -> Arc<RwLock<String>> {
        let current = Arc::new(RwLock::new(initial.to_string()));
        if let Some(cat) = self.categories.get_mut(cat_idx) {
            cat.entries.push(ConfigEntry {
                label: label.to_string(),
                tooltip: tooltip.map(|s| s.to_string()),
                entry_type: ConfigEntryType::StringField {
                    current: current.clone(),
                },
                display_scale: 1.0,
                display_unit: None,
            });
        }
        mark_dirty();
        current
    }

    pub fn add_color_picker(
        &mut self,
        cat_idx: usize,
        label: &str,
        initial_rgba: u32,
        tooltip: Option<&str>,
    ) -> Arc<RwLock<u32>> {
        let current = Arc::new(RwLock::new(initial_rgba));
        if let Some(cat) = self.categories.get_mut(cat_idx) {
            cat.entries.push(ConfigEntry {
                label: label.to_string(),
                tooltip: tooltip.map(|s| s.to_string()),
                entry_type: ConfigEntryType::ColorPicker {
                    current_rgba: current.clone(),
                },
                display_scale: 1.0,
                display_unit: None,
            });
        }
        mark_dirty();
        current
    }

    /// エントリ一覧をホスト画面行プロトコルへ直列化 (開画面/再描画で共用)。
    pub fn render_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for cat in &self.categories {
            lines.push(format!("#{}", cat.name));
            for entry in &cat.entries {
                let value = match &entry.entry_type {
                    ConfigEntryType::IntSlider { min, max, current } => {
                        format!(
                            "int:{}:{}:{}",
                            min,
                            max,
                            current.read().map(|v| *v).unwrap_or(0)
                        )
                    }
                    ConfigEntryType::BooleanToggle { current } => {
                        format!("bool:{}", current.read().map(|v| *v).unwrap_or(false))
                    }
                    ConfigEntryType::StringField { current } => {
                        format!(
                            "str:{}",
                            current.read().map(|v| v.clone()).unwrap_or_default()
                        )
                    }
                    ConfigEntryType::ColorPicker { current_rgba } => {
                        format!(
                            "color:{:08X}",
                            current_rgba.read().map(|v| *v).unwrap_or(0xFFFFFFFF)
                        )
                    }
                };
                lines.push(format!("{}|{}", entry.label, value));
            }
        }
        lines
    }

    /// Serialize entries and open the real Minecraft config host screen.
    pub fn open_screen(&self) {
        let lines = self.render_lines();
        set_open_config(self);
        info!(
            "[ClothConfig] Opening Minecraft screen \"{}\" ({} lines)",
            self.title,
            lines.len()
        );
        request_open_cloth(&self.title, lines);
    }
}

// ============================================================
// ホスト画面行押下 → 値更新の書き戻し路 (wave 201)。
// これまでは cloth_config 行の押下が no-op (表示専用) だった。RsZoom の
// 「Mods画面 → RsZoom → Config で倍率をいじれる」の実現には、押下で値が
// 実際に変わり、永続化 hook が走り、画面が新しい値で開き直すまでを一体で
// 配線する必要がある (旧実装は表示のみで書き戻し不在 = 構造的欠陥)。
// ============================================================

fn open_config_slot() -> &'static RwLock<Option<ClothConfigBuilder>> {
    static SLOT: OnceLock<RwLock<Option<ClothConfigBuilder>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// いま開いている (直近に open 要求した) cloth 設定画面の builder を保持。
/// 値セル (Arc<RwLock>) は builder 複製でも共有されるため、保持側の更新が
/// mod 側の参照へそのまま届く。
pub fn set_open_config(builder: &ClothConfigBuilder) {
    *open_config_slot().write().unwrap() = Some(builder.clone());
    mark_dirty();
}

/// 直近に開いた設定画面の builder を取得 (行押下の意味解釈元)。
pub fn open_config() -> Option<ClothConfigBuilder> {
    open_config_slot().read().unwrap().clone()
}

/// IntSlider の 1 押下あたりの増分。全幅を約 20 クリックで一周する粒度で、
/// 小数切上げ 1 以上 (狭幅でも必ず進む)。
pub fn slider_step(min: i32, max: i32) -> i32 {
    if max <= min {
        return 1;
    }
    ((max - min) as f64 / 20.0).ceil().max(1.0) as i32
}

/// バニラ設定ボタン風の表示ラベル ("<label>: <値><単位>")。
/// raw 行 (`label|int:min:max:cur` 等) → 表示文字列。ツールチップは別路で
/// raw 行全体が渡るため min/max 情報は失われない。
pub fn cloth_display_label(line: &str) -> String {
    if let Some(cat) = line.strip_prefix('#') {
        return crate::mc_style::category_header(cat);
    }
    let Some((label, spec)) = line.split_once('|') else {
        return line.to_string();
    };
    if let Some(rest) = spec.strip_prefix("int:") {
        let cur = parse_int_spec_cur(rest);
        let scale = display_scale_for(label);
        let unit = display_unit_for(label);
        return format!("{label}: {}{unit}", format_scaled(cur as f64 * scale));
    }
    if let Some(rest) = spec.strip_prefix("bool:") {
        // バニラ準拠: "ON"/"OFF" (Video Settings 等と同じ語彙)。
        return format!("{label}: {}", if rest == "true" { "ON" } else { "OFF" });
    }
    label.to_string()
}

/// `min:max:cur` の cur のみ抽出 (表示ラベルは現在値だけを使う)。
fn parse_int_spec_cur(rest: &str) -> i32 {
    rest.splitn(3, ':')
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn display_scale_for(label: &str) -> f64 {
    open_config()
        .and_then(|b| b.find_entry(label))
        .map(|e| e.display_scale as f64)
        .unwrap_or(1.0)
}

fn display_unit_for(label: &str) -> String {
    open_config()
        .and_then(|b| b.find_entry(label))
        .and_then(|e| e.display_unit.clone())
        .unwrap_or_default()
}

/// "4.00"→"4", "4.50"→"4.5", 整数相当は整数表示 (バニラ値表示と同系)。
fn format_scaled(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        return format!("{}", v.round() as i64);
    }
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// ホスト画面で設定行が押された。値をサイクル/反転して on_change → 再描画
/// 要求までを一貫して行う。カテゴリ行 ('#…')・未知ラベルは no-op。
pub fn press_row(line: &str) {
    if line.starts_with('#') {
        return;
    }
    let Some((label, _spec)) = line.split_once('|') else {
        return;
    };
    let Some(mut builder) = open_config() else {
        debug!("[ClothConfig] press_row without open config: {}", line);
        return;
    };
    let mut changed = false;
    for cat in &mut builder.categories {
        if let Some(entry) = cat.entries.iter_mut().find(|e| e.label == label) {
            match &entry.entry_type {
                ConfigEntryType::IntSlider { min, max, current } => {
                    let step = slider_step(*min, *max);
                    if let Ok(mut v) = current.write() {
                        let next = *v + step;
                        *v = if next > *max { *min } else { next };
                        changed = true;
                    }
                }
                ConfigEntryType::BooleanToggle { current } => {
                    if let Ok(mut v) = current.write() {
                        *v = !*v;
                        changed = true;
                    }
                }
                // 文字列/色はホスト画面の行押下では編集経路を持たない (押下
                // no-op を明示。嘘の「編集できる」UI を構築しない)。
                ConfigEntryType::StringField { .. } | ConfigEntryType::ColorPicker { .. } => {}
            }
        }
    }
    if !changed {
        return;
    }
    info!("[ClothConfig] row pressed -> value changed: {}", label);
    if let Some(cb) = &builder.on_change {
        cb(&builder);
    }
    // 新しい値で開き直す (表示と値の乖離を残さない)。
    let lines = builder.render_lines();
    set_open_config(&builder);
    request_open_cloth(&builder.title, lines);
}

impl ClothConfigBuilder {
    /// ラベル一致のエントリ検索 (表示層・ツールチップ生成の共用)。
    pub fn find_entry(&self, label: &str) -> Option<ConfigEntry> {
        self.categories
            .iter()
            .flat_map(|c| c.entries.iter())
            .find(|e| e.label == label)
            .cloned()
    }
}

#[cfg(test)]
mod writeback_tests {
    //! wave 201: ホスト画面書き戻し路の検定。グローバル open スロットを共有
    //! するため SERIAL 直列化。
    use super::*;

    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn builder() -> (ClothConfigBuilder, Arc<RwLock<i32>>, Arc<RwLock<bool>>) {
        let mut b = ClothConfigBuilder::new();
        b.set_title("RsZoom Config");
        let cat = b.add_category("Zoom");
        let f = b.add_scaled_slider(cat, "Zoom 倍率", 15, 100, 40, 0.1, Some("x"), None);
        b.add_bool_toggle(cat, "滑らか移動", true, None);
        b.add_string_field(cat, "備考", "", None);
        let smooth_cell = match b
            .find_entry("滑らか移動")
            .expect("bool entry exists")
            .entry_type
        {
            ConfigEntryType::BooleanToggle { current } => current,
            _ => panic!("型が違う"),
        };
        (b, f, smooth_cell)
    }

    #[test]
    fn slider_step_is_about_twenty_clicks_per_span() {
        assert_eq!(slider_step(15, 100), 5);
        assert_eq!(slider_step(0, 1), 1); // 狭幅でも必ず前進
        assert_eq!(slider_step(50, 950), 45);
        assert_eq!(slider_step(10, 10), 1); // 縮退幅は安全側
    }

    #[test]
    fn press_row_cycles_int_and_wraps_to_min() {
        let _g = SERIAL.lock().unwrap();
        let (b, factor, _) = builder();
        b.open_screen();
        assert_eq!(*factor.read().unwrap(), 40);
        // step=5: 40→45→…→100→15 (wrap)
        for _ in 0..12 {
            press_row("Zoom 倍率|int:15:100:40");
        }
        assert_eq!(*factor.read().unwrap(), 100);
        press_row("Zoom 倍率|int:15:100:100");
        assert_eq!(*factor.read().unwrap(), 15, "上限超過は最小へ一周");
    }

    #[test]
    fn press_row_flips_bool_and_string_is_noop() {
        let _g = SERIAL.lock().unwrap();
        let (b, _, smooth) = builder();
        b.open_screen();
        assert!(*smooth.read().unwrap());
        press_row("滑らか移動|bool:true");
        assert!(!*smooth.read().unwrap());
        press_row("滑らか移動|bool:false");
        assert!(*smooth.read().unwrap());
        // 文字列フィールドは行押下 no-op (実装経路が無いことを明示)
        let v0 = b.find_entry("備考").unwrap();
        press_row("備考|str:");
        let v1 = b.find_entry("備考").unwrap();
        assert_eq!(
            match (&v0.entry_type, &v1.entry_type) {
                (
                    ConfigEntryType::StringField { current: a },
                    ConfigEntryType::StringField { current: c },
                ) => *a.read().unwrap() == *c.read().unwrap(),
                _ => false,
            },
            true
        );
    }

    #[test]
    fn press_row_noop_lines_do_not_touch_values() {
        let _g = SERIAL.lock().unwrap();
        let (b, factor, _) = builder();
        b.open_screen();
        press_row("#Zoom"); // カテゴリヘッダ
        press_row("存在しない|int:0:1:0"); // 未知ラベル
        press_row("bare line"); // 区切り無し
        assert_eq!(*factor.read().unwrap(), 40);
    }

    #[test]
    fn change_fires_on_change_and_requests_reopen_with_new_value() {
        let _g = SERIAL.lock().unwrap();
        let (mut b, factor, _) = builder();
        let hits = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let hits2 = hits.clone();
        let factor2 = factor.clone();
        b.set_on_change(Arc::new(move |_: &ClothConfigBuilder| {
            hits2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            assert_eq!(*factor2.read().unwrap(), 45, "on_change 時点で新値が見える");
        }));
        // リクエストスロットを一度空にしてから open
        let _ = crate::platform::test_take_cloth_request();
        b.open_screen();
        let _ = crate::platform::test_take_cloth_request();
        press_row("Zoom 倍率|int:15:100:40");
        assert_eq!(hits.load(std::sync::atomic::Ordering::Relaxed), 1);
        let req = crate::platform::test_take_cloth_request().expect("変化後に再描画要求が出る");
        assert_eq!(req.0, "RsZoom Config");
        assert!(
            req.1.iter().any(|l| l == "Zoom 倍率|int:15:100:45"),
            "直列化で新値 45 を含む: {:?}",
            req.1
        );
    }

    #[test]
    fn display_labels_follow_vanilla_option_style() {
        let _g = SERIAL.lock().unwrap();
        let (b, _, _) = builder();
        b.open_screen();
        assert_eq!(cloth_display_label("#Zoom"), "§e— Zoom —");
        assert_eq!(
            cloth_display_label("Zoom 倍率|int:15:100:40"),
            "Zoom 倍率: 4x"
        );
        b.find_entry("Zoom 倍率").unwrap();
        // scale 0.1 → cur 45 = 4.5x
        press_row("Zoom 倍率|int:15:100:40");
        assert_eq!(
            cloth_display_label("Zoom 倍率|int:15:100:45"),
            "Zoom 倍率: 4.5x"
        );
        assert_eq!(
            cloth_display_label("滑らか移動|bool:true"),
            "滑らか移動: ON"
        );
        assert_eq!(
            cloth_display_label("滑らか移動|bool:false"),
            "滑らか移動: OFF"
        );
    }
}
