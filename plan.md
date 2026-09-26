# Native Video Downloader

## 1. Project Overview

Build a **high-performance native desktop video download manager** using **Rust + Slint**.

The application must run as a native desktop application on:

* macOS
* Windows
* Linux

The application should **not use web technologies** such as:

* HTML
* CSS
* JavaScript
* React
* Vue
* Electron
* Tauri/WebView

The application is intended for downloading media from **sources that permit downloading**. It must not bypass DRM, encryption, authentication controls, geographic restrictions, subscription restrictions, or other access controls.

---

# 2. Main Goals

The application should provide:

1. Fast and reliable downloads
2. Multiple concurrent downloads
3. Pause/resume
4. Retry and recovery
5. Download queue management
6. Video/audio/subtitle selection when exposed by the authorized source
7. Batch downloads
8. Download history
9. Native desktop UI
10. Low memory usage
11. Cross-platform support
12. Media processing through FFmpeg
13. Persistent download state
14. Clean and extensible architecture

---

# 3. Technology Stack

## Core

### Rust

Primary programming language.

Responsibilities:

* Application logic
* Networking
* Download engine
* Concurrency
* File I/O
* Database
* Queue management
* Error handling
* Process management
* FFmpeg integration

Target:

```text
Rust stable
```

---

## UI

### Slint

Native declarative desktop UI.

Use `.slint` files for:

* Windows
* Pages
* Components
* Layout
* State binding
* User interactions

The UI must communicate with Rust through Slint's Rust API.

No web frontend.

---

# 4. Rust Dependencies

Initial dependencies:

```toml
[dependencies]
slint = "1"
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["rustls-tls", "stream"] }
tokio-util = "0.7"
futures = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.8", features = [
    "runtime-tokio-rustls",
    "sqlite"
] }
tracing = "0.1"
tracing-subscriber = "0.3"
thiserror = "2"
uuid = { version = "1", features = ["v4", "serde"] }
```

Build dependency:

```toml
[build-dependencies]
slint-build = "1"
```

FFmpeg can be installed as an external system dependency initially.

---

# 5. Architecture

```text
┌─────────────────────────────────────────────┐
│                 Slint UI                    │
│                                             │
│ Home │ Downloads │ History │ Settings       │
└──────────────────────┬──────────────────────┘
                       │
                Slint callbacks
                       │
                       ▼
┌─────────────────────────────────────────────┐
│              Application Layer              │
│                                             │
│ Commands │ State │ Events │ Validation      │
└──────────────────────┬──────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────┐
│             Download Manager               │
│                                             │
│ Queue │ Scheduler │ Workers │ Retry        │
└──────────────┬──────────────────┬───────────┘
               │                  │
               ▼                  ▼
        Network Layer       Storage Layer
               │                  │
           Reqwest              SQLite
               │                  │
               ▼                  ▼
           HTTP/S              Filesystem
                       │
                       ▼
                  FFmpeg
```

---

# 6. Project Structure

```text
native-video-downloader/
│
├── Cargo.toml
├── Cargo.lock
├── build.rs
├── README.md
├── LICENSE
│
├── ui/
│   ├── app.slint
│   │
│   ├── components/
│   │   ├── download_item.slint
│   │   ├── download_list.slint
│   │   ├── progress_bar.slint
│   │   ├── url_input.slint
│   │   ├── video_info.slint
│   │   └── settings_panel.slint
│   │
│   └── pages/
│       ├── home.slint
│       ├── downloads.slint
│       ├── history.slint
│       └── settings.slint
│
├── src/
│   ├── main.rs
│   ├── app.rs
│   │
│   ├── commands/
│   │   ├── mod.rs
│   │   ├── download.rs
│   │   └── media.rs
│   │
│   ├── downloader/
│   │   ├── mod.rs
│   │   ├── manager.rs
│   │   ├── queue.rs
│   │   ├── worker.rs
│   │   ├── job.rs
│   │   ├── progress.rs
│   │   ├── retry.rs
│   │   └── cancellation.rs
│   │
│   ├── network/
│   │   ├── mod.rs
│   │   ├── client.rs
│   │   ├── range.rs
│   │   └── headers.rs
│   │
│   ├── parser/
│   │   ├── mod.rs
│   │   ├── metadata.rs
│   │   └── source.rs
│   │
│   ├── database/
│   │   ├── mod.rs
│   │   ├── migrations.rs
│   │   └── downloads.rs
│   │
│   ├── filesystem/
│   │   ├── mod.rs
│   │   ├── files.rs
│   │   └── temp.rs
│   │
│   ├── ffmpeg/
│   │   ├── mod.rs
│   │   ├── convert.rs
│   │   ├── merge.rs
│   │   └── metadata.rs
│   │
│   ├── models/
│   │   ├── mod.rs
│   │   ├── download.rs
│   │   ├── video.rs
│   │   └── settings.rs
│   │
│   ├── config/
│   │   ├── mod.rs
│   │   └── settings.rs
│   │
│   └── error.rs
│
├── migrations/
│   └── ...
│
└── tests/
    ├── downloader.rs
    ├── parser.rs
    └── filesystem.rs
```

