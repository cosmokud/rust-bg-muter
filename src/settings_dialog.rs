//! Native Win32 Settings Dialog
//!
//! A lightweight settings window using pure Win32 API with Windows 11 visual styles.
//! Uses GDI rendering (CPU-based) - zero GPU/VRAM usage.

use crate::audio::AudioManager;
use crate::config::Config;
use crate::startup;
use parking_lot::RwLock;
use std::cell::RefCell;
use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, HBRUSH, HFONT, HGDIOBJ, InvalidateRect,
    DEFAULT_CHARSET, OUT_TT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
    DEFAULT_PITCH, FF_DONTCARE, FW_NORMAL, COLOR_BTNFACE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{InitCommonControlsEx, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX};
use windows::Win32::UI::WindowsAndMessaging::*;

// Control IDs
const ID_LIST_DETECTED: i32 = 101;
const ID_LIST_EXCLUDED: i32 = 102;
const ID_LIST_ALWAYS_MUTED: i32 = 112;
const ID_BTN_ADD_EXCLUSION: i32 = 103;
const ID_BTN_REMOVE_EXCLUSION: i32 = 104;
const ID_BTN_ADD_ALWAYS_MUTED: i32 = 115;
const ID_BTN_REMOVE_ALWAYS_MUTED: i32 = 116;
const ID_BTN_REFRESH: i32 = 105;
const ID_CHECK_ENABLED: i32 = 106;
const ID_CHECK_START_MINIMIZED: i32 = 107;
const ID_CHECK_START_WINDOWS: i32 = 108;
const ID_EDIT_POLL_INTERVAL: i32 = 109;
const ID_BTN_SAVE_CLOSE: i32 = 110;
const ID_BTN_CLOSE: i32 = 111;
const ID_BTN_SAVE_ONLY: i32 = 126;
const ID_GROUP_SETTINGS: i32 = 113;
const ID_LABEL_POLL: i32 = 114;
const ID_EDIT_SEARCH: i32 = 117;
const ID_GRP_DETECTED: i32 = 118;
const ID_GRP_EXCLUDED: i32 = 119;
const ID_GRP_ALWAYS_MUTED: i32 = 120;
const ID_LABEL_SEARCH: i32 = 121;
const ID_LABEL_MS: i32 = 122;
const ID_LABEL_RANGE: i32 = 123;
const ID_LABEL_CONFIG: i32 = 124;
const ID_LABEL_PATH: i32 = 125;

// Edit notification
const EN_CHANGE: u16 = 0x0300;

// Window dimensions (wider so process name + PID fits without truncation)
const WINDOW_WIDTH: i32 = 920;
const WINDOW_HEIGHT: i32 = 680;
const MIN_WINDOW_WIDTH: i32 = 820;
const MIN_WINDOW_HEIGHT: i32 = 600;

// Button states
const BST_CHECKED: usize = 1;
const BST_UNCHECKED: usize = 0;

// Thread-local storage for dialog state and font
thread_local! {
    static DIALOG_STATE: RefCell<Option<DialogState>> = const { RefCell::new(None) };
    static UI_FONT: RefCell<Option<HFONT>> = const { RefCell::new(None) };
}

struct DialogState {
    config: Arc<RwLock<Config>>,
    muting_enabled: Arc<AtomicBool>,
    audio_manager: Arc<AudioManager>,
    detected_apps: Vec<(u32, String)>, // (pid, name)
    all_detected_apps: Vec<(u32, String)>, // All apps before filtering
    search_filter: String,
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn to_wide_ptr(s: &str) -> PCWSTR {
    // This leaks memory, only use for static strings
    let wide: Vec<u16> = to_wide(s);
    PCWSTR(Box::leak(wide.into_boxed_slice()).as_ptr())
}

/// Helper to get dialog item handle, returns default HWND on error
unsafe fn get_dlg_item(hwnd: HWND, id: i32) -> HWND {
    GetDlgItem(hwnd, id).unwrap_or_default()
}

/// Create the Segoe UI font for modern Windows look
unsafe fn create_ui_font() -> HFONT {
    CreateFontW(
        -14,                    // Height (negative = character height)
        0,                      // Width (0 = default)
        0,                      // Escapement
        0,                      // Orientation
        FW_NORMAL.0 as i32,     // Weight
        0,                      // Italic
        0,                      // Underline
        0,                      // StrikeOut
        DEFAULT_CHARSET.0 as u32,
        OUT_TT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
        to_wide_ptr("Segoe UI"),
    )
}

/// Initialize common controls for Windows 11 visual styles
fn init_common_controls() {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }
}

