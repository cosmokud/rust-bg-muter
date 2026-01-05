//! GUI module using egui
//! Implements the main application window

use crate::config::Config;
use crate::muter::{AppAudioState, MuterEngine};
use crate::startup;
use crate::tray::{SystemTray, TrayEvent};
use eframe::egui::{self, Color32, RichText, Vec2, Visuals};
use crossbeam_channel::Sender;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Embedded icon bytes (icon.png from assets folder)
const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.png");

/// Application state for the GUI
pub struct BackgroundMuterApp {
    config: Arc<RwLock<Config>>,
    engine: Arc<RwLock<MuterEngine>>,
    should_exit: Arc<AtomicBool>,
    shutdown_tx: Sender<()>,
    tray: Option<SystemTray>,
    tray_init_failed: bool,
    allow_close: bool,
    start_hidden: bool,
    applied_start_hidden: bool,
    detected_apps: Vec<AppAudioState>,
    manual_exe_input: String,
    last_refresh: Instant,
    refresh_interval: Duration,
    show_settings: bool,
    status_message: Option<(String, Instant)>,
    search_filter: String,
    /// Track the last known minimized state to detect minimize button clicks
    was_minimized: bool,
}

impl BackgroundMuterApp {
    /// Creates a new application instance
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: Arc<RwLock<Config>>,
        engine: Arc<RwLock<MuterEngine>>,
        should_exit: Arc<AtomicBool>,
        shutdown_tx: Sender<()>,
    ) -> Self {
        let start_hidden = config.read().start_minimized;

        let mut tray: Option<SystemTray> = None;
        let mut tray_init_failed = false;

        // Create tray on the GUI thread (winit message pump must be alive for Windows tray).
        match SystemTray::new() {
            Ok(mut t) => {
                let enabled = config.read().muting_enabled;
                if let Err(e) = t.initialize(enabled, should_exit.clone(), shutdown_tx.clone()) {
                    log::error!("Failed to initialize tray: {}", e);
                    tray_init_failed = true;
                } else {
                    tray = Some(t);
                }
            }
            Err(e) => {
                log::error!("Failed to create tray: {}", e);
                tray_init_failed = true;
            }
        }

        // If we started hidden, hide the window ASAP.
        if start_hidden {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        Self {
            config,
            engine,
            should_exit,
            shutdown_tx,
            tray,
            tray_init_failed,
            allow_close: false,
            start_hidden,
            applied_start_hidden: start_hidden,
            detected_apps: Vec::new(),
            manual_exe_input: String::new(),
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_millis(500),
            show_settings: false,
            status_message: None,
            search_filter: String::new(),
            was_minimized: start_hidden,
        }
    }

    fn hide_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn process_tray(&mut self, ctx: &egui::Context) {
        let events = match self.tray.as_ref() {
            Some(tray) => tray.process_events(),
            None => return,
        };

        for event in events {
            match event {
                TrayEvent::OpenWindow => self.show_window(ctx),
                TrayEvent::ToggleMuting => {
                    let enabled = self.config.write().toggle_muting();
                    if let Some(tray) = self.tray.as_mut() {
                        let _ = tray.update_icon(enabled);
                    }
                }
                TrayEvent::Exit => {
                    // Ensure the background thread is requested to stop immediately.
                    self.should_exit.store(true, Ordering::Relaxed);
                    let _ = self.shutdown_tx.try_send(());
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        // Keep tray icon state synced (cheap because SystemTray caches last state).
        let enabled = self.config.read().muting_enabled;
        if let Some(tray) = self.tray.as_mut() {
            let _ = tray.update_icon(enabled);
        }
    }

    /// Refreshes the detected apps list
    fn refresh_apps(&mut self) {
        if let Some(engine) = self.engine.try_read() {
            self.detected_apps = engine.get_active_sessions();
        }
        self.last_refresh = Instant::now();
    }

    /// Sets a temporary status message
    fn set_status(&mut self, message: impl Into<String>) {
        self.status_message = Some((message.into(), Instant::now()));
    }

    /// Renders the header section
    fn render_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("🔇 Background Muter").size(24.0));
            
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚙ Settings").clicked() {
                    self.show_settings = !self.show_settings;
                }
                
                let mut config = self.config.write();
                let toggle_text = if config.muting_enabled {
                    RichText::new("● ACTIVE").color(Color32::from_rgb(76, 175, 80))
                } else {
                    RichText::new("○ DISABLED").color(Color32::from_rgb(244, 67, 54))
                };
                
                if ui.button(toggle_text).clicked() {
                    config.toggle_muting();
                    let status = if config.muting_enabled {
                        "Muting enabled"
                    } else {
                        "Muting disabled"
                    };
                    drop(config);
                    self.set_status(status);
                }
            });
        });
        
        ui.separator();
    }

    /// Renders the detected apps section
    fn render_detected_apps(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        
        let mut should_refresh = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("🎵 Detected Audio Apps").size(18.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("🔄 Refresh").clicked() {
                    should_refresh = true;
                }
            });
        });
        
        if should_refresh {
            self.refresh_apps();
            self.set_status("Refreshed app list");
        }
        
        ui.add_space(4.0);
        
        // Search/filter box
        ui.horizontal(|ui| {
            ui.label("🔍");
            ui.add(
                egui::TextEdit::singleline(&mut self.search_filter)
                    .hint_text("Filter apps...")
                    .desired_width(200.0)
            );
            if !self.search_filter.is_empty() {
                if ui.button("✕").clicked() {
                    self.search_filter.clear();
                }
            }
        });
        
        ui.add_space(4.0);
        
        let excluded_apps = self.config.read().excluded_apps.clone();
        let filter_lower = self.search_filter.to_lowercase();
        
        // Clone the detected apps to avoid borrow issues
        let apps_snapshot: Vec<_> = self.detected_apps
            .iter()
            .filter(|app| {
                filter_lower.is_empty() ||
                app.process_name.to_lowercase().contains(&filter_lower) ||
                app.display_name.to_lowercase().contains(&filter_lower)
            })
            .cloned()
            .collect();
        
        // Track actions to perform after the UI rendering
        let mut action: Option<(String, u32, bool)> = None; // (name, pid, is_add)
        
        egui::ScrollArea::vertical()
            .id_salt("detected_apps_scroll")
            .max_height(200.0)
            .show(ui, |ui| {
                if apps_snapshot.is_empty() {
                    ui.label(
                        RichText::new("No audio apps detected")
                            .italics()
                            .color(Color32::GRAY)
                    );
                } else {
                    for app in &apps_snapshot {
                        let is_excluded = excluded_apps.contains(&app.process_name.to_lowercase());
                        
                        ui.horizontal(|ui| {
                            // Status indicator
                            let (status_icon, status_color) = if is_excluded {
                                ("✓", Color32::from_rgb(33, 150, 243))
                            } else if app.is_muted_by_us {
                                ("🔇", Color32::from_rgb(244, 67, 54))
                            } else {
                                ("🔊", Color32::from_rgb(76, 175, 80))
                            };
                            
                            ui.label(RichText::new(status_icon).color(status_color));
                            
                            // App name
                            let display = if app.display_name != app.process_name && !app.display_name.is_empty() {
                                format!("{} ({})", app.display_name, app.process_name)
                            } else {
                                app.process_name.clone()
                            };
                            
                            ui.label(&display);
                            
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if is_excluded {
                                    if ui.button("Remove from Exclusions").clicked() {
                                        action = Some((app.process_name.clone(), app.pid, false));
                                    }
                                } else {
                                    if ui.button("➕ Exclude").clicked() {
                                        action = Some((app.process_name.clone(), app.pid, true));
                                    }
                                }
                            });
                        });
                        
                        ui.add_space(2.0);
                    }
                }
            });
        
        // Process any pending action
        if let Some((name, _pid, is_add)) = action {
            if is_add {
                self.config.write().add_excluded_app(&name);
                self.set_status(format!("Added {} to exclusions", name));
            } else {
                self.config.write().remove_excluded_app(&name);
                self.set_status(format!("Removed {} from exclusions", name));
            }
        }
    }

    /// Renders the exclusion list section
    fn render_exclusion_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);
        
        ui.label(RichText::new("📋 Exclusion List").size(18.0).strong());
        ui.label(
            RichText::new("Apps in this list will never be muted")
                .size(12.0)
                .color(Color32::GRAY)
        );
        
        ui.add_space(8.0);
        
        // Manual add input
        ui.horizontal(|ui| {
            ui.label("Add manually:");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.manual_exe_input)
                    .hint_text("e.g., spotify.exe")
                    .desired_width(200.0)
            );
            
            let should_add = ui.button("➕ Add").clicked() ||
                (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            
            if should_add && !self.manual_exe_input.is_empty() {
                let name = self.manual_exe_input.trim().to_string();
                if !name.is_empty() {
                    self.config.write().add_excluded_app(&name);
                    self.set_status(format!("Added {} to exclusions", name));
                    self.manual_exe_input.clear();
                }
            }
        });
        
        ui.add_space(8.0);
        
        let excluded_apps: Vec<_> = self.config.read().excluded_apps.iter().cloned().collect();
        
        egui::ScrollArea::vertical()
            .id_salt("exclusion_list_scroll")
            .max_height(150.0)
            .show(ui, |ui| {
                if excluded_apps.is_empty() {
                    ui.label(
                        RichText::new("No excluded apps")
                            .italics()
                            .color(Color32::GRAY)
                    );
                } else {
                    let mut to_remove = None;
                    
                    for app in &excluded_apps {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("✓").color(Color32::from_rgb(33, 150, 243)));
                            ui.label(app);
                            
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("🗑 Remove").clicked() {
                                    to_remove = Some(app.clone());
                                }
                            });
                        });
                    }
                    
                    if let Some(app) = to_remove {
                        self.config.write().remove_excluded_app(&app);
                        self.set_status(format!("Removed {} from exclusions", app));
                    }
                }
            });
    }

    /// Renders the settings panel
    fn render_settings(&mut self, ui: &mut egui::Ui) {
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);
        
        ui.label(RichText::new("⚙ Settings").size(18.0).strong());
        ui.add_space(8.0);
        
        let mut pending_startup_change: Option<bool> = None;
        let mut pending_status: Option<String> = None;

        {
            let mut config = self.config.write();

            ui.horizontal(|ui| {
                ui.label("Poll interval (ms):");
                let mut interval = config.poll_interval_ms as i32;
                if ui.add(egui::Slider::new(&mut interval, 50..=1000)).changed() {
                    config.poll_interval_ms = interval as u64;
                    let _ = config.save();
                }
            });

            ui.horizontal(|ui| {
                let mut start_minimized = config.start_minimized;
                if ui.checkbox(&mut start_minimized, "Start minimized to tray").changed() {
                    config.start_minimized = start_minimized;
                    let _ = config.save();
                }
            });

            ui.horizontal(|ui| {
                let mut minimize_to_tray = config.minimize_to_tray;
                if ui.checkbox(&mut minimize_to_tray, "Minimize to tray on close (X button)").changed() {
                    config.minimize_to_tray = minimize_to_tray;
                    let _ = config.save();
                }
            });

            ui.horizontal(|ui| {
                let mut minimize_button_to_tray = config.minimize_button_to_tray;
                if ui.checkbox(&mut minimize_button_to_tray, "Minimize to tray on minimize (─ button)").changed() {
                    config.minimize_button_to_tray = minimize_button_to_tray;
                    let _ = config.save();
                }
            });

            ui.horizontal(|ui| {
                let mut start_with_windows = config.start_with_windows;
                if ui.checkbox(&mut start_with_windows, "Run at startup").changed() {
                    pending_startup_change = Some(start_with_windows);
                }
            });
        }

        if let Some(enable) = pending_startup_change {
            match startup::set_run_at_startup(enable) {
                Ok(()) => {
                    let mut config = self.config.write();
                    config.start_with_windows = enable;
                    let _ = config.save();
                }
                Err(e) => {
                    pending_status = Some(format!("Failed to update startup: {}", e));
                }
            }
        }

        if let Some(msg) = pending_status {
            self.set_status(msg);
        }
        
        ui.add_space(8.0);
        
        let mut clear_clicked = false;
        let mut reset_clicked = false;
        
        ui.horizontal(|ui| {
            if ui.button("Clear All Exclusions").clicked() {
                clear_clicked = true;
            }
            
            if ui.button("Reset to Defaults").clicked() {
                reset_clicked = true;
            }
        });
        
        if clear_clicked {
            self.config.write().excluded_apps.clear();
            let _ = self.config.read().save();
            self.set_status("Cleared all exclusions");
        }
        
        if reset_clicked {
            *self.config.write() = Config::default();
            let _ = self.config.read().save();
            self.set_status("Reset to defaults");
        }
    }

    /// Renders the status bar
    fn render_status_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        
        ui.horizontal(|ui| {
            // Show status message or default info
            if let Some((ref msg, time)) = self.status_message {
                if time.elapsed() < Duration::from_secs(3) {
                    ui.label(RichText::new(msg).color(Color32::from_rgb(33, 150, 243)));
                } else {
                    self.status_message = None;
                }
            }
            
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let config = self.config.read();
                let muted_count = if let Some(engine) = self.engine.try_read() {
                    engine.muted_count()
                } else {
                    0
                };
                
                let status = format!(
                    "Apps: {} | Muted: {} | {}",
                    self.detected_apps.len(),
                    muted_count,
                    if config.muting_enabled { "Active" } else { "Disabled" }
                );
                
                ui.label(RichText::new(status).size(12.0).color(Color32::GRAY));
            });
        });
    }
}

