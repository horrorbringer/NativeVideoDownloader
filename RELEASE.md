# Cross-Platform Release & Packaging Guide

This guide details how to build, package, and distribute **Native Video Downloader** into ready-to-run release artifacts for **macOS**, **Windows**, and **Linux**.

---

## Target Matrix Summary

| Platform | Target Architecture | Distribution Format | Recommended Packaging Tool |
| :--- | :--- | :--- | :--- |
| **macOS** | Apple Silicon (`aarch64`) & Intel (`x86_64`) | `.dmg`, `.app` bundle, `.zip` | `cargo-bundle` / `create-dmg` |
| **Windows** | Windows 10/11 64-bit (`x86_64-pc-windows-msvc`) | Installer (`.exe`), Portable `.zip` | Inno Setup / NSIS / WiX |
| **Linux** | Universal Linux (`x86_64-unknown-linux-gnu`) | `.AppImage`, `.deb`, `.tar.gz` | `cargo-deb` / `appimage-builder` |

---

## 1. Automated GitHub Actions CI/CD (Recommended)

The easiest and most reliable way to generate downloadable packages for all operating systems simultaneously is using **GitHub Actions**. Whenever you push a git version tag, GitHub will compile, package, and publish the downloads directly to your GitHub Releases page.

### Workflow Configuration: `.github/workflows/release.yml`

Create the file `.github/workflows/release.yml` with the following content:

```yaml
name: Release Multi-Platform Binaries

on:
  push:
    tags:
      - 'v*'

permissions:
  contents: write

jobs:
  # -------------------------------------------------------------
  # macOS Build (Universal: Apple Silicon + Intel)
  # -------------------------------------------------------------
  build-macos:
    name: macOS Universal Build
    runs-on: macos-latest
    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-apple-darwin, x86_64-apple-darwin

      - name: Rust Cache
        uses: Swatinem/rust-cache@v2

      - name: Build Apple Silicon binary
        run: cargo build --release --target aarch64-apple-darwin

      - name: Build Intel binary
        run: cargo build --release --target x86_64-apple-darwin

      - name: Combine Universal 2 Binary (lipo)
        run: |
          mkdir -p target/universal-apple-darwin/release
          lipo -create -output target/universal-apple-darwin/release/native_video_downloader \
            target/aarch64-apple-darwin/release/native_video_downloader \
            target/x86_64-apple-darwin/release/native_video_downloader

      - name: Create macOS .app Bundle
        run: |
          APP_DIR="Native Video Downloader.app"
          mkdir -p "$APP_DIR/Contents/MacOS"
          mkdir -p "$APP_DIR/Contents/Resources"
          cp target/universal-apple-darwin/release/native_video_downloader "$APP_DIR/Contents/MacOS/native_video_downloader"
          chmod +x "$APP_DIR/Contents/MacOS/native_video_downloader"

          # Create Info.plist
          cat << 'EOF' > "$APP_DIR/Contents/Info.plist"
          <?xml version="1.0" encoding="UTF-8"?>
          <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
          <plist version="1.0">
          <dict>
              <key>CFBundleExecutable</key>
              <string>native_video_downloader</string>
              <key>CFBundleIdentifier</key>
              <string>com.nativevideodownloader.app</string>
              <key>CFBundleName</key>
              <string>Native Video Downloader</string>
              <key>CFBundlePackageType</key>
              <string>APPL</string>
              <key>CFBundleShortVersionString</key>
              <string>0.1.0</string>
              <key>LSMinimumSystemVersion</key>
              <string>11.0</string>
              <key>NSHighResolutionCapable</key>
              <true/>
          </dict>
          </plist>
          EOF

          # Package into ZIP and DMG
          zip -r "NativeVideoDownloader-macOS-Universal.zip" "$APP_DIR"
          hdiutil create -volname "Native Video Downloader" -srcfolder "$APP_DIR" -ov -format UDZO "NativeVideoDownloader-macOS-Universal.dmg"

      - name: Upload macOS Assets to Release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            NativeVideoDownloader-macOS-Universal.dmg
            NativeVideoDownloader-macOS-Universal.zip

  # -------------------------------------------------------------
  # Windows Build (.exe & Portable .zip)
  # -------------------------------------------------------------
  build-windows:
    name: Windows x64 Build
    runs-on: windows-latest
    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-pc-windows-msvc

      - name: Rust Cache
        uses: Swatinem/rust-cache@v2

      - name: Build Release Executable
        run: cargo build --release --target x86_64-pc-windows-msvc

      - name: Create Windows Portable Archive
        shell: pwsh
        run: |
          New-Item -ItemType Directory -Force -Path dist-win
          Copy-Item target/x86_64-pc-windows-msvc/release/native_video_downloader.exe dist-win/
          Copy-Item README.md dist-win/ -ErrorAction SilentlyContinue
          Compress-Archive -Path dist-win/* -DestinationPath NativeVideoDownloader-Windows-x64.zip

      - name: Upload Windows Assets to Release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            NativeVideoDownloader-Windows-x64.zip
            target/x86_64-pc-windows-msvc/release/native_video_downloader.exe

  # -------------------------------------------------------------
  # Linux Build (x86_64 Portable Tarball & AppImage)
  # -------------------------------------------------------------
  build-linux:
    name: Linux x64 Build
    runs-on: ubuntu-22.04
    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      - name: Install System Dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y libfontconfig1-dev libxcb1-dev libx11-xcb-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-unknown-linux-gnu

      - name: Rust Cache
        uses: Swatinem/rust-cache@v2

      - name: Build Release Binary
        run: cargo build --release --target x86_64-unknown-linux-gnu

      - name: Package Linux Tarball
        run: |
          mkdir -p dist-linux
          cp target/x86_64-unknown-linux-gnu/release/native_video_downloader dist-linux/
          chmod +x dist-linux/native_video_downloader
          tar -czvf NativeVideoDownloader-Linux-x64.tar.gz -C dist-linux .

      - name: Upload Linux Assets to Release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            NativeVideoDownloader-Linux-x64.tar.gz
            target/x86_64-unknown-linux-gnu/release/native_video_downloader
```

