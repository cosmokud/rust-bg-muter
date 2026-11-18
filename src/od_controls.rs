//! Owner-drawn Win32 controls and GDI paint helpers.
//!
//! * `BgMuterToggle` - pill switch, reports BM_SETCHECK/BM_GETCHECK and BN_CLICKED.
//! * `BgMuterCard`   - rounded section panel with a caption.
//! * hover subclass  - hot state for native BUTTONs and hovered row for LISTBOXes.
//!
//! Pure GDI (CPU rasterized): no themes, no GPU, no extra allocation per paint.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteObject, DrawTextW, Ellipse,
    EndPaint, FillRect, GetDC, GetStockObject, InvalidateRect, ReleaseDC, RoundRect, SelectObject,
    SetBkMode, SetDCBrushColor, SetDCPenColor, SetTextColor, DRAW_TEXT_FORMAT, DC_BRUSH, DC_PEN,
    DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ,
    NULL_BRUSH, PAINTSTRUCT, SRCCOPY, TRANSPARENT, SetViewportOrgEx,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, VK_RETURN, VK_SPACE,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::settings_dialog::current_palette;
use crate::theme::Dip;

pub const CLASS_TOGGLE: &str = "BgMuterToggle";
pub const CLASS_CARD: &str = "BgMuterCard";

/// Selects DC_BRUSH and DC_PEN in `fill`/`pen`, draws, then restores the DC.
/// Colours are DC attributes, so a paint pass creates no GDI objects at all (measured:
/// the old CreateSolidBrush + CreatePen per row left ~4 objects behind after each repaint).
unsafe fn shaped(hdc: HDC, fill: u32, pen: u32, draw: impl FnOnce()) {
    let brush = GetStockObject(DC_BRUSH);
    let pen_obj = GetStockObject(DC_PEN);
    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, pen_obj);
    SetDCBrushColor(hdc, COLORREF(fill));
    SetDCPenColor(hdc, COLORREF(pen));
    draw();
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
}

pub fn fill(hdc: HDC, rc: &RECT, color: u32) {
    unsafe {
        let brush = GetStockObject(DC_BRUSH);
        let old = SelectObject(hdc, brush);
        SetDCBrushColor(hdc, COLORREF(color));
        FillRect(hdc, rc, HBRUSH(brush.0));
        SelectObject(hdc, old);
    }
}

/// Filled rounded rect; `border` draws a 1px outline in that color.
pub fn round(hdc: HDC, rc: &RECT, radius: i32, fill: u32, border: Option<u32>) {
    let edge = border.unwrap_or(fill);
    unsafe {
        shaped(hdc, fill, edge, || {
            let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, radius, radius);
        });
    }
}

/// Outline only (1px), rounded.
pub fn stroke(hdc: HDC, rc: &RECT, radius: i32, color: u32) {
    unsafe {
        let brush = GetStockObject(NULL_BRUSH);
        let pen = GetStockObject(DC_PEN);
        let old_brush = SelectObject(hdc, brush);
        let old_pen = SelectObject(hdc, pen);
        SetDCPenColor(hdc, COLORREF(color));
        let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, radius, radius);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
    }
}

/// Filled circle (toggle knob).
pub fn dot(hdc: HDC, rc: &RECT, color: u32) {
    unsafe {
        shaped(hdc, color, color, || {
            let _ = Ellipse(hdc, rc.left, rc.top, rc.right, rc.bottom);
        });
    }
}

pub fn text(hdc: HDC, rc: &RECT, s: &str, font: HFONT, color: u32, flags: DRAW_TEXT_FORMAT) {
    let mut wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let old_font = SelectObject(hdc, HGDIOBJ(font.0));
            let old_color = SetTextColor(hdc, COLORREF(color));
            SetBkMode(hdc, TRANSPARENT);
            let mut r = *rc;
            DrawTextW(hdc, &mut wide, &mut r, flags);
            SetTextColor(hdc, old_color);
            SelectObject(hdc, old_font);
    }
}

pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

pub fn control_font(hwnd: HWND) -> HFONT {
    let h = unsafe { SendMessageW(hwnd, WM_GETFONT, WPARAM(0), LPARAM(0)) };
    HFONT(h.0 as *mut c_void)
}

/// Blends `base` toward `toward` by `amount` (0..=1), channel-wise on a COLORREF.
pub fn mix(base: u32, toward: u32, amount: f32) -> u32 {
    let ch = |shift: u32| {
        let a = ((base >> shift) & 0xFF) as f32;
        let b = ((toward >> shift) & 0xFF) as f32;
        ((a + (b - a) * amount).round() as u32) & 0xFF
    };
    ch(0) | (ch(8) << 8) | (ch(16) << 16)
}

fn inside(hwnd: HWND, lparam: LPARAM) -> bool {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return false;
    }
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    x >= 0 && y >= 0 && x < rc.right && y < rc.bottom
}

