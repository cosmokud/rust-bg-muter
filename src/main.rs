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

    // Main application loop (always run under GUI event loop so tray is responsive)
    run_with_gui(
        config.clone(),
        engine.clone(),
        should_exit.clone(),
        !start_minimized,
    )?;

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
    start_visible: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = create_native_options(start_visible);
    
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_load_save() {
        let config = Config::default();
        assert!(config.muting_enabled);
    }
}
