<p align="center">
  <img src="assets/icon.png" width="112" height="112" alt="Rust Background Muter Windows system tray icon" />
</p>

<h1 align="center">Rust Background Muter — Auto-Mute Background Apps on Windows</h1>

<p align="center">
  <b>Only the active window plays audio. Everything else goes quiet.</b>
</p>

<p align="center">
  A lightweight Windows 10 &amp; 11 system tray utility that automatically mutes background<br/>
  and unfocused applications while keeping your foreground app audible — native WASAPI, no virtual audio drivers, zero GPU usage, and sub-1% idle CPU.
</p>

<p align="center">
  <a href="https://github.com/cosmokud/rust-bg-muter/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/cosmokud/rust-bg-muter?style=for-the-badge&color=a32b8d" /></a>
  <img alt="Rust" src="https://img.shields.io/badge/Rust-1.99%2B-orange?style=for-the-badge&logo=rust" />
  <img alt="Windows 10 and 11" src="https://img.shields.io/badge/Platform-Windows%2010%2F11-blue?style=for-the-badge&logo=windows" />
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-111111?style=for-the-badge" /></a>
</p>

<p align="center">
  <a href="#installation">Install</a> ·
  <a href="#features">Features</a> ·
  <a href="#usage">Usage</a> ·
  <a href="#configuration">Configuration</a> ·
  <a href="#faq">FAQ</a> ·
  <a href="#build-from-source">Build</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#development">Develop</a> ·
  <a href="CONTRIBUTING.md">Contribute</a>
</p>

---

## Overview

Rust Background Muter runs quietly in the Windows system tray, tracks the currently focused foreground window, and automatically mutes audio from unfocused background applications via the native Windows Audio Session API (WASAPI).

Whether you are Alt-Tabbing out of a game, switching between browser tabs and video players, or keeping distractions silent while working, it manages per-application audio cleanly with minimal overhead. All preferences are saved in a simple local JSON configuration file—no background services, no Electron bloat, and no virtual audio drivers required.

<p align="center">
  <img src="docs/images/settings.jpg" width="720" alt="Rust Background Muter Windows settings window showing detected audio apps, excluded apps allowlist, and always-muted apps list" />
</p>

## Features

- **Automatic background muting:** Keeps only the active foreground application audible and mutes unfocused windows in real time
- **Excluded apps list (allowlist):** Keep selected apps like Spotify, Discord, or voice chat always audible in the background
- **Always-muted apps list:** Force specific applications to stay muted at all times, even when focused
- **Real-time audio session detection:** Automatically detects active Windows audio sessions as apps open and close
- **Ultra-lightweight native Win32 UI:** Zero GPU/VRAM usage and sub-1% idle CPU cost using native Win32 windows and GDI rendering
- **Native system tray controls:**
  - Toggle automatic muting on or off
  - Open the settings dialog (or double-click the tray icon)
  - Exit the application
- **Built-in settings dialog:**
  - Live list of detected audio applications
  - Instant search and filtering for detected apps
  - Quick add/remove controls for **Excluded** and **Always Muted** lists
  - Double-click any row to move an app between detected, excluded, and always-muted lists
  - Configurable poll interval (`100`–`2000` ms)
  - **Start with Windows** toggle
  - **Reset Mixer** button to reset Windows Volume Mixer per-app volume memory
  - **Edit Config** shortcut to open `config.json` in your default editor
- **Persistent local configuration:** Stored at `%APPDATA%\rust-bg-muter\config.json`
- **Safe automatic unmute on exit:** Restores any app-muted audio sessions when the program closes

## Installation

Download the latest Windows 10 / Windows 11 build from the repository's Releases page:

- [Download from GitHub Releases](../../releases)

Every release includes two options:

- `rust-bg-muter_x.y.z_x64-setup.exe` — Standard Windows installer that installs to `%LOCALAPPDATA%\rust-bg-muter`, adds Start menu and desktop shortcuts, and registers an uninstaller. **No administrator rights required.**
- `rust-bg-muter.exe` — Portable standalone executable. Copy it anywhere and run it directly.

Both versions share the same configuration file at `%APPDATA%\rust-bg-muter\config.json`.

## Usage