---

# 7. Core Features

## 7.1 URL Input

User can paste a supported URL.

UI:

```text
┌──────────────────────────────────────────────┐
│ Video URL                                    │
│                                              │
│ https://example.com/video/123                │
│                                              │
│                         [ Analyze ]           │
└──────────────────────────────────────────────┘
```

Requirements:

* Validate URL
* Detect unsupported URLs
* Normalize URL
* Prevent malformed input
* Do not expose sensitive credentials in logs

---

# 8. Video Analysis

After URL submission:

```text
URL
 ↓
Source adapter
 ↓
Metadata
 ↓
Video information
```

Display:

```text
Title
Thumbnail
Duration
Available qualities
Video codecs
Audio tracks
Subtitle tracks
Estimated size
Available formats
```

Only use media information that is legitimately exposed by the source.

---

# 9. Quality Selection

Support available quality options such as:

```text
360p
480p
720p
1080p
1440p
2160p
```

Quality selection should be source-dependent.

Example:

```text
Quality

○ Best available
○ 1080p
● 720p
○ 480p
○ 360p
```

---

# 10. Format Selection

Possible output formats:

```text
MP4
MKV
WebM
```

The application should only offer formats compatible with the selected media streams.

---

# 11. Audio Selection

When multiple authorized audio tracks are available:

```text
Audio

● English
○ Khmer
○ Chinese
○ Japanese
```

---

# 12. Subtitle Selection

When subtitles are legitimately available:

```text
Subtitles

☑ English
☐ Khmer
☐ Chinese
☐ Japanese
```

Supported subtitle formats may include:

```text
SRT
ASS
VTT
TTML
```

---

# 13. Download Queue

Every download becomes a job.

```text
DownloadJob
├── id
├── source_url
├── title
├── filename
├── output_path
├── status
├── total_size
├── downloaded_size
├── progress
├── speed
├── eta
├── quality
├── format
├── created_at
├── started_at
├── completed_at
└── error
```

Statuses:

```text
Queued
Downloading
Paused
Completed
Failed
Cancelled
Processing
```

---

# 14. Concurrent Downloads

Allow configurable concurrent workers.

Default:

```text
3 downloads
```

Settings:

```text
Maximum concurrent downloads:
[ 3 ]
```

Architecture:

```text
                 Download Manager
                       │
          ┌────────────┼────────────┐
          ▼            ▼            ▼
       Worker 1     Worker 2     Worker 3
          │            │            │
          ▼            ▼            ▼
        HTTP         HTTP         HTTP
```

Use Tokio tasks and bounded concurrency.

Do not create an unlimited number of threads.

---

# 15. Pause / Resume

Each job supports:

```text
[ Pause ]
[ Resume ]
[ Cancel ]
```

Resume should use HTTP range requests when supported by the source.

If the server does not support resuming, clearly report that behavior rather than pretending the download can resume.

---

# 16. Retry System

Retry transient failures.

Example:

```text
Network timeout
Connection reset
Temporary server failure
```

