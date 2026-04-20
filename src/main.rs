#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
pub mod gameinput;
pub mod epic;
pub mod gog;
pub mod process;
pub mod steam;
pub mod webserver;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use epic::EpicGame;
use eframe::egui;
use gog::GogGame;
use steam::SteamGame;
use webserver::{GamesPayload, SharedGames};

fn main() {
    let windowed = std::env::args().any(|a| a == "--windowed");

    let shared_games: SharedGames = Arc::new(Mutex::new(GamesPayload::default()));
    webserver::start(Arc::clone(&shared_games));

    let game_input = Arc::new(match gameinput::GameInputHandle::init() {
        Ok(handle) => handle,
        Err(hr) => {
            eprintln!("GameInput init failed: 0x{hr:08X}");
            std::process::exit(1);
        }
    });

    let viewport = if windowed {
        egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_decorations(true)
    } else {
        egui::ViewportBuilder::default().with_decorations(false)
    };

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Launcher",
        native_options,
        Box::new(move |_cc| {
            Ok(Box::new(LauncherApp::new(
                Arc::clone(&game_input),
                shared_games,
                windowed,
            )))
        }),
    )
    .unwrap_or_else(|e| eprintln!("eframe error: {e}"));
}

#[derive(PartialEq)]
enum Panel {
    Windows,
    Steam,
}

/// Actions collected during the UI pass, executed afterwards to avoid
/// holding an immutable borrow on `self` while also needing a mutable one.
#[derive(Default)]
struct PendingAction {
    hide: bool,
    refresh: bool,
    launch_library_idx: Option<usize>,
    select_steam: Option<usize>,
    select_window: Option<usize>,
    switch_panel: Option<Panel>,
    resume_window_pid: Option<u32>,
}

struct LauncherApp {
    game_input: Arc<gameinput::GameInputHandle>,
    shared_games: SharedGames,
    windows: Vec<process::AppInfo>,
    steam_games: Vec<SteamGame>,
    epic_games: Vec<EpicGame>,
    gog_games: Vec<GogGame>,
    visible: bool,
    shown_at: Option<Instant>,
    initialized: bool,
    windowed: bool,
    active_panel: Panel,
    selected_steam: usize,
    selected_window: usize,
    scroll_to_steam: bool,
    scroll_to_window: bool,
    /// PID of the process that was suspended when we opened the launcher.
    /// Resumed when we hide.
    suspended_pid: Option<u32>,
}

const DEBOUNCE: Duration = Duration::from_millis(500);

impl LauncherApp {
    fn new(game_input: Arc<gameinput::GameInputHandle>, shared_games: SharedGames, windowed: bool) -> Self {
        game_input.drain_guide_presses();
        game_input.drain_combo_presses();
        let mut app = Self {
            game_input,
            shared_games,
            windows: Vec::new(),
            steam_games: Vec::new(),
            epic_games: Vec::new(),
            gog_games: Vec::new(),
            visible: windowed,
            shown_at: if windowed { Some(Instant::now()) } else { None },
            initialized: false,
            windowed,
            active_panel: Panel::Steam,
            selected_steam: 0,
            selected_window: 0,
            scroll_to_steam: false,
            scroll_to_window: false,
            suspended_pid: None,
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        self.windows = process::get_all_windows();
        self.steam_games = steam::get_steam_games_from_manifests();
        self.epic_games = epic::get_installed_epic_games();
        self.gog_games = gog::get_installed_gog_games();
        let library_len = self.library_len();
        self.selected_steam = self
            .selected_steam
            .min(library_len.saturating_sub(1));
        self.selected_window = self
            .selected_window
            .min(self.windows.len().saturating_sub(1));
        if let Ok(mut g) = self.shared_games.lock() {
            g.steam = self.steam_games.clone();
            g.epic = self.epic_games.clone();
            g.gog = self.gog_games.clone();
        }
    }

    fn library_len(&self) -> usize {
        self.steam_games.len() + self.epic_games.len() + self.gog_games.len()
    }

    /// Grab the foreground PID *before* we steal focus, suspend that process,
    /// then bring the launcher to the foreground.
    fn show(&mut self, ctx: &egui::Context) {
        // Find & suspend whatever is currently in the foreground.
        if let Some(pid) = process::get_foreground_pid() {
            let own_pid = std::process::id();
            if pid != own_pid {
                if let Err(e) = process::suspend_process_by_pid_and_minimize(pid) {
                    eprintln!("suspend pid {pid} failed: {e:?}");
                } else {
                    self.suspended_pid = Some(pid);
                }
            }
        }

        self.refresh();
        self.visible = true;
        self.shown_at = Some(Instant::now());
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        if !self.windowed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Hide the launcher and resume the previously suspended process.
    fn hide(&mut self, ctx: &egui::Context) {
        self.visible = false;
        self.shown_at = None;
        if !self.windowed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));

        if let Some(pid) = self.suspended_pid.take() {
            if let Err(e) = process::resume_process_by_pid_and_restore(pid) {
                eprintln!("resume pid {pid} failed: {e:?}");
            }
        }
    }

