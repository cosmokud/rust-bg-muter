//! Configuration and persistence module
//! Handles saving and loading of application settings and exclusion lists

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// List of excluded process names (e.g., "spotify.exe")
    ///
    /// Shared behind an `Arc`: every reader that needs the rules to survive a lock drop - the
    /// muting cycle, the dialog's list refresh - clones the handle, not the names, and a
    /// write copies the set once (`Arc::make_mut`).
    #[serde(default)]
    pub excluded_apps: Arc<HashSet<String>>,

    /// List of apps that should always be muted (even when foreground)
    #[serde(default)]
    pub always_muted_apps: Arc<HashSet<String>>,

    /// Whether the muting functionality is enabled
    #[serde(default = "default_enabled")]
    pub muting_enabled: bool,

    /// Polling interval in milliseconds for checking foreground changes
    #[serde(default = "default_poll_interval")]
    pub poll_interval_ms: u64,

    /// Whether to minimize to tray instead of closing
    #[serde(default = "default_minimize_to_tray")]
    pub minimize_to_tray: bool,

    /// Whether to minimize to tray when minimize button is clicked
    #[serde(default = "default_minimize_button_to_tray")]
    pub minimize_button_to_tray: bool,

    /// Whether to start with Windows
    #[serde(default)]
    pub start_with_windows: bool,

    /// Window position and size
    #[serde(default)]
    pub window_state: Option<WindowState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

fn default_enabled() -> bool {
    true
}

fn default_poll_interval() -> u64 {
    500 // 500ms polling interval - balances responsiveness with CPU efficiency
}

fn default_minimize_to_tray() -> bool {
    true
}

fn default_minimize_button_to_tray() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            excluded_apps: Arc::default(),
            always_muted_apps: Arc::default(),
            muting_enabled: true,
            poll_interval_ms: 500,
            minimize_to_tray: true,
            minimize_button_to_tray: true,
            start_with_windows: false,
            window_state: None,
        }
    }
}

/// Points the config at a directory of one's own: a test run, a CI job or a portable copy.
/// Set, it is the only place this module reads or writes, so the user's real config cannot be
/// reached at all.
pub const CONFIG_DIR_ENV: &str = "RUST_BG_MUTER_CONFIG_DIR";

#[allow(dead_code)]
impl Config {
    /// Gets the config file path
    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    /// The directory `config.json` lives in, in order of precedence:
    /// the `RUST_BG_MUTER_CONFIG_DIR` override, then a throwaway directory for a test binary,
    /// then the machine's per-user config directory.
    ///
    /// The test branch is what keeps a `cargo test` away from a real config: the dialog tests
    /// drive the same handlers the app does, and one saving insert (`add_excluded_app` and its
    /// kind) rewrote a user's exclusion list with the fixtures the test had just made.
    pub fn config_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os(CONFIG_DIR_ENV).filter(|dir| !dir.is_empty()) {
            let dir = PathBuf::from(dir);
            let _ = fs::create_dir_all(&dir);
            return dir;
        }

        let dir = if cfg!(test) {
            std::env::temp_dir().join(format!("rust-bg-muter-test-{}", std::process::id()))
        } else {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rust-bg-muter")
        };

        if !dir.exists() {
            let _ = fs::create_dir_all(&dir);
        }

        dir
    }

    /// Loads configuration from disk
    ///
    /// A file that exists but cannot be read or parsed is left exactly as it is and defaults are
    /// used for this run: writing defaults over it would destroy a list the user configured (and
    /// any hand edit they were in the middle of). The file is only created when it is missing.
    pub fn load() -> Self {
        let path = Self::config_path();

        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => match serde_json::from_str(&contents) {
                    Ok(config) => return config,
                    Err(e) => {
                        log::error!(
                            "Failed to parse {}, keeping it as it is: {}",
                            path.display(),
                            e
                        );
                    }
                },
                Err(e) => {
                    log::error!(
                        "Failed to read {}, keeping it as it is: {}",
                        path.display(),
                        e
                    );
                }
            }
            return Self::default();
        }

        let default = Self::default();
        let _ = default.save();
        default
    }

    /// Saves configuration to disk
    ///
    /// Written beside the file and renamed over it, so an interrupted or failed write leaves the
    /// previous config intact instead of a half-written one.
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::config_path();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let contents = serde_json::to_string_pretty(self)?;
        let staged = path.with_extension("json.tmp");
        fs::write(&staged, contents)?;
        fs::rename(&staged, &path)?;

        log::info!("Config saved to {}", path.display());
        Ok(())
    }

    /// Adds an app to the exclusion list
    pub fn add_excluded_app(&mut self, app_name: &str) {
        self.insert_excluded_app(app_name);
        let _ = self.save();
    }

    /// Adds an app to the exclusion list without touching the config file: the dialog keeps
    /// every list edit in memory and the file is written once, by `save_settings`.
    pub fn insert_excluded_app(&mut self, app_name: &str) {
        Arc::make_mut(&mut self.excluded_apps).insert(app_name.to_lowercase());
    }

    /// Removes an app from the exclusion list
    pub fn remove_excluded_app(&mut self, app_name: &str) {
        self.remove_excluded_app_in_memory(app_name);
        let _ = self.save();
    }

    /// Removes an app from the exclusion list without touching the config file.
    pub fn remove_excluded_app_in_memory(&mut self, app_name: &str) {
        Arc::make_mut(&mut self.excluded_apps).remove(&app_name.to_lowercase());
    }

    /// Checks if an app is in the exclusion list
    pub fn is_excluded(&self, app_name: &str) -> bool {
        let normalized = app_name.to_lowercase();
        self.excluded_apps.contains(&normalized)
    }

    /// Adds an app to the always-muted list
    pub fn add_always_muted_app(&mut self, app_name: &str) {
        self.insert_always_muted_app(app_name);
        let _ = self.save();
    }

    /// Adds an app to the always-muted list without touching the config file.
    pub fn insert_always_muted_app(&mut self, app_name: &str) {
        Arc::make_mut(&mut self.always_muted_apps).insert(app_name.to_lowercase());
    }

    /// Removes an app from the always-muted list
    pub fn remove_always_muted_app(&mut self, app_name: &str) {
        self.remove_always_muted_app_in_memory(app_name);
        let _ = self.save();
    }

    /// Removes an app from the always-muted list without touching the config file.
    pub fn remove_always_muted_app_in_memory(&mut self, app_name: &str) {
        Arc::make_mut(&mut self.always_muted_apps).remove(&app_name.to_lowercase());
    }

    /// Checks if an app is in the always-muted list
    pub fn is_always_muted(&self, app_name: &str) -> bool {
        let normalized = app_name.to_lowercase();
        self.always_muted_apps.contains(&normalized)
    }

    /// Toggles muting functionality
    pub fn toggle_muting(&mut self) -> bool {
        self.muting_enabled = !self.muting_enabled;
        let _ = self.save();
        self.muting_enabled
    }

    /// Sets muting state
    pub fn set_muting(&mut self, enabled: bool) {
        self.muting_enabled = enabled;
        let _ = self.save();
    }
}

