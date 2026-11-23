//! Persistent cache of application icons, so a list row can still show the icon of an app
//! that is not running.
//!
//! The live lookup in `settings_dialog` can only read an icon out of the executable of a
//! running process. An exclusion kept for an app that has since been closed therefore drew
//! no icon at all, and the row shifted its text left. Every icon extracted live is written
//! here as well, keyed by executable name - the same key the exclusion and always-muted
//! lists use - and read back for the rows that have no process behind them.
//!
//! One file, `icon-cache.bin` beside `config.json` in the per-user config directory:
//!
//! ```text
//! "RBMI" | version: u32 | count: u32         (little endian)
//! count x { name: u16 length + UTF-8 | png: u32 length + bytes }
//! ```
//!
//! PNG keeps the file to a few hundred bytes per icon and lets the `image` crate - already
//! built in for the tray icon - do the encoding. Only the drawing is kept, never the path:
//! a cached icon must not make the app touch an executable it has not been asked about.

use crate::config::Config;
use image::ImageEncoder;
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use windows::Win32::Foundation::{HANDLE, HWND, TRUE};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HBRUSH, HDC,
    HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DrawIconEx, DI_MASK, DI_NORMAL, HICON, ICONINFO,
};

/// File name inside the config directory.
const CACHE_FILE: &str = "icon-cache.bin";

const MAGIC: &[u8; 4] = b"RBMI";
const VERSION: u32 = 1;

/// Rows draw at 16px (scaled by DPI at paint time); the cached bitmap is the small size
/// `PrivateExtractIconsW` is asked for, so no large icon is ever built.
const ICON_SIZE: i32 = 16;

/// The file's contents, read once on first use. `loaded` keeps a missing or unreadable file
/// from being retried on every row of every repaint.
struct Cache {
    icons: BTreeMap<String, Vec<u8>>,
    loaded: bool,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

/// Decoded icons, one per name, misses included: a repaint must never decode a PNG.
/// Icons are handles, not pointers we dereference, so they are kept as `isize` to stay
/// `Send` - the same convention as the live icon caches in `settings_dialog`.
static HANDLES: OnceLock<Mutex<HashMap<String, Option<isize>>>> = OnceLock::new();

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| {
        Mutex::new(Cache {
            icons: BTreeMap::new(),
            loaded: false,
        })
    })
}

fn handles() -> &'static Mutex<HashMap<String, Option<isize>>> {
    HANDLES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Where the cache lives: one file beside `config.json`, so the installer's "remove the
/// configuration" option takes it with the rest.
pub fn cache_path() -> PathBuf {
    Config::config_dir().join(CACHE_FILE)
}

/// Executable name of `path`, lowercased: the key an exclusion is stored under and the key
/// this cache uses, so a row finds its icon whichever of the two it starts from.
pub fn key_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| path.to_lowercase())
}

/// The icon cached for `name`, decoded once per process. `None` when nothing was ever
/// stored for it, in which case the row simply draws without one.
///
/// # Safety
/// `HICON` is a GDI handle the caller only passes back to `DrawIconEx`; it stays valid for
/// the process lifetime because the handle map is never cleared.
pub unsafe fn icon_for(name: &str) -> Option<HICON> {
    let key = key_of(name);
    if let Some(hit) = handles().lock().get(&key).copied() {
        return hit.map(|h| HICON(h as *mut c_void));
    }
    let icon = png_for(&key).and_then(|png| decode(&png));
    handles().lock().insert(key, icon.map(|h| h.0 as isize));
    icon
}

/// Stored PNG for `name`, if any.
fn png_for(name: &str) -> Option<Vec<u8>> {
    load_once();
    cache().lock().icons.get(name).cloned()
}

/// Stores the live icon of `exe_name` for the runs where that app is closed.
///
/// The first icon seen for a name wins: an app that updates its executable keeps the icon
/// the cache already has until the file is removed, which is cheaper than stat-ing every
/// executable to notice, and the file only changes when the icon genuinely does.
///
/// # Safety
/// `icon` must be the handle the caller just extracted; it is only drawn from here.
pub unsafe fn remember(exe_name: &str, icon: HICON) {
    let key = key_of(exe_name);
    if key.is_empty() {
        return;
    }
    load_once();
    if cache().lock().icons.contains_key(&key) {
        return;
    }
    let Some(png) = encode(icon, ICON_SIZE) else {
        return;
    };

    let icons = {
        let mut cache = cache().lock();
        cache.icons.insert(key, png);
        cache.icons.clone()
    };
    save(&icons);
}

/// Reads the file into the in-memory map on first use. A missing, truncated or unreadable
/// file leaves the cache empty: icons are a convenience, and the next successful extraction
/// rewrites the file.
fn load_once() {
    let mut cache = cache().lock();
    if cache.loaded {
        return;
    }
    cache.loaded = true;

    let path = cache_path();
    let Ok(bytes) = fs::read(&path) else {
        return;
    };
    match parse(&bytes) {
        Some(icons) => cache.icons = icons,
        None => log::warn!("Ignoring unreadable icon cache at {}", path.display()),
    }
}

