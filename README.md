# 🔇 Background Muter

A lightweight, ultra-efficient Windows application written in Rust that automatically mutes applications running in the background. Never be interrupted by unexpected sounds from background apps again!

![Rust](https://img.shields.io/badge/Rust-1.75+-orange?logo=rust)
![Windows](https://img.shields.io/badge/Platform-Windows-blue?logo=windows)
![License](https://img.shields.io/badge/License-MIT-green)

## ✨ Features

- **Automatic Background Muting**: Instantly mutes any application that loses focus
- **Smart Detection**: Automatically detects all applications producing audio
- **Exclusion List**: Add apps to a whitelist so they continue playing in the background
- **System Tray Integration**: Runs silently in your system tray
- **Ultra-Low Resource Usage**: ~0% CPU, ~3MB RAM, **no GPU/VRAM usage**
- **Persistent Settings**: Your preferences are saved between sessions (edit `config.json`)
- **Native Windows**: Pure Win32 implementation - no heavy GUI frameworks
- **Fast Response**: Sub-500ms response time to focus changes

## 📊 Resource Usage

| Metric            | Value      |
| ----------------- | ---------- |
| CPU Usage         | ~0% (idle) |
| RAM (Private)     | ~3 MB      |
| RAM (Working Set) | ~15 MB     |
| GPU/VRAM          | **0 MB**   |
| Binary Size       | ~800 KB    |

## 🚀 Installation

### Prerequisites

- Windows 10 or later
- [Rust](https://rustup.rs/) 1.75 or later

### Building from Source

```bash
# Clone the repository
git clone https://github.com/username/rust-bg-muter.git
cd rust-bg-muter

# Build in release mode (recommended)
cargo build --release

# The executable will be at target/release/bg-muter.exe
```

### Running

```bash
# Run directly with cargo
cargo run --release

# Or run the executable
./target/release/bg-muter.exe
```

## 🎯 Usage

### Basic Usage

1. **Launch the application** - It will appear in your system tray
2. **Left-click the tray icon** to open the menu
3. **Toggle muting** from the tray menu
4. **View settings** to see current configuration

### Configuration

Settings are stored in a JSON file at:

- `%APPDATA%\rust-bg-muter\config.json`

Edit this file to configure:

- `excluded_apps`: List of apps to never mute (e.g., `["spotify.exe", "discord.exe"]`)
- `poll_interval_ms`: How often to check for changes (default: 500ms)
- `start_minimized`: Start hidden in tray (default: false)
- `start_with_windows`: Auto-start with Windows (default: false)

Example config:

```json
{
  "excluded_apps": ["spotify.exe", "discord.exe", "vlc.exe"],
  "muting_enabled": true,
  "poll_interval_ms": 500,
  "start_minimized": true,
  "start_with_windows": true
}
```

### System Tray

- **Left-click**: Open the context menu
- **Right-click**: Open the context menu
  - Toggle muting on/off
  - View settings
  - Exit the application

## ⚙️ Configuration

## 🏗️ Architecture

The application is built with a lightweight, modular architecture:

```
src/
├── main.rs       # Application entry point and tray loop
├── lib.rs        # Library exports
├── audio.rs      # Windows Audio Session API (WASAPI) integration
├── config.rs     # Configuration management and persistence
├── muter.rs      # Core muting logic and engine
├── process.rs    # Process detection and foreground tracking
├── startup.rs    # Windows startup registry integration
└── tray.rs       # System tray integration (native Win32)
```

### Key Technologies

- **[tray-icon](https://github.com/tauri-apps/tray-icon)**: Lightweight system tray
- **[windows-rs](https://github.com/microsoft/windows-rs)**: Windows API bindings
- **WASAPI**: Windows Audio Session API for audio control
- **Native Win32**: Message pump and dialogs (no heavy GUI frameworks)

### Design Principles

1. **Minimal Dependencies**: No eframe/egui/tokio - pure Win32 tray app
2. **Event-Driven**: Uses `MsgWaitForMultipleObjectsEx` for efficient waiting
3. **Lazy Refresh**: Audio sessions only refreshed every 2s or on foreground change
4. **Zero GPU**: No OpenGL/DirectX - all rendering via OS

## 🔧 Development

### Project Structure

```bash
rust-bg-muter/
├── Cargo.toml      # Dependencies and metadata
├── build.rs        # Build script for Windows resources
├── assets/         # Icon files
├── src/            # Source code
│   └── ...
└── README.md       # This file
```

### Building for Development

```bash
# Debug build (faster compilation)
cargo build

# Run with logging
RUST_LOG=debug cargo run

# Run tests
cargo test
```

### Code Quality

```bash
# Format code
cargo fmt

# Run clippy lints
cargo clippy -- -D warnings

# Check for issues
cargo check
```

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## 📝 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- The Rust community for excellent crates
- Microsoft for the Windows Audio Session API
- The tray-icon project for lightweight tray integration

## 📬 Support

If you encounter any issues or have questions:

1. Check the [Issues](https://github.com/username/rust-bg-muter/issues) page
2. Open a new issue with detailed information about your problem

---

Made with ❤️ and Rust
