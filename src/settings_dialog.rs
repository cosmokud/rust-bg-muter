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
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, FillRect, GetSysColorBrush, HBRUSH, HDC, HFONT,
    HGDIOBJ, DEFAULT_CHARSET, OUT_TT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
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
const ID_BTN_SAVE: i32 = 110;
const ID_BTN_CLOSE: i32 = 111;
const ID_GROUP_SETTINGS: i32 = 113;
const ID_LABEL_POLL: i32 = 114;
const ID_GROUP_DETECTED: i32 = 117;
const ID_GROUP_EXCLUDED: i32 = 118;
const ID_GROUP_ALWAYS_MUTED: i32 = 119;
const ID_LABEL_SEARCH: i32 = 120;
const ID_EDIT_SEARCH: i32 = 121;
const ID_LABEL_MS: i32 = 122;
const ID_LABEL_POLL_RANGE: i32 = 123;
const ID_LABEL_CONFIG: i32 = 124;
const ID_LABEL_CONFIG_PATH: i32 = 125;

// Window dimensions (wider so process name + PID fits without truncation)
const WINDOW_WIDTH: i32 = 820;
const WINDOW_HEIGHT: i32 = 560;

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
    all_detected_apps: Vec<(u32, String)>,
    detected_apps: Vec<(u32, String)>, // (pid, name)
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
            all_detected_apps: Vec::new(),
            detected_apps: Vec::new(),
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

        // Create window
        let hwnd_result = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_TOPMOST,
            PCWSTR(class_name.as_ptr()),
            to_wide_ptr("Background Muter - Settings"),
            WS_OVERLAPPED
                | WS_CAPTION
                | WS_SYSMENU
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_THICKFRAME
                | WS_CLIPSIBLINGS
                | WS_CLIPCHILDREN
                | WS_VISIBLE,
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
            layout_controls(hwnd);
            refresh_detected_apps(hwnd);
            load_settings_to_controls(hwnd);
            LRESULT(0)
        }
        WM_SIZE => {
            layout_controls(hwnd);
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
            mmi.ptMinTrackSize.x = WINDOW_WIDTH;
            mmi.ptMinTrackSize.y = WINDOW_HEIGHT;
            LRESULT(0)
        }
        WM_COMMAND => {
            let control_id = (wparam.0 & 0xFFFF) as i32;
            let notification = ((wparam.0 >> 16) & 0xFFFF) as u16;
            handle_command(hwnd, control_id, notification);
            LRESULT(0)
        }
        WM_CTLCOLORLISTBOX | WM_CTLCOLOREDIT => {
            let brush = GetSysColorBrush(COLOR_BTNFACE);
            LRESULT(brush.0 as isize)
        }
        WM_ERASEBKGND => {
            let hdc = HDC(wparam.0 as *mut c_void);
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let brush = GetSysColorBrush(COLOR_BTNFACE);
            let _ = FillRect(hdc, &rect, brush);
            LRESULT(1)
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

    // === Audio Apps Section ===
    // Group box for detected apps
    let grp_detected = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Detected Audio Apps",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_GROUPBOX as u32),
        12,
        8,
        390,
        230,
        ID_GROUP_DETECTED,
    );
    set_font(grp_detected, font);

    // Search label + box (within detected apps)
    let lbl_search = create_control(
        hwnd,
        hmodule,
        "STATIC",
        "Search:",
        WS_CHILD | WS_VISIBLE,
        22,
        30,
        50,
        20,
        ID_LABEL_SEARCH,
    );
    set_font(lbl_search, font);

    let edit_search = create_control(
        hwnd,
        hmodule,
        "EDIT",
        "",
        WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        76,
        28,
        316,
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
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_BORDER | WINDOW_STYLE(LBS_NOTIFY as u32),
        22,
        54,
        370,
        149,
        ID_LIST_DETECTED,
    );
    set_font(list_detected, font);

    // Refresh button under detected list
    let btn_refresh = create_control(hwnd, hmodule, "BUTTON", "Refresh List", 
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32), 
        22, 206, 100, 26, ID_BTN_REFRESH);
    set_font(btn_refresh, font);

    // Group box for excluded apps
    let grp_excluded = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Excluded Apps (Always Audible)",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_GROUPBOX as u32),
        418,
        8,
        390,
        115,
        ID_GROUP_EXCLUDED,
    );
    set_font(grp_excluded, font);

    // Excluded apps listbox
    let list_excluded = create_control(
        hwnd,
        hmodule,
        "LISTBOX",
        "",
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_BORDER | WINDOW_STYLE(LBS_NOTIFY as u32 | LBS_SORT as u32),
        428,
        28,
        370,
        85,
        ID_LIST_EXCLUDED,
    );
    set_font(list_excluded, font);

    // Group box for always muted apps
    let grp_always_muted = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Always Muted",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_GROUPBOX as u32),
        418,
        128,
        390,
        110,
        ID_GROUP_ALWAYS_MUTED,
    );
    set_font(grp_always_muted, font);

    // Always muted listbox
    let list_always_muted = create_control(
        hwnd,
        hmodule,
        "LISTBOX",
        "",
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_BORDER | WINDOW_STYLE(LBS_NOTIFY as u32 | LBS_SORT as u32),
        428,
        148,
        370,
        80,
        ID_LIST_ALWAYS_MUTED,
    );
    set_font(list_always_muted, font);

    // Add/Remove buttons
    let btn_add = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Add to Exclusions →",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        22,
        245,
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
        22,
        275,
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
        588,
        245,
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
        568,
        275,
        240,
        28,
        ID_BTN_REMOVE_ALWAYS_MUTED,
    );
    set_font(btn_remove_always, font);

    // === Settings Section ===
    let grp_settings = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Settings",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_GROUPBOX as u32),
        12,
        312,
        796,
        130,
        ID_GROUP_SETTINGS,
    );
    set_font(grp_settings, font);

    // Checkboxes
    let chk_enabled = create_control(hwnd, hmodule, "BUTTON", "Muting Enabled", 
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32), 
        25, 334, 200, 22, ID_CHECK_ENABLED);
    set_font(chk_enabled, font);

    let chk_minimized = create_control(hwnd, hmodule, "BUTTON", "Start Minimized to Tray", 
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32), 
        25, 358, 200, 22, ID_CHECK_START_MINIMIZED);
    set_font(chk_minimized, font);

    let chk_startup = create_control(hwnd, hmodule, "BUTTON", "Start with Windows", 
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32), 
        25, 382, 200, 22, ID_CHECK_START_WINDOWS);
    set_font(chk_startup, font);

    // Poll interval on the right side
    let lbl_poll = create_control(
        hwnd,
        hmodule,
        "STATIC",
        "Poll Interval:",
        WS_CHILD | WS_VISIBLE,
        560,
        337,
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
        655,
        334,
        60,
        24,
        ID_EDIT_POLL_INTERVAL,
    );
    set_font(edit_poll, font);

    let lbl_ms = create_control(hwnd, hmodule, "STATIC", "ms", WS_CHILD | WS_VISIBLE, 722, 337, 25, 20, ID_LABEL_MS);
    set_font(lbl_ms, font);

    let lbl_range = create_control(
        hwnd,
        hmodule,
        "STATIC",
        "(Range: 100-2000 ms)",
        WS_CHILD | WS_VISIBLE,
        560,
        362,
        180,
        18,
        ID_LABEL_POLL_RANGE,
    );
    set_font(lbl_range, font);

    // Config path info
    let lbl_config = create_control(hwnd, hmodule, "STATIC", "Config file:", 
        WS_CHILD | WS_VISIBLE, 
        25, 410, 70, 18, ID_LABEL_CONFIG);
    set_font(lbl_config, font);
    
    let config_path = Config::config_path();
    let path_str = config_path.to_string_lossy();
    let lbl_path = create_control(hwnd, hmodule, "STATIC", &path_str, WS_CHILD | WS_VISIBLE, 95, 410, 710, 18, ID_LABEL_CONFIG_PATH);
    set_font(lbl_path, font);

    // === Bottom Buttons ===
    let btn_save = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Save && Close",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        595,
        465,
        105,
        32,
        ID_BTN_SAVE,
    );
    set_font(btn_save, font);

    let btn_cancel = create_control(
        hwnd,
        hmodule,
        "BUTTON",
        "Cancel",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        708,
        465,
        100,
        32,
        ID_BTN_CLOSE,
    );
    set_font(btn_cancel, font);
}

