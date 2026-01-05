//! Process detection and foreground/background tracking module
//! Handles detection of which application is in the foreground

#![allow(dead_code)]

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use windows::Win32::Foundation::{FALSE, CloseHandle};
use windows::Win32::System::ProcessStatus::K32GetModuleFileNameExW;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId,
};

/// Information about a process
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub exe_path: Option<String>,
}

impl ProcessInfo {
    /// Creates a new ProcessInfo from a PID
    pub fn from_pid(pid: u32) -> Option<Self> {
        let (name, exe_path) = get_process_info(pid)?;
        Some(Self {
            pid,
            name,
            exe_path: Some(exe_path),
        })
    }

    /// Gets just the executable name (e.g., "chrome.exe")
    pub fn exe_name(&self) -> &str {
        &self.name
    }

    /// Normalizes the name for comparison (lowercase, no extension)
    pub fn normalized_name(&self) -> String {
        self.name
            .to_lowercase()
            .trim_end_matches(".exe")
            .to_string()
    }
}

/// Gets the PID of the foreground window's process
pub fn get_foreground_pid() -> Option<u32> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == std::ptr::null_mut() {
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

/// Gets the foreground process info
pub fn get_foreground_process() -> Option<ProcessInfo> {
    let pid = get_foreground_pid()?;
    ProcessInfo::from_pid(pid)
}

/// Gets process name and path from PID
fn get_process_info(pid: u32) -> Option<(String, String)> {
    unsafe {
        let handle = OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
            FALSE,
            pid,
        ).ok()?;
        
        let mut buffer = [0u16; 260];
        let len = K32GetModuleFileNameExW(handle, None, &mut buffer);
        let _ = CloseHandle(handle);
        
        if len > 0 {
            let path = OsString::from_wide(&buffer[..len as usize]);
            let path_str = path.to_string_lossy().to_string();
            
            let name = std::path::Path::new(&path_str)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("Process {}", pid));
            
            Some((name, path_str))
        } else {
            None
        }
    }
}

/// Checks if a process is the foreground application
pub fn is_foreground_process(pid: u32) -> bool {
    get_foreground_pid() == Some(pid)
}

/// Tracker for foreground window changes
pub struct ForegroundTracker {
    last_foreground_pid: Option<u32>,
}

impl ForegroundTracker {
    pub fn new() -> Self {
        Self {
            last_foreground_pid: None,
        }
    }

    /// Checks if foreground has changed, returns the new foreground PID if changed
    pub fn check_foreground_change(&mut self) -> Option<u32> {
        let current = get_foreground_pid();
        
        if current != self.last_foreground_pid {
            self.last_foreground_pid = current;
            current
        } else {
            None
        }
    }

    /// Gets the current foreground PID
    pub fn current_foreground(&self) -> Option<u32> {
        get_foreground_pid()
    }

    /// Gets the last known foreground PID
    pub fn last_foreground(&self) -> Option<u32> {
        self.last_foreground_pid
    }
}

impl Default for ForegroundTracker {
    fn default() -> Self {
        Self::new()
    }
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

    #[test]
    fn test_foreground_tracker() {
        let mut tracker = ForegroundTracker::new();
        let _ = tracker.check_foreground_change();
        // Second call should return None if foreground hasn't changed
    }
}
