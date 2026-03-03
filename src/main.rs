#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
pub mod gameinput;
pub mod process;
pub mod steam;

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use process::AppInfo;
use steam::SteamGame;

fn main() {
    let game_input = Arc::new(match gameinput::GameInputHandle::init() {
        Ok(handle) => handle,
        Err(hr) => {
            eprintln!("GameInput init failed: 0x{hr:08X}");
            std::process::exit(1);
        }
    });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_decorations(false),
        ..Default::default()
    };

    eframe::run_native(
        "Launcher",
        native_options,
        Box::new(move |_cc| Ok(Box::new(LauncherApp::new(Arc::clone(&game_input))))),
    )
    .unwrap_or_else(|e| eprintln!("eframe error: {e}"));
}

#[derive(PartialEq)]
enum Panel {
    Windows,
    Steam,
}

struct LauncherApp {
    game_input: Arc<gameinput::GameInputHandle>,
    windows: Vec<AppInfo>,
    steam_games: Vec<SteamGame>,
    visible: bool,
    shown_at: Option<Instant>,
    initialized: bool,
    active_panel: Panel,
    selected_steam: usize,
}

const DEBOUNCE: Duration = Duration::from_millis(500);

impl LauncherApp {
    fn new(game_input: Arc<gameinput::GameInputHandle>) -> Self {
        game_input.drain_guide_presses();
        Self {
            game_input,
            windows: Vec::new(),
            steam_games: Vec::new(),
            visible: false,
            shown_at: None,
            initialized: false,
            active_panel: Panel::Steam,
            selected_steam: 0,
        }
    }

    fn refresh(&mut self) {
        self.windows = process::get_all_windows();
        self.steam_games = steam::get_steam_games_from_manifests();
        self.selected_steam = self
            .selected_steam
            .min(self.steam_games.len().saturating_sub(1));
    }

    fn show(&mut self, ctx: &egui::Context) {
        self.refresh();
        self.visible = true;
        self.shown_at = Some(Instant::now());
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.visible = false;
        self.shown_at = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }

    fn debounce_elapsed(&self) -> bool {
        self.shown_at
            .map(|t| t.elapsed() >= DEBOUNCE)
            .unwrap_or(true)
    }

    fn launch_selected_steam_game(&self) {
        if let Some(game) = self.steam_games.get(self.selected_steam) {
            let url = format!("steam://rungameid/{}", game.appid);
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", "", &url])
                .spawn();
        }
    }
}