Config:

```text
Maximum retries: 5
Retry delay: 3 seconds
```

Use exponential backoff where appropriate.

Do not automatically retry permanent errors indefinitely.

Examples:

```text
404 → permanent failure
Invalid URL → permanent failure
Unsupported format → permanent failure
Authentication failure → user action required
```

---

# 17. Download Progress

Every download should display:

```text
Title
Progress
Downloaded size
Total size
Speed
ETA
Status
```

Example:

```text
Episode 01

██████████████░░░░░░  72%

850 MB / 1.2 GB

Speed: 8.4 MB/s
ETA: 00:42

[ Pause ] [ Cancel ]
```

---

# 18. Speed Limiter

Settings:

```text
Download speed limit

Unlimited
1 MB/s
5 MB/s
10 MB/s
20 MB/s
Custom
```

Implement throttling inside the download stream.

---

# 19. Batch Downloads

Allow multiple URLs/jobs.

Example:

```text
☑ Episode 01
☑ Episode 02
☑ Episode 03
☑ Episode 04
☐ Episode 05

[ Download Selected ]
```

Support queue operations:

```text
Start All
Pause All
Resume All
Cancel All
Remove Completed
Clear Failed
```

---

# 20. File Management

Allow the user to select:

```text
Download directory
Temporary directory
```

Automatic filename:

```text
{title}.{extension}
```

Optional template:

```text
{series}/{episode} - {title}.{ext}
```

Example:

```text
Downloads/
└── My Series/
    ├── 01 - Episode One.mp4
    ├── 02 - Episode Two.mp4
    └── 03 - Episode Three.mp4
```

---

# 21. Duplicate Detection

Before starting:

```text
File already exists.

Episode 01.mp4

○ Replace
○ Rename
● Skip
```

Possible strategies:

```text
filename
file size
database record
checksum
```

---

# 22. Download History

SQLite stores completed and failed downloads.

UI:

```text
History

Search...

✓ Episode 01      1.2 GB
✓ Episode 02      1.1 GB
✓ Episode 03      1.4 GB
✗ Episode 04      Failed
```

Actions:

```text
Open File
Open Folder
Download Again
Remove History
```

---

# 23. SQLite Database

Initial schema:

```text
downloads
────────────────────
id
url
title
filename
output_path
status
quality
format
total_size
downloaded_size
speed
error
created_at
started_at
completed_at
```

Settings can be stored separately.

---

# 24. Temporary Files

Never directly write an incomplete download to the final filename.

Use:

```text
episode.mp4.part
```

After successful completion:

```text
episode.mp4.part
       ↓
verification
       ↓
episode.mp4
```

If the application crashes, unfinished `.part` files can be detected on startup.

---

# 25. Crash Recovery

On application startup:

```text
Database
   +
.part files
   ↓
Recovery Manager
   ↓
Restore incomplete jobs
```

Prompt:

```text
3 unfinished downloads found.

[ Resume All ]

[ Remove ]
```

---

# 26. FFmpeg Integration

Use FFmpeg for media processing.

Features:

```text
Merge video + audio
Convert format
Extract audio
Extract subtitles
Embed subtitles
Read media metadata
```

Example:

```text
Video stream
      +
Audio stream
      +
Subtitle
      ↓
FFmpeg
      ↓
Final media file
```

FFmpeg should run as a separate process rather than blocking the Slint UI.

---

# 27. System Tray

Application should continue downloading while the main window is hidden.

Tray menu:

```text
Video Downloader

Downloads: 3
Speed: 12.4 MB/s

Open
Pause All
Resume All
Show Downloads
Quit
```

---

# 28. Notifications

Notify the user:

```text
Download completed
Download failed
All downloads completed
Download paused
```

Notifications should be platform-specific where necessary.

---

# 29. Drag & Drop

Support dragging URLs/files into the application where the platform and Slint integration permit it.

Possible behavior:

```text
Drop URL
   ↓
Analyze
   ↓
Show metadata
```

Local files can optionally be used for FFmpeg processing.

---

# 30. Keyboard Shortcuts