fn class_is_listbox(hwnd: HWND) -> bool {
    let mut buf = [0u16; 32];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    len > 0 && String::from_utf16_lossy(&buf[..len as usize]).eq_ignore_ascii_case("listbox")
}

unsafe fn track_leave(hwnd: HWND) {
    let mut tme = windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT>(
        ) as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    let _ = TrackMouseEvent(&mut tme);
}

struct Buffer {
    mem: HDC,
    bmp: HBITMAP,
    w: i32,
    h: i32,
}

thread_local! {
    // One off-screen buffer per window, so a frame that paints the dialog and every child
    // never rebuilds a buffer per control. GDI objects are created only on a size change.
    static BUFFERS: RefCell<HashMap<isize, Buffer>> = RefCell::new(HashMap::new());
}

/// Paints `size` into a cached off-screen buffer owned by `hwnd`, then flips it to `target`
/// at `at` with one BitBlt/SRCCOPY; `f` gets the memory DC and a rect whose origin is the
/// buffer's. The target DC's clip (BeginPaint's update region) still clips the flip, so a
/// partial update stays partial. Every surface we paint goes through here: a direct write
/// to the window DC shows the old pixels of that band for one frame.
///
/// # Safety
/// `hwnd` must be a live window and `target` a valid DC.
pub unsafe fn dib(
    hwnd: HWND,
    target: HDC,
    at: (i32, i32),
    size: (i32, i32),
    erase: u32,
    f: impl FnOnce(HDC, &RECT),
) {
    if size.0 <= 0 || size.1 <= 0 {
        return;
    }
    let key = hwnd.0 as isize;
    let mem = BUFFERS.with(|b| {
        let mut map = b.borrow_mut();
        let slot = map.entry(key).or_insert_with(|| Buffer {
            mem: HDC::default(),
            bmp: HBITMAP::default(),
            w: 0,
            h: 0,
        });
        let stale = slot.bmp.0.is_null() || slot.w != size.0 || slot.h != size.1;
        if stale {
            let win_dc = GetDC(hwnd);
            if slot.mem.0.is_null() {
                slot.mem = CreateCompatibleDC(win_dc);
            }
            let bmp = CreateCompatibleBitmap(win_dc, size.0, size.1);
            ReleaseDC(hwnd, win_dc);
            if !slot.bmp.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(slot.bmp.0));
            }
            slot.bmp = bmp;
            slot.w = size.0;
            slot.h = size.1;
        }
        slot.mem
    });
    if mem.0.is_null() {
        return;
    }
    let rc = RECT {
        left: 0,
        top: 0,
        right: size.0,
        bottom: size.1,
    };
    // Client-space rect handed to `f`: the viewport origin is shifted to `at`, so callers
    // can draw entirely in their own client coordinates, exactly as on the window DC.
    let client = RECT {
        left: at.0,
        top: at.1,
        right: at.0 + size.0,
        bottom: at.1 + size.1,
    };
    BUFFERS.with(|b| {
        let bmp = b.borrow().get(&key).map(|s| s.bmp).unwrap_or_default();
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        // Start from the flat surface colour: the buffer is reused, so every pixel the
        // callback leaves unpainted must not show the previous frame.
        fill(mem, &rc, erase);
        let _ = SetViewportOrgEx(mem, at.0, at.1, None);
        f(mem, &client);
        // 0,0 origin for the flip so the source and destination spaces are 1:1.
        let _ = SetViewportOrgEx(mem, 0, 0, None);
        let _ = BitBlt(target, at.0, at.1, size.0, size.1, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
    });
}

/// Frees the off-screen buffer of `hwnd` on WM_NCDESTROY, or a closed window keeps its
/// bitmap (and the size it was built for) for the life of the thread.
///
/// # Safety
/// `hwnd` must be a window that painted through `dib`.
pub unsafe fn release(hwnd: HWND) {
    let slot = BUFFERS.with(|b| b.borrow_mut().remove(&(hwnd.0 as isize)));
    if let Some(slot) = slot {
        if !slot.bmp.0.is_null() {
            let _ = DeleteObject(HGDIOBJ(slot.bmp.0));
        }
        if !slot.mem.0.is_null() {
            let _ = windows::Win32::Graphics::Gdi::DeleteDC(slot.mem);
        }
    }
}

unsafe fn paint_banner(hwnd: HWND, erase: u32, f: impl FnOnce(HDC, &RECT)) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    dib(hwnd, hdc, (0, 0), (rc.right, rc.bottom), erase, f);
    let _ = EndPaint(hwnd, &ps);
}

// --------------------------------------------------------------- pill toggle

#[derive(Default)]
struct Toggle {
    on: bool,
    hot: bool,
    pressed: bool,
}

fn toggle_of(hwnd: HWND) -> *mut Toggle {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Toggle }
}

