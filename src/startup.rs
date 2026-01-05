//! Windows startup integration (Run at login)
//!
//! Uses HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run
//! so no admin elevation is required.

use std::error::Error;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_SZ,
};

const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "Background Muter";

fn to_wide_null(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn open_hkcu_run_key_set_value() -> Result<HKEY, Box<dyn Error>> {
    let mut key = HKEY::default();
    let subkey = to_wide_null(RUN_SUBKEY);

    unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut key,
        )
        .ok()?;
    }

    Ok(key)
}

/// Enables/disables launching at Windows login for the current user.
pub fn set_run_at_startup(enabled: bool) -> Result<(), Box<dyn Error>> {
    let key = open_hkcu_run_key_set_value()?;
    let value_name = to_wide_null(VALUE_NAME);

    let result = if enabled {
        let exe = std::env::current_exe()?;
        let exe = exe.to_string_lossy();
        // Quote to survive spaces in the path.
        let command = format!("\"{}\"", exe);
        let command_w = to_wide_null(&command);

        // REG_SZ expects bytes including the NUL terminator.
        let data_u8: &[u8] = unsafe {
            std::slice::from_raw_parts(
                command_w.as_ptr().cast::<u8>(),
                command_w.len() * std::mem::size_of::<u16>(),
            )
        };

        unsafe { RegSetValueExW(key, PCWSTR(value_name.as_ptr()), 0, REG_SZ, Some(data_u8)).ok() }
    } else {
        unsafe {
            let code: WIN32_ERROR = RegDeleteValueW(key, PCWSTR(value_name.as_ptr()));
            if code == ERROR_SUCCESS || code == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(windows::core::Error::from_win32())
            }
        }
    };

    unsafe {
        let _ = RegCloseKey(key);
    }

    result.map_err(|e| -> Box<dyn Error> { Box::new(e) })
}

/// Applies the persisted config setting best-effort (logs errors upstream).
pub fn apply_startup_setting(start_with_windows: bool) -> Result<(), Box<dyn Error>> {
    set_run_at_startup(start_with_windows)
}
