//! Windows Audio Session API (WASAPI) module
//! Provides efficient, low-overhead audio session management.

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::sync::Arc;
use crate::process;
use windows::core::Interface;
use windows::Win32::Foundation::{FALSE, TRUE};
use windows::Win32::Media::Audio::{
    eCommunications, eConsole, eMultimedia, eRender, IAudioSessionControl2,
    IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
};


/// Represents an audio session with minimal metadata
#[derive(Debug, Clone)]
pub struct AudioSession {
    pub process_id: u32,
    pub process_name: String,
    pub display_name: String,
    pub is_muted: bool,
}

/// One reading of the mixer: plain rows, and no way back into the device. What a caller
/// does with it is a decision about data, not about audio.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub rows: Vec<AudioSession>,
}

impl Snapshot {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
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

    /// Reads every audio session once. This is the only path that touches COM: callers get
    /// plain rows back, so nothing above this line holds an interface.
    pub fn snapshot(&self) -> Snapshot {
        self.try_snapshot().unwrap_or_default()
    }

    /// Refreshes the list of audio sessions
    /// This is the expensive operation - call sparingly
    fn try_snapshot(&self) -> windows::core::Result<Snapshot> {
        unsafe {
            // Initialize COM for this thread (idempotent)
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            // Enumerate sessions from all default render roles for better coverage. The
            // roles resolve to the same endpoint on almost every machine, and a device
            // walked twice redoes its whole COM enumeration - session count, process id and
            // process-name lookup per session - for the same answers, so each endpoint is
            // visited once.
            let mut devices: Vec<IMMDevice> = Vec::new();
            let mut seen_devices: HashSet<String> = HashSet::new();
            for role in [eConsole, eMultimedia, eCommunications] {
                if let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, role) {
                    match device_id(&device) {
                        Some(id) => {
                            if seen_devices.insert(id) {
                                devices.push(device);
                            }
                        }
                        // No id to compare against: keep the device rather than drop one
                        // the other roles did not report.
                        None => devices.push(device),
                    }
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

            Ok(Snapshot { rows: result })
        }
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

    /// Puts every live app session back to the state a fresh session starts in: unmuted,
    /// full volume.
    ///
    /// This is the runtime half of the Volume Mixer reset - the stored per-app
    /// preferences are cleared separately - and it is what makes a reset visible now
    /// instead of at the next session start. A session that cannot be written is
    /// skipped rather than aborting the pass: one protected process must not leave the
    /// rest of the mixer at its old level. Returns the number of sessions updated.
    ///
    /// The device endpoint is deliberately not touched. Windows' mixer reset resets
    /// apps, not the speaker: the master level and master mute belong to the user's
    /// hardware, and a reset that quietly turned the speakers to 100% (or unmuted them)
    /// would be a different, louder action than what was asked for.
    pub fn reset_all(&self) -> usize {
        // The cache is emptied into this pass, so every interface is written through while
        // this function owns its one reference. Going through a raw pointer cloned out of
        // the cache instead would leave two owners of a single reference, and the first of
        // them to drop would free an interface the cache still pointed at.
        let sessions: Vec<ISimpleAudioVolume> = {
            let mut guard = self.sessions.lock();
            std::mem::take(&mut *guard)
                .into_values()
                .map(|session| session.volume)
                .collect()
        };

        // A session write is what the user sees in the mixer, and the endpoint above it is
        // left alone, so the order a session is visited in does not matter.
        let mut changed = 0;
        for volume in &sessions {
            let landed = unsafe {
                let unmuted = volume.SetMute(FALSE, std::ptr::null()).is_ok();
                let full = volume.SetMasterVolume(1.0, std::ptr::null()).is_ok();
                unmuted && full
            };
            if landed {
                changed += 1;
            }
        }

        // The cache is empty now, describing a mixer the muter has not read yet; it is
        // repopulated on the next poll.
        changed
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
                            display_name_of(pid)
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

/// Display name of a session's process, normalized for the lists.
///
/// A pid whose name cannot be read at all is the system sound session, which is what is
/// left when no process owns the audio.
fn display_name_of(pid: u32) -> String {
    match process::name(pid) {
        Some(name) => normalize_system_process_name(name),
        None => "System Sounds".to_string(),
    }
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
                let name = if len > 0 {
                    let slice = std::slice::from_raw_parts(name_ptr.0, len);
                    OsString::from_wide(slice).to_string_lossy().to_string()
                } else {
                    String::new()
                };
                // WASAPI allocates this string with `CoTaskMemAlloc` and hands its
                // ownership over: not freeing it leaked a block per session per refresh,
                // which grew the working set for as long as the app ran.
                CoTaskMemFree(Some(name_ptr.0 as *const std::ffi::c_void));
                if !name.is_empty() && !name.starts_with('@') {
                    return Some(name);
                }
            }
        }
        None
    }
}

/// Endpoint id of `device`, copied and then freed: `GetId` hands back a
/// `CoTaskMemAlloc`ed string whose ownership passes to the caller.
unsafe fn device_id(device: &IMMDevice) -> Option<String> {
    let id = device.GetId().ok()?;
    if id.is_null() {
        return None;
    }
    let len = (0..).take_while(|&i| *id.0.add(i) != 0).count();
    let text = OsString::from_wide(std::slice::from_raw_parts(id.0, len))
        .to_string_lossy()
        .to_string();
    CoTaskMemFree(Some(id.0 as *const std::ffi::c_void));
    Some(text)
}