/// Opens the settings dialog (blocks until closed)
pub fn open_settings_dialog(
    config: Arc<RwLock<Config>>,
    muting_enabled: Arc<AtomicBool>,
) {
    // Create audio manager for detecting apps
    let audio_manager = match AudioManager::new() {
        Ok(am) => Arc::new(am),
        Err(e) => {
            log::error!("Failed to create audio manager for settings: {}", e);
            return;
        }
    };

    // Store state in thread-local
    DIALOG_STATE.with(|state| {
        *state.borrow_mut() = Some(DialogState {
            config,
            muting_enabled,
            audio_manager,
            detected_apps: Vec::new(),
            all_detected_apps: Vec::new(),
            search_filter: String::new(),
        });
    });

    // Initialize common controls for visual styles
    init_common_controls();

    unsafe {
        // Create the UI font
        let font = create_ui_font();
        UI_FONT.with(|f| *f.borrow_mut() = Some(font));

        // Register window class
        let class_name = to_wide("BgMuterSettingsClass");
        let hmodule = GetModuleHandleW(None).unwrap_or_default();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hmodule.into(),
            hIcon: HICON::default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as *mut c_void), // System button face color
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            hIconSm: HICON::default(),
        };

        RegisterClassExW(&wc);

        // Calculate centered position
        let screen_width = GetSystemMetrics(SM_CXSCREEN);
        let screen_height = GetSystemMetrics(SM_CYSCREEN);
        let x = (screen_width - WINDOW_WIDTH) / 2;
        let y = (screen_height - WINDOW_HEIGHT) / 2;

        // Create window (resizable)
        let hwnd_result = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_TOPMOST,
            PCWSTR(class_name.as_ptr()),
            to_wide_ptr("Background Muter - Settings"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX,
            x,
            y,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            None,
            None,
            hmodule,
            None,
        );

        let hwnd = match hwnd_result {
            Ok(h) => h,
            Err(e) => {
                log::error!("Failed to create settings window: {}", e);
                return;
            }
        };

        let _ = ShowWindow(hwnd, SW_SHOW);

        // Message loop for this window
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            
            // Check if window was destroyed
            if !IsWindow(hwnd).as_bool() {
                break;
            }
        }

        // Cleanup
        let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), hmodule);
        
        // Delete the font
        UI_FONT.with(|f| {
            if let Some(font) = f.borrow_mut().take() {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
        });
    }

    // Clear thread-local state
    DIALOG_STATE.with(|state| {
        *state.borrow_mut() = None;
    });
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            create_controls(hwnd);
            refresh_detected_apps(hwnd);
            load_settings_to_controls(hwnd);
            LRESULT(0)
        }
        WM_SHOWWINDOW => {
            if wparam.0 != 0 {
                refresh_detected_apps(hwnd);
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let state = (wparam.0 & 0xFFFF) as u32;
            if state != WA_INACTIVE {
                refresh_detected_apps(hwnd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let control_id = (wparam.0 & 0xFFFF) as i32;
            let notification = ((wparam.0 >> 16) & 0xFFFF) as u16;
            handle_command(hwnd, control_id, notification);
            LRESULT(0)
        }
        WM_SIZE => {
            let width = (lparam.0 & 0xFFFF) as i32;
            let height = ((lparam.0 >> 16) & 0xFFFF) as i32;
            resize_controls(hwnd, width, height);
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let mmi = lparam.0 as *mut MINMAXINFO;
            if !mmi.is_null() {
                (*mmi).ptMinTrackSize.x = MIN_WINDOW_WIDTH;
                (*mmi).ptMinTrackSize.y = MIN_WINDOW_HEIGHT;
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn create_controls(hwnd: HWND) {
    let hmodule = GetModuleHandleW(None).unwrap_or_default();

    // Get the font
    let font = UI_FONT.with(|f| f.borrow().unwrap_or_default());

    // Get client rect for initial sizing
    let mut rect = std::mem::zeroed();
    let _ = GetClientRect(hwnd, &mut rect);
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    
    // Calculate layout
    let margin = 12;
    let left_panel_width = (width - margin * 3) / 2;
    let right_panel_x = margin * 2 + left_panel_width;
    let right_panel_width = width - right_panel_x - margin;
    
    // Calculate list heights - split the right side into two equal parts
    let detected_group_height = 280;
    let right_panel_height = (detected_group_height - 20) / 2;

    // === Audio Apps Section ===
    // Group box for detected apps (left side)
    let grp_detected = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Detected Audio Apps",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_GROUPBOX as u32),
        margin,
        8,
        left_panel_width,
        detected_group_height,
        ID_GRP_DETECTED,
    );
    set_font(grp_detected, font);

    // Search label
    let lbl_search = create_control(
        hwnd,
        hmodule,
        "STATIC",
        "Search:",
        WS_CHILD | WS_VISIBLE,
        margin + 10,
        28,
        50,
        20,
        ID_LABEL_SEARCH,
    );
    set_font(lbl_search, font);

    // Search box
    let edit_search = create_control(
        hwnd,
        hmodule,
        "EDIT",
        "",
        WS_CHILD | WS_VISIBLE | WS_BORDER,
        margin + 65,
        26,
        left_panel_width - 85,
        22,
        ID_EDIT_SEARCH,
    );
    set_font(edit_search, font);

    // Detected apps listbox
    let list_detected = create_control(
        hwnd,
        hmodule,
        "LISTBOX",
        "",
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_HSCROLL | WS_BORDER | WINDOW_STYLE(LBS_NOTIFY as u32),
        margin + 10,
        52,
        left_panel_width - 20,
        195,
        ID_LIST_DETECTED,
    );
    set_font(list_detected, font);

    // Refresh button under detected list
    let btn_refresh = create_control(hwnd, hmodule, "BUTTON", "Refresh List", 
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32), 
        margin + 10, 252, 100, 26, ID_BTN_REFRESH);
    set_font(btn_refresh, font);

    // Group box for excluded apps (right side, top)
    let grp_excluded = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Excluded Apps (Always Audible)",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_GROUPBOX as u32),
        right_panel_x,
        8,
        right_panel_width,
        right_panel_height,
        ID_GRP_EXCLUDED,
    );
    set_font(grp_excluded, font);

    // Excluded apps listbox
    let list_excluded = create_control(
        hwnd,
        hmodule,
        "LISTBOX",
        "",
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_HSCROLL | WS_BORDER | WINDOW_STYLE(LBS_NOTIFY as u32),
        right_panel_x + 10,
        28,
        right_panel_width - 20,
        right_panel_height - 28,
        ID_LIST_EXCLUDED,
    );
    set_font(list_excluded, font);

    // Group box for always muted apps (right side, bottom)
    let grp_always_muted = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Always Muted",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_GROUPBOX as u32),
        right_panel_x,
        8 + right_panel_height + 5,
        right_panel_width,
        right_panel_height,
        ID_GRP_ALWAYS_MUTED,
    );
    set_font(grp_always_muted, font);

    // Always muted listbox
    let list_always_muted = create_control(
        hwnd,
        hmodule,
        "LISTBOX",
        "",
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_HSCROLL | WS_BORDER | WINDOW_STYLE(LBS_NOTIFY as u32),
        right_panel_x + 10,
        28 + right_panel_height + 5,
        right_panel_width - 20,
        right_panel_height - 28,
        ID_LIST_ALWAYS_MUTED,
    );
    set_font(list_always_muted, font);

    // Add/Remove buttons row
    let buttons_y = detected_group_height + 15;
    
    let btn_add = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Add to Exclusions →",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        margin + 10,
        buttons_y,
        180,
        28,
        ID_BTN_ADD_EXCLUSION,
    );
    set_font(btn_add, font);

    let btn_add_always = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Add to Always Muted →",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        margin + 10,
        buttons_y + 32,
        180,
        28,
        ID_BTN_ADD_ALWAYS_MUTED,
    );
    set_font(btn_add_always, font);

    let btn_remove = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "← Remove from Exclusions",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        right_panel_x + right_panel_width - 220,
        buttons_y,
        220,
        28,
        ID_BTN_REMOVE_EXCLUSION,
    );
    set_font(btn_remove, font);

    let btn_remove_always = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "← Remove from Always Muted",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        right_panel_x + right_panel_width - 240,
        buttons_y + 32,
        240,
        28,
        ID_BTN_REMOVE_ALWAYS_MUTED,
    );
    set_font(btn_remove_always, font);

    // === Settings Section ===
    let settings_y = buttons_y + 75;
    let settings_height = 130;
    
    let grp_settings = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Settings",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_GROUPBOX as u32),
        margin,
        settings_y,
        width - margin * 2,
        settings_height,
        ID_GROUP_SETTINGS,
    );
    set_font(grp_settings, font);

    // Checkboxes
    let chk_enabled = create_control(hwnd, hmodule, "BUTTON", "Muting Enabled", 
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32), 
        margin + 13, settings_y + 22, 200, 22, ID_CHECK_ENABLED);
    set_font(chk_enabled, font);

    let chk_minimized = create_control(hwnd, hmodule, "BUTTON", "Start Minimized to Tray", 
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32), 
        margin + 13, settings_y + 46, 200, 22, ID_CHECK_START_MINIMIZED);
    set_font(chk_minimized, font);

    let chk_startup = create_control(hwnd, hmodule, "BUTTON", "Start with Windows", 
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32), 
        margin + 13, settings_y + 70, 200, 22, ID_CHECK_START_WINDOWS);
    set_font(chk_startup, font);

    // Poll interval on the right side
    let lbl_poll = create_control(
        hwnd,
        hmodule,
        "STATIC",
        "Poll Interval:",
        WS_CHILD | WS_VISIBLE,
        width - 260,
        settings_y + 25,
        90,
        20,
        ID_LABEL_POLL,
    );
    set_font(lbl_poll, font);

    let edit_poll = create_control(
        hwnd,
        hmodule,
        "EDIT",
        "",
        WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_NUMBER as u32),
        width - 165,
        settings_y + 22,
        60,
        24,
        ID_EDIT_POLL_INTERVAL,
    );
    set_font(edit_poll, font);

    let lbl_ms = create_control(hwnd, hmodule, "STATIC", "ms", WS_CHILD | WS_VISIBLE, width - 100, settings_y + 25, 25, 20, ID_LABEL_MS);
    set_font(lbl_ms, font);

    let lbl_range = create_control(
        hwnd,
        hmodule,
        "STATIC",
        "(Range: 100-2000 ms)",
        WS_CHILD | WS_VISIBLE,
        width - 260,
        settings_y + 50,
        180,
        18,
        ID_LABEL_RANGE,
    );
    set_font(lbl_range, font);

    // Config path info
    let lbl_config = create_control(hwnd, hmodule, "STATIC", "Config file:", 
        WS_CHILD | WS_VISIBLE, 
        margin + 13, settings_y + 98, 70, 18, ID_LABEL_CONFIG);
    set_font(lbl_config, font);
    
    let config_path = Config::config_path();
    let path_str = config_path.to_string_lossy();
    let lbl_path = create_control(hwnd, hmodule, "STATIC", &path_str, WS_CHILD | WS_VISIBLE, margin + 85, settings_y + 98, width - margin * 2 - 90, 18, ID_LABEL_PATH);
    set_font(lbl_path, font);

    // === Bottom Buttons ===
    let btn_save_only = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Save",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        width - 330,
        height - 45,
        95,
        32,
        ID_BTN_SAVE_ONLY,
    );
    set_font(btn_save_only, font);

    let btn_save_close = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Save && Close",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        width - 225,
        height - 45,
        105,
        32,
        ID_BTN_SAVE_CLOSE,
    );
    set_font(btn_save_close, font);

    let btn_cancel = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Cancel",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        width - 112,
        height - 45,
        100,
        32,
        ID_BTN_CLOSE,
    );
    set_font(btn_cancel, font);
}