### Triggering a Release
To publish a new release with downloadable binaries for every OS, run:
```bash
git tag v0.1.0
git push origin v0.1.0
```

---

## 2. Local Builds for macOS

### Step 1: Optimized Release Build
```bash
# Build for your current Mac architecture (Intel or Apple Silicon)
cargo build --release
```
The optimized, stripped binary will be located at:
```bash
target/release/native_video_downloader
```

### Step 2: Creating a Universal macOS Binary (Runs on both M1/M2/M3/M4 & Intel Macs)
```bash
# Install cross-compilation targets
rustup target add aarch64-apple-darwin
rustup target add x86_64-apple-darwin

# Compile both targets
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

# Combine into a single Universal binary
mkdir -p target/universal
lipo -create -output target/universal/native_video_downloader \
  target/aarch64-apple-darwin/release/native_video_downloader \
  target/x86_64-apple-darwin/release/native_video_downloader
```

### Step 3: Packaging into a macOS `.app` & `.dmg`
Create an `.app` structure:
```bash
APP_NAME="Native Video Downloader"
mkdir -p "$APP_NAME.app/Contents/MacOS"
mkdir -p "$APP_NAME.app/Contents/Resources"

# Copy binary
cp target/universal/native_video_downloader "$APP_NAME.app/Contents/MacOS/native_video_downloader"
chmod +x "$APP_NAME.app/Contents/MacOS/native_video_downloader"

# Copy app icon if available
if [ -f assets/app_icon.png ]; then
  # Convert PNG to ICNS (optional, or use iconutil)
  cp assets/app_icon.png "$APP_NAME.app/Contents/Resources/AppIcon.png"
fi

# Create Info.plist
cat << EOF > "$APP_NAME.app/Contents/Info.plist"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>native_video_downloader</string>
    <key>CFBundleIdentifier</key>
    <string>com.nativevideodownloader.app</string>
    <key>CFBundleName</key>
    <string>Native Video Downloader</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

# Build Drag-and-Drop DMG
hdiutil create -volname "$APP_NAME" -srcfolder "$APP_NAME.app" -ov -format UDZO "$APP_NAME-macOS.dmg"
```

