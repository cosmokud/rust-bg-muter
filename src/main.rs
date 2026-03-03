//! Background Muter - A lightweight Windows tray application
//!
//! Automatically mutes background applications with minimal resource usage.
//! Target: <0.2% CPU, <5MB RAM, 0 VRAM

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod muter;
mod process;
mod settings_dialog;
mod startup;
mod tray;

use config::Config;
use muter::MuterEngine;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tray::{SystemTray, TrayCommand};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

/// Application entry point
fn main() {
    // Initialize logging (minimal in release)
    #[cfg(debug_assertions)]
    {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .format_timestamp_millis()
            .init();
    }

    log::info!("Background Muter starting (lightweight mode)...");

    // Load configuration
    let config = Arc::new(RwLock::new(Config::load()));
    log::info!("Configuration loaded");

    // Apply startup registry setting
    {
        let start_with_windows = config.read().start_with_windows;
        if let Err(e) = startup::apply_startup_setting(start_with_windows) {
            log::warn!("Failed to apply startup setting: {}", e);
        }
    }

    // Shared state
    let should_exit = Arc::new(AtomicBool::new(false));
    let muting_enabled = Arc::new(AtomicBool::new(config.read().muting_enabled));

    // Create muter engine
    let engine = match MuterEngine::new(config.clone()) {
        Ok(e) => Arc::new(RwLock::new(e)),
        Err(e) => {
            log::error!("Failed to create muter engine: {}", e);
            return;
        }
    };

    // Start background muting thread
    let muting_thread = {
        let config = config.clone();
        let engine = engine.clone();
        let should_exit = should_exit.clone();
        let muting_enabled = muting_enabled.clone();

        thread::spawn(move || {
            // Initialize COM for audio operations
            unsafe {
                let _ = CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }

            log::info!("Muting thread started");

            while !should_exit.load(Ordering::Relaxed) {
                // Only do work if muting is enabled
                if muting_enabled.load(Ordering::Relaxed) {
                    if let Some(mut eng) = engine.try_write() {
                        if let Err(e) = eng.update() {
                            log::error!("Muter update error: {}", e);
                        }
                    }
                }

                // Sleep for poll interval (longer = less CPU)
                let poll_ms = config.read().poll_interval_ms;
                thread::sleep(Duration::from_millis(poll_ms));
            }

            // Cleanup: unmute all before exit
            if let Some(mut eng) = engine.try_write() {
                eng.unmute_all();
            }

            unsafe {
                CoUninitialize();
            }

            log::info!("Muting thread stopped");
        })
    };

    // Run tray application on main thread (COM apartment-threaded for UI)
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    run_tray_loop(
        config.clone(),
        engine.clone(),
        should_exit.clone(),
        muting_enabled.clone(),
    );

    // Signal exit and wait for muting thread
    should_exit.store(true, Ordering::SeqCst);
    let _ = muting_thread.join();

    // Final cleanup
    if let Some(mut eng) = engine.try_write() {
        eng.unmute_all();
    }

    unsafe {
        CoUninitialize();
    }

    log::info!("Background Muter shutdown complete");
}

/// Main tray message loop - blocks until exit
fn run_tray_loop(
    config: Arc<RwLock<Config>>,
    engine: Arc<RwLock<MuterEngine>>,
    should_exit: Arc<AtomicBool>,
    muting_enabled: Arc<AtomicBool>,
) {
    let mut tray = match SystemTray::new(muting_enabled.load(Ordering::Relaxed)) {
        Ok(t) => t,
        Err(e) => {
            log::error!("Failed to create system tray: {}", e);
            return;
        }
    };

    log::info!("System tray initialized");

    // Message pump with minimal CPU usage
    loop {
        // Process Windows messages (blocking with timeout for efficiency)
        if !tray.pump_messages(Duration::from_millis(500)) {
            break;
        }

        // Check for exit signal from other threads
        if should_exit.load(Ordering::Relaxed) {
            break;
        }

        // Process tray commands
        while let Some(cmd) = tray.poll_command() {
            match cmd {
                TrayCommand::ToggleMuting => {
                    let enabled = {
                        let mut cfg = config.write();
                        cfg.toggle_muting()
                    };
                    muting_enabled.store(enabled, Ordering::SeqCst);
                    tray.update_state(enabled);

                    // If disabling, unmute everything immediately
                    if !enabled {
                        if let Some(mut eng) = engine.try_write() {
                            eng.unmute_all();
                        }
                    }

                    log::info!("Muting toggled: {}", enabled);
                }
                TrayCommand::OpenSettings => {
                    // Open native Win32 settings dialog
                    settings_dialog::open_settings_dialog(
                        config.clone(),
                        muting_enabled.clone(),
                    );
                    
                    // Sync muting state after dialog closes (user may have changed it)
                    let new_enabled = config.read().muting_enabled;
                    muting_enabled.store(new_enabled, Ordering::SeqCst);
                    tray.update_state(new_enabled);
                    
                    // If muting was disabled, unmute everything
                    if !new_enabled {
                        if let Some(mut eng) = engine.try_write() {
                            eng.unmute_all();
                        }
                    }
                }
                TrayCommand::Exit => {
                    should_exit.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }

        if should_exit.load(Ordering::Relaxed) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_load_save() {
        let config = Config::default();
        assert!(config.muting_enabled);
    }
}