/// Handle window resize - reposition all controls
unsafe fn resize_controls(hwnd: HWND, width: i32, height: i32) {
    if width < 100 || height < 100 {
        return; // Prevent invalid sizes during minimize
    }

    let margin = 12;
    let left_panel_width = (width - margin * 3) / 2;
    let right_panel_x = margin * 2 + left_panel_width;
    let right_panel_width = width - right_panel_x - margin;
    
    // Calculate available height for app lists (leave room for settings, buttons)
    let settings_height = 130;
    let bottom_buttons_height = 50;
    let buttons_row_height = 75;
    let available_for_lists = height - settings_height - bottom_buttons_height - buttons_row_height - 30;
    let detected_group_height = available_for_lists.max(200);
    let right_panel_height = (detected_group_height - 20) / 2;

    // Detected apps group
    move_control(hwnd, ID_GRP_DETECTED, margin, 8, left_panel_width, detected_group_height);
    move_control(hwnd, ID_LABEL_SEARCH, margin + 10, 28, 50, 20);
    move_control(hwnd, ID_EDIT_SEARCH, margin + 65, 26, left_panel_width - 85, 22);
    move_control(hwnd, ID_LIST_DETECTED, margin + 10, 52, left_panel_width - 20, detected_group_height - 85);
    move_control(hwnd, ID_BTN_REFRESH, margin + 10, detected_group_height - 25, 100, 26);

    // Excluded apps group
    move_control(hwnd, ID_GRP_EXCLUDED, right_panel_x, 8, right_panel_width, right_panel_height);
    move_control(hwnd, ID_LIST_EXCLUDED, right_panel_x + 10, 28, right_panel_width - 20, right_panel_height - 28);

    // Always muted group
    move_control(hwnd, ID_GRP_ALWAYS_MUTED, right_panel_x, 8 + right_panel_height + 5, right_panel_width, right_panel_height);
    move_control(hwnd, ID_LIST_ALWAYS_MUTED, right_panel_x + 10, 28 + right_panel_height + 5, right_panel_width - 20, right_panel_height - 28);

    // Add/Remove buttons
    let buttons_y = detected_group_height + 15;
    move_control(hwnd, ID_BTN_ADD_EXCLUSION, margin + 10, buttons_y, 180, 28);
    move_control(hwnd, ID_BTN_ADD_ALWAYS_MUTED, margin + 10, buttons_y + 32, 180, 28);
    move_control(hwnd, ID_BTN_REMOVE_EXCLUSION, right_panel_x + right_panel_width - 220, buttons_y, 220, 28);
    move_control(hwnd, ID_BTN_REMOVE_ALWAYS_MUTED, right_panel_x + right_panel_width - 240, buttons_y + 32, 240, 28);

    // Settings group
    let settings_y = buttons_y + 75;
    move_control(hwnd, ID_GROUP_SETTINGS, margin, settings_y, width - margin * 2, settings_height);
    move_control(hwnd, ID_CHECK_ENABLED, margin + 13, settings_y + 22, 200, 22);
    move_control(hwnd, ID_CHECK_START_MINIMIZED, margin + 13, settings_y + 46, 200, 22);
    move_control(hwnd, ID_CHECK_START_WINDOWS, margin + 13, settings_y + 70, 200, 22);
    move_control(hwnd, ID_LABEL_POLL, width - 260, settings_y + 25, 90, 20);
    move_control(hwnd, ID_EDIT_POLL_INTERVAL, width - 165, settings_y + 22, 60, 24);
    move_control(hwnd, ID_LABEL_MS, width - 100, settings_y + 25, 25, 20);
    move_control(hwnd, ID_LABEL_RANGE, width - 260, settings_y + 50, 180, 18);
    move_control(hwnd, ID_LABEL_CONFIG, margin + 13, settings_y + 98, 70, 18);
    move_control(hwnd, ID_LABEL_PATH, margin + 85, settings_y + 98, width - margin * 2 - 90, 18);

    // Bottom buttons
    move_control(hwnd, ID_BTN_SAVE_ONLY, width - 330, height - 45, 95, 32);
    move_control(hwnd, ID_BTN_SAVE_CLOSE, width - 225, height - 45, 105, 32);
    move_control(hwnd, ID_BTN_CLOSE, width - 112, height - 45, 100, 32);

    // Force redraw
    let _ = InvalidateRect(hwnd, None, true);
}

