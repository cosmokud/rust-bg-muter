//! Native Win32 settings dialog.
//!
//! GDI only (CPU rasterized) - zero GPU/VRAM. All layout numbers are DIPs scaled by
//! GetDpiForWindow, so per-monitor DPI v2 keeps the 4px grid intact.
//!
//! Control IDs and WM_COMMAND branches are unchanged from the pre-owner-draw dialog:
//! toggles answer BM_SETCHECK/BM_GETCHECK and emit BN_CLICKED.

use crate::audio::AudioManager;
use crate::config::Config;
use crate::od_controls::{self, to_wide, Gdi};
use crate::startup;
use crate::theme::{apply_chrome, Dip, Palette};
use parking_lot::RwLock;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    SetBkColor, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    FF_DONTCARE, FW_NORMAL, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_TT_PRECIS, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, SetWindowTheme, DRAWITEMSTRUCT, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
    EM_SETCUEBANNER, ODS_FOCUS, ODS_SELECTED,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::Shell::ExtractIconExW;
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

const EN_CHANGE: u16 = 0x0300;

// Window size in DIPs; clamped to 90% of the work area when created.
const WINDOW_WIDTH: f32 = 920.0;
const WINDOW_HEIGHT: f32 = 680.0;
const MIN_WINDOW_WIDTH: f32 = 780.0;
const MIN_WINDOW_HEIGHT: f32 = 560.0;

const BST_CHECKED: usize = 1;
const BST_UNCHECKED: usize = 0;

thread_local! {
    static DIALOG_STATE: RefCell<Option<DialogState>> = const { RefCell::new(None) };
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

struct DialogState {
    config: Arc<RwLock<Config>>,
    muting_enabled: Arc<AtomicBool>,
    audio_manager: Arc<AudioManager>,
    detected_apps: Vec<(u32, String)>, // (pid, name)
    all_detected_apps: Vec<(u32, String)>, // all apps before filtering
    search_filter: String,
}

/// Per-dialog GDI resources; freed when the dialog closes.
struct Ui {
    palette: Palette,
    dip: Dip,
    font: HFONT,
    font_small: HFONT,
    br_window: HBRUSH,
    br_card: HBRUSH,
    br_well: HBRUSH,
    /// Small-icon cache keyed by exe path; icons are owned and destroyed here.
    icons: HashMap<String, HICON>,
}

impl Ui {
    unsafe fn new(hwnd: HWND) -> Self {
        let dip = Dip::at(hwnd);
        let palette = Palette::system();
        Self {
            palette,
            dip,
            font: make_font(dip, 13.0, FW_NORMAL.0 as i32),
            font_small: make_font(dip, 11.0, FW_NORMAL.0 as i32),
            br_window: CreateSolidBrush(COLORREF(palette.window)),
            br_card: CreateSolidBrush(COLORREF(palette.card)),
            br_well: CreateSolidBrush(COLORREF(palette.well)),
            icons: HashMap::new(),
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        unsafe {
            for (_, icon) in self.icons.drain() {
                let _ = DestroyIcon(icon);
            }
            let _ = DeleteObject(HGDIOBJ(self.font.0));
            let _ = DeleteObject(HGDIOBJ(self.font_small.0));
            let _ = DeleteObject(HGDIOBJ(self.br_window.0));
            let _ = DeleteObject(HGDIOBJ(self.br_card.0));
            let _ = DeleteObject(HGDIOBJ(self.br_well.0));
        }
    }
}

/// Palette used by owner-drawn child windows during their own WM_PAINT.
pub fn current_palette() -> Palette {
    UI.with(|u| u.borrow().as_ref().map(|ui| ui.palette))
        .unwrap_or_else(Palette::system)
}

fn with_ui<R>(f: impl FnOnce(&Ui) -> R) -> Option<R> {
    UI.with(|u| u.borrow().as_ref().map(f))
}

fn with_ui_mut<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.borrow_mut().as_mut().map(f))
}

fn dip_of() -> Dip {
    with_ui(|u| u.dip).unwrap_or(Dip::from_dpi(96))
}

fn ui_font() -> HFONT {
    with_ui(|u| u.font).unwrap_or(HFONT(std::ptr::null_mut()))
}

