//! Background Muter Library
//!
//! A lightweight crate for detecting and muting background
//! applications on Windows with minimal resource usage.

pub mod audio;
pub mod config;
pub mod muter;
pub mod process;
pub mod startup;
pub mod tray;

pub use audio::AudioManager;
pub use config::Config;
pub use muter::MuterEngine;
pub use process::{get_foreground_pid, ProcessInfo};