impl eframe::App for BackgroundMuterApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // If shutdown was requested (e.g. from the tray callback while hidden), allow the
        // window to close instead of being intercepted by minimize-to-tray logic.
        if self.should_exit.load(Ordering::Relaxed) {
            self.allow_close = true;
        }

        // Auto-refresh
        if self.last_refresh.elapsed() >= self.refresh_interval {
            self.refresh_apps();
        }

        // Ensure we continue receiving events (including tray events) even when minimized/hidden.
        ctx.request_repaint_after(Duration::from_millis(100));

        // Set dark theme
        ctx.set_visuals(Visuals::dark());

        // If tray failed to init in `new`, don't keep retrying every frame.
        if self.tray.is_none() && !self.tray_init_failed {
            // Should be unreachable, but keep it safe.
            self.tray_init_failed = true;
        }

        // Apply initial hidden state once if needed.
        if self.start_hidden && !self.applied_start_hidden {
            self.hide_window(ctx);
            self.applied_start_hidden = true;
        }

        // Process tray events
        self.process_tray(ctx);

        // Handle minimize button: detect transition to minimized state
        let is_minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if is_minimized && !self.was_minimized {
            // Window was just minimized
            let minimize_button_to_tray = self.config.read().minimize_button_to_tray;
            if minimize_button_to_tray && self.tray.is_some() {
                // Hide to tray instead of showing in taskbar as minimized
                self.hide_window(ctx);
            }
        }
        self.was_minimized = is_minimized;

        // Close button (or Alt+F4): check config for minimize to tray behavior
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && !self.allow_close {
            let minimize_to_tray = self.config.read().minimize_to_tray;
            if minimize_to_tray && self.tray.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_window(ctx);
            }
            // If minimize_to_tray is false, allow the close to proceed
        }

        let _ = frame; // keep borrowed for future integrations

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.set_min_width(500.0);
            
            self.render_header(ui);
            self.render_detected_apps(ui);
            self.render_exclusion_list(ui);
            
            if self.show_settings {
                self.render_settings(ui);
            }
            
            self.render_status_bar(ui);
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Ensure we stop the muting thread as early as possible.
        self.should_exit.store(true, Ordering::Relaxed);
        let _ = self.shutdown_tx.try_send(());
        
        // Save config
        let _ = self.config.read().save();
    }
}

/// Native options for the application window
pub fn create_native_options(start_visible: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Background Muter")
            .with_inner_size(Vec2::new(550.0, 600.0))
            .with_min_inner_size(Vec2::new(450.0, 400.0))
            .with_visible(start_visible)
            .with_icon(load_app_icon()),
        centered: true,
        ..Default::default()
    }
}

/// Loads the application icon from the embedded PNG
fn load_app_icon() -> egui::IconData {
    match image::load_from_memory(ICON_BYTES) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            }
        }
        Err(_) => {
            // Fallback: simple green square
            let size = 64u32;
            let rgba = vec![76u8, 175, 80, 255].repeat((size * size) as usize);
            egui::IconData {
                rgba,
                width: size,
                height: size,
            }
        }
    }
}
