//! Native Win32 Settings Dialog
//!
//! A lightweight settings window using pure Win32 API.
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
use windows::Win32::Graphics::Gdi::{GetStockObject, HBRUSH, WHITE_BRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

// Control IDs
const ID_LIST_DETECTED: i32 = 101;
const ID_LIST_EXCLUDED: i32 = 102;
const ID_BTN_ADD_EXCLUSION: i32 = 103;
const ID_BTN_REMOVE_EXCLUSION: i32 = 104;
const ID_BTN_REFRESH: i32 = 105;
const ID_CHECK_ENABLED: i32 = 106;
const ID_CHECK_START_MINIMIZED: i32 = 107;
const ID_CHECK_START_WINDOWS: i32 = 108;
const ID_EDIT_POLL_INTERVAL: i32 = 109;
const ID_BTN_SAVE: i32 = 110;
const ID_BTN_CLOSE: i32 = 111;

// Window dimensions
const WINDOW_WIDTH: i32 = 500;
const WINDOW_HEIGHT: i32 = 520;

// Button states (not in windows-rs WindowsAndMessaging by default)
const BST_CHECKED: usize = 1;
const BST_UNCHECKED: usize = 0;

// Thread-local storage for dialog state
thread_local! {
    static DIALOG_STATE: RefCell<Option<DialogState>> = const { RefCell::new(None) };
}

struct DialogState {
    config: Arc<RwLock<Config>>,
    muting_enabled: Arc<AtomicBool>,
    audio_manager: Arc<AudioManager>,
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
        });
    });

    unsafe {
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
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
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
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
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
        WM_COMMAND => {
            let control_id = (wparam.0 & 0xFFFF) as i32;
            let notification = ((wparam.0 >> 16) & 0xFFFF) as u16;
            handle_command(hwnd, control_id, notification);
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

    // Static labels
    create_static(hwnd, hmodule, "Detected Audio Apps:", 10, 10, 200, 20);
    create_static(hwnd, hmodule, "Excluded Apps:", 260, 10, 200, 20);

    // Detected apps listbox
    let _ = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        to_wide_ptr("LISTBOX"),
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_BORDER | WINDOW_STYLE(LBS_NOTIFY as u32),
        10,
        30,
        230,
        200,
        hwnd,
        HMENU(ID_LIST_DETECTED as *mut c_void),
        hmodule,
        None,
    );

    // Excluded apps listbox
    let _ = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        to_wide_ptr("LISTBOX"),
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_BORDER | WINDOW_STYLE(LBS_NOTIFY as u32),
        260,
        30,
        220,
        200,
        hwnd,
        HMENU(ID_LIST_EXCLUDED as *mut c_void),
        hmodule,
        None,
    );

    // Buttons row 1
    create_button(hwnd, hmodule, "Add Exclusion >>", 10, 235, 140, 28, ID_BTN_ADD_EXCLUSION);
    create_button(hwnd, hmodule, "<< Remove", 330, 235, 100, 28, ID_BTN_REMOVE_EXCLUSION);
    create_button(hwnd, hmodule, "Refresh", 160, 235, 70, 28, ID_BTN_REFRESH);

    // Separator line using static text
    create_static(hwnd, hmodule, "_______________________________________________________________", 10, 265, 470, 15);

    // Settings section
    create_static(hwnd, hmodule, "Settings", 10, 290, 200, 20);

    // Checkboxes
    let _ = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        to_wide_ptr("BUTTON"),
        to_wide_ptr("Muting Enabled"),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        10,
        315,
        200,
        25,
        hwnd,
        HMENU(ID_CHECK_ENABLED as *mut c_void),
        hmodule,
        None,
    );

    let _ = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        to_wide_ptr("BUTTON"),
        to_wide_ptr("Start Minimized to Tray"),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        10,
        345,
        200,
        25,
        hwnd,
        HMENU(ID_CHECK_START_MINIMIZED as *mut c_void),
        hmodule,
        None,
    );

    let _ = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        to_wide_ptr("BUTTON"),
        to_wide_ptr("Start with Windows"),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
        10,
        375,
        200,
        25,
        hwnd,
        HMENU(ID_CHECK_START_WINDOWS as *mut c_void),
        hmodule,
        None,
    );

    // Poll interval
    create_static(hwnd, hmodule, "Poll Interval (ms):", 260, 318, 120, 20);
    let _ = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        to_wide_ptr("EDIT"),
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_NUMBER as u32),
        385,
        315,
        80,
        24,
        hwnd,
        HMENU(ID_EDIT_POLL_INTERVAL as *mut c_void),
        hmodule,
        None,
    );

    create_static(hwnd, hmodule, "(100-2000)", 385, 342, 80, 18);

    // Info text
    create_static(hwnd, hmodule, "Config file location:", 10, 410, 120, 20);
    
    let config_path = Config::config_path();
    let path_str = config_path.to_string_lossy();
    create_static(hwnd, hmodule, &path_str, 10, 428, 470, 20);

    // Bottom buttons
    create_button(hwnd, hmodule, "Save && Close", 260, 455, 110, 30, ID_BTN_SAVE);
    create_button(hwnd, hmodule, "Cancel", 380, 455, 90, 30, ID_BTN_CLOSE);
}

unsafe fn create_static(hwnd: HWND, hmodule: HMODULE, text: &str, x: i32, y: i32, w: i32, h: i32) {
    let _ = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        to_wide_ptr("STATIC"),
        to_wide_ptr(text),
        WS_CHILD | WS_VISIBLE,
        x,
        y,
        w,
        h,
        hwnd,
        None,
        hmodule,
        None,
    );
}

unsafe fn create_button(hwnd: HWND, hmodule: HMODULE, text: &str, x: i32, y: i32, w: i32, h: i32, id: i32) {
    let _ = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        to_wide_ptr("BUTTON"),
        to_wide_ptr(text),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
        x,
        y,
        w,
        h,
        hwnd,
        HMENU(id as *mut c_void),
        hmodule,
        None,
    );
}

unsafe fn refresh_detected_apps(hwnd: HWND) {
    let list_detected = get_dlg_item(hwnd, ID_LIST_DETECTED);
    
    // Clear list
    SendMessageW(list_detected, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    DIALOG_STATE.with(|state| {
        if let Some(ref mut s) = *state.borrow_mut() {
            // Get current audio sessions
            let sessions = s.audio_manager.get_sessions();
            s.detected_apps.clear();

            let excluded = s.config.read().excluded_apps.clone();

            for session in sessions {
                // Skip if already excluded
                if excluded.contains(&session.process_name.to_lowercase()) {
                    continue;
                }

                s.detected_apps.push((session.process_id, session.process_name.clone()));
                
                let display = format!("{} (PID: {})", session.process_name, session.process_id);
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

    // Refresh excluded list too
    refresh_excluded_list(hwnd);
}

unsafe fn refresh_excluded_list(hwnd: HWND) {
    let list_excluded = get_dlg_item(hwnd, ID_LIST_EXCLUDED);
    
    // Clear list
    SendMessageW(list_excluded, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let excluded: Vec<_> = s.config.read().excluded_apps.iter().cloned().collect();
            
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
        ID_BTN_REFRESH => {
            refresh_detected_apps(hwnd);
        }
        ID_BTN_ADD_EXCLUSION => {
            add_selected_to_exclusions(hwnd);
        }
        ID_BTN_REMOVE_EXCLUSION => {
            remove_selected_exclusion(hwnd);
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
