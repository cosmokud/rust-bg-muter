//! Background Muter - A Windows application to automatically mute background applications
//!
//! This application runs in the system tray and automatically mutes any applications
//! that are producing audio while in the background. Users can add apps to an
//! exclusion list to prevent them from being muted.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod gui;
mod muter;
mod process;
mod startup;
mod tray;

use config::Config;
use gui::{create_native_options, BackgroundMuterApp};
use muter::MuterEngine;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tray::{SystemTray, TrayEvent};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    log::info!("Background Muter starting...");

    // Initialize COM for the main thread (apartment threaded for GUI)
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    // Load configuration
    let config = Arc::new(RwLock::new(Config::load()));
    log::info!("Configuration loaded");

    // Best-effort: ensure startup registry matches config
    {
        let start_with_windows = config.read().start_with_windows;
        if let Err(e) = startup::apply_startup_setting(start_with_windows) {
            log::warn!("Failed to apply startup setting: {}", e);
        }
    }

    // Create the muting engine
    let engine = Arc::new(RwLock::new(
        MuterEngine::new(config.clone()).expect("Failed to create muter engine"),
    ));
    log::info!("Muter engine initialized");

    // Create shared state
    let should_exit = Arc::new(AtomicBool::new(false));
    let start_minimized = config.read().start_minimized;

    // Start the background muting thread
    let muting_config = config.clone();
    let muting_engine = engine.clone();
    let muting_should_exit = should_exit.clone();
    
    let muting_thread = thread::spawn(move || {
        // Initialize COM for this thread
        unsafe {
            let _ = CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }

        log::info!("Background muting thread started");

        while !muting_should_exit.load(Ordering::Relaxed) {
            let poll_interval = muting_config.read().poll_interval_ms;

            if let Some(mut engine) = muting_engine.try_write() {
                if let Err(e) = engine.update() {
                    log::error!("Error updating muter engine: {}", e);
                }
            }

            thread::sleep(Duration::from_millis(poll_interval));
        }

        log::info!("Background muting thread stopped");
    });

    // Main application loop
    if start_minimized {
        run_tray_only(config.clone(), engine.clone(), should_exit.clone())?;
    } else {
        run_with_gui(config.clone(), engine.clone(), should_exit.clone())?;
    }

    // Signal background thread to stop
    should_exit.store(true, Ordering::Relaxed);

    // Wait for muting thread to finish
    let _ = muting_thread.join();

    // Unmute all apps before exiting
    if let Some(mut engine) = engine.try_write() {
        engine.unmute_all();
    }

    log::info!("Background Muter shutdown complete");
    Ok(())
}

/// Runs the application with the GUI visible
fn run_with_gui(
    config: Arc<RwLock<Config>>,
    engine: Arc<RwLock<MuterEngine>>,
    should_exit: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = create_native_options();
    
    eframe::run_native(
        "Background Muter",
        options,
        Box::new(move |cc| {
            Ok(Box::new(BackgroundMuterApp::new(cc, config.clone(), engine.clone())))
        }),
    )?;

    should_exit.store(true, Ordering::Relaxed);
    Ok(())
}

/// Runs the application in tray-only mode
fn run_tray_only(
    config: Arc<RwLock<Config>>,
    engine: Arc<RwLock<MuterEngine>>,
    should_exit: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    log::info!("Running in tray-only mode");

    // Initialize system tray on main thread
    let mut tray = SystemTray::new()?;
    tray.initialize(config.read().muting_enabled)?;
    log::info!("System tray initialized");

    let mut last_muting_enabled = config.read().muting_enabled;

    while !should_exit.load(Ordering::Relaxed) {
        for event in tray.process_events() {
            match event {
                TrayEvent::OpenWindow | TrayEvent::SingleClick => {
                    // Open the GUI window - we need to drop tray first
                    drop(tray);
                    return run_with_gui(config, engine, should_exit);
                }
                TrayEvent::ToggleMuting => {
                    let enabled = config.write().toggle_muting();
                    let _ = tray.update_icon(enabled);
                    last_muting_enabled = enabled;

                    if !enabled {
                        if let Some(mut eng) = engine.try_write() {
                            eng.unmute_all();
                        }
                    }

                    log::info!("Muting toggled: {}", enabled);
                }
                TrayEvent::Exit => {
                    should_exit.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }

        // Update tray icon only if state changed (avoids flicker/unresponsive tray)
        let muting_enabled = config.read().muting_enabled;
        if muting_enabled != last_muting_enabled {
            let _ = tray.update_icon(muting_enabled);
            last_muting_enabled = muting_enabled;
        }

        thread::sleep(Duration::from_millis(50));
    }

    Ok(())
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