/// Helper to move/resize a control
unsafe fn move_control(hwnd: HWND, id: i32, x: i32, y: i32, w: i32, h: i32) {
    let ctrl = get_dlg_item(hwnd, id);
    if !ctrl.0.is_null() {
        let _ = SetWindowPos(ctrl, None, x, y, w, h, SWP_NOZORDER);
    }
}

/// Helper to create a control and return its handle
unsafe fn create_control(hwnd: HWND, hmodule: HMODULE, class: &str, text: &str, style: WINDOW_STYLE, x: i32, y: i32, w: i32, h: i32, id: i32) -> HWND {
    let hwnd_ctl = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        to_wide_ptr(class),
        to_wide_ptr(text),
        style,
        x,
        y,
        w,
        h,
        hwnd,
        if id != 0 { HMENU(id as *mut c_void) } else { HMENU::default() },
        hmodule,
        None,
    );
    hwnd_ctl.unwrap_or_default()
}

/// Set font on a control
unsafe fn set_font(hwnd: HWND, font: HFONT) {
    SendMessageW(hwnd, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
}

unsafe fn refresh_detected_apps(hwnd: HWND) {
    let list_detected = get_dlg_item(hwnd, ID_LIST_DETECTED);
    
    // Clear list
    SendMessageW(list_detected, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    DIALOG_STATE.with(|state| {
        if let Some(ref mut s) = *state.borrow_mut() {
            // Get current audio sessions
            let sessions = s.audio_manager.get_sessions();
            s.all_detected_apps.clear();
            s.detected_apps.clear();

            let config = s.config.read();
            let excluded = config.excluded_apps.clone();
            let always_muted = config.always_muted_apps.clone();
            drop(config);

            // Build the full list of apps (excluding already excluded/always-muted)
            let mut apps: Vec<(u32, String)> = Vec::new();
            for session in sessions {
                // Skip if already excluded
                if excluded.contains(&session.process_name.to_lowercase()) {
                    continue;
                }

                // Skip if already always-muted
                if always_muted.contains(&session.process_name.to_lowercase()) {
                    continue;
                }

                apps.push((session.process_id, session.process_name.clone()));
            }

            // Sort alphabetically by process name (case-insensitive)
            apps.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
            s.all_detected_apps = apps;

            // Apply search filter
            let filter = s.search_filter.to_lowercase();
            for (pid, name) in &s.all_detected_apps {
                if filter.is_empty() || name.to_lowercase().contains(&filter) {
                    s.detected_apps.push((*pid, name.clone()));
                    
                    let display = format!("{} (PID: {})", name, pid);
                    let wide = to_wide(&display);
                    SendMessageW(
                        list_detected,
                        LB_ADDSTRING,
                        WPARAM(0),
                        LPARAM(wide.as_ptr() as isize),
                    );
                }
            }
        }
    });

    // Refresh excluded list too
    refresh_excluded_list(hwnd);

    // Refresh always muted list too
    refresh_always_muted_list(hwnd);
}

/// Apply search filter without refreshing from audio manager
unsafe fn apply_search_filter(hwnd: HWND) {
    let list_detected = get_dlg_item(hwnd, ID_LIST_DETECTED);
    
    // Clear list
    SendMessageW(list_detected, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    DIALOG_STATE.with(|state| {
        if let Some(ref mut s) = *state.borrow_mut() {
            s.detected_apps.clear();

            // Apply search filter
            let filter = s.search_filter.to_lowercase();
            for (pid, name) in &s.all_detected_apps {
                if filter.is_empty() || name.to_lowercase().contains(&filter) {
                    s.detected_apps.push((*pid, name.clone()));
                    
                    let display = format!("{} (PID: {})", name, pid);
                    let wide = to_wide(&display);
                    SendMessageW(
                        list_detected,
                        LB_ADDSTRING,
                        WPARAM(0),
                        LPARAM(wide.as_ptr() as isize),
                    );
                }
            }
        }
    });
}

unsafe fn refresh_excluded_list(hwnd: HWND) {
    let list_excluded = get_dlg_item(hwnd, ID_LIST_EXCLUDED);
    
    // Clear list
    SendMessageW(list_excluded, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let mut excluded: Vec<_> = s.config.read().excluded_apps.iter().cloned().collect();
            
            // Sort alphabetically (case-insensitive)
            excluded.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
            
            for app in excluded {
                let wide = to_wide(&app);
                SendMessageW(
                    list_excluded,
                    LB_ADDSTRING,
                    WPARAM(0),
                    LPARAM(wide.as_ptr() as isize),
                );
            }
        }
    });
}

unsafe fn refresh_always_muted_list(hwnd: HWND) {
    let list_always_muted = get_dlg_item(hwnd, ID_LIST_ALWAYS_MUTED);

    // Clear list
    SendMessageW(list_always_muted, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let mut always_muted: Vec<_> = s.config.read().always_muted_apps.iter().cloned().collect();

            // Sort alphabetically (case-insensitive)
            always_muted.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

            for app in always_muted {
                let wide = to_wide(&app);
                SendMessageW(
                    list_always_muted,
                    LB_ADDSTRING,
                    WPARAM(0),
                    LPARAM(wide.as_ptr() as isize),
                );
            }
        }
    });
}