impl eframe::App for LauncherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Minimize on the very first frame before anything is painted visibly.
        if !self.initialized {
            self.initialized = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            ctx.request_repaint();
            return;
        }

        let guide_presses = self.game_input.drain_guide_presses();
        if guide_presses > 0 {
            if !self.visible {
                self.show(ctx);
            } else if self.debounce_elapsed() {
                self.hide(ctx);
            }
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.hide(ctx);
        }

        // Always repaint so Guide/D-pad presses are never missed.
        ctx.request_repaint();

        // Render nothing while minimized.
        if !self.visible {
            return;
        }

        // ── Gamepad D-pad + A ────────────────────────────────────────────
        let pad = self.game_input.drain_gamepad_events();

        // Left/Right switches active panel.
        if pad.dpad_left > 0 {
            self.active_panel = Panel::Windows;
        }
        if pad.dpad_right > 0 {
            self.active_panel = Panel::Steam;
        }

        // Up/Down moves selection within the active panel.
        if self.active_panel == Panel::Steam && !self.steam_games.is_empty() {
            if pad.dpad_up > 0 {
                if self.selected_steam > 0 {
                    self.selected_steam -= 1;
                }
            }
            if pad.dpad_down > 0 {
                if self.selected_steam + 1 < self.steam_games.len() {
                    self.selected_steam += 1;
                }
            }
        }

        // A launches selected Steam game.
        if pad.a > 0 && self.active_panel == Panel::Steam {
            self.launch_selected_steam_game();
            self.hide(ctx);
            return;
        }

        // Keyboard arrow fallback (useful when testing without a controller).
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            self.active_panel = Panel::Windows;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            self.active_panel = Panel::Steam;
        }
        if self.active_panel == Panel::Steam && !self.steam_games.is_empty() {
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) && self.selected_steam > 0 {
                self.selected_steam -= 1;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown))
                && self.selected_steam + 1 < self.steam_games.len()
            {
                self.selected_steam += 1;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.launch_selected_steam_game();
                self.hide(ctx);
                return;
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            // ── Header bar ───────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.heading("Launcher");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✖ Quit").clicked() {
                        std::process::exit(0);
                    }
                    if ui.button("Hide").clicked() {
                        self.hide(ctx);
                    }
                    if ui.button("⟳ Refresh").clicked() {
                        self.refresh();
                    }
                    ui.weak("[◀▶] panel  [▲▼] navigate  [A/Enter] launch  [Guide/Esc] hide");
                });
            });

            ui.separator();

            let steam_focused = self.active_panel == Panel::Steam;
            let selected_steam = self.selected_steam;

            // ── Two-column layout ────────────────────────────────────────
            // Compute sizes once so both columns share them.
            let total_w = ui.available_width();
            let total_h = ui.available_height();
            let col_w = (total_w - ui.spacing().item_spacing.x) / 2.0;
            let accent = ui.visuals().selection.bg_fill;

            ui.horizontal_top(|ui| {
                // ── Left column: Running Windows ─────────────────────────
                ui.vertical(|ui| {
                    ui.set_width(col_w);

                    // Column header
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

                    // Active-panel border drawn around the scroll area.
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
                                .max_height(total_h - 60.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(col_w - 16.0);
                                    for app in &self.windows {
                                        ui.group(|ui| {
                                            ui.set_min_width(ui.available_width());
                                            ui.label(egui::RichText::new(&app.name).strong());
                                            if !app.path.is_empty() {
                                                ui.label(
                                                    egui::RichText::new(&app.path).small().weak(),
                                                );
                                            }
                                        });
                                    }
                                });
                        });
                });

                // ── Right column: Steam Library ───────────────────────────
                ui.vertical(|ui| {
                    ui.set_width(col_w);

                    // Column header
                    ui.horizontal(|ui| {
                        if steam_focused {
                            ui.label(
                                egui::RichText::new("▶ Steam Library")
                                    .strong()
                                    .color(accent),
                            );
                        } else {
                            ui.strong("Steam Library");
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(format!("{} games", self.steam_games.len()));
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
                                .max_height(total_h - 60.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(col_w - 16.0);
                                    for (i, game) in self.steam_games.iter().enumerate() {
                                        let is_selected = steam_focused && i == selected_steam;

                                        let row_response = ui.group(|ui| {
                                            ui.set_min_width(ui.available_width());

                                            // Selection highlight
                                            if is_selected {
                                                let rect = ui.max_rect();
                                                ui.painter().rect_filled(
                                                    rect,
                                                    4.0,
                                                    accent.gamma_multiply(0.25),
                                                );
                                            }

                                            ui.horizontal(|ui| {
                                                if is_selected {
                                                    ui.label(
                                                        egui::RichText::new("▶").color(accent),
                                                    );
                                                } else {
                                                    ui.label(egui::RichText::new("  "));
                                                }
                                                ui.label(
                                                    egui::RichText::new(&game.name).strong().color(
                                                        if is_selected {
                                                            accent
                                                        } else {
                                                            ui.visuals().text_color()
                                                        },
                                                    ),
                                                );
                                            });

                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "  {} · {}",
                                                    game.appid, game.install_dir
                                                ))
                                                .small()
                                                .weak(),
                                            );
                                        });

                                        if is_selected {
                                            row_response
                                                .response
                                                .scroll_to_me(Some(egui::Align::Center));
                                        }

                                        if row_response.response.clicked() {
                                            self.selected_steam = i;
                                            self.active_panel = Panel::Steam;
                                        }
                                        if row_response.response.double_clicked() {
                                            self.selected_steam = i;
                                            self.launch_selected_steam_game();
                                            self.visible = false;
                                            self.shown_at = None;
                                            ctx.send_viewport_cmd(
                                                egui::ViewportCommand::Fullscreen(false),
                                            );
                                            ctx.send_viewport_cmd(
                                                egui::ViewportCommand::Minimized(true),
                                            );
                                        }
                                    }
                                });
                        });
                });
            });
        });
    }
}
