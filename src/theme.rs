//! System theme (dark/light + accent), DPI scaling and DWM window chrome.
//!
//! Colors are COLORREF (0x00BBGGRR). DWM stores AccentColor as 0xAABBGGRR, so its
//! low 3 bytes are already COLORREF - no channel swap needed.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetSystemMetrics, LoadImageW, SendMessageW, HICON, ICON_BIG, ICON_SMALL,
    ICON_SMALL2, IMAGE_FLAGS, IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON, SM_CYICON,
    SM_CYSMICON, WM_SETICON,
};

const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const DWM: &str = r"Software\Microsoft\Windows\DWM";

/// Fallback accent (Win11 default #0078D4) used when the DWM value is missing.
const DEFAULT_ACCENT: u32 = 0x00D47800;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub window: u32,
    pub card: u32,
    pub hairline: u32,
    /// Inset field/list background.
    pub well: u32,
    pub text: u32,
    pub secondary: u32,
    pub accent: u32,
    /// Text drawn on top of `accent`.
    pub on_accent: u32,
    pub hover: u32,
    pub pressed: u32,
}

impl Palette {
    pub fn system() -> Self {
        let dark = read_dword(PERSONALIZE, "AppsUseLightTheme").unwrap_or(1) == 0;
        let accent = read_dword(DWM, "AccentColor")
            .map(|v| v & 0x00FF_FFFF)
            .unwrap_or(DEFAULT_ACCENT);
        let text_on_accent = if luminance(accent) > 150 {
            0x0000_0000
        } else {
            0x00FF_FFFF
        };

        if dark {
            Self {
                dark,
                window: 0x0020_2020,
                card: 0x002B_2B2B,
                hairline: 0x003A_3A3A,
                well: 0x001A_1A1A,
                text: 0x00E8_E8E8,
                secondary: 0x00A0_A0A0,
                accent,
                on_accent: text_on_accent,
                hover: 0x0036_3636,
                pressed: 0x0022_2222,
            }
        } else {
            Self {
                dark,
                window: 0x00F3_F3F3,
                card: 0x00FF_FFFF,
                hairline: 0x00E5_E5E5,
                well: 0x00FA_FAFA,
                text: 0x001A_1A1A,
                secondary: 0x0076_7676,
                accent,
                on_accent: text_on_accent,
                hover: 0x00F0_F0F0,
                pressed: 0x00E4_E4E4,
            }
        }
    }
}

fn luminance(c: u32) -> u32 {
    let r = c & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = (c >> 16) & 0xFF;
    (299 * r + 587 * g + 114 * b) / 1000
}

fn wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn read_dword(key: &str, value: &str) -> Option<u32> {
    unsafe {
        let k = wide(key);
        let v = wide(value);
        let mut data: u32 = 0;
        let mut size: u32 = std::mem::size_of::<u32>() as u32;
        let rc = RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut std::ffi::c_void),
            Some(&mut size),
        );
        (rc.0 == 0).then_some(data)
    }
}

/// Per-monitor DPI v2 scale factor; every layout number is expressed in DIPs.
#[derive(Clone, Copy, Debug)]
pub struct Dip {
    pub dpi: u32,
}

impl Dip {
    pub fn at(hwnd: HWND) -> Self {
        Self {
            dpi: unsafe { GetDpiForWindow(hwnd) }.max(96),
        }
    }

    pub fn from_dpi(dpi: u32) -> Self {
        Self { dpi: dpi.max(96) }
    }

    pub fn px(self, dips: f32) -> i32 {
        (dips * self.dpi as f32 / 96.0).round() as i32
    }
}

/// Resource id of the icon the build script embeds into the executable.
const APP_ICON_ID: u32 = 1;

/// Gives a window the application icon at every size the shell asks for.
///
/// `LoadImageW` with a resource id never reads a resource below the size it was asked
/// for, so each icon is loaded from the closest size the ICO carries (32/48/64 and not
/// the same 32 for every request). The ICO itself is embedded by `build.rs`.
///
/// Windows takes the small icon for the caption, the notification area and the
/// Alt+Tab / taskbar hover previews, and the big one for Alt+Tab, so a window created
/// without this pair has no icon at all in those places: the app has no top-level
/// window of its own, the tray hides its icon, and the settings dialog is created later
/// and never sets one.
pub fn apply_app_icon(hwnd: HWND) {
    unsafe {
        // HINSTANCE null: the image is searched for in the executable's resources.
        let hinst = HINSTANCE::default();
        // ICON_BIG, then ICON_SMALL and ICON_SMALL2: the second small slot is the one
        // the system reads when the first was never filled.
        for (slot, cx, cy) in [
            (ICON_BIG, SM_CXICON, SM_CYICON),
            (ICON_SMALL, SM_CXSMICON, SM_CYSMICON),
            (ICON_SMALL2, SM_CXSMICON, SM_CYSMICON),
        ] {
            let size_x = GetSystemMetrics(cx).max(16);
            let size_y = GetSystemMetrics(cy).max(16);
            let Ok(handle) = LoadImageW(
                hinst,
                PCWSTR(APP_ICON_ID as usize as *const u16),
                IMAGE_ICON,
                size_x,
                size_y,
                IMAGE_FLAGS(LR_DEFAULTCOLOR.0),
            ) else {
                continue;
            };
            // The window owns the handle now; DestroyIcon drops our reference only, so
            // the icon stays valid for the window and dies with it.
            SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(slot as usize),
                LPARAM(handle.0 as isize),
            );
            let _ = DestroyIcon(HICON(handle.0));
        }
    }
}

/// Dark title bar + rounded corners; silently ignored where unsupported (Win10).
pub fn apply_chrome(hwnd: HWND, dark: bool) {
    unsafe {
        let flag: i32 = dark as i32;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &flag as *const i32 as *const std::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&corner) as u32,
        );
    }

    apply_app_icon(hwnd);
}

#[cfg(test)]
mod icon_probe {
    #[test]
    fn probe() {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::HINSTANCE;
        use windows::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            for (id, cx, cy) in [(1u32, 32, 32), (1u32, 16, 16), (32512, 32, 32)] {
                let r = LoadImageW(
                    HINSTANCE::default(),
                    PCWSTR(id as usize as *const u16),
                    IMAGE_ICON,
                    cx,
                    cy,
                    LR_DEFAULTCOLOR,
                );
                println!("id {id} {cx}x{cy} -> {:?}", r.map(|h| h.0 as isize));
            }
        }
    }
}
