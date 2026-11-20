# Changelog

All notable changes to this project are documented in this file.

## [Unreleased]

### Added

- App icons are cached in `icon-cache.bin` beside `config.json`, so an app that is not running keeps its icon in the detected, exclusion and always-muted lists instead of losing it.
- `Edit Config` button in the settings dialog, left of `Save`: saves the dialog and opens `config.json` in the default editor.
- `Rust Background Muter` as the application name, shown in the tray tooltip (`- Active` / `- Disabled`), the settings window title, the login command, the executable's product metadata and the installer.

### Changed

- Installing over a copy published as `Background Muter` updates it in place: the old uninstall registration, shortcuts and login command are removed rather than left beside the new ones.

## [0.1.2] - 2026-05-19

### Added

- NSIS installer built with cargo-packager: per-user install under `%LOCALAPPDATA%\rust-bg-muter`, with an uninstaller that can remove the configuration in `%APPDATA%\rust-bg-muter`. A previous installation is closed and removed silently before anything is written, and the `v*.*.*` tag workflow publishes both the installer and the portable executable.
- Application icon, in the executable's resources, the tray, and the installer and uninstaller.

### Changed

- Settings dialog modernized: a themed Win32 dialog whose caption and control fonts share one face.
- Dialog opens reuse the engine's audio manager instead of rebuilding it, and fonts, brushes, and small process icons are shared process-wide.
- Painting uses the stock `DC_BRUSH`/`DC_PEN`, so no per-item GDI objects are created.
- Small process icons are extracted at small size only and cached per executable path.
- The tray menu shows the muting state as a check mark, and carries a mute-all entry.
- Release builds link with `rust-lld`, target `x86-64-v2`, use a shared target dir, and stamp the commit into the executable.

### Fixed

- Tests no longer write the user's configuration file.

## [0.1.1] - 2026-03-03

### Added

- **Edit Config.json** tray menu item that opens the configuration file in the user's default text editor on Windows (creates file if missing).
- ShellExecute-based helper and associated Windows API feature to launch external editors.

### Changed

- Updated tray menu ordering and command handling to accommodate new action.

## [0.1.0] - 2026-03-03

Initial release.

### Added

- Native Windows tray application for automatic background audio muting.
- Foreground-process-aware muting engine that keeps the active app audible and mutes background apps.
- Excluded apps rule set to keep selected apps always audible.
- Always-muted apps rule set to keep selected apps muted, including when foregrounded.
- Per-process mute state tracking and cleanup for stale/inactive sessions.
- WASAPI-based audio session discovery across default render roles (`eConsole`, `eMultimedia`, `eCommunications`).
- Cached per-process volume controls for low-overhead mute/unmute operations.
- Process name resolution with fallback strategies for better system process coverage.
- Normalization of known Windows system sound processes under `System Sounds`.
- System tray integration with menu actions: toggle muting, open settings, and exit.
- Dynamic tray tooltip and menu label updates reflecting muting state.
- Tray icon double-click shortcut to open the settings dialog.
- Native Win32 settings dialog with a modern common-controls UI.
- Detected audio apps list with manual refresh and text search filtering.
- UI flows to add/remove apps in Excluded and Always Muted lists.
- Configurable poll interval with validation/clamping (`100`-`2000` ms).
- Settings toggles for muting enabled, start minimized, and start with Windows.
- Persistent JSON configuration load/save with sensible defaults.
- Config storage under `%APPDATA%\rust-bg-muter\config.json`.
- Windows startup integration via `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (no admin elevation required).
- Worker thread model with COM initialization for background audio operations.
- Safe shutdown behavior that unmutes sessions previously muted by the app.
- Debug logging support through `env_logger` in debug builds.
- Windows resource embedding for application icon and visual-styles manifest during build.
