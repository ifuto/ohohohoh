//! # RsZoom — OptiFine 式イーズアウトズーム Mod (`rszoom.dll`)
//!
//! C キー押下中、視線方向中心に FOV を絞ってズームする。押し始め・戻りの
//! 両方とも **cubic イーズアウト** (初速が速く、到達時に滑らかに落ち着く)。
//! 倍率とアニメ時間はゲーム内の「タイトル → Mods → RsZoom → Config」
//! (バニラ外観のホスト設定画面) で変更でき、`config/rszoom.cfg` に保存する。
//!
//! ## 実配線の構成 (偽装なし)
//! - ズーム本体: `rsift_mod_get_fov_scale` (本 mod) → `mod_dispatch::query_fov_scale`
//!   → opt-gfx `camera_zoom::current_effective_camera` → DX12 present 定数と
//!   culling の両方に同一実効カメラが届く (片側ズームは構造的に不可能)。
//! - キー検出: Windows は `GetAsyncKeyState(VK=0x43)` の生ポーリングを
//!   各描画フレームで実行 (押下/解放エッジ両対応)。manifest の capability
//!   `input_capture` を正直に申告する = セキュリティ審査対象。
//!   **macOS/Linux は現状キー検出経路が無い** (GLFW キーフックは後続 wave)。
//!   非 Windows ではズームは発動せず、起動時に 1 行だけ明示ログを出す。
//! - 設定画面: `cloth_config` ホスト画面 = 本物の Minecraft Screen にバニラ
//!   Button を並べるため、背景ブラー・フォント・ボタンは全てマイクラ味。
//!   行押下で値がサイクルし、即座に cfg 保存 + 画面再描画される。
//!
//! ## 既知の正直な制約
//! - チャット等で「c」をタイプしても反応する (生ポーリングの副作用)。
//!   画面状態ガードはホスト側スクリーン判定 API の実装待ち。
//! - キー固定 C (GLFW 67) — バニラのキー設定画面への再割当は未実装。
//! - GL パススルー (描画をバニラ GL に戻したモード) では FOV 消費者が
//!   rsift 側に無いため視覚ズームは掛からない。発動時に 1 回だけ明示ログ。

