//! Core muting logic module
//!
//! The policy is one decision table (`decide`) applied to one reading of the mixer at a
//! time (`apply`). Nothing here holds an audio interface: the reading is plain rows, and
//! `AudioManager` is the only thing that writes a mute back.

use crate::audio::{AudioManager, AudioSession, Snapshot};
use crate::config::Config;
use crate::process::get_foreground_pid;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a state with no session behind it is kept, so a row does not flicker out of
/// the detected list the moment an app stops playing.
const STALE_AFTER: Duration = Duration::from_secs(30);

/// Represents the state of an audio-producing application
#[derive(Debug, Clone)]
pub struct AppAudioState {
    pub pid: u32,
    pub process_name: String,
    pub display_name: String,
    pub is_muted_by_us: bool,
    pub last_seen: Instant,
    pub is_active: bool,
}

/// What one cycle should do to one session's mute state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Mute,
    Unmute,
    Leave,
}

/// The whole policy, as a pure function of one session, the rule sets and the foreground
/// app: always-muted beats excluded beats foreground beats background, and anything the
/// rules say nothing about is left as the user had it.
///
/// `muted_by_us` is whether this engine muted the session, so a rule only ever undoes its
/// own mute - a session muted by the user stays muted when muting is switched off.
pub fn decide(
    row: &AudioSession,
    config: &Config,
    foreground: Option<u32>,
    own_pid: u32,
    muted_by_us: bool,
) -> Action {
    if !config.muting_enabled {
        return if muted_by_us {
            Action::Unmute
        } else {
            Action::Leave
        };
    }
    if row.process_id == own_pid {
        return Action::Leave;
    }

    // One lowercased name per session: the rule sets are stored lowercased.
    let name = row.process_name.to_lowercase();
    if config.always_muted_apps.contains(&name) {
        // Stays muted even while it is the foreground app.
        return if row.is_muted {
            Action::Leave
        } else {
            Action::Mute
        };
    }
    if config.excluded_apps.contains(&name) {
        return if muted_by_us {
            Action::Unmute
        } else {
            Action::Leave
        };
    }
    if foreground == Some(row.process_id) {
        // The active app is audible even if something else muted it.
        return Action::Unmute;
    }
    if row.is_muted {
        Action::Leave
    } else {
        Action::Mute
    }
}

/// The core muting engine - optimized for minimal CPU usage
pub struct MuterEngine {
    audio_manager: Arc<AudioManager>,
    config: Arc<RwLock<Config>>,
    app_states: HashMap<u32, AppAudioState>,
    own_pid: u32,
    last_foreground_pid: Option<u32>,
    last_session_refresh: Instant,
    session_refresh_interval: Duration,
}

impl MuterEngine {
    /// Creates a new MuterEngine
    pub fn new(config: Arc<RwLock<Config>>) -> Result<Self, Box<dyn std::error::Error>> {
        let audio_manager = Arc::new(AudioManager::new()?);

        Ok(Self {
            audio_manager,
            config,
            app_states: HashMap::new(),
            own_pid: std::process::id(),
            last_foreground_pid: None,
            last_session_refresh: Instant::now(),
            session_refresh_interval: Duration::from_secs(2), // Only refresh sessions every 2s
        })
    }

    /// Gets the audio manager
    pub fn audio_manager(&self) -> Arc<AudioManager> {
        self.audio_manager.clone()
    }

    /// One poll: read the foreground window, then run a cycle if anything moved. An idle
    /// poll returns before it copies anything out of the config.
    pub fn update(&mut self) -> Result<UpdateResult, Box<dyn std::error::Error>> {
        let foreground_pid = get_foreground_pid();
        let foreground_changed = foreground_pid != self.last_foreground_pid;

        if !foreground_changed
            && self.last_session_refresh.elapsed() < self.session_refresh_interval
        {
            return Ok(UpdateResult {
                foreground_pid,
                foreground_changed: false,
                active_sessions: self.app_states.len(),
                muted_count: self.muted_count(),
            });
        }

        // Refresh audio sessions (expensive COM operation), then decide on what it read.
        self.last_session_refresh = Instant::now();
        let snapshot = self.audio_manager.snapshot();
        Ok(self.apply(&snapshot, foreground_pid))
    }