/// Negative height = character cell height in pixels.
unsafe fn make_font(dip: Dip, size: f32, weight: i32) -> HFONT {
    let name = to_wide("Segoe UI");
    CreateFontW(
        -dip.px(size),
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_TT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
        PCWSTR(name.as_ptr()),
    )
}

fn init_common_controls() {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }
}

unsafe fn get_dlg_item(hwnd: HWND, id: i32) -> HWND {
    GetDlgItem(hwnd, id).unwrap_or_default()
}

fn invalidate(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(hwnd, None, false);
    }
}

/// Opens the settings dialog (blocks until closed)
pub fn open_settings_dialog(config: Arc<RwLock<Config>>, muting_enabled: Arc<AtomicBool>) {
    let audio_manager = match AudioManager::new() {
        Ok(am) => Arc::new(am),
        Err(e) => {
            log::error!("Failed to create audio manager for settings: {}", e);
            return;
        }
    };

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

    init_common_controls();

    unsafe {
        let class_name = to_wide("BgMuterSettingsClass");
        let hmodule = GetModuleHandleW(None).unwrap_or_default();

        od_controls::register_classes(hmodule.into());

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(window_proc),
            hInstance: hmodule.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            // Every pixel is painted by us; no system erase brush.
            hbrBackground: HBRUSH::default(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..WNDCLASSEXW::default()
        };
        RegisterClassExW(&wc);

        let (width, height, x, y) = initial_geometry();
        let title = to_wide("Background Muter - Settings");

        let hwnd_result = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_TOPMOST,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPED
                | WS_CAPTION
                | WS_SYSMENU
                | WS_VISIBLE
                | WS_THICKFRAME
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_CLIPCHILDREN,
            x,
            y,
            width,
            height,
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

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if !IsWindow(hwnd).as_bool() {
                break;
            }
        }

        let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), hmodule);
        UI.with(|u| *u.borrow_mut() = None);
    }

    DIALOG_STATE.with(|state| {
        *state.borrow_mut() = None;
    });
}

