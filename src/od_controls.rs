//! GDI paint helpers for the owner-drawn settings dialog.
//!
//! The dialog owns no child windows: it draws every control itself, so what is left
//! here are the primitives (fill/round/stroke/dot/text) plus the flat/rounded shapes
//! and `mix` colour blending they share. All of it is pure GDI (CPU rasterized): the
//! shapes select the DC brush and pen, so a paint pass allocates no GDI object.

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    DrawTextW, Ellipse, FillRect, GetStockObject, RoundRect, SelectObject, SetBkMode,
    SetDCBrushColor, SetDCPenColor, SetTextColor, DC_BRUSH, DC_PEN, DRAW_TEXT_FORMAT, HBRUSH, HDC,
    HFONT, HGDIOBJ, NULL_BRUSH, TRANSPARENT,
};

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

/// Blends `base` toward `toward` by `amount` (0..=1), channel-wise on a COLORREF.
pub fn mix(base: u32, toward: u32, amount: f32) -> u32 {
    let ch = |shift: u32| {
        let a = ((base >> shift) & 0xFF) as f32;
        let b = ((toward >> shift) & 0xFF) as f32;
        ((a + (b - a) * amount).round() as u32) & 0xFF
    };
    ch(0) | (ch(8) << 8) | (ch(16) << 16)
}