unsafe fn layout_controls(hwnd: HWND) {
    let mut rect = RECT::default();
    let _ = GetClientRect(hwnd, &mut rect);
    let client_w = rect.right - rect.left;
    let client_h = rect.bottom - rect.top;

    let margin = 12;
    let gap = 8;
    let col_gap = 16;
    let button_height = 32;
    let settings_height = 130;
    let buttons_area_height = 62;

    let top_section_height = client_h - (margin * 3) - settings_height - button_height;
    let top_section_height = top_section_height.max(220);

    let top_y = margin;
    let settings_y = top_y + top_section_height + margin;
    let buttons_y = client_h - margin - button_height;

    let left_w = (client_w - margin * 2 - col_gap) / 2;
    let right_w = client_w - margin * 2 - col_gap - left_w;
    let left_x = margin;
    let right_x = left_x + left_w + col_gap;

    let group_area_height = top_section_height - buttons_area_height - gap;
    let group_area_height = group_area_height.max(140);

    let right_top_h = (group_area_height - gap) / 2;
    let right_bottom_h = group_area_height - gap - right_top_h;

    // Detected group
    let detected_group = get_dlg_item(hwnd, ID_GROUP_DETECTED);
    move_window(detected_group, left_x, top_y, left_w, group_area_height);

    let inner_margin = 10;
    let group_header = 22;
    let search_height = 22;
    let refresh_height = 26;

    let search_label = get_dlg_item(hwnd, ID_LABEL_SEARCH);
    let search_edit = get_dlg_item(hwnd, ID_EDIT_SEARCH);

    let search_y = top_y + group_header;
    let search_label_w = 50;
    let search_label_x = left_x + inner_margin;
    move_window(search_label, search_label_x, search_y, search_label_w, 20);

    let search_edit_x = search_label_x + search_label_w + 6;
    let search_edit_w = left_w - inner_margin * 2 - search_label_w - 6;
    move_window(search_edit, search_edit_x, search_y - 2, search_edit_w, search_height);

    let list_detected = get_dlg_item(hwnd, ID_LIST_DETECTED);
    let refresh_btn = get_dlg_item(hwnd, ID_BTN_REFRESH);

    let refresh_y = top_y + group_area_height - inner_margin - refresh_height;
    move_window(refresh_btn, left_x + inner_margin, refresh_y, 110, refresh_height);

    let list_y = search_y + search_height + 8;
    let list_h = (refresh_y - list_y - 8).max(60);
    move_window(list_detected, left_x + inner_margin, list_y, left_w - inner_margin * 2, list_h);

    // Excluded group
    let excluded_group = get_dlg_item(hwnd, ID_GROUP_EXCLUDED);
    move_window(excluded_group, right_x, top_y, right_w, right_top_h);

    let list_excluded = get_dlg_item(hwnd, ID_LIST_EXCLUDED);
    let list_excluded_y = top_y + group_header;
    let list_excluded_h = right_top_h - group_header - inner_margin;
    move_window(
        list_excluded,
        right_x + inner_margin,
        list_excluded_y,
        right_w - inner_margin * 2,
        list_excluded_h.max(40),
    );

    // Always muted group
    let always_group = get_dlg_item(hwnd, ID_GROUP_ALWAYS_MUTED);
    let always_y = top_y + right_top_h + gap;
    move_window(always_group, right_x, always_y, right_w, right_bottom_h);

    let list_always = get_dlg_item(hwnd, ID_LIST_ALWAYS_MUTED);
    let list_always_y = always_y + group_header;
    let list_always_h = right_bottom_h - group_header - inner_margin;
    move_window(
        list_always,
        right_x + inner_margin,
        list_always_y,
        right_w - inner_margin * 2,
        list_always_h.max(40),
    );

    // Add/Remove buttons area
    let buttons_top = top_y + group_area_height + gap;
    let row_gap = 6;
    let row1_y = buttons_top;
    let row2_y = buttons_top + button_height + row_gap;

    let add_btn = get_dlg_item(hwnd, ID_BTN_ADD_EXCLUSION);
    let add_always_btn = get_dlg_item(hwnd, ID_BTN_ADD_ALWAYS_MUTED);
    let remove_btn = get_dlg_item(hwnd, ID_BTN_REMOVE_EXCLUSION);
    let remove_always_btn = get_dlg_item(hwnd, ID_BTN_REMOVE_ALWAYS_MUTED);

    let left_btn_w = (left_w - inner_margin * 2).max(120);
    move_window(add_btn, left_x + inner_margin, row1_y, left_btn_w, 28);
    move_window(add_always_btn, left_x + inner_margin, row2_y, left_btn_w, 28);

    let right_btn_w = (right_w - inner_margin * 2).max(160);
    move_window(remove_btn, right_x + inner_margin, row1_y, right_btn_w, 28);
    move_window(remove_always_btn, right_x + inner_margin, row2_y, right_btn_w, 28);

    // Settings group
    let grp_settings = get_dlg_item(hwnd, ID_GROUP_SETTINGS);
    move_window(grp_settings, margin, settings_y, client_w - margin * 2, settings_height);

    let chk_enabled = get_dlg_item(hwnd, ID_CHECK_ENABLED);
    let chk_minimized = get_dlg_item(hwnd, ID_CHECK_START_MINIMIZED);
    let chk_startup = get_dlg_item(hwnd, ID_CHECK_START_WINDOWS);

    move_window(chk_enabled, margin + 13, settings_y + 22, 220, 22);
    move_window(chk_minimized, margin + 13, settings_y + 46, 220, 22);
    move_window(chk_startup, margin + 13, settings_y + 70, 220, 22);

    let lbl_poll = get_dlg_item(hwnd, ID_LABEL_POLL);
    let edit_poll = get_dlg_item(hwnd, ID_EDIT_POLL_INTERVAL);
    let lbl_ms = get_dlg_item(hwnd, ID_LABEL_MS);
    let lbl_range = get_dlg_item(hwnd, ID_LABEL_POLL_RANGE);

    let right_inner = margin + (client_w - margin * 2) - inner_margin;
    let edit_w = 60;
    let ms_w = 22;
    let poll_label_w = 90;
    let poll_label_x = right_inner - (poll_label_w + edit_w + ms_w + 10);
    move_window(lbl_poll, poll_label_x, settings_y + 25, poll_label_w, 20);
    move_window(edit_poll, poll_label_x + poll_label_w + 6, settings_y + 22, edit_w, 24);
    move_window(lbl_ms, poll_label_x + poll_label_w + 6 + edit_w + 4, settings_y + 25, ms_w, 20);
    move_window(lbl_range, poll_label_x, settings_y + 50, 180, 18);

    let lbl_config = get_dlg_item(hwnd, ID_LABEL_CONFIG);
    let lbl_path = get_dlg_item(hwnd, ID_LABEL_CONFIG_PATH);
    move_window(lbl_config, margin + 13, settings_y + 92, 80, 18);
    move_window(lbl_path, margin + 90, settings_y + 92, client_w - (margin + 90) - margin, 18);

    // Bottom buttons
    let btn_save = get_dlg_item(hwnd, ID_BTN_SAVE);
    let btn_cancel = get_dlg_item(hwnd, ID_BTN_CLOSE);

    let btn_cancel_w = 100;
    let btn_save_w = 105;
    let btn_gap = 10;
    let btn_cancel_x = client_w - margin - btn_cancel_w;
    let btn_save_x = btn_cancel_x - btn_gap - btn_save_w;

    move_window(btn_save, btn_save_x, buttons_y, btn_save_w, button_height);
    move_window(btn_cancel, btn_cancel_x, buttons_y, btn_cancel_w, button_height);
}

