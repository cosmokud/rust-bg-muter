# Privacy Policy for Rust Background Muter

**In short:** this app collects nothing and sends nothing about you. It has no accounts, no telemetry, no crash reporting, no ads, and no network code at all. Everything it reads stays on your PC, and everything it keeps is listed below.

## What the app touches, and why

To decide what to mute, the app asks Windows two questions, on a timer you set in the settings dialog (`500` ms by default):

- **Which process owns the foreground window.** The window handle comes from `GetForegroundWindow`, and its process id from `GetWindowThreadProcessId`. It is used for one comparison and never stored.
- **Which audio sessions are playing.** Windows Audio Session API (`IAudioSessionManager2`) reports the process id, process name and display name of every session on the default render device, plus whether the session is muted. These are held in memory for the life of the process, so a window can show the list.

Muting itself is `ISimpleAudioVolume::SetMute` on the session that belongs to a process the rules name. It changes that application's own volume, nothing else on the system.

## What it writes, and where

Everything lives under `%APPDATA%\rust-bg-muter`, in your own user profile:

- `config.json` — the exclusion list, the always-muted list, `muting_enabled`, `poll_interval_ms`, the tray toggles, `start_with_windows`, and the last dialog position and size. Nothing in it is sent anywhere; `Edit Config` opens it in your editor.
- `icon-cache.bin` — one small PNG per application whose icon the dialog has already drawn, keyed by executable name so a list row still has an icon after the app has closed. Only the drawing is kept, never the path to an executable: a cached icon must not make the app touch a file it was not asked about.

Two registry writes, both under `HKEY_CURRENT_USER`, both only when you ask for them:

- `Software\Microsoft\Windows\CurrentVersion\Run` — the app's own startup entry, written when `Start with Windows` is switched on and deleted when it is switched off.
- `Software\Microsoft\Internet Explorer\LowRegistry\Audio\PolicyConfig` — the per-app volume memory `Reset Mixer` clears. Deleting it is exactly what the Windows Volume Mixer's reset does to your own preferences, and the next audio stream gets a fresh session. The device-level defaults under `HKLM\...\MMDevices` are deliberately left alone.

## What it reads besides that

To put a real icon beside a name rather than a placeholder, the app asks Windows for the icon of an executable path it already knows from the audio sessions: `PrivateExtractIconsW` on that file, at 16×16. It opens no document, reads no file contents, and lists no directories.

## What the app does not do

- No telemetry, analytics, usage statistics, or crash reporting.
- No accounts, sign-in, or cloud sync.
- No network access: there is no update checker, no HTTP client in the dependency tree, and nothing to switch off.
- No elevation. The installer is per-user and needs no administrator rights, and every registry write above is to `HKEY_CURRENT_USER`.
- No audio is recorded, buffered, or transmitted. The app reads and sets each session's mute flag; the audio itself never passes through it.

## Uninstalling

The installer's uninstaller removes the app and, when asked, the configuration under `%APPDATA%\rust-bg-muter`. To remove everything by hand: delete that folder, delete the `RustBackgroundMuter` value from the `Run` key above, and delete `%LOCALAPPDATA%\rust-bg-muter` where the installer put the program.

## Changes

If this policy changes, the change lands here in the repository, in the same commit as the code that makes it necessary.

## Contact

Open an issue at https://github.com/cosmokud/rust-bg-muter/issues. For a security problem, see [SECURITY.md](SECURITY.md) instead of opening a public issue.
