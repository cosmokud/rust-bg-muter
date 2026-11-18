//! Native Win32 settings dialog.
//!
//! GDI only (CPU rasterized) - zero GPU/VRAM. All layout numbers are DIPs scaled by
//! GetDpiForWindow, so per-monitor DPI v2 keeps the 4px grid intact.
//!
//! One window, no children: every control in `layout()` is painted by `paint()` from a
//! thread-local memory bitmap and blitted in one `BitBlt` per frame. Native children paint
//! on their own schedule, and Windows repaints only the exposed bands during a live resize,
//! so children ghost and trail structurally - no invalidation scheme can fix that. With one
//! surface, every WM_SIZE ends on exactly one complete frame.
//! WS_EX_COMPOSITED is deliberately absent: DWM compositing a single already-double-buffered
//! window buys nothing and costs a full extra copy per frame.

use crate::audio::AudioManager;
use crate::config::Config;
use crate::od_controls::{self, to_wide};
use crate::startup;
use crate::theme::{apply_chrome, Dip, Palette};
use parking_lot::{Mutex, RwLock};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use windows::core::PCWSTR;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, DeleteDC,
    DeleteObject, EndPaint, GetDC, GetTextExtentPoint32W, IntersectClipRect, InvalidateRect,
    ReleaseDC, RestoreDC, SaveDC, SelectObject, SetBrushOrgEx, SetStretchBltMode,
    StretchBlt, UpdateWindow, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    FF_DONTCARE, FW_NORMAL, HALFTONE, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_TT_PRECIS,
    PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Controls::WM_MOUSELEAVE as CONTROLS_MOUSELEAVE;
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
    VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR,
    VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
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
const ID_EDIT_FILTER_EXCLUDED: i32 = 127;
const ID_EDIT_FILTER_ALWAYS_MUTED: i32 = 128;
const ID_BTN_EDIT_CONFIG: i32 = 129;

const EN_CHANGE: u16 = 0x0300;

// Window size in DIPs; clamped to 90% of the work area when created.
const WINDOW_WIDTH: f32 = 920.0;
const WINDOW_HEIGHT: f32 = 680.0;
const MIN_WINDOW_WIDTH: f32 = 780.0;
const MIN_WINDOW_HEIGHT: f32 = 560.0;

thread_local! {
    static DIALOG_STATE: RefCell<Option<DialogState>> = const { RefCell::new(None) };
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };

}

struct DialogState {
    config: Arc<RwLock<Config>>,
    muting_enabled: Arc<AtomicBool>,
    audio_manager: Arc<AudioManager>,
    detected_apps: Vec<(u32, String)>,     // (pid, name)
    all_detected_apps: Vec<(u32, String)>, // all apps before filtering
    // Lowercased filters, one per list; the lists are substring-matched against them.
    filter_detected: String,
    filter_excluded: String,
    filter_always_muted: String,
}

/// One editable field: text plus a selection. `caret`/`anchor` are char indices, not
/// bytes, so editing never splits a surrogate pair.
#[derive(Clone, Default)]
struct Field {
    text: String,
    caret: usize,
    anchor: usize,
}

impl Field {
    /// Selection as (start, end), caret-first order.
    fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }
}

#[derive(Clone, Default, PartialEq)]
struct Row {
    text: String,
    /// PID in the detected list, 0 in the named lists: what the icon lookup keys on.
    data: isize,
}

/// Scroll/selection of one list; `rows` is the whole content, `top` the first visible row.
#[derive(Default)]
struct ListView {
    rows: Vec<Row>,
    top: usize,
    sel: Option<usize>,
    hot: Option<usize>,
}

impl ListView {
    fn selected_text(&self) -> Option<String> {
        self.sel
            .and_then(|i| self.rows.get(i))
            .map(|r| r.text.clone())
    }

    /// Replace rows, keeping the selection when the same text survives; clamp scroll.
    fn set_rows(&mut self, rows: Vec<Row>, visible: usize) {
        let keep = self.selected_text();
        self.rows = rows;
        self.sel = keep.and_then(|t| self.rows.iter().position(|r| r.text == t));
        self.hot = self.hot.filter(|i| *i < self.rows.len());
        self.clamp(visible);
    }