    /// One cycle against a given reading of the mixer and foreground app: the decisions and
    /// the writes they imply. This is the whole engine minus the clock and the COM read,
    /// which is what makes the policy testable without a mixer.
    pub fn apply(&mut self, snapshot: &Snapshot, foreground_pid: Option<u32>) -> UpdateResult {
        let previous_foreground_pid = self.last_foreground_pid;
        let foreground_changed = foreground_pid != previous_foreground_pid;
        self.last_foreground_pid = foreground_pid;

        let config = self.config.read();
        let audio = self.audio_manager.clone();

        // A reading that came back empty is a failed reading, not an empty mixer: the
        // cached states stand in for it, with their mute state as far as it is known, so a
        // transient COM failure cannot silently unmute what this engine muted.
        let cached: Vec<AudioSession> = if snapshot.is_empty() {
            self.cached_rows(previous_foreground_pid)
        } else {
            Vec::new()
        };
        let rows: &[AudioSession] = if cached.is_empty() {
            &snapshot.rows
        } else {
            &cached
        };

        let mut seen_pids = HashSet::with_capacity(rows.len());
        for row in rows {
            seen_pids.insert(row.process_id);

            let state = self
                .app_states
                .entry(row.process_id)
                .or_insert_with(|| AppAudioState {
                    pid: row.process_id,
                    process_name: row.process_name.clone(),
                    display_name: row.display_name.clone(),
                    is_muted_by_us: false,
                    last_seen: Instant::now(),
                    is_active: true,
                });

            state.last_seen = Instant::now();
            state.is_active = true;

            let muted_by_us = state.is_muted_by_us;
            match decide(row, &config, foreground_pid, self.own_pid, muted_by_us) {
                Action::Mute => {
                    let _ = audio.mute_process(row.process_id);
                    state.is_muted_by_us = true;
                }
                Action::Unmute => {
                    let _ = audio.unmute_process(row.process_id);
                    state.is_muted_by_us = false;
                }
                Action::Leave => {}
            }
        }

        // Unseen apps: inactive, and unmuted once if the mute was ours. Their session is
        // gone, so the write only matters for a process that is still alive.
        let now = Instant::now();
        self.app_states.retain(|pid, state| {
            if seen_pids.contains(pid) {
                return true;
            }
            state.is_active = false;
            if state.is_muted_by_us {
                let _ = audio.unmute_process(*pid);
                state.is_muted_by_us = false;
            }
            now.duration_since(state.last_seen) <= STALE_AFTER
        });

        UpdateResult {
            foreground_pid,
            foreground_changed,
            active_sessions: self.app_states.len(),
            muted_count: self.muted_count(),
        }
    }

    /// Active states as rows, for the cycle that has no fresh reading to work from.
    fn cached_rows(&self, previous_foreground_pid: Option<u32>) -> Vec<AudioSession> {
        self.app_states
            .values()
            .filter(|state| state.is_active)
            .map(|state| AudioSession {
                process_id: state.pid,
                process_name: state.process_name.clone(),
                display_name: state.display_name.clone(),
                // "Unknown" is read as muted when the mute is ours or the app just lost
                // focus, so deciding again cannot unmute an app we muted.
                is_muted: state.is_muted_by_us || previous_foreground_pid == Some(state.pid),
            })
            .collect()
    }

    /// Unmutes all apps that we muted
    pub fn unmute_all(&mut self) {
        let audio = self.audio_manager.clone();
        for state in self.app_states.values_mut() {
            if state.is_muted_by_us {
                let _ = audio.unmute_process(state.pid);
                state.is_muted_by_us = false;
            }
        }
    }

    /// Gets the number of currently muted apps
    pub fn muted_count(&self) -> usize {
        self.app_states
            .values()
            .filter(|state| state.is_muted_by_us)
            .count()
    }

    /// Checks if a specific PID is muted by us
    #[allow(dead_code)]
    pub fn is_muted_by_us(&self, pid: u32) -> bool {
        self.app_states
            .get(&pid)
            .is_some_and(|state| state.is_muted_by_us)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, name: &str, muted: bool) -> AudioSession {
        AudioSession {
            process_id: pid,
            process_name: name.to_string(),
            display_name: name.to_string(),
            is_muted: muted,
        }
    }

    fn rules(excluded: &[&str], always_muted: &[&str]) -> Config {
        let mut config = Config::default();
        for name in excluded {
            config.insert_excluded_app(name);
        }
        for name in always_muted {
            config.insert_always_muted_app(name);
        }
        config
    }