/// Runs of cache entries, in the file format documented at the top of this module.
fn encode_file(icons: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let entries: Vec<(&String, &Vec<u8>)> = icons
        .iter()
        .filter(|(name, _)| name.len() <= u16::MAX as usize)
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, png) in entries {
        let bytes = name.as_bytes();
        out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(bytes);
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(png);
    }
    out
}

/// Parses the file; `None` for anything that is not a complete cache of this version.
fn parse(bytes: &[u8]) -> Option<BTreeMap<String, Vec<u8>>> {
    if bytes.len() < 12 || &bytes[..4] != MAGIC {
        return None;
    }
    if u32::from_le_bytes(bytes[4..8].try_into().ok()?) != VERSION {
        return None;
    }
    let count = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let mut at = 12;
    let mut icons = BTreeMap::new();
    for _ in 0..count {
        let name_len = u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize;
        at += 2;
        let name = String::from_utf8(bytes.get(at..at + name_len)?.to_vec()).ok()?;
        at += name_len;
        let png_len = u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
        at += 4;
        let png = bytes.get(at..at + png_len)?.to_vec();
        at += png_len;
        icons.insert(name, png);
    }
    Some(icons)
}

/// Writes the cache, staged beside the file and renamed over it, so a failed or interrupted
/// write leaves the previous one whole - the same scheme `Config::save` uses.
fn save(icons: &BTreeMap<String, Vec<u8>>) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let staged = path.with_extension("bin.tmp");
    if let Err(e) = fs::write(&staged, encode_file(icons)) {
        log::warn!("Failed to write icon cache: {}", e);
        return;
    }
    if let Err(e) = fs::rename(&staged, &path) {
        log::warn!("Failed to replace icon cache: {}", e);
    }
}

/// 32bpp top-down DIB section and a pointer to its pixels: `DrawIconEx` draws straight into
/// it and the pixels are read without a `GetDIBits` copy.
unsafe fn dib(dc: HDC, w: i32, h: i32) -> Option<(HBITMAP, *mut u8)> {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // Negative height = top-down, so row 0 is the first byte of the buffer.
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0).ok()?;
    if bmp.0.is_null() || bits.is_null() {
        if !bmp.0.is_null() {
            let _ = DeleteObject(bmp);
        }
        return None;
    }
    Some((bmp, bits as *mut u8))
}

/// PNG bytes for `icon`, drawn at `size` x `size`.
unsafe fn encode(icon: HICON, size: i32) -> Option<Vec<u8>> {
    let screen = GetDC(HWND::default());
    let dc = CreateCompatibleDC(screen);
    let _ = ReleaseDC(HWND::default(), screen);
    if dc.0.is_null() {
        return None;
    }
    let png = encode_in(dc, icon, size);
    let _ = DeleteDC(dc);
    png
}

unsafe fn encode_in(dc: HDC, icon: HICON, size: i32) -> Option<Vec<u8>> {
    let (bmp, bits) = dib(dc, size, size)?;
    let old = SelectObject(dc, HGDIOBJ(bmp.0));
    let png = draw_and_encode(dc, bits, icon, size);
    SelectObject(dc, old);
    let _ = DeleteObject(bmp);
    png
}

