//! System tray module
//! Handles system tray icon and interactions

#![allow(dead_code)]

use crossbeam_channel::{bounded, Receiver, Sender};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, MenuId},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use once_cell::sync::OnceCell;

/// Embedded icon bytes (icon.png from assets folder)
const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.png");

/// Global storage for menu item IDs so the event handler can access them
static MENU_IDS: OnceCell<MenuIds> = OnceCell::new();
/// Global event sender for tray events
static EVENT_SENDER: OnceCell<Sender<TrayEvent>> = OnceCell::new();

struct MenuIds {
    open_id: MenuId,
    toggle_id: MenuId,
    exit_id: MenuId,
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
        let (event_sender, event_receiver) = bounded(100);

        // Store the sender globally so event handlers can use it
        let _ = EVENT_SENDER.set(event_sender);

        Ok(Self {
            tray_icon: None,
            event_receiver,
            menu_toggle: None,
            last_muting_enabled: None,
        })
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

        // Store menu IDs globally for the event handler
        let _ = MENU_IDS.set(MenuIds {
            open_id: menu_open.id().clone(),
            toggle_id: menu_toggle.id().clone(),
            exit_id: menu_exit.id().clone(),
        });

        self.menu_toggle = Some(menu_toggle.clone());
        self.last_muting_enabled = Some(muting_enabled);

        // Set up the menu event handler - this is called synchronously when menu items are clicked
        // even during the modal menu loop on Windows
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let (Some(ids), Some(sender)) = (MENU_IDS.get(), EVENT_SENDER.get()) {
                let tray_event = if event.id == ids.open_id {
                    Some(TrayEvent::OpenWindow)
                } else if event.id == ids.toggle_id {
                    Some(TrayEvent::ToggleMuting)
                } else if event.id == ids.exit_id {
                    Some(TrayEvent::Exit)
                } else {
                    None
                };

                if let Some(ev) = tray_event {
                    let _ = sender.try_send(ev);
                }
            }
        }));

        // Set up tray icon click handler
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let Some(sender) = EVENT_SENDER.get() {
                match event {
                    TrayIconEvent::DoubleClick { .. } => {
                        let _ = sender.try_send(TrayEvent::OpenWindow);
                    }
                    TrayIconEvent::Click { .. } => {
                        let _ = sender.try_send(TrayEvent::SingleClick);
                    }
                    _ => {}
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
        // Clear the event handlers by setting them to a no-op
        MenuEvent::set_event_handler(Some(|_: MenuEvent| {}));
        TrayIconEvent::set_event_handler(Some(|_: TrayIconEvent| {}));
    }
}