use rsift_api::{
    ClothChangeFn, ClothConfigBuilder, ModContext, ModManifest, RsiftStatus,
    TARGET_MINECRAFT_VERSION,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tracing::{info, warn};

/// ズームキー (C)。GLFW key code と Win32 VK が一致して 0x43。
pub const ZOOM_KEY_CODE: i32 = 0x43;
/// 倍率の内部表現は e10 整数 (40 = 4.0x)。UI スライダーと cfg を同一表現に。
pub const FACTOR_MIN_E10: i32 = 20; // 2.0x
pub const FACTOR_MAX_E10: i32 = 100; // 10.0x
pub const FACTOR_DEFAULT_E10: i32 = 40; // 4.0x
pub const ANIM_MS_MIN: i32 = 50;
pub const ANIM_MS_MAX: i32 = 500;
pub const ANIM_MS_DEFAULT: i32 = 200;

/// cubic イーズアウト: 0→1。微係数 3(1-x)^2 で初速最大・到達時に減衰。
pub fn ease_out_cubic(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let inv = 1.0 - x;
    1.0 - inv * inv * inv
}

/// ズームアニメ状態機械。ズーム量 `amount ∈ [0,1]` (0=等倍, 1=満倍率) を
/// 「現在値 from → 目標 to を t∈[0,1] で補間 (ease_out)」で進める。
/// 方向転換は現在値を新 from として仕切り直すため、途中で押し離ししても
/// 値が不連続に跳ばない (継続性は検定でピン)。
#[derive(Debug, Clone)]
pub struct ZoomEase {
    /// 現在のズーム量 (0..1)。ここから fov 倍率を作る。
    pub amount: f32,
    from: f32,
    to: f32,
    /// 移行進行 0..1 (anim_secs かけて 1 へ)。
    t: f32,
    /// アニメ時間 (秒)。0 以下は即時遷移扱い。
    pub anim_secs: f32,
    /// 直近の押下状態 (エッジ検出用)。
    pub held: bool,
}

impl ZoomEase {
    pub fn new(anim_secs: f32) -> Self {
        Self {
            amount: 0.0,
            from: 0.0,
            to: 0.0,
            t: 1.0,
            anim_secs,
            held: false,
        }
    }

    /// 押下/解放エッジ。戻り値 true = 状態変化が起きた。
    pub fn set_held(&mut self, held: bool) -> bool {
        if held == self.held {
            return false;
        }
        self.held = held;
        self.from = self.amount;
        self.to = if held { 1.0 } else { 0.0 };
        self.t = 0.0;
        true
    }

    /// dt 秒分アニメを進める。dt は呼出側で実フレーム時間 (ガード済み)。
    pub fn tick(&mut self, dt_secs: f32) {
        if self.t >= 1.0 {
            self.amount = self.to;
            return;
        }
        if self.anim_secs <= 0.0 {
            self.t = 1.0;
            self.amount = self.to;
            return;
        }
        self.t = (self.t + dt_secs / self.anim_secs).min(1.0);
        let e = ease_out_cubic(self.t);
        self.amount = self.from + (self.to - self.from) * e;
    }

    /// 現在の実効 FOV 倍率。amount=1 で factor_max に一致、0 で 1.0。
    pub fn fov_scale(&self, factor_max: f32) -> f32 {
        let factor_max = if factor_max.is_finite() {
            factor_max.max(1.0)
        } else {
            1.0
        };
        1.0 + (factor_max - 1.0) * self.amount.clamp(0.0, 1.0)
    }
}

/// mod 全体の共有状態。
pub struct ZoomState {
    pub ease: Mutex<ZoomEase>,
    /// Arc セルは cloth 設定画面と共有 (画面操作 = 即 mod 反映の単一真実)。
    pub factor_e10: std::sync::Arc<std::sync::RwLock<i32>>,
    pub anim_ms: std::sync::Arc<std::sync::RwLock<i32>>,
    pub smooth: std::sync::Arc<std::sync::RwLock<bool>>,
}

impl ZoomState {
    pub fn factor(&self) -> f32 {
        *self.factor_e10.read().unwrap() as f32 / 10.0
    }

    pub fn current_fov_scale(&self) -> f32 {
        let smooth = *self.smooth.read().unwrap();
        if smooth {
            self.ease.lock().unwrap().fov_scale(self.factor())
        } else {
            // 滑らか移動 OFF = バニラ的な即時切替 (ease 状態は進めない)。
            let ease = self.ease.lock().unwrap();
            if ease.held {
                self.factor()
            } else {
                1.0
            }
        }
    }
}

static STATE: OnceLock<ZoomState> = OnceLock::new();

fn state() -> &'static ZoomState {
    STATE.get_or_init(|| ZoomState {
        ease: Mutex::new(ZoomEase::new(ANIM_MS_DEFAULT as f32 / 1000.0)),
        factor_e10: std::sync::Arc::new(std::sync::RwLock::new(FACTOR_DEFAULT_E10)),
        anim_ms: std::sync::Arc::new(std::sync::RwLock::new(ANIM_MS_DEFAULT)),
        smooth: std::sync::Arc::new(std::sync::RwLock::new(true)),
    })
}

/// 設定ファイルの実パス。テスト/検証用に `RSZOOM_CONFIG_PATH` で差し替え可
/// (setup の RSIFT_MC_DIR と同じ説明可能な上書き規則)。既定はゲーム起動
/// ディレクトリ (`.minecraft`) 直下の `config/rszoom.cfg`。
pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("RSZOOM_CONFIG_PATH") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    PathBuf::from("./config/rszoom.cfg")
}

