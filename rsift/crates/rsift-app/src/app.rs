//! Main application — ふわっとしたモダングラス UI +
//! 「Minecraft ランチャー方式」の本実装起動構成 (java 子プロセス)。

use eframe::egui::{self, Color32, FontId, RichText, Stroke, Ui, Vec2};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::Child;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::profiles::{detect_install_root, ProfileStore};
use crate::theme::{
    self, chip_frame, console_frame, ease_out_expo, glass_card, hero_card, lerp, sidebar_frame,
    title_bar_frame, AQUA, AQUA_DEEP, AQUA_GLOW, INK, LAVENDER, MINT, PEACH, SKY,
};
use rsift_installer::{InstallOptions, LauncherInstaller};
use rsift_launch::{detect_java_home, prepare_vanilla_launch, LaunchPlan, OfflineLaunchConfig};

const MAX_LOG_LINES: usize = 600;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Profiles,
    Install,
    Settings,
    Logs,
}

enum BgMsg {
    InstallOk(String),
    InstallErr(String),
    Prepared(Result<LaunchPlan, String>),
    LogLine(String),
}

pub struct RsiftApp {
    store: ProfileStore,
    page: Page,
    open_time: f64,
    page_blend: f32,
    status: String,

    // install
    install_log: String,
    install_busy: bool,

    // launch
    launch_busy: bool,
    mc_child: Option<Child>,

    // log
    log_lines: VecDeque<String>,

    // bg channel
    tx: Option<Sender<BgMsg>>,
    rx: Receiver<BgMsg>,

    // ui edit buffers
    new_profile_name: String,
    minecraft_dir_edit: String,
    java_home_display: String,
    java_override_edit: String,
    xms_gb: u32,
    xmx_gb: u32,
    extra_args_edit: String,
    last_selected: usize,
    confirm_delete: bool,
}

