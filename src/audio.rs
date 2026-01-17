//! Windows Audio Session API (WASAPI) module
//! Provides efficient, low-overhead audio session management.

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::sync::Arc;
use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, FALSE, TRUE};
use windows::Win32::Media::Audio::{
    eCommunications, eConsole, eMultimedia, eRender, IAudioSessionControl2,
    IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::ProcessStatus::K32GetModuleFileNameExW;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows::core::PWSTR;

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

            // Enumerate sessions from all default render roles for better coverage
            let mut devices: Vec<IMMDevice> = Vec::new();
            for role in [eConsole, eMultimedia, eCommunications] {
                if let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, role) {
                    devices.push(device);
                }
            }

            let mut result = Vec::new();
            let mut seen_pids = HashSet::new();
            let mut new_sessions: HashMap<u32, CachedSession> = HashMap::new();

            for device in devices {
                if let Err(e) = collect_sessions_for_device(
                    &device,
                    &mut new_sessions,
                    &mut seen_pids,
                    &mut result,
                ) {
                    log::warn!("Failed to enumerate audio sessions for a device: {}", e);
                }
            }

            let mut sessions_lock = self.sessions.lock();
            *sessions_lock = new_sessions;

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

/// Collects sessions for a specific audio device
fn collect_sessions_for_device(
    device: &IMMDevice,
    sessions: &mut HashMap<u32, CachedSession>,
    seen_pids: &mut HashSet<u32>,
    result: &mut Vec<AudioSession>,
) -> windows::core::Result<()> {
    unsafe {
        let session_manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
        let session_enumerator = session_manager.GetSessionEnumerator()?;
        let count = session_enumerator.GetCount()?;

        for i in 0..count {
            if let Ok(control) = session_enumerator.GetSession(i) {
                if let Ok(control2) = control.cast::<IAudioSessionControl2>() {
                    if let Ok(pid) = control2.GetProcessId() {
                        let mut process_name = if pid == 0 {
                            "System Sounds".to_string()
                        } else {
                            get_process_name_cached(pid)
                        };

                        let display_name =
                            get_session_display_name(&control2).unwrap_or_else(|| process_name.clone());

                        if pid != 0
                            && process_name == "System Sounds"
                            && display_name.to_lowercase() != "system sounds"
                        {
                            process_name = display_name.clone();
                        }

                        if let Ok(volume) = control.cast::<ISimpleAudioVolume>() {
                            let is_muted = volume.GetMute().map(|b| b.as_bool()).unwrap_or(false);

                            if seen_pids.insert(pid) {
                                result.push(AudioSession {
                                    process_id: pid,
                                    process_name: process_name.clone(),
                                    display_name: display_name.clone(),
                                    is_muted,
                                });
                            }

                            if !sessions.contains_key(&pid) {
                                sessions.insert(
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
        }
    }

    Ok(())
}

/// Gets the process name from a PID with minimal overhead
/// Uses multiple fallback methods to handle system processes like Explorer.exe
fn get_process_name_cached(pid: u32) -> String {
    unsafe {
        // First try: PROCESS_QUERY_INFORMATION | PROCESS_VM_READ with K32GetModuleFileNameExW
        if let Ok(handle) = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, FALSE, pid) {
            let mut buffer = [0u16; 260];
            let len = K32GetModuleFileNameExW(handle, None, &mut buffer);
            let _ = CloseHandle(handle);
            if len > 0 {
                let path = OsString::from_wide(&buffer[..len as usize]);
                if let Some(path_str) = path.to_str() {
                    if let Some(name) = std::path::Path::new(path_str).file_name() {
                        return normalize_system_process_name(name.to_string_lossy().to_string());
                    }
                }
            }
        }

        // Second try: PROCESS_QUERY_LIMITED_INFORMATION with QueryFullProcessImageNameW
        // This works for system processes like Explorer.exe where the first method fails
        if let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) {
            let mut buffer = [0u16; 260];
            let mut size = buffer.len() as u32;
            if QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut size,
            )
            .is_ok()
            {
                let _ = CloseHandle(handle);
                if size > 0 {
                    let path = OsString::from_wide(&buffer[..size as usize]);
                    if let Some(path_str) = path.to_str() {
                        if let Some(name) = std::path::Path::new(path_str).file_name() {
                            return normalize_system_process_name(name.to_string_lossy().to_string());
                        }
                    }
                }
            } else {
                let _ = CloseHandle(handle);
            }
        }
    }
    // If we can't detect the process, it's likely a system sound
    "System Sounds".to_string()
}

/// Normalizes known system processes to "System Sounds" for cleaner display
fn normalize_system_process_name(name: String) -> String {
    let lower = name.to_lowercase();
    
    // Known Windows system sound processes - these should be tagged as "System Sounds"
    const SYSTEM_SOUND_PROCESSES: &[&str] = &[
        "audiodg.exe",           // Windows Audio Device Graph Isolation
        "svchost.exe",           // Service Host (often handles system sounds)
        "dwm.exe",               // Desktop Window Manager
        "systemsounds.exe",      // System Sounds
        "rundll32.exe",          // Often used for playing system sounds
    ];
    
    for sys_proc in SYSTEM_SOUND_PROCESSES {
        if lower == *sys_proc {
            return "System Sounds".to_string();
        }
    }
    
    name
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