unsafe fn load_settings_to_controls(hwnd: HWND) {
    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let config = s.config.read();
            let muting_enabled = s.muting_enabled.load(Ordering::Relaxed);

            // Muting enabled checkbox
            SendMessageW(
                get_dlg_item(hwnd, ID_CHECK_ENABLED),
                BM_SETCHECK,
                WPARAM(if muting_enabled { BST_CHECKED } else { BST_UNCHECKED }),
                LPARAM(0),
            );

            // Start minimized checkbox
            SendMessageW(
                get_dlg_item(hwnd, ID_CHECK_START_MINIMIZED),
                BM_SETCHECK,
                WPARAM(if config.start_minimized { BST_CHECKED } else { BST_UNCHECKED }),
                LPARAM(0),
            );

            // Start with Windows checkbox
            SendMessageW(
                get_dlg_item(hwnd, ID_CHECK_START_WINDOWS),
                BM_SETCHECK,
                WPARAM(if config.start_with_windows { BST_CHECKED } else { BST_UNCHECKED }),
                LPARAM(0),
            );

            // Poll interval edit
            let poll_str = to_wide(&config.poll_interval_ms.to_string());
            let _ = SetWindowTextW(get_dlg_item(hwnd, ID_EDIT_POLL_INTERVAL), PCWSTR(poll_str.as_ptr()));
        }
    });
}

