//! Global Rsift runtime — shared across launcher, JVMTI agent, and DLL mods.

use crate::advancements::{AdvancementRegistry, PlayerAdvancementState};
use crate::keybinds::KeybindRegistry;
use crate::mod_api::{RsiftModOnFovScaleFn, RsiftModOnPacketFn, RsiftModOnRenderFn};
use crate::mod_menu::RsiftModMenuScreen;
use crate::registry::ModRegistry;
use crate::ui_ext::ScreenRegistry;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use tracing::info;

pub struct RsiftRuntime {
    pub screen_registry: ScreenRegistry,
    pub mod_menu: RsiftModMenuScreen,
    pub registry: Mutex<ModRegistry>,
    pub mod_dir: RwLock<PathBuf>,
    pub packet_handlers: RwLock<Vec<RsiftModOnPacketFn>>,
    pub render_handlers: RwLock<Vec<RsiftModOnRenderFn>>,
    /// 任意 export `rsift_mod_get_fov_scale` を持つ mod の一覧 (wave 201)。
    /// 毎フレーム `query_fov_scale` が総積を評価する (export が無い mod は
    /// そもそも登録されない = 恒等 1.0 扱いで呼出コストも掛からない)。
    pub fov_scale_handlers: RwLock<Vec<RsiftModOnFovScaleFn>>,
    pub mods_loaded: RwLock<bool>,
    /// Mod 登録アドバンスメントの単一真実 (register 後は全クレートから可視)。
    pub advancements: Mutex<AdvancementRegistry>,
    /// バニラ KeyMapping 橋渡しの登録表 (wave 203: Mod の宣言をエージェントが
    /// 拾って `Options.keyMappings` へ追記し、isDown() 状態を書き戻す)。
    pub keybinds: KeybindRegistry,
    /// ローカルプレイヤーの進捗状態機械 (grant_progress が実grant判定を実施)。
    pub advancement_state: Mutex<PlayerAdvancementState>,
    render_tick_wanted: AtomicU32,
    idle_mode: AtomicBool,
}

impl RsiftRuntime {
    pub fn new(mod_dir: PathBuf) -> Self {
        Self {
            screen_registry: ScreenRegistry::new(),
            mod_menu: RsiftModMenuScreen::new(),
            registry: Mutex::new(ModRegistry::new()),
            mod_dir: RwLock::new(mod_dir),
            packet_handlers: RwLock::new(Vec::new()),
            render_handlers: RwLock::new(Vec::new()),
            fov_scale_handlers: RwLock::new(Vec::new()),
            mods_loaded: RwLock::new(false),
            advancements: Mutex::new(AdvancementRegistry::new()),
            keybinds: KeybindRegistry::new(),
            advancement_state: Mutex::new(PlayerAdvancementState::new()),
            render_tick_wanted: AtomicU32::new(0),
            idle_mode: AtomicBool::new(false),
        }
    }

    /// アドバンスメント進捗を加算し、(基準新規完了, アドバンスメント新規付与) を返す。
    /// Mod は戻り値 true に反応してトースト等を実表示できる。
    pub fn grant_advancement_progress(
        &self,
        adv_id: &crate::registry::RegistryKey,
        criterion: &str,
        amount: u32,
    ) -> (bool, bool) {
        let reg = self.advancements.lock().unwrap();
        let mut state = self.advancement_state.lock().unwrap();
        let (criterion_done, granted) = state.grant_progress(&reg, adv_id, criterion, amount);
        if granted {
            info!(
                "[Advancement] granted {} (criterion '{}' complete: {})",
                adv_id.as_str(),
                criterion,
                criterion_done
            );
        }
        drop(state);
        drop(reg);
        (criterion_done, granted)
    }

    pub fn set_mod_dir(&self, dir: PathBuf) {
        *self.mod_dir.write().unwrap() = dir;
    }

    pub fn screen_registry(&self) -> &ScreenRegistry {
        &self.screen_registry
    }

    pub fn keybinds(&self) -> &KeybindRegistry {
        &self.keybinds
    }

