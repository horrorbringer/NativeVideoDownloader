# Native Video Downloader

A blazing-fast, lightweight, cross-platform native desktop media and video downloader built with **Rust** and **Slint GUI**. Engineered for high throughput, minimal memory footprint, and a seamless native user experience on **macOS**, **Windows**, and **Linux**.

---

## Features

- **Blazing Fast & Ultra-Lightweight**: Native Rust architecture with asynchronous Tokio runtime and hardware-accelerated Slint UI. No Electron, Chromium, or web-view overhead.
- **Universal Stream & Video Support**:
  - Direct HTTP/HTTPS media streams with chunked resumable downloads.
  - HLS (`.m3u8`) streaming playlists and video segments.
  - Video platform extraction via `yt-dlp` integration (YouTube, Bilibili, Dailymotion, Vimeo, etc.).
  - Automatic MacCMS & web-scraper fallback for drama and anime series portals (e.g. DonghuaFun).
- **Episode & Series Selector**:
  - Full series/playlist detection with interactive checklist.
  - Range syntax support (`1-10`, `1, 3, 5`, `40-`, `-5`).
  - Batch **All** / **None** quick actions and selective downloading.
- **Format & Quality Flexibility**:
  - Choose between Best Quality, 1080p FHD, 720p HD, 480p SD, or extract Audio Only (MP3).
  - Subtitle detection, extraction, and embedding (`.srt` / embedded soft subs).
- **Advanced Download Management**:
  - Pause, resume, retry with exponential backoff, and cancel controls.
  - Speed limits / bandwidth throttling (e.g., 500 KB/s, 2 MB/s, 10 MB/s, Unlimited).
  - Configurable worker concurrency (1–10 parallel downloads).
  - Batch download queue (paste multi-line URLs).
- **Native OS Integration**:
  - **macOS**: Native pill buttons, AppleScript system chime notifications, `open -R` in Finder, `pbcopy`/`pbpaste`.
  - **Windows**: Windows 10/11 PowerShell Toast Notifications, Explorer reveal (`/select`), `clip` / PowerShell clipboard.
  - **Linux**: Desktop notifications via `notify-send`, X11 (`xclip`) & Wayland (`wl-clipboard`), `xdg-open` file manager.
- **Local Persistence & Diagnostics**:
  - Embedded SQLite database (`~/.native_video_downloader/downloads.db`) tracks download history and file locations.
  - Real-time in-app logging console with filtering by log level (`Info`, `Warn`, `Error`) and search.

---

## Operating System Compatibility & Comparison

| Capability | macOS | Windows 10 / 11 | Linux (X11 & Wayland) |
|---|---|---|---|
| **GUI Backend** | Metal / Cocoa | DirectX / Direct2D / OpenGL | Wayland / X11 (OpenGL / Vulkan) |
| **System Notifications** | Native AppleScript + Chime | PowerShell Windows Toast | `notify-send` (libnotify) |
| **Clipboard Support** | `pbcopy` / `pbpaste` | `clip` / PowerShell `Get-Clipboard` | `wl-clipboard` (Wayland) / `xclip` (X11) |
| **Reveal in File Manager** | `open -R <path>` (Finder) | `explorer.exe /select,<path>` | `xdg-open <dir>` |
| **Default Media Player** | `open <file>` | `cmd /C start "" <file>` | `xdg-open <file>` |
| **yt-dlp Auto-Install** | `yt-dlp_macos` | `yt-dlp.exe` | `yt-dlp_linux` |
| **Packaging Target** | `.app` Bundle / DMG | `.exe` / MSI / Portable Zip | Standalone ELF / AppImage / `.deb` |

---

## Development Requirements by OS

