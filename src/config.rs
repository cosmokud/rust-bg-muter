//! Configuration and persistence module
//! Handles saving and loading of application settings and exclusion lists

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// List of excluded process names (e.g., "spotify.exe")
    #[serde(default)]
    pub excluded_apps: HashSet<String>,

    /// List of apps that should always be muted (even when foreground)
    #[serde(default)]
    pub always_muted_apps: HashSet<String>,
    
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
            always_muted_apps: HashSet::new(),
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
        
        config_dir.join("config.ini")
    }

    /// Gets legacy JSON config path (for migration)
    fn legacy_json_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rust-bg-muter");

        config_dir.join("config.json")
    }

    /// Loads configuration from disk (INI format, with JSON migration fallback)
    pub fn load() -> Self {
        let path = Self::config_path();

        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => {
                    match Self::from_ini_str(&contents) {
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

        // Migration path: if INI doesn't exist (or failed to parse), try legacy JSON
        let legacy_path = Self::legacy_json_path();
        if legacy_path.exists() {
            match fs::read_to_string(&legacy_path) {
                Ok(contents) => {
                    match serde_json::from_str::<Config>(&contents) {
                        Ok(config) => {
                            if let Err(e) = config.save() {
                                log::warn!("Failed to migrate JSON config to INI: {}", e);
                            } else {
                                log::info!("Migrated legacy config from {:?} to {:?}", legacy_path, path);
                            }
                            return config;
                        }
                        Err(e) => {
                            log::error!("Failed to parse legacy JSON config: {}", e);
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to read legacy JSON config: {}", e);
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
        
        let contents = self.to_ini_string();
        fs::write(&path, contents)?;
        
        log::info!("Config saved to {:?}", path);
        Ok(())
    }

    /// Opens config.ini with the default text editor on Windows
    pub fn open_in_default_editor() -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::config_path();

        // Ensure file exists before opening
        if !path.exists() {
            Self::default().save()?;
        }

        Command::new("cmd")
            .arg("/C")
            .arg("start")
            .arg("")
            .arg(path.as_os_str())
            .spawn()?;

        Ok(())
    }

    fn to_ini_string(&self) -> String {
        let mut output = String::new();

        output.push_str("; Background Muter configuration\n");
        output.push_str("[general]\n");
        output.push_str(&format!("muting_enabled={}\n", self.muting_enabled));
        output.push_str(&format!("poll_interval_ms={}\n", self.poll_interval_ms));
        output.push_str(&format!("start_minimized={}\n", self.start_minimized));
        output.push_str(&format!("minimize_to_tray={}\n", self.minimize_to_tray));
        output.push_str(&format!(
            "minimize_button_to_tray={}\n",
            self.minimize_button_to_tray
        ));
        output.push_str(&format!("start_with_windows={}\n", self.start_with_windows));

        output.push_str("\n[excluded_apps]\n");
        let mut excluded_apps: Vec<_> = self.excluded_apps.iter().collect();
        excluded_apps.sort_unstable();
        for app in excluded_apps {
            output.push_str(app);
            output.push_str("=1\n");
        }

        output.push_str("\n[always_muted_apps]\n");
        let mut always_muted_apps: Vec<_> = self.always_muted_apps.iter().collect();
        always_muted_apps.sort_unstable();
        for app in always_muted_apps {
            output.push_str(app);
            output.push_str("=1\n");
        }

        if let Some(window_state) = &self.window_state {
            output.push_str("\n[window_state]\n");
            output.push_str(&format!("x={}\n", window_state.x));
            output.push_str(&format!("y={}\n", window_state.y));
            output.push_str(&format!("width={}\n", window_state.width));
            output.push_str(&format!("height={}\n", window_state.height));
        }

        output
    }

    fn from_ini_str(contents: &str) -> Result<Self, String> {
        let mut config = Self::default();
        let mut section = String::new();

        let mut win_x: Option<f32> = None;
        let mut win_y: Option<f32> = None;
        let mut win_width: Option<f32> = None;
        let mut win_height: Option<f32> = None;

        for (index, raw_line) in contents.lines().enumerate() {
            let line_no = index + 1;
            let line = raw_line.trim();

            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_lowercase();
                continue;
            }

            let (raw_key, raw_value) = line
                .split_once('=')
                .map(|(k, v)| (k.trim(), v.trim()))
                .unwrap_or((line, "1"));

            if raw_key.is_empty() {
                continue;
            }

            match section.as_str() {
                "general" => match raw_key {
                    "muting_enabled" => {
                        config.muting_enabled =
                            Self::parse_bool(raw_value).map_err(|e| format!("line {}: {}", line_no, e))?
                    }
                    "poll_interval_ms" => {
                        config.poll_interval_ms = Self::parse_u64(raw_value)
                            .map_err(|e| format!("line {}: {}", line_no, e))?
                    }
                    "start_minimized" => {
                        config.start_minimized =
                            Self::parse_bool(raw_value).map_err(|e| format!("line {}: {}", line_no, e))?
                    }
                    "minimize_to_tray" => {
                        config.minimize_to_tray =
                            Self::parse_bool(raw_value).map_err(|e| format!("line {}: {}", line_no, e))?
                    }
                    "minimize_button_to_tray" => {
                        config.minimize_button_to_tray =
                            Self::parse_bool(raw_value).map_err(|e| format!("line {}: {}", line_no, e))?
                    }
                    "start_with_windows" => {
                        config.start_with_windows =
                            Self::parse_bool(raw_value).map_err(|e| format!("line {}: {}", line_no, e))?
                    }
                    _ => {}
                },
                "excluded_apps" => {
                    if Self::parse_app_flag(raw_value)
                        .map_err(|e| format!("line {}: {}", line_no, e))?
                    {
                        config.excluded_apps.insert(raw_key.to_lowercase());
                    }
                }
                "always_muted_apps" => {
                    if Self::parse_app_flag(raw_value)
                        .map_err(|e| format!("line {}: {}", line_no, e))?
                    {
                        config.always_muted_apps.insert(raw_key.to_lowercase());
                    }
                }
                "window_state" => match raw_key {
                    "x" => {
                        win_x = Some(
                            Self::parse_f32(raw_value).map_err(|e| format!("line {}: {}", line_no, e))?,
                        )
                    }
                    "y" => {
                        win_y = Some(
                            Self::parse_f32(raw_value).map_err(|e| format!("line {}: {}", line_no, e))?,
                        )
                    }
                    "width" => {
                        win_width = Some(
                            Self::parse_f32(raw_value).map_err(|e| format!("line {}: {}", line_no, e))?,
                        )
                    }
                    "height" => {
                        win_height = Some(
                            Self::parse_f32(raw_value).map_err(|e| format!("line {}: {}", line_no, e))?,
                        )
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        if win_x.is_some() || win_y.is_some() || win_width.is_some() || win_height.is_some() {
            if let (Some(x), Some(y), Some(width), Some(height)) = (win_x, win_y, win_width, win_height)
            {
                config.window_state = Some(WindowState {
                    x,
                    y,
                    width,
                    height,
                });
            }
        }

        Ok(config)
    }

    fn parse_bool(value: &str) -> Result<bool, String> {
        match value.trim().to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            other => Err(format!("invalid boolean value: '{}'", other)),
        }
    }

    fn parse_app_flag(value: &str) -> Result<bool, String> {
        if value.trim().is_empty() {
            return Ok(true);
        }
        Self::parse_bool(value)
    }

    fn parse_u64(value: &str) -> Result<u64, String> {
        value
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("invalid integer value: '{}'", value.trim()))
    }

    fn parse_f32(value: &str) -> Result<f32, String> {
        value
            .trim()
            .parse::<f32>()
            .map_err(|_| format!("invalid float value: '{}'", value.trim()))
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

    /// Adds an app to the always-muted list
    pub fn add_always_muted_app(&mut self, app_name: &str) {
        let normalized = app_name.to_lowercase();
        self.always_muted_apps.insert(normalized);
        let _ = self.save();
    }

    /// Removes an app from the always-muted list
    pub fn remove_always_muted_app(&mut self, app_name: &str) {
        let normalized = app_name.to_lowercase();
        self.always_muted_apps.remove(&normalized);
        let _ = self.save();
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
        
        config.add_excluded_app("Spotify.exe");
        assert!(config.is_excluded("spotify.exe"));
        assert!(config.is_excluded("SPOTIFY.EXE"));
        
        config.remove_excluded_app("spotify.exe");
        assert!(!config.is_excluded("spotify.exe"));
    }

    #[test]
    fn test_serialization() {
        let mut config = Config::default();
        config.excluded_apps.insert("test.exe".to_string());
        config.always_muted_apps.insert("always.exe".to_string());
        config.window_state = Some(WindowState {
            x: 10.0,
            y: 20.0,
            width: 1024.0,
            height: 768.0,
        });
        
        let ini = config.to_ini_string();
        let loaded = Config::from_ini_str(&ini).unwrap();
        
        assert!(loaded.is_excluded("test.exe"));
        assert!(loaded.is_always_muted("always.exe"));
        assert!(loaded.window_state.is_some());
    }

    #[test]
    fn test_legacy_json_deserialization() {
        let json = r#"{
            "excluded_apps": ["spotify.exe"],
            "always_muted_apps": ["steam.exe"],
            "muting_enabled": true,
            "poll_interval_ms": 500,
            "start_minimized": false,
            "minimize_to_tray": true,
            "minimize_button_to_tray": true,
            "start_with_windows": false,
            "window_state": null
        }"#;

        let loaded: Config = serde_json::from_str(json).unwrap();
        assert!(loaded.is_excluded("spotify.exe"));
        assert!(loaded.is_always_muted("steam.exe"));
    }
}
