//! System tray module
//! Handles system tray icon and interactions.
//!
//! Windows tray context menus are modal; the GUI event loop may not tick while the menu is open.
//! Therefore, "Open Window" must not rely on egui/eframe processing. We show the window directly
//! via Win32 from the tray callback.

#![allow(dead_code)]

use crossbeam_channel::{unbounded, Receiver, Sender, TrySendError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM, FALSE, TRUE};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetWindow, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, SetWindowPos,
    ShowWindow, GW_OWNER, HWND_TOP, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SW_RESTORE, SW_SHOW, SW_SHOWNORMAL, PostMessageW, WM_CLOSE,
};

/// Embedded icon bytes (icon.png from assets folder)
const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.png");

/// Events sent from the system tray
#[derive(Debug, Clone)]
pub enum TrayEvent {
    /// User selected "Open Window" from the menu or double-clicked the tray icon
    OpenWindow,
    /// User selected "Toggle Muting" from menu
    ToggleMuting,
    /// User selected "Exit" from menu
    Exit,
}

/// System tray manager
pub struct SystemTray {
    tray_icon: Option<TrayIcon>,
    sender: Sender<TrayEvent>,
    event_receiver: Receiver<TrayEvent>,
    menu_toggle: Option<MenuItem>,
    last_muting_enabled: Option<bool>,
}

impl SystemTray {
    /// Creates a new system tray instance
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (sender, event_receiver) = unbounded();

        Ok(Self {
            tray_icon: None,
            sender,
            event_receiver,
            menu_toggle: None,
            last_muting_enabled: None,
        })
    }

    /// Initializes the tray icon (must be called from main thread)
    pub fn initialize(
        &mut self,
        muting_enabled: bool,
        should_exit: Arc<AtomicBool>,
        shutdown_tx: Sender<()>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let icon = load_tray_icon()?;

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

        let open_id = menu_open.id().clone();
        let toggle_id = menu_toggle.id().clone();
        let exit_id = menu_exit.id().clone();
        let sender = self.sender.clone();
        let should_exit = should_exit.clone();
        let shutdown_tx = shutdown_tx.clone();

        // Menu callback: keep this lightweight. Crucially, Open Window uses Win32 directly.
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == open_id {
                show_main_window_native();
                let _ = sender.try_send(TrayEvent::OpenWindow);
                return;
            }

            // Exit must work even if the main window is currently hidden and the GUI loop
            // isn't producing frames. We request shutdown and also post a WM_CLOSE to the
            // main window so eframe/winit exits promptly.
            if event.id == exit_id {
                should_exit.store(true, Ordering::Relaxed);
                let _ = shutdown_tx.try_send(());
                request_app_exit_native();
            }

            let ev = if event.id == toggle_id {
                Some(TrayEvent::ToggleMuting)
            } else if event.id == exit_id {
                Some(TrayEvent::Exit)
            } else {
                None
            };

            if let Some(ev) = ev {
                match sender.try_send(ev) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => {}
                }
            }
        }));

        // Tray icon callback: ONLY double-click opens the window. Single-click does nothing.
        let sender = self.sender.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                show_main_window_native();
                let _ = sender.try_send(TrayEvent::OpenWindow);
            }
        }));

        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("Background Muter")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        self.menu_toggle = Some(menu_toggle);
        self.last_muting_enabled = Some(muting_enabled);
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

    /// Processes tray icon events (call from GUI event loop when available)
    pub fn process_events(&self) -> Vec<TrayEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_receiver.try_recv() {
            events.push(event);
        }
        events
    }
}

/// Requests application shutdown by posting WM_CLOSE to the main window for this process.
///
/// This is intentionally independent of egui/eframe and works even when the window is hidden.
fn request_app_exit_native() {
    #[repr(C)]
    struct EnumCtx {
        target_pid: u32,
        posted: bool,
    }

    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut EnumCtx);

        let mut window_pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut window_pid));
        if window_pid != ctx.target_pid {
            return TRUE;
        }

        // Prefer true top-level windows.
        let owner = GetWindow(hwnd, GW_OWNER).unwrap_or(HWND(std::ptr::null_mut()));
        if !owner.0.is_null() {
            return TRUE;
        }

        let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        ctx.posted = true;
        FALSE
    }

    let mut ctx = Box::new(EnumCtx {
        target_pid: std::process::id(),
        posted: false,
    });

    let ptr = (&mut *ctx) as *mut EnumCtx;
    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(ptr as isize));
    }
}

