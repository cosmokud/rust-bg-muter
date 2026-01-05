//! Core muting logic module
//! Implements the background muting algorithm

use crate::audio::AudioManager;
use crate::config::Config;
use crate::process::{get_foreground_pid, ForegroundTracker};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Represents the state of an audio-producing application
#[derive(Debug, Clone)]
pub struct AppAudioState {
    pub pid: u32,
    pub process_name: String,
    pub display_name: String,
    pub is_muted_by_us: bool,
    pub original_mute_state: bool,
    pub last_seen: Instant,
    pub is_active: bool,
}

/// The core muting engine
pub struct MuterEngine {
    audio_manager: Arc<AudioManager>,
    config: Arc<RwLock<Config>>,
    foreground_tracker: ForegroundTracker,
    app_states: HashMap<u32, AppAudioState>,
    muted_pids: HashSet<u32>,
    own_pid: u32,
}

#[allow(dead_code)]
impl MuterEngine {
    /// Creates a new MuterEngine
    pub fn new(config: Arc<RwLock<Config>>) -> Result<Self, Box<dyn std::error::Error>> {
        let audio_manager = Arc::new(AudioManager::new()?);
        
        Ok(Self {
            audio_manager,
            config,
            foreground_tracker: ForegroundTracker::new(),
            app_states: HashMap::new(),
            muted_pids: HashSet::new(),
            own_pid: std::process::id(),
        })
    }

    /// Gets the audio manager
    pub fn audio_manager(&self) -> Arc<AudioManager> {
        self.audio_manager.clone()
    }

    /// Updates the engine state and applies muting logic
    pub fn update(&mut self) -> Result<UpdateResult, Box<dyn std::error::Error>> {
        let config = self.config.read();
        let muting_enabled = config.muting_enabled;
        let excluded_apps = config.excluded_apps.clone();
        drop(config);

        // Get current foreground PID
        let foreground_pid = get_foreground_pid();
        let foreground_changed = self.foreground_tracker.check_foreground_change().is_some();

        // Refresh audio sessions
        let sessions = self.audio_manager.get_sessions();
        
        // Track which PIDs we've seen this update
        let mut seen_pids = HashSet::new();

        // Update app states from audio sessions
        for session in &sessions {
            seen_pids.insert(session.process_id);

            let is_foreground = foreground_pid == Some(session.process_id);
            let is_excluded = excluded_apps.contains(&session.process_name.to_lowercase());
            let is_own_process = session.process_id == self.own_pid;

            let app_state = self.app_states
                .entry(session.process_id)
                .or_insert_with(|| AppAudioState {
                    pid: session.process_id,
                    process_name: session.process_name.clone(),
                    display_name: session.display_name.clone(),
                    is_muted_by_us: false,
                    original_mute_state: session.is_muted,
                    last_seen: Instant::now(),
                    is_active: true,
                });

            app_state.last_seen = Instant::now();
            app_state.is_active = true;
            app_state.display_name = session.display_name.clone();

            // Apply muting logic if enabled
            if muting_enabled && !is_excluded && !is_own_process {
                if is_foreground {
                    // Unmute foreground app if we muted it
                    if app_state.is_muted_by_us {
                        let _ = self.audio_manager.unmute_process(session.process_id);
                        app_state.is_muted_by_us = false;
                        self.muted_pids.remove(&session.process_id);
                    }
                } else {
                    // Mute background app
                    if !app_state.is_muted_by_us && !session.is_muted {
                        app_state.original_mute_state = session.is_muted;
                        let _ = self.audio_manager.mute_process(session.process_id);
                        app_state.is_muted_by_us = true;
                        self.muted_pids.insert(session.process_id);
                    }
                }
            } else if !muting_enabled || is_excluded {
                // Unmute if we previously muted this app
                if app_state.is_muted_by_us {
                    let _ = self.audio_manager.unmute_process(session.process_id);
                    app_state.is_muted_by_us = false;
                    self.muted_pids.remove(&session.process_id);
                }
            }
        }

        // Mark unseen apps as inactive and clean up old entries
        let now = Instant::now();
        let cleanup_threshold = Duration::from_secs(30);
        
        self.app_states.retain(|pid, state| {
            if !seen_pids.contains(pid) {
                state.is_active = false;
                
                // Unmute if we were muting this app
                if state.is_muted_by_us {
                    let _ = self.audio_manager.unmute_process(*pid);
                    self.muted_pids.remove(pid);
                }
                
                // Remove if not seen for too long
                if now.duration_since(state.last_seen) > cleanup_threshold {
                    return false;
                }
            }
            true
        });

        Ok(UpdateResult {
            foreground_pid,
            foreground_changed,
            active_sessions: sessions.len(),
            muted_count: self.muted_pids.len(),
        })
    }

    /// Gets the current app states
    pub fn get_app_states(&self) -> Vec<AppAudioState> {
        self.app_states.values().cloned().collect()
    }

    /// Gets active audio sessions
    pub fn get_active_sessions(&self) -> Vec<AppAudioState> {
        self.app_states
            .values()
            .filter(|s| s.is_active)
            .cloned()
            .collect()
    }

    /// Unmutes all apps that we muted
    pub fn unmute_all(&mut self) {
        for pid in self.muted_pids.drain() {
            let _ = self.audio_manager.unmute_process(pid);
        }
        
        for state in self.app_states.values_mut() {
            state.is_muted_by_us = false;
        }
    }

    /// Forces a refresh of audio sessions
    pub fn force_refresh(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.audio_manager.refresh_sessions()?;
        Ok(())
    }

    /// Gets the number of currently muted apps
    pub fn muted_count(&self) -> usize {
        self.muted_pids.len()
    }

    /// Checks if a specific PID is muted by us
    pub fn is_muted_by_us(&self, pid: u32) -> bool {
        self.muted_pids.contains(&pid)
    }
}

impl Drop for MuterEngine {
    fn drop(&mut self) {
        // Unmute all apps when the engine is dropped
        self.unmute_all();
    }
}

/// Result of an update cycle
#[derive(Debug)]
#[allow(dead_code)]
pub struct UpdateResult {
    pub foreground_pid: Option<u32>,
    pub foreground_changed: bool,
    pub active_sessions: usize,
    pub muted_count: usize,
}
