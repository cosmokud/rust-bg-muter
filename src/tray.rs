//! Lightweight system tray module
//!
//! Pure Win32-based tray icon with minimal resource usage.
//! No dependency on eframe/egui - uses native Windows message pump.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

/// Embedded icon bytes
const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.png");

/// Commands from tray interactions
#[derive(Debug, Clone, Copy)]
pub enum TrayCommand {
    ToggleMuting,
    OpenSettings,
    Exit,
}

/// Lightweight system tray manager
pub struct SystemTray {
    _tray_icon: TrayIcon,
    menu_toggle: MenuItem,
    command_rx: Receiver<TrayCommand>,
    last_muting_state: bool,
    exit_flag: Arc<AtomicBool>,
}

impl SystemTray {
    /// Creates a new system tray instance
    pub fn new(muting_enabled: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let icon = load_tray_icon()?;

        // Create menu items
        let toggle_text = if muting_enabled {
            "Disable Muting"
        } else {
            "Enable Muting"
        };

        let menu_toggle = MenuItem::new(toggle_text, true, None);
        let menu_settings = MenuItem::new("Settings...", true, None);
        let menu_separator = PredefinedMenuItem::separator();
        let menu_exit = MenuItem::new("Exit", true, None);

        let menu = Menu::new();
        menu.append(&menu_toggle)?;
        menu.append(&menu_settings)?;
        menu.append(&menu_separator)?;
        menu.append(&menu_exit)?;

        // Set up command channel
        let (command_tx, command_rx) = unbounded();

        let toggle_id = menu_toggle.id().clone();
        let settings_id = menu_settings.id().clone();
        let exit_id = menu_exit.id().clone();

        let exit_flag = Arc::new(AtomicBool::new(false));
        let exit_flag_clone = exit_flag.clone();
        let tx = command_tx.clone();

        // Menu event handler
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let cmd = if event.id == toggle_id {
                Some(TrayCommand::ToggleMuting)
            } else if event.id == settings_id {
                Some(TrayCommand::OpenSettings)
            } else if event.id == exit_id {
                exit_flag_clone.store(true, Ordering::SeqCst);
                Some(TrayCommand::Exit)
            } else {
                None
            };

            if let Some(cmd) = cmd {
                let _ = tx.try_send(cmd);
            }
        }));

        // Tray icon event handler for double-click
        let tx_tray = command_tx.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            // Only handle double-click with left mouse button
            if let TrayIconEvent::DoubleClick { button, .. } = event {
                if button == MouseButton::Left {
                    let _ = tx_tray.try_send(TrayCommand::OpenSettings);
                }
            }
        }));

        // Build tray icon
        let tooltip = if muting_enabled {
            "Background Muter - Active"
        } else {
            "Background Muter - Disabled"
        };

        // Do NOT show menu on left click - only right click shows context menu
        let tray_icon = TrayIconBuilder::new()
            .with_tooltip(tooltip)
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)  // Only right-click shows menu
            .build()?;

        Ok(Self {
            _tray_icon: tray_icon,
            menu_toggle,
            command_rx,
            last_muting_state: muting_enabled,
            exit_flag,
        })
    }

    /// Updates the tray state (tooltip and menu text)
    pub fn update_state(&mut self, muting_enabled: bool) {
        if self.last_muting_state == muting_enabled {
            return;
        }
        self.last_muting_state = muting_enabled;

        let toggle_text = if muting_enabled {
            "Disable Muting"
        } else {
            "Enable Muting"
        };
        let _ = self.menu_toggle.set_text(toggle_text);

        let tooltip = if muting_enabled {
            "Background Muter - Active"
        } else {
            "Background Muter - Disabled"
        };
        let _ = self._tray_icon.set_tooltip(Some(tooltip));
    }

    /// Polls for a command (non-blocking)
    pub fn poll_command(&self) -> Option<TrayCommand> {
        self.command_rx.try_recv().ok()
    }

    /// Pumps Windows messages with timeout
    /// Returns false if exit was requested
    pub fn pump_messages(&self, timeout: Duration) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage,
            MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT,
        };

        // Check exit flag first
        if self.exit_flag.load(Ordering::Relaxed) {
            return false;
        }

        unsafe {
            // Wait for messages with timeout (efficient - doesn't spin CPU)
            let timeout_ms = timeout.as_millis() as u32;
            let _result = MsgWaitForMultipleObjectsEx(
                None,
                timeout_ms,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            );

            // Process all pending messages
            let mut msg = std::mem::zeroed();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        !self.exit_flag.load(Ordering::Relaxed)
    }
}

impl Drop for SystemTray {
    fn drop(&mut self) {
        // Clear event handlers
        MenuEvent::set_event_handler(Some(|_: MenuEvent| {}));
        TrayIconEvent::set_event_handler(Some(|_: TrayIconEvent| {}));
    }
}

/// Loads the tray icon from embedded PNG
fn load_tray_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    let img = image::load_from_memory(ICON_BYTES)?
        .resize(32, 32, image::imageops::FilterType::Triangle) // Faster than Lanczos
        .to_rgba8();
    let (width, height) = img.dimensions();
    let rgba = img.into_raw();
    Ok(Icon::from_rgba(rgba, width, height)?)
}
