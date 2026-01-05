# 🔇 Background Muter

A high-performance Windows application written in Rust that automatically mutes applications running in the background. Never be interrupted by unexpected sounds from background apps again!

![Rust](https://img.shields.io/badge/Rust-1.75+-orange?logo=rust)
![Windows](https://img.shields.io/badge/Platform-Windows-blue?logo=windows)
![License](https://img.shields.io/badge/License-MIT-green)

## ✨ Features

- **Automatic Background Muting**: Instantly mutes any application that loses focus
- **Smart Detection**: Automatically detects all applications producing audio
- **Exclusion List**: Add apps to a whitelist so they continue playing in the background
- **System Tray Integration**: Runs silently in your system tray
- **Low Resource Usage**: Minimal CPU and memory footprint
- **Beautiful GUI**: Modern, responsive interface built with egui
- **Persistent Settings**: Your preferences are saved between sessions
- **Fast Response**: Sub-100ms response time to focus changes

## 📸 Screenshots

The application features a clean, modern dark-themed interface:

- **Detected Apps List**: See all applications currently producing audio
- **Exclusion List**: Manage which apps should never be muted
- **Toggle Control**: Enable/disable muting with one click
- **System Tray**: Access core features without opening the main window

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

# The executable will be at target/release/rust-bg-muter.exe
```

### Running

```bash
# Run directly with cargo
cargo run --release

# Or run the executable
./target/release/rust-bg-muter.exe
```

## 🎯 Usage

### Basic Usage

1. **Launch the application** - It will appear in your system tray
2. **Double-click the tray icon** to open the main window
3. **Play some audio** in different applications - they'll appear in the detected list
4. **Add exclusions** by clicking the "Exclude" button next to any app
5. **Toggle muting** using the status button in the header

### Exclusion List

Apps in the exclusion list will **never be muted**, even when running in the background. This is useful for:

- Music players (Spotify, VLC, etc.)
- Communication apps (Discord, Teams, etc.)
- Notification sounds
- Any app you want to hear regardless of focus

### Adding Exclusions

**Method 1: From Detected Apps**

- Click the "➕ Exclude" button next to any detected app

**Method 2: Manual Entry**

- Type the executable name (e.g., `spotify.exe`) in the manual input field
- Press Enter or click "Add"

### System Tray

- **Double-click**: Open the main window
- **Right-click**: Access the context menu
  - Toggle muting on/off
  - Exit the application

### Tray Icon Colors

- 🟢 **Green**: Muting is active
- 🔴 **Red**: Muting is disabled

## ⚙️ Configuration

Settings are automatically saved to:

```
%APPDATA%/rust-bg-muter/config.json
```

### Available Settings

| Setting            | Default | Description                               |
| ------------------ | ------- | ----------------------------------------- |
| `muting_enabled`   | `true`  | Whether background muting is active       |
| `poll_interval_ms` | `100`   | How often to check for focus changes (ms) |
| `start_minimized`  | `false` | Start directly to system tray             |
| `excluded_apps`    | `[]`    | List of apps that won't be muted          |

## 🏗️ Architecture

The application is built with a modular architecture:

```
src/
├── main.rs       # Application entry point and orchestration
├── lib.rs        # Library exports
├── audio.rs      # Windows Audio Session API (WASAPI) integration
├── config.rs     # Configuration management and persistence
├── gui.rs        # egui-based graphical interface
├── muter.rs      # Core muting logic and engine
├── process.rs    # Process detection and foreground tracking
└── tray.rs       # System tray integration
```

### Key Technologies

- **[egui](https://github.com/emilk/egui)**: Immediate mode GUI framework
- **[eframe](https://github.com/emilk/egui/tree/master/crates/eframe)**: Native application framework
- **[tray-icon](https://github.com/tauri-apps/tray-icon)**: Cross-platform system tray
- **[windows-rs](https://github.com/microsoft/windows-rs)**: Windows API bindings
- **WASAPI**: Windows Audio Session API for audio control

## 🔧 Development

### Project Structure

```bash
rust-bg-muter/
├── Cargo.toml      # Dependencies and metadata
├── build.rs        # Build script for Windows resources
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
- The egui project for the beautiful GUI framework

## 📬 Support

If you encounter any issues or have questions:

1. Check the [Issues](https://github.com/username/rust-bg-muter/issues) page
2. Open a new issue with detailed information about your problem

---

Made with ❤️ and Rust