    fn clamp(&mut self, visible: usize) {
        let max_top = self.rows.len().saturating_sub(visible.max(1));
        self.top = self.top.min(max_top);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Focus {
    #[default]
    None,
    Field(i32),
    List(i32),
    Button(i32),
    Toggle(i32),
}

thread_local! {
    static FIELDS: RefCell<HashMap<i32, Field>> = RefCell::new(HashMap::new());
    static LISTS: RefCell<HashMap<i32, ListView>> = RefCell::new(HashMap::new());
    static TOGGLES: RefCell<HashMap<i32, bool>> = RefCell::new(HashMap::new());
    // Hot control id, 0 = none; the hot row of a list lives in that list's `ListView`.
    static HOT: Cell<i32> = const { Cell::new(0) };
    // Button/toggle held down by the current mouse drag, 0 = none.
    static PRESSED: Cell<i32> = const { Cell::new(0) };
    static FOCUS: Cell<Focus> = const { Cell::new(Focus::None) };
    static CARET_ON: Cell<bool> = const { Cell::new(false) };
}

fn with_field<R>(id: i32, f: impl FnOnce(&mut Field) -> R) -> R {
    FIELDS.with(|fields| {
        let mut fields = fields.borrow_mut();
        f(fields.entry(id).or_default())
    })
}

fn field_text(id: i32) -> String {
    with_field(id, |f| f.text.clone())
}

/// Replace field text, caret at the end, selection collapsed.
fn set_field(id: i32, text: String) {
    with_field(id, |f| {
        f.text = text;
        f.caret = f.text.chars().count();
        f.anchor = f.caret;
    });
}

fn with_list<R>(id: i32, f: impl FnOnce(&mut ListView) -> R) -> R {
    LISTS.with(|lists| {
        let mut lists = lists.borrow_mut();
        f(lists.entry(id).or_default())
    })
}

fn list_sel(id: i32) -> Option<usize> {
    with_list(id, |l| l.sel)
}

fn toggle(id: i32) -> bool {
    TOGGLES.with(|t| t.borrow().get(&id).copied().unwrap_or(false))
}

fn set_toggle(id: i32, on: bool) {
    TOGGLES.with(|t| t.borrow_mut().insert(id, on));
}

fn focus() -> Focus {
    FOCUS.with(|f| f.get())
}

fn set_focus_value(focus: Focus) {
    FOCUS.with(|f| f.set(focus));
}

/// GDI resources for one dialog; freed when it closes. Fonts and brushes live in the
/// process-wide `SHARED`, so a reopen neither creates nor deletes any GDI object.
struct Ui {
    palette: Palette,
    dip: Dip,
    font: HFONT,
    font_small: HFONT,
}

/// Process-wide fonts and brushes, keyed by the palette/DPI they were built for.
/// Re-parenting or a DPI change reuses the same handles instead of churning GDI objects.
/// Handles are kept as `isize` so the cache is `Send`; they are handles, never dereferenced.
struct Shared {
    dip: u32,
    dark: bool,
    font: isize,
    font_small: isize,
}

static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();

impl Shared {
    unsafe fn get(dip: u32, palette: &Palette) -> Shared {
        let cache = SHARED.get_or_init(|| Mutex::new(Shared::build(Dip::from_dpi(dip), palette)));
        let mut cache = cache.lock();
        if cache.dip != Dip::from_dpi(dip).dpi || cache.dark != palette.dark {
            // The retired handles are never deleted: a control still selecting one would
            // be left drawing with a freed object. A DPI change is rare; the leak is bounded.
            *cache = Shared::build(Dip::from_dpi(dip), palette);
        }
        Shared { ..*cache }
    }

    unsafe fn build(dip: Dip, palette: &Palette) -> Self {
        Self {
            dip: dip.dpi,
            dark: palette.dark,
            font: make_font(dip, 13.0, FW_NORMAL.0 as i32).0 as isize,
            font_small: make_font(dip, 11.0, FW_NORMAL.0 as i32).0 as isize,
        }
    }
}

impl Ui {
    unsafe fn new(hwnd: HWND) -> Self {
        let dip = Dip::at(hwnd);
        let palette = Palette::system();
        let shared = Shared::get(dip.dpi, &palette);
        Self {
            palette,
            dip,
            font: HFONT(shared.font as *mut c_void),
            font_small: HFONT(shared.font_small as *mut c_void),
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        // Nothing to release: fonts and brushes are the process-wide `SHARED` handles.
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

/// The dialog's UI font: the caption, the buttons, field text and list rows all draw in
/// it, and `od_controls` picks it up when a control answers null from `WM_GETFONT`.
pub fn ui_font() -> HFONT {
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

/// Opens the settings dialog (blocks until closed)
pub fn open_settings_dialog(
    config: Arc<RwLock<Config>>,
    muting_enabled: Arc<AtomicBool>,
    audio_manager: Arc<AudioManager>,
) {
    DIALOG_STATE.with(|state| {
        *state.borrow_mut() = Some(DialogState {
            config,
            muting_enabled,
            audio_manager,
            detected_apps: Vec::new(),
            all_detected_apps: Vec::new(),
            filter_detected: String::new(),
            filter_excluded: String::new(),
            filter_always_muted: String::new(),
        });
    });

    init_common_controls();

    unsafe {
        let Some(hwnd) = create_dialog_window() else {
            DIALOG_STATE.with(|state| {
                *state.borrow_mut() = None;
            });
            return;
        };

        // No IsDialogMessageW: with no children there is no dialog navigation to steal,
        // and the window proc does Tab/Space/Enter itself.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if !IsWindow(hwnd).as_bool() {
                break;
            }
        }

        let class_name = to_wide(SETTINGS_CLASS);
        let _ = UnregisterClassW(
            PCWSTR(class_name.as_ptr()),
            GetModuleHandleW(None).unwrap_or_default(),
        );
        UI.with(|u| *u.borrow_mut() = None);
    }

    DIALOG_STATE.with(|state| {
        *state.borrow_mut() = None;
    });
}

const SETTINGS_CLASS: &str = "BgMuterSettingsClass";

/// Registers the dialog class and creates the window; the caller owns the message loop.
unsafe fn create_dialog_window() -> Option<HWND> {
    let class_name = to_wide(SETTINGS_CLASS);
    let hmodule = GetModuleHandleW(None).unwrap_or_default();

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        // CS_HREDRAW | CS_VREDRAW: a size change invalidates the whole window, so a frame
        // can never keep pixels that belong to the previous size.
        style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
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
    let title = to_wide(&format!("{} - Settings", crate::tray::APP_NAME));

    let hwnd = CreateWindowExW(
        // No WS_EX_COMPOSITED: with one window painting one complete frame per size
        // message, DWM has nothing left to compose and the extra copy per frame is pure
        // overhead. No WS_CLIPCHILDREN either: there are no children to clip.
        // No WS_EX_TOPMOST: the main window stays behind other apps like a normal window.
        WS_EX_DLGMODALFRAME,
        PCWSTR(class_name.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_OVERLAPPED
            | WS_CAPTION
            | WS_SYSMENU
            | WS_VISIBLE
            | WS_THICKFRAME
            | WS_MINIMIZEBOX
            | WS_MAXIMIZEBOX,
        x,
        y,
        width,
        height,
        None,
        None,
        hmodule,
        None,
    );

    let hwnd = match hwnd {
        Ok(h) => h,
        Err(e) => {
            log::error!("Failed to create settings window: {}", e);
            return None;
        }
    };

    let _ = ShowWindow(hwnd, SW_SHOW);
    Some(hwnd)
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

    // Cards first, then their children: the paint pass draws in this order, so a card
    // never covers a control on top of it, and `hit_test` reverses it for the same reason.
    push(
        ID_GRP_DETECTED,
        Kind::Card,
        "Detected Audio Apps",
        margin,
        card_y,
        left_panel_width,
        card_h,
    );
    push(
        ID_GRP_EXCLUDED,
        Kind::Card,
        "Excluded Apps (Always Audible)",
        right_panel_x,
        card_y,
        right_panel_width,
        right_panel_height,
    );
    push(
        ID_GRP_ALWAYS_MUTED,
        Kind::Card,
        "Always Muted",
        right_panel_x,
        card_y + right_panel_height + s(8.0),
        right_panel_width,
        right_panel_height,
    );
    push(
        ID_GROUP_SETTINGS,
        Kind::Card,
        "Settings",
        margin,
        settings_y,
        width - margin * 2,
        settings_height,
    );

    push(
        ID_LABEL_SEARCH,
        Kind::Label,
        "Search:",
        margin + s(12.0),
        card_y + s(32.0),
        s(50.0),
        s(20.0),
    );
    push(
        ID_EDIT_SEARCH,
        Kind::Edit,
        "",
        margin + s(64.0),
        card_y + s(30.0),
        left_panel_width - s(76.0),
        s(24.0),
    );
    push(
        ID_LIST_DETECTED,
        Kind::List,
        "",
        margin + s(12.0),
        card_y + s(60.0),
        left_panel_width - s(24.0),
        card_h - s(60.0) - s(36.0),
    );
    push(
        ID_BTN_REFRESH,
        Kind::Button,
        "Refresh List",
        margin + s(12.0),
        card_y + card_h - s(30.0),
        s(100.0),
        s(26.0),
    );

    // Right cards: caption row, filter edit, then the list; the card bottom is unchanged.
    push(
        ID_EDIT_FILTER_EXCLUDED,
        Kind::Edit,
        "",
        right_panel_x + s(12.0),
        card_y + s(32.0),
        right_panel_width - s(24.0),
        s(24.0),
    );
    push(
        ID_LIST_EXCLUDED,
        Kind::List,
        "",
        right_panel_x + s(12.0),
        card_y + s(60.0),
        right_panel_width - s(24.0),
        right_panel_height - s(72.0),
    );
    let muted_card_y = card_y + right_panel_height + s(8.0);
    push(
        ID_EDIT_FILTER_ALWAYS_MUTED,
        Kind::Edit,
        "",
        right_panel_x + s(12.0),
        muted_card_y + s(32.0),
        right_panel_width - s(24.0),
        s(24.0),
    );
    push(
        ID_LIST_ALWAYS_MUTED,
        Kind::List,
        "",
        right_panel_x + s(12.0),
        muted_card_y + s(60.0),
        right_panel_width - s(24.0),
        right_panel_height - s(72.0),
    );

    push(
        ID_BTN_ADD_EXCLUSION,
        Kind::Button,
        "Add to Exclusions \u{2192}",
        margin + s(12.0),
        buttons_y,
        s(180.0),
        row + s(4.0),
    );
    push(
        ID_BTN_ADD_ALWAYS_MUTED,
        Kind::Button,
        "Add to Always Muted \u{2192}",
        margin + s(12.0),
        buttons_y + row + s(8.0),
        s(180.0),
        row + s(4.0),
    );
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

    push(
        ID_CHECK_ENABLED,
        Kind::Toggle,
        "Muting Enabled",
        margin + s(16.0),
        settings_y + s(26.0),
        s(200.0),
        s(22.0),
    );
    push(
        ID_CHECK_START_MINIMIZED,
        Kind::Toggle,
        "Start Minimized to Tray",
        margin + s(16.0),
        settings_y + s(50.0),
        s(200.0),
        s(22.0),
    );
    push(
        ID_CHECK_START_WINDOWS,
        Kind::Toggle,
        "Start with Windows",
        margin + s(16.0),
        settings_y + s(74.0),
        s(200.0),
        s(22.0),
    );

    push(
        ID_LABEL_POLL,
        Kind::Label,
        "Poll Interval:",
        width - s(268.0),
        settings_y + s(32.0),
        s(90.0),
        s(20.0),
    );
    push(
        ID_EDIT_POLL_INTERVAL,
        Kind::Edit,
        "",
        width - s(176.0),
        settings_y + s(30.0),
        s(60.0),
        s(24.0),
    );
    push(
        ID_LABEL_MS,
        Kind::Label,
        "ms",
        width - s(108.0),
        settings_y + s(32.0),
        s(25.0),
        s(20.0),
    );
    push(
        ID_LABEL_RANGE,
        Kind::Label,
        "Range: 100-2000 ms",
        width - s(268.0),
        settings_y + s(58.0),
        s(180.0),
        s(18.0),
    );
    push(
        ID_LABEL_CONFIG,
        Kind::Label,
        "Config file:",
        margin + s(16.0),
        settings_y + s(102.0),
        s(70.0),
        s(18.0),
    );
    let path = Config::config_path().to_string_lossy().to_string();
    push(
        ID_LABEL_PATH,
        Kind::Label,
        &path,
        margin + s(88.0),
        settings_y + s(102.0),
        width - margin * 2 - s(96.0),
        s(18.0),
    );

    push(
        ID_BTN_EDIT_CONFIG,
        Kind::Button,
        "Edit Config",
        width - s(438.0),
        height - s(45.0),
        s(100.0),
        s(32.0),
    );
    push(
        ID_BTN_SAVE_ONLY,
        Kind::Button,
        "Save",
        width - s(330.0),
        height - s(45.0),
        s(95.0),
        s(32.0),
    );
    push(
        ID_BTN_SAVE_CLOSE,
        Kind::Button,
        "Save && Close",
        width - s(225.0),
        height - s(45.0),
        s(105.0),
        s(32.0),
    );
    push(
        ID_BTN_CLOSE,
        Kind::Button,
        "Cancel",
        width - s(112.0),
        height - s(45.0),
        s(100.0),
        s(32.0),
    );

    out
}

/// Small icons keyed by exe path, process-wide: extracted once per path and kept until
/// exit, so reopening the dialog costs no shell work at all.
/// Icons are handles, not pointers we dereference: kept as `isize` so the cache is `Send`.
static ICONS: OnceLock<Mutex<HashMap<String, isize>>> = OnceLock::new();

/// Per-row icon answers, misses included: a repaint must do no syscall at all, so neither
/// OpenProcess/QueryFullProcessImageNameW nor a toolhelp snapshot runs twice for one row.
static ICON_OF_PID: OnceLock<Mutex<HashMap<u32, Option<isize>>>> = OnceLock::new();
static ICON_OF_NAME: OnceLock<Mutex<HashMap<String, Option<isize>>>> = OnceLock::new();

/// Names that resolved to no running process; a repaint must not rescan for them.
static MISSING_PATHS: OnceLock<Mutex<HashMap<String, ()>>> = OnceLock::new();

/// Small icon for a list row. `pid` is the real process (detected list); otherwise the
/// text is a process name, so the first running process with that name supplies the path.
unsafe fn row_icon(pid: u32, text: &str) -> Option<HICON> {
    let name = match text.find(" (PID: ") {
        Some(cut) => &text[..cut],
        None => text,
    };

    // Locks are dropped before any syscall; only the map probe is inside one.
    if pid > 0 {
        if let Some(hit) = ICON_OF_PID
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .get(&pid)
            .copied()
        {
            return hit.map(|i| HICON(i as *mut c_void));
        }
        let path = process_image_path(pid).or_else(|| path_for_name(name));
        let icon = icon_of_path(path.as_deref().unwrap_or(name));
        ICON_OF_PID
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .insert(pid, icon.map(|h| h.0 as isize));
        return icon;
    }

    if let Some(hit) = ICON_OF_NAME
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .get(name)
        .copied()
    {
        return hit.map(|i| HICON(i as *mut c_void));
    }
    let path = path_for_name(name);
    let icon = icon_of_path(path.as_deref().unwrap_or(name));
    ICON_OF_NAME
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .insert(name.to_string(), icon.map(|h| h.0 as isize));
    icon
}

/// Extracts (once per path) the small icon of an exe. `path` may be a bare name that has
/// no file behind it; PrivateExtractIconsW then just fails and the miss is remembered.
unsafe fn icon_of_path(path: &str) -> Option<HICON> {
    let icons = ICONS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(icon) = icons.lock().get(path) {
        return Some(HICON(*icon as *mut c_void));
    }

    // Small box only, so no large bitmap is built. Measured shell working set on first use:
    // PrivateExtractIconsW 0.33 MB, ExtractIconExW 1.8 MB, SHGetFileInfoW 7 MB - hence this
    // API for real exe paths and no SHGetFileInfoW fallback at all.
    let mut file = [0u16; 260];
    for (i, unit) in path.encode_utf16().take(259).enumerate() {
        file[i] = unit;
    }
    let mut small = [HICON::default(); 1];
    PrivateExtractIconsW(&file, 0, 16, 16, Some(&mut small), None, 0);
    let icon = small[0];
    if icon.0.is_null() {
        return None;
    }
    icons.lock().insert(path.to_string(), icon.0 as isize);
    Some(icon)
}

/// Full exe path of a running process, capped so a dead PID costs one syscall.
unsafe fn process_image_path(pid: u32) -> Option<String> {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
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
// GDI has no antialiasing, so curved edges are supersampled: the shape is rasterized at
// AA_SUPERSAMPLE x into a small off-screen bitmap and downscaled with HALFTONE (a box
// average of the covered pixels) -> sub-pixel pill edges and a smooth knob.
const AA_SUPERSAMPLE: i32 = 8;

pub enum Shape {
    Round,
    Dot,
}

/// Shapes to rasterize supersampled, in draw order: kind, rect (dialog coords), corner
/// radius at 1x, fill colour.
type Shapes<'a> = &'a [(Shape, RECT, i32, u32)];

/// Cached supersampled bitmap identity: (control id, rect, backdrop, dark palette, on,
/// hot, pressed, focused, accent). Every palette is derived from dark + accent, so that
/// pair identifies the colours too; the rect carries the DPI-scaled geometry.
type SsKey = (i32, [i32; 4], u32, bool, bool, bool, bool, bool, u32);

// One slot per control: state permutations would otherwise pile up megabytes of bitmaps.
thread_local! {
    static SS: RefCell<HashMap<i32, (SsKey, HDC, HBITMAP, i32, i32)>> = RefCell::new(HashMap::new());
}

// Creation count, so a test can prove a repaint of an unchanged toggle reuses the bitmap.
#[cfg(test)]
thread_local! {
    static SS_BUILDS: Cell<usize> = const { Cell::new(0) };
}

/// Draws `shapes` antialiased into `rc`, then strokes `border` at 1x on top: a downscaled
/// 1px line softens into grey haze, so the outline must stay on the final 1x buffer.
/// `bg` is the flat surface behind `rc`; the rounded corners show it through.
/// `state` is (on, hot, pressed, focused) and only selects the cache slot.
unsafe fn aa_shape(
    hwnd: HWND,
    hdc: HDC,
    id: i32,
    rc: &RECT,
    radius: i32,
    p: &Palette,
    bg: u32,
    border: Option<u32>,
    state: (bool, bool, bool, bool),
    shapes: Shapes,
) {
    let (on, hot, pressed, focused) = state;
    let key = (
        id,
        [rc.left, rc.top, rc.right, rc.bottom],
        bg,
        p.dark,
        on,
        hot,
        pressed,
        focused,
        p.accent,
    );
    if let Some(dc) = ss_buffer(hwnd, &key, rc, bg, shapes) {
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        SetStretchBltMode(hdc, HALFTONE);
        // HALFTONE samples from the brush origin; a stale one shifts the downscale by it.
        let _ = SetBrushOrgEx(hdc, 0, 0, None);
        let _ = StretchBlt(
            hdc,
            rc.left,
            rc.top,
            w,
            h,
            dc,
            0,
            0,
            w * AA_SUPERSAMPLE,
            h * AA_SUPERSAMPLE,
            SRCCOPY,
        );
    }
    if let Some(color) = border {
        od_controls::stroke(hdc, rc, radius, color);
    }
}

/// Memory DC holding `shapes` at AA_SUPERSAMPLE scale over `bg`, created once per `key`.
unsafe fn ss_buffer(
    hwnd: HWND,
    key: &SsKey,
    rc: &RECT,
    bg: u32,
    shapes: Shapes,
) -> Option<HDC> {
    SS.with(|ss| {
        let mut ss = ss.borrow_mut();
        if let Some((held, dc, _, _, _)) = ss.get(&key.0) {
            if *held == *key {
                return Some(*dc);
            }
        }
        // DC first: destroying it deselects the bitmap, which is what makes deleting it legal.
        if let Some((_, dc, bmp, _, _)) = ss.remove(&key.0) {
            let _ = DeleteDC(dc);
            let _ = DeleteObject(bmp);
        }
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        let (sw, sh) = (w * AA_SUPERSAMPLE, h * AA_SUPERSAMPLE);
        // The colour format must come from a device DC: CreateCompatibleBitmap on a
        // memory DC (the dialog's back buffer) would hand back a 1bpp monochrome bitmap,
        // so the supersampled shapes would render as black and white.
        let window_dc = GetDC(hwnd);
        let dc = CreateCompatibleDC(window_dc);
        let bmp = CreateCompatibleBitmap(window_dc, sw, sh);
        let _ = ReleaseDC(hwnd, window_dc);
        if dc.0.is_null() || bmp.0.is_null() {
            if !dc.0.is_null() {
                let _ = DeleteDC(dc);
            }
            if !bmp.0.is_null() {
                let _ = DeleteObject(bmp);
            }
            return None;
        }
        // The memory DC outlives the bitmap it holds; the returned handle is not kept.
        SelectObject(dc, bmp);
        let full = RECT {
            left: 0,
            top: 0,
            right: sw,
            bottom: sh,
        };
        od_controls::fill(dc, &full, bg);
        for (shape, rect, radius, color) in shapes {
            let at = |v: i32| (v - rc.left) * AA_SUPERSAMPLE;
            let at_y = |v: i32| (v - rc.top) * AA_SUPERSAMPLE;
            let r = RECT {
                left: at(rect.left),
                top: at_y(rect.top),
                right: at(rect.right),
                bottom: at_y(rect.bottom),
            };
            match shape {
                Shape::Round => {
                    od_controls::round(dc, &r, radius * AA_SUPERSAMPLE, *color, None)
                }
                Shape::Dot => od_controls::dot(dc, &r, *color),
            }
        }
        #[cfg(test)]
        SS_BUILDS.with(|c| c.set(c.get() + 1));
        ss.insert(key.0, (*key, dc, bmp, sw, sh));
        Some(dc)
    })
}

unsafe fn free_ss_buffers() {
    SS.with(|ss| {
        for (_, (_, dc, bmp, _, _)) in ss.borrow_mut().drain() {
            let _ = DeleteDC(dc);
            let _ = DeleteObject(bmp);
        }
    });
}

thread_local! {
    // One off-screen surface per window, rebuilt only when the client size changes.
    static BACK: RefCell<HashMap<isize, (HDC, HBITMAP, i32, i32)>> = RefCell::new(HashMap::new());
}

/// Memory DC for `hwnd` sized to the client area, created once per size.
unsafe fn back_buffer(hwnd: HWND, w: i32, h: i32) -> Option<HDC> {
    BACK.with(|back| {
        let mut back = back.borrow_mut();
        let key = hwnd.0 as isize;
        if let Some((dc, _, bw, bh)) = back.get(&key) {
            if *bw == w && *bh == h {
                return Some(*dc);
            }
        }
        if let Some((dc, bmp, _, _)) = back.remove(&key) {
            // DC first: destroying it deselects the bitmap, which is what makes deleting
            // the bitmap legal.
            let _ = DeleteDC(dc);
            let _ = DeleteObject(bmp);
        }
        let window_dc = GetDC(hwnd);
        let dc = CreateCompatibleDC(window_dc);
        let bmp = CreateCompatibleBitmap(window_dc, w, h);
        let _ = ReleaseDC(hwnd, window_dc);
        if dc.0.is_null() || bmp.0.is_null() {
            if !dc.0.is_null() {
                let _ = DeleteDC(dc);
            }
            if !bmp.0.is_null() {
                let _ = DeleteObject(bmp);
            }
            return None;
        }
        SelectObject(dc, bmp);
        back.insert(key, (dc, bmp, w, h));
        Some(dc)
    })
}

unsafe fn free_back_buffer(hwnd: HWND) {
    BACK.with(|back| {
        if let Some((dc, bmp, _, _)) = back.borrow_mut().remove(&(hwnd.0 as isize)) {
            let _ = DeleteDC(dc);
            let _ = DeleteObject(bmp);
        }
    });
}

/// One complete frame: every pixel is written to a memory DC, then blitted once.
/// Nothing is drawn straight to the window DC unless the buffer could not be created.
unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    if w > 0 && h > 0 {
        let source = match back_buffer(hwnd, w, h) {
            Some(mem) => {
                paint_frame(hwnd, mem, &rc);
                mem
            }
            None => hdc,
        };
        let _ = BitBlt(hdc, 0, 0, w, h, source, 0, 0, SRCCOPY);
    }
    let _ = EndPaint(hwnd, &ps);
}

/// Invalidates and completes the frame before returning: one WM_SIZE, one blit, no
/// half-painted state left on screen for the next message to expose.
unsafe fn repaint(hwnd: HWND) {
    let _ = InvalidateRect(hwnd, None, true);
    let _ = UpdateWindow(hwnd);
}

unsafe fn paint_frame(hwnd: HWND, hdc: HDC, rc: &RECT) {
    let p = current_palette();
    let dip = dip_of();
    od_controls::fill(hdc, rc, p.window);
    let window_focused = GetFocus() == hwnd;
    let ctls = layout(hwnd);
    let cards: Vec<RECT> = ctls
        .iter()
        .filter(|c| c.kind == Kind::Card)
        .map(|c| c.rect)
        .collect();
    // Cards come first in `layout()`, so their children paint over them.
    for ctl in &ctls {
        match ctl.kind {
            Kind::Card => draw_card(hdc, ctl, &p, dip),
            Kind::Label => od_controls::text(
                hdc,
                &ctl.rect,
                &ctl.text,
                ui_font(),
                p.text,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX,
            ),
            Kind::Button => draw_button(hdc, ctl, &p, dip, window_focused, &cards),
            Kind::Toggle => draw_toggle(hwnd, hdc, ctl, &p, dip, window_focused, &cards),
            Kind::Edit => draw_field(hdc, ctl, &p, dip, window_focused),
            Kind::List => draw_list(hdc, ctl, &p, dip, window_focused),
        }
    }
}

fn inset(rc: RECT, dx: i32, dy: i32) -> RECT {
    RECT {
        left: rc.left + dx,
        top: rc.top + dy,
        right: rc.right - dx,
        bottom: rc.bottom - dy,
    }
}

unsafe fn draw_card(hdc: HDC, ctl: &Ctl, p: &Palette, dip: Dip) {
    let rc = inset(ctl.rect, dip.px(1.0), dip.px(1.0));
    od_controls::round(hdc, &rc, dip.px(8.0), p.card, Some(p.hairline));
    if !ctl.text.is_empty() {
        let caption = RECT {
            left: rc.left + dip.px(12.0),
            right: rc.right - dip.px(12.0),
            top: rc.top + dip.px(6.0),
            bottom: rc.top + dip.px(6.0) + dip.px(20.0),
        };
        od_controls::text(
            hdc,
            &caption,
            &ctl.text,
            ui_font(),
            p.secondary,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
    }
}

/// Background behind a control: the card its centre lands on, else the window. Both come
/// from the one `layout()` the frame was drawn from, so a resize cannot mix geometries.
fn surface_of(rc: RECT, cards: &[RECT]) -> u32 {
    let p = current_palette();
    let (cx, cy) = ((rc.left + rc.right) / 2, (rc.top + rc.bottom) / 2);
    if cards
        .iter()
        .any(|r| cx >= r.left && cx <= r.right && cy >= r.top && cy <= r.bottom)
    {
        p.card
    } else {
        p.window
    }
}

unsafe fn draw_button(
    hdc: HDC,
    ctl: &Ctl,
    p: &Palette,
    dip: Dip,
    window_focused: bool,
    cards: &[RECT],
) {
    let s = |v: f32| dip.px(v);
    let rc = ctl.rect;
    let id = ctl.id;
    let primary = id == ID_BTN_SAVE_CLOSE;
    let hot = HOT.with(|h| h.get()) == id;
    let pressed = PRESSED.with(|p| p.get()) == id;
    let focused = window_focused && matches!(focus(), Focus::Button(f) if f == id);
    let small = with_ui(|u| u.font_small).unwrap_or(HFONT(std::ptr::null_mut()));

    od_controls::fill(hdc, &rc, surface_of(rc, cards));

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
    od_controls::round(hdc, &inner, s(6.0), fill, border);
    od_controls::text(
        hdc,
        &inner,
        &ctl.text,
        small,
        fg,
        DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_END_ELLIPSIS,
    );
}

unsafe fn draw_toggle(
    hwnd: HWND,
    hdc: HDC,
    ctl: &Ctl,
    p: &Palette,
    dip: Dip,
    window_focused: bool,
    cards: &[RECT],
) {
    let rc = ctl.rect;
    let on = toggle(ctl.id);
    let hot = HOT.with(|h| h.get()) == ctl.id;
    let pressed = PRESSED.with(|p| p.get()) == ctl.id;
    let focused = window_focused && matches!(focus(), Focus::Toggle(f) if f == ctl.id);
    // Toggles sit on the settings card.
    od_controls::fill(hdc, &rc, surface_of(rc, cards));

    let pill_h = dip.px(18.0);
    // Centred on the control's own rect: `rc` is in dialog coordinates, so a bare
    // `rc.bottom / 2` would put the pill at half the window height, well above its row,
    // and every toggle would land on nearly the same y (the three overlapping pills).
    let pill_top = rc.top + ((rc.bottom - rc.top) - pill_h) / 2;
    let pill = RECT {
        left: rc.left + dip.px(1.0),
        top: pill_top,
        right: rc.left + dip.px(1.0) + dip.px(36.0),
        bottom: pill_top + pill_h,
    };
    let track = if on {
        if pressed {
            od_controls::mix(p.accent, p.text, 0.25)
        } else if hot {
            od_controls::mix(p.accent, p.text, 0.12)
        } else {
            p.accent
        }
    } else if pressed {
        p.pressed
    } else if hot {
        p.hover
    } else {
        p.well
    };
    let knob = RECT {
        left: if on {
            pill.right - dip.px(2.0) - (pill_h - dip.px(4.0))
        } else {
            pill.left + dip.px(2.0)
        },
        top: pill.top + dip.px(2.0),
        ..Default::default()
    };
    let knob = RECT {
        right: knob.left + pill_h - dip.px(4.0),
        bottom: knob.top + pill_h - dip.px(4.0),
        ..knob
    };
    let ring = RECT {
        left: pill.left - dip.px(2.0),
        top: pill.top - dip.px(2.0),
        right: pill.right + dip.px(2.0),
        bottom: pill.bottom + dip.px(2.0),
    };
    // The focus ring is stroked at 1x, so it is not part of the supersampled pass.
    aa_shape(
        hwnd,
        hdc,
        ctl.id,
        &pill,
        pill_h / 2,
        p,
        surface_of(rc, cards),
        (!on).then_some(p.hairline),
        (on, hot, pressed, focused),
        &[
            (Shape::Round, pill, pill_h / 2, track),
            (
                Shape::Dot,
                knob,
                0,
                if on { p.on_accent } else { p.secondary },
            ),
        ],
    );
    if focused {
        od_controls::stroke(hdc, &ring, pill_h / 2 + dip.px(2.0), p.accent);
    }

    if !ctl.text.is_empty() {
        od_controls::text(
            hdc,
            &RECT {
                left: pill.right + dip.px(10.0),
                right: rc.right,
                ..rc
            },
            &ctl.text,
            ui_font(),
            p.text,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
    }
}

/// Cue shown while a field is empty and unfocused, as the native EDIT's cue banner did.
fn cue_text(id: i32) -> &'static str {
    match id {
        ID_EDIT_SEARCH => "Filter by process name",
        ID_EDIT_FILTER_EXCLUDED => "Filter exclusions",
        ID_EDIT_FILTER_ALWAYS_MUTED => "Filter always muted",
        _ => "",
    }
}

unsafe fn text_width(hdc: HDC, s: &str, font: HFONT) -> i32 {
    let wide: Vec<u16> = s.encode_utf16().collect();
    if wide.is_empty() {
        return 0;
    }
    let old = SelectObject(hdc, HGDIOBJ(font.0));
    let mut size = SIZE::default();
    let _ = GetTextExtentPoint32W(hdc, &wide, &mut size);
    SelectObject(hdc, old);
    size.cx
}

/// Horizontal scroll of a field: 0 unless the caret would leave the text area.
fn field_scroll(inner: &RECT, caret_rel: i32) -> i32 {
    let width = inner.right - inner.left;
    (caret_rel + 2 - width).max(0)
}

/// Where the caret sits and how far the text is scrolled, in client coordinates.
/// Paint and SetCaretPos share this, so the drawn caret can never disagree with the
/// measured text.
unsafe fn field_caret(hwnd: HWND, ctl: &Ctl) -> (i32, i32) {
    let dip = dip_of();
    let inner = inset(ctl.rect, dip.px(6.0), dip.px(2.0));
    let hdc = GetDC(hwnd);
    let font = ui_font();
    let (scroll, x) = with_field(ctl.id, |f| {
        let prefix: String = f.text.chars().take(f.caret).collect();
        let rel = text_width(hdc, &prefix, font);
        (field_scroll(&inner, rel), inner.left + rel)
    });
    let _ = ReleaseDC(hwnd, hdc);
    (x - scroll, inner.top)
}

unsafe fn draw_field(hdc: HDC, ctl: &Ctl, p: &Palette, dip: Dip, window_focused: bool) {
    let focused = window_focused && matches!(focus(), Focus::Field(f) if f == ctl.id);
    od_controls::round(
        hdc,
        &ctl.rect,
        dip.px(4.0),
        p.well,
        Some(if focused { p.accent } else { p.hairline }),
    );
    let inner = inset(ctl.rect, dip.px(6.0), dip.px(2.0));
    let font = ui_font();
    let (text, selection) = with_field(ctl.id, |f| (f.text.clone(), f.selection()));
    if text.is_empty() {
        let cue = cue_text(ctl.id);
        if !focused && !cue.is_empty() {
            od_controls::text(
                hdc,
                &inner,
                cue,
                font,
                p.secondary,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
            );
        }
        return;
    }

    let scroll = with_field(ctl.id, |f| {
        let prefix: String = f.text.chars().take(f.caret).collect();
        field_scroll(&inner, text_width(hdc, &prefix, font))
    });
    let saved = SaveDC(hdc);
    let _ = IntersectClipRect(hdc, inner.left, inner.top, inner.right, inner.bottom);
    if selection.0 != selection.1 {
        let at = |idx: usize| {
            let prefix: String = text.chars().take(idx).collect();
            inner.left + text_width(hdc, &prefix, font) - scroll
        };
        let (a, b) = (at(selection.0), at(selection.1));
        let band = RECT {
            left: a.max(inner.left),
            top: inner.top,
            right: b.min(inner.right),
            bottom: inner.bottom,
        };
        if band.right > band.left {
            od_controls::fill(hdc, &band, od_controls::mix(p.well, p.accent, 0.30));
        }
    }
    od_controls::text(
        hdc,
        &RECT {
            left: inner.left - scroll,
            ..inner
        },
        &text,
        font,
        p.text,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX,
    );
    let _ = RestoreDC(hdc, saved);
}

fn row_height() -> i32 {
    dip_of().px(24.0)
}

/// Rows that fit in a list rect; a list always reports at least one.
fn visible_rows(rc: RECT) -> usize {
    ((rc.bottom - rc.top).max(0) / row_height()).max(1) as usize
}

/// Custom scrollbar band width, reserved inside the list rect when rows overflow.
fn scrollbar_width() -> i32 {
    dip_of().px(10.0)
}

unsafe fn draw_list(hdc: HDC, ctl: &Ctl, p: &Palette, dip: Dip, window_focused: bool) {
    let rc = ctl.rect;
    od_controls::fill(hdc, &rc, p.well);
    let visible = visible_rows(rc);
    let rh = row_height();
    let focused = window_focused && matches!(focus(), Focus::List(l) if l == ctl.id);
    let saved = SaveDC(hdc);
    let _ = IntersectClipRect(hdc, rc.left, rc.top, rc.right, rc.bottom);

    LISTS.with(|lists| {
        let lists = lists.borrow();
        let Some(lv) = lists.get(&ctl.id) else { return };
        let overflow = lv.rows.len() > visible;
        let right = if overflow {
            rc.right - scrollbar_width()
        } else {
            rc.right
        };
        for idx in lv.top..(lv.top + visible).min(lv.rows.len()) {
            let row = &lv.rows[idx];
            let row_rc = RECT {
                left: rc.left,
                top: rc.top + (idx - lv.top) as i32 * rh,
                right,
                bottom: rc.top + (idx - lv.top + 1) as i32 * rh,
            };
            let selected = lv.sel == Some(idx);
            let bg = if selected {
                od_controls::mix(p.well, p.accent, 0.30)
            } else if lv.hot == Some(idx) {
                p.hover
            } else {
                p.well
            };
            od_controls::fill(hdc, &row_rc, bg);
            if selected && focused {
                od_controls::round(hdc, &row_rc, dip.px(4.0), bg, Some(p.accent));
            }

            let icon_size = dip.px(16.0);
            let icon = row_icon(row.data.max(0) as u32, &row.text);
            let text_left = match icon {
                Some(icon) => {
                    let y = row_rc.top + ((row_rc.bottom - row_rc.top) - icon_size) / 2;
                    let _ = DrawIconEx(
                        hdc,
                        row_rc.left + dip.px(6.0),
                        y,
                        icon,
                        icon_size,
                        icon_size,
                        0,
                        HBRUSH::default(),
                        DI_NORMAL,
                    );
                    row_rc.left + dip.px(6.0) + icon_size + dip.px(8.0)
                }
                None => row_rc.left + dip.px(8.0),
            };
            od_controls::text(
                hdc,
                &RECT {
                    left: text_left,
                    right: row_rc.right - dip.px(4.0),
                    ..row_rc
                },
                &row.text,
                ui_font(),
                p.text,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
        }

        if overflow {
            let track = RECT {
                left: rc.right - scrollbar_width(),
                top: rc.top + dip.px(2.0),
                right: rc.right - dip.px(2.0),
                bottom: rc.bottom - dip.px(2.0),
            };
            od_controls::fill(hdc, &track, p.hairline);
            let track_h = (track.bottom - track.top).max(1);
            let thumb_h = ((visible as f32 / lv.rows.len() as f32) * track_h as f32)
                .max(dip.px(12.0) as f32) as i32;
            let max_top = lv.rows.len().saturating_sub(visible);
            let offset = if max_top == 0 {
                0
            } else {
                (track_h - thumb_h) * lv.top as i32 / max_top as i32
            };
            od_controls::fill(
                hdc,
                &RECT {
                    top: track.top + offset,
                    bottom: track.top + offset + thumb_h,
                    ..track
                },
                p.secondary,
            );
        }
    });
    let _ = RestoreDC(hdc, saved);
}

// ----------------------------------------------------------------- interaction

/// Topmost control at client point (x, y), 0 when the point hits the bare window.
/// Cards are the first entries in `layout()`, so reverse iteration makes them the
/// last-resort match: a card never swallows a click meant for something on top of it.
fn hit_test(ctls: &[Ctl], x: i32, y: i32) -> i32 {
    ctls.iter()
        .rev()
        .find(|c| {
            c.kind != Kind::Card
                && x >= c.rect.left
                && x < c.rect.right
                && y >= c.rect.top
                && y < c.rect.bottom
        })
        .map_or(0, |c| c.id)
}

fn point_of(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

fn ctl_of(hwnd: HWND, id: i32) -> Option<Ctl> {
    layout(hwnd).into_iter().find(|c| c.id == id)
}

fn kind_of(hwnd: HWND, id: i32) -> Option<Kind> {
    ctl_of(hwnd, id).map(|c| c.kind)
}

/// Row index under a client point in a list, or None outside the rows.
fn row_at(rc: RECT, y: i32, top: usize, rows: usize) -> Option<usize> {
    if y < rc.top || y >= rc.bottom {
        return None;
    }
    let idx = top + ((y - rc.top) / row_height()) as usize;
    (idx < rows).then_some(idx)
}

/// Scroll so row `idx` is on screen; `visible` rows fit.
fn reveal(id: i32, idx: usize, visible: usize) {
    with_list(id, |l| {
        if idx < l.top {
            l.top = idx;
        } else if idx >= l.top + visible {
            l.top = idx + 1 - visible;
        }
        l.clamp(visible);
    });
}

fn select_row(hwnd: HWND, id: i32, idx: usize) {
    let Some(ctl) = ctl_of(hwnd, id) else { return };
    let visible = visible_rows(ctl.rect);
    with_list(id, |l| l.sel = Some(idx));
    reveal(id, idx, visible);
}

/// Field caret from a client x: nearest char boundary, measured through the same
/// horizontal scroll the paint uses.
unsafe fn caret_from_x(hwnd: HWND, ctl: &Ctl, x: i32) -> usize {
    let dip = dip_of();
    let inner = inset(ctl.rect, dip.px(6.0), dip.px(2.0));
    let (text, caret) = with_field(ctl.id, |f| (f.text.clone(), f.caret));
    let prefix_len = caret;
    let font = ui_font();
    let hdc = GetDC(hwnd);
    let prefix: String = text.chars().take(prefix_len).collect();
    let scroll = field_scroll(&inner, text_width(hdc, &prefix, font));
    let target = x - inner.left + scroll;
    let count = text.chars().count();
    let mut best = count;
    let mut prev = 0;
    for idx in 0..=count {
        let head: String = text.chars().take(idx).collect();
        let at = text_width(hdc, &head, font);
        if at >= target {
            best = if at - target <= (target - prev) {
                idx
            } else {
                idx.saturating_sub(1)
            };
            break;
        }
        prev = at;
    }
    let _ = ReleaseDC(hwnd, hdc);
    best.min(count)
}

/// Caret bar: created once per focused field, sized to the DIP line height.
unsafe fn update_caret(hwnd: HWND) {
    let Focus::Field(id) = focus() else {
        hide_caret(hwnd);
        return;
    };
    let Some(ctl) = ctl_of(hwnd, id) else {
        hide_caret(hwnd);
        return;
    };
    let dip = dip_of();
    let line = (ctl.rect.bottom - ctl.rect.top - dip.px(4.0)).max(1);
    if !CARET_ON.with(|c| c.get()) {
        let _ = CreateCaret(hwnd, HBITMAP::default(), dip.px(1.0).max(1), line);
        CARET_ON.with(|c| c.set(true));
    }
    let (x, top) = field_caret(hwnd, &ctl);
    let _ = SetCaretPos(x, top);
    // A caret on an inactive window would blink on top of every repaint; the window that
    // is not focused hides it the way a native edit does.
    if GetFocus() == hwnd {
        let _ = ShowCaret(hwnd);
    } else {
        let _ = HideCaret(hwnd);
    }
}

unsafe fn hide_caret(hwnd: HWND) {
    let _ = HideCaret(hwnd);
    // DestroyCaret is thread-global, so the flag, not the window, decides.
    if CARET_ON.with(|c| c.replace(false)) {
        let _ = DestroyCaret();
    }
}

/// Focus moved: repaint the rings and move (or drop) the caret in one step.
unsafe fn refocus(hwnd: HWND) {
    repaint(hwnd);
    update_caret(hwnd);
}

/// Exe path for a process named `name` that is no longer running (it was excluded or
/// always-muted earlier). One toolhelp snapshot per miss, cached per name.
unsafe fn path_for_name(name: &str) -> Option<String> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let wanted = name.to_lowercase();
    if MISSING_PATHS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .contains_key(&wanted)
    {
        return None;
    }

    let mut found: Option<u32> = None;
    if let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
        while ok {
            let exe = String::from_utf16_lossy(&entry.szExeFile);
            let exe = exe.trim_end_matches('\0').to_lowercase();
            if exe == wanted {
                found = Some(entry.th32ProcessID);
                break;
            }
            ok = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = windows::Win32::Foundation::CloseHandle(snapshot);
    }

    let path = found.and_then(|process| process_image_path(process));
    if path.is_none() {
        // Not running: remember the miss so a repaint never walks the process list again.
        MISSING_PATHS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .insert(wanted, ());
    }
    path
}

// ------------------------------------------------------------- list content

/// Detected rows visible under `filter_detected`; `detected_apps` mirrors the list so a
/// selected row index maps back to its pid.
fn filtered_detected() -> Vec<(u32, String)> {
    DIALOG_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(s) = state.as_mut() else {
            return Vec::new();
        };
        let rows: Vec<(u32, String)> = s
            .all_detected_apps
            .iter()
            .filter(|(_, name)| {
                s.filter_detected.is_empty() || name.to_lowercase().contains(&s.filter_detected)
            })
            .cloned()
            .collect();
        s.detected_apps = rows.clone();
        rows
    })
}

unsafe fn refill_detected_list(hwnd: HWND) {
    let visible = ctl_of(hwnd, ID_LIST_DETECTED).map_or(1, |c| visible_rows(c.rect));
    let rows: Vec<Row> = filtered_detected()
        .into_iter()
        // Row data = PID, so a repaint can resolve the real icon.
        .map(|(pid, name)| Row {
            text: format!("{} (PID: {})", name, pid),
            data: pid as isize,
        })
        .collect();
    with_list(ID_LIST_DETECTED, |l| l.set_rows(rows, visible));
}

unsafe fn refill_named_list(hwnd: HWND, id: i32, names: &[String]) {
    let visible = ctl_of(hwnd, id).map_or(1, |c| visible_rows(c.rect));
    let rows: Vec<Row> = names
        .iter()
        .map(|n| Row {
            text: n.clone(),
            data: 0,
        })
        .collect();
    with_list(id, |l| l.set_rows(rows, visible));
}

unsafe fn refresh_detected_apps(hwnd: HWND) {
    // A fresh scan may now find apps that were gone last time, so retry their icons.
    // PIDs are recycled, so the id-keyed answers go too; path-keyed icons stay.
    if let Some(missing) = MISSING_PATHS.get() {
        missing.lock().clear();
    }
    if let Some(by_pid) = ICON_OF_PID.get() {
        by_pid.lock().clear();
    }
    DIALOG_STATE.with(|state| {
        if let Some(ref mut s) = *state.borrow_mut() {
            let sessions = s.audio_manager.get_sessions();

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

            apps.sort_by_key(|a| a.1.to_lowercase());
            s.all_detected_apps = apps;
        }
    });

    refill_detected_list(hwnd);
    refresh_excluded_list(hwnd);
    refresh_always_muted_list(hwnd);
    repaint(hwnd);
}

unsafe fn refresh_excluded_list(hwnd: HWND) {
    let (names, filter) = DIALOG_STATE.with(|state| {
        let state = state.borrow();
        match state.as_ref() {
            Some(s) => (
                {
                    let mut names: Vec<String> =
                        s.config.read().excluded_apps.iter().cloned().collect();
                    names.sort_by_key(|a| a.to_lowercase());
                    names
                },
                s.filter_excluded.clone(),
            ),
            None => (Vec::new(), String::new()),
        }
    });
    let names: Vec<String> = names
        .into_iter()
        .filter(|n| filter.is_empty() || n.to_lowercase().contains(&filter))
        .collect();
    refill_named_list(hwnd, ID_LIST_EXCLUDED, &names);
}

unsafe fn refresh_always_muted_list(hwnd: HWND) {
    let (names, filter) = DIALOG_STATE.with(|state| {
        let state = state.borrow();
        match state.as_ref() {
            Some(s) => (
                {
                    let mut names: Vec<String> =
                        s.config.read().always_muted_apps.iter().cloned().collect();
                    names.sort_by_key(|a| a.to_lowercase());
                    names
                },
                s.filter_always_muted.clone(),
            ),
            None => (Vec::new(), String::new()),
        }
    });
    let names: Vec<String> = names
        .into_iter()
        .filter(|n| filter.is_empty() || n.to_lowercase().contains(&filter))
        .collect();
    refill_named_list(hwnd, ID_LIST_ALWAYS_MUTED, &names);
}

unsafe fn load_settings_to_field(hwnd: HWND) {
    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let config = s.config.read();
            let muting_enabled = s.muting_enabled.load(Ordering::Relaxed);

            set_toggle(ID_CHECK_ENABLED, muting_enabled);
            set_toggle(ID_CHECK_START_MINIMIZED, config.start_minimized);
            set_toggle(ID_CHECK_START_WINDOWS, config.start_with_windows);
            set_field(ID_EDIT_POLL_INTERVAL, config.poll_interval_ms.to_string());
        }
    });
    repaint(hwnd);
}

/// One filter field changed: store the needle lowercased and refill only its own list,
/// so the other two lists keep their selection.
unsafe fn apply_edit_filter(hwnd: HWND, id: i32) {
    let needle = field_text(id).to_lowercase();
    DIALOG_STATE.with(|state| {
        if let Some(ref mut s) = *state.borrow_mut() {
            match id {
                ID_EDIT_SEARCH => s.filter_detected = needle,
                ID_EDIT_FILTER_EXCLUDED => s.filter_excluded = needle,
                _ => s.filter_always_muted = needle,
            }
        }
    });
    match id {
        ID_EDIT_SEARCH => refill_detected_list(hwnd),
        ID_EDIT_FILTER_EXCLUDED => refresh_excluded_list(hwnd),
        _ => refresh_always_muted_list(hwnd),
    }
}

/// Command dispatch for a button, a toggle flip and a list double-click; the ids and
/// branches are the ones the WM_COMMAND plumbing used to carry.
unsafe fn handle_command(hwnd: HWND, control_id: i32, notification: u16) {
    match control_id {
        ID_EDIT_SEARCH | ID_EDIT_FILTER_EXCLUDED | ID_EDIT_FILTER_ALWAYS_MUTED => {
            if notification == EN_CHANGE {
                apply_edit_filter(hwnd, control_id);
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
        ID_BTN_EDIT_CONFIG => {
            // Save first so the file on disk matches the dialog the user is looking at.
            save_settings(hwnd);
            crate::shell::open_config_in_default_editor();
        }
        ID_BTN_SAVE_CLOSE => {
            save_settings(hwnd);
            let _ = DestroyWindow(hwnd);
        }
        ID_BTN_CLOSE => {
            let _ = DestroyWindow(hwnd);
        }
        // Double-click moves the row across the panels: detected -> excluded,
        // excluded -> detected, always muted -> detected.
        ID_LIST_DETECTED | ID_LIST_EXCLUDED | ID_LIST_ALWAYS_MUTED
            if notification == LBN_DBLCLK as u16 =>
        {
            match control_id {
                ID_LIST_DETECTED => add_selected_to_exclusions(hwnd),
                ID_LIST_EXCLUDED => remove_selected_exclusion(hwnd),
                _ => remove_selected_always_muted(hwnd),
            }
        }
        _ => {}
    }
}

unsafe fn add_selected_to_exclusions(hwnd: HWND) {
    let Some(sel_idx) = list_sel(ID_LIST_DETECTED) else {
        return;
    };
    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            if let Some((_, name)) = s.detected_apps.get(sel_idx) {
                let mut config = s.config.write();
                config.add_excluded_app(name);
            }
        }
    });
    refresh_detected_apps(hwnd);
}

unsafe fn remove_selected_exclusion(hwnd: HWND) {
    let Some(app_name) = with_list(ID_LIST_EXCLUDED, |l| l.selected_text()) else {
        return;
    };
    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let mut config = s.config.write();
            config.remove_excluded_app(&app_name);
        }
    });
    refresh_detected_apps(hwnd);
}

