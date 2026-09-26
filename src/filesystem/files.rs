use std::path::{Path, PathBuf};
use crate::error::Result;

/// Sanitizes a filename, preventing directory traversal and removing illegal characters.
pub fn sanitize_filename(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();

    // Neutralize directory traversal sequences
    let clean = clean.replace("..", "__");
    let trimmed = clean.trim_matches(|c| c == '.' || c == ' ');
    if trimmed.is_empty() || trimmed.chars().all(|c| c == '_') {
        "downloaded_media".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Returns the user's default Downloads directory or fallback
pub fn default_download_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join("Downloads");
        if p.exists() {
            return p;
        }
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(userprofile).join("Downloads");
        if p.exists() {
            return p;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Prompts user with native desktop folder chooser dialog
pub async fn pick_directory() -> Option<PathBuf> {
    tokio::task::spawn_blocking(|| {
        #[cfg(target_os = "macos")]
        {
            let output = std::process::Command::new("osascript")
                .arg("-e")
                .arg("POSIX path of (choose folder with prompt \"Select Download Directory:\")")
                .output()
                .ok()?;
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    return Some(PathBuf::from(path_str));
                }
            }
        }
        #[cfg(target_os = "windows")]
        {
            let script = "[System.Reflection.Assembly]::LoadWithPartialName('System.windows.forms') | Out-Null; $f = New-Object System.Windows.Forms.FolderBrowserDialog; $f.ShowNewFolderButton = $true; if ($f.ShowDialog() -eq 'OK') { $f.SelectedPath }";
            let output = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", script])
                .output()
                .ok()?;
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    return Some(PathBuf::from(path_str));
                }
            }
        }
        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        {
            let output = std::process::Command::new("zenity")
                .args(["--file-selection", "--directory", "--title=Select Download Directory"])
                .output()
                .ok()?;
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    return Some(PathBuf::from(path_str));
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// Validates that a path is safe, creates parent directories, and resolves duplicate filenames
pub fn validate_destination_path(dir: &Path, filename: &str) -> Result<PathBuf> {
    let sanitized = sanitize_filename(filename);

    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
    }

    Ok(resolve_unique_path(dir, &sanitized))
}

/// Common media extensions produced by yt-dlp or direct downloads
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "webm", "mp3", "m4a", "mov", "avi", "flv", "m4v", "wav", "aac", "opus", "ogg",
    "ts", "wmv", "3gp",
];

/// Known file extensions recognized when parsing filename stems and extensions
pub const KNOWN_FILE_EXTENSIONS: &[&str] = &[
    // Video
    "mp4", "mkv", "webm", "mov", "avi", "flv", "m4v", "ts", "wmv", "3gp", "f4v",
    // Audio
    "mp3", "m4a", "wav", "aac", "opus", "ogg", "flac", "wma", "alac",
    // Archives & docs
    "zip", "tar", "gz", "7z", "rar", "pdf", "iso", "dmg", "pkg",
];

fn split_stem_and_ext(filename: &str) -> (&str, Option<&str>) {
    let path = Path::new(filename);
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        if KNOWN_FILE_EXTENSIONS.contains(&ext_lower.as_str()) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                return (stem, Some(ext));
            }
        }
    }
    (filename, None)
}

fn parse_stem_and_index(stem: &str) -> (&str, usize) {
    if let Some(open_paren) = stem.rfind(" (") {
        if stem.ends_with(')') {
            let num_str = &stem[open_paren + 2..stem.len() - 1];
            if let Ok(num) = num_str.parse::<usize>() {
                // Avoid interpreting release years like (2024) as duplicate counters
                if num > 0 && num < 500 {
                    return (&stem[..open_paren], num + 1);
                }
            }
        }
    }
    (stem, 1)
}

/// Checks whether a file with this stem and extension (or common media extensions / .part files) exists
pub fn does_conflict_exist(dir: &Path, stem: &str, ext: Option<&str>) -> bool {
    match ext {
        Some(ext_str) => {
            let clean_ext = ext_str.trim_start_matches('.');
            // Direct file: <stem>.<clean_ext>
            if dir.join(format!("{}.{}", stem, clean_ext)).exists() {
                return true;
            }
            // Partial downloads: <stem>.<clean_ext>.part or .ytdl
            if dir.join(format!("{}.{}.part", stem, clean_ext)).exists() {
                return true;
            }
            if dir.join(format!("{}.{}.ytdl", stem, clean_ext)).exists() {
                return true;
            }
        }
        None => {
            // Exact file match without extension
            if dir.join(stem).exists() {
                return true;
            }
            if dir.join(format!("{}.part", stem)).exists() {
                return true;
            }
            if dir.join(format!("{}.ytdl", stem)).exists() {
                return true;
            }
            // Extractor download without extension: check all common media extensions
            for &media_ext in MEDIA_EXTENSIONS {
                if dir.join(format!("{}.{}", stem, media_ext)).exists() {
                    return true;
                }
                if dir.join(format!("{}.{}.part", stem, media_ext)).exists() {
                    return true;
                }
                if dir.join(format!("{}.{}.ytdl", stem, media_ext)).exists() {
                    return true;
                }
            }
        }
    }
    false
}

/// If a file already exists at the destination, resolves a unique non-conflicting filename (e.g. video (1).mp4)
pub fn resolve_unique_path(dir: &Path, filename: &str) -> PathBuf {
    let (stem, ext_opt) = split_stem_and_ext(filename);

    if !does_conflict_exist(dir, stem, ext_opt) {
        return dir.join(filename);
    }

    let (base_stem, mut counter) = parse_stem_and_index(stem);
    let ext_suffix = ext_opt.map(|e| format!(".{}", e)).unwrap_or_default();

    loop {
        let candidate_stem = format!("{} ({})", base_stem, counter);
        if !does_conflict_exist(dir, &candidate_stem, ext_opt) {
            let candidate_name = format!("{}{}", candidate_stem, ext_suffix);
            return dir.join(candidate_name);
        }
        counter += 1;
    }
}