    const OWN_PID: u32 = 99;

    /// The purpose of the app in one assertion: the active app is audible and the rest are
    /// muted, whoever muted them before.
    #[test]
    fn the_foreground_app_is_audible_and_the_rest_are_muted() {
        let config = rules(&[], &[]);
        assert_eq!(
            decide(&row(1, "game.exe", false), &config, Some(1), OWN_PID, false),
            Action::Unmute
        );
        assert_eq!(
            decide(&row(1, "game.exe", true), &config, Some(1), OWN_PID, true),
            Action::Unmute
        );
        assert_eq!(
            decide(
                &row(2, "music.exe", false),
                &config,
                Some(1),
                OWN_PID,
                false
            ),
            Action::Mute
        );
    }

    /// Always-muted outranks the foreground rule; excluded outranks it in the other
    /// direction, and neither touches a mute it does not own.
    #[test]
    fn rule_sets_outrank_the_foreground() {
        let config = rules(&["spotify.exe"], &["steam.exe"]);
        assert_eq!(
            decide(
                &row(3, "Steam.exe", false),
                &config,
                Some(3),
                OWN_PID,
                false
            ),
            Action::Mute,
            "an always-muted app must stay muted while focused"
        );
        assert_eq!(
            decide(
                &row(4, "Spotify.exe", true),
                &config,
                Some(1),
                OWN_PID,
                false
            ),
            Action::Leave,
            "a mute we do not own is the user's"
        );
        assert_eq!(
            decide(
                &row(4, "spotify.exe", true),
                &config,
                Some(1),
                OWN_PID,
                true
            ),
            Action::Unmute
        );
    }

    /// Turning muting off undoes this engine's mutes and nothing else.
    #[test]
    fn disabling_muting_only_undoes_our_own_mutes() {
        let config = Config {
            muting_enabled: false,
            ..Config::default()
        };
        assert_eq!(
            decide(&row(2, "music.exe", true), &config, Some(1), OWN_PID, true),
            Action::Unmute
        );
        assert_eq!(
            decide(&row(2, "music.exe", true), &config, Some(1), OWN_PID, false),
            Action::Leave
        );
    }

    /// The engine itself, over a fabricated reading: no mixer, no window, no clock.
    #[test]
    fn a_cycle_decides_over_the_reading_it_is_given() {
        let engine_config = Arc::new(RwLock::new(rules(&["spotify.exe"], &[])));
        let mut engine = MuterEngine::new(engine_config).expect("engine");
        let snapshot = Snapshot {
            rows: vec![
                row(1, "game.exe", false),
                row(2, "music.exe", false),
                row(3, "spotify.exe", false),
                row(4, "steam.exe", false),
            ],
        };

        let result = engine.apply(&snapshot, Some(1));
        assert_eq!(result.active_sessions, 4);
        assert!(result.foreground_changed);

        let muted = |pid: u32| engine.is_muted_by_us(pid);
        assert!(!muted(1), "the foreground app was muted");
        assert!(muted(2), "a background app stayed audible");
        assert!(!muted(3), "an excluded app was muted");
        assert!(muted(4));
        assert_eq!(engine.muted_count(), 2);

        // The same reading again is a no-op: nothing is re-decided in a way that writes.
        let again = engine.apply(&snapshot, Some(1));
        assert!(!again.foreground_changed);
        assert_eq!(again.muted_count, 2);

        engine.unmute_all();
        assert_eq!(engine.muted_count(), 0);
    }

    /// An empty reading is a failed one: the cached states decide, so an app that loses its
    /// session - or a COM hiccup - does not resurrect a mute this engine made.
    #[test]
    fn an_empty_reading_keeps_our_mutes_in_place() {
        let engine_config = Arc::new(RwLock::new(Config::default()));
        let mut engine = MuterEngine::new(engine_config).expect("engine");
        engine.apply(
            &Snapshot {
                rows: vec![row(1, "game.exe", false), row(2, "music.exe", false)],
            },
            Some(1),
        );
        assert!(engine.is_muted_by_us(2));

        // Focus moves to an app with no session while the mixer cannot be read: the muted
        // app keeps the mute this engine owns, and nothing is unmuted by accident.
        engine.apply(&Snapshot::default(), Some(3));
        assert!(engine.is_muted_by_us(2), "the mute we own was dropped");
        assert!(!engine.is_muted_by_us(1));
    }
}
