//! Confirmation for the mixer reset, as its own dark top-level window.
//!
//! Not a `MessageBoxW`: the native box is drawn by the shell in the shell's own theme,
//! so it can never take the app palette. This window is modal in the Win32 sense - the
//! settings dialog that owns it is disabled for its lifetime and it runs the thread's
//! message loop until it is answered - without being a child, so it is never clipped to
//! the parent and never moves with it.
//!
//! Paint and input mirror the settings dialog: one memory bitmap per size, one `BitBlt`
//! per frame, GDI only, and every number in DIPs scaled by this window's own DPI.

use crate::od_controls;
use crate::settings_dialog::{current_palette, make_font};
use crate::theme::{apply_chrome, Dip, Palette};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    EndPaint, GetDC, GetTextExtentPoint32W, InvalidateRect, ReleaseDC, SelectObject, UpdateWindow,
    DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_TOP, DT_VCENTER, DT_WORDBREAK, HBITMAP,
    HBRUSH, HDC, HFONT, HGDIOBJ, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE as CONTROLS_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE,
    TRACKMOUSEEVENT, VK_ESCAPE, VK_RETURN, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const ID_BTN_CONFIRM: i32 = 201;
const ID_BTN_CANCEL: i32 = 202;

const DIALOG_CLASS: &str = "BgMuterConfirmClass";

const WIDTH: f32 = 480.0;
const HEIGHT: f32 = 196.0;
const PAD: f32 = 16.0;
const BUTTON_W: f32 = 100.0;
const BUTTON_H: f32 = 32.0;
const GAP: f32 = 8.0;

thread_local! {
    static HOT: Cell<i32> = const { Cell::new(0) };
    static PRESSED: Cell<i32> = const { Cell::new(0) };
    static BACK: RefCell<HashMap<isize, (HDC, HBITMAP, i32, i32)>> = RefCell::new(HashMap::new());
    /// Font handles per DPI: the prompt opens many times, and each rebuild would be a
    /// GDI object kept for the process lifetime.
    static FONTS: RefCell<HashMap<u32, (isize, isize)>> = RefCell::new(HashMap::new());
}

/// Text shown by the open prompt; fixed by the caller, read by paint.
#[derive(Clone, Copy)]
struct Prompt {
    title: &'static str,
    body: &'static str,
    confirm: &'static str,
}