impl RsiftApp {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let mut store = ProfileStore::load();
        store.install_root = detect_install_root();
        store.refresh_versions();
        let minecraft_dir_edit = store.minecraft_dir.display().to_string();
        let java_override_edit = store.java_override.clone();
        let mut app = Self {
            store,
            page: Page::Home,
            open_time: 0.0,
            page_blend: 1.0,
            status: "Rsift 直接起動モード — 公式ランチャー不要".into(),
            install_log: String::new(),
            install_busy: false,
            launch_busy: false,
            mc_child: None,
            log_lines: VecDeque::new(),
            tx: Some(tx),
            rx,
            new_profile_name: "新しい構成".into(),
            minecraft_dir_edit,
            java_home_display: String::new(),
            java_override_edit,
            xms_gb: 4,
            xmx_gb: 8,
            extra_args_edit: String::new(),
            last_selected: usize::MAX,
            confirm_delete: false,
        };
        app.sync_profile_fields();
        app.refresh_java_display();
        app
    }

    // ---------- helpers ----------

    fn bg_tx(&self) -> Sender<BgMsg> {
        self.tx.as_ref().expect("tx").clone()
    }

    fn sync_profile_fields(&mut self) {
        let p = self.store.selected_profile();
        self.xms_gb = parse_gb(&p.min_heap, 4);
        self.xmx_gb = parse_gb(&p.max_heap, 8);
        self.extra_args_edit = p.extra_jvm_args.join(" ");
        self.last_selected = self.store.selected;
        self.confirm_delete = false;
    }

    fn write_back_profile_fields(&mut self) {
        if self.xmx_gb < self.xms_gb {
            self.xmx_gb = self.xms_gb;
        }
        let p = self.store.selected_profile_mut();
        p.min_heap = format!("-Xms{}G", self.xms_gb);
        p.max_heap = format!("-Xmx{}G", self.xmx_gb);
        p.extra_jvm_args = self
            .extra_args_edit
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
    }

    fn refresh_java_display(&mut self) {
        let o = self.store.java_override.trim().to_string();
        if !o.is_empty() {
            self.java_home_display = format!("指定: {}", o);
        } else {
            self.java_home_display = detect_java_home(&self.store.minecraft_dir, 21)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "未検出 — 設定でパス指定可".into());
        }
    }

    fn game_running(&self) -> bool {
        self.mc_child.is_some()
    }

    fn poll_bg(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                BgMsg::InstallOk(id) => {
                    self.install_busy = false;
                    self.status = format!("インストール完了: {id}");
                    self.install_log.push_str(&format!("\n✓ {id}\n"));
                    self.store.refresh_versions();
                    self.store.selected_profile_mut().version_id = id;
                    self.store.save();
                }
                BgMsg::InstallErr(e) => {
                    self.install_busy = false;
                    self.status = format!("インストール失敗: {e}");
                    self.install_log.push_str(&format!("\n✗ {e}\n"));
                }
                BgMsg::Prepared(Ok(plan)) => {
                    self.spawn_plan(plan);
                }
                BgMsg::Prepared(Err(e)) => {
                    self.launch_busy = false;
                    self.status = format!("起動失敗: {e}");
                    self.log_lines.push_back(format!("[rsift] ✗ {e}"));
                    self.trim_log();
                }
                BgMsg::LogLine(line) => {
                    self.log_lines.push_back(line);
                    self.trim_log();
                }
            }
        }
    }

    fn trim_log(&mut self) {
        while self.log_lines.len() > MAX_LOG_LINES {
            self.log_lines.pop_front();
        }
    }

    fn poll_child(&mut self) {
        let mut finished: Option<String> = None;
        if let Some(child) = &mut self.mc_child {
            match child.try_wait() {
                Ok(Some(code)) => {
                    finished = Some(format!("Minecraft 終了 (コード: {code})"));
                }
                Ok(None) => {}
                Err(e) => {
                    finished = Some(format!("プロセス監視エラー: {e}"));
                }
            }
        }
        if let Some(msg) = finished {
            self.mc_child = None;
            self.status = msg.clone();
            self.log_lines.push_back(format!("[rsift] {msg}"));
            self.trim_log();
        }
    }

    // ---------- launch / install ----------

    fn spawn_plan(&mut self, plan: LaunchPlan) {
        self.log_lines.push_back("─".repeat(40));
        self.log_lines.push_back(format!("[rsift] java: {}", plan.java.display()));
        self.log_lines.push_back(format!("[rsift] {} 引数: {} 個", "launch", plan.args.len()));
        self.trim_log();
        match plan.spawn() {
            Ok(mut child) => {
                if let Some(out) = child.stdout.take() {
                    pipe_lines(out, self.bg_tx());
                }
                if let Some(err) = child.stderr.take() {
                    pipe_lines(err, self.bg_tx());
                }
                let pid = child.id();
                self.mc_child = Some(child);
                self.status = format!("起動しました (PID {pid}) — ランチャーを閉じても続きます");
            }
            Err(e) => {
                self.status = e.clone();
                self.log_lines.push_back(format!("[rsift] ✗ {e}"));
            }
        }
        self.launch_busy = false;
        self.trim_log();
    }

    fn start_launch(&mut self) {
        if self.game_running() || self.launch_busy {
            return;
        }
        self.write_back_profile_fields();
        self.store.save();
        let p = self.store.selected_profile();
        if p.version_id.is_empty() {
            self.status = "バージョン未選択 — インストールタブでセットアップしてください".into();
            self.page = Page::Install;
            return;
        }
        self.launch_busy = true;
        self.status = "起動構成を組み立てています…".into();
        let cfg = OfflineLaunchConfig {
            minecraft_dir: self.store.minecraft_dir.clone(),
            install_root: self.store.install_root.clone(),
            profile: p.clone(),
        };
        let java_override = self.store.java_override.clone();
        let tx = self.bg_tx();
        thread::spawn(move || {
            let r = prepare_vanilla_launch(&cfg, &java_override);
            let _ = tx.send(BgMsg::Prepared(r));
        });
    }

    fn kill_game(&mut self) {
        if let Some(mut child) = self.mc_child.take() {
            let _ = child.kill();
            let _ = child.wait();
            self.status = "Minecraft プロセスを終了しました".into();
            self.log_lines.push_back("[rsift] ユーザーがプロセスを終了".into());
            self.trim_log();
        }
    }

    fn start_install(&mut self) {
        self.install_busy = true;
        self.install_log.push_str("インストール開始…\n");
        let root = self.store.install_root.clone();
        let tx = self.bg_tx();
        thread::spawn(move || {
            let result = (|| {
                let installer = LauncherInstaller::new()?;
                installer.install_with_options(&root, InstallOptions { shader_model: None })
            })();
            match result {
                Ok(r) => {
                    let _ = tx.send(BgMsg::InstallOk(r.version_id));
                }
                Err(e) => {
                    let _ = tx.send(BgMsg::InstallErr(e.to_string()));
                }
            }
        });
    }

    // ---------- chrome ----------

    fn draw_ambient(&self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let t = ui.ctx().input(|i| i.time) as f32;
        let p = ui.painter();

        let orbs = [
            (lerp(0.15, 0.24, (t * 0.28).sin() * 0.5 + 0.5), 0.40, 230.0, AQUA_GLOW),
            (lerp(0.78, 0.66, (t * 0.22).cos() * 0.5 + 0.5), 0.55, 190.0, AQUA),
            (0.50, lerp(0.80, 0.70, (t * 0.18).sin() * 0.5 + 0.5), 150.0, LAVENDER),
            (lerp(0.30, 0.42, (t * 0.15).cos() * 0.5 + 0.5), 0.85, 120.0, PEACH),
            (0.85, lerp(0.15, 0.30, (t * 0.20).sin() * 0.5 + 0.5), 110.0, MINT),
        ];
        for (nx, ny, r, c) in orbs {
            let center = egui::pos2(
                rect.min.x + rect.width() * nx,
                rect.min.y + rect.height() * ny,
            );
            p.circle_filled(
                center,
                r,
                Color32::from_rgba_premultiplied(c.r(), c.g(), c.b(), 15),
            );
        }

        // ふわっと瞬く小さな光
        for i in 0..12 {
            let fx = ((i * 61 + 13) % 97) as f32 / 97.0;
            let fy = ((i * 37 + 7) % 89) as f32 / 89.0;
            let phase = i as f32 * 0.83;
            let tw = (t * 1.2 + phase).sin() * 0.5 + 0.5;
            let center = egui::pos2(
                rect.min.x + rect.width() * fx,
                rect.min.y + rect.height() * fy,
            );
            let base = if i % 2 == 0 { LAVENDER } else { MINT };
            p.circle_filled(
                center,
                1.2 + (i % 3) as f32 * 0.6,
                Color32::from_rgba_premultiplied(
                    base.r(),
                    base.g(),
                    base.b(),
                    (10.0 + 22.0 * tw) as u8,
                ),
            );
        }
    }

    fn title_bar(&mut self, ctx: &egui::Context) {
        const BAR_H: f32 = 44.0;
        const CTRL_W: f32 = 100.0;

        egui::TopBottomPanel::top("titlebar")
            .exact_height(BAR_H)
            .frame(title_bar_frame())
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let controls = egui::Rect::from_min_max(
                    egui::pos2(full.max.x - CTRL_W, full.min.y),
                    full.max,
                );
                let drag = egui::Rect::from_min_max(full.min, egui::pos2(controls.min.x, full.max.y));

                if ui
                    .interact(drag, ui.id().with("title_drag"), egui::Sense::click_and_drag())
                    .dragged()
                {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                ui.allocate_ui_at_rect(drag.shrink2(egui::vec2(12.0, 0.0)), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("◆").size(14.0).color(AQUA));
                        ui.label(RichText::new("Rsift").strong().size(19.0).color(INK));
                        ui.label(RichText::new("Launcher").size(15.0).color(SKY));
                        ui.label(
                            RichText::new("1.21.11")
                                .size(11.0)
                                .color(Color32::from_rgba_premultiplied(196, 181, 253, 220)),
                        );
                    });
                });

                ui.allocate_ui_at_rect(controls, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.chrome_btn(ui, "X", true).clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if self.chrome_btn(ui, "—", false).clicked() {
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    });
                });
            });
    }

    fn chrome_btn(&self, ui: &mut Ui, label: &str, close: bool) -> egui::Response {
        let size = egui::vec2(44.0, 30.0);
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        let hover = resp.hovered();
        let fill = if close && hover {
            Color32::from_rgb(232, 17, 35)
        } else if hover {
            Color32::from_rgba_premultiplied(255, 255, 255, 60)
        } else {
            Color32::from_rgba_premultiplied(255, 255, 255, 20)
        };
        ui.painter().rect(
            rect,
            10.0,
            fill,
            Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 55)),
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            FontId::proportional(14.0),
            if close && hover { Color32::WHITE } else { INK },
        );
        resp
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(200.0)
            .frame(sidebar_frame())
            .show(ctx, |ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("メニュー")
                        .size(12.0)
                        .color(Color32::from_rgba_premultiplied(186, 230, 253, 180)),
                );
                ui.add_space(14.0);
                self.side_item(ui, "▶", "ホーム", "プレイ", Page::Home);
                self.side_item(ui, "◆", "構成", "プロファイル", Page::Profiles);
                self.side_item(ui, "▼", "インストール", "セットアップ", Page::Install);
                self.side_item(ui, "⚙", "設定", "Java・メモリ", Page::Settings);
                self.side_item(ui, "≡", "ログ", "起動ログ", Page::Logs);
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Java")
                            .size(11.0)
                            .color(Color32::from_rgba_premultiplied(125, 211, 252, 200)),
                    );
                    ui.label(
                        RichText::new(&self.java_home_display)
                            .size(10.0)
                            .color(Color32::from_rgba_premultiplied(186, 230, 253, 160)),
                    );
                });
            });
    }

    fn side_item(&mut self, ui: &mut Ui, icon: &str, title: &str, sub: &str, page: Page) {
        let active = self.page == page;
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 54.0), egui::Sense::click());
        let hover = resp.hovered();
        let fill = if active {
            Color32::from_rgba_premultiplied(34, 211, 238, 70)
        } else if hover {
            theme::GLASS_FILL_HOVER
        } else {
            Color32::TRANSPARENT
        };
        let stroke = if active {
            Stroke::new(1.5, AQUA_GLOW)
        } else {
            Stroke::NONE // vendored epaint では関連定数 NONE (none() は廃止)
        };
        ui.painter().rect(rect, 14.0, fill, stroke);
        if active {
            ui.painter().rect_filled(
                egui::Rect::from_min_max(rect.min, egui::pos2(rect.min.x + 3.0, rect.max.y)),
                2.0,
                AQUA,
            );
        }
        ui.painter().text(
            egui::pos2(rect.min.x + 14.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            icon,
            FontId::proportional(15.0),
            if active { AQUA_GLOW } else { SKY },
        );
        ui.painter().text(
            egui::pos2(rect.min.x + 36.0, rect.min.y + 11.0),
            egui::Align2::LEFT_TOP,
            title,
            FontId::proportional(15.0),
            if active { INK } else { SKY },
        );
        ui.painter().text(
            egui::pos2(rect.min.x + 36.0, rect.min.y + 31.0),
            egui::Align2::LEFT_TOP,
            sub,
            FontId::proportional(11.0),
            Color32::from_rgba_premultiplied(186, 230, 253, 140),
        );
        if resp.clicked() {
            self.page = page;
            self.page_blend = 0.0;
        }
    }

    // ---------- pages ----------

    fn page_home(&mut self, ui: &mut Ui) {
        let t = ui.ctx().input(|i| i.time);
        let anim = ease_out_expo(((t - self.open_time) * 0.9) as f32);
        let slide = lerp(32.0, 0.0, anim);
        let bob = ((t as f32) * 0.9).sin() * 4.0 + ((t as f32) * 0.5).cos() * 2.0;

        ui.add_space(slide + (34.0 + bob).max(20.0));

        let p = self.store.selected_profile().clone();
        let has_version = !p.version_id.is_empty();
        let running = self.game_running();

        hero_card().show(ui, |ui| {
            ui.set_max_width(540.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("✦ READY — ふわっと軽い").size(11.0).color(LAVENDER).strong());
                ui.add_space(8.0);
                ui.label(RichText::new("オフラインでプレイ").size(34.0).strong().color(INK));
                ui.add_space(6.0);
                ui.label(RichText::new(&p.name).size(19.0).color(AQUA));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{}  ·  {}",
                        p.username,
                        if has_version { p.version_id.clone() } else { "バージョン未選択".to_string() }
                    ))
                    .size(13.0)
                    .color(SKY),
                );
                ui.add_space(18.0);

                // ふわふわ情報チップ
                ui.horizontal(|ui| {
                    info_chip(ui, "バージョン", if has_version { short_version(&p.version_id) } else { "—".into() }, AQUA_GLOW);
                    info_chip(ui, "メモリ", format!("{} – {} GB", self.xms_gb, self.xmx_gb), LAVENDER);
                    info_chip(ui, "解像度", format!("{} × {}", p.width, p.height), MINT);
                });
                ui.add_space(26.0);

                self.play_button(ui, has_version, running, t as f32);
                ui.add_space(14.0);

                if running {
                    let pid = self.mc_child.as_ref().map(|c| c.id()).unwrap_or(0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("⚡ Minecraft 実行中 (PID {pid})"))
                                .size(12.0)
                                .color(MINT)
                                .strong(),
                        );
                        if ui
                            .add(
                                egui::Button::new(RichText::new("終了").size(11.0).color(INK))
                                    .fill(Color32::from_rgba_premultiplied(248, 113, 113, 110))
                                    .rounding(10.0)
                                    .min_size(Vec2::new(64.0, 24.0)),
                            )
                            .clicked()
                        {
                            self.kill_game();
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("構成を編集").size(12.0).color(SKY))
                                    .fill(Color32::from_rgba_premultiplied(255, 255, 255, 26))
                                    .rounding(10.0)
                                    .min_size(Vec2::new(96.0, 28.0)),
                            )
                            .clicked()
                        {
                            self.page = Page::Profiles;
                            self.page_blend = 0.0;
                        }
                        ui.add_space(8.0);
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("公式ランチャーで開く").size(12.0).color(SKY),
                                )
                                .fill(Color32::from_rgba_premultiplied(255, 255, 255, 26))
                                .rounding(10.0)
                                .min_size(Vec2::new(148.0, 28.0)),
                            )
                            .clicked()
                        {
                            match open_official_launcher() {
                                Ok(()) => self.status = "公式ランチャーを開きました".into(),
                                Err(e) => self.status = e,
                            }
                        }
                    });
                }
            });
        });
    }

    fn play_button(&mut self, ui: &mut Ui, has_version: bool, running: bool, t: f32) {
        let btn_size = Vec2::new(300.0, 64.0);
        let (rect, resp) = ui.allocate_exact_size(btn_size, egui::Sense::click());
        let busy = self.launch_busy;
        let enabled = has_version && !running && !busy;
        let hover = resp.hovered() && enabled;

        let breath = 0.5 + 0.5 * (t * 1.7).sin();
        if enabled {
            let glow_a = 20.0 + 24.0 * breath;
            ui.painter().rect_filled(
                rect.expand(6.0),
                36.0,
                Color32::from_rgba_premultiplied(34, 211, 238, glow_a as u8),
            );
            let grad_bot = if hover { Color32::from_rgb(8, 145, 178) } else { Color32::from_rgb(6, 116, 144) };
            let grad_top = if hover { Color32::from_rgb(34, 211, 238) } else { Color32::from_rgb(14, 165, 233) };
            ui.painter().rect_filled(rect, 32.0, grad_bot);
            let top_half = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.center().y));
            ui.painter().rect_filled(top_half, 32.0, grad_top);
            ui.painter().rect(
                rect,
                32.0,
                Color32::TRANSPARENT,
                Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 110)),
            );
        } else {
            ui.painter().rect_filled(rect, 32.0, Color32::from_rgba_premultiplied(100, 116, 139, 70));
        }

        let main_label = if running {
            "実行中…".to_string()
        } else if busy {
            "準備中…".to_string()
        } else if !has_version {
            "先にインストール".to_string()
        } else {
            "プレイ".to_string()
        };
        ui.painter().text(
            egui::pos2(rect.center().x, rect.center().y - 9.0),
            egui::Align2::CENTER_CENTER,
            main_label,
            FontId::proportional(24.0),
            INK,
        );
        ui.painter().text(
            egui::pos2(rect.center().x, rect.center().y + 18.0),
            egui::Align2::CENTER_CENTER,
            "Rsift 直接起動 — 公式ランチャー不要",
            FontId::proportional(11.0),
            Color32::from_rgba_premultiplied(240, 249, 255, 210),
        );

        if resp.clicked() && enabled {
            self.start_launch();
        }
    }

    fn page_profiles(&mut self, ui: &mut Ui) {
        glass_card().show(ui, |ui| {
            ui.label(RichText::new("起動構成").size(24.0).strong().color(INK));
            ui.add_space(10.0);

            // プロファイル切り替え
            let names: Vec<String> = self.store.profiles.iter().map(|p| p.name.clone()).collect();
            let mut sel = self.store.selected;
            egui::ComboBox::from_id_salt("profile_pick")
                .selected_text(self.store.selected_profile().name.clone())
                .width(280.0)
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut sel, i, n.clone());
                    }
                });
            if sel != self.store.selected {
                self.store.selected = sel.min(self.store.profiles.len().saturating_sub(1));
                self.sync_profile_fields();
                self.store.save();
            }
            ui.add_space(12.0);

            egui::Grid::new("profile_grid")
                .num_columns(2)
                .spacing([16.0, 12.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("名前").color(SKY));
                    ui.text_edit_singleline(&mut self.store.selected_profile_mut().name);
                    ui.end_row();

                    ui.label(RichText::new("ユーザー").color(SKY));
                    let p = self.store.selected_profile_mut();
                    ui.text_edit_singleline(&mut p.username);
                    let u = p.username.clone();
                    p.uuid = rsift_launch::offline_uuid(&u);
                    ui.end_row();

                    ui.label(RichText::new("バージョン").color(SKY));
                    let versions = rsift_launch::list_launchable_versions(&self.store.minecraft_dir);
                    egui::ComboBox::from_id_salt("ver")
                        .selected_text(if self.store.selected_profile().version_id.is_empty() {
                            "未選択".to_string()
                        } else {
                            self.store.selected_profile().version_id.clone()
                        })
                        .show_ui(ui, |ui| {
                            for v in versions {
                                ui.selectable_value(
                                    &mut self.store.selected_profile_mut().version_id,
                                    v.clone(),
                                    v,
                                );
                            }
                        });
                    ui.end_row();

                    ui.label(RichText::new("解像度").color(SKY));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.store.selected_profile_mut().width)
                                .range(640..=3840),
                        );
                        ui.label("×");
                        ui.add(
                            egui::DragValue::new(&mut self.store.selected_profile_mut().height)
                                .range(360..=2160),
                        );
                    });
                    ui.end_row();
                });
            ui.add_space(16.0);

            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("保存").color(INK))
                            .fill(Color32::from_rgba_premultiplied(34, 211, 238, 90))
                            .rounding(12.0)
                            .min_size(Vec2::new(88.0, 32.0)),
                    )
                    .clicked()
                {
                    self.store.save();
                    self.status = "構成を保存しました".into();
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_profile_name).desired_width(140.0),
                );
                if ui.add(egui::Button::new("＋ 新規").rounding(10.0)).clicked() {
                    let mut p = rsift_launch::LaunchProfile::default();
                    p.name = self.new_profile_name.clone();
                    p.version_id = self.store.selected_profile().version_id.clone();
                    self.store.profiles.push(p);
                    self.store.selected = self.store.profiles.len() - 1;
                    self.sync_profile_fields();
                    self.store.save();
                }
                if self.store.profiles.len() > 1 {
                    let del_label = if self.confirm_delete { "本当に削除？" } else { "削除" };
                    let del = ui.add(
                        egui::Button::new(RichText::new(del_label).color(if self.confirm_delete {
                            PEACH
                        } else {
                            SKY
                        }))
                        .rounding(10.0),
                    );
                    if del.clicked() {
                        if self.confirm_delete {
                            self.store.profiles.remove(self.store.selected);
                            self.store.selected = self
                                .store
                                .selected
                                .min(self.store.profiles.len().saturating_sub(1));
                            self.sync_profile_fields();
                            self.store.save();
                            self.status = "構成を削除しました".into();
                        } else {
                            self.confirm_delete = true;
                        }
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new("※ メモリと Java は「設定」タブにあります")
                    .size(11.0)
                    .color(Color32::from_rgba_premultiplied(186, 230, 253, 150)),
            );
        });
    }

    fn page_install(&mut self, ui: &mut Ui) {
        glass_card().show(ui, |ui| {
            ui.label(RichText::new("ローカルインストール").size(24.0).strong().color(INK));
            ui.label(
                RichText::new("バニラ 1.21.11 が .minecraft にあれば OK — Rsift Loader のバージョンを登録します")
                    .color(SKY),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(".minecraft").color(SKY));
                ui.add(egui::TextEdit::singleline(&mut self.minecraft_dir_edit).desired_width(360.0));
            });
            if ui.add(egui::Button::new("パスを適用").rounding(10.0)).clicked() {
                self.store.minecraft_dir = std::path::PathBuf::from(&self.minecraft_dir_edit);
                self.store.refresh_versions();
                self.store.save();
                self.refresh_java_display();
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.install_busy,
                        egui::Button::new(RichText::new("Rsift をインストール / 更新").color(INK))
                            .fill(AQUA_DEEP)
                            .rounding(12.0)
                            .min_size(Vec2::new(240.0, 40.0)),
                    )
                    .clicked()
                {
                    self.start_install();
                }
                if self.install_busy {
                    ui.add(egui::Spinner::new());
                }
            });
            if !self.install_log.is_empty() {
                ui.add_space(12.0);
                console_frame().show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        ui.monospace(&self.install_log);
                    });
                });
            }
        });
    }

    fn page_settings(&mut self, ui: &mut Ui) {
        glass_card().show(ui, |ui| {
            ui.label(RichText::new("設定").size(24.0).strong().color(INK));
            ui.add_space(12.0);

            ui.label(RichText::new("Java (空欄 = 自動検出)").color(SKY).size(13.0));
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.java_override_edit)
                        .desired_width(360.0)
                        .hint_text("C:\\Program Files\\Java\\jdk-21  (or java.exe)"),
                );
                if ui.add(egui::Button::new("自動検出に戻す").rounding(10.0)).clicked() {
                    self.java_override_edit.clear();
                }
            });
            ui.label(
                RichText::new(format!("検出結果: {}", self.java_home_display))
                    .size(11.0)
                    .color(Color32::from_rgba_premultiplied(186, 230, 253, 170)),
            );
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(10.0);

            ui.label(RichText::new("メモリ").color(SKY).size(13.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new("最小").size(12.0).color(SKY));
                ui.add(egui::Slider::new(&mut self.xms_gb, 1..=16).suffix(" GB"));
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("最大").size(12.0).color(SKY));
                ui.add(egui::Slider::new(&mut self.xmx_gb, 1..=32).suffix(" GB"));
            });
            ui.add_space(8.0);

            ui.label(RichText::new("追加 JVM 引数 (空白区切り)").color(SKY).size(13.0));
            ui.add(
                egui::TextEdit::singleline(&mut self.extra_args_edit)
                    .desired_width(460.0)
                    .hint_text("-XX:+UseZGC -XX:+ZGenerational …"),
            );
            ui.add_space(16.0);

            if ui
                .add(
                    egui::Button::new(RichText::new("設定を保存").color(INK))
                        .fill(Color32::from_rgba_premultiplied(34, 211, 238, 90))
                        .rounding(12.0)
                        .min_size(Vec2::new(120.0, 34.0)),
                )
                .clicked()
            {
                self.store.java_override = self.java_override_edit.trim().to_string();
                self.write_back_profile_fields();
                self.store.save();
                self.refresh_java_display();
                self.status = "設定を保存しました".into();
            }
        });
    }

    fn page_logs(&mut self, ui: &mut Ui) {
        glass_card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("起動ログ").size(24.0).strong().color(INK));
                ui.add_space(12.0);
                if ui.add(egui::Button::new("クリア").rounding(10.0)).clicked() {
                    self.log_lines.clear();
                }
                ui.label(
                    RichText::new(format!("{} 行", self.log_lines.len()))
                        .size(12.0)
                        .color(SKY),
                );
            });
            ui.add_space(10.0);
            console_frame().show(ui, |ui| {
                ui.set_max_width(680.0);
                egui::ScrollArea::vertical().max_height(420.0).stick_to_bottom(true).show(ui, |ui| {
                    let mut text: String = if self.log_lines.is_empty() {
                        "まだログはありません。ホームからプレイすると、ここに Minecraft の出力が流れます。"
                            .to_string()
                    } else {
                        self.log_lines.iter().cloned().collect::<Vec<_>>().join("\n")
                    };
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(20)
                            .desired_width(f32::INFINITY),
                    );
                });
            });
        });
    }

    // ---------- status ----------

    fn status_bar(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(32.0)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgba_premultiplied(255, 255, 255, 20))
                    .inner_margin(egui::Margin::symmetric(16.0, 6.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let dot = if self.game_running() {
                        MINT
                    } else if self.launch_busy || self.install_busy {
                        Color32::from_rgb(251, 191, 36)
                    } else {
                        AQUA_GLOW
                    };
                    ui.painter().circle_filled(
                        egui::pos2(ui.min_rect().min.x + 8.0, ui.min_rect().center().y),
                        4.0,
                        dot,
                    );
                    ui.add_space(16.0);
                    ui.label(RichText::new(&self.status).size(12.0).color(SKY));
                });
            });
    }
}

