//! Rust Background Muter Library
//!
//! A lightweight crate for detecting and muting background
//! applications on Windows with minimal resource usage.

pub mod audio;
pub mod config;
pub mod confirm_dialog;
pub mod field;
pub mod icon_cache;
pub mod mixer_reset;
pub mod muter;
pub mod od_controls;
pub mod process;
pub mod settings_dialog;
pub mod shell;
pub mod startup;
pub mod theme;
pub mod tray;

pub use audio::AudioManager;
pub use config::Config;
pub use muter::MuterEngine;
pub use process::get_foreground_pid;
