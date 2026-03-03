# Changelog

All notable changes to this project are documented in this file.

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