// ---------- free functions ----------

fn info_chip(ui: &mut Ui, label: &str, value: String, accent: Color32) {
    chip_frame().show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(
                RichText::new(label)
                    .size(10.0)
                    .color(Color32::from_rgba_premultiplied(186, 230, 253, 170)),
            );
            ui.label(RichText::new(value).size(13.0).color(accent).strong());
        });
    });
}

fn parse_gb(s: &str, def: u32) -> u32 {
    let t = s.trim();
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    let n = digits.parse::<u32>().unwrap_or(def);
    if t.ends_with('M') || t.ends_with('m') {
        (n / 1024).max(1)
    } else {
        n.max(1)
    }
}

fn short_version(id: &str) -> String {
    // "rsift-loader-1.21.11" → "1.21.11 ✦", "1.21.11" → "1.21.11 (バニラ)"
    if let Some(v) = id.strip_prefix("rsift-loader-") {
        format!("{v} ✦")
    } else {
        format!("{id} (バニラ)")
    }
}

fn pipe_lines<R: std::io::Read + Send + 'static>(reader: R, tx: Sender<BgMsg>) {
    thread::spawn(move || {
        let buf = BufReader::new(reader);
        for line in buf.lines().map_while(Result::ok) {
            if tx.send(BgMsg::LogLine(line)).is_err() {
                break;
            }
        }
    });
}

