//! System tray module
//! Handles system tray icon and interactions

#![allow(dead_code)]

use crossbeam_channel::{bounded, Receiver, Sender};
use image::{ImageBuffer, Rgba};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

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
    event_sender: Sender<TrayEvent>,
    menu_open_id: Option<tray_icon::menu::MenuId>,
    menu_toggle: Option<MenuItem>,
    menu_toggle_id: Option<tray_icon::menu::MenuId>,
    menu_exit_id: Option<tray_icon::menu::MenuId>,
    last_muting_enabled: Option<bool>,
}

impl SystemTray {
    /// Creates a new system tray instance
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (event_sender, event_receiver) = bounded(100);

        Ok(Self {
            tray_icon: None,
            event_receiver,
            event_sender,
            menu_open_id: None,
            menu_toggle: None,
            menu_toggle_id: None,
            menu_exit_id: None,
            last_muting_enabled: None,
        })
    }

    /// Initializes the tray icon (must be called from main thread)
    pub fn initialize(&mut self, muting_enabled: bool) -> Result<(), Box<dyn std::error::Error>> {
        // Create the tray icon image (a simple colored square)
        let icon = create_tray_icon(muting_enabled)?;

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

        self.menu_open_id = Some(menu_open.id().clone());
        self.menu_toggle_id = Some(menu_toggle.id().clone());
        self.menu_exit_id = Some(menu_exit.id().clone());
        self.menu_toggle = Some(menu_toggle.clone());
        self.last_muting_enabled = Some(muting_enabled);

        // Build the tray icon
        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("Background Muter")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()?;

        self.tray_icon = Some(tray_icon);

        Ok(())
    }

    /// Returns a receiver for tray events
    pub fn event_receiver(&self) -> Receiver<TrayEvent> {
        self.event_receiver.clone()
    }

    /// Returns the event sender for external use
    pub fn event_sender(&self) -> Sender<TrayEvent> {
        self.event_sender.clone()
    }

    /// Updates the tray icon based on muting state
    pub fn update_icon(&mut self, muting_enabled: bool) -> Result<(), Box<dyn std::error::Error>> {
        if self.last_muting_enabled == Some(muting_enabled) {
            return Ok(());
        }
        self.last_muting_enabled = Some(muting_enabled);

        if let Some(ref tray) = self.tray_icon {
            let icon = create_tray_icon(muting_enabled)?;
            tray.set_icon(Some(icon))?;

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

        // Process tray icon click events
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                TrayIconEvent::DoubleClick { .. } => {
                    events.push(TrayEvent::OpenWindow);
                }
                TrayIconEvent::Click { .. } => {
                    events.push(TrayEvent::SingleClick);
                }
                _ => {}
            }
        }

        // Process menu events
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if self.menu_open_id.as_ref() == Some(&event.id) {
                events.push(TrayEvent::OpenWindow);
            } else if self.menu_toggle_id.as_ref() == Some(&event.id) {
                events.push(TrayEvent::ToggleMuting);
            } else if self.menu_exit_id.as_ref() == Some(&event.id) {
                events.push(TrayEvent::Exit);
            } else {
                log::debug!("Unknown menu event: {:?}", event.id);
            }
        }

        events
    }
}

/// Creates a tray icon image
fn create_tray_icon(muting_enabled: bool) -> Result<Icon, Box<dyn std::error::Error>> {
    let size = 32u32;
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(size, size);

    // Choose color based on state
    let (primary, secondary) = if muting_enabled {
        // Green when active
        (Rgba([76u8, 175, 80, 255]), Rgba([56u8, 142, 60, 255]))
    } else {
        // Red when disabled
        (Rgba([244u8, 67, 54, 255]), Rgba([198u8, 40, 40, 255]))
    };

    let center = size as f32 / 2.0;
    let outer_radius = (size as f32 / 2.0) - 2.0;
    let inner_radius = outer_radius - 4.0;

    // Draw the icon
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= outer_radius {
                if dist <= inner_radius {
                    // Inner circle
                    img.put_pixel(x, y, secondary);
                } else {
                    // Outer ring
                    img.put_pixel(x, y, primary);
                }

                // Draw a speaker icon in the center
                let rel_x = (x as f32 - center) / inner_radius;
                let rel_y = (y as f32 - center) / inner_radius;

                // Simple speaker shape
                if rel_x.abs() < 0.3 && rel_y.abs() < 0.25 {
                    img.put_pixel(x, y, Rgba([255u8, 255, 255, 255]));
                }
                // Speaker cone
                if rel_x > 0.1 && rel_x < 0.5 && rel_y.abs() < (rel_x - 0.1) * 1.2 {
                    img.put_pixel(x, y, Rgba([255u8, 255, 255, 255]));
                }

                // Draw X over speaker when muting is active
                if muting_enabled {
                    let line_width = 0.12;
                    // Diagonal line 1
                    if (rel_x - rel_y).abs() < line_width && rel_x.abs() < 0.6 {
                        img.put_pixel(x, y, Rgba([255u8, 0, 0, 255]));
                    }
                    // Diagonal line 2
                    if (rel_x + rel_y).abs() < line_width && rel_x.abs() < 0.6 {
                        img.put_pixel(x, y, Rgba([255u8, 0, 0, 255]));
                    }
                }
            } else {
                // Transparent background
                img.put_pixel(x, y, Rgba([0u8, 0, 0, 0]));
            }
        }
    }

    let rgba = img.into_raw();
    let icon = Icon::from_rgba(rgba, size, size)?;

    Ok(icon)
}

impl Drop for SystemTray {
    fn drop(&mut self) {
        // Tray icon will be cleaned up automatically
    }
}