    fn debounce_elapsed(&self) -> bool {
        self.shown_at
            .map(|t| t.elapsed() >= DEBOUNCE)
            .unwrap_or(true)
    }

    fn launch_library_game(&self, idx: usize) {
        let steam_len = self.steam_games.len();
        if let Some(game) = self.steam_games.get(idx) {
            let url = format!("steam://rungameid/{}", game.appid);
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", "", &url])
                .spawn();
            return;
        }

        let epic_len = self.epic_games.len();
        let epic_idx = idx.saturating_sub(steam_len);
        if let Some(game) = self.epic_games.get(epic_idx) {
            if let Err(e) = epic::launch_epic_game(game) {
                eprintln!("epic launch failed ({}): {e}", game.display_name);
            }
            return;
        }

        let gog_idx = idx.saturating_sub(steam_len + epic_len);
        if let Some(game) = self.gog_games.get(gog_idx) {
            if let Err(e) = gog::launch_gog_game(game) {
                eprintln!("gog launch failed ({}): {e}", game.display_name);
            }
        }
    }
}

impl eframe::App for LauncherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // On the very first frame, register the repaint callback and (unless
        // in windowed mode) minimize before anything is painted.
        if !self.initialized {
            self.initialized = true;
            let ctx_clone = ctx.clone();
            self.game_input
                .set_repaint_callback(std::sync::Arc::new(move || {
                    ctx_clone.request_repaint();
                }));
            if !self.windowed {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                ctx.request_repaint();
                return;
            }
        }

        // ── Gamepad input ────────────────────────────────────────────────
        // drain_gamepad_events MUST run every frame (even while minimized)
        // because it is what detects the combo rising edge and increments
        // COMBO_PRESSED. If we only called it while visible we would never
        // see the press that opens the launcher.
        let pad = self.game_input.drain_gamepad_events();

        // ── Hotkey: View + B ─────────────────────────────────────────────
        // Drain Guide presses so the counter never overflows (callback still
        // runs), but we only act on the combo.
        self.game_input.drain_guide_presses();
        let combo_presses = self.game_input.drain_combo_presses();
        if combo_presses > 0 {
            if !self.visible {
                self.show(ctx);
            } else if self.debounce_elapsed() {
                self.hide(ctx);
            }
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.hide(ctx);
        }

        // Always repaint so combo/D-pad presses are never missed.
        ctx.request_repaint();

        // If the window was restored externally (e.g. clicking the taskbar)
        // while visible=false, sync up so we don't render a black screen.
        let is_minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if !is_minimized && !self.visible {
            self.visible = true;
            self.shown_at = Some(Instant::now());
            self.refresh();
        }

        if !self.visible {
            return;
        }

        if pad.dpad_left > 0 {
            self.active_panel = Panel::Windows;
        }
        if pad.dpad_right > 0 {
            self.active_panel = Panel::Steam;
        }

        match self.active_panel {
            Panel::Steam if self.library_len() > 0 => {
                if pad.dpad_up > 0 && self.selected_steam > 0 {
                    self.selected_steam -= 1;
                    self.scroll_to_steam = true;
                }
                if pad.dpad_down > 0 && self.selected_steam + 1 < self.library_len() {
                    self.selected_steam += 1;
                    self.scroll_to_steam = true;
                }
            }
            Panel::Windows if !self.windows.is_empty() => {
                if pad.dpad_up > 0 && self.selected_window > 0 {
                    self.selected_window -= 1;
                    self.scroll_to_window = true;
                }
                if pad.dpad_down > 0 && self.selected_window + 1 < self.windows.len() {
                    self.selected_window += 1;
                    self.scroll_to_window = true;
                }
            }
            _ => {}
        }

        if pad.a > 0 && self.active_panel == Panel::Steam {
            let idx = self.selected_steam;
            self.launch_library_game(idx);
            self.hide(ctx);
            return;
        }

        // ── Keyboard fallbacks ───────────────────────────────────────────
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            self.active_panel = Panel::Windows;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            self.active_panel = Panel::Steam;
        }

        match self.active_panel {
            Panel::Steam if self.library_len() > 0 => {
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) && self.selected_steam > 0 {
                    self.selected_steam -= 1;
                    self.scroll_to_steam = true;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown))
                    && self.selected_steam + 1 < self.library_len()
                {
                    self.selected_steam += 1;
                    self.scroll_to_steam = true;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let idx = self.selected_steam;
                    self.launch_library_game(idx);
                    self.hide(ctx);
                    return;
                }
            }
            Panel::Windows if !self.windows.is_empty() => {
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) && self.selected_window > 0 {
                    self.selected_window -= 1;
                    self.scroll_to_window = true;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown))
                    && self.selected_window + 1 < self.windows.len()
                {
                    self.selected_window += 1;
                    self.scroll_to_window = true;
                }
            }
            _ => {}
        }

        // ── UI ───────────────────────────────────────────────────────────
        // Actions that require &mut self are collected here and applied
        // after the UI block so we never hold an immutable borrow on self
        // while also needing a mutable one.
        let mut action = PendingAction::default();

        egui::CentralPanel::default().show(ctx, |ui| {
            // Header bar
            ui.horizontal(|ui| {
                ui.heading("Launcher");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✖ Quit").clicked() {
                        std::process::exit(0);
                    }
                    if ui.button("Hide").clicked() {
                        action.hide = true;
                    }
                    if ui.button("⟳ Refresh").clicked() {
                        action.refresh = true;
                    }
                    ui.weak("[◀▶] panel  [▲▼] navigate  [A/Enter] launch  [View+B/Esc] hide");
                });
            });

            // Suspended process banner
            if let Some(pid) = self.suspended_pid {
                let name = self
                    .windows
                    .iter()
                    .find(|w| w.pid == pid)
                    .map(|w| w.name.as_str())
                    .unwrap_or("unknown process");
                ui.horizontal(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 180, 50),
                        format!("⏸  Suspended: {name}  — will resume on hide"),
                    );
                });
            }

            ui.separator();

            let steam_focused = self.active_panel == Panel::Steam;
            let selected_steam = self.selected_steam;
            let selected_window = self.selected_window;
            let suspended_pid = self.suspended_pid;
            let scroll_to_steam = self.scroll_to_steam;
            let scroll_to_window = self.scroll_to_window;
            self.scroll_to_steam = false;
            self.scroll_to_window = false;

            let total_w = ui.available_width();
            let total_h = ui.available_height();
            let col_w = (total_w - ui.spacing().item_spacing.x) / 2.0;
            let accent = ui.visuals().selection.bg_fill;
            let suspended_colour = egui::Color32::from_rgb(255, 180, 50);

            ui.horizontal_top(|ui| {
                // ── Left column: Running Windows ─────────────────────────
                ui.vertical(|ui| {
                    ui.set_width(col_w);

                    ui.horizontal(|ui| {
                        if !steam_focused {
                            ui.label(
                                egui::RichText::new("▶ Running Windows")
                                    .strong()
                                    .color(accent),
                            );
                        } else {
                            ui.strong("Running Windows");
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(format!("{} open", self.windows.len()));
                        });
                    });

                    let stroke = if !steam_focused {
                        egui::Stroke::new(1.5, accent)
                    } else {
                        egui::Stroke::NONE
                    };

                    egui::Frame::default()
                        .stroke(stroke)
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("windows_scroll")
                                .max_height(total_h - 80.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(col_w - 16.0);
                                    for (i, app) in self.windows.iter().enumerate() {
                                        let is_selected = !steam_focused && i == selected_window;
                                        let is_suspended = suspended_pid == Some(app.pid);

                                        let mut switch_clicked = false;
                                        let inner = ui.vertical(|ui| {
                                            ui.set_min_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                if is_suspended {
                                                    ui.label(
                                                        egui::RichText::new("⏸ ")
                                                            .color(suspended_colour),
                                                    );
                                                }
                                                ui.label(
                                                    egui::RichText::new(&app.name).strong().color(
                                                        if is_suspended {
                                                            suspended_colour
                                                        } else {
                                                            ui.visuals().text_color()
                                                        },
                                                    ),
                                                );
                                            });
                                            if !app.path.is_empty() {
                                                ui.label(
                                                    egui::RichText::new(&app.path).small().weak(),
                                                );
                                            }
                                            if is_selected {
                                                let btn_label = if is_suspended {
                                                    "▶ Resume & Switch"
                                                } else {
                                                    "⇱ Switch To"
                                                };
                                                if ui.button(btn_label).clicked() {
                                                    switch_clicked = true;
                                                }
                                            }
                                        });

                                        let row_rect = inner.response.rect;
                                        if is_selected {
                                            ui.painter().rect_filled(
                                                row_rect,
                                                2.0,
                                                accent.gamma_multiply(0.2),
                                            );
                                        }
                                        let row_interact = ui.interact(
                                            row_rect,
                                            ui.id().with(("win_row", i)),
                                            egui::Sense::click(),
                                        );

                                        if switch_clicked {
                                            action.resume_window_pid = Some(app.pid);
                                            action.hide = true;
                                        } else if row_interact.clicked() {
                                            action.select_window = Some(i);
                                            action.switch_panel = Some(Panel::Windows);
                                        } else if row_interact.double_clicked() {
                                            action.select_window = Some(i);
                                            action.switch_panel = Some(Panel::Windows);
                                            action.resume_window_pid = Some(app.pid);
                                            action.hide = true;
                                        }
                                        if is_selected && scroll_to_window {
                                            row_interact.scroll_to_me(Some(egui::Align::Center));
                                        }
                                    }
                                });
                        });
                });

                // ── Right column: Steam Library ───────────────────────────
                ui.vertical(|ui| {
                    ui.set_width(col_w);

                    ui.horizontal(|ui| {
                        if steam_focused {
                            ui.label(
                                egui::RichText::new("▶ Library")
                                    .strong()
                                    .color(accent),
                            );
                        } else {
                            ui.strong("Library");
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(format!("{} games", self.library_len()));
                        });
                    });

                    let stroke = if steam_focused {
                        egui::Stroke::new(1.5, accent)
                    } else {
                        egui::Stroke::NONE
                    };

                    egui::Frame::default()
                        .stroke(stroke)
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("steam_scroll")
                                .max_height(total_h - 80.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(col_w - 16.0);
                                    for i in 0..self.library_len() {
                                        let is_selected = steam_focused && i == selected_steam;
                                        let steam_len = self.steam_games.len();
                                        let epic_len = self.epic_games.len();
                                        let is_steam = i < steam_len;
                                        let is_epic = i >= steam_len && i < steam_len + epic_len;

                                        let mut launch_clicked = false;

                                        let inner = ui.vertical(|ui| {
                                            ui.set_min_width(ui.available_width());
                                            if is_steam {
                                                let game = &self.steam_games[i];
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new("STEAM")
                                                            .small()
                                                            .strong()
                                                            .color(egui::Color32::from_rgb(25, 35, 55))
                                                            .background_color(egui::Color32::from_rgb(125, 170, 235)),
                                                    );
                                                    ui.label(egui::RichText::new(&game.name).strong());
                                                });
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "Steam · {} · {}",
                                                        game.appid, game.install_dir
                                                    ))
                                                    .small()
                                                    .weak(),
                                                );
                                            } else if is_epic {
                                                let epic_idx = i - steam_len;
                                                let game = &self.epic_games[epic_idx];
                                                let title = if game.display_name.is_empty() {
                                                    &game.app_name
                                                } else {
                                                    &game.display_name
                                                };
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new("EPIC")
                                                            .small()
                                                            .strong()
                                                            .color(egui::Color32::from_rgb(245, 245, 245))
                                                            .background_color(egui::Color32::from_rgb(45, 45, 45)),
                                                    );
                                                    ui.label(egui::RichText::new(title).strong());
                                                });
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "Epic · {} · {}",
                                                        game.app_name, game.install_location
                                                    ))
                                                    .small()
                                                    .weak(),
                                                );
                                            } else {
                                                let gog_idx = i - steam_len - epic_len;
                                                let game = &self.gog_games[gog_idx];
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new("GOG")
                                                            .small()
                                                            .strong()
                                                            .color(egui::Color32::from_rgb(250, 240, 220))
                                                            .background_color(egui::Color32::from_rgb(95, 55, 25)),
                                                    );
                                                    ui.label(egui::RichText::new(&game.display_name).strong());
                                                });
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "GOG · {}",
                                                        game.install_location
                                                    ))
                                                    .small()
                                                    .weak(),
                                                );
                                            }
                                            if is_selected {
                                                if ui.button("▶ Launch").clicked() {
                                                    launch_clicked = true;
                                                }
                                            }
                                        });

                                        let row_rect = inner.response.rect;
                                        if is_selected {
                                            ui.painter().rect_filled(
                                                row_rect,
                                                2.0,
                                                accent.gamma_multiply(0.2),
                                            );
                                        }
                                        let row_interact = ui.interact(
                                            row_rect,
                                            ui.id().with(("steam_row", i)),
                                            egui::Sense::click(),
                                        );

                                        if launch_clicked {
                                            action.select_steam = Some(i);
                                            action.launch_library_idx = Some(i);
                                            action.hide = true;
                                        } else if row_interact.clicked() {
                                            action.select_steam = Some(i);
                                            action.switch_panel = Some(Panel::Steam);
                                        } else if row_interact.double_clicked() {
                                            action.select_steam = Some(i);
                                            action.launch_library_idx = Some(i);
                                            action.hide = true;
                                        }
                                        if is_selected && scroll_to_steam {
                                            row_interact.scroll_to_me(Some(egui::Align::Center));
                                        }
                                    }
                                });
                        });
                });
            });
        });

        // ── Apply pending actions ────────────────────────────────────────
        if action.refresh {
            self.refresh();
        }
        if let Some(p) = action.switch_panel {
            self.active_panel = p;
        }
        if let Some(i) = action.select_steam {
            self.selected_steam = i;
        }
        if let Some(i) = action.select_window {
            self.selected_window = i;
        }
        if let Some(pid) = action.resume_window_pid {
            // If this is the suspended process, clear it so hide() doesn't
            // resume it again — we handle it explicitly here.
            if self.suspended_pid == Some(pid) {
                self.suspended_pid = None;
            }
            if let Err(e) = process::resume_process_by_pid_and_restore(pid) {
                eprintln!("resume pid {pid} failed: {e:?}");
            }
        }
        if let Some(idx) = action.launch_library_idx {
            self.launch_library_game(idx);
        }
        if action.hide {
            self.hide(ctx);
        }
    }
}