thread_local! {
    static PROMPT: Cell<Prompt> = const {
        Cell::new(Prompt { title: "", body: "", confirm: "Reset" })
    };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Shows the prompt and blocks until it is answered; `true` means the user confirmed.
/// The parent is disabled before the prompt can be seen and re-enabled only after it is
/// gone, so no click or key can reach the settings dialog while the question is up.
pub fn open_confirm(
    parent: HWND,
    title: &'static str,
    body: &'static str,
    confirm: &'static str,
) -> bool {
    PROMPT.with(|p| {
        p.set(Prompt {
            title,
            body,
            confirm,
        })
    });
    HOT.with(|h| h.set(0));
    PRESSED.with(|p| p.set(0));

    unsafe {
        // Disabled before the prompt exists: otherwise a click landing between the
        // prompt appearing and the parent going dead reaches the settings dialog.
        let _ = EnableWindow(parent, false);
        let Some(hwnd) = create_prompt_window(parent) else {
            let _ = EnableWindow(parent, true);
            return false;
        };

        // Same pump style as the settings dialog: this is the same thread, and the
        // caller is deep inside its own message handler, so the nested loop here only
        // ever sees messages for this prompt.
        let mut msg = MSG::default();
        while IsWindow(hwnd).as_bool() && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        if IsWindow(parent).as_bool() {
            let _ = EnableWindow(parent, true);
            let _ = SetForegroundWindow(parent);
            let _ = SetFocus(parent);
        }

        ANSWER.with(|a| a.get())
    }
}

thread_local! {
    /// The answer, written before the window is destroyed and read after the pump.
    static ANSWER: Cell<bool> = const { Cell::new(false) };
}

unsafe fn create_prompt_window(parent: HWND) -> Option<HWND> {
    let hmodule = GetModuleHandleW(None).unwrap_or_default();
    let class_name = wide(DIALOG_CLASS);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
        lpfnWndProc: Some(window_proc),
        hInstance: hmodule.into(),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        // Every pixel is painted here; no system erase brush.
        hbrBackground: HBRUSH::default(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..WNDCLASSEXW::default()
    };
    // Registered once and left registered: reopening the prompt must not race a
    // re-registration, and Windows drops the class with the process anyway.
    let _ = RegisterClassExW(&wc);

    // Centred on the parent, not on the work area: the question belongs to that window.
    let dip = Dip::at(parent);
    let width = dip.px(WIDTH);
    let height = dip.px(HEIGHT);
    let mut prc = RECT::default();
    let _ = GetWindowRect(parent, &mut prc);
    let x = prc.left + ((prc.right - prc.left) - width) / 2;
    let y = prc.top + ((prc.bottom - prc.top) - height) / 2;
    let title = wide(PROMPT.with(|p| p.get().title));

    let hwnd = CreateWindowExW(
        // No thick frame: a prompt has no size to change, and no maximize box either.
        WS_EX_DLGMODALFRAME,
        PCWSTR(class_name.as_ptr()),
        PCWSTR(title.as_ptr()),
        WS_POPUP | WS_CAPTION | WS_SYSMENU,
        x,
        y,
        width,
        height,
        parent,
        None,
        hmodule,
        None,
    );

    match hwnd {
        Ok(h) => {
            // Showing, focus and the first frame happen after WM_CREATE, so `fit_to_text`
            // can widen the window before it is ever visible.
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
            let _ = SetFocus(h);
            repaint(h);
            Some(h)
        }
        Err(e) => {
            log::error!("Failed to create mixer reset prompt: {}", e);
            None
        }
    }
}

struct Btn {
    id: i32,
    text: &'static str,
    rect: RECT,
    primary: bool,
}

/// Prompt geometry in DIPs: a title row, the message block, two buttons bottom-right.
fn layout(hwnd: HWND) -> (RECT, Vec<Btn>) {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    let dip = Dip::at(hwnd);
    let s = |v: f32| dip.px(v);
    let (w, h) = (rc.right, rc.bottom);
    let y = h - s(PAD) - s(BUTTON_H);
    let cancel_x = w - s(PAD) - s(BUTTON_W);
    let confirm_x = cancel_x - s(GAP) - s(BUTTON_W);
    let confirm = PROMPT.with(|p| p.get().confirm);

    let buttons = vec![
        Btn {
            id: ID_BTN_CANCEL,
            text: "Cancel",
            rect: RECT {
                left: cancel_x,
                top: y,
                right: cancel_x + s(BUTTON_W),
                bottom: y + s(BUTTON_H),
            },
            primary: false,
        },
        Btn {
            id: ID_BTN_CONFIRM,
            text: confirm,
            rect: RECT {
                left: confirm_x,
                top: y,
                right: confirm_x + s(BUTTON_W),
                bottom: y + s(BUTTON_H),
            },
            primary: true,
        },
    ];
    (rc, buttons)
}

/// The prompt's own font pair (body text + button labels), built once per DPI. It never
/// reads the settings dialog's fonts: this window can open on another monitor's DPI.
fn fonts(dip: Dip) -> (HFONT, HFONT) {
    FONTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let (font, small) = match cache.get(&dip.dpi) {
            Some(pair) => *pair,
            None => {
                let pair = unsafe {
                    (
                        make_font(dip, 13.0, 400).0 as isize,
                        make_font(dip, 11.0, 400).0 as isize,
                    )
                };
                cache.insert(dip.dpi, pair);
                pair
            }
        };
        (
            HFONT(font as *mut std::ffi::c_void),
            HFONT(small as *mut std::ffi::c_void),
        )
    })
}

fn measure(hdc: HDC, text: &str, font: HFONT) -> i32 {
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return 0;
    }
    unsafe {
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        let mut size = SIZE::default();
        let _ = GetTextExtentPoint32W(hdc, &wide, &mut size);
        SelectObject(hdc, old);
        size.cx
    }
}