/// cfg 読込 (欠落・破損は既定値採用、ファイルは上書きしない)。
pub fn load_config(st: &ZoomState, path: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return; // 初回起動: ファイルが無いのは正常経路
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue; // 壊れた行は読み飛ばす (全体中断しない)
        };
        match k.trim() {
            "factor_e10" => {
                if let Ok(n) = v.trim().parse::<i32>() {
                    *st.factor_e10.write().unwrap() = n.clamp(FACTOR_MIN_E10, FACTOR_MAX_E10);
                }
            }
            "anim_ms" => {
                if let Ok(n) = v.trim().parse::<i32>() {
                    let n = n.clamp(ANIM_MS_MIN, ANIM_MS_MAX);
                    *st.anim_ms.write().unwrap() = n;
                    st.ease.lock().unwrap().anim_secs = n as f32 / 1000.0;
                }
            }
            "smooth" => {
                *st.smooth.write().unwrap() = matches!(v.trim(), "true" | "1" | "on");
            }
            _ => {}
        }
    }
    info!("[RsZoom] config loaded from {:?}", path);
}

/// cfg 保存 (on_change hook の実消費者)。失敗は warn のみで落とさない。
pub fn save_config(st: &ZoomState, path: &std::path::Path) {
    let body = format!(
        "factor_e10={}\nanim_ms={}\nsmooth={}\n",
        *st.factor_e10.read().unwrap(),
        *st.anim_ms.read().unwrap(),
        *st.smooth.read().unwrap()
    );
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!("[RsZoom] config dir create failed: {}", e);
            return;
        }
    }
    // tmp → rename で中途半端な状態の cfg を残さない (setup と同規則)。
    let tmp = path.with_extension("cfg.tmp");
    match std::fs::write(&tmp, body) {
        Ok(()) => {
            if let Err(e) = std::fs::rename(&tmp, path) {
                warn!("[RsZoom] config rename failed: {}", e);
            }
        }
        Err(e) => warn!("[RsZoom] config write failed: {}", e),
    }
}

/// 設定画面の構築 (Mods → RsZoom → Config)。バニラ風表示ラベルの係数/単位
/// と、値変化時の cfg 保存 hook をここで一括配線する。
pub fn build_config(st: &'static ZoomState) -> ClothConfigBuilder {
    let mut b = ClothConfigBuilder::new();
    b.set_title("RsZoom Config");
    let cat = b.add_category("Zoom");
    b.add_scaled_slider(
        cat,
        "Zoom 倍率",
        FACTOR_MIN_E10,
        FACTOR_MAX_E10,
        *st.factor_e10.read().unwrap(),
        0.1,
        Some("x"),
        Some("C キー押下時の拡大率 (既定 4.0x)"),
    );
    // 設計: builder が生成する表示セル (押下で直接変化) と mod 状態セルは
    // 2 系に分け、on_change → sync_from_builder で単方向に同期する。
    // 逆方向初期値は下記 initial が担う (ロード済み cfg 値をここで反映)。
    b.add_int_slider(
        cat,
        "アニメ時間",
        ANIM_MS_MIN,
        ANIM_MS_MAX,
        *st.anim_ms.read().unwrap(),
        Some("拡大/縮小のイーズアウト所要時間 (ミリ秒)"),
    );
    b.add_bool_toggle(
        cat,
        "滑らか移動",
        *st.smooth.read().unwrap(),
        Some("OFF のときは押した瞬間に満倍率へ切替 (バニラ FOV 切替相当)"),
    );
    b.set_on_change(save_on_change());
    b
}

/// on_change hook: 画面のセル値を mod 状態へ同期し cfg へ保存する。
fn save_on_change() -> ClothChangeFn {
    std::sync::Arc::new(|b: &ClothConfigBuilder| {
        let st = state();
        sync_from_builder(st, b);
        save_config(st, &config_path());
    })
}