fn initial_geometry() -> (i32, i32, i32, i32) {
    unsafe {
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut work as *mut RECT as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let dip = Dip::from_dpi(windows::Win32::UI::HiDpi::GetDpiForSystem());
        let avail_w = (work.right - work.left).max(1);
        let avail_h = (work.bottom - work.top).max(1);
        // Keep the whole dialog on small or heavily scaled screens.
        let width = dip.px(WINDOW_WIDTH).min(avail_w * 9 / 10);
        let height = dip.px(WINDOW_HEIGHT).min(avail_h * 9 / 10);
        let x = work.left + (avail_w - width) / 2;
        let y = work.top + (avail_h - height) / 2;
        (width, height, x, y)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Button,
    Toggle,
    Label,
    Edit,
    List,
    Card,
}

struct Ctl {
    id: i32,
    kind: Kind,
    text: String,
    rect: RECT,
}

/// Single source of truth for control geometry: creation, WM_SIZE and DPI changes all
/// go through here so the DIP grid cannot drift between paths.
fn layout(hwnd: HWND) -> Vec<Ctl> {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    let width = rc.right - rc.left;
    let height = rc.bottom - rc.top;
    let s = |v: f32| dip_of().px(v);

    let margin = s(12.0);
    let left_panel_width = (width - margin * 3) / 2;
    let right_panel_x = margin * 2 + left_panel_width;
    let right_panel_width = width - right_panel_x - margin;

    // Cards fill whatever height the window has; the lower sections are fixed.
    let detected_group_height = (height - s(130.0) - s(75.0) - s(50.0) - s(30.0)).max(s(200.0));
    let right_panel_height = (detected_group_height - s(20.0)) / 2;
    let buttons_y = detected_group_height + s(15.0);
    let settings_y = buttons_y + s(75.0);
    let settings_height = s(130.0);
    let card_y = s(4.0);
    let card_h = detected_group_height;
    let row = s(24.0);

    let mut out: Vec<Ctl> = Vec::new();
    let mut push = |id: i32, kind: Kind, text: &str, x: i32, y: i32, w: i32, h: i32| {
        out.push(Ctl {
            id,
            kind,
            text: text.to_string(),
            rect: RECT {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
        });
    };

    // Cards first: sibling z-order puts the controls created after them on top.
    push(ID_GRP_DETECTED, Kind::Card, "Detected Audio Apps", margin, card_y, left_panel_width, card_h);
    push(ID_GRP_EXCLUDED, Kind::Card, "Excluded Apps (Always Audible)", right_panel_x, card_y, right_panel_width, right_panel_height);
    push(
        ID_GRP_ALWAYS_MUTED,
        Kind::Card,
        "Always Muted",
        right_panel_x,
        card_y + right_panel_height + s(8.0),
        right_panel_width,
        right_panel_height,
    );
    push(ID_GROUP_SETTINGS, Kind::Card, "Settings", margin, settings_y, width - margin * 2, settings_height);

    push(ID_LABEL_SEARCH, Kind::Label, "Search:", margin + s(12.0), card_y + s(32.0), s(50.0), s(20.0));
    push(ID_EDIT_SEARCH, Kind::Edit, "", margin + s(64.0), card_y + s(30.0), left_panel_width - s(76.0), s(24.0));
    push(
        ID_LIST_DETECTED,
        Kind::List,
        "",
        margin + s(12.0),
        card_y + s(60.0),
        left_panel_width - s(24.0),
        card_h - s(60.0) - s(36.0),
    );
    push(ID_BTN_REFRESH, Kind::Button, "Refresh List", margin + s(12.0), card_y + card_h - s(30.0), s(100.0), s(26.0));

    push(
        ID_LIST_EXCLUDED,
        Kind::List,
        "",
        right_panel_x + s(12.0),
        card_y + s(36.0),
        right_panel_width - s(24.0),
        right_panel_height - s(48.0),
    );
    push(
        ID_LIST_ALWAYS_MUTED,
        Kind::List,
        "",
        right_panel_x + s(12.0),
        card_y + right_panel_height + s(8.0) + s(32.0),
        right_panel_width - s(24.0),
        right_panel_height - s(44.0),
    );

    push(ID_BTN_ADD_EXCLUSION, Kind::Button, "Add to Exclusions \u{2192}", margin + s(12.0), buttons_y, s(180.0), row + s(4.0));
    push(ID_BTN_ADD_ALWAYS_MUTED, Kind::Button, "Add to Always Muted \u{2192}", margin + s(12.0), buttons_y + row + s(8.0), s(180.0), row + s(4.0));
    push(
        ID_BTN_REMOVE_EXCLUSION,
        Kind::Button,
        "\u{2190} Remove from Exclusions",
        right_panel_x + right_panel_width - s(220.0),
        buttons_y,
        s(220.0),
        row + s(4.0),
    );
    push(
        ID_BTN_REMOVE_ALWAYS_MUTED,
        Kind::Button,
        "\u{2190} Remove from Always Muted",
        right_panel_x + right_panel_width - s(240.0),
        buttons_y + row + s(8.0),
        s(240.0),
        row + s(4.0),
    );

    push(ID_CHECK_ENABLED, Kind::Toggle, "Muting Enabled", margin + s(16.0), settings_y + s(26.0), s(200.0), s(22.0));
    push(ID_CHECK_START_MINIMIZED, Kind::Toggle, "Start Minimized to Tray", margin + s(16.0), settings_y + s(50.0), s(200.0), s(22.0));
    push(ID_CHECK_START_WINDOWS, Kind::Toggle, "Start with Windows", margin + s(16.0), settings_y + s(74.0), s(200.0), s(22.0));

    push(ID_LABEL_POLL, Kind::Label, "Poll Interval:", width - s(268.0), settings_y + s(32.0), s(90.0), s(20.0));
    push(ID_EDIT_POLL_INTERVAL, Kind::Edit, "", width - s(176.0), settings_y + s(30.0), s(60.0), s(24.0));
    push(ID_LABEL_MS, Kind::Label, "ms", width - s(108.0), settings_y + s(32.0), s(25.0), s(20.0));
    push(ID_LABEL_RANGE, Kind::Label, "Range: 100-2000 ms", width - s(268.0), settings_y + s(58.0), s(180.0), s(18.0));
    push(ID_LABEL_CONFIG, Kind::Label, "Config file:", margin + s(16.0), settings_y + s(102.0), s(70.0), s(18.0));
    let path = Config::config_path().to_string_lossy().to_string();
    push(ID_LABEL_PATH, Kind::Label, &path, margin + s(88.0), settings_y + s(102.0), width - margin * 2 - s(96.0), s(18.0));

    push(ID_BTN_SAVE_ONLY, Kind::Button, "Save", width - s(330.0), height - s(45.0), s(95.0), s(32.0));
    push(ID_BTN_SAVE_CLOSE, Kind::Button, "Save && Close", width - s(225.0), height - s(45.0), s(105.0), s(32.0));
    push(ID_BTN_CLOSE, Kind::Button, "Cancel", width - s(112.0), height - s(45.0), s(100.0), s(32.0));

    out
}

unsafe fn create_controls(hwnd: HWND) {
    let hmodule = GetModuleHandleW(None).unwrap_or_default();
    let font = ui_font();
    let p = current_palette();

    let mut lists: Vec<HWND> = Vec::new();

    for ctl in layout(hwnd) {
        let (class, style) = match ctl.kind {
            Kind::Card => (
                od_controls::CLASS_CARD,
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            ),
            Kind::Toggle => (
                od_controls::CLASS_TOGGLE,
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_CLIPSIBLINGS,
            ),
            Kind::Button => (
                "BUTTON",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_CLIPSIBLINGS | WINDOW_STYLE(BS_OWNERDRAW as u32),
            ),
            Kind::Label => ("STATIC", WS_CHILD | WS_VISIBLE),
            Kind::Edit => (
                "EDIT",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            ),
            Kind::List => (
                "LISTBOX",
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | WS_HSCROLL | WS_BORDER
                    | WINDOW_STYLE((LBS_NOTIFY | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS) as u32),
            ),
        };

        let mut style = style;
        if ctl.id == ID_EDIT_POLL_INTERVAL {
            style = WINDOW_STYLE(style.0 | ES_NUMBER as u32);
        }

        let class_w = to_wide(class);
        let text_w = to_wide(&ctl.text);
        let created = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_w.as_ptr()),
            PCWSTR(text_w.as_ptr()),
            style,
            ctl.rect.left,
            ctl.rect.top,
            ctl.rect.right - ctl.rect.left,
            ctl.rect.bottom - ctl.rect.top,
            hwnd,
            HMENU(ctl.id as *mut c_void),
            hmodule,
            None,
        );

        if let Ok(ctl_hwnd) = created {
            set_font(ctl_hwnd, font);
            match ctl.kind {
                Kind::Button => od_controls::attach_hover(ctl_hwnd),
                Kind::List => {
                    lists.push(ctl_hwnd);
                    let _ = SendMessageW(
                        ctl_hwnd,
                        LB_SETITEMHEIGHT,
                        WPARAM(0),
                        LPARAM(dip_of().px(24.0) as isize),
                    );
                    let _ = SendMessageW(
                        ctl_hwnd,
                        LB_SETHORIZONTALEXTENT,
                        WPARAM(dip_of().px(600.0) as usize),
                        LPARAM(0),
                    );
                }
                Kind::Toggle => {
                    // The toggle draws its own background, so it needs no parent erase.
                    let _ = InvalidateRect(ctl_hwnd, None, false);
                }
                _ => {}
            }
        }
    }

    // Cue banner on the search box (native EDIT supports it).
    let search = get_dlg_item(hwnd, ID_EDIT_SEARCH);
    if !search.0.is_null() {
        let cue = to_wide("Filter by process name");
        let _ = SendMessageW(
            search,
            EM_SETCUEBANNER,
            WPARAM(1),
            LPARAM(cue.as_ptr() as isize),
        );
    }

    // Dark scrollbars: documented uxtheme class name; ignored if unavailable.
    if p.dark {
        let dark = to_wide("DarkMode_Explorer");
        for list in lists {
            let _ = SetWindowTheme(list, PCWSTR(dark.as_ptr()), PCWSTR::null());
        }
    }
}

