# Changelog

All notable changes to this project are documented in this file.

## [0.1.2] - 2026-11-23

### Added

- NSIS installer built with cargo-packager: per-user install under `%LOCALAPPDATA%\rust-bg-muter`, with an uninstaller that can remove the configuration in `%APPDATA%\rust-bg-muter`. A previous installation is closed and removed silently before anything is written, and the `v*.*.*` tag workflow publishes both the installer and the portable executable.
- Application icon, in the executable's resources, the tray, and the installer and uninstaller. One SVG design (`assets/icon.svg`) rendered by `scripts/gen-icons.py`, with a bitmap per notification-area, taskbar and Alt+Tab size instead of one stretched image.
- `Edit Config` button in the settings dialog, left of `Save`: saves the dialog and opens `config.json` in the default editor; the file's path is no longer shown in the dialog.
- `Reset Mixer` button that redoes the Windows Volume Mixer reset, leaving the speaker level and mute alone.
- Poll interval up/down steppers below the toggles.
- Detected app icons, and icons cached in `icon-cache.bin` beside `config.json`, so an app that is not running keeps its icon in the detected, exclusion and always-muted lists instead of losing it.
- Double-click a row to move it between the detected, excluded and always-muted panels, and a filter edit per rule list.
- `Rust Background Muter` as the application name, shown in the tray tooltip (`- Active` / `- Disabled`), the settings window title, the login command, the executable's product metadata and the installer.

### Changed

- Settings dialog modernized: a themed Win32 dialog whose caption and control fonts share one face, ownerdraw controls, a fully rounded antialiased toggle pill, and scroll lists that take the wheel and a dragged scrollbar, all drawn in one window and one paint pass per frame.
- Dialog opens reuse the engine's audio manager instead of rebuilding it, and fonts, brushes, and small process icons are shared process-wide.
- Painting uses the stock `DC_BRUSH`/`DC_PEN`, so no per-item GDI objects are created.
- Small process icons are extracted at small size only and cached per executable path.
- The tray menu shows the muting state as a check mark, and carries a mute-all entry.
- The audio engine walks each endpoint once per pass, keeps one decision table over a mixer snapshot, one lookup ladder per process, and shares the rule sets instead of copying them.
- Installing over a copy published as `Background Muter` updates it in place: the old uninstall registration, shortcuts and login command are removed rather than left beside the new ones; the login value is now `RustBackgroundMuter`.
- Release builds link with `rust-lld`, target `x86-64-v2`, use a shared target dir, and stamp the commit into the executable.
- The `Start Minimized to Tray` toggle is gone; the window state is remembered instead.

### Fixed

- Backspace, Delete and every text edit in a settings field crashed the whole app: the delete helpers re-entered the field store they were already holding (`RefCell already borrowed`), which aborts the process in release builds. Field editing now runs once, in place.
- Field keys a native edit box handles now behave: Ctrl+Backspace/Ctrl+Delete take a word, Ctrl+Left/Ctrl+Right jump by word, Shift+Delete clears the selection without touching the clipboard, Ctrl+Insert copies and Shift+Insert pastes, double click selects a word, and Escape and the scroll keys no longer reach `DefWindowProc` while a field has focus.
- Resizing and repainting the dialog left ghosting and artifact trails; cards no longer sit above their own controls and swallow clicks and focus.
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
