# Native Video Downloader — Developer & Operations Notes

---

## 1. Hot-Reloading & Live Development (No Manual Terminal Restarts)

When working on the codebase, standard `cargo run` keeps the running process locked in memory until stopped with `Ctrl+C`.

To have your application **automatically recompile and restart whenever you save code changes**:

### Installation (One-Time Setup)
```bash
cargo install cargo-watch
```

### Running with Live File Watcher
```bash
# Watches both Rust source files and Slint UI files:
cargo watch -w src -w ui -x run
```

### Quick Reference Flags
- `-w src -w ui`: Only triggers on edits inside `src/` and `ui/` (ignores changes in logs, database, or temp files).
- `-x run`: Executes `cargo run` whenever a file change is detected.
- `-c`: Clear screen between rebuilds (`cargo watch -c -w src -w ui -x run`).
- `-x check`: Fast compilation check without running (`cargo watch -w src -w ui -x check`).

---

## 2. In-App Auto-Updater & In-Place Self-Updater (End-User Mode)

For users who download official releases (DMG, ZIP, or standalone binary), they **do not need a terminal or cargo**:

### How It Works:
1. **GitHub Releases Query**:
   - Queries `https://api.github.com/repos/horrorbringer/NativeVideoDownloader/releases/latest`.
   - Parses the latest semver tag (e.g. `v0.2.2`) and compares against current `CARGO_PKG_VERSION` (`v0.2.1`).
2. **Asset Resolution**:
   - **macOS**: `native_video_downloader` (Universal 2 binary), `NativeVideoDownloader-macOS-Universal.dmg`
   - **Windows**: `native_video_downloader.exe`
   - **Linux**: `native_video_downloader` / `.tar.gz`
3. **Atomic Binary Swap**:
   - Streams the new executable into a temporary file (`.native_video_downloader_update_<uuid>.tmp`).
   - Sets Unix permissions (`0o755`).
   - Renames current running executable to a timestamped backup (`native_video_downloader.old_<uuid>`).
   - Atomically moves the new binary into place.
4. **Instant Relaunch**:
   - On macOS: launches `.app` bundle via `open -n` or spawns binary directly.

---

## 3. Useful Development & Test Commands

```bash
# Run all unit and integration tests (65 tests)
cargo test --bin native_video_downloader

# Check compilation and type safety quickly
cargo check

# Build optimized production release
cargo build --release

# Inspect live Slint files without full Rust compilation (optional)
# slint-viewer ui/app.slint
```