1. Launch `rust-bg-muter.exe` (or open **Rust Background Muter** from the Start menu).
2. Right-click the icon in the Windows system tray to open the menu.
3. Select **Enable/Disable Muting** to toggle automatic background muting.
4. Open **Settings...** (or double-click the tray icon) to manage excluded apps, always-muted apps, and startup behavior.

## Configuration

Configuration is stored locally at `%APPDATA%\rust-bg-muter\config.json`.

Example:

```json
{
  "excluded_apps": ["spotify.exe", "discord.exe"],
  "always_muted_apps": ["steam.exe"],
  "muting_enabled": true,
  "poll_interval_ms": 500,
  "minimize_to_tray": true,
  "minimize_button_to_tray": true,
  "start_with_windows": false,
  "window_state": null
}
```

## FAQ

### How do I automatically mute games when I Alt-Tab on Windows 10 or 11?

Run Rust Background Muter in the system tray with muting enabled. As soon as you Alt-Tab away from a game to another window, the game's audio session is automatically muted—and unmuted the moment you switch back.

### Can I keep Spotify, Discord, or my browser playing while other background apps are muted?

Yes. Open **Settings** and add `spotify.exe`, `discord.exe`, or any other executable to the **Excluded** list. Excluded apps remain audible even when they are in the background.

### Does this require virtual audio drivers or administrator privileges?

No. Rust Background Muter uses Windows' built-in WASAPI audio session controls directly and installs per-user under `%LOCALAPPDATA%`, so no virtual audio cables, kernel drivers, or admin rights are needed.

### How is this different from the Windows Volume Mixer or EarTrumpet?

Windows Volume Mixer and EarTrumpet are designed for _manual_ per-app volume control. Rust Background Muter _automates_ muting based on window focus, so you do not have to manually mute and unmute apps every time you switch windows.

### Will my apps stay muted if I close Rust Background Muter?

No. On exit, Rust Background Muter automatically unmutes every audio session it muted so your apps return to normal immediately.

## Build from Source

```bash
git clone https://github.com/cosmokud/rust-bg-muter.git
cd rust-bg-muter
cargo build --release
```

Binary output:

- `target/release/rust-bg-muter.exe`

Windows installer (NSIS, built with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager); its template is `packaging/nsis/installer.nsi`):

```powershell
.\build-installers.ps1
```

Output: `target/packager/rust-bg-muter_<version>_x64-setup.exe`.

## Architecture

```text
src/
├── main.rs            # App entry point and tray event loop
├── audio.rs           # WASAPI session enumeration and per-process mute control
├── muter.rs           # Foreground-aware muting engine and rules
├── process.rs         # Foreground process detection
├── tray.rs            # Native tray menu and message pump integration
├── settings_dialog.rs # Native Win32 settings UI: layout, painting, input
├── field.rs           # Single-line text field editing rules
├── od_controls.rs     # Ownerdraw paint helpers (toggles, buttons, cards)
├── theme.rs           # Dialog theming, fonts, brushes, icons
├── icon_cache.rs      # Per-executable icon extraction and on-disk cache
├── mixer_reset.rs     # Windows Volume Mixer reset
├── confirm_dialog.rs  # Native confirmation prompt
├── shell.rs           # ShellExecute helper (open config in an editor)
├── config.rs          # JSON config persistence and defaults
└── startup.rs         # Windows startup registry integration
```

## Development

`rust-toolchain.toml` pins the toolchain, so `cargo`, `rustfmt`, and `clippy` behave identically on your machine and in CI.

```bash
cargo build
cargo test
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup, the pre-commit tip, and what the checks do not cover.

## Privacy

Rust Background Muter is local-first and contains no network code at all: no telemetry, no analytics, no crash reporting, and no update checker. It only reads which process owns the foreground window and which audio sessions are playing, and mutes by setting each session's own mute flag. Settings and the icon cache live under `%APPDATA%\rust-bg-muter`, and the only registry writes are the app's own startup entry and the per-app volume memory cleared by `Reset Mixer`—both under `HKEY_CURRENT_USER` and both only when you request them. See [PRIVACY.md](PRIVACY.md) for full details.

## Security

Please report vulnerabilities privately, not in a public issue. See [SECURITY.md](SECURITY.md).

## License

Licensed under the MIT License. See [LICENSE](LICENSE).