    /// バニラ KeyMapping 登録宣言 (Mod は戻りセルを保持し描画 tick で読む)。
    pub fn register_keybind(
        &self,
        name: &str,
        category: &str,
        default_code: i32,
    ) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        let arc = self.keybinds.register(name, category, default_code);
        crate::platform::mark_dirty();
        arc
    }

    /// 状態読取り (登録名のみ真を返す)。
    pub fn keybind_is_down(&self, name: &str) -> bool {
        self.keybinds.is_down(name)
    }

    /// エージェント側の isDown() ポーリング結果の書き戻し (変化検出つき)。
    pub fn keybind_set_state(&self, name: &str, down: bool) -> bool {
        self.keybinds.set_state(name, down)
    }

    pub fn mod_menu(&self) -> &RsiftModMenuScreen {
        &self.mod_menu
    }

    pub fn add_packet_handler(&self, f: RsiftModOnPacketFn) {
        self.packet_handlers.write().unwrap().push(f);
    }

    pub fn add_render_handler(&self, f: RsiftModOnRenderFn) {
        self.render_handlers.write().unwrap().push(f);
    }

    pub fn add_fov_scale_handler(&self, f: RsiftModOnFovScaleFn) {
        self.fov_scale_handlers.write().unwrap().push(f);
    }

    pub fn has_fov_scale_handlers(&self) -> bool {
        !self.fov_scale_handlers.read().unwrap().is_empty()
    }

    /// 全 mod の FOV スケール宣言を総積で評価 (`[sanitize_fov_scales]` 適用)。
    /// 呼出はレンダースレッドのフレーム境界のみ (mod export は即時復帰契約)。
    pub fn query_fov_scale(&self) -> f32 {
        let handlers = self.fov_scale_handlers.read().unwrap();
        if handlers.is_empty() {
            return 1.0;
        }
        let raw: Vec<f32> = handlers.iter().map(|f| f()).collect();
        drop(handlers);
        sanitize_fov_scales(&raw)
    }

    pub fn dispatch_packet(&self, packet_id: u32, ptr: i64, len: i32) -> bool {
        for f in self.packet_handlers.read().unwrap().iter() {
            if !f(packet_id, ptr, len) {
                return false;
            }
        }
        true
    }

    pub fn dispatch_render(&self, width: u32, height: u32, delta: f32) {
        let handlers = self.render_handlers.read().unwrap();
        if handlers.is_empty() {
            return;
        }
        for f in handlers.iter() {
            f(width, height, delta);
        }
    }

    pub fn has_render_handlers(&self) -> bool {
        !self.render_handlers.read().unwrap().is_empty()
    }

    pub fn has_packet_handlers(&self) -> bool {
        !self.packet_handlers.read().unwrap().is_empty()
    }

    /// Request periodic render callbacks (e.g. RsReplay overlay while recording).
    pub fn request_render_ticks(&self) {
        self.render_tick_wanted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn release_render_ticks(&self) {
        self.render_tick_wanted
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(1))
            })
            .ok();
    }

    pub fn needs_render_dispatch(&self) -> bool {
        self.render_tick_wanted.load(Ordering::Relaxed) > 0
    }

    pub fn set_idle_mode(&self, idle: bool) {
        self.idle_mode.store(idle, Ordering::Relaxed);
    }

    pub fn is_idle(&self) -> bool {
        self.idle_mode.load(Ordering::Relaxed)
    }

    pub fn mark_mods_loaded(&self) {
        *self.mods_loaded.write().unwrap() = true;
        info!("[RsiftRuntime] Native mods loaded and wired to global dispatchers");
    }

    pub fn mods_loaded(&self) -> bool {
        *self.mods_loaded.read().unwrap()
    }
}

static RUNTIME: OnceLock<RsiftRuntime> = OnceLock::new();

pub fn init_runtime(mod_dir: PathBuf) -> &'static RsiftRuntime {
    RUNTIME.get_or_init(|| RsiftRuntime::new(mod_dir))
}

pub fn runtime() -> Option<&'static RsiftRuntime> {
    RUNTIME.get()
}

pub fn runtime_or_init(mod_dir: PathBuf) -> &'static RsiftRuntime {
    RUNTIME.get_or_init(|| RsiftRuntime::new(mod_dir))
}

/// 1 mod が返せる FOV スケールの絶対域 (悪質 mod の暴走防止柵)。
pub const FOV_SCALE_PER_MOD_MIN: f32 = 0.01;
pub const FOV_SCALE_PER_MOD_MAX: f32 = 100.0;
/// 全 mod 総積の最終域。20 mod が全部 100x を返しても射影が負転倒しない。
pub const FOV_SCALE_TOTAL_MIN: f32 = FOV_SCALE_PER_MOD_MIN;
pub const FOV_SCALE_TOTAL_MAX: f32 = FOV_SCALE_PER_MOD_MAX;

/// mod 宣言スケール列の無害化 + 総積 (wave 201)。
/// - 非有限 (NaN/±inf)・非正値の mod 貢献は恒等 1.0 に置換 (mod が壊れても
///   他 mod と射影計算を道連れにしない)。
/// - 各 mod は [PER_MOD_MIN, PER_MOD_MAX] にクランプ。
/// - 総積は [TOTAL_MIN, TOTAL_MAX] にクランプ (積爆発の構造的抑止)。
pub fn sanitize_fov_scales(scales: &[f32]) -> f32 {
    let mut acc: f64 = 1.0;
    for &s in scales {
        let clean = if s.is_finite() && s > 0.0 {
            s.clamp(FOV_SCALE_PER_MOD_MIN, FOV_SCALE_PER_MOD_MAX)
        } else {
            1.0
        };
        acc *= clean as f64;
    }
    (acc as f32).clamp(FOV_SCALE_TOTAL_MIN, FOV_SCALE_TOTAL_MAX)
}

#[cfg(test)]
mod fov_scale_tests {
    use super::*;

    #[test]
    fn empty_and_identity_are_1() {
        assert_eq!(sanitize_fov_scales(&[]), 1.0);
        assert_eq!(sanitize_fov_scales(&[1.0, 1.0, 1.0]), 1.0);
    }

    #[test]
    fn product_of_valid_scales() {
        assert_eq!(sanitize_fov_scales(&[2.0, 3.0]), 6.0);
        assert!((sanitize_fov_scales(&[0.5, 4.0]) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn broken_mod_neutralized_not_contagious() {
        // NaN/inf/0/負を返す mod が居ても正常 mod の 2x はそのまま効く。
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -3.0] {
            assert_eq!(
                sanitize_fov_scales(&[2.0, bad]),
                2.0,
                "bad={bad} は恒等置換されること"
            );
        }
    }

    #[test]
    fn per_mod_and_total_clamps_hold() {
        assert_eq!(sanitize_fov_scales(&[1e9]), FOV_SCALE_PER_MOD_MAX);
        assert_eq!(sanitize_fov_scales(&[1e-9]), FOV_SCALE_PER_MOD_MIN);
        assert_eq!(sanitize_fov_scales(&[100.0, 100.0]), FOV_SCALE_TOTAL_MAX);
        assert_eq!(sanitize_fov_scales(&[0.01, 0.01]), FOV_SCALE_TOTAL_MIN);
    }
}
