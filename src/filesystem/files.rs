use std::path::{Path, PathBuf};
use tracing::info;
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

/// Prompts user with native desktop file chooser dialog for text / link files
pub async fn pick_file() -> Option<PathBuf> {
    tokio::task::spawn_blocking(|| {
        #[cfg(target_os = "macos")]
        {
            let output = std::process::Command::new("osascript")
                .arg("-e")
                .arg("POSIX path of (choose file with prompt \"Select Links File (.txt, .m3u, .csv):\")")
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
            let script = "[System.Reflection.Assembly]::LoadWithPartialName('System.windows.forms') | Out-Null; $f = New-Object System.Windows.Forms.OpenFileDialog; $f.Title = 'Select Links File'; $f.Filter = 'Text & Playlist Files (*.txt;*.m3u;*.m3u8;*.csv)|*.txt;*.m3u;*.m3u8;*.csv|All Files (*.*)|*.*'; if ($f.ShowDialog() -eq 'OK') { $f.FileName }";
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
                .args([
                    "--file-selection",
                    "--title=Select Links File",
                    "--file-filter=Text & Playlist Files (*.txt, *.m3u, *.m3u8, *.csv) | *.txt *.m3u *.m3u8 *.csv",
                ])
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

/// Prompts user with native desktop save file dialog
pub async fn pick_save_file(default_filename: &str, prompt_title: &str) -> Option<PathBuf> {
    let def_name = default_filename.to_string();
    let prompt = prompt_title.to_string();
    tokio::task::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        {
            let script = format!(
                "POSIX path of (choose file name with prompt \"{}\" default name \"{}\")",
                prompt.replace('\"', "\\\""),
                def_name.replace('\"', "\\\"")
            );
            let output = std::process::Command::new("osascript")
                .arg("-e")
                .arg(&script)
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
            let script = format!(
                "[System.Reflection.Assembly]::LoadWithPartialName('System.windows.forms') | Out-Null; $f = New-Object System.Windows.Forms.SaveFileDialog; $f.Title = '{}'; $f.FileName = '{}'; $f.Filter = 'CSV Files (*.csv)|*.csv|All Files (*.*)|*.*'; if ($f.ShowDialog() -eq 'OK') {{ $f.FileName }}",
                prompt.replace('\'', "''"),
                def_name.replace('\'', "''")
            );
            let output = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &script])
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
                .args([
                    "--file-selection",
                    "--save",
                    "--confirm-overwrite",
                    &format!("--filename={}", def_name),
                    &format!("--title={}", prompt),
                ])
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

/// Escapes a CSV column value according to RFC 4180
pub fn escape_csv_field(val: &str) -> String {
    if val.contains(',') || val.contains('\"') || val.contains('\n') || val.contains('\r') {
        format!("\"{}\"", val.replace('\"', "\"\""))
    } else {
        val.to_string()
    }
}

/// Formats a list of database history records into RFC 4180 compliant CSV text
pub fn format_history_csv(records: &[crate::database::HistoryRecord]) -> String {
    let mut out = String::from("ID,Title,URL,Filename,Output Path,Status,Total Size (Bytes),Downloaded Size (Bytes),Error,Created At,Completed At\r\n");
    for r in records {
        let id = escape_csv_field(&r.id.to_string());
        let title = escape_csv_field(&r.title);
        let url = escape_csv_field(&r.url);
        let filename = escape_csv_field(&r.filename);
        let output_path = escape_csv_field(&r.output_path);
        let status = escape_csv_field(&r.status);
        let total_size = r.total_size.map(|s| s.to_string()).unwrap_or_default();
        let downloaded_size = r.downloaded_size.to_string();
        let error = escape_csv_field(r.error.as_deref().unwrap_or(""));
        let created_at = escape_csv_field(&r.created_at);
        let completed_at = escape_csv_field(r.completed_at.as_deref().unwrap_or(""));

        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{}\r\n",
            id, title, url, filename, output_path, status, total_size, downloaded_size, error, created_at, completed_at
        ));
    }
    out
}

/// Exports a list of history records to a destination CSV file
pub async fn export_history_to_csv(
    records: &[crate::database::HistoryRecord],
    path: &Path,
) -> Result<usize> {
    let content = format_history_csv(records);
    tokio::fs::write(path, content).await?;
    Ok(records.len())
}


/// Extracts all unique valid http/https URLs from a text file, playlist, or document
pub fn parse_links_from_text(content: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in content.lines() {
        let line = line.trim();
        // Skip comment-only lines in m3u or scripts
        if line.starts_with('#') || line.starts_with("//") {
            continue;
        }

        // Search for http:// or https:// substrings in the line
        let mut remainder = line;
        while let Some(start_idx) = remainder.find("http://").or_else(|| remainder.find("https://")) {
            let slice = &remainder[start_idx..];

            // Delimiters that terminate a URL
            let end_idx = slice
                .find(|c: char| {
                    c.is_whitespace()
                        || c == '"'
                        || c == '\''
                        || c == '<'
                        || c == '>'
                        || c == '`'
                        || c == '['
                        || c == ']'
                })
                .unwrap_or(slice.len());

            let mut candidate = &slice[..end_idx];

            // Strip trailing punctuation like '.', ',', ';', ')'
            while candidate.ends_with('.')
                || candidate.ends_with(',')
                || candidate.ends_with(';')
                || (candidate.ends_with(')') && !candidate.contains('('))
            {
                candidate = &candidate[..candidate.len() - 1];
            }

            if let Ok(url) = reqwest::Url::parse(candidate) {
                let url_str = url.to_string();
                if (url_str.starts_with("http://") || url_str.starts_with("https://"))
                    && seen.insert(url_str.clone())
                {
                    links.push(url_str);
                }
            }

            remainder = &slice[end_idx..];
        }
    }

    links
}

/// Validates that a path is safe, creates parent directories, and resolves duplicate filenames
#[allow(dead_code)]
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

pub fn split_stem_and_ext(filename: &str) -> (&str, Option<&str>) {
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

/// Normalizes a media title for audio extraction, replacing any existing video extension with the target audio format
#[allow(dead_code)]
pub fn ensure_audio_filename(title: &str, format: &str) -> String {
    let clean_fmt = format.trim_start_matches('.').to_lowercase();
    let (stem, ext) = split_stem_and_ext(title);
    if ext.is_some() {
        format!("{}.{}", stem, clean_fmt)
    } else {
        format!("{}.{}", title, clean_fmt)
    }
}

/// Context for formatting filename templates
#[derive(Debug, Clone, Default)]
pub struct FilenameContext<'a> {
    pub title: &'a str,
    pub ext: &'a str,
    pub resolution: Option<&'a str>,
    pub series: Option<&'a str>,
    pub index: Option<usize>,
    pub date: Option<&'a str>,
}

/// Converts Unix epoch days to standard YYYY-MM-DD string without external crates
pub fn format_epoch_days_date(epoch_days: u64) -> String {
    let z = epoch_days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Applies a filename template with variable substitutions and returns a clean, relative PathBuf supporting subfolders
pub fn apply_filename_template(template: &str, ctx: &FilenameContext) -> PathBuf {
    let clean_template = if template.trim().is_empty() {
        "{title}.{ext}"
    } else {
        template.trim()
    };

    let today = if let Some(d) = ctx.date {
        d.to_string()
    } else {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        format_epoch_days_date(secs / 86400)
    };

    // Strip existing media extension from title to avoid double extensions
    let (stem, _) = split_stem_and_ext(ctx.title);
    let title_clean = sanitize_filename(stem);
    let series_clean = ctx.series.map(sanitize_filename).unwrap_or_default();
    let res_clean = ctx.resolution.unwrap_or("");
    let ext_clean = ctx.ext.trim_start_matches('.');
    let index_str = ctx.index.map(|i| format!("{:02}", i)).unwrap_or_default();

    let mut result = clean_template.to_string();
    result = result.replace("{title}", &title_clean);
    result = result.replace("{ext}", ext_clean);
    result = result.replace("{date}", &today);

    if !series_clean.is_empty() {
        result = result.replace("{series}", &series_clean);
        result = result.replace("{playlist}", &series_clean);
    } else {
        result = result.replace("{series}/", "");
        result = result.replace("{series}\\", "");
        result = result.replace("{series}", "");
        result = result.replace("{playlist}/", "");
        result = result.replace("{playlist}\\", "");
        result = result.replace("{playlist}", "");
    }

    if !res_clean.is_empty() {
        result = result.replace("{resolution}", res_clean);
        result = result.replace("{quality}", res_clean);
    } else {
        result = result.replace("[{resolution}]", "");
        result = result.replace("({resolution})", "");
        result = result.replace("- {resolution}", "");
        result = result.replace("{resolution}", "");
        result = result.replace("[{quality}]", "");
        result = result.replace("({quality})", "");
        result = result.replace("- {quality}", "");
        result = result.replace("{quality}", "");
    }

    if !index_str.is_empty() {
        result = result.replace("{index}", &index_str);
        result = result.replace("{ep}", &index_str);
    } else {
        result = result.replace("{index} - ", "");
        result = result.replace("{index}-", "");
        result = result.replace("{index}", "");
        result = result.replace("{ep} - ", "");
        result = result.replace("{ep}-", "");
        result = result.replace("{ep}", "");
    }

    // Clean up double spaces or dangling brackets
    while result.contains("  ") {
        result = result.replace("  ", " ");
    }
    result = result.replace(" - .", ".");
    result = result.replace(" [].", ".");
    result = result.replace(" ().", ".");
    result = result.replace(" .", ".");

    // Ensure extension is present at the end
    if !ext_clean.is_empty() && !result.ends_with(&format!(".{}", ext_clean)) {
        result = format!("{}.{}", result.trim_end_matches('.'), ext_clean);
    }

    // Split into segments to support subfolders safely
    let segments: Vec<&str> = result
        .split(|c| c == '/' || c == '\\')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let mut final_path = PathBuf::new();
    for seg in segments {
        let clean = sanitize_filename(seg);
        if !clean.is_empty() {
            final_path.push(clean);
        }
    }

    if final_path.as_os_str().is_empty() {
        PathBuf::from(format!("downloaded_media.{}", ext_clean))
    } else {
        final_path
    }
}

/// Collision and resume resolution strategy when target file already exists or has an incomplete download
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum FileConflictPolicy {
    /// Auto-Resume or Auto-Rename (Default): Reuses incomplete .part/.ytdl downloads to resume byte streams.
    /// If an already-completed file exists, generates a numbered copy (1), (2), etc.
    AutoResumeOrRename = 0,
    /// Always appends (1), (2) on any existing file or partial download.
    AutoRename = 1,
    /// Overwrites existing files or removes old .part files to re-download from scratch.
    Overwrite = 2,
    /// Skips the download entirely if a completed target file already exists on disk.
    SkipExisting = 3,
}

impl Default for FileConflictPolicy {
    fn default() -> Self {
        FileConflictPolicy::AutoResumeOrRename
    }
}

impl FileConflictPolicy {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => FileConflictPolicy::AutoResumeOrRename,
            1 => FileConflictPolicy::AutoRename,
            2 => FileConflictPolicy::Overwrite,
            3 => FileConflictPolicy::SkipExisting,
            _ => FileConflictPolicy::AutoResumeOrRename,
        }
    }

    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

/// Destination path resolution result
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDestination {
    pub path: PathBuf,
    pub should_skip: bool,
    pub is_resuming: bool,
}

/// Resolves a full destination path using template formatting, creating subfolders if needed, and applying FileConflictPolicy
pub fn resolve_template_destination_with_policy(
    dir: &Path,
    template: &str,
    ctx: &FilenameContext,
    policy: FileConflictPolicy,
) -> Result<ResolvedDestination> {
    let rel_path = apply_filename_template(template, ctx);
    let full_path = dir.join(&rel_path);

    if let Some(parent) = full_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let file_name = full_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("media");
    let parent = full_path.parent().unwrap_or(dir);
    Ok(resolve_path_with_policy(parent, file_name, policy))
}

/// Resolves a full destination path using template formatting, creating subfolders if needed, and avoiding file collisions
#[allow(dead_code)]
pub fn resolve_template_destination(
    dir: &Path,
    template: &str,
    ctx: &FilenameContext,
) -> Result<PathBuf> {
    resolve_template_destination_with_policy(dir, template, ctx, FileConflictPolicy::AutoResumeOrRename)
        .map(|res| res.path)
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

/// Checks whether a completed file with this stem and extension exists on disk
pub fn has_completed_file(dir: &Path, stem: &str, ext: Option<&str>) -> bool {
    match ext {
        Some(ext_str) => {
            let clean_ext = ext_str.trim_start_matches('.');
            dir.join(format!("{}.{}", stem, clean_ext)).exists()
        }
        None => {
            if dir.join(stem).exists() {
                return true;
            }
            for &media_ext in MEDIA_EXTENSIONS {
                if dir.join(format!("{}.{}", stem, media_ext)).exists() {
                    return true;
                }
            }
            false
        }
    }
}

/// Checks whether an incomplete partial download (.part or .ytdl) exists on disk
pub fn has_incomplete_part(dir: &Path, stem: &str, ext: Option<&str>) -> bool {
    match ext {
        Some(ext_str) => {
            let clean_ext = ext_str.trim_start_matches('.');
            dir.join(format!("{}.{}.part", stem, clean_ext)).exists()
                || dir.join(format!("{}.{}.ytdl", stem, clean_ext)).exists()
        }
        None => {
            if dir.join(format!("{}.part", stem)).exists()
                || dir.join(format!("{}.ytdl", stem)).exists()
            {
                return true;
            }
            for &media_ext in MEDIA_EXTENSIONS {
                if dir.join(format!("{}.{}.part", stem, media_ext)).exists()
                    || dir.join(format!("{}.{}.ytdl", stem, media_ext)).exists()
                {
                    return true;
                }
            }
            false
        }
    }
}

/// Checks whether a file with this stem and extension (or common media extensions / .part files) exists
pub fn does_conflict_exist(dir: &Path, stem: &str, ext: Option<&str>) -> bool {
    has_completed_file(dir, stem, ext) || has_incomplete_part(dir, stem, ext)
}

/// Resolves a target destination path based on the selected FileConflictPolicy
pub fn resolve_path_with_policy(
    dir: &Path,
    filename: &str,
    policy: FileConflictPolicy,
) -> ResolvedDestination {
    let (stem, ext_opt) = split_stem_and_ext(filename);
    let ext_suffix = ext_opt.map(|e| format!(".{}", e)).unwrap_or_default();

    match policy {
        FileConflictPolicy::Overwrite => {
            // Overwrite: clear any partial .part file so download starts cleanly from 0
            if let Some(ext_str) = ext_opt {
                let clean_ext = ext_str.trim_start_matches('.');
                let part_file = dir.join(format!("{}.{}.part", stem, clean_ext));
                if part_file.exists() {
                    let _ = std::fs::remove_file(part_file);
                }
            } else {
                let part_file = dir.join(format!("{}.part", stem));
                if part_file.exists() {
                    let _ = std::fs::remove_file(part_file);
                }
            }
            ResolvedDestination {
                path: dir.join(filename),
                should_skip: false,
                is_resuming: false,
            }
        }
        FileConflictPolicy::SkipExisting => {
            if has_completed_file(dir, stem, ext_opt) {
                ResolvedDestination {
                    path: dir.join(filename),
                    should_skip: true,
                    is_resuming: false,
                }
            } else {
                let is_resuming = has_incomplete_part(dir, stem, ext_opt);
                ResolvedDestination {
                    path: dir.join(filename),
                    should_skip: false,
                    is_resuming,
                }
            }
        }
        FileConflictPolicy::AutoRename => {
            let unique_path = resolve_unique_path(dir, filename);
            ResolvedDestination {
                path: unique_path,
                should_skip: false,
                is_resuming: false,
            }
        }
        FileConflictPolicy::AutoResumeOrRename => {
            // If incomplete .part exists and complete file does NOT exist: resume it!
            if has_incomplete_part(dir, stem, ext_opt) && !has_completed_file(dir, stem, ext_opt) {
                return ResolvedDestination {
                    path: dir.join(filename),
                    should_skip: false,
                    is_resuming: true,
                };
            }

            // If neither complete file nor incomplete part exists: fresh file!
            if !has_completed_file(dir, stem, ext_opt) && !has_incomplete_part(dir, stem, ext_opt) {
                return ResolvedDestination {
                    path: dir.join(filename),
                    should_skip: false,
                    is_resuming: false,
                };
            }

            // Completed file exists! We must find an unused or partial-resumable copy: (1), (2)...
            let (base_stem, mut counter) = parse_stem_and_index(stem);
            loop {
                let candidate_stem = format!("{} ({})", base_stem, counter);
                let candidate_name = format!("{}{}", candidate_stem, ext_suffix);

                if has_incomplete_part(dir, &candidate_stem, ext_opt)
                    && !has_completed_file(dir, &candidate_stem, ext_opt)
                {
                    return ResolvedDestination {
                        path: dir.join(candidate_name),
                        should_skip: false,
                        is_resuming: true,
                    };
                }

                if !has_completed_file(dir, &candidate_stem, ext_opt)
                    && !has_incomplete_part(dir, &candidate_stem, ext_opt)
                {
                    return ResolvedDestination {
                        path: dir.join(candidate_name),
                        should_skip: false,
                        is_resuming: false,
                    };
                }

                counter += 1;
            }
        }
    }
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

/// Plays or previews a streaming media URL (e.g. mp4, m3u8, or web link) with the system default player or browser
#[allow(dead_code)]
pub fn play_stream_url(stream_url: &str, referer: Option<&str>) -> Result<()> {
    let trimmed = stream_url.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    tracing::info!("Launching stream preview: {} (referer: {:?})", trimmed, referer);

    #[cfg(target_os = "macos")]
    {
        // 1. If mpv CLI is installed, use mpv with header forwarding
        if let Ok(_) = std::process::Command::new("mpv")
            .arg(trimmed)
            .args(if let Some(r) = referer {
                if crate::downloader::extractor::is_streaming_platform(trimmed) {
                    vec![]
                } else {
                    vec![format!("--http-header-fields=Referer: {}", r)]
                }
            } else {
                vec![]
            })
            .spawn()
        {
            return Ok(());
        }

        // 2. If IINA is installed on macOS
        if std::path::Path::new("/Applications/IINA.app").exists() {
            if let Ok(_) = std::process::Command::new("open")
                .args(["-a", "IINA", trimmed])
                .spawn()
            {
                return Ok(());
            }
        }

        // 3. If VLC is installed on macOS
        if std::path::Path::new("/Applications/VLC.app").exists() {
            if let Ok(_) = std::process::Command::new("open")
                .args(["-a", "VLC", trimmed])
                .spawn()
            {
                return Ok(());
            }
        }

        // 4. Default: launch system handler / browser (QuickTime / Safari / Chrome)
        let _ = std::process::Command::new("open").arg(trimmed).spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd").args(["/C", "start", "", trimmed]).spawn();
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(trimmed).spawn();
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetOrganizationMode {
    /// Dedicated folder per video: `<parent>/<stem>/[ <stem>.<ext>, Subtitles/, Thumbnails/, Audio/ ]`
    /// 100% compatible with VLC, IINA, MPV, Plex, Jellyfin, and Infuse out-of-the-box.
    DedicatedVideoFolder = 0,
    /// Per-video subtitles subfolder: `<parent>/Subtitles/<stem>/[ <stem>.<lang>.<ext> ]`
    PerVideoSubtitles = 1,
    /// Shared flat subtitles folder: `<parent>/Subtitles/[ <stem>.<lang>.<ext> ]`
    SharedSubtitles = 2,
}

impl From<u8> for AssetOrganizationMode {
    fn from(val: u8) -> Self {
        match val {
            1 => AssetOrganizationMode::PerVideoSubtitles,
            2 => AssetOrganizationMode::SharedSubtitles,
            _ => AssetOrganizationMode::DedicatedVideoFolder,
        }
    }
}

/// Organizes auxiliary media assets (subtitles, thumbnails/posters, and extra audio tracks)
/// associated with a video according to the selected `AssetOrganizationMode`.
/// Returns `(number_of_assets_moved, final_media_file_path)`.
pub async fn organize_media_assets_with_mode(
    media_path: &Path,
    mode: AssetOrganizationMode,
) -> Result<(usize, PathBuf)> {
    let parent = match media_path.parent() {
        Some(p) if p.is_dir() => p,
        _ => return Ok((0, media_path.to_path_buf())),
    };

    let file_stem = match media_path.file_stem().and_then(|s| s.to_str()) {
        Some(s) if !s.is_empty() => s,
        _ => return Ok((0, media_path.to_path_buf())),
    };

    let file_name = match media_path.file_name() {
        Some(n) => n,
        _ => return Ok((0, media_path.to_path_buf())),
    };

    let is_video = media_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| ["mp4", "webm", "mkv", "avi", "mov", "flv", "ts"].contains(&e.to_lowercase().as_str()))
        .unwrap_or(false);

    let sub_exts = ["vtt", "srt", "ass", "ssa", "sub", "sbv", "lrc"];
    let thumb_exts = ["jpg", "jpeg", "png", "webp", "avif"];
    let audio_exts = ["m4a", "aac", "opus", "mp3", "flac", "wav", "ogg"];

    let mut matching_subs = Vec::new();
    let mut matching_thumbs = Vec::new();
    let mut matching_audio = Vec::new();

    let mut read_dir = match tokio::fs::read_dir(parent).await {
        Ok(rd) => rd,
        Err(_) => return Ok((0, media_path.to_path_buf())),
    };

    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let p = entry.path();
        if !p.is_file() || p == media_path {
            continue;
        }

        let name = match p.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };

        let lower = name.to_lowercase();
        let stem_prefix = format!("{}.", file_stem);
        let stem_dash = format!("{}-", file_stem);

        // 1. Subtitles: "<stem>.<lang>.<ext>"
        let is_sub = sub_exts.iter().any(|&ext| lower.ends_with(&format!(".{}", ext)));
        if is_sub && (name.starts_with(&stem_prefix) || name.starts_with(&stem_dash)) {
            matching_subs.push(p);
            continue;
        }

        // 2. Thumbnails & Art: "<stem>.<ext>" or "<stem>-thumb.<ext>" or "<stem>-poster.<ext>"
        let is_thumb = thumb_exts.iter().any(|&ext| lower.ends_with(&format!(".{}", ext)));
        if is_thumb && (name.starts_with(&stem_prefix) || name.starts_with(&stem_dash)) {
            matching_thumbs.push(p);
            continue;
        }

        // 3. Separate Audio Tracks: (only when main file is video) "<stem>.<lang>.<ext>"
        if is_video {
            let is_audio = audio_exts.iter().any(|&ext| lower.ends_with(&format!(".{}", ext)));
            if is_audio && (name.starts_with(&stem_prefix) || name.starts_with(&stem_dash)) {
                matching_audio.push(p);
                continue;
            }
        }
    }

    if matching_subs.is_empty() && matching_thumbs.is_empty() && matching_audio.is_empty() {
        return Ok((0, media_path.to_path_buf()));
    }

    let mut total_moved = 0;
    let mut final_media_path = media_path.to_path_buf();

    match mode {
        AssetOrganizationMode::DedicatedVideoFolder => {
            let is_already_in_dedicated_folder = parent
                .file_name()
                .and_then(|s| s.to_str())
                .map(|name| name == file_stem)
                .unwrap_or(false);

            let (target_root, media_dest) = if is_already_in_dedicated_folder {
                (parent.to_path_buf(), media_path.to_path_buf())
            } else {
                let video_dir = parent.join(file_stem);
                tokio::fs::create_dir_all(&video_dir).await?;
                let target_media = video_dir.join(file_name);
                if media_path.exists() && media_path != target_media {
                    if tokio::fs::rename(media_path, &target_media).await.is_err() {
                        tokio::fs::copy(media_path, &target_media).await?;
                        let _ = tokio::fs::remove_file(media_path).await;
                    }
                }
                (video_dir, target_media)
            };
            final_media_path = media_dest;

            // Move Subtitles into <target_root>/Subtitles/
            if !matching_subs.is_empty() {
                let subs_dir = target_root.join("Subtitles");
                tokio::fs::create_dir_all(&subs_dir).await?;
                for sub in matching_subs {
                    if let Some(name) = sub.file_name() {
                        let target = subs_dir.join(name);
                        if tokio::fs::rename(&sub, &target).await.is_ok() {
                            total_moved += 1;
                        } else if tokio::fs::copy(&sub, &target).await.is_ok() {
                            let _ = tokio::fs::remove_file(&sub).await;
                            total_moved += 1;
                        }
                    }
                }
            }

            // Move Thumbnails into <target_root>/Thumbnails/
            if !matching_thumbs.is_empty() {
                let thumbs_dir = target_root.join("Thumbnails");
                tokio::fs::create_dir_all(&thumbs_dir).await?;
                for thumb in matching_thumbs {
                    if let Some(name) = thumb.file_name() {
                        let target = thumbs_dir.join(name);
                        if tokio::fs::rename(&thumb, &target).await.is_ok() {
                            total_moved += 1;
                        } else if tokio::fs::copy(&thumb, &target).await.is_ok() {
                            let _ = tokio::fs::remove_file(&thumb).await;
                            total_moved += 1;
                        }
                    }
                }
            }

            // Move Audio into <target_root>/Audio/
            if !matching_audio.is_empty() {
                let audio_dir = target_root.join("Audio");
                tokio::fs::create_dir_all(&audio_dir).await?;
                for audio in matching_audio {
                    if let Some(name) = audio.file_name() {
                        let target = audio_dir.join(name);
                        if tokio::fs::rename(&audio, &target).await.is_ok() {
                            total_moved += 1;
                        } else if tokio::fs::copy(&audio, &target).await.is_ok() {
                            let _ = tokio::fs::remove_file(&audio).await;
                            total_moved += 1;
                        }
                    }
                }
            }
        }
        AssetOrganizationMode::PerVideoSubtitles => {
            // Move Subtitles into <parent>/Subtitles/<file_stem>/
            if !matching_subs.is_empty() {
                let subs_dir = parent.join("Subtitles").join(file_stem);
                tokio::fs::create_dir_all(&subs_dir).await?;
                for sub in matching_subs {
                    if let Some(name) = sub.file_name() {
                        let target = subs_dir.join(name);
                        if tokio::fs::rename(&sub, &target).await.is_ok() {
                            total_moved += 1;
                        } else if tokio::fs::copy(&sub, &target).await.is_ok() {
                            let _ = tokio::fs::remove_file(&sub).await;
                            total_moved += 1;
                        }
                    }
                }
            }

            // Move Thumbnails into <parent>/Thumbnails/
            if !matching_thumbs.is_empty() {
                let thumbs_dir = parent.join("Thumbnails");
                tokio::fs::create_dir_all(&thumbs_dir).await?;
                for thumb in matching_thumbs {
                    if let Some(name) = thumb.file_name() {
                        let target = thumbs_dir.join(name);
                        if tokio::fs::rename(&thumb, &target).await.is_ok() {
                            total_moved += 1;
                        } else if tokio::fs::copy(&thumb, &target).await.is_ok() {
                            let _ = tokio::fs::remove_file(&thumb).await;
                            total_moved += 1;
                        }
                    }
                }
            }

            // Move Audio into <parent>/Audio/
            if !matching_audio.is_empty() {
                let audio_dir = parent.join("Audio");
                tokio::fs::create_dir_all(&audio_dir).await?;
                for audio in matching_audio {
                    if let Some(name) = audio.file_name() {
                        let target = audio_dir.join(name);
                        if tokio::fs::rename(&audio, &target).await.is_ok() {
                            total_moved += 1;
                        } else if tokio::fs::copy(&audio, &target).await.is_ok() {
                            let _ = tokio::fs::remove_file(&audio).await;
                            total_moved += 1;
                        }
                    }
                }
            }
        }
        AssetOrganizationMode::SharedSubtitles => {
            // Move Subtitles into <parent>/Subtitles/
            if !matching_subs.is_empty() {
                let subs_dir = parent.join("Subtitles");
                tokio::fs::create_dir_all(&subs_dir).await?;
                for sub in matching_subs {
                    if let Some(name) = sub.file_name() {
                        let target = subs_dir.join(name);
                        if tokio::fs::rename(&sub, &target).await.is_ok() {
                            total_moved += 1;
                        } else if tokio::fs::copy(&sub, &target).await.is_ok() {
                            let _ = tokio::fs::remove_file(&sub).await;
                            total_moved += 1;
                        }
                    }
                }
            }

            // Move Thumbnails into <parent>/Thumbnails/
            if !matching_thumbs.is_empty() {
                let thumbs_dir = parent.join("Thumbnails");
                tokio::fs::create_dir_all(&thumbs_dir).await?;
                for thumb in matching_thumbs {
                    if let Some(name) = thumb.file_name() {
                        let target = thumbs_dir.join(name);
                        if tokio::fs::rename(&thumb, &target).await.is_ok() {
                            total_moved += 1;
                        } else if tokio::fs::copy(&thumb, &target).await.is_ok() {
                            let _ = tokio::fs::remove_file(&thumb).await;
                            total_moved += 1;
                        }
                    }
                }
            }

            // Move Audio into <parent>/Audio/
            if !matching_audio.is_empty() {
                let audio_dir = parent.join("Audio");
                tokio::fs::create_dir_all(&audio_dir).await?;
                for audio in matching_audio {
                    if let Some(name) = audio.file_name() {
                        let target = audio_dir.join(name);
                        if tokio::fs::rename(&audio, &target).await.is_ok() {
                            total_moved += 1;
                        } else if tokio::fs::copy(&audio, &target).await.is_ok() {
                            let _ = tokio::fs::remove_file(&audio).await;
                            total_moved += 1;
                        }
                    }
                }
            }
        }
    }

    if total_moved > 0 {
        info!(
            "Organized {} media assets for '{}' (mode: {:?}) -> {:?}",
            total_moved, file_stem, mode, final_media_path
        );
    }

    Ok((total_moved, final_media_path))
}

/// Compatibility wrapper for `organize_media_assets_with_mode` using the default dedicated folder mode
#[allow(dead_code)]
pub async fn organize_media_assets(media_path: &Path) -> Result<usize> {
    let (count, _) = organize_media_assets_with_mode(media_path, AssetOrganizationMode::DedicatedVideoFolder).await?;
    Ok(count)
}

/// Compatibility alias for `organize_media_assets`
#[allow(dead_code)]
pub async fn organize_subtitles(media_path: &Path) -> Result<usize> {
    organize_media_assets(media_path).await
}

/// Organizes all subtitle files in the specified directory into a "Subtitles" subfolder.
#[allow(dead_code)]
pub async fn organize_all_subtitles_in_dir(dir: &Path) -> Result<usize> {
    if !dir.is_dir() {
        return Ok(0);
    }

    let sub_exts = ["vtt", "srt", "ass", "ssa", "sub", "sbv", "lrc"];
    let mut matching_subs = Vec::new();

    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(_) => return Ok(0),
    };

    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }

        let name = match p.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };

        let is_sub = sub_exts.iter().any(|&ext| name.to_lowercase().ends_with(&format!(".{}", ext)));
        if is_sub {
            matching_subs.push(p);
        }
    }

    if matching_subs.is_empty() {
        return Ok(0);
    }

    let subs_dir = dir.join("Subtitles");
    tokio::fs::create_dir_all(&subs_dir).await?;

    let mut moved = 0;
    for sub in matching_subs {
        if let Some(file_name) = sub.file_name() {
            let target = subs_dir.join(file_name);
            if tokio::fs::rename(&sub, &target).await.is_ok() {
                moved += 1;
            } else if tokio::fs::copy(&sub, &target).await.is_ok() {
                let _ = tokio::fs::remove_file(&sub).await;
                moved += 1;
            }
        }
    }

    info!("Organized {} subtitle files in {:?} into {:?}", moved, dir, subs_dir);
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_organize_subtitles() {
        let temp_dir = std::env::temp_dir().join(format!("nvd_sub_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        // --- 1. Test Mode 0: DedicatedVideoFolder (Approach 1) ---
        let video = temp_dir.join("My Movie.mp4");
        tokio::fs::write(&video, b"video content").await.unwrap();

        let sub_en = temp_dir.join("My Movie.en.vtt");
        let sub_es = temp_dir.join("My Movie.es.srt");
        let thumb = temp_dir.join("My Movie.webp");
        let audio = temp_dir.join("My Movie.en.m4a");
        let other_sub = temp_dir.join("Other Video.fr.vtt");

        tokio::fs::write(&sub_en, b"WEBVTT").await.unwrap();
        tokio::fs::write(&sub_es, b"1\n00:00:01 --> 00:00:02\nHola").await.unwrap();
        tokio::fs::write(&thumb, b"WEBPIMAGE").await.unwrap();
        tokio::fs::write(&audio, b"AUDIODATA").await.unwrap();
        tokio::fs::write(&other_sub, b"WEBVTT").await.unwrap();

        let (count, new_video_path) = organize_media_assets_with_mode(&video, AssetOrganizationMode::DedicatedVideoFolder).await.unwrap();
        assert_eq!(count, 4); // 2 subs + 1 thumb + 1 audio

        let movie_folder = temp_dir.join("My Movie");
        assert_eq!(new_video_path, movie_folder.join("My Movie.mp4"));
        assert!(new_video_path.exists());

        let subs_dir = movie_folder.join("Subtitles");
        let thumbs_dir = movie_folder.join("Thumbnails");
        let audio_dir = movie_folder.join("Audio");

        assert!(subs_dir.join("My Movie.en.vtt").exists());
        assert!(subs_dir.join("My Movie.es.srt").exists());
        assert!(thumbs_dir.join("My Movie.webp").exists());
        assert!(audio_dir.join("My Movie.en.m4a").exists());

        assert!(!temp_dir.join("My Movie.mp4").exists());
        assert!(!temp_dir.join("My Movie.en.vtt").exists());
        assert!(!temp_dir.join("My Movie.webp").exists());
        assert!(!temp_dir.join("My Movie.en.m4a").exists());
        assert!(other_sub.exists()); // Other video's asset untouched

        // --- 2. Test Mode 1: PerVideoSubtitles ---
        let v2 = temp_dir.join("Show S01E02.mp4");
        let v2_sub = temp_dir.join("Show S01E02.ja.vtt");
        tokio::fs::write(&v2, b"v2 content").await.unwrap();
        tokio::fs::write(&v2_sub, b"WEBVTT").await.unwrap();

        let (c2, v2_path) = organize_media_assets_with_mode(&v2, AssetOrganizationMode::PerVideoSubtitles).await.unwrap();
        assert_eq!(c2, 1);
        assert_eq!(v2_path, v2);
        assert!(temp_dir.join("Subtitles").join("Show S01E02").join("Show S01E02.ja.vtt").exists());

        // --- 3. Test Mode 2: SharedSubtitles ---
        let v3 = temp_dir.join("Show S01E03.mp4");
        let v3_sub = temp_dir.join("Show S01E03.zh.vtt");
        tokio::fs::write(&v3, b"v3 content").await.unwrap();
        tokio::fs::write(&v3_sub, b"WEBVTT").await.unwrap();

        let (c3, v3_path) = organize_media_assets_with_mode(&v3, AssetOrganizationMode::SharedSubtitles).await.unwrap();
        assert_eq!(c3, 1);
        assert_eq!(v3_path, v3);
        assert!(temp_dir.join("Subtitles").join("Show S01E03.zh.vtt").exists());

        // Organize all remaining
        let all_count = organize_all_subtitles_in_dir(&temp_dir).await.unwrap();
        assert_eq!(all_count, 1);
        assert!(temp_dir.join("Subtitles").join("Other Video.fr.vtt").exists());

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

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

    #[test]
    fn test_parse_links_from_text() {
        let sample = r#"
#EXTM3U
#EXTINF:-1,Sample Stream
https://example.com/live/stream.m3u8
// comment line
Check this link: https://www.youtube.com/watch?v=dQw4w9WgXcQ.
"https://vimeo.com/12345678"
[Markdown Link](https://dailymotion.com/video/x7xyz)
https://example.com/live/stream.m3u8
http://bilibili.com/video/BV1xx411c7mD, extra text
"#;

        let links = parse_links_from_text(sample);
        assert_eq!(links.len(), 5);
        assert_eq!(links[0], "https://example.com/live/stream.m3u8");
        assert_eq!(links[1], "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(links[2], "https://vimeo.com/12345678");
        assert_eq!(links[3], "https://dailymotion.com/video/x7xyz");
        assert_eq!(links[4], "http://bilibili.com/video/BV1xx411c7mD");
    }

    #[test]
    fn test_format_history_csv() {
        use uuid::Uuid;
        let id1 = Uuid::new_v4();
        let record1 = crate::database::HistoryRecord {
            id: id1,
            url: "https://example.com/video,test".to_string(),
            title: "My \"Special\" Video, Part 1".to_string(),
            filename: "my_video.mp4".to_string(),
            output_path: "/Downloads/my_video.mp4".to_string(),
            status: "Completed".to_string(),
            total_size: Some(10485760),
            downloaded_size: 10485760,
            error: None,
            created_at: "2026-09-27T10:00:00Z".to_string(),
            completed_at: Some("2026-09-27T10:05:00Z".to_string()),
        };

        let csv = format_history_csv(&[record1]);
        assert!(csv.contains("ID,Title,URL,Filename,Output Path"));
        // Quotes and comma escaped
        assert!(csv.contains("\"My \"\"Special\"\" Video, Part 1\""));
        assert!(csv.contains("\"https://example.com/video,test\""));
        assert!(csv.contains("10485760"));
        assert!(csv.contains("Completed"));
    }

    #[test]
    fn test_ensure_audio_filename() {
        assert_eq!(ensure_audio_filename("Concert Live.mp4", "mp3"), "Concert Live.mp3");
        assert_eq!(ensure_audio_filename("Podcast Episode.webm", "m4a"), "Podcast Episode.m4a");
        assert_eq!(ensure_audio_filename("Audio Track", "flac"), "Audio Track.flac");
        assert_eq!(ensure_audio_filename("Classical Symphony.mp3", "mp3"), "Classical Symphony.mp3");
        assert_eq!(ensure_audio_filename("Video.mkv", ".opus"), "Video.opus");
    }

    #[test]
    fn test_apply_filename_template() {
        // 1. Basic title and extension
        let ctx1 = FilenameContext {
            title: "Nature Documentary.mp4",
            ext: "mp4",
            resolution: None,
            series: None,
            index: None,
            date: Some("2026-09-27"),
        };
        assert_eq!(
            apply_filename_template("{title}.{ext}", &ctx1),
            PathBuf::from("Nature Documentary.mp4")
        );

        // 2. Resolution tag present
        let ctx2 = FilenameContext {
            title: "Cyberpunk 2077",
            ext: "mp4",
            resolution: Some("1080p"),
            series: None,
            index: None,
            date: Some("2026-09-27"),
        };
        assert_eq!(
            apply_filename_template("{title} [{resolution}].{ext}", &ctx2),
            PathBuf::from("Cyberpunk 2077 [1080p].mp4")
        );

        // 3. Resolution tag absent (empty brackets cleanly eliminated)
        let ctx3 = FilenameContext {
            title: "Anime Episode",
            ext: "mkv",
            resolution: None,
            series: None,
            index: None,
            date: Some("2026-09-27"),
        };
        assert_eq!(
            apply_filename_template("{title} [{resolution}].{ext}", &ctx3),
            PathBuf::from("Anime Episode.mkv")
        );

        // 4. Series subfolder with index
        let ctx4 = FilenameContext {
            title: "City of Dreams",
            ext: "mp4",
            resolution: Some("1080p"),
            series: Some("Edgerunners"),
            index: Some(1),
            date: Some("2026-09-27"),
        };
        let p4 = apply_filename_template("{series}/{index} - {title}.{ext}", &ctx4);
        assert_eq!(p4, PathBuf::from("Edgerunners").join("01 - City of Dreams.mp4"));

        // 5. Date prefix
        let p5 = apply_filename_template("[{date}] {title}.{ext}", &ctx1);
        assert_eq!(p5, PathBuf::from("[2026-09-27] Nature Documentary.mp4"));
    }

    #[test]
    fn test_file_conflict_policies() {
        let temp_dir = std::env::temp_dir().join(format!("test_conflict_policies_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Fresh file - all policies return base name
        let r_fresh = resolve_path_with_policy(&temp_dir, "video.mp4", FileConflictPolicy::AutoResumeOrRename);
        assert_eq!(r_fresh.path, temp_dir.join("video.mp4"));
        assert!(!r_fresh.is_resuming);
        assert!(!r_fresh.should_skip);

        // 2. Partial file exists (video.mp4.part)
        std::fs::write(temp_dir.join("video.mp4.part"), b"partial 50%").unwrap();

        // AutoResumeOrRename: reuses video.mp4 to resume downloading!
        let r_resume = resolve_path_with_policy(&temp_dir, "video.mp4", FileConflictPolicy::AutoResumeOrRename);
        assert_eq!(r_resume.path, temp_dir.join("video.mp4"));
        assert!(r_resume.is_resuming);
        assert!(!r_resume.should_skip);

        // AutoRename: forces (1) because part file counts as conflict
        let r_rename = resolve_path_with_policy(&temp_dir, "video.mp4", FileConflictPolicy::AutoRename);
        assert_eq!(r_rename.path, temp_dir.join("video (1).mp4"));
        assert!(!r_rename.is_resuming);

        // SkipExisting: since file is NOT yet complete, it resumes the partial download
        let r_skip_part = resolve_path_with_policy(&temp_dir, "video.mp4", FileConflictPolicy::SkipExisting);
        assert_eq!(r_skip_part.path, temp_dir.join("video.mp4"));
        assert!(r_skip_part.is_resuming);
        assert!(!r_skip_part.should_skip);

        // 3. Completed file exists (video.mp4)
        std::fs::remove_file(temp_dir.join("video.mp4.part")).unwrap();
        std::fs::write(temp_dir.join("video.mp4"), b"complete video").unwrap();

        // SkipExisting: should_skip is true
        let r_skip_done = resolve_path_with_policy(&temp_dir, "video.mp4", FileConflictPolicy::SkipExisting);
        assert_eq!(r_skip_done.path, temp_dir.join("video.mp4"));
        assert!(r_skip_done.should_skip);

        // AutoResumeOrRename: complete file exists, so it advances to (1)
        let r_resume_after_complete = resolve_path_with_policy(&temp_dir, "video.mp4", FileConflictPolicy::AutoResumeOrRename);
        assert_eq!(r_resume_after_complete.path, temp_dir.join("video (1).mp4"));
        assert!(!r_resume_after_complete.is_resuming);

        // 4. Incomplete part for copy (1) exists
        std::fs::write(temp_dir.join("video (1).mp4.part"), b"partial copy 1").unwrap();
        let r_resume_copy1 = resolve_path_with_policy(&temp_dir, "video.mp4", FileConflictPolicy::AutoResumeOrRename);
        assert_eq!(r_resume_copy1.path, temp_dir.join("video (1).mp4"));
        assert!(r_resume_copy1.is_resuming);

        // 5. Overwrite: clears any .part and returns base target path
        let r_overwrite = resolve_path_with_policy(&temp_dir, "video (1).mp4", FileConflictPolicy::Overwrite);
        assert_eq!(r_overwrite.path, temp_dir.join("video (1).mp4"));
        assert!(!r_overwrite.is_resuming);
        assert!(!r_overwrite.should_skip);
        assert!(!temp_dir.join("video (1).mp4.part").exists()); // Cleaned up

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}