unsafe fn handle_command(hwnd: HWND, control_id: i32, notification: u16) {
    match control_id {
        ID_EDIT_SEARCH => {
            // Handle search box text change
            if notification == EN_CHANGE {
                // Get search text
                let edit_search = get_dlg_item(hwnd, ID_EDIT_SEARCH);
                let mut buffer: [u16; 256] = [0; 256];
                let len = GetWindowTextW(edit_search, &mut buffer);
                let search_text = if len > 0 {
                    String::from_utf16_lossy(&buffer[..len as usize])
                } else {
                    String::new()
                };
                
                // Update search filter in state
                DIALOG_STATE.with(|state| {
                    if let Some(ref mut s) = *state.borrow_mut() {
                        s.search_filter = search_text;
                    }
                });
                
                // Apply filter
                apply_search_filter(hwnd);
            }
        }
        ID_BTN_REFRESH => {
            refresh_detected_apps(hwnd);
        }
        ID_BTN_ADD_EXCLUSION => {
            add_selected_to_exclusions(hwnd);
        }
        ID_BTN_REMOVE_EXCLUSION => {
            remove_selected_exclusion(hwnd);
        }
        ID_BTN_ADD_ALWAYS_MUTED => {
            add_selected_to_always_muted(hwnd);
        }
        ID_BTN_REMOVE_ALWAYS_MUTED => {
            remove_selected_always_muted(hwnd);
        }
        ID_BTN_SAVE_ONLY => {
            save_settings(hwnd);
        }
        ID_BTN_SAVE_CLOSE => {
            save_settings(hwnd);
            let _ = DestroyWindow(hwnd);
        }
        ID_BTN_CLOSE => {
            let _ = DestroyWindow(hwnd);
        }
        _ => {}
    }
}

