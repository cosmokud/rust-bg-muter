//! GUI module using egui
//! Implements the main application window

use crate::config::Config;
use crate::muter::{AppAudioState, MuterEngine};
use crate::startup;
use eframe::egui::{self, Color32, RichText, Vec2, Visuals};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Application state for the GUI
pub struct BackgroundMuterApp {
    config: Arc<RwLock<Config>>,
    engine: Arc<RwLock<MuterEngine>>,
    detected_apps: Vec<AppAudioState>,
    manual_exe_input: String,
    last_refresh: Instant,
    refresh_interval: Duration,
    show_settings: bool,
    status_message: Option<(String, Instant)>,
    search_filter: String,
}

impl BackgroundMuterApp {
    /// Creates a new application instance
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        config: Arc<RwLock<Config>>,
        engine: Arc<RwLock<MuterEngine>>,
    ) -> Self {
        Self {
            config,
            engine,
            detected_apps: Vec::new(),
            manual_exe_input: String::new(),
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_millis(500),
            show_settings: false,
            status_message: None,
            search_filter: String::new(),
        }
    }

    /// Refreshes the detected apps list
    fn refresh_apps(&mut self) {
        if let Some(mut engine) = self.engine.try_write() {
            let _ = engine.update();
            self.detected_apps = engine.get_active_sessions();
        } else if let Some(engine) = self.engine.try_read() {
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
                    let should_unmute = !config.muting_enabled;
                    drop(config);
                    self.set_status(status);
                    
                    // Unmute all if disabled
                    if should_unmute {
                        if let Some(mut engine) = self.engine.try_write() {
                            engine.unmute_all();
                        }
                    }
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
        if let Some((name, pid, is_add)) = action {
            if is_add {
                self.config.write().add_excluded_app(&name);
                self.set_status(format!("Added {} to exclusions", name));
                
                // Immediately unmute this app
                if let Some(engine) = self.engine.try_read() {
                    let _ = engine.audio_manager().unmute_process(pid);
                }
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Auto-refresh
        if self.last_refresh.elapsed() >= self.refresh_interval {
            self.refresh_apps();
        }

        // Request repaint for animation
        ctx.request_repaint_after(Duration::from_millis(100));

        // Set dark theme
        ctx.set_visuals(Visuals::dark());

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
        // Unmute all on exit
        if let Some(mut engine) = self.engine.try_write() {
            engine.unmute_all();
        }
        
        // Save config
        let _ = self.config.read().save();
    }
}

/// Native options for the application window
pub fn create_native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Background Muter")
            .with_inner_size(Vec2::new(550.0, 600.0))
            .with_min_inner_size(Vec2::new(450.0, 400.0))
            .with_icon(create_app_icon()),
        centered: true,
        ..Default::default()
    }
}

/// Creates the application icon
fn create_app_icon() -> egui::IconData {
    let size = 64u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    
    let center = size as f32 / 2.0;
    let radius = (size as f32 / 2.0) - 4.0;
    
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            
            let idx = ((y * size + x) * 4) as usize;
            
            if dist <= radius {
                // Green circle
                rgba[idx] = 76;      // R
                rgba[idx + 1] = 175; // G
                rgba[idx + 2] = 80;  // B
                rgba[idx + 3] = 255; // A
            } else if dist <= radius + 2.0 {
                // Anti-aliased edge
                let alpha = ((radius + 2.0 - dist) * 127.0) as u8;
                rgba[idx] = 76;
                rgba[idx + 1] = 175;
                rgba[idx + 2] = 80;
                rgba[idx + 3] = alpha;
            }
        }
    }
    
    egui::IconData {
        rgba,
        width: size,
        height: size,
    }
}
