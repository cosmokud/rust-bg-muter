//! Configuration and persistence module
//! Handles saving and loading of application settings and exclusion lists

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// List of excluded process names (e.g., "spotify.exe")
    #[serde(default)]
    pub excluded_apps: HashSet<String>,
    
    /// Whether the muting functionality is enabled
    #[serde(default = "default_enabled")]
    pub muting_enabled: bool,
    
    /// Polling interval in milliseconds for checking foreground changes
    #[serde(default = "default_poll_interval")]
    pub poll_interval_ms: u64,
    
    /// Whether to start minimized to system tray
    #[serde(default = "default_start_minimized")]
    pub start_minimized: bool,
    
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

fn default_start_minimized() -> bool {
    false
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
            excluded_apps: HashSet::new(),
            muting_enabled: true,
            poll_interval_ms: 500,
            start_minimized: false,
            minimize_to_tray: true,
            minimize_button_to_tray: true,
            start_with_windows: false,
            window_state: None,
        }
    }
}

#[allow(dead_code)]
impl Config {
    /// Gets the config file path
    pub fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rust-bg-muter");
        
        if !config_dir.exists() {
            let _ = fs::create_dir_all(&config_dir);
        }
        
        config_dir.join("config.json")
    }

    /// Loads configuration from disk
    pub fn load() -> Self {
        let path = Self::config_path();
        
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => {
                    match serde_json::from_str(&contents) {
                        Ok(config) => return config,
                        Err(e) => {
                            log::error!("Failed to parse config: {}", e);
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to read config: {}", e);
                }
            }
        }
        
        // Return default config if loading fails
        let default = Self::default();
        let _ = default.save();
        default
    }

    /// Saves configuration to disk
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::config_path();
        
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        let contents = serde_json::to_string_pretty(self)?;
        fs::write(&path, contents)?;
        
        log::info!("Config saved to {:?}", path);
        Ok(())
    }

    /// Adds an app to the exclusion list
    pub fn add_excluded_app(&mut self, app_name: &str) {
        let normalized = app_name.to_lowercase();
        self.excluded_apps.insert(normalized);
        let _ = self.save();
    }

    /// Removes an app from the exclusion list
    pub fn remove_excluded_app(&mut self, app_name: &str) {
        let normalized = app_name.to_lowercase();
        self.excluded_apps.remove(&normalized);
        let _ = self.save();
    }

    /// Checks if an app is in the exclusion list
    pub fn is_excluded(&self, app_name: &str) -> bool {
        let normalized = app_name.to_lowercase();
        self.excluded_apps.contains(&normalized)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.muting_enabled);
        assert!(config.excluded_apps.is_empty());
        assert_eq!(config.poll_interval_ms, 100);
    }

    #[test]
    fn test_exclusion_list() {
        let mut config = Config::default();
        
        config.add_excluded_app("Spotify.exe");
        assert!(config.is_excluded("spotify.exe"));
        assert!(config.is_excluded("SPOTIFY.EXE"));
        
        config.remove_excluded_app("spotify.exe");
        assert!(!config.is_excluded("spotify.exe"));
    }

    #[test]
    fn test_serialization() {
        let mut config = Config::default();
        config.add_excluded_app("test.exe");
        
        let json = serde_json::to_string(&config).unwrap();
        let loaded: Config = serde_json::from_str(&json).unwrap();
        
        assert!(loaded.is_excluded("test.exe"));
    }
}
