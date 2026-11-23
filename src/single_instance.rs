//! Single instance guard plus the way back into an already running instance.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, HWND, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::{AttachThreadInput, CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, SetForegroundWindow,
    ShowWindow, SW_RESTORE,
};

use crate::od_controls::to_wide;
use crate::tray::APP_NAME;

/// Owned for the process lifetime: the handle *is* the lock and Windows drops it on exit.
static INSTANCE: OnceLock<isize> = OnceLock::new();

/// A second copy of the app must not spawn a twin tray icon or a second muter engine.
/// Returns false when another instance already holds the lock.
pub fn acquire() -> bool {
    acquire_named(&format!("Local\\{APP_NAME}"))
}

fn acquire_named(name: &str) -> bool {
    let name = to_wide(name);
    let Ok(handle) = (unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) }) else {
        // No mutex, no guard: starting is better than refusing to run.
        return true;
    };
    // Read before anything else runs: any successful call in between resets the error, so
    // logging first made a second instance look like the first.
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

    if already_running {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return false;
    }

    let _ = INSTANCE.set(handle.0 as isize);
    true
}

/// Raises and focuses a window this process owns.
pub fn bring_forward(hwnd: HWND) {
    unsafe {
        // A minimized window is not brought forward by SetForegroundWindow alone.
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd).as_bool() {
            return;
        }

        // Foreground lock: a process that does not own the foreground window is refused.
        // Sharing the input state of the thread that does own it lifts the restriction.
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == hwnd {
            return;
        }
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let this_thread = GetCurrentThreadId();
        if fg_thread == 0 || fg_thread == this_thread {
            return;
        }
        if AttachThreadInput(this_thread, fg_thread, true).as_bool() {
            let _ = SetForegroundWindow(hwnd);
            let _ = AttachThreadInput(this_thread, fg_thread, false);
        }
    }
}

/// Brings the running instance's window forward, or reports that it has none yet.
///
/// The holder may still be starting up, so this waits briefly for the window to appear -
/// a double-launch from Explorer or a shortcut often lands before the first paint.
/// Returns false when no window showed up within the wait.
pub fn focus_existing(title: &str) -> bool {
    let title = to_wide(title);
    let deadline = Instant::now() + Duration::from_secs(3);

    loop {
        if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) } {
            if !hwnd.is_invalid() {
                bring_forward(hwnd);
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_acquire_is_refused() {
        // A test-only name: the real one is taken whenever the app itself is running.
        let name = format!("Local\\{APP_NAME} test {}", std::process::id());
        assert!(acquire_named(&name));
        assert!(!acquire_named(&name));
    }
}
