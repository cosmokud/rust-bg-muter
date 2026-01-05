//! System tray module
//! Handles system tray icon and interactions

#![allow(dead_code)]

use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::Mutex;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, MenuId},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

/// Embedded icon bytes (icon.png from assets folder)
const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.png");

/// Global state shared with event handlers
static GLOBAL_STATE: Mutex<Option<TrayGlobalState>> = Mutex::new(None);

struct TrayGlobalState {
    sender: Sender<TrayEvent>,
    open_id: MenuId,
    toggle_id: MenuId,
    exit_id: MenuId,
    ctx: Option<egui::Context>,
}

/// Events sent from the system tray
#[derive(Debug, Clone)]
pub enum TrayEvent {
    /// User double-clicked the tray icon
    OpenWindow,
    /// User selected "Toggle Muting" from menu
    ToggleMuting,
    /// User selected "Exit" from menu
    Exit,
    /// User single-clicked the tray icon
    SingleClick,
}

/// System tray manager
pub struct SystemTray {
    tray_icon: Option<TrayIcon>,
    event_receiver: Receiver<TrayEvent>,
    menu_toggle: Option<MenuItem>,
    last_muting_enabled: Option<bool>,
}

impl SystemTray {
    /// Creates a new system tray instance
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (event_sender, event_receiver) = unbounded();

        // Store the sender globally - use a mutex so we can replace it
        {
            let mut global = GLOBAL_STATE.lock();
            *global = Some(TrayGlobalState {
                sender: event_sender,
                open_id: MenuId::new(""),
                toggle_id: MenuId::new(""),
                exit_id: MenuId::new(""),
                ctx: None,
            });
        }

        Ok(Self {
            tray_icon: None,
            event_receiver,
            menu_toggle: None,
            last_muting_enabled: None,
        })
    }

    /// Sets the egui context for waking up the event loop
    pub fn set_egui_context(&self, ctx: egui::Context) {
        let mut global = GLOBAL_STATE.lock();
        if let Some(ref mut state) = *global {
            state.ctx = Some(ctx);
        }
    }

    /// Initializes the tray icon (must be called from main thread)
    pub fn initialize(&mut self, muting_enabled: bool) -> Result<(), Box<dyn std::error::Error>> {
        // Load the icon from embedded PNG
        let icon = load_tray_icon()?;

        // Create context menu
        let toggle_text = if muting_enabled {
            "Disable Muting"
        } else {
            "Enable Muting"
        };

        let menu_toggle = MenuItem::new(toggle_text, true, None);
        let menu_open = MenuItem::new("Open Window", true, None);
        let menu_separator = PredefinedMenuItem::separator();
        let menu_exit = MenuItem::new("Exit", true, None);

        let menu = Menu::new();
        menu.append(&menu_open)?;
        menu.append(&menu_toggle)?;
        menu.append(&menu_separator)?;
        menu.append(&menu_exit)?;

        // Store menu IDs globally
        {
            let mut global = GLOBAL_STATE.lock();
            if let Some(ref mut state) = *global {
                state.open_id = menu_open.id().clone();
                state.toggle_id = menu_toggle.id().clone();
                state.exit_id = menu_exit.id().clone();
            }
        }

        self.menu_toggle = Some(menu_toggle.clone());
        self.last_muting_enabled = Some(muting_enabled);

        // Set up the menu event handler - called synchronously even during modal menu
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let global = GLOBAL_STATE.lock();
            if let Some(ref state) = *global {
                let tray_event = if event.id == state.open_id {
                    Some(TrayEvent::OpenWindow)
                } else if event.id == state.toggle_id {
                    Some(TrayEvent::ToggleMuting)
                } else if event.id == state.exit_id {
                    Some(TrayEvent::Exit)
                } else {
                    None
                };

                if let Some(ev) = tray_event {
                    let _ = state.sender.send(ev);
                    // Wake up the egui event loop
                    if let Some(ref ctx) = state.ctx {
                        ctx.request_repaint();
                    }
                }
            }
        }));

        // Set up tray icon click handler
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let global = GLOBAL_STATE.lock();
            if let Some(ref state) = *global {
                let tray_event = match event {
                    TrayIconEvent::DoubleClick { .. } => Some(TrayEvent::OpenWindow),
                    TrayIconEvent::Click { .. } => Some(TrayEvent::SingleClick),
                    _ => None,
                };

                if let Some(ev) = tray_event {
                    let _ = state.sender.send(ev);
                    // Wake up the egui event loop
                    if let Some(ref ctx) = state.ctx {
                        ctx.request_repaint();
                    }
                }
            }
        }));

        // Build the tray icon
        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("Background Muter")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()?;

        self.tray_icon = Some(tray_icon);

        Ok(())
    }

    /// Returns a clone of the event receiver
    pub fn event_receiver(&self) -> Receiver<TrayEvent> {
        self.event_receiver.clone()
    }

    /// Updates the tray tooltip and menu text based on muting state
    pub fn update_icon(&mut self, muting_enabled: bool) -> Result<(), Box<dyn std::error::Error>> {
        if self.last_muting_enabled == Some(muting_enabled) {
            return Ok(());
        }
        self.last_muting_enabled = Some(muting_enabled);

        if let Some(ref tray) = self.tray_icon {
            let tooltip = if muting_enabled {
                "Background Muter - Active"
            } else {
                "Background Muter - Disabled"
            };
            tray.set_tooltip(Some(tooltip))?;
        }

        if let Some(ref menu_toggle) = self.menu_toggle {
            let toggle_text = if muting_enabled {
                "Disable Muting"
            } else {
                "Enable Muting"
            };
            let _ = menu_toggle.set_text(toggle_text);
        }

        Ok(())
    }

    /// Processes tray icon events (call from event loop)
    pub fn process_events(&self) -> Vec<TrayEvent> {
        let mut events = Vec::new();

        // Drain all events from the channel
        while let Ok(event) = self.event_receiver.try_recv() {
            events.push(event);
        }

        events
    }
}

/// Loads the tray icon from the embedded PNG
fn load_tray_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    let img = image::load_from_memory(ICON_BYTES)?
        .resize(32, 32, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let (width, height) = img.dimensions();
    let rgba = img.into_raw();
    let icon = Icon::from_rgba(rgba, width, height)?;
    Ok(icon)
}

impl Drop for SystemTray {
    fn drop(&mut self) {
        // Clear event handlers
        MenuEvent::set_event_handler(Some(|_: MenuEvent| {}));
        TrayIconEvent::set_event_handler(Some(|_: TrayIconEvent| {}));
        // Clear global state
        *GLOBAL_STATE.lock() = None;
    }
}