Recommended:

```text
Ctrl/Cmd + V
Paste URL

Ctrl/Cmd + Enter
Analyze / Start

Space
Pause / Resume

Delete
Remove selected job

Ctrl/Cmd + F
Search

Ctrl/Cmd + ,
Settings
```

---

# 31. Search / Filtering

Downloads page:

```text
Search downloads...

[ All ]
[ Downloading ]
[ Queued ]
[ Completed ]
[ Failed ]
[ Paused ]
```

Search by:

```text
Title
Filename
URL
```

---

# 32. Themes

Support:

```text
Dark
Light
System
```

Default:

```text
System
```

---

# 33. Settings

Settings page:

```text
General
────────────────────────
Download directory
Temporary directory
Filename template


Downloads
────────────────────────
Concurrent downloads
Speed limit
Retry attempts
Retry delay


Media
────────────────────────
Default quality
Default format
Default audio
Default subtitles


Behavior
────────────────────────
☑ Start downloads automatically
☑ Resume unfinished downloads
☑ Show notifications
☑ Minimize to tray


Appearance
────────────────────────
Theme
```

---

# 34. Security Requirements

The application must:

* Never log passwords
* Never log authentication cookies
* Avoid storing credentials in plaintext
* Validate URLs
* Validate filesystem paths
* Prevent unsafe path traversal
* Sanitize filenames
* Restrict temporary files to controlled directories
* Handle external process arguments safely
* Avoid shell command injection
* Validate FFmpeg arguments

Never implement:

* DRM circumvention
* Encryption bypass
* Authentication bypass
* Paywall bypass
* Geo-restriction bypass
* Token theft
* Cookie theft
* Unauthorized stream extraction

---

# 35. Performance Requirements

Target:

```text
Low CPU usage when idle
Low memory usage
Responsive UI during downloads
Non-blocking filesystem operations
Non-blocking network operations
Bounded concurrency
Streaming downloads
```

Rules:

```text
UI thread
   ↓
Never block

Network
   ↓
Async

File I/O
   ↓
Async / buffered

Media processing
   ↓
Separate process

Database
   ↓
Async
```

---

# 36. Error Handling

Use structured Rust errors.

Recommended:

```rust
thiserror
```

Error categories:

```text
InvalidUrl
UnsupportedSource
NetworkError
HttpError
FileSystemError
DatabaseError
MediaProcessingError
Cancelled
UnsupportedFormat
AuthenticationRequired
```

UI should display human-readable messages.

Example:

```text
Download failed

Connection timed out.

[ Retry ] [ Remove ]
```

Don't show raw Rust stack traces to normal users.

---

# 37. Logging

Use:

```text
tracing
tracing-subscriber
```

Log levels:

```text
ERROR
WARN
INFO
DEBUG
TRACE
```

Example:

```text
INFO  download started
INFO  download progress
WARN  retrying request
ERROR download failed
```

Never log sensitive authentication information.

---

# 38. Application State

Central application state:

```text
AppState
├── downloads
├── selected_download
├── settings
├── connection_status
└── application_status
```

Rust owns the authoritative state.

Slint displays and interacts with that state.

---

# 39. Event Architecture

Use events/channels between workers and UI.

```text
Download Worker
      │
      ▼
Progress Event
      │
      ▼
Download Manager
      │
      ▼
Application State
      │
      ▼
Slint UI
```

Possible events:

```text
DownloadStarted
DownloadProgress
DownloadPaused
DownloadResumed
DownloadCompleted
DownloadFailed
DownloadCancelled
DownloadRetrying
MediaProcessingStarted
MediaProcessingCompleted
```

---

# 40. Testing

## Unit tests

Test:

```text
URL validation
Filename sanitization
Queue logic
Retry logic
Progress calculation
Speed calculation
ETA calculation
Path handling
```

## Integration tests

Test:

```text
HTTP download
Resume
Retry
Cancellation
Database persistence
Crash recovery
FFmpeg processing
```

## Manual testing

Platforms:

```text
macOS
Windows
Linux
```

---

# 41. Development Roadmap