unsafe fn add_selected_to_always_muted(hwnd: HWND) {
    let Some(sel_idx) = list_sel(ID_LIST_DETECTED) else {
        return;
    };
    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            if let Some((_, name)) = s.detected_apps.get(sel_idx) {
                let mut config = s.config.write();
                config.add_always_muted_app(name);
            }
        }
    });
    refresh_detected_apps(hwnd);
}

unsafe fn remove_selected_always_muted(hwnd: HWND) {
    let Some(app_name) = with_list(ID_LIST_ALWAYS_MUTED, |l| l.selected_text()) else {
        return;
    };
    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let mut config = s.config.write();
            config.remove_always_muted_app(&app_name);
        }
    });
    refresh_detected_apps(hwnd);
}

unsafe fn save_settings(hwnd: HWND) {
    let _ = hwnd;
    DIALOG_STATE.with(|state| {
        if let Some(ref s) = *state.borrow() {
            let mut config = s.config.write();

            let muting_checked = toggle(ID_CHECK_ENABLED);
            let start_minimized_checked = toggle(ID_CHECK_START_MINIMIZED);
            let start_windows_checked = toggle(ID_CHECK_START_WINDOWS);

            let poll_interval: u64 = field_text(ID_EDIT_POLL_INTERVAL)
                .trim()
                .parse()
                .unwrap_or(500)
                .clamp(100, 2000);

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

// ---------------------------------------------------------------- text editing

/// Numeric field: digits only, so `parse` in save never sees a stray character.
fn numeric_field(id: i32) -> bool {
    id == ID_EDIT_POLL_INTERVAL
}

fn byte_index(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map_or(text.len(), |(i, _)| i)
}

/// Replace the selection (or, collapsed, insert at the caret) with `insert`.
fn insert_at_caret(id: i32, insert: &str) {
    with_field(id, |f| {
        let (start, end) = f.selection();
        let (a, b) = (byte_index(&f.text, start), byte_index(&f.text, end));
        let mut next = f.text.clone();
        next.replace_range(a..b, insert);
        f.text = next;
        f.caret = start + insert.chars().count();
        f.anchor = f.caret;
    })
}

fn delete_selection(id: i32) -> bool {
    with_field(id, |f| {
        let (start, end) = f.selection();
        if start == end {
            return false;
        }
        let (a, b) = (byte_index(&f.text, start), byte_index(&f.text, end));
        f.text.replace_range(a..b, "");
        f.caret = start;
        f.anchor = start;
        true
    })
}

fn backspace(id: i32) -> bool {
    with_field(id, |f| {
        if delete_selection(id) {
            return true;
        }
        let caret = f.caret;
        if caret == 0 {
            return false;
        }
        let a = byte_index(&f.text, caret - 1);
        let b = byte_index(&f.text, caret);
        f.text.replace_range(a..b, "");
        f.caret = caret - 1;
        f.anchor = f.caret;
        true
    })
}

fn delete_forward(id: i32) -> bool {
    with_field(id, |f| {
        if delete_selection(id) {
            return true;
        }
        let caret = f.caret;
        if caret >= f.text.chars().count() {
            return false;
        }
        let a = byte_index(&f.text, caret);
        let b = byte_index(&f.text, caret + 1);
        f.text.replace_range(a..b, "");
        f.anchor = f.caret;
        true
    })
}

fn select_all(id: i32) {
    with_field(id, |f| {
        f.anchor = 0;
        f.caret = f.text.chars().count();
    });
}

fn selected_slice(id: i32) -> Option<String> {
    with_field(id, |f| {
        let (start, end) = f.selection();
        (start != end).then(|| f.text.chars().skip(start).take(end - start).collect())
    })
}

unsafe fn clipboard_open(hwnd: HWND) -> bool {
    use windows::Win32::System::DataExchange::OpenClipboard;
    OpenClipboard(hwnd).is_ok()
}

unsafe fn clipboard_copy(hwnd: HWND, id: i32) {
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, SetClipboardData};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    let Some(text) = selected_slice(id) else {
        return;
    };
    if !clipboard_open(hwnd) {
        return;
    }
    let _ = EmptyClipboard();
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // GMEM_MOVEABLE is what SetClipboardData requires; the clipboard owns the block.
    if let Ok(block) = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) {
        let dst = GlobalLock(block) as *mut u16;
        if !dst.is_null() {
            std::ptr::copy_nonoverlapping(units.as_ptr(), dst, units.len());
            let _ = GlobalUnlock(block);
            let _ = SetClipboardData(
                CF_UNICODETEXT.0 as u32,
                windows::Win32::Foundation::HANDLE(block.0),
            );
        }
    }
    let _ = CloseClipboard();
}

unsafe fn clipboard_cut(hwnd: HWND, id: i32) {
    clipboard_copy(hwnd, id);
    if delete_selection(id) {
        after_field_edit(hwnd, id);
    }
}

unsafe fn clipboard_paste(hwnd: HWND, id: i32) {
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    if !clipboard_open(hwnd) {
        return;
    }
    let text = GetClipboardData(CF_UNICODETEXT.0 as u32)
        .ok()
        .and_then(|handle| {
            let ptr = GlobalLock(windows::Win32::Foundation::HGLOBAL(handle.0)) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(handle.0));
            Some(String::from_utf16_lossy(slice))
        });
    let _ = CloseClipboard();
    let Some(text) = text else { return };
    // A single-line field drops line breaks, and the numeric field keeps digits only,
    // so pasting can never put something `save` would have to reject into the box.
    let text: String = text
        .chars()
        .filter(|c| *c >= ' ' && (!numeric_field(id) || c.is_ascii_digit()))
        .collect();
    if !text.is_empty() {
        insert_at_caret(id, &text);
        after_field_edit(hwnd, id);
    }
}

/// A field's text changed by an edit key: run the filter command it used to report as
/// EN_CHANGE, then repaint with the caret in its new place.
unsafe fn after_field_edit(hwnd: HWND, id: i32) {
    handle_command(hwnd, id, EN_CHANGE);
    repaint(hwnd);
    update_caret(hwnd);
}

// -------------------------------------------------------------------- input

fn focus_target(kind: Kind, id: i32) -> Focus {
    match kind {
        Kind::Button => Focus::Button(id),
        Kind::Toggle => Focus::Toggle(id),
        Kind::Edit => Focus::Field(id),
        _ => Focus::List(id),
    }
}

/// Keyboard focus order: buttons, toggles, fields and lists in the order `layout()`
/// lists them, which is the order a user reads the dialog in.
fn tab_order(hwnd: HWND) -> Vec<Focus> {
    layout(hwnd)
        .into_iter()
        .filter_map(|c| match c.kind {
            Kind::Button | Kind::Toggle | Kind::Edit | Kind::List => {
                Some(focus_target(c.kind, c.id))
            }
            _ => None,
        })
        .collect()
}

unsafe fn cycle_focus(hwnd: HWND, backwards: bool) {
    let order = tab_order(hwnd);
    if order.is_empty() {
        return;
    }
    let current = order.iter().position(|f| *f == focus());
    let next = match (current, backwards) {
        (Some(0), true) => order.len() - 1,
        (Some(i), true) => i - 1,
        (Some(i), false) => (i + 1) % order.len(),
        (None, true) => order.len() - 1,
        (None, false) => 0,
    };
    set_focus_value(order[next]);
    refocus(hwnd);
}

unsafe fn activate_button(hwnd: HWND, id: i32) {
    handle_command(hwnd, id, BN_CLICKED as u16);
    if IsWindow(hwnd).as_bool() {
        repaint(hwnd);
    }
}

unsafe fn flip_toggle(hwnd: HWND, id: i32) {
    set_toggle(id, !toggle(id));
    handle_command(hwnd, id, BN_CLICKED as u16);
    repaint(hwnd);
}

unsafe fn mouse_move(hwnd: HWND, lparam: LPARAM) {
    let (x, y) = point_of(lparam);
    // Dragging out of a field after pressing in it extends the selection.
    let pressed = PRESSED.with(|p| p.get());
    if let Focus::Field(id) = focus() {
        if pressed == id {
            if let Some(ctl) = ctl_of(hwnd, id) {
                let caret = caret_from_x(hwnd, &ctl, x);
                with_field(id, |f| f.caret = caret);
                repaint(hwnd);
                update_caret(hwnd);
            }
            return;
        }
    }
    let ctls = layout(hwnd);
    let hit = hit_test(&ctls, x, y);
    let hot_id = HOT.with(|h| h.get());
    let mut changed = false;
    if hit != hot_id {
        HOT.with(|h| h.set(hit));
        changed = true;
    }
    // Hovered row of a list.
    if let Some(ctl) = ctls.iter().find(|c| c.id == hit && c.kind == Kind::List) {
        let index = with_list(ctl.id, |l| row_at(ctl.rect, y, l.top, l.rows.len()));
        changed |= with_list(ctl.id, |l| {
            let before = l.hot;
            l.hot = index;
            before != l.hot
        });
    }
    for ctl in ctls.iter().filter(|c| c.kind == Kind::List && c.id != hit) {
        changed |= with_list(ctl.id, |l| std::mem::take(&mut l.hot).is_some());
    }
    if changed {
        repaint(hwnd);
    }

    let cursor = if hit != 0 && kind_of(hwnd, hit) == Some(Kind::Edit) {
        IDC_IBEAM
    } else {
        IDC_ARROW
    };
    let load = |name: PCWSTR| LoadCursorW(None, name).unwrap_or_default();
    SetCursor(load(cursor));

    let mut tme = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    let _ = TrackMouseEvent(&mut tme);
}

unsafe fn mouse_down(hwnd: HWND, lparam: LPARAM) {
    let (x, y) = point_of(lparam);
    let hit = hit_test(&layout(hwnd), x, y);
    PRESSED.with(|p| p.set(hit));
    HOT.with(|h| h.set(hit));
    match kind_of(hwnd, hit) {
        Some(Kind::Edit) => {
            set_focus_value(Focus::Field(hit));
            if let Some(ctl) = ctl_of(hwnd, hit) {
                let caret = caret_from_x(hwnd, &ctl, x);
                with_field(hit, |f| {
                    f.caret = caret;
                    f.anchor = caret;
                });
            }
            SetCapture(hwnd);
            refocus(hwnd);
        }
        Some(Kind::List) => {
            set_focus_value(Focus::List(hit));
            let Some(ctl) = ctl_of(hwnd, hit) else { return };
            let index = with_list(hit, |l| row_at(ctl.rect, y, l.top, l.rows.len()));
            if let Some(idx) = index {
                with_list(hit, |l| l.sel = Some(idx));
                select_row(hwnd, hit, idx);
            }
            SetCapture(hwnd);
            refocus(hwnd);
            click_list_row(hwnd, hit, x, y, index);
        }
        Some(Kind::Button) => {
            set_focus_value(Focus::Button(hit));
            SetCapture(hwnd);
            refocus(hwnd);
        }
        Some(Kind::Toggle) => {
            set_focus_value(Focus::Toggle(hit));
            SetCapture(hwnd);
            refocus(hwnd);
        }
        _ => {
            set_focus_value(Focus::None);
            refocus(hwnd);
        }
    }
}

/// Button or toggle released inside itself: run its command, then repaint the result.
unsafe fn mouse_up(hwnd: HWND, lparam: LPARAM) {
    let (x, y) = point_of(lparam);
    let pressed = PRESSED.with(|p| p.replace(0));
    let _ = ReleaseCapture();
    let released_on = hit_test(&layout(hwnd), x, y);
    if pressed != 0 && released_on == pressed {
        match kind_of(hwnd, pressed) {
            Some(Kind::Button) => activate_button(hwnd, pressed),
            Some(Kind::Toggle) => flip_toggle(hwnd, pressed),
            _ => {}
        }
    }
    if IsWindow(hwnd).as_bool() {
        repaint(hwnd);
    }
}

// Last list click: (tick, x, y, list id, row) - a second click inside the double-click
// time on the same row is what the list's LBN_DBLCLK used to report.
thread_local! {
    static LAST_CLICK: Cell<(u32, i32, i32, i32, i32)> = const { Cell::new((0, 0, 0, 0, 0)) };
}

// The class carries no CS_DBLCLKS, so the dialog decides a double click itself: the
// second press on the same row within the system double-click time moves the row.
unsafe fn click_list_row(hwnd: HWND, id: i32, x: i32, y: i32, index: Option<usize>) {
    let now = GetTickCount();
    let previous = LAST_CLICK.with(|c| c.get());
    LAST_CLICK.with(|c| c.set((now, x, y, id, index.map_or(-1, |i| i as i32))));
    let Some(index) = index else { return };
    let within_time = windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime();
    let near = (x - previous.1).abs() <= GetSystemMetrics(SM_CXDOUBLECLK)
        && (y - previous.2).abs() <= GetSystemMetrics(SM_CYDOUBLECLK);
    if previous.3 == id
        && previous.4 == index as i32
        && near
        && now.saturating_sub(previous.0) <= within_time
    {
        handle_command(hwnd, id, LBN_DBLCLK as u16);
    }
}

/// Wheel scrolls the list under the cursor by three rows, clamped to its content.
unsafe fn mouse_wheel(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let mut pt = POINT {
        x: (lparam.0 & 0xFFFF) as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    };
    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
    let Some(ctl) = layout(hwnd).into_iter().find(|c| {
        c.kind == Kind::List
            && pt.x >= c.rect.left
            && pt.x < c.rect.right
            && pt.y >= c.rect.top
            && pt.y < c.rect.bottom
    }) else {
        return;
    };
    let visible = visible_rows(ctl.rect);
    let backwards = (((wparam.0 >> 16) & 0xFFFF) as i16) < 0;
    with_list(ctl.id, |l| {
        l.top = if backwards {
            l.top.saturating_add(3)
        } else {
            l.top.saturating_sub(3)
        };
        l.clamp(visible);
    });
    repaint(hwnd);
}

/// Arrows/Home/End/PageUp/PageDown move a list selection; the scroll follows it.
unsafe fn key_list(hwnd: HWND, id: i32, vk: u16) -> bool {
    let Some(ctl) = ctl_of(hwnd, id) else {
        return false;
    };
    let visible = visible_rows(ctl.rect);
    let page = visible.saturating_sub(1).max(1);
    let selected = with_list(id, |l| {
        let len = l.rows.len();
        if len == 0 {
            return None;
        }
        let cur = l.sel;
        let next = match vk {
            v if v == VK_DOWN.0 => cur.map_or(0, |i| (i + 1).min(len - 1)),
            v if v == VK_UP.0 => cur.map_or(0, |i| i.saturating_sub(1)),
            v if v == VK_NEXT.0 => cur.map_or(0, |i| (i + page).min(len - 1)),
            v if v == VK_PRIOR.0 => cur.map_or(0, |i| i.saturating_sub(page)),
            v if v == VK_HOME.0 => 0,
            v if v == VK_END.0 => len - 1,
            _ => return None,
        };
        l.sel = Some(next);
        Some(next)
    });
    if let Some(idx) = selected {
        reveal(id, idx, visible);
    }
    selected.is_some()
}

/// Caret/selection keys inside a field.
unsafe fn key_field(hwnd: HWND, id: i32, vk: u16, shift: bool, ctrl: bool) -> bool {
    let len = field_text(id).chars().count();
    let mut edited = false;
    let mut moved = false;
    with_field(id, |f| {
        let caret = f.caret;
        match vk {
            v if v == VK_LEFT.0 => {
                f.caret = if ctrl { 0 } else { caret.saturating_sub(1) };
                moved = true;
            }
            v if v == VK_RIGHT.0 => {
                f.caret = if ctrl { len } else { (caret + 1).min(len) };
                moved = true;
            }
            v if v == VK_HOME.0 => {
                f.caret = 0;
                moved = true;
            }
            v if v == VK_END.0 => {
                f.caret = len;
                moved = true;
            }
            _ => {}
        }
        if moved && !shift {
            f.anchor = f.caret;
        }
    });
    match vk {
        v if v == VK_BACK.0 => edited = backspace(id),
        v if v == VK_DELETE.0 => edited = delete_forward(id),
        _ => {}
    }
    if edited {
        after_field_edit(hwnd, id);
    } else if moved {
        // The paint scrolls the caret into view; update_caret puts the bar where it landed.
        repaint(hwnd);
        update_caret(hwnd);
    }
    true
}

unsafe fn key_down(hwnd: HWND, vk: u16) {
    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
    match focus() {
        Focus::Field(id) => {
            if ctrl && vk == 'A' as u16 {
                select_all(id);
                repaint(hwnd);
                return;
            }
            if ctrl && vk == 'C' as u16 {
                clipboard_copy(hwnd, id);
                return;
            }
            if ctrl && vk == 'X' as u16 {
                clipboard_cut(hwnd, id);
                return;
            }
            if ctrl && vk == 'V' as u16 {
                clipboard_paste(hwnd, id);
                return;
            }
            // Enter/Ctrl+Enter on a field is the dialog's default action.
            if vk == VK_RETURN.0 {
                activate_button(hwnd, ID_BTN_SAVE_CLOSE);
                return;
            }
            if vk == VK_TAB.0 {
                cycle_focus(hwnd, shift);
                return;
            }
            if key_field(hwnd, id, vk, shift, ctrl) {
                return;
            }
        }
        Focus::List(id) => {
            if vk == VK_TAB.0 {
                cycle_focus(hwnd, shift);
            } else if key_list(hwnd, id, vk) {
                repaint(hwnd);
            }
        }
        Focus::Button(id) => {
            if vk == VK_TAB.0 {
                cycle_focus(hwnd, shift);
            } else if vk == VK_SPACE.0 || vk == VK_RETURN.0 {
                activate_button(hwnd, id);
            }
        }
        Focus::Toggle(id) => {
            if vk == VK_TAB.0 {
                cycle_focus(hwnd, shift);
            } else if vk == VK_SPACE.0 || vk == VK_RETURN.0 {
                flip_toggle(hwnd, id);
            }
        }
        // Nothing focused yet: Tab picks the first control, and no other key acts, so a
        // stray Enter before any click cannot save and close the dialog.
        Focus::None => {
            if vk == VK_TAB.0 {
                cycle_focus(hwnd, shift);
            }
        }
    }
}

/// Printable input into the focused field, then the filter command it used to report.
unsafe fn char_input(hwnd: HWND, wparam: WPARAM) {
    let Focus::Field(id) = focus() else { return };
    let ch = wparam.0 as u32;
    // Control characters arrive here too (Enter, Tab, Ctrl+A after TranslateMessage).
    if ch < 0x20 || ch == 0x7F {
        return;
    }
    let Some(ch) = char::from_u32(ch) else { return };
    if numeric_field(id) && !ch.is_ascii_digit() {
        return;
    }
    insert_at_caret(id, &ch.to_string());
    after_field_edit(hwnd, id);
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            UI.with(|u| *u.borrow_mut() = Some(Ui::new(hwnd)));
            with_ui(|u| apply_chrome(hwnd, u.palette.dark));
            refresh_detected_apps(hwnd);
            load_settings_to_field(hwnd);
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
        WM_SIZE => {
            // Nothing to move: the geometry is derived at paint time. The whole client is
            // repainted before this message returns, so a live resize can never expose a
            // band painted for the previous size.
            repaint(hwnd);
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
            update_caret(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            mouse_move(hwnd, lparam);
            LRESULT(0)
        }
        msg if msg == CONTROLS_MOUSELEAVE => {
            HOT.with(|h| h.set(0));
            LISTS.with(|l| {
                for list in l.borrow_mut().values_mut() {
                    list.hot = None;
                }
            });
            repaint(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            mouse_down(hwnd, lparam);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            mouse_up(hwnd, lparam);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            mouse_wheel(hwnd, wparam, lparam);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            key_down(hwnd, wparam.0 as u16);
            LRESULT(0)
        }
        WM_CHAR => {
            char_input(hwnd, wparam);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            // Over a field the I-beam must survive every mouse move, not just the ones
            // that changed the hot control. DefWindowProc sets the class arrow otherwise.
            if GetFocus() == hwnd {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
                let hit = hit_test(&layout(hwnd), pt.x, pt.y);
                if kind_of(hwnd, hit) == Some(Kind::Edit) {
                    let cursor = LoadCursorW(None, IDC_IBEAM).unwrap_or_default();
                    SetCursor(cursor);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
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
        WM_SETFOCUS | WM_KILLFOCUS => {
            if msg == WM_KILLFOCUS {
                hide_caret(hwnd);
            }
            repaint(hwnd);
            if msg == WM_SETFOCUS {
                update_caret(hwnd);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            free_back_buffer(hwnd);
            free_ss_buffers();
            // Still DefWindowProc: it is what clears the thread's last-active-popup state.
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_DESTROY => {
            hide_caret(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn reinit_for_dpi(hwnd: HWND, dpi: u32) {
    with_ui_mut(|ui| {
        let dip = Dip::from_dpi(dpi);
        let shared = Shared::get(dip.dpi, &ui.palette);
        ui.dip = dip;
        ui.font = HFONT(shared.font as *mut c_void);
        ui.font_small = HFONT(shared.font_small as *mut c_void);
    });

    // A caret created for the old DPI keeps its old size until recreated.
    if CARET_ON.with(|c| c.replace(false)) {
        let _ = DestroyCaret();
    }
    repaint(hwnd);
    update_caret(hwnd);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn lparam_of(x: i32, y: i32) -> isize {
        (((y as u32) << 16) | (x as u32 & 0xFFFF)) as isize
    }

    /// Two dialogs in one test binary would share the thread-local field/list state and
    /// fight over the foreground window: the window tests take turns.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn make_dialog() -> HWND {
        init_common_controls();
        reset_controls();
        DIALOG_STATE.with(|state| {
            *state.borrow_mut() = Some(DialogState {
                config: Arc::new(RwLock::new(Config::default())),
                muting_enabled: Arc::new(AtomicBool::new(true)),
                // COM must answer on this thread for the session refresh inside WM_CREATE.
                audio_manager: Arc::new(AudioManager::new().expect("audio manager")),
                detected_apps: Vec::new(),
                all_detected_apps: Vec::new(),
                filter_detected: String::new(),
                filter_excluded: String::new(),
                filter_always_muted: String::new(),
            });
        });
        unsafe { create_dialog_window().expect("settings window") }
    }

    fn teardown(hwnd: HWND) {
        unsafe {
            let _ = DestroyWindow(hwnd);
            let name = to_wide(SETTINGS_CLASS);
            let _ = UnregisterClassW(
                PCWSTR(name.as_ptr()),
                GetModuleHandleW(None).unwrap_or_default(),
            );
        }
        UI.with(|u| *u.borrow_mut() = None);
        DIALOG_STATE.with(|s| *s.borrow_mut() = None);
        reset_controls();
    }

    /// Control state is thread-local, so a test must not inherit the state of the dialog
    /// it just closed.
    fn reset_controls() {
        FIELDS.with(|f| f.borrow_mut().clear());
        LISTS.with(|l| l.borrow_mut().clear());
        TOGGLES.with(|t| t.borrow_mut().clear());
        BACK.with(|b| b.borrow_mut().clear());
        unsafe { free_ss_buffers() };
        #[cfg(test)]
        SS_BUILDS.with(|c| c.set(0));
        HOT.with(|h| h.set(0));
        PRESSED.with(|p| p.set(0));
        set_focus_value(Focus::None);
        LAST_CLICK.with(|c| c.set((0, 0, 0, 0, 0)));
    }

    const CARDS: [i32; 4] = [
        ID_GRP_DETECTED,
        ID_GRP_EXCLUDED,
        ID_GRP_ALWAYS_MUTED,
        ID_GROUP_SETTINGS,
    ];

    /// No control may sit on another, or a click lands on the wrong one. A card contains
    /// its children by design, so only pairs with exactly one card are exempt.
    fn assert_no_overlaps(hwnd: HWND) {
        let rects: Vec<(i32, RECT)> = layout(hwnd).into_iter().map(|c| (c.id, c.rect)).collect();
        for (i, (id_a, a)) in rects.iter().enumerate() {
            for (id_b, b) in rects.iter().skip(i + 1) {
                let overlap =
                    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
                let containment = CARDS.contains(id_a) != CARDS.contains(id_b);
                assert!(
                    !overlap || containment,
                    "controls {id_a} and {id_b} overlap"
                );
            }
        }
    }

    fn center(rc: RECT) -> (i32, i32) {
        ((rc.left + rc.right) / 2, (rc.top + rc.bottom) / 2)
    }

    fn select_text_row(id: i32, text: &str) {
        with_list(id, |l| l.sel = l.rows.iter().position(|r| r.text == text));
    }

    fn rows_of(id: i32) -> Vec<String> {
        with_list(id, |l| l.rows.iter().map(|r| r.text.clone()).collect())
    }

    fn config_of_dialog() -> Arc<RwLock<Config>> {
        DIALOG_STATE.with(|state| state.borrow().as_ref().unwrap().config.clone())
    }

    /// The dialog's own name, which the rename must not leave behind.
    #[test]
    fn window_title_uses_the_app_name() {
        assert_eq!(
            format!("{} - Settings", crate::tray::APP_NAME),
            "Rust Background Muter - Settings"
        );
    }

    /// A button's backdrop is decided by the layout, so it cannot flip between the card
    /// and the window colour while a resize is still in flight. Every control on a card
    /// must answer the card, and every control outside one the window.
    #[test]
    fn control_surfaces_follow_the_layout() {
        let _serial = serial();
        let hwnd = make_dialog();
        let ctls = layout(hwnd);
        let cards: Vec<RECT> = ctls
            .iter()
            .filter(|c| c.kind == Kind::Card)
            .map(|c| c.rect)
            .collect();
        for ctl in ctls.iter().filter(|c| c.kind != Kind::Card) {
            let (cx, cy) = center(ctl.rect);
            let on_card = cards
                .iter()
                .any(|r| cx >= r.left && cx <= r.right && cy >= r.top && cy <= r.bottom);
            let surface = surface_of(ctl.rect, &cards);
            assert_eq!(
                surface == current_palette().card,
                on_card,
                "control {} backdrop does not match the layout",
                ctl.id
            );
        }
        teardown(hwnd);
    }

    /// A card that covered its children would take their clicks: every control's own
    /// centre must hit-test back to it, and a corner of the window to nothing.
    #[test]
    fn hit_test_lands_on_the_control_a_point_is_over() {
        let _serial = serial();
        let hwnd = make_dialog();
        let ctls = layout(hwnd);
        for ctl in ctls.iter().filter(|c| c.kind != Kind::Card) {
            let (x, y) = center(ctl.rect);
            assert_eq!(
                hit_test(&ctls, x, y),
                ctl.id,
                "centre of control {} hits another control",
                ctl.id
            );
        }
        assert_eq!(hit_test(&ctls, 0, 0), 0, "bare window hit a control");
        assert_no_overlaps(hwnd);
        teardown(hwnd);
    }

    /// A window with no icon shows a blank or generic square in the caption and in the
    /// taskbar hover preview, where the shell reads this app's icon from.
    #[test]
    fn dialog_carries_the_app_icon_in_every_slot() {
        let _serial = serial();
        let hwnd = make_dialog();
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{ICON_BIG, ICON_SMALL, WM_GETICON};
            for slot in [ICON_BIG, ICON_SMALL] {
                let icon = SendMessageW(hwnd, WM_GETICON, WPARAM(slot as usize), LPARAM(0)).0;
                assert!(
                    icon != 0,
                    "icon slot {slot} is empty on the settings window"
                );
            }
        }
        teardown(hwnd);
    }

    /// Typing goes to the dialog, which owns the field text; a filter field must refill
    /// its own list and leave the other panels alone.
    #[test]
    fn fields_accept_typing_and_filter_their_lists() {
        let _serial = serial();
        let hwnd = make_dialog();
        unsafe {
            set_focus_value(Focus::Field(ID_EDIT_SEARCH));
            for ch in ['a', 'l', 'p'] {
                SendMessageW(hwnd, WM_CHAR, WPARAM(ch as usize), LPARAM(0));
            }
            assert_eq!(
                field_text(ID_EDIT_SEARCH),
                "alp",
                "search field dropped a keystroke"
            );

            // In-memory config only: the saving variants would rewrite the user's file.
            DIALOG_STATE.with(|state| {
                let mut state = state.borrow_mut();
                let s = state.as_mut().unwrap();
                let mut config = Config::default();
                config.insert_excluded_app("beta.exe");
                config.insert_excluded_app("gamma.exe");
                config.insert_always_muted_app("delta.exe");
                config.insert_always_muted_app("epsilon.exe");
                s.config = Arc::new(RwLock::new(config));
            });
            refresh_excluded_list(hwnd);
            refresh_always_muted_list(hwnd);

            set_focus_value(Focus::Field(ID_EDIT_FILTER_EXCLUDED));
            SendMessageW(hwnd, WM_CHAR, WPARAM('g' as usize), LPARAM(0));
            assert_eq!(rows_of(ID_LIST_EXCLUDED), vec!["gamma.exe".to_string()]);
            assert_eq!(
                rows_of(ID_LIST_ALWAYS_MUTED).len(),
                2,
                "filtering one list refilled another"
            );

            // Clearing the filter keeps the row that was selected before.
            select_row(hwnd, ID_LIST_EXCLUDED, 0);
            with_field(ID_EDIT_FILTER_EXCLUDED, |f| {
                f.text.clear();
                f.caret = 0;
                f.anchor = 0;
            });
            handle_command(hwnd, ID_EDIT_FILTER_EXCLUDED, EN_CHANGE);
            assert_eq!(rows_of(ID_LIST_EXCLUDED).len(), 2, "filter did not clear");
            assert_eq!(
                with_list(ID_LIST_EXCLUDED, |l| l.selected_text()),
                Some("gamma.exe".to_string()),
                "the surviving row lost its selection"
            );
        }
        teardown(hwnd);
    }

    /// A list click selects the row under the point; a second click on the same row
    /// inside the double-click time moves it across the panels.
    #[test]
    fn list_rows_select_and_double_click_moves_them() {
        let _serial = serial();
        let hwnd = make_dialog();
        unsafe {
            DIALOG_STATE.with(|state| {
                let mut state = state.borrow_mut();
                let s = state.as_mut().unwrap();
                let mut config = Config::default();
                config.insert_excluded_app("gamma.exe");
                config.insert_always_muted_app("delta.exe");
                s.config = Arc::new(RwLock::new(config));
                s.all_detected_apps = vec![(1, "alpha.exe".into()), (2, "beta.exe".into())];
            });
            // Not refresh_detected_apps: that would replace the rows with the sessions
            // of whatever the test host plays.
            refill_detected_list(hwnd);
            refresh_excluded_list(hwnd);
            refresh_always_muted_list(hwnd);

            let ctl = ctl_of(hwnd, ID_LIST_DETECTED).unwrap();
            let (x, y) = (center(ctl.rect).0, ctl.rect.top + row_height() / 2);
            SendMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(1), LPARAM(lparam_of(x, y)));
            SendMessageW(hwnd, WM_LBUTTONUP, WPARAM(0), LPARAM(lparam_of(x, y)));
            assert_eq!(
                list_sel(ID_LIST_DETECTED),
                Some(0),
                "row click did not select"
            );
            assert_eq!(focus(), Focus::List(ID_LIST_DETECTED));
            assert_eq!(
                rows_of(ID_LIST_EXCLUDED).len(),
                1,
                "a single click moved a row"
            );

            // The same point immediately again is the double click.
            SendMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(1), LPARAM(lparam_of(x, y)));
            assert!(config_of_dialog().read().is_excluded("alpha.exe"));
            SendMessageW(hwnd, WM_LBUTTONUP, WPARAM(0), LPARAM(lparam_of(x, y)));

            // excluded -> detected, and always muted -> detected. By text: the panel is
            // sorted, so row 0 is not necessarily the row this test added.
            select_text_row(ID_LIST_EXCLUDED, "gamma.exe");
            handle_command(hwnd, ID_LIST_EXCLUDED, LBN_DBLCLK as u16);
            select_text_row(ID_LIST_ALWAYS_MUTED, "delta.exe");
            handle_command(hwnd, ID_LIST_ALWAYS_MUTED, LBN_DBLCLK as u16);
            let config = config_of_dialog();
            let config = config.read();
            assert!(!config.is_excluded("gamma.exe"), "exclusion survived");
            assert!(
                !config.always_muted_apps.contains("delta.exe"),
                "always muted row survived"
            );
        }
        teardown(hwnd);
    }

    /// Every WM_SIZE must end on one complete frame and must not lose list content: a
    /// resize repaints the rows it already has, it does not drop them.
    #[test]
    fn resize_paints_and_keeps_the_rows() {
        let _serial = serial();
        let hwnd = make_dialog();
        unsafe {
            let mut config = Config::default();
            for n in 0..40 {
                config.insert_excluded_app(&format!("app{n}.exe"));
            }
            DIALOG_STATE.with(|state| {
                state.borrow_mut().as_mut().unwrap().config = Arc::new(RwLock::new(config));
            });
            refresh_excluded_list(hwnd);
            let before = rows_of(ID_LIST_EXCLUDED).len();
            assert!(before > 0, "no rows to guard");

            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let (w, h) = (client.right, client.bottom);
            for (dw, dh) in [(0, 0), (40, 30), (-40, -30), (0, 120)] {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    w + dw,
                    h + dh,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                assert_eq!(
                    rows_of(ID_LIST_EXCLUDED).len(),
                    before,
                    "a resize lost rows"
                );
            }
            let mut dirty = RECT::default();
            assert!(
                !windows::Win32::Graphics::Gdi::GetUpdateRect(hwnd, Some(&mut dirty), false)
                    .as_bool(),
                "WM_SIZE returned with the window still dirty"
            );
            assert_no_overlaps(hwnd);
        }
        teardown(hwnd);
    }

    /// Rows in the exclusion / always-muted panels carry no PID, so their icon comes from
    /// the first running process with that exe name.
    #[test]
    fn named_rows_resolve_an_icon_from_a_running_process() {
        unsafe {
            MISSING_PATHS
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .clear();
            ICON_OF_NAME
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .clear();
            let a = row_icon(0, "explorer.exe");
            let b = row_icon(0, "nonexistent-xyz.exe");
            assert!(a.is_some(), "explorer.exe row drew no icon");
            assert!(b.is_none(), "a missing exe name drew an icon");
        }
    }

    /// Repainting an unchanged toggle must reuse its supersampled bitmap; a state change
    /// must rebuild it, or the pill would show the previous state.
    #[test]
    fn toggle_supersample_buffer_is_reused_until_state_changes() {
        let _serial = serial();
        let hwnd = make_dialog();
        unsafe {
            let ctls = layout(hwnd);
            let cards: Vec<RECT> = ctls
                .iter()
                .filter(|c| c.kind == Kind::Card)
                .map(|c| c.rect)
                .collect();
            let ctl = ctls
                .iter()
                .find(|c| c.kind == Kind::Toggle)
                .unwrap()
                .clone();
            let p = current_palette();
            let dip = dip_of();

            let mut full = RECT::default();
            let _ = GetClientRect(hwnd, &mut full);
            let window_dc = GetDC(hwnd);
            let dc = CreateCompatibleDC(window_dc);
            let bmp = CreateCompatibleBitmap(window_dc, full.right, full.bottom);
            let _ = ReleaseDC(hwnd, window_dc);
            SelectObject(dc, bmp);

            // The dialog's own creation paint already filled a slot; start from empty.
            free_ss_buffers();
            SS_BUILDS.with(|c| c.set(0));
            draw_toggle(hwnd, dc, &ctl, &p, dip, false, &cards);
            let first = SS_BUILDS.with(|c| c.get());
            assert!(first > 0, "the toggle never built a supersampled bitmap");
            draw_toggle(hwnd, dc, &ctl, &p, dip, false, &cards);
            assert_eq!(
                SS_BUILDS.with(|c| c.get()),
                first,
                "an unchanged toggle re-rendered its bitmap"
            );

            set_toggle(ctl.id, !toggle(ctl.id));
            draw_toggle(hwnd, dc, &ctl, &p, dip, false, &cards);
            assert_eq!(
                SS_BUILDS.with(|c| c.get()),
                first + 1,
                "a flipped toggle kept the old bitmap"
            );

            let _ = DeleteDC(dc);
            let _ = DeleteObject(bmp);
        }
        teardown(hwnd);
    }


    /// Every control must land inside the card that owns it, and no row may collide with
    /// another. A pill painted away from its row (all three at one height) is exactly the
    /// class of bug a wrong scale or a wrong origin introduces, so pin the numbers down.
    #[test]
    fn toggle_rows_are_distinct_and_inside_the_settings_card() {
        let _serial = serial();
        let hwnd = make_dialog();
        let ctls = layout(hwnd);
        let card = ctls
            .iter()
            .find(|c| c.id == ID_GROUP_SETTINGS)
            .expect("settings card")
            .rect;
        let toggles: Vec<RECT> = ctls
            .iter()
            .filter(|c| c.kind == Kind::Toggle)
            .map(|c| c.rect)
            .collect();
        assert_eq!(toggles.len(), 3, "three toggles");
        for (i, a) in toggles.iter().enumerate() {
            assert!(
                a.top >= card.top && a.bottom <= card.bottom && a.left >= card.left
                    && a.right <= card.right,
                "toggle {i} at {a:?} is outside the settings card {card:?}"
            );
            for b in toggles.iter().skip(i + 1) {
                assert!(
                    a.bottom <= b.top || b.bottom <= a.top,
                    "toggles {a:?} and {b:?} overlap vertically"
                );
            }
        }
        // Rows are separated by at least one row height, not stacked on each other.
        let gap = toggles[1].top - toggles[0].top;
        assert!(gap >= row_height(), "toggle rows are only {gap}px apart");
        teardown(hwnd);
    }

    /// The frame must put each control's colour exactly where the layout says it is. A
    /// sample at the centre of every control's rect reads the buffer back, so a control
    /// drawn at the wrong coordinates (or one painted over another) fails here.
    #[test]
    fn frame_paints_each_control_in_its_own_rect() {
        let _serial = serial();
        let hwnd = make_dialog();
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let (w, h) = (rc.right, rc.bottom);
            let window_dc = GetDC(hwnd);
            let mem = CreateCompatibleDC(window_dc);
            let bmp = CreateCompatibleBitmap(window_dc, w, h);
            let _ = ReleaseDC(hwnd, window_dc);
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            paint_frame(hwnd, mem, &rc);

            let p = current_palette();
            let at = |x: i32, y: i32| {
                let c = windows::Win32::Graphics::Gdi::GetPixel(mem, x, y);
                (c.0 & 0xFF) as u32 | (c.0 & 0xFF00) | (c.0 & 0xFF0000)
            };
            for ctl in layout(hwnd) {
                let (cx, cy) = (
                    (ctl.rect.left + ctl.rect.right) / 2,
                    (ctl.rect.top + ctl.rect.bottom) / 2,
                );
                match ctl.kind {
                    Kind::Card => {
                        // The centre of a card is behind its own list, so sample the caption row.
                        let y = ctl.rect.top + dip_of().px(10.0);
                        let seen = at(cx, y);
                        assert_eq!(seen, p.card, "card {} centre is not the card colour", ctl.id);
                    }
                    Kind::Toggle => {
                        // Sample the track, never the knob: the knob sits on the left when
                        // the toggle is off and on the right when it is on.
                        let on = toggle(ctl.id);
                        let x = if on {
                            ctl.rect.left + dip_of().px(5.0)
                        } else {
                            ctl.rect.left + dip_of().px(32.0)
                        };
                        let y = (ctl.rect.top + ctl.rect.bottom) / 2;
                        let seen = at(x, y);
                        let want = if on { p.accent } else { p.well };
                        assert_eq!(
                            seen, want,
                            "toggle {} pill is not drawn at its rect {x},{y}", ctl.id
                        );
                    }
                    // A list is filled with rows and a field with text, so the body colour is
                    // counted over the rect instead of trusted from one pixel.
                    Kind::List | Kind::Edit => {
                        let mut same = 0;
                        let mut total = 0;
                        for y in (ctl.rect.top + 2..ctl.rect.bottom - 2).step_by(5) {
                            for x in (ctl.rect.left + 2..ctl.rect.right - 2).step_by(5) {
                                total += 1;
                                if at(x, y) == p.well {
                                    same += 1;
                                }
                            }
                        }
                        assert!(
                            total > 0 && same * 2 > total,
                            "{} {} body covers {same}/{total} of its rect",
                            if ctl.kind == Kind::List { "list" } else { "field" },
                            ctl.id
                        );
                    }
                    Kind::Button => {
                        let primary = ctl.id == ID_BTN_SAVE_CLOSE;
                        let want = if primary { p.accent } else { p.card };
                        // The label covers part of the rect, so count the fill over the whole
                        // button instead of trusting one pixel.
                        let mut same = 0;
                        let mut total = 0;
                        for y in (ctl.rect.top + 6..ctl.rect.bottom - 6).step_by(3) {
                            for x in (ctl.rect.left + 6..ctl.rect.right - 6).step_by(3) {
                                total += 1;
                                if at(x, y) == want {
                                    same += 1;
                                }
                            }
                        }
                        assert!(
                            total > 0 && same * 2 > total,
                            "button {} fill covers {same}/{total} of its rect, wanted {want:#08x}",
                            ctl.id
                        );
                    }
                    Kind::Label => {}
                }
            }
            // A point no control covers shows the window colour.
            let bare = (w - dip_of().px(2.0), h - dip_of().px(2.0));
            assert_eq!(at(bare.0, bare.1), p.window, "bare window pixels");

            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(mem);
        }
        teardown(hwnd);
    }

    /// Writes one painted frame to .tmp/frame.bin (raw BGRA) so the render can be inspected
    /// offscreen. Development aid only.
    #[test]
    #[ignore]
    fn dump_frame() {
        let _serial = serial();
        let hwnd = make_dialog();
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let (w, h) = (rc.right, rc.bottom);
            let window_dc = GetDC(hwnd);
            let mem = CreateCompatibleDC(window_dc);
            let bmp = CreateCompatibleBitmap(window_dc, w, h);
            let _ = ReleaseDC(hwnd, window_dc);
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            paint_frame(hwnd, mem, &rc);

            let mut bmi = windows::Win32::Graphics::Gdi::BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h;
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = 0;
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            let _ = windows::Win32::Graphics::Gdi::GetDIBits(
                mem,
                bmp,
                0,
                h as u32,
                Some(pixels.as_mut_ptr() as *mut c_void),
                &mut bmi,
                windows::Win32::Graphics::Gdi::DIB_RGB_COLORS,
            );
            std::fs::write(".tmp/frame.bin", &pixels).unwrap();
            std::fs::write(".tmp/frame.size", format!("{w}x{h}")).unwrap();

            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(mem);
        }
        teardown(hwnd);
    }

}