/// builder セル → mod 状態の逆方向同期 (on_change 実装心)。
pub fn sync_from_builder(st: &ZoomState, b: &ClothConfigBuilder) {
    if let Some(e) = b.find_entry("Zoom 倍率") {
        if let rsift_api::ConfigEntryType::IntSlider { current, .. } = e.entry_type {
            *st.factor_e10.write().unwrap() = *current.read().unwrap();
        }
    }
    if let Some(e) = b.find_entry("アニメ時間") {
        if let rsift_api::ConfigEntryType::IntSlider { current, .. } = e.entry_type {
            let v = *current.read().unwrap();
            *st.anim_ms.write().unwrap() = v;
            st.ease.lock().unwrap().anim_secs = v as f32 / 1000.0;
        }
    }
    if let Some(e) = b.find_entry("滑らか移動") {
        if let rsift_api::ConfigEntryType::BooleanToggle { current } = e.entry_type {
            *st.smooth.write().unwrap() = *current.read().unwrap();
        }
    }
}

// ============================================================
// キー状態供給 (押下/解放エッジの一次情報)。
// ============================================================
#[cfg(all(windows, not(test)))]
fn physical_zoom_key_down() -> bool {
    // user32!GetAsyncKeyState (0x43 = 'C')。上位ビットが押下中の一次情報。
    // 代理入力フックを介さないため、ゲームのスクリーン状態は考慮されない
    // (既知制約として文書に記載済)。
    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(v_key: i32) -> i16;
    }
    unsafe { GetAsyncKeyState(ZOOM_KEY_CODE) as u16 & 0x8000 != 0 }
}

#[cfg(all(windows, test))]
fn physical_zoom_key_down() -> bool {
    TEST_KEY_HELD.load(Ordering::Relaxed)
}

#[cfg(not(windows))]
fn physical_zoom_key_down() -> bool {
    // 現状 GLFW キーフック未配線のため macOS/Linux では発動しない。
    // 黙って無効化するのではなく、初回のみ明示ログを出す (後述 once)。
    #[cfg(test)]
    {
        TEST_KEY_HELD.load(Ordering::Relaxed)
    }
    #[cfg(not(test))]
    {
        static NOTICED: AtomicBool = AtomicBool::new(false);
        if !NOTICED.swap(true, Ordering::Relaxed) {
            warn!("[RsZoom] この OS ではまだキー検出経路がありません (現在 Windows 直結のみ)。ズームは発動しません");
        }
        false
    }
}

#[cfg(test)]
static TEST_KEY_HELD: AtomicBool = AtomicBool::new(false);

static PASSTHROUGH_NOTICED: AtomicBool = AtomicBool::new(false);

#[no_mangle]
pub extern "C" fn rsift_mod_init(ctx: &mut ModContext) -> i32 {
    info!("==============================================================================");
    info!(" 🔭 [RsZoom v1.0] OptiFine-style ease-out zoom — hold [C] to zoom");
    info!("    Config: Title → Mods → RsZoom → Config (vanilla-styled screen)");
    info!("==============================================================================");

    let st = state();
    load_config(st, &config_path());
    {
        let mut ease = st.ease.lock().unwrap();
        ease.anim_secs = *st.anim_ms.read().unwrap() as f32 / 1000.0;
    }

    let manifest = ModManifest {
        id: "rszoom".into(),
        name: "RsZoom".into(),
        version: "1.0.0".into(),
        author: "Rsift Project".into(),
        description:
            "C キーで視線先へ滑らかにズーム (イーズアウト)。倍率は Mods メニューから変更可".into(),
        target_rsift_version: TARGET_MINECRAFT_VERSION.into(),
        // 生キー状態ポーリング = input_capture を正直に申告 (審査・同意対象)。
        capabilities: vec!["input_capture".into()],
    };
    ctx.mod_menu()
        .register_mod(manifest, None, None, Some(build_config(st)));
    ctx.request_render_ticks();
    info!(
        "[RsZoom] initialized for {} (factor={}x)",
        TARGET_MINECRAFT_VERSION,
        st.factor()
    );
    RsiftStatus::Success as i32
}