---

## 3. Local Builds for Windows

### Option A: Direct Compilation on Windows 10/11
In PowerShell:
```powershell
# 1. Compile release executable
cargo build --release

# 2. Executable location
# target\release\native_video_downloader.exe
```

### Option B: Cross-Compiling for Windows from macOS / Linux
Using `cargo-xwin` (compiles Windows MSVC binaries without needing a Windows machine):
```bash
# 1. Install cargo-xwin
cargo install cargo-xwin

# 2. Add Windows target
rustup target add x86_64-pc-windows-msvc

# 3. Build Windows .exe
cargo xwin build --release --target x86_64-pc-windows-msvc
```
Output:
```bash
target/x86_64-pc-windows-msvc/release/native_video_downloader.exe
```

### Creating a Windows Installer with Inno Setup
Using an Inno Setup script (`installer.iss`), you can package the `.exe` into an installer that creates desktop shortcuts and an uninstaller:
```ini
[Setup]
AppName=Native Video Downloader
AppVersion=0.1.0
DefaultDirName={autopf}\Native Video Downloader
DefaultGroupName=Native Video Downloader
OutputDir=.\Output
OutputBaseFilename=NativeVideoDownloader-Setup
Compression=lzma
SolidCompression=yes

[Files]
Source: "target\release\native_video_downloader.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Native Video Downloader"; Filename: "{app}\native_video_downloader.exe"
Name: "{autodesktop}\Native Video Downloader"; Filename: "{app}\native_video_downloader.exe"
```

---

## 4. Local Builds for Linux

### Option A: Native Linux Build
On Ubuntu/Debian/Fedora/Arch:
```bash
# 1. Install Slint/GUI system development libraries
# Ubuntu / Debian:
sudo apt-get install -y libfontconfig1-dev libxcb1-dev libx11-xcb-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev

# Fedora:
sudo dnf install -y fontconfig-devel libxcb-devel libxkbcommon-devel libxkbcommon-x11-devel wayland-devel

# 2. Build release binary
cargo build --release
```

### Option B: Creating a `.deb` Installer for Ubuntu / Debian
```bash
# 1. Install cargo-deb
cargo install cargo-deb

# 2. Build Debian package
cargo deb
```
Output:
```bash
target/debian/native-video-downloader_0.1.0_amd64.deb
```
Users can install it with:
```bash
sudo dpkg -i native-video-downloader_0.1.0_amd64.deb
```

### Option C: Standalone Portable Tarball
```bash
mkdir -p dist-linux
cp target/release/native_video_downloader dist-linux/
chmod +x dist-linux/native_video_downloader
tar -czvf NativeVideoDownloader-Linux-x64.tar.gz -C dist-linux .
```

---

## 5. Summary of Release Assets

When distributing releases, publish these standard downloadable files:

| File Name | Intended Audience |
| :--- | :--- |
| **`NativeVideoDownloader-macOS-Universal.dmg`** | macOS users (M1/M2/M3/M4 & Intel, macOS 11+) |
| **`NativeVideoDownloader-macOS-Universal.zip`** | Portable macOS `.app` |
| **`NativeVideoDownloader-Windows-x64.zip`** | Windows 10/11 portable archive (unzip and run) |
| **`NativeVideoDownloader-Setup.exe`** | Windows 10/11 installer with desktop shortcut |
| **`NativeVideoDownloader-Linux-x64.tar.gz`** | Linux portable binary (runs on any modern distro) |
| **`native-video-downloader_0.1.0_amd64.deb`** | Debian / Ubuntu / Linux Mint one-click installer |
