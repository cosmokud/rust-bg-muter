# Background Muter

![Rust](https://img.shields.io/badge/Rust-1.75+-orange?logo=rust) ![Platform](https://img.shields.io/badge/Platform-Windows-blue?logo=windows) ![License](https://img.shields.io/badge/License-MIT-green)

Background Muter is a native Windows tray application written in Rust that automatically mutes audio from background applications while keeping the active app audible.

![Showcase](https://github.com/user-attachments/assets/ee6017fe-11a3-4727-a5e1-9ac254e502ec)

## Overview

The app runs quietly in the system tray, tracks the current foreground process, and applies muting rules to active audio sessions through WASAPI. It is designed for low overhead and predictable behavior, with settings persisted in a local INI config file.

## Features

- Automatic background muting based on the currently focused application
- Excluded apps list to keep selected apps always audible
- Always-muted apps list to force selected apps muted at all times
- Real-time detection of active audio sessions
- Zero GPU/VRAM usage and sub‑1 % idle CPU cost due to the native Win32 window and GDI rendering
- Native system tray controls:
  - Enable or disable muting
  - Open the settings dialog
  - Edit `config.ini` in the default text editor
  - Exit the application
- Tray icon double-click shortcut to open settings
- Native Win32 settings dialog with:
  - Detected audio apps list
  - Search and filtering for detected apps
  - Add/remove flows for Excluded and Always Muted lists
  - Poll interval configuration (`100`-`2000` ms)
  - Start with Windows toggle
  - Start minimized toggle
- Persistent configuration in `%APPDATA%\rust-bg-muter\config.ini`
- Automatic unmute cleanup for app-muted sessions on exit

## Installation

Download the latest Windows executable from the repository's GitHub Releases tab:

- [GitHub Releases](../../releases)

After downloading, run `bg-muter.exe`. No installer is required.

## Usage

1. Launch `bg-muter.exe`.
2. Right-click the tray icon to open the menu.
3. Select **Enable/Disable Muting** as needed.
4. Open **Settings...** to manage app rules and behavior.

## Configuration

Configuration is stored at `%APPDATA%\rust-bg-muter\config.ini`.

Example:

```ini
[general]
muting_enabled=true
poll_interval_ms=500
start_minimized=false
minimize_to_tray=true
minimize_button_to_tray=true
start_with_windows=false

[excluded_apps]
spotify.exe=1
discord.exe=1

[always_muted_apps]
steam.exe=1
```

## Build from Source

```bash
git clone https://github.com/username/rust-bg-muter.git
cd rust-bg-muter
cargo build --release
```

Binary output:

- `target/release/bg-muter.exe`

## Architecture

```text
src/
├── main.rs            # App entry point and tray event loop
├── audio.rs           # WASAPI session enumeration and per-process mute control
├── muter.rs           # Foreground-aware muting engine and rules
├── process.rs         # Foreground process detection
├── tray.rs            # Native tray menu and message pump integration
├── settings_dialog.rs # Native Win32 settings UI
├── config.rs          # INI config persistence and defaults
└── startup.rs         # Windows startup registry integration
```

## Development

```bash
cargo build
cargo test
cargo fmt
cargo clippy -- -D warnings
```

## License

Licensed under the MIT License. See [LICENSE](LICENSE).