#[no_mangle]
pub extern "C" fn rsift_mod_on_render(_width: u32, _height: u32, delta_time: f32) {
    let st = state();
    let held = physical_zoom_key_down();
    let mut ease = st.ease.lock().unwrap();
    if ease.set_held(held)
        && held
        && matches!(
            rsift_render::backend::realized_backend(),
            Some(rsift_render::backend::RenderBackendKind::GlPassthrough)
        )
        && !PASSTHROUGH_NOTICED.swap(true, Ordering::Relaxed)
    {
        // GL パススルー中は rsift 側射影が無い = ズーム非適用。黙らずに 1 回だけ明示。
        info!("[RsZoom] 注意: 現在は GL 互換 (パススルー) 描画のため FOV ズームは視覚的に適用されません");
    }
    // 滑らか移動 ON のみアニメを進める。dt ガード: 非有限/負/巨大 (長い
    // ハッチ後の吸収飛び防止で 0.1s 上限) を構造的に処理。
    let dt = if delta_time.is_finite() {
        delta_time.clamp(0.0, 0.1)
    } else {
        0.0
    };
    ease.tick(dt);
}

/// 実消費 query (opt-gfx camera_zoom が毎フレーム評価)。
#[no_mangle]
pub extern "C" fn rsift_mod_get_fov_scale() -> f32 {
    state().current_fov_scale()
}

#[cfg(test)]
mod tests {
    //! RsZoom の動作検定 (グローバル STATE を共有するため SERIAL 直列化)。
    use super::*;

    static SERIAL: Mutex<()> = Mutex::new(());

    fn reset_state(anim_ms: i32) {
        let st = state();
        *st.factor_e10.write().unwrap() = FACTOR_DEFAULT_E10;
        *st.anim_ms.write().unwrap() = anim_ms;
        *st.smooth.write().unwrap() = true;
        let mut e = st.ease.lock().unwrap();
        *e = ZoomEase::new(anim_ms as f32 / 1000.0);
    }

