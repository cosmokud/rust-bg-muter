//! The stored half of "Reset sound devices and volumes for all apps".
//!
//! Windows remembers each app's per-device volume, mute and routing under
//! `HKCU\Software\Microsoft\Internet Explorer\LowRegistry\Audio\PolicyConfig`, one
//! subkey per remembered session. Deleting that subtree is exactly what the Volume
//! Mixer's reset does to this user's preferences, and it needs no elevation: the next
//! time an app opens a stream it gets a fresh session, unmuted at full volume on the
//! current default device.
//!
//! Deliberately untouched: `HKLM\...\MMDevices\Audio\Render`, where the device-level
//! defaults live. It is SYSTEM-owned, so changing it can only be honoured behind an
//! elevation prompt, and this reset is meant to be one click.

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyW, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, HKEY,
    HKEY_CURRENT_USER, KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE, KEY_SET_VALUE,
};

const POLICY_CONFIG: &str = r"Software\Microsoft\Internet Explorer\LowRegistry\Audio\PolicyConfig";

/// The registry subtrees whose children are the remembered sessions.
const PREFERENCE_STORES: [&str; 2] = ["PropertyStore", "Grouping"];

/// What a reset did, for the confirmation text and for the log.
#[derive(Debug, Default, Clone)]
pub struct Report {
    /// Remembered session keys removed across `PREFERENCE_STORES`.
    pub keys_removed: usize,
    /// Subkeys that would not go; non-empty means the reset is incomplete.
    pub failures: Vec<String>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Removes the remembered per-app preferences of this user. The live sessions are
/// reset separately: the caller owns that, so one click covers both halves.
pub fn clear_stored_preferences() -> Report {
    let mut report = Report::default();
    for sub in PREFERENCE_STORES {
        let path = format!("{POLICY_CONFIG}\\{sub}");
        let path_w = wide(&path);
        let mut key = HKEY::default();
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path_w.as_ptr()),
                0,
                KEY_QUERY_VALUE | KEY_ENUMERATE_SUB_KEYS,
                &mut key,
            )
        };
        match opened {
            ERROR_SUCCESS => {
                let names = subkey_names(key, &mut report);
                let _ = unsafe { RegCloseKey(key) };
                for name in names {
                    let child = format!("{path}\\{name}");
                    report.keys_removed += remove_key(&child, &mut report);
                }
            }
            // No store for this user yet (fresh profile, or an earlier reset): every
            // preference is already at its default, so there is nothing to remove.
            ERROR_FILE_NOT_FOUND => {}
            other => report
                .failures
                .push(format!("{path}: open failed ({})", other.0)),
        }
    }
    log::info!(
        "Mixer reset: {} stored preference key(s) removed, {} failure(s)",
        report.keys_removed,
        report.failures.len()
    );
    report
}

/// Subkey names of `key`, as the registry lists them.
fn subkey_names(key: HKEY, report: &mut Report) -> Vec<String> {
    let mut names = Vec::new();
    let mut index = 0u32;
    loop {
        let mut buffer = [0u16; 512];
        let mut len = buffer.len() as u32;
        let rc = unsafe {
            RegEnumKeyExW(
                key,
                index,
                PWSTR(buffer.as_mut_ptr()),
                &mut len,
                None,
                PWSTR::null(),
                None,
                None,
            )
        };
        if rc != ERROR_SUCCESS {
            if rc != ERROR_NO_MORE_ITEMS {
                report
                    .failures
                    .push(format!("enumeration stopped ({})", rc.0));
            }
            break;
        }
        names.push(String::from_utf16_lossy(&buffer[..len as usize]));
        index += 1;
    }
    names
}

/// Deletes one remembered session: its values, then its children, then the key itself
/// once it is empty. Returns 1 when the key is gone - a key that was never there is 0,
/// because the only thing that matters is whether a preference survived the reset.
fn remove_key(path: &str, report: &mut Report) -> usize {
    let path_w = wide(path);
    let mut key = HKEY::default();
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path_w.as_ptr()),
            0,
            KEY_QUERY_VALUE | KEY_SET_VALUE | KEY_ENUMERATE_SUB_KEYS,
            &mut key,
        )
    } != ERROR_SUCCESS
    {
        return 0;
    }

    let mut removed = 0;
    let children = subkey_names(key, report);
    let values_clear = delete_values(key, path, report);
    let _ = unsafe { RegCloseKey(key) };

    // Children go after this key's own values: `RegDeleteKeyW` refuses a key that still
    // has subkeys, so every child has to be gone first.
    for name in children {
        removed += remove_key(&format!("{path}\\{name}"), report);
    }

    let cut = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(path_w.as_ptr())) };
    if cut == ERROR_SUCCESS {
        removed += 1;
    } else if values_clear {
        report
            .failures
            .push(format!("{path}: key delete failed ({})", cut.0));
    }
    removed
}

/// Deletes every value of `key`; false when one of them survived.
fn delete_values(key: HKEY, path: &str, report: &mut Report) -> bool {
    loop {
        let mut name = [0u16; 512];
        let mut len = name.len() as u32;
        let rc = unsafe {
            RegEnumValueW(
                key,
                0,
                PWSTR(name.as_mut_ptr()),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if rc != ERROR_SUCCESS {
            return rc == ERROR_NO_MORE_ITEMS;
        }
        let rc = unsafe { RegDeleteValueW(key, PCWSTR(name.as_ptr())) };
        if rc != ERROR_SUCCESS {
            report
                .failures
                .push(format!("{path}: value delete failed ({})", rc.0));
            return false;
        }
    }
}