/// Opens the file's parent folder in the native OS desktop file manager
#[allow(dead_code)]
pub fn open_parent_folder(path: &Path) -> Result<()> {
    reveal_in_file_manager(path)
}

/// Resolves actual file path on disk, testing for video/audio container extensions if missing
pub fn find_actual_path(path: &Path) -> PathBuf {
    if path.exists() {
        return path.to_path_buf();
    }
    for ext in MEDIA_EXTENSIONS {
        let candidate = PathBuf::from(format!("{}.{}", path.display(), ext));
        if candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

/// Reveals the file or folder in Finder (or selects it in File Explorer / file manager)
pub fn reveal_in_file_manager(path: &Path) -> Result<()> {
    let resolved = find_actual_path(path);
    let parent = resolved.parent().unwrap_or(&resolved);

    #[cfg(target_os = "macos")]
    {
        if resolved.exists() && resolved.is_file() {
            let _ = std::process::Command::new("open").arg("-R").arg(&resolved).spawn();
        } else if resolved.exists() && resolved.is_dir() {
            let _ = std::process::Command::new("open").arg(&resolved).spawn();
        } else {
            let _ = std::process::Command::new("open").arg(parent).spawn();
        }
    }
    #[cfg(target_os = "windows")]
    {
        if resolved.exists() && resolved.is_file() {
            let _ = std::process::Command::new("explorer").arg(format!("/select,\"{}\"", resolved.display())).spawn();
        } else if resolved.exists() && resolved.is_dir() {
            let _ = std::process::Command::new("explorer").arg(&resolved).spawn();
        } else {
            let _ = std::process::Command::new("explorer").arg(parent).spawn();
        }
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let target = if resolved.exists() { &resolved } else { parent };
        let _ = std::process::Command::new("xdg-open").arg(target).spawn();
    }

    Ok(())
}

/// Plays or opens the media file with the system default media player
pub fn play_media_file(path: &Path) -> Result<()> {
    let resolved = find_actual_path(path);
    let target = if resolved.exists() {
        resolved
    } else {
        resolved.parent().unwrap_or(&resolved).to_path_buf()
    };

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&target).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd").args(["/C", "start", "", &target.to_string_lossy()]).spawn();
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&target).spawn();
    }

    Ok(())
}

/// Reads text currently stored in the system clipboard
pub fn read_clipboard_text() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("pbpaste").output().ok()?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "Get-Clipboard"])
            .output()
            .ok()?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        if let Ok(output) = std::process::Command::new("wl-paste").output() {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        if let Ok(output) = std::process::Command::new("xclip").args(["-selection", "clipboard", "-o"]).output() {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }
    None
}

/// Copies text into the native system clipboard
pub fn write_clipboard_text(text: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        if let Ok(mut child) = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::io::Write;
        if let Ok(mut child) = std::process::Command::new("clip")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        use std::io::Write;
        if let Ok(mut child) = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().map(|s| s.success()).unwrap_or(false) {
                return true;
            }
        }
        if let Ok(mut child) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize_filename("video: test? <1>.mp4"), "video_ test_ _1_.mp4");
        assert_eq!(sanitize_filename("..."), "downloaded_media");
    }

    #[test]
    fn test_resolve_unique_path_scenarios() {
        let temp_dir = std::env::temp_dir().join(format!("nvd_unit_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. No conflict returns original
        let p1 = resolve_unique_path(&temp_dir, "my_video.mp4");
        assert_eq!(p1, temp_dir.join("my_video.mp4"));

        // 2. Direct extension conflict auto-increments
        std::fs::write(temp_dir.join("my_video.mp4"), b"test").unwrap();
        let p2 = resolve_unique_path(&temp_dir, "my_video.mp4");
        assert_eq!(p2, temp_dir.join("my_video (1).mp4"));

        // Existing (1) increments to (2)
        std::fs::write(temp_dir.join("my_video (1).mp4"), b"test").unwrap();
        let p3 = resolve_unique_path(&temp_dir, "my_video.mp4");
        assert_eq!(p3, temp_dir.join("my_video (2).mp4"));

        // Starting from an existing (1) advances to (2)
        let p3_alt = resolve_unique_path(&temp_dir, "my_video (1).mp4");
        assert_eq!(p3_alt, temp_dir.join("my_video (2).mp4"));

        // 3. Extensionless extractor title detects media file on disk
        std::fs::write(temp_dir.join("Extractor Video.webm"), b"test").unwrap();
        let p4 = resolve_unique_path(&temp_dir, "Extractor Video");
        assert_eq!(p4, temp_dir.join("Extractor Video (1)"));

        // 4. In-progress .part file blocks collision
        std::fs::write(temp_dir.join("track.mp3.part"), b"partial").unwrap();
        let p5 = resolve_unique_path(&temp_dir, "track.mp3");
        assert_eq!(p5, temp_dir.join("track (1).mp3"));

        // 5. Preserves movie release years
        std::fs::write(temp_dir.join("Inception (2010).mp4"), b"movie").unwrap();
        let p6 = resolve_unique_path(&temp_dir, "Inception (2010).mp4");
        assert_eq!(p6, temp_dir.join("Inception (2010) (1).mp4"));

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