## Phase 1 — Foundation

Build:

```text
Rust project
Slint window
Navigation
Settings
Logging
Error handling
```

Goal:

```text
Application launches successfully.
```

---

## Phase 2 — Basic Downloader

Implement:

```text
URL input
HTTP download
Progress
Speed
ETA
Save file
Cancel
```

Goal:

```text
URL → downloaded file
```

---

## Phase 3 — Download Manager

Add:

```text
Queue
Multiple downloads
Pause
Resume
Retry
Concurrent workers
```

Goal:

```text
Multiple reliable downloads.
```

---

## Phase 4 — Persistence

Add:

```text
SQLite
Download history
Crash recovery
.part files
Duplicate detection
```

Goal:

```text
Close application → reopen → downloads remain.
```

---

## Phase 5 — Media

Add:

```text
Metadata
Quality selection
Audio selection
Subtitle selection
FFmpeg
Format conversion
Stream merging
```

Goal:

```text
Download → process → final media
```

---

## Phase 6 — Desktop Experience

Add:

```text
System tray
Notifications
Drag & drop
Keyboard shortcuts
Dark/light theme
Settings
```

Goal:

```text
Professional desktop experience.
```

---

## Phase 7 — Optimization

Profile:

```text
CPU
Memory
Disk I/O
Network throughput
Database performance
Startup time
UI responsiveness
```

Optimize only after profiling.

---

# 42. MVP Definition

The first working version should contain only:

```text
✓ Native Slint UI
✓ URL input
✓ Analyze supported URL
✓ Download
✓ Progress
✓ Speed
✓ ETA
✓ Pause
✓ Resume
✓ Cancel
✓ Retry
✓ Concurrent downloads
✓ Output directory
✓ SQLite history
✓ Error handling
```

Do NOT start with:

```text
✗ FFmpeg conversion
✗ Embedded player
✗ System tray
✗ Complex themes
✗ Plugin architecture
✗ Advanced scheduling
```

Those come later.

---

# 43. Version Roadmap

## v0.1

```text
Native UI
Basic downloader
Progress
Queue
```

## v0.2

```text
Pause/resume
Retry
Concurrent downloads
SQLite
History
```

## v0.3

```text
Batch downloads
Quality selection
Audio/subtitle selection
Crash recovery
```

## v0.4

```text
FFmpeg
Media processing
Format conversion
Merge streams
```

## v0.5

```text
System tray
Notifications
Drag & drop
Keyboard shortcuts
```

## v1.0

```text
Stable cross-platform release
Performance optimization
Security audit
Automated tests
Packaging
Documentation
```

---

# 44. Design Principles

Follow these principles throughout development:

### Rust owns business logic

Slint should primarily handle presentation and user interaction.

### UI must never block

Downloads, database operations, FFmpeg, and filesystem operations must not freeze the UI.

### Prefer async I/O

Use Tokio for network and concurrent operations.

### Use bounded concurrency

Don't create unlimited workers.

### Persist important state

Download jobs should survive application restarts.

### Fail safely

Partial downloads should never be mistaken for completed files.

### Design for extensibility

Source-specific parsing should be isolated behind interfaces/adapters.

Example:

```text
Source
├── can_handle(url)
├── analyze(url)
└── create_download(...)
```

This allows additional **authorized sources** to be added later without rewriting the download manager.

---

# 45. First Coding Milestone

Start with:

```text
native-video-downloader/
│
├── Cargo.toml
├── build.rs
│
├── ui/
│   └── app.slint
│
└── src/
    └── main.rs
```

The first milestone is simply:

```text
cargo run
      ↓
Native desktop window
      ↓
URL input
      ↓
Analyze button
      ↓
Rust callback
      ↓
Console/log output
```

Once that works, implement the download engine independently:

```text
Rust Downloader
      ↓
HTTP
      ↓
File
```

Then connect:

```text
Slint
  ↕
Rust Application
  ↕
Download Manager
  ↕
Network / Filesystem
```

This keeps the project maintainable and lets us build it incrementally rather than putting the entire application into one large `main.rs`.