unsafe fn move_window(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    if hwnd.0 == std::ptr::null_mut() {
        return;
    }
    let _ = MoveWindow(hwnd, x, y, w, h, true);
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
    DIALOG_STATE.with(|state| {
        if let Some(ref mut s) = *state.borrow_mut() {
            // Get current audio sessions
            let sessions = s.audio_manager.get_sessions();
            s.all_detected_apps.clear();

            let config = s.config.read();
            let excluded = config.excluded_apps.clone();
            let always_muted = config.always_muted_apps.clone();
            drop(config);

            for session in sessions {
                // Skip if already excluded
                if excluded.contains(&session.process_name.to_lowercase()) {
                    continue;
                }

                // Skip if already always-muted
                if always_muted.contains(&session.process_name.to_lowercase()) {
                    continue;
                }

                s.all_detected_apps
                    .push((session.process_id, session.process_name.clone()));
            }
        }
    });

    apply_detected_filter(hwnd);

    // Refresh excluded list too
    refresh_excluded_list(hwnd);

    // Refresh always muted list too
    refresh_always_muted_list(hwnd);
}

unsafe fn get_search_text(hwnd: HWND) -> String {
    let edit_search = get_dlg_item(hwnd, ID_EDIT_SEARCH);
    if edit_search.0 == std::ptr::null_mut() {
        return String::new();
    }

    let mut buffer: Vec<u16> = vec![0; 256];
    let len = GetWindowTextW(edit_search, &mut buffer) as usize;
    if len == 0 {
        return String::new();
    }

    let text = String::from_utf16_lossy(&buffer[..len]);
    text.trim().to_string()
}

