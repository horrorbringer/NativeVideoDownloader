# Docker Development Infrastructure & Guide

This directory contains a complete, multi-option Docker setup for developing, testing, and building **Native Video Downloader** without needing to manually install Rust, Visual Studio C++ Build Tools, FFmpeg, yt-dlp, or system graphics libraries on your host operating system.

---

## 1. Why Use Docker for This Project?

| Feature | Host Machine (Native) | Docker Environment |
|---|---|---|
| **Setup Time** | High (Rust 1.85+, MSVC C++ tools, FFmpeg, yt-dlp, Slint libs) | **Zero host dependencies** (only Docker required) |
| **Reproducibility** | Depends on local PATH & compiler versions | **100% identical across Windows, macOS, and Linux** |
| **Compilation Cache** | Shared locally | **Preserved across sessions via Docker named volumes** |
| **GUI Accessibility** | Native window | **Web browser (noVNC at `http://localhost:6080`) or host X11** |
| **Clean Teardown** | Leftover builds & registry files | `docker compose down` removes everything cleanly |

---

## 2. Quick Start Cheat Sheet

Using the provided helper scripts:

### Windows (PowerShell):
```powershell
# Option 1: Run GUI app in your web browser (Recommended)
.\docker-dev.ps1 gui

# Option 2: Run GUI with auto-recompile on code save (cargo-watch)
.\docker-dev.ps1 watch

# Option 3: Open an interactive bash terminal inside the container
.\docker-dev.ps1 cli

# Option 4: Run all unit and integration tests (65+ tests)
.\docker-dev.ps1 test

# Option 5: Compile an optimized Linux release binary
.\docker-dev.ps1 build

# Stop containers
.\docker-dev.ps1 down
```

### macOS / Linux (Bash):
```bash
chmod +x docker-dev.sh
./docker-dev.sh gui
./docker-dev.sh watch
./docker-dev.sh cli
./docker-dev.sh test
./docker-dev.sh build
./docker-dev.sh down
```

### Direct `docker compose` Commands:
```bash
# Start GUI container
docker compose up app-gui

# Start with live hot-reloading
docker compose up app-gui-watch

# Interactive CLI shell
docker compose run --rm app-cli

# Run test suite
docker compose run --rm app-test

# Build release
docker compose run --rm app-build
```

---

## 3. Development Options Detailed Breakdown

### Option 1: Web-Based GUI Development (noVNC) — *(Recommended)*
* **How it works**: Spawns an internal virtual X11 framebuffer (`Xvfb`) and streams the desktop window to an in-browser HTML5 VNC viewer using `noVNC` on port `6080`.
* **Access URL**: [http://localhost:6080/vnc.html](http://localhost:6080/vnc.html)
* **Advantages**:
  - Works on **Windows Home, Pro, macOS, and Linux** without configuring WSLg or installing third-party X-servers (like VcXsrv or XQuartz).
  - Uses Mesa software rasterization (`LIBGL_ALWAYS_SOFTWARE=1`), bypassing host GPU driver mismatches.
  - Full mouse, keyboard, and window resizing support.

### Option 2: Hot-Reloading GUI (`cargo-watch`)
* **How it works**: Monitors both `src/` (Rust) and `ui/` (Slint) files.
* Whenever you modify and save code on your host editor, the container automatically recompiles and restarts the application within the noVNC browser tab.
* **Command**: `docker compose up app-gui-watch`

### Option 3: Interactive CLI Terminal (`app-cli`)
* **How it works**: Gives you a high-performance Debian Linux shell with Rust 1.85 pre-installed, along with `ffmpeg`, `yt-dlp`, and `sqlite3`.
* **Use case**:
  - Developing and debugging platform extractors (`src/downloader/extractor.rs`).
  - Running fast compilation checks: `cargo check`.
  - Running specific test subsets: `cargo test extractor::tests`.

### Option 4: VS Code Dev Containers (`.devcontainer`)
* **How it works**: Uses the `.devcontainer/devcontainer.json` configuration.
* **How to use**:
  1. Open the project folder in **Visual Studio Code**.
  2. Install the **Dev Containers** extension (`ms-vscode-remote.remote-containers`).
  3. Press `F1` or `Ctrl+Shift+P` -> Select **Dev Containers: Reopen in Container**.
  4. VS Code connects inside the container with `rust-analyzer` and `Slint` extension automatically configured.
  5. The GUI web port `6080` is automatically forwarded.

### Option 5: Direct Host X11 Forwarding (`app-x11`)
* If you have an active X11 server running on Windows (e.g. VcXsrv, Xming, or native WSLg):
* **Command**: `docker compose run --rm app-x11`
* The application window renders directly as a native desktop window on your host OS.

---

## 4. Volume Persistence & Build Performance

To prevent slow compilation due to Windows NTFS filesystem virtualization overhead, `docker-compose.yml` uses isolated Docker named volumes for caching:

* `cargo-cache`: Stores downloaded crate index and packages (`/usr/local/cargo/registry`).
* `cargo-git`: Stores git dependencies (`/usr/local/cargo/git`).
* `target-cache`: Stores compiled object files and intermediate dependencies (`/app/target`).

Your source code is mounted directly (`.:/app`), so any changes you make in your IDE are instantly visible in the container.

---

## 5. Architectural Recommendations

1. **For Daily Feature & UI Development**:
   Use **Option 1 (`.\docker-dev.ps1 gui`)** or **Option 2 (`.\docker-dev.ps1 watch`)** together with your favorite local editor. Open the browser side-by-side with your code editor.
2. **For CI / Backend / Test Automation**:
   Use **Option 3/4 (`.\docker-dev.ps1 test`)**.
3. **For Native Release Packaging**:
   While Docker builds Linux x86_64 binaries seamlessly, Windows native binaries (`.exe` with toast notifications) and macOS bundles (`.app` with AppleScript) are best compiled natively or through the existing GitHub Actions CI workflow in `.github/workflows/release.yml`.
