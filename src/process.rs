//! Process lookups: which window has focus, and what a pid or an executable name is.
//!
//! One ladder per question. Two ladders per question existed before - the audio path
//! resolved names its own way, the settings dialog resolved paths and names-to-pids its own
//! way - each with a different set of access rights, so a protected process was handled
//! three different ways and every fix had to land three times.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, FALSE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::K32GetModuleFileNameExW;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// Gets the PID of the foreground window's process
pub fn get_foreground_pid() -> Option<u32> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        if pid == 0 {
            None
        } else {
            Some(pid)
        }
    }
}

/// Full image path of a running process.
///
/// `PROCESS_QUERY_LIMITED_INFORMATION` is the one right that also covers a protected or
/// elevated target, so a dead or unreadable pid costs exactly one `OpenProcess`.
pub fn path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid).ok()?;
        let mut buf = [0u16; 260];
        let mut size = buf.len() as u32;
        let read = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
        .is_ok();
        let _ = CloseHandle(process);
        (read && size > 0).then(|| String::from_utf16_lossy(&buf[..size as usize]))
    }
}

/// Executable file name of a running process, e.g. `Spotify.exe`.
///
/// The module name is the cheap answer, but it needs `QUERY_INFORMATION|VM_READ`, which
/// system processes such as Explorer refuse; the image name is asked for next.
pub fn name(pid: u32) -> Option<String> {
    module_name(pid).or_else(|| path(pid).and_then(|path| file_name(&path)))
}

/// First running process whose executable is `name`, case-insensitively. One toolhelp
/// snapshot per call: this is the lookup for an app that is named in a rule but may not be
/// running.
pub fn pid_of(name: &str) -> Option<u32> {
    let wanted = name.to_lowercase();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = None;
        let mut next = Process32FirstW(snapshot, &mut entry).is_ok();
        while next {
            let exe = String::from_utf16_lossy(&entry.szExeFile);
            if exe.trim_end_matches('\0').to_lowercase() == wanted {
                found = Some(entry.th32ProcessID);
                break;
            }
            next = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

/// The process's own module path, the first rung of `name`.
fn module_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, FALSE, pid).ok()?;
        let mut buffer = [0u16; 260];
        let len = K32GetModuleFileNameExW(handle, None, &mut buffer);
        let _ = CloseHandle(handle);
        if len == 0 {
            return None;
        }
        let full = OsString::from_wide(&buffer[..len as usize]);
        full.to_str().and_then(file_name)
    }
}

fn file_name(full_path: &str) -> Option<String> {
    std::path::Path::new(full_path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_foreground_pid() {
        // Should return Some on a Windows system with a foreground window
        let pid = get_foreground_pid();
        // May be None in headless environments
        println!("Foreground PID: {:?}", pid);
    }

    /// The ladders answer for a process that is certainly running: this one.
    #[test]
    fn a_running_process_has_a_name_a_path_and_a_pid() {
        let pid = std::process::id();
        let name = name(pid).expect("no name for our own process");
        assert!(name.to_lowercase().ends_with(".exe"), "{name}");
        assert!(path(pid).is_some(), "no path for our own process");
        assert_eq!(
            pid_of(&name).map(|found| found > 0),
            Some(true),
            "our own executable was not found by name"
        );
        assert!(pid_of("no-such-executable-anywhere.exe").is_none());
    }
}