unsafe fn add_selected_to_exclusions(hwnd: HWND) {
    let list_detected = get_dlg_item(hwnd, ID_LIST_DETECTED);
    let sel_idx = SendMessageW(list_detected, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as i32;
    
    if sel_idx < 0 {
        return; // Nothing selected
    }

    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            if let Some((_, name)) = s.detected_apps.get(sel_idx as usize) {
                let mut config = s.config.write();
                config.add_excluded_app(name);
            }
        }
    });

    refresh_detected_apps(hwnd);
}

unsafe fn remove_selected_exclusion(hwnd: HWND) {
    let list_excluded = get_dlg_item(hwnd, ID_LIST_EXCLUDED);
    let sel_idx = SendMessageW(list_excluded, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as i32;
    
    if sel_idx < 0 {
        return; // Nothing selected
    }

    // Get the selected text
    let text_len = SendMessageW(list_excluded, LB_GETTEXTLEN, WPARAM(sel_idx as usize), LPARAM(0)).0 as usize;
    if text_len == 0 {
        return;
    }

    let mut buffer: Vec<u16> = vec![0; text_len + 1];
    SendMessageW(
        list_excluded,
        LB_GETTEXT,
        WPARAM(sel_idx as usize),
        LPARAM(buffer.as_mut_ptr() as isize),
    );

    let app_name = String::from_utf16_lossy(&buffer[..text_len]);

    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let mut config = s.config.write();
            config.remove_excluded_app(&app_name);
        }
    });

    refresh_detected_apps(hwnd);
}