fn open_official_launcher() -> Result<(), String> {
    let launcher = find_official_launcher().ok_or_else(|| {
        "公式 Minecraft Launcher が見つかりません (Rsift は直接起動できるので不要です)".to_string()
    })?;
    std::process::Command::new(&launcher)
        .spawn()
        .map_err(|e| format!("公式ランチャー起動失敗 {:?}: {}", launcher, e))?;
    Ok(())
}

fn find_official_launcher() -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(
            std::path::PathBuf::from(&local)
                .join("Programs")
                .join("Minecraft Launcher")
                .join("MinecraftLauncher.exe"),
        );
        candidates.push(
            std::path::PathBuf::from(local)
                .join("Microsoft")
                .join("WindowsApps")
                .join("MinecraftLauncher.exe"),
        );
    }
    if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
        candidates.push(
            std::path::PathBuf::from(pf86)
                .join("Minecraft Launcher")
                .join("MinecraftLauncher.exe"),
        );
    }
    if let Ok(pf) = std::env::var("ProgramFiles") {
        candidates.push(
            std::path::PathBuf::from(pf)
                .join("Minecraft Launcher")
                .join("MinecraftLauncher.exe"),
        );
    }
    candidates.into_iter().find(|p| p.is_file())
}

// ---------- eframe entry ----------

impl eframe::App for RsiftApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.open_time == 0.0 {
            self.open_time = ctx.input(|i| i.time);
        }
        self.poll_bg();
        self.poll_child();
        if self.last_selected != self.store.selected && self.last_selected != usize::MAX {
            self.sync_profile_fields();
        }
        self.page_blend = (self.page_blend + ctx.input(|i| i.stable_dt) * 4.0).min(1.0);
        ctx.request_repaint();

        self.title_bar(ctx);
        self.sidebar(ctx);
        self.status_bar(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                self.draw_ambient(ui);
                ui.vertical_centered(|ui| {
                    ui.add_space(lerp(14.0, 0.0, self.page_blend));
                    let alpha = (self.page_blend * 255.0) as u8;
                    ui.scope(|ui| {
                        ui.style_mut().visuals.override_text_color =
                            Some(Color32::from_rgba_premultiplied(240, 249, 255, alpha));
                        match self.page {
                            Page::Home => self.page_home(ui),
                            Page::Profiles => self.page_profiles(ui),
                            Page::Install => self.page_install(ui),
                            Page::Settings => self.page_settings(ui),
                            Page::Logs => self.page_logs(ui),
                        }
                    });
                });
            });
    }
}