    #[test]
    fn ease_out_cubic_endpoints_and_monotonic() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        // 初速が速い: t=0.25 でもう 57.8% 進む (ease-out の特徴式値)。
        let q = ease_out_cubic(0.25);
        assert!((q - (1.0 - 0.75f32.powi(3))).abs() < 1e-6);
        let mut prev = 0.0;
        for i in 1..=10 {
            let v = ease_out_cubic(i as f32 / 10.0);
            assert!(v > prev, "単調増加");
            assert!(v <= 1.0, "オーバーシュートしない");
            prev = v;
        }
        // 定義域外クランプ
        assert_eq!(ease_out_cubic(-0.5), 0.0);
        assert_eq!(ease_out_cubic(1.5), 1.0);
    }

    #[test]
    fn press_zooms_in_with_ease_out_and_release_returns() {
        let _g = SERIAL.lock().unwrap();
        reset_state(200); // 200ms
        TEST_KEY_HELD.store(false, Ordering::Relaxed);
        let st = state();
        // 押下: 100ms でどこまで進むか (ease-out: t=0.5 → 1-0.125=0.875)
        TEST_KEY_HELD.store(true, Ordering::Relaxed);
        for _ in 0..5 {
            rsift_mod_on_render(1920, 1080, 0.02);
        }
        let mid = rsift_mod_get_fov_scale();
        assert!(mid > 1.0, "押下でズーム開始: {mid}");
        let expected = 1.0 + (4.0 - 1.0) * (1.0 - (1.0f32 - 0.5).powi(3));
        assert!(
            (mid - expected).abs() < 0.05,
            "100ms 時点で ease-out 軌道: {mid} vs {expected}"
        );
        // 合計 200ms で満倍率にちょうど収束
        for _ in 0..5 {
            rsift_mod_on_render(1920, 1080, 0.02);
        }
        assert!(
            (rsift_mod_get_fov_scale() - 4.0).abs() < 1e-6,
            "満倍率 4.0x"
        );
        assert!(st.ease.lock().unwrap().held);
        // 解放: 同じ時間で同じカーブを逆走 (ease-out で戻る)
        TEST_KEY_HELD.store(false, Ordering::Relaxed);
        rsift_mod_on_render(1920, 1080, 0.02);
        let back_early = rsift_mod_get_fov_scale();
        assert!(back_early < 4.0, "解放で戻り開始: {back_early}");
        for _ in 0..9 {
            rsift_mod_on_render(1920, 1080, 0.02);
        }
        assert!(
            (rsift_mod_get_fov_scale() - 1.0).abs() < 1e-6,
            "完全に等倍へ戻る"
        );
    }

    #[test]
    fn mid_animation_release_is_continuous_no_jump() {
        let _g = SERIAL.lock().unwrap();
        reset_state(400);
        TEST_KEY_HELD.store(true, Ordering::Relaxed);
        rsift_mod_on_render(0, 0, 0.05); // 50ms 押下 (t=0.125)
        let before = rsift_mod_get_fov_scale();
        TEST_KEY_HELD.store(false, Ordering::Relaxed);
        rsift_mod_on_render(0, 0, 0.0); // エッジのみ、時間経過なし
        let after = rsift_mod_get_fov_scale();
        assert!(
            (after - before).abs() < 1e-6,
            "方向転換の瞬間に値が跳ばない: {before} → {after}"
        );
        // 戻り始めは単調減少
        let mut prev = after;
        for _ in 0..4 {
            rsift_mod_on_render(0, 0, 0.05);
            let v = rsift_mod_get_fov_scale();
            assert!(v < prev, "解放後は単調減少: {prev} → {v}");
            prev = v;
        }
    }

    #[test]
    fn animation_is_frame_rate_independent() {
        let _g = SERIAL.lock().unwrap();
        // 同じ 0.3 秒 (アニメ 500ms の 60% 地点、飽和しない区間) を 60fps 刻みと
        // 240fps 刻みで回して同じ到達値になること。
        let run = |dt: f32, steps: usize| -> f32 {
            reset_state(500);
            TEST_KEY_HELD.store(true, Ordering::Relaxed);
            for _ in 0..steps {
                rsift_mod_on_render(0, 0, dt);
            }
            rsift_mod_get_fov_scale()
        };
        let a = run(1.0 / 60.0, 18); // 0.3s
        let b = run(1.0 / 240.0, 72); // 0.3s
        assert!(
            (a - b).abs() < 1e-3,
            "フレームレート非依存: 60fps={a} 240fps={b}"
        );
        // 期待絶対値もピン (t=0.6 → ease-out 0.936 → 1+3*0.936 = 3.808)
        let want = 1.0 + 3.0 * (1.0 - (1.0f32 - 0.6).powi(3));
        assert!((a - want).abs() < 0.02, "60% 地点の絶対値: {a} vs {want}");
    }

    #[test]
    fn dt_guards_survive_nan_negative_and_huge() {
        let _g = SERIAL.lock().unwrap();
        reset_state(200);
        TEST_KEY_HELD.store(true, Ordering::Relaxed);
        rsift_mod_on_render(0, 0, f32::NAN); // 0 として扱う
        assert!((rsift_mod_get_fov_scale() - 1.0).abs() < 1e-6);
        rsift_mod_on_render(0, 0, -1.0); // 負は 0
        assert!((rsift_mod_get_fov_scale() - 1.0).abs() < 1e-6);
        rsift_mod_on_render(0, 0, 999.0); // 0.1s にクランプ = t 0.5 相当
        let v = rsift_mod_get_fov_scale();
        assert!(v > 1.9 && v < 4.0, "巨大 dt は 0.1s クランプ済: {v}");
    }

    #[test]
    fn smooth_off_snaps_immediately() {
        let _g = SERIAL.lock().unwrap();
        reset_state(200);
        *state().smooth.write().unwrap() = false;
        TEST_KEY_HELD.store(true, Ordering::Relaxed);
        rsift_mod_on_render(0, 0, 0.001);
        assert!((rsift_mod_get_fov_scale() - 4.0).abs() < 1e-6, "即時満倍率");
        TEST_KEY_HELD.store(false, Ordering::Relaxed);
        rsift_mod_on_render(0, 0, 0.001);
        assert!((rsift_mod_get_fov_scale() - 1.0).abs() < 1e-6, "即時等倍");
    }

    #[test]
    fn config_roundtrip_and_malformed_lines_are_skipped() {
        let _g = SERIAL.lock().unwrap();
        reset_state(200);
        let dir = std::env::temp_dir().join(format!("rszoom_cfg_{}", std::process::id()));
        let path = dir.join("rszoom.cfg");
        *state().factor_e10.write().unwrap() = 65;
        *state().anim_ms.write().unwrap() = 123;
        *state().smooth.write().unwrap() = false;
        save_config(state(), &path);
        // 破損行を混ぜても読めること
        let mut body = std::fs::read_to_string(&path).unwrap();
        body.push_str("broken line\nfactor_e10=notanumber\nunknown_key=1\n");
        std::fs::write(&path, body).unwrap();
        reset_state(200); // 既定に戻す
        load_config(state(), &path);
        assert_eq!(*state().factor_e10.read().unwrap(), 65);
        assert_eq!(*state().anim_ms.read().unwrap(), 123);
        assert!(!*state().smooth.read().unwrap());
        // 範囲外の保存値は読込でクランプ
        std::fs::write(&path, "factor_e10=9999\nanim_ms=-5\n").unwrap();
        load_config(state(), &path);
        assert_eq!(*state().factor_e10.read().unwrap(), FACTOR_MAX_E10);
        assert_eq!(*state().anim_ms.read().unwrap(), ANIM_MS_MIN);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_registers_menu_and_config_flow_changes_factor_end_to_end() {
        let _g = SERIAL.lock().unwrap();
        reset_state(200);
        let mut registry = rsift_api::ModRegistry::new();
        let runtime = rsift_api::runtime::RsiftRuntime::new(PathBuf::from("./mods"));
        let manifest = ModManifest {
            id: "rszoom".into(),
            name: "RsZoom".into(),
            version: "1.0.0".into(),
            author: "test".into(),
            description: "t".into(),
            target_rsift_version: TARGET_MINECRAFT_VERSION.into(),
            capabilities: vec!["input_capture".into()],
        };
        let mut ctx = ModContext::new(manifest, &mut registry, true, &runtime);
        assert_eq!(rsift_mod_init(&mut ctx), 0, "init success");
        // Mods メニューに登録され、config 画面 builder が抱えている
        let entry = runtime
            .mod_menu
            .entries
            .read()
            .unwrap()
            .get("rszoom")
            .cloned()
            .expect("menu へ登録");
        let cfg = entry.cloth_config_screen.clone().expect("config builder");
        // 設定画面を開く → セル値 40 (4.0x)
        cfg.open_screen();
        // ホスト画面行押下 (バニラボタン) → 値サイクル → sync → cfg 保存
        let dir = std::env::temp_dir().join(format!("rszoom_ui_{}", std::process::id()));
        std::env::set_var("RSZOOM_CONFIG_PATH", dir.join("rszoom.cfg"));
        rsift_api::press_row("Zoom 倍率|int:20:100:40");
        assert_eq!(
            *state().factor_e10.read().unwrap(),
            44,
            "1 押下で step4 (0.4x) 上昇"
        );
        assert!(
            dir.join("rszoom.cfg").is_file(),
            "on_change で cfg が保存される"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fov_scale_mapping_and_clamps() {
        let _g = SERIAL.lock().unwrap();
        reset_state(200);
        let ease = state().ease.lock().unwrap().clone();
        assert_eq!(ease.fov_scale(4.0), 1.0);
        let mut z = ZoomEase::new(0.1);
        z.amount = 1.0;
        assert!((z.fov_scale(4.0) - 4.0).abs() < 1e-6);
        // factor の下限ガード (壊れた設定でも <=1x にはしない)
        z.amount = 0.5;
        assert_eq!(z.fov_scale(0.1), 1.0, "factor<1 は 1.0 へ矯正");
        assert_eq!(z.fov_scale(f32::NAN), 1.0);
    }
}