/// Sets the state without notifying the parent (used by load_settings_to_controls).
///
/// # Safety
/// `hwnd` must be a live toggle window.
pub unsafe fn toggle_set(hwnd: HWND, on: bool) {
    if let Some(t) = toggle_of(hwnd).as_mut() {
        t.on = on;
    }
    let _ = InvalidateRect(hwnd, None, false);
}

/// # Safety
/// `hwnd` must be null or a live toggle window.
pub unsafe fn toggle_get(hwnd: HWND) -> bool {
    toggle_of(hwnd).as_ref().is_some_and(|t| t.on)
}

unsafe fn flip(hwnd: HWND) {
    let on = !toggle_get(hwnd);
    toggle_set(hwnd, on);
    if let Ok(parent) = GetParent(hwnd) {
        let id = GetDlgCtrlID(hwnd);
        // BN_CLICKED: the dialog's WM_COMMAND branches stay unchanged.
        SendMessageW(
            parent,
            WM_COMMAND,
            WPARAM((id as usize) | ((BN_CLICKED as usize) << 16)),
            LPARAM(hwnd.0 as isize),
        );
    }
}

unsafe fn paint_toggle(hwnd: HWND) {
    let p = current_palette();
    let dip = Dip::at(hwnd);
    // A custom class never receives `WM_SETFONT` unless the dialog sends it, and
    // `WM_GETFONT` answers null until then: use the dialog's font in that case.
    let font = {
        let f = control_font(hwnd);
        if f.0.is_null() {
            crate::settings_dialog::ui_font()
        } else {
            f
        }
    };
    let focused = GetFocus() == hwnd;
    let (on, hot, pressed) = match toggle_of(hwnd).as_ref() {
        Some(t) => (t.on, t.hot, t.pressed),
        None => (false, false, false),
    };

    // Toggles sit on the settings card.
    paint_banner(hwnd, p.card, |hdc, rc| {
        let pill_h = dip.px(18.0);
        let pill = RECT {
            left: rc.left + dip.px(1.0),
            top: (rc.bottom - pill_h) / 2,
            right: rc.left + dip.px(1.0) + dip.px(36.0),
            bottom: (rc.bottom - pill_h) / 2 + pill_h,
        };
        let track = if on {
            if pressed {
                mix(p.accent, p.text, 0.25)
            } else if hot {
                mix(p.accent, p.text, 0.12)
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
        round(hdc, &pill, pill_h / 2, track, (!on).then_some(p.hairline));

        let knob_d = pill_h - dip.px(4.0);
        let knob_x = if on {
            pill.right - dip.px(2.0) - knob_d
        } else {
            pill.left + dip.px(2.0)
        };
        dot(
            hdc,
            &RECT {
                left: knob_x,
                top: pill.top + dip.px(2.0),
                right: knob_x + knob_d,
                bottom: pill.top + dip.px(2.0) + knob_d,
            },
            if on { p.on_accent } else { p.secondary },
        );

        if focused {
            stroke(
                hdc,
                &RECT {
                    left: pill.left - dip.px(2.0),
                    top: pill.top - dip.px(2.0),
                    right: pill.right + dip.px(2.0),
                    bottom: pill.bottom + dip.px(2.0),
                },
                pill_h / 2 + dip.px(2.0),
                p.accent,
            );
        }

        let label = window_text(hwnd);
        if !label.is_empty() {
            text(
                hdc,
                &RECT {
                    left: pill.right + dip.px(10.0),
                    right: rc.right,
                    ..*rc
                },
                &label,
                font,
                p.text,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
            );
        }
    });
}

unsafe extern "system" fn toggle_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                Box::into_raw(Box::new(Toggle::default())) as isize,
            );
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let t = toggle_of(hwnd);
            if !t.is_null() {
                drop(Box::from_raw(t));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            release(hwnd);
            LRESULT(0)
        }
        BM_SETCHECK => {
            toggle_set(hwnd, wparam.0 == 1);
            LRESULT(0)
        }
        BM_GETCHECK => LRESULT(toggle_get(hwnd) as isize),
        WM_LBUTTONDOWN => {
            if let Some(t) = toggle_of(hwnd).as_mut() {
                t.pressed = true;
            }
            let _ = SetFocus(hwnd);
            SetCapture(hwnd);
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let was_pressed = toggle_of(hwnd)
                .as_mut()
                .is_some_and(|t| std::mem::take(&mut t.pressed));
            let _ = ReleaseCapture();
            if was_pressed && inside(hwnd, lparam) {
                flip(hwnd);
            }
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some(t) = toggle_of(hwnd).as_mut() {
                if !t.hot {
                    t.hot = true;
                    track_leave(hwnd);
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if let Some(t) = toggle_of(hwnd).as_mut() {
                t.hot = false;
            }
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let key = wparam.0 as u16;
            if key == VK_SPACE.0 || key == VK_RETURN.0 {
                flip(hwnd);
            }
            LRESULT(0)
        }
        WM_GETDLGCODE => LRESULT((DLGC_WANTCHARS | DLGC_BUTTON) as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint_toggle(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ------------------------------------------------------------------ card panel

/// Rounded card with a caption; the caption is the window title.
unsafe extern "system" fn card_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let p = current_palette();
            let dip = Dip::at(hwnd);
            let font = crate::settings_dialog::ui_font();
            paint_banner(hwnd, p.window, |hdc, rc| {
                let card = RECT {
                    left: rc.left + dip.px(1.0),
                    top: rc.top + dip.px(1.0),
                    right: rc.right - dip.px(1.0),
                    bottom: rc.bottom - dip.px(1.0),
                };
                round(hdc, &card, dip.px(8.0), p.card, Some(p.hairline));

                let caption = window_text(hwnd);
                if !caption.is_empty() {
                    text(
                        hdc,
                        &RECT {
                            left: card.left + dip.px(12.0),
                            right: card.right - dip.px(12.0),
                            top: card.top + dip.px(6.0),
                            bottom: card.top + dip.px(6.0) + dip.px(20.0),
                        },
                        &caption,
                        font,
                        p.secondary,
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
                    );
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Registers both owner-draw classes once; ERROR_CLASS_ALREADY_EXISTS is harmless.
pub fn register_classes(hinstance: HINSTANCE) {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() };
    for (name, proc_fn) in [
        (CLASS_TOGGLE, toggle_proc as unsafe extern "system" fn(_, _, _, _) -> _),
        (CLASS_CARD, card_proc as unsafe extern "system" fn(_, _, _, _) -> _),
    ] {
        let class = to_wide(name);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_fn),
            hInstance: hinstance,
            hCursor: cursor,
            // Owner-drawn classes are repositioned by the dialog's layout pass; a size
            // change must repaint the whole control, never a subset of it.
            style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..WNDCLASSEXW::default()
        };
        unsafe {
            RegisterClassExW(&wc);
        }
    }
}

// ------------------------------------------- hover tracking for native controls

#[derive(Clone, Copy, Default)]
struct Hover {
    hot: bool,
    item: isize,
}

thread_local! {
    static HOVER: RefCell<HashMap<isize, Hover>> = RefCell::new(HashMap::new());
}

const HOVER_SUBCLASS_ID: usize = 1;

unsafe extern "system" fn hover_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _refdata: usize,
) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let item = if class_is_listbox(hwnd) {
                // HIWORD(DWORD) != 0 => the point is outside the client area.
                let point = SendMessageW(hwnd, LB_ITEMFROMPOINT, WPARAM(0), LPARAM(lparam.0));
                if (point.0 >> 16) != 0 {
                    -1
                } else {
                    point.0 & 0xFFFF
                }
            } else {
                -1
            };
            let changed = HOVER.with(|h| {
                let mut h = h.borrow_mut();
                let entry = h.entry(hwnd.0 as isize).or_default();
                let old = *entry;
                entry.hot = true;
                entry.item = item;
                old.hot != entry.hot || old.item != item
            });
            if changed {
                track_leave(hwnd);
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
        WM_MOUSELEAVE => {
            let changed = HOVER.with(|h| {
                let mut h = h.borrow_mut();
                match h.get_mut(&(hwnd.0 as isize)) {
                    Some(e) if e.hot || e.item >= 0 => {
                        e.hot = false;
                        e.item = -1;
                        true
                    }
                    _ => false,
                }
            });
            if changed {
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
        WM_NCDESTROY => {
            // Owner-drawn BUTTONs and LISTBOXes paint through the buffer cache: free it
            // here, the only place that sees these controls die.
            release(hwnd);
            let _ = RemoveWindowSubclass(hwnd, Some(hover_subclass), HOVER_SUBCLASS_ID);
            HOVER.with(|h| {
                h.borrow_mut().remove(&(hwnd.0 as isize));
            });
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// Attaches hot/row tracking to an owner-drawn native control (BUTTON or LISTBOX).
///
/// # Safety
/// `hwnd` must be a live control window; the subclass unhooks on WM_NCDESTROY.
pub unsafe fn attach_hover(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let _ = SetWindowSubclass(hwnd, Some(hover_subclass), HOVER_SUBCLASS_ID, 0);
}

pub fn is_hot(hwnd: HWND) -> bool {
    HOVER.with(|h| h.borrow().get(&(hwnd.0 as isize)).is_some_and(|e| e.hot))
}

/// Hovered list row, or -1.
pub fn hot_item(hwnd: HWND) -> isize {
    HOVER.with(|h| h.borrow().get(&(hwnd.0 as isize)).map_or(-1, |e| e.item))
}