impl Drop for SystemTray {
    fn drop(&mut self) {
        // Clear global handlers
        MenuEvent::set_event_handler(Some(|_: MenuEvent| {}));
        TrayIconEvent::set_event_handler(Some(|_: TrayIconEvent| {}));
    }
}

/// Loads the tray icon from the embedded PNG
fn load_tray_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    let img = image::load_from_memory(ICON_BYTES)?
        .resize(32, 32, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let (width, height) = img.dimensions();
    let rgba = img.into_raw();
    Ok(Icon::from_rgba(rgba, width, height)?)
}

/// Shows the main window using Win32 APIs.
///
/// This is intentionally independent of egui/eframe so it works even while the tray menu is open
/// and the GUI event loop is not ticking.
///
/// When the window is hidden via egui's ViewportCommand::Visible(false), it sets the window
/// to be invisible but doesn't minimize it. We need to use SetWindowPos with SWP_SHOWWINDOW
/// combined with ShowWindow to properly restore such windows.
fn show_main_window_native() {
    #[repr(i32)]
    enum Mode {
        PreferExactTitle = 0,
        PreferAnyTitledWindow = 1,
        AnyTopLevelWindow = 2,
    }

    #[repr(C)]
    struct EnumCtx {
        target_pid: u32,
        mode: Mode,
        found: bool,
    }

    fn enum_and_show(target_pid: u32, mode: Mode) -> bool {
        unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let ctx = &mut *(lparam.0 as *mut EnumCtx);

            let mut window_pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut window_pid));
            if window_pid != ctx.target_pid {
                return TRUE;
            }

            // Prefer true top-level windows in the first passes.
            // In the final fallback pass, accept owned windows too (some frameworks create
            // owned popups for the main surface in certain configurations).
            let owner = GetWindow(hwnd, GW_OWNER).unwrap_or(HWND(std::ptr::null_mut()));
            if !owner.0.is_null() && !matches!(ctx.mode, Mode::AnyTopLevelWindow) {
                return TRUE;
            }

            let title_len = GetWindowTextLengthW(hwnd);
            let mut title_matches = false;
            let mut has_title = title_len > 0;

            if title_len > 0 {
                let mut buf = vec![0u16; (title_len as usize) + 1];
                let copied = GetWindowTextW(hwnd, &mut buf);
                if copied > 0 {
                    let title = String::from_utf16_lossy(&buf[..copied as usize]);
                    let title_lower = title.to_lowercase();
                    title_matches = title_lower.contains("background muter")
                        || title_lower.contains("rust-bg-muter")
                        || title_lower.contains("rust bg muter");
                } else {
                    has_title = false;
                }
            }

            match ctx.mode {
                Mode::PreferExactTitle => {
                    if !title_matches {
                        return TRUE;
                    }
                }
                Mode::PreferAnyTitledWindow => {
                    if !has_title {
                        return TRUE;
                    }
                }
                Mode::AnyTopLevelWindow => {
                    // accept
                }
            }

            // Robust restore/show sequence. This works for both minimized windows and
            // windows hidden via winit/egui visibility.
            let is_visible = IsWindowVisible(hwnd).as_bool();
            if !is_visible {
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
            }

            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);

            ctx.found = true;
            FALSE
        }

        let mut ctx = Box::new(EnumCtx {
            target_pid,
            mode,
            found: false,
        });

        let ptr = (&mut *ctx) as *mut EnumCtx;
        unsafe {
            let _ = EnumWindows(Some(enum_cb), LPARAM(ptr as isize));
        }
        ctx.found
    }

    let target_pid = std::process::id();

    // Pass 1: exact-ish title match (best case).
    if enum_and_show(target_pid, Mode::PreferExactTitle) {
        return;
    }

    // Pass 2: any titled top-level window for this process.
    if enum_and_show(target_pid, Mode::PreferAnyTitledWindow) {
        return;
    }

    // Pass 3: absolutely any top-level window for this process.
    let _ = enum_and_show(target_pid, Mode::AnyTopLevelWindow);
}
