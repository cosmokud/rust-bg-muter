//! Windows Audio Session API (WASAPI) module
//! Provides low-level audio session management for muting background applications.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::sync::Arc;
use windows::core::Interface;
use windows::Win32::Foundation::{FALSE, TRUE, CloseHandle};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, AudioSessionState,
    AudioSessionStateActive, AudioSessionStateExpired, AudioSessionStateInactive,
    IAudioSessionControl2, IAudioSessionEnumerator,
    IAudioSessionManager2, IMMDevice,
    IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::ProcessStatus::K32GetModuleFileNameExW;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};

/// Represents an audio session with all relevant metadata
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AudioSession {
    pub session_id: String,
    pub process_id: u32,
    pub process_name: String,
    pub display_name: String,
    pub is_muted: bool,
    pub volume: f32,
    pub state: SessionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Active,
    Inactive,
    Expired,
}

impl From<AudioSessionState> for SessionState {
    fn from(state: AudioSessionState) -> Self {
        if state == AudioSessionStateActive {
            SessionState::Active
        } else if state == AudioSessionStateInactive {
            SessionState::Inactive
        } else if state == AudioSessionStateExpired {
            SessionState::Expired
        } else {
            SessionState::Inactive
        }
    }
}

/// Thread-safe audio session manager
#[allow(dead_code)]
pub struct AudioManager {
    sessions: Arc<Mutex<HashMap<u32, AudioSessionInfo>>>,
    notification_callback: Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>,
}

#[allow(dead_code)]
struct AudioSessionInfo {
    #[allow(dead_code)]
    control: IAudioSessionControl2,
    volume: ISimpleAudioVolume,
    process_name: String,
    #[allow(dead_code)]
    display_name: String,
}

unsafe impl Send for AudioSessionInfo {}
unsafe impl Sync for AudioSessionInfo {}

#[allow(dead_code)]
impl AudioManager {
    /// Creates a new AudioManager instance
    pub fn new() -> windows::core::Result<Self> {
        Ok(Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            notification_callback: Arc::new(Mutex::new(None)),
        })
    }

    /// Sets a callback for when sessions change
    pub fn set_notification_callback<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut cb = self.notification_callback.lock();
        *cb = Some(Box::new(callback));
    }

    /// Refreshes the list of audio sessions
    pub fn refresh_sessions(&self) -> windows::core::Result<Vec<AudioSession>> {
        unsafe {
            // Initialize COM for this thread
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

            let mut result = Vec::new();

            for i in 0..count {
                if let Ok(control) = session_enumerator.GetSession(i) {
                    if let Ok(control2) = control.cast::<IAudioSessionControl2>() {
                        if let Ok(pid) = control2.GetProcessId() {
                            // Skip system sounds (PID 0)
                            if pid == 0 {
                                continue;
                            }

                            let process_name = get_process_name(pid);
                            let display_name = get_session_display_name(&control2)
                                .unwrap_or_else(|| process_name.clone());

                            if let Ok(volume) = control.cast::<ISimpleAudioVolume>() {
                                let is_muted = volume.GetMute().map(|b| b.as_bool()).unwrap_or(false);
                                let vol_level = volume.GetMasterVolume().unwrap_or(1.0);
                                let state: SessionState = control2
                                    .GetState()
                                    .unwrap_or(AudioSessionStateInactive)
                                    .into();

                                let session = AudioSession {
                                    session_id: format!("{}_{}", pid, i),
                                    process_id: pid,
                                    process_name: process_name.clone(),
                                    display_name: display_name.clone(),
                                    is_muted,
                                    volume: vol_level,
                                    state,
                                };

                                result.push(session);

                                sessions_lock.insert(
                                    pid,
                                    AudioSessionInfo {
                                        control: control2,
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

    /// Mutes a specific process by PID
    pub fn mute_process(&self, pid: u32) -> windows::core::Result<()> {
        let sessions = self.sessions.lock();
        if let Some(info) = sessions.get(&pid) {
            unsafe {
                info.volume.SetMute(TRUE, std::ptr::null())?;
            }
        }
        Ok(())
    }

    /// Unmutes a specific process by PID
    pub fn unmute_process(&self, pid: u32) -> windows::core::Result<()> {
        let sessions = self.sessions.lock();
        if let Some(info) = sessions.get(&pid) {
            unsafe {
                info.volume.SetMute(FALSE, std::ptr::null())?;
            }
        }
        Ok(())
    }

    /// Sets the mute state for a process
    pub fn set_mute(&self, pid: u32, mute: bool) -> windows::core::Result<()> {
        if mute {
            self.mute_process(pid)
        } else {
            self.unmute_process(pid)
        }
    }

    /// Checks if a process is currently muted
    pub fn is_muted(&self, pid: u32) -> bool {
        let sessions = self.sessions.lock();
        if let Some(info) = sessions.get(&pid) {
            unsafe { info.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false) }
        } else {
            false
        }
    }

    /// Gets the process name for a PID
    pub fn get_process_name(&self, pid: u32) -> Option<String> {
        let sessions = self.sessions.lock();
        sessions.get(&pid).map(|info| info.process_name.clone())
    }
}

impl Default for AudioManager {
    fn default() -> Self {
        Self::new().expect("Failed to create AudioManager")
    }
}

/// Gets the process name from a PID
fn get_process_name(pid: u32) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_manager_creation() {
        // This test requires Windows
        #[cfg(target_os = "windows")]
        {
            let manager = AudioManager::new();
            assert!(manager.is_ok());
        }
    }
}
