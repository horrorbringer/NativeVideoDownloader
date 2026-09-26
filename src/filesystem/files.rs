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

/// If a file already exists at the destination, resolves a unique non-conflicting filename (e.g. video (1).mp4)
pub fn resolve_unique_path(dir: &Path, filename: &str) -> PathBuf {
    let initial_path = dir.join(filename);
    if !initial_path.exists() {
        return initial_path;
    }

    let path_obj = Path::new(filename);
    let stem = path_obj
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let ext = path_obj
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{}", e))
        .unwrap_or_default();

    let mut counter = 1;
    loop {
        let candidate_name = format!("{} ({}){}", stem, counter, ext);
        let candidate_path = dir.join(candidate_name);
        if !candidate_path.exists() {
            return candidate_path;
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
    for ext in &["mp4", "mkv", "webm", "mp3", "m4a", "mov"] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize_filename("video: test? <1>.mp4"), "video_ test_ _1_.mp4");
        assert_eq!(sanitize_filename("..."), "downloaded_media");
    }
}