/// One lock for every test that touches the per-user config directory or the process-wide
/// override that points it elsewhere.
///
/// The override is an environment variable, so it is shared by the whole test binary: a
/// test that sets it while another saves a config makes that other test write into the
/// override's directory, and makes every path resolved afterwards point somewhere else.
#[cfg(test)]
pub(crate) fn config_dir_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.muting_enabled);
        assert!(config.excluded_apps.is_empty());
        assert!(config.always_muted_apps.is_empty());
        assert_eq!(config.poll_interval_ms, 500);
    }

    #[test]
    fn test_exclusion_list() {
        let mut config = Config::default();

        config.insert_excluded_app("Spotify.exe");
        assert!(config.is_excluded("spotify.exe"));
        assert!(config.is_excluded("SPOTIFY.EXE"));

        config.remove_excluded_app_in_memory("spotify.exe");
        assert!(!config.is_excluded("spotify.exe"));
    }

    #[test]
    fn test_serialization() {
        let mut config = Config::default();
        config.insert_excluded_app("test.exe");
        config.insert_always_muted_app("always.exe");

        let json = serde_json::to_string(&config).unwrap();
        let loaded: Config = serde_json::from_str(&json).unwrap();

        assert!(loaded.is_excluded("test.exe"));
        assert!(loaded.is_always_muted("always.exe"));
    }

    /// The lists above must never reach the config file: an insert that saves wipes the
    /// lists a user configured, which is how a run of the test suite emptied them once.
    #[test]
    fn test_inserts_do_not_write_the_config_file() {
        let _config_dir = config_dir_lock();
        let path = Config::config_path();
        let before = std::fs::read(&path).ok();
        let mut config = Config::default();
        config.insert_excluded_app("test.exe");
        config.insert_always_muted_app("always.exe");
        let after = std::fs::read(&path).ok();
        assert_eq!(before, after, "a pure insert wrote {path:?}");
    }

    /// The user's real config, resolved without going through `config_path`, so a bug in the
    /// test isolation cannot hide behind the isolation it is testing.
    fn real_config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rust-bg-muter")
            .join("config.json")
    }

    /// The saving handlers must resolve somewhere throwaway, whatever else is set.
    #[test]
    fn a_test_run_never_resolves_the_users_config() {
        let _config_dir = config_dir_lock();
        let resolved = Config::config_path();
        assert_ne!(
            resolved,
            real_config_path(),
            "tests use the user's config file"
        );
        assert!(resolved.starts_with(std::env::temp_dir()), "{resolved:?}");
    }

    /// Any handler that saves, driven here directly, must leave the user's file byte for byte
    /// as it was: this is the sequence that emptied a real exclusion list once.
    #[test]
    fn a_saving_handler_leaves_the_users_config_untouched() {
        let _config_dir = config_dir_lock();
        let real = real_config_path();
        let before = std::fs::read(&real).ok();

        let mut config = Config::default();
        config.add_excluded_app("test-only.exe");
        config.add_always_muted_app("test-only.exe");
        config.remove_excluded_app("test-only.exe");
        config.toggle_muting();
        config.set_muting(true);

        assert_eq!(before, std::fs::read(&real).ok(), "a test wrote {real:?}");
    }

    /// A file that cannot be parsed is a list to repair, not one to overwrite.
    #[test]
    fn load_keeps_a_file_it_cannot_parse() {
        let _config_dir = config_dir_lock();
        let dir = Config::config_dir().join("unparseable");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let broken = b"{\"excluded_apps\": [\"spotify.exe\",}";
        std::fs::write(&path, broken).unwrap();
        std::env::set_var(CONFIG_DIR_ENV, &dir);

        let loaded = Config::load();
        std::env::remove_var(CONFIG_DIR_ENV);

        assert_eq!(
            std::fs::read(&path).unwrap(),
            broken,
            "load rewrote the file"
        );
        assert!(
            loaded.excluded_apps.is_empty(),
            "defaults expected for this run"
        );
    }
}