unsafe fn resize_controls(hwnd: HWND) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    if rc.right < 100 || rc.bottom < 100 {
        return; // minimized
    }
    for ctl in layout(hwnd) {
        let ctrl = get_dlg_item(hwnd, ctl.id);
        if !ctrl.0.is_null() {
            let _ = SetWindowPos(
                ctrl,
                None,
                ctl.rect.left,
                ctl.rect.top,
                ctl.rect.right - ctl.rect.left,
                ctl.rect.bottom - ctl.rect.top,
                SWP_NOZORDER,
            );
        }
    }
    invalidate(hwnd);
}

unsafe fn set_font(hwnd: HWND, font: HFONT) {
    SendMessageW(hwnd, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
}

/// Window background. A memory DC was measured at ~2.5 MB resident for the full
/// client area, and WS_CLIPCHILDREN + WM_ERASEBKGND=1 already keeps child paints from
/// flickering, so the flat fill is drawn straight to the window DC.
unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    if rc.right > 0 && rc.bottom > 0 {
        FillRect(hdc, &ps.rcPaint, HBRUSH(with_ui(|u| u.br_window).unwrap_or_default().0));
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            UI.with(|u| *u.borrow_mut() = Some(Ui::new(hwnd)));
            with_ui(|u| apply_chrome(hwnd, u.palette.dark));
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
            resize_controls(hwnd);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let new_dpi = (wparam.0 & 0xFFFF) as u32;
            if let Some(suggested) = (lparam.0 as *const RECT).as_ref() {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            reinit_for_dpi(hwnd, new_dpi);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1), // fully painted in WM_PAINT
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let p = current_palette();
            let hdc = HDC(wparam.0 as *mut c_void);
            SetTextColor(hdc, COLORREF(p.text));
            SetBkMode(hdc, TRANSPARENT);
            LRESULT(with_ui(|u| u.br_card).unwrap_or_default().0 as isize)
        }
        WM_CTLCOLOREDIT => {
            let p = current_palette();
            let hdc = HDC(wparam.0 as *mut c_void);
            SetTextColor(hdc, COLORREF(p.text));
            SetBkColor(hdc, COLORREF(p.well));
            LRESULT(with_ui(|u| u.br_well).unwrap_or_default().0 as isize)
        }

        WM_CTLCOLORLISTBOX => {
            let p = current_palette();
            SetBkColor(HDC(wparam.0 as *mut c_void), COLORREF(p.well));
            LRESULT(with_ui(|u| u.br_well).unwrap_or_default().0 as isize)
        }
        WM_DRAWITEM => {
            draw_item(lparam);
            LRESULT(1)
        }
        WM_GETMINMAXINFO => {
            let mmi = lparam.0 as *mut MINMAXINFO;
            if !mmi.is_null() {
                let dip = dip_of();
                (*mmi).ptMinTrackSize.x = dip.px(MIN_WINDOW_WIDTH);
                (*mmi).ptMinTrackSize.y = dip.px(MIN_WINDOW_HEIGHT);
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

unsafe fn reinit_for_dpi(hwnd: HWND, dpi: u32) {
    with_ui_mut(|ui| {
        let dip = Dip::from_dpi(dpi);
        let _ = DeleteObject(HGDIOBJ(ui.font.0));
        let _ = DeleteObject(HGDIOBJ(ui.font_small.0));
        ui.dip = dip;
        ui.font = make_font(dip, 13.0, FW_NORMAL.0 as i32);
        ui.font_small = make_font(dip, 11.0, FW_NORMAL.0 as i32);
    });
    let font = ui_font();
    for ctl in layout(hwnd) {
        let ctrl = get_dlg_item(hwnd, ctl.id);
        if !ctrl.0.is_null() {
            set_font(ctrl, font);
            if ctl.kind == Kind::List {
                let _ = SendMessageW(ctrl, LB_SETITEMHEIGHT, WPARAM(0), LPARAM(dip_of().px(24.0) as isize));
            }
        }
    }
    resize_controls(hwnd);
}

unsafe fn draw_item_bg(hdc: HDC, rc: RECT) {
    let p = current_palette();
    let mut g = Gdi::default();
    g.fill(hdc, &rc, p.well);
}

unsafe fn draw_item(lparam: LPARAM) {
    let dis = lparam.0 as *const DRAWITEMSTRUCT;
    let Some(dis) = dis.as_ref() else { return };
    if dis.CtlID == 0 {
        return;
    }
    match dis.CtlID as i32 {
        ID_LIST_DETECTED | ID_LIST_EXCLUDED | ID_LIST_ALWAYS_MUTED => draw_list_item(dis),
        _ => draw_button(dis),
    }
}

/// Background surface under a control: the card it overlaps, else the window.
fn surface_under(parent: HWND, rc: RECT) -> u32 {
    let p = current_palette();
    let center = windows::Win32::Foundation::POINT {
        x: (rc.left + rc.right) / 2,
        y: (rc.top + rc.bottom) / 2,
    };
    let on_card = layout(parent).iter().any(|c| {
        c.kind == Kind::Card
            && center.x >= c.rect.left
            && center.x <= c.rect.right
            && center.y >= c.rect.top
            && center.y <= c.rect.bottom
    });
    if on_card {
        p.card
    } else {
        p.window
    }
}

unsafe fn draw_button(dis: &DRAWITEMSTRUCT) {
    let p = current_palette();
    let dip = dip_of();
    let s = |v: f32| dip.px(v);
    let mut g = Gdi::default();
    let rc = dis.rcItem;
    let label = od_controls::window_text(dis.hwndItem);
    let primary = dis.CtlID as i32 == ID_BTN_SAVE_CLOSE;
    let hot = od_controls::is_hot(dis.hwndItem);
    let pressed = (dis.itemState.0 & ODS_SELECTED.0) != 0;
    let focused = (dis.itemState.0 & ODS_FOCUS.0) != 0 || GetFocus() == dis.hwndItem;
    let small = with_ui(|u| u.font_small).unwrap_or(HFONT(std::ptr::null_mut()));

    let parent = GetParent(dis.hwndItem).unwrap_or_default();
    g.fill(dis.hDC, &rc, surface_under(parent, rc));

    let (fill, fg, border) = if primary {
        let f = if pressed {
            od_controls::mix(p.accent, 0x000000, 0.25)
        } else if hot {
            od_controls::mix(p.accent, p.text, 0.12)
        } else {
            p.accent
        };
        (f, p.on_accent, None)
    } else {
        let f = if pressed {
            p.pressed
        } else if hot {
            p.hover
        } else {
            p.card
        };
        (f, p.text, Some(if focused { p.accent } else { p.hairline }))
    };

    let inner = RECT {
        left: rc.left + s(2.0),
        top: rc.top + s(3.0),
        right: rc.right - s(2.0),
        bottom: rc.bottom - s(3.0),
    };
    g.round(dis.hDC, &inner, s(6.0), fill, border);
    g.text(
        dis.hDC,
        &inner,
        &label,
        small,
        fg,
        DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_END_ELLIPSIS,
    );
}

unsafe fn draw_list_item(dis: &DRAWITEMSTRUCT) {
    if dis.itemID as usize == usize::MAX {
        // LB_SETITEMHEIGHT / empty list: paint the empty row so the well keeps its color.
        draw_item_bg(dis.hDC, dis.rcItem);
        return;
    }
    let p = current_palette();
    let dip = dip_of();
    let s = |v: f32| dip.px(v);
    let mut g = Gdi::default();
    let rc = dis.rcItem;
    let selected = (dis.itemState.0 & ODS_SELECTED.0) != 0;
    let focused = selected && ((dis.itemState.0 & ODS_FOCUS.0) != 0 || GetFocus() == dis.hwndItem);

    let bg = if selected {
        od_controls::mix(p.well, p.accent, 0.30)
    } else if od_controls::hot_item(dis.hwndItem) == dis.itemID as isize {
        p.hover
    } else {
        p.well
    };
    g.fill(dis.hDC, &rc, bg);
    if focused {
        g.round(dis.hDC, &rc, s(4.0), bg, Some(p.accent));
    }

    let len = SendMessageW(dis.hwndItem, LB_GETTEXTLEN, WPARAM(dis.itemID as usize), LPARAM(0)).0 as usize;
    if len == 0 {
        return;
    }
    let mut buf: Vec<u16> = vec![0; len + 1];
    SendMessageW(
        dis.hwndItem,
        LB_GETTEXT,
        WPARAM(dis.itemID as usize),
        LPARAM(buf.as_mut_ptr() as isize),
    );
    let text = String::from_utf16_lossy(&buf[..len]);

    let icon_size = s(16.0);
    let pid = SendMessageW(dis.hwndItem, LB_GETITEMDATA, WPARAM(dis.itemID as usize), LPARAM(0)).0;
    let icon = row_icon(pid.max(0) as u32, &text);

    let text_left;
    if let Some(icon) = icon {
        let y = rc.top + ((rc.bottom - rc.top) - icon_size) / 2;
        let _ = DrawIconEx(dis.hDC, rc.left + s(6.0), y, icon, icon_size, icon_size, 0, HBRUSH::default(), DI_NORMAL);
        text_left = rc.left + s(6.0) + icon_size + s(8.0);
    } else {
        text_left = rc.left + s(8.0);
    }

    let text_rc = RECT {
        left: text_left,
        top: rc.top,
        right: rc.right - s(4.0),
        bottom: rc.bottom,
    };
    g.text(
        dis.hDC,
        &text_rc,
        &text,
        ui_font(),
        p.text,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
}

/// Small icon for a list row. `pid` is the real process (detected list); otherwise the
/// text is a process name and the generic executable icon is used.
unsafe fn row_icon(pid: u32, text: &str) -> Option<HICON> {
    let path = if pid > 0 {
        process_image_path(pid).unwrap_or_else(|| text.to_string())
    } else {
        text.to_string()
    };

    if let Some(icon) = with_ui(|u| u.icons.get(&path).copied()).flatten() {
        return Some(icon);
    }

    // shortcut: ExtractIconEx only, SHGetFileInfoW costs ~7 MB of shell heap per dialog,
    // measured; revisit if a per-extension icon (non-exe names) is needed.
    let wide = to_wide(&path);
    let mut large = HICON::default();
    let mut small = HICON::default();
    let count = ExtractIconExW(PCWSTR(wide.as_ptr()), 0, Some(&mut large), Some(&mut small), 1);
    if !large.0.is_null() {
        let _ = DestroyIcon(large);
    }
    if count == 0 || small.0.is_null() {
        return None;
    }
    with_ui_mut(|u| {
        u.icons.insert(path, small);
    });
    Some(small)
}

/// Full exe path of a running process, capped so a dead PID costs one syscall.
unsafe fn process_image_path(pid: u32) -> Option<String> {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buf = [0u16; 260];
    let mut size = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(
        process,
        PROCESS_NAME_WIN32,
        windows::core::PWSTR(buf.as_mut_ptr()),
        &mut size,
    )
    .is_ok();
    let _ = windows::Win32::Foundation::CloseHandle(process);
    (ok && size > 0).then(|| String::from_utf16_lossy(&buf[..size as usize]))
}

unsafe fn refresh_detected_apps(hwnd: HWND) {
    let list_detected = get_dlg_item(hwnd, ID_LIST_DETECTED);

    SendMessageW(list_detected, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    DIALOG_STATE.with(|state| {
        if let Some(ref mut s) = *state.borrow_mut() {
            let sessions = s.audio_manager.get_sessions();
            s.all_detected_apps.clear();
            s.detected_apps.clear();

            let config = s.config.read();
            let excluded = config.excluded_apps.clone();
            let always_muted = config.always_muted_apps.clone();
            drop(config);

            let mut apps: Vec<(u32, String)> = Vec::new();
            for session in sessions {
                if excluded.contains(&session.process_name.to_lowercase()) {
                    continue;
                }
                if always_muted.contains(&session.process_name.to_lowercase()) {
                    continue;
                }
                apps.push((session.process_id, session.process_name.clone()));
            }

            apps.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
            s.all_detected_apps = apps;

            let filter = s.search_filter.to_lowercase();
            for (pid, name) in &s.all_detected_apps {
                if filter.is_empty() || name.to_lowercase().contains(&filter) {
                    s.detected_apps.push((*pid, name.clone()));
                    let display = format!("{} (PID: {})", name, pid);
                    let wide = to_wide(&display);
                    let idx = SendMessageW(
                        list_detected,
                        LB_ADDSTRING,
                        WPARAM(0),
                        LPARAM(wide.as_ptr() as isize),
                    );
                    if idx.0 >= 0 {
                        // Item data = PID, so the owner-draw can resolve the real icon.
                        SendMessageW(
                            list_detected,
                            LB_SETITEMDATA,
                            WPARAM(idx.0 as usize),
                            LPARAM(*pid as isize),
                        );
                    }
                }
            }
        }
    });

    refresh_excluded_list(hwnd);
    refresh_always_muted_list(hwnd);
}

unsafe fn apply_search_filter(hwnd: HWND) {
    let list_detected = get_dlg_item(hwnd, ID_LIST_DETECTED);

    SendMessageW(list_detected, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    DIALOG_STATE.with(|state| {
        if let Some(ref mut s) = *state.borrow_mut() {
            s.detected_apps.clear();

            let filter = s.search_filter.to_lowercase();
            for (pid, name) in &s.all_detected_apps {
                if filter.is_empty() || name.to_lowercase().contains(&filter) {
                    s.detected_apps.push((*pid, name.clone()));
                    let display = format!("{} (PID: {})", name, pid);
                    let wide = to_wide(&display);
                    let idx = SendMessageW(
                        list_detected,
                        LB_ADDSTRING,
                        WPARAM(0),
                        LPARAM(wide.as_ptr() as isize),
                    );
                    if idx.0 >= 0 {
                        SendMessageW(
                            list_detected,
                            LB_SETITEMDATA,
                            WPARAM(idx.0 as usize),
                            LPARAM(*pid as isize),
                        );
                    }
                }
            }
        }
    });
}

unsafe fn refresh_excluded_list(hwnd: HWND) {
    let list_excluded = get_dlg_item(hwnd, ID_LIST_EXCLUDED);

    SendMessageW(list_excluded, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let mut excluded: Vec<_> = s.config.read().excluded_apps.iter().cloned().collect();
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

    SendMessageW(list_always_muted, LB_RESETCONTENT, WPARAM(0), LPARAM(0));

    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let mut always_muted: Vec<_> = s.config.read().always_muted_apps.iter().cloned().collect();
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

            set_check(hwnd, ID_CHECK_ENABLED, muting_enabled);
            set_check(hwnd, ID_CHECK_START_MINIMIZED, config.start_minimized);
            set_check(hwnd, ID_CHECK_START_WINDOWS, config.start_with_windows);

            let poll_str = to_wide(&config.poll_interval_ms.to_string());
            let _ = SetWindowTextW(get_dlg_item(hwnd, ID_EDIT_POLL_INTERVAL), PCWSTR(poll_str.as_ptr()));
        }
    });
}

unsafe fn set_check(hwnd: HWND, id: i32, on: bool) {
    SendMessageW(
        get_dlg_item(hwnd, id),
        BM_SETCHECK,
        WPARAM(if on { BST_CHECKED } else { BST_UNCHECKED }),
        LPARAM(0),
    );
}

unsafe fn get_check(hwnd: HWND, id: i32) -> bool {
    SendMessageW(get_dlg_item(hwnd, id), BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == BST_CHECKED as isize
}

unsafe fn handle_command(hwnd: HWND, control_id: i32, notification: u16) {
    match control_id {
        ID_EDIT_SEARCH => {
            if notification == EN_CHANGE {
                let edit_search = get_dlg_item(hwnd, ID_EDIT_SEARCH);
                let mut buffer: [u16; 256] = [0; 256];
                let len = GetWindowTextW(edit_search, &mut buffer);
                let search_text = if len > 0 {
                    String::from_utf16_lossy(&buffer[..len as usize])
                } else {
                    String::new()
                };

                DIALOG_STATE.with(|state| {
                    if let Some(ref mut s) = *state.borrow_mut() {
                        s.search_filter = search_text;
                    }
                });

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

            let muting_checked = get_check(hwnd, ID_CHECK_ENABLED);
            let start_minimized_checked = get_check(hwnd, ID_CHECK_START_MINIMIZED);
            let start_windows_checked = get_check(hwnd, ID_CHECK_START_WINDOWS);

            let mut buffer: [u16; 32] = [0; 32];
            GetWindowTextW(get_dlg_item(hwnd, ID_EDIT_POLL_INTERVAL), &mut buffer);
            let poll_str = String::from_utf16_lossy(&buffer);
            let poll_str = poll_str.trim_matches('\0').trim();
            let poll_interval: u64 = poll_str.parse().unwrap_or(500).clamp(100, 2000);

            config.muting_enabled = muting_checked;
            config.start_minimized = start_minimized_checked;
            config.poll_interval_ms = poll_interval;

            if config.start_with_windows != start_windows_checked {
                config.start_with_windows = start_windows_checked;
                if let Err(e) = startup::set_run_at_startup(start_windows_checked) {
                    log::error!("Failed to update startup setting: {}", e);
                }
            }

            s.muting_enabled.store(muting_checked, Ordering::SeqCst);
            let _ = config.save();
        }
    });
}