unsafe fn draw_and_encode(dc: HDC, bits: *mut u8, icon: HICON, size: i32) -> Option<Vec<u8>> {
    let len = (size * size * 4) as usize;
    // DI_NORMAL: image and mask in one pass; on a clear 32bpp surface the icon's alpha
    // channel comes through with it.
    let _ = DrawIconEx(dc, 0, 0, icon, size, size, 0, HBRUSH::default(), DI_NORMAL);
    let color = std::slice::from_raw_parts(bits, len).to_vec();

    let mut rgba = Vec::with_capacity(len);
    if color.as_chunks::<4>().0.iter().any(|px| px[3] != 0) {
        for px in color.as_chunks::<4>().0 {
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    } else {
        // A legacy icon carries no alpha channel, so the AND mask is the only shape it has.
        // Furthermore a redraw would lose the colours, hence `color` was copied first.
        std::ptr::write_bytes(bits, 0, len);
        let _ = DrawIconEx(dc, 0, 0, icon, size, size, 0, HBRUSH::default(), DI_MASK);
        let mask = std::slice::from_raw_parts(bits, len);
        for (px, m) in color.as_chunks::<4>().0.iter().zip(mask.as_chunks::<4>().0) {
            // White in the mask = the background shows through.
            let alpha = if m[0] > 0x80 { 0 } else { 255 };
            rgba.extend_from_slice(&[px[2], px[1], px[0], alpha]);
        }
    }

    let image = image::RgbaImage::from_raw(size as u32, size as u32, rgba)?;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            image.as_raw(),
            size as u32,
            size as u32,
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(png)
}

/// A fresh `HICON` from stored PNG bytes, for rows whose app is not running.
unsafe fn decode(png: &[u8]) -> Option<HICON> {
    let rgba = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    let (w, h) = (rgba.width() as i32, rgba.height() as i32);
    if w == 0 || h == 0 {
        return None;
    }

    let screen = GetDC(HWND::default());
    let dc = CreateCompatibleDC(screen);
    let _ = ReleaseDC(HWND::default(), screen);
    if dc.0.is_null() {
        return None;
    }

    let mut icon = None;
    if let Some((bmp, bits)) = dib(dc, w, h) {
        let len = (w * h * 4) as usize;
        let dst = std::slice::from_raw_parts_mut(bits, len);
        // PNG hands out RGBA; a DIB takes BGRA.
        for (dst, px) in dst
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(rgba.as_raw().as_chunks::<4>().0)
        {
            dst[0] = px[2];
            dst[1] = px[1];
            dst[2] = px[0];
            dst[3] = px[3];
        }

        // Monochrome mask of zeros - opaque everywhere - so the colour bitmap's alpha
        // channel alone decides what is transparent. Rows are padded to 16 bits.
        let mask_bits = vec![0u8; ((w + 15) / 16 * 2 * h) as usize];
        let mask = CreateBitmap(w, h, 1, 1, Some(mask_bits.as_ptr() as *const c_void));
        if !mask.0.is_null() {
            let info = ICONINFO {
                fIcon: TRUE,
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: mask,
                hbmColor: bmp,
            };
            // The icon copies both bitmaps; the handles below stay ours to delete.
            icon = CreateIconIndirect(&info).ok();
            let _ = DeleteObject(mask);
        }
        let _ = DeleteObject(bmp);
    }
    let _ = DeleteDC(dc);
    icon
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::PrivateExtractIconsW;

    /// The icon of a process that is certainly running: this test binary's own executable.
    fn own_icon() -> HICON {
        let exe = std::env::current_exe().expect("current exe");
        let mut file = [0u16; 260];
        for (slot, unit) in file.iter_mut().zip(exe.to_string_lossy().encode_utf16()) {
            *slot = unit;
        }
        let mut small = [HICON::default(); 1];
        unsafe {
            PrivateExtractIconsW(&file, 0, 16, 16, Some(&mut small), None, 0);
        }
        assert!(!small[0].0.is_null(), "no icon in the test executable");
        small[0]
    }

    /// The key is the executable name in lower case whichever form the row arrives in.
    #[test]
    fn key_is_the_lowercased_executable_name() {
        assert_eq!(
            key_of("C:\\Program Files\\Spotify\\Spotify.exe"),
            "spotify.exe"
        );
        assert_eq!(key_of("SPOTIFY.EXE"), "spotify.exe");
        assert_eq!(key_of("spotify.exe"), "spotify.exe");
    }

    /// The format round-trips, and anything that is not a whole cache of this version is
    /// rejected rather than half-read.
    #[test]
    fn file_format_round_trips_and_rejects_damage() {
        let mut icons = BTreeMap::new();
        icons.insert("spotify.exe".to_string(), vec![1u8, 2, 3, 4]);
        icons.insert("discord.exe".to_string(), vec![9u8; 300]);

        let bytes = encode_file(&icons);
        assert_eq!(parse(&bytes), Some(icons));

        for cut in 0..bytes.len() {
            assert!(
                parse(&bytes[..cut]).is_none(),
                "a file cut to {cut} bytes parsed as a cache"
            );
        }

        let mut next_version = bytes.clone();
        next_version[4..8].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(parse(&next_version).is_none(), "a newer version was read");
    }

    /// A live icon survives the trip through PNG: encoded, then decoded back into a handle
    /// that can be drawn.
    #[test]
    fn a_live_icon_round_trips_through_png() {
        let png = unsafe { encode(own_icon(), ICON_SIZE) }.expect("icon did not encode");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
        let icon = unsafe { decode(&png) }.expect("icon did not decode");
        assert!(!icon.0.is_null(), "decoded to a null handle");
    }

    /// The reason the file exists: a name with no process behind it still answers with the
    /// icon that was stored while it ran.
    #[test]
    fn a_remembered_icon_comes_back_without_the_process() {
        // The cache lives beside the config, whose directory another test can point
        // elsewhere while this one is resolving it.
        let _config_dir = crate::config::config_dir_lock();
        let name = "remember-test.exe";
        unsafe {
            remember(name, own_icon());
        }
        assert!(cache_path().exists(), "remember wrote no cache file");

        // Forget the decoded handle, so the answer below has to come from the file.
        handles().lock().remove(name);
        assert!(
            unsafe { icon_for(name) }.is_some(),
            "a stored icon did not come back"
        );
        assert!(
            unsafe { icon_for("never-seen-icon.exe") }.is_none(),
            "an unknown name produced an icon"
        );
    }
}
