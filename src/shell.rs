//! Shell helpers shared by the tray loop and the settings dialog.

use crate::config::Config;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Opens a path with the user's default handler (editor for `.json`, Explorer for folders).
pub fn open_path(path: &Path) -> std::io::Result<()> {
    let operation: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let target: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };

    if (result.0 as isize) <= 32 {
        Err(std::io::Error::other(format!(
            "ShellExecuteW failed with code {}",
            result.0 as isize
        )))
    } else {
        Ok(())
    }
}

/// Opens `config.json` in the default editor, creating it first when missing.
pub fn open_config_in_default_editor() {
    let config_path = Config::config_path();

    if !config_path.exists() {
        if let Err(err) = Config::default().save() {
            log::error!("Failed to create config file: {}", err);
            return;
        }
    }

    if let Err(err) = open_path(&config_path) {
        log::error!("Failed to open config file {:?}: {}", config_path, err);
    }
}