unsafe fn apply_detected_filter(hwnd: HWND) {
    let list_detected = get_dlg_item(hwnd, ID_LIST_DETECTED);
    if list_detected.0 == std::ptr::null_mut() {
        return;
    }

    // Clear list
    SendMessageW(list_detected, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    let query = get_search_text(hwnd).to_lowercase();

    DIALOG_STATE.with(|state| {
        if let Some(ref mut s) = *state.borrow_mut() {
            s.detected_apps.clear();

            for (pid, name) in &s.all_detected_apps {
                let name_lower = name.to_lowercase();
                if !query.is_empty() && !name_lower.contains(&query) {
                    continue;
                }

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
    });
}

unsafe fn refresh_excluded_list(hwnd: HWND) {
    let list_excluded = get_dlg_item(hwnd, ID_LIST_EXCLUDED);
    
    // Clear list
    SendMessageW(list_excluded, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let mut excluded: Vec<_> = s.config.read().excluded_apps.iter().cloned().collect();
            excluded.sort();
            
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
            always_muted.sort();

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

unsafe fn handle_command(hwnd: HWND, control_id: i32, _notification: u16) {
    match control_id {
        ID_EDIT_SEARCH => {
            if _notification as u32 == EN_CHANGE {
                apply_detected_filter(hwnd);
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
        ID_BTN_SAVE => {
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