### 1. General Prerequisite (All Platforms)
- **Rust Toolchain**: Rust **1.85.0+** (supporting Edition 2024).
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  rustup update stable
  ```

---

### 2. macOS Setup

1. **Install Command Line Tools**:
   ```bash
   xcode-select --install
   ```
2. **Install FFmpeg** *(Recommended)*:
   ```bash
   brew install ffmpeg
   ```
3. **Run in Development**:
   ```bash
   cargo run
   ```
4. **Package Application Bundle**:
   ```bash
   ./scripts/build_app.sh
   # Outputs: dist/Native Video Downloader.app
   ```

---

### 3. Windows Setup (Windows 10 / 11)

1. **Install Visual Studio C++ Build Tools**:
   - Install **Visual Studio Community** or **Build Tools for Visual Studio** with the **"Desktop development with C++"** workload.
   - Alternatively via `winget`:
     ```powershell
     winget install Microsoft.VisualStudio.2022.BuildTools --override "--passive --add Microsoft.VisualStudio.Workload.VCTools"
     ```
2. **Install FFmpeg**:
   ```powershell
   winget install Gyan.FFmpeg
   # Or download from https://ffmpeg.org and add the bin/ folder to your System PATH
   ```
3. **Run in Development**:
   ```powershell
   cargo run
   ```
4. **Build Release Executable**:
   ```powershell
   cargo build --release
   # Output binary located at:
   # .\target\release\native_video_downloader.exe
   ```
   *Note: On Windows, `yt-dlp.exe` is automatically downloaded into `%USERPROFILE%\.native_video_downloader\bin\yt-dlp.exe` if not found in PATH.*

---

### 4. Linux Setup (Ubuntu / Debian / Fedora / Arch)

1. **Install Required Build Libraries & Dependencies**:

   - **Ubuntu / Debian / Linux Mint**:
     ```bash
     sudo apt update
     sudo apt install -y build-essential cmake pkg-config \
       libfontconfig1-dev libfreetype6-dev libx11-dev libxext-dev \
       libxkbcommon-dev libxkbcommon-x11-dev ffmpeg libnotify-bin
     
     # For clipboard support (install whichever your desktop session uses):
     sudo apt install -y wl-clipboard # Wayland
     sudo apt install -y xclip        # X11
     ```

   - **Fedora / RHEL**:
     ```bash
     sudo dnf install -y gcc gcc-c++ cmake pkgconfig \
       fontconfig-devel freetype-devel libX11-devel libXext-devel \
       libxkbcommon-devel libxkbcommon-x11-devel ffmpeg libnotify \
       wl-clipboard xclip
     ```

   - **Arch Linux / Manjaro**:
     ```bash
     sudo pacman -S --needed base-devel cmake pkgconf \
       fontconfig freetype2 libx11 libxext libxkbcommon libxkbcommon-x11 \
       ffmpeg libnotify wl-clipboard xclip
     ```

2. **Run in Development**:
   ```bash
   cargo run
   ```

3. **Build Release Binary**:
   ```bash
   cargo build --release
   # Output binary located at:
   # ./target/release/native_video_downloader
   ```

4. **Linux Desktop Integration (`.desktop` launcher)**:
   You can install the binary and create a desktop entry:
   ```bash
   # 1. Copy binary
   sudo cp target/release/native_video_downloader /usr/local/bin/

   # 2. Copy icon
   sudo mkdir -p /usr/local/share/icons/hicolor/512x512/apps/
   sudo cp assets/app_icon.png /usr/local/share/icons/hicolor/512x512/apps/native-video-downloader.png

   # 3. Create desktop launcher entry
   cat << 'EOF' > ~/.local/share/applications/native-video-downloader.desktop
   [Desktop Entry]
   Name=Native Video Downloader
   Comment=Fast, native desktop video and media stream downloader
   Exec=/usr/local/bin/native_video_downloader
   Icon=native-video-downloader
   Terminal=false
   Type=Application
   Categories=AudioVideo;Video;Network;
   StartupNotify=true
   EOF
   ```

---

## Project Structure

```
NativeVideoDownloader/
├── Cargo.toml                  # Rust dependencies & build configuration
├── build.rs                    # Slint UI compiler build script
├── assets/                     # Application icons and branding assets
├── dist/                       # Output folder for packaged application bundles (.app)
├── scripts/
│   ├── build_app.sh            # Release build & macOS .app bundle packager
│   └── make_transparent_icon.swift # Swift helper for icon alpha-mask generation
├── src/
│   ├── main.rs                 # Application entry point, Slint UI bindings & event loop
│   ├── database/               # SQLite database setup, queries, and history management
│   ├── downloader/
│   │   ├── extractor.rs        # URL analysis, yt-dlp, HTML scraping, playlist & episode parser
│   │   ├── manager.rs          # Download queue manager, concurrency limiter, speed throttler
│   │   ├── progress.rs         # Real-time ETA, transfer speeds, and progress calculations
│   │   └── retry.rs            # Network retry policies and error recovery
│   ├── filesystem/             # File storage paths, unique naming, cross-platform clipboard
│   ├── logger/                 # Tracing subscriber feeding live UI log records
│   ├── models/                 # Shared data structures (DownloadItem, VideoMetadata, etc.)
│   ├── network/                # HTTP client configuration, headers, range requests
│   └── notifications.rs        # Cross-platform desktop notifications (macOS / Windows / Linux)
└── ui/
    ├── app.slint               # Main Slint application window and sidebar routing
    ├── models.slint            # Slint data structures exported to Rust
    ├── components/             # Reusable UI widgets (buttons, chips, nav items)
    └── tabs/
        ├── home_tab.slint      # Media URL analysis, episode checklist, format selection
        ├── downloads_tab.slint # Active downloads queue, pause/resume, progress bars
        ├── history_tab.slint   # Completed records, search, open file/folder
        ├── logs_tab.slint      # Engine log console with live search & filters
        └── settings_tab.slint  # Download folder, concurrency, speed limits, notifications
```

---

## Testing & Quality Assurance

Run the test suite across all platforms:
```bash
cargo test
```

All unit tests (including episode range parsers, filename sanitization, retry backoff algorithms, and HTML scraping) run natively on all platforms without external network calls.

---

## Configuration & Storage Paths

All persistent data is stored in the user's home directory across operating systems:
- **macOS / Linux**: `~/.native_video_downloader/`
- **Windows**: `%USERPROFILE%\.native_video_downloader\`

Files:
- **Database**: `downloads.db` (embedded SQLite)
- **Binaries**: `bin/yt-dlp` or `bin/yt-dlp.exe` (auto-provisioned on demand)
- **Default Downloads**: User's `Downloads` folder (customizable in **Settings**)

---

## Releasing & Packaging

For complete instructions on building standalone `.dmg`, `.exe`, `.AppImage`, `.deb`, and automated multi-platform CI/CD releases, see the **[Cross-Platform Release Guide](RELEASE.md)**.

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.