unsafe fn add_selected_to_always_muted(hwnd: HWND) {
    let list_detected = get_dlg_item(hwnd, ID_LIST_DETECTED);
    let sel_idx = SendMessageW(list_detected, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as i32;

    if sel_idx < 0 {
        return; // Nothing selected
    }

    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            if let Some((_, name)) = s.detected_apps.get(sel_idx as usize) {
                let mut config = s.config.write();
                config.add_always_muted_app(name);
            }
        }
    });

    refresh_detected_apps(hwnd);
}

unsafe fn remove_selected_always_muted(hwnd: HWND) {
    let list_always_muted = get_dlg_item(hwnd, ID_LIST_ALWAYS_MUTED);
    let sel_idx = SendMessageW(list_always_muted, LB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as i32;

    if sel_idx < 0 {
        return; // Nothing selected
    }

    // Get the selected text
    let text_len = SendMessageW(list_always_muted, LB_GETTEXTLEN, WPARAM(sel_idx as usize), LPARAM(0)).0 as usize;
    if text_len == 0 {
        return;
    }

    let mut buffer: Vec<u16> = vec![0; text_len + 1];
    SendMessageW(
        list_always_muted,
        LB_GETTEXT,
        WPARAM(sel_idx as usize),
        LPARAM(buffer.as_mut_ptr() as isize),
    );

    let app_name = String::from_utf16_lossy(&buffer[..text_len]);

    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let mut config = s.config.write();
            config.remove_always_muted_app(&app_name);
        }
    });

    refresh_detected_apps(hwnd);
}

unsafe fn save_settings(hwnd: HWND) {
    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let mut config = s.config.write();

            // Read checkbox states
            let muting_checked = SendMessageW(
                get_dlg_item(hwnd, ID_CHECK_ENABLED),
                BM_GETCHECK,
                WPARAM(0),
                LPARAM(0),
            ).0 == BST_CHECKED as isize;

            let start_minimized_checked = SendMessageW(
                get_dlg_item(hwnd, ID_CHECK_START_MINIMIZED),
                BM_GETCHECK,
                WPARAM(0),
                LPARAM(0),
            ).0 == BST_CHECKED as isize;

            let start_windows_checked = SendMessageW(
                get_dlg_item(hwnd, ID_CHECK_START_WINDOWS),
                BM_GETCHECK,
                WPARAM(0),
                LPARAM(0),
            ).0 == BST_CHECKED as isize;

            // Read poll interval
            let mut buffer: [u16; 32] = [0; 32];
            GetWindowTextW(get_dlg_item(hwnd, ID_EDIT_POLL_INTERVAL), &mut buffer);
            let poll_str = String::from_utf16_lossy(&buffer);
            let poll_str = poll_str.trim_matches('\0').trim();
            let poll_interval: u64 = poll_str.parse().unwrap_or(500).clamp(100, 2000);

            // Update config
            config.muting_enabled = muting_checked;
            config.start_minimized = start_minimized_checked;
            config.poll_interval_ms = poll_interval;

            // Handle startup setting change
            if config.start_with_windows != start_windows_checked {
                config.start_with_windows = start_windows_checked;
                if let Err(e) = startup::set_run_at_startup(start_windows_checked) {
                    log::error!("Failed to update startup setting: {}", e);
                }
            }

            // Update atomic muting state
            s.muting_enabled.store(muting_checked, Ordering::SeqCst);

            // Save to disk
            let _ = config.save();
        }
    });
}
