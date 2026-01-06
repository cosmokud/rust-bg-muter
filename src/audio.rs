//! Windows Audio Session API (WASAPI) module
//! Provides efficient, low-overhead audio session management.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::sync::Arc;
use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, FALSE, TRUE};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IAudioSessionControl2, IAudioSessionEnumerator, IAudioSessionManager2,
    IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::ProcessStatus::K32GetModuleFileNameExW;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};

/// Represents an audio session with minimal metadata
#[derive(Debug, Clone)]
pub struct AudioSession {
    pub process_id: u32,
    pub process_name: String,
    pub display_name: String,
    pub is_muted: bool,
}

/// Lightweight audio session manager
/// Minimizes COM overhead by caching volume controls
pub struct AudioManager {
    sessions: Arc<Mutex<HashMap<u32, CachedSession>>>,
}

#[allow(dead_code)]
struct CachedSession {
    volume: ISimpleAudioVolume,
    process_name: String,
    display_name: String,
}

unsafe impl Send for CachedSession {}
unsafe impl Sync for CachedSession {}

impl AudioManager {
    /// Creates a new AudioManager instance
    pub fn new() -> windows::core::Result<Self> {
        Ok(Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Refreshes the list of audio sessions
    /// This is the expensive operation - call sparingly
    pub fn refresh_sessions(&self) -> windows::core::Result<Vec<AudioSession>> {
        unsafe {
            // Initialize COM for this thread (idempotent)
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            let device: IMMDevice = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;

            let session_manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;

            let session_enumerator: IAudioSessionEnumerator =
                session_manager.GetSessionEnumerator()?;

            let count = session_enumerator.GetCount()?;
            let mut sessions_lock = self.sessions.lock();
            sessions_lock.clear();

            let mut result = Vec::with_capacity(count as usize);

            for i in 0..count {
                if let Ok(control) = session_enumerator.GetSession(i) {
                    if let Ok(control2) = control.cast::<IAudioSessionControl2>() {
                        if let Ok(pid) = control2.GetProcessId() {
                            // Skip system sounds (PID 0)
                            if pid == 0 {
                                continue;
                            }

                            let process_name = get_process_name_cached(pid);
                            let display_name =
                                get_session_display_name(&control2).unwrap_or_else(|| process_name.clone());

                            if let Ok(volume) = control.cast::<ISimpleAudioVolume>() {
                                let is_muted = volume.GetMute().map(|b| b.as_bool()).unwrap_or(false);

                                let session = AudioSession {
                                    process_id: pid,
                                    process_name: process_name.clone(),
                                    display_name: display_name.clone(),
                                    is_muted,
                                };

                                result.push(session);

                                sessions_lock.insert(
                                    pid,
                                    CachedSession {
                                        volume,
                                        process_name,
                                        display_name,
                                    },
                                );
                            }
                        }
                    }
                }
            }

            Ok(result)
        }
    }

    /// Gets all current audio sessions
    pub fn get_sessions(&self) -> Vec<AudioSession> {
        self.refresh_sessions().unwrap_or_default()
    }

    /// Mutes a specific process by PID (uses cached volume control)
    pub fn mute_process(&self, pid: u32) -> windows::core::Result<()> {
        let sessions = self.sessions.lock();
        if let Some(info) = sessions.get(&pid) {
            unsafe {
                info.volume.SetMute(TRUE, std::ptr::null())?;
            }
        }
        Ok(())
    }

    /// Unmutes a specific process by PID (uses cached volume control)
    pub fn unmute_process(&self, pid: u32) -> windows::core::Result<()> {
        let sessions = self.sessions.lock();
        if let Some(info) = sessions.get(&pid) {
            unsafe {
                info.volume.SetMute(FALSE, std::ptr::null())?;
            }
        }
        Ok(())
    }

    /// Checks if a process is currently muted
    #[allow(dead_code)]
    pub fn is_muted(&self, pid: u32) -> bool {
        let sessions = self.sessions.lock();
        if let Some(info) = sessions.get(&pid) {
            unsafe { info.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false) }
        } else {
            false
        }
    }
}

impl Default for AudioManager {
    fn default() -> Self {
        Self::new().expect("Failed to create AudioManager")
    }
}

/// Gets the process name from a PID with minimal overhead
fn get_process_name_cached(pid: u32) -> String {
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, FALSE, pid) {
            let mut buffer = [0u16; 260];
            let len = K32GetModuleFileNameExW(handle, None, &mut buffer);
            let _ = CloseHandle(handle);
            if len > 0 {
                let path = OsString::from_wide(&buffer[..len as usize]);
                if let Some(path_str) = path.to_str() {
                    if let Some(name) = std::path::Path::new(path_str).file_name() {
                        return name.to_string_lossy().to_string();
                    }
                }
            }
        }
    }
    format!("Process {}", pid)
}

/// Gets the display name of an audio session
fn get_session_display_name(control: &IAudioSessionControl2) -> Option<String> {
    unsafe {
        if let Ok(name_ptr) = control.GetDisplayName() {
            if !name_ptr.is_null() {
                let len = (0..).take_while(|&i| *name_ptr.0.add(i) != 0).count();
                if len > 0 {
                    let slice = std::slice::from_raw_parts(name_ptr.0, len);
                    let name = OsString::from_wide(slice).to_string_lossy().to_string();
                    if !name.is_empty() && !name.starts_with('@') {
                        return Some(name);
                    }
                }
            }
        }
        None
    }
}