/// Widens the window when the longest word of the message would not fit: a message that
/// wraps is fine, a word split mid-way is not.
unsafe fn fit_to_text(hwnd: HWND) {
    let dip = Dip::at(hwnd);
    let (font, _small) = fonts(dip);
    let body = PROMPT.with(|p| p.get().body);
    let hdc = GetDC(hwnd);
    let longest = body
        .split_whitespace()
        .map(|word| measure(hdc, word, font))
        .max()
        .unwrap_or(0);
    let _ = ReleaseDC(hwnd, hdc);

    let want = (longest + dip.px(PAD) * 2).max(dip.px(WIDTH));
    let mut rc = RECT::default();
    let _ = GetWindowRect(hwnd, &mut rc);
    if rc.right - rc.left < want {
        let _ = SetWindowPos(
            hwnd,
            None,
            rc.left,
            rc.top,
            want,
            rc.bottom - rc.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn back_buffer(hwnd: HWND, w: i32, h: i32) -> Option<HDC> {
    BACK.with(|back| {
        let mut back = back.borrow_mut();
        let key = hwnd.0 as isize;
        if let Some((dc, _, bw, bh)) = back.get(&key) {
            if *bw == w && *bh == h {
                return Some(*dc);
            }
        }
        if let Some((dc, bmp, _, _)) = back.remove(&key) {
            // DC first: destroying it deselects the bitmap, which makes deleting it legal.
            unsafe {
                let _ = DeleteDC(dc);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
            }
        }
        let (dc, bmp) = unsafe {
            let window_dc = GetDC(hwnd);
            let dc = CreateCompatibleDC(window_dc);
            let bmp = CreateCompatibleBitmap(window_dc, w, h);
            let _ = ReleaseDC(hwnd, window_dc);
            (dc, bmp)
        };
        if dc.0.is_null() || bmp.0.is_null() {
            unsafe {
                if !dc.0.is_null() {
                    let _ = DeleteDC(dc);
                }
                if !bmp.0.is_null() {
                    let _ = DeleteObject(HGDIOBJ(bmp.0));
                }
            }
            return None;
        }
        unsafe { SelectObject(dc, HGDIOBJ(bmp.0)) };
        back.insert(key, (dc, bmp, w, h));
        Some(dc)
    })
}

fn free_back_buffer(hwnd: HWND) {
    BACK.with(|back| {
        if let Some((dc, bmp, _, _)) = back.borrow_mut().remove(&(hwnd.0 as isize)) {
            unsafe {
                let _ = DeleteDC(dc);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
            }
        }
    });
}

unsafe fn repaint(hwnd: HWND) {
    let _ = InvalidateRect(hwnd, None, true);
    let _ = UpdateWindow(hwnd);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right, rc.bottom);
    if w > 0 && h > 0 {
        match back_buffer(hwnd, w, h) {
            Some(mem) => {
                paint_frame(hwnd, mem, &rc);
                let _ = BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
            }
            None => paint_frame(hwnd, hdc, &rc),
        }
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn paint_frame(hwnd: HWND, hdc: HDC, rc: &RECT) {
    let p = current_palette();
    let dip = Dip::at(hwnd);
    let s = |v: f32| dip.px(v);
    let (title_font, body_font) = fonts(dip);
    let prompt = PROMPT.with(|q| q.get());
    od_controls::fill(hdc, rc, p.window);
    od_controls::text(
        hdc,
        &RECT {
            left: s(PAD),
            top: s(PAD),
            right: rc.right - s(PAD),
            bottom: s(PAD) + s(22.0),
        },
        prompt.title,
        title_font,
        p.text,
        DT_SINGLELINE | DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS,
    );
    od_controls::text(
        hdc,
        &RECT {
            left: s(PAD),
            top: s(PAD) + s(28.0),
            right: rc.right - s(PAD),
            bottom: rc.bottom - s(PAD) - s(BUTTON_H) - s(10.0),
        },
        prompt.body,
        body_font,
        p.secondary,
        DT_WORDBREAK | DT_TOP | DT_LEFT,
    );

    let (_, buttons) = layout(hwnd);
    for btn in &buttons {
        draw_button(hdc, btn, &p, dip, body_font);
    }
}

unsafe fn draw_button(hdc: HDC, btn: &Btn, p: &Palette, dip: Dip, font: HFONT) {
    let s = |v: f32| dip.px(v);
    let rc = btn.rect;
    let hot = HOT.with(|h| h.get()) == btn.id;
    let pressed = PRESSED.with(|q| q.get()) == btn.id;

    od_controls::fill(hdc, &rc, p.window);
    // The two skins the settings dialog already uses: the confirming action is the
    // accent one, the other is the quiet card one.
    let (fill, fg, border) = if btn.primary {
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
        (f, p.text, Some(p.hairline))
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
        btn.text,
        font,
        fg,
        DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_END_ELLIPSIS,
    );
}

fn hit_test(buttons: &[Btn], x: i32, y: i32) -> i32 {
    buttons
        .iter()
        .find(|b| x >= b.rect.left && x < b.rect.right && y >= b.rect.top && y < b.rect.bottom)
        .map(|b| b.id)
        .unwrap_or(0)
}

fn point_of(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xFFFF) as i16 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    )
}

/// Records the answer and closes the prompt; the opener reads it after its pump.
unsafe fn answer(hwnd: HWND, confirmed: bool) {
    ANSWER.with(|a| a.set(confirmed));
    let _ = DestroyWindow(hwnd);
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            apply_chrome(hwnd, current_palette().dark);
            fit_to_text(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1), // fully painted in WM_PAINT
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = point_of(lparam);
            let (_, buttons) = layout(hwnd);
            let hit = hit_test(&buttons, x, y);
            if HOT.with(|h| h.get()) != hit {
                HOT.with(|h| h.set(hit));
                let cursor = if hit == 0 { IDC_ARROW } else { IDC_HAND };
                let _ = SetCursor(LoadCursorW(None, cursor).unwrap_or_default());
                repaint(hwnd);
            }
            // Without the leave request the window never hears the mouse going away,
            // and a button would keep its hover skin while the pointer is elsewhere.
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut tme);
            LRESULT(0)
        }
        msg if msg == CONTROLS_MOUSELEAVE => {
            HOT.with(|h| h.set(0));
            repaint(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point_of(lparam);
            let (_, buttons) = layout(hwnd);
            let hit = hit_test(&buttons, x, y);
            if hit != 0 {
                PRESSED.with(|q| q.set(hit));
                let _ = SetCapture(hwnd);
                repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = point_of(lparam);
            let pressed = PRESSED.with(|q| q.replace(0));
            let _ = ReleaseCapture();
            let (_, buttons) = layout(hwnd);
            if pressed != 0 && hit_test(&buttons, x, y) == pressed {
                answer(hwnd, pressed == ID_BTN_CONFIRM);
            } else {
                repaint(hwnd);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wparam.0 as u16;
            if vk == VK_ESCAPE.0 {
                answer(hwnd, false);
            } else if vk == VK_RETURN.0 || vk == VK_SPACE.0 {
                answer(hwnd, true);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            answer(hwnd, false);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            free_back_buffer(hwnd);
            // Still DefWindowProc: it is what clears the thread's last-active-popup state.
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btn(id: i32, left: i32, top: i32) -> Btn {
        Btn {
            id,
            text: "",
            rect: RECT {
                left,
                top,
                right: left + 100,
                bottom: top + 30,
            },
            primary: id == ID_BTN_CONFIRM,
        }
    }

    fn teardown(hwnd: HWND) {
        unsafe {
            let _ = DestroyWindow(hwnd);
            let name = wide(DIALOG_CLASS);
            let _ = UnregisterClassW(
                PCWSTR(name.as_ptr()),
                GetModuleHandleW(None).unwrap_or_default(),
            );
        }
        BACK.with(|b| b.borrow_mut().clear());
        HOT.with(|h| h.set(0));
        PRESSED.with(|p| p.set(0));
    }

    /// Two targets at one point would send a confirm click to Cancel.
    #[test]
    fn buttons_never_overlap_and_own_their_centres() {
        let buttons = [btn(ID_BTN_CANCEL, 8, 100), btn(ID_BTN_CONFIRM, 116, 100)];
        let a = buttons[0].rect;
        let b = buttons[1].rect;
        let overlap = a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
        assert!(!overlap, "prompt buttons overlap");
        assert_eq!(hit_test(&buttons, 58, 115), ID_BTN_CANCEL);
        assert_eq!(hit_test(&buttons, 166, 115), ID_BTN_CONFIRM);
        assert_eq!(hit_test(&buttons, 0, 0), 0, "bare pixel hit a button");
    }

    /// The prompt has to share the app's type: a font that never resolved would draw the
    /// message in the DC's default face.
    #[test]
    fn fonts_are_real_handles_at_every_dpi() {
        for dpi in [96, 120, 144, 192] {
            let (title, body) = fonts(Dip::from_dpi(dpi));
            assert!(!title.0.is_null(), "title font at {dpi} dpi");
            assert!(!body.0.is_null(), "body font at {dpi} dpi");
        }
    }

    /// The prompt is painted dark whatever the app theme is, and its message has to fit
    /// the client width: a word split by `DT_WORDBREAK` would read as a typo.
    #[test]
    fn prompt_paints_dark_and_fits_its_message() {
        PROMPT.with(|p| {
            p.set(Prompt {
                title: "Reset Mixer",
                body: "Every app's volume and mute state goes back to the Windows defaults.",
                confirm: "Reset",
            })
        });
        let hwnd = unsafe { create_prompt_window(GetDesktopWindow()).expect("prompt window") };
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

            let p = crate::settings_dialog::current_palette();
            let at = |x: i32, y: i32| {
                let c = windows::Win32::Graphics::Gdi::GetPixel(mem, x, y);
                c.0 as u32 & 0xFF_FFFF
            };
            // A pixel under the buttons' row and one in the message block: both are the
            // window colour, never the settings dialog's card or the system grey.
            assert_eq!(at(w / 2, h - 4), p.window, "prompt bottom edge");
            assert_eq!(at(4, h / 2), p.window, "prompt left edge");

            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(mem);
        }
        teardown(hwnd);
    }
}
