use std::path::{Path, PathBuf};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub html_url: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
    pub draft: bool,
    #[serde(default)]
    pub assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub title: String,
    pub release_notes: String,
    pub release_url: String,
    pub published_at: String,
    pub direct_download_url: Option<String>,
    pub asset_name: Option<String>,
    pub asset_size: Option<u64>,
    pub bundle_download_url: Option<String>,
    pub bundle_name: Option<String>,
}

/// Parses a semver-style version string (e.g. "v0.2.1", "0.3.0") into a numeric (major, minor, patch) tuple
pub fn parse_version_tuple(v: &str) -> (u64, u64, u64) {
    let clean = v.trim().trim_start_matches('v');
    let mut parts = clean.split('.');
    let major = parts
        .next()
        .and_then(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().ok())
        .unwrap_or(0);
    let minor = parts
        .next()
        .and_then(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().ok())
        .unwrap_or(0);
    let patch = parts
        .next()
        .and_then(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().ok())
        .unwrap_or(0);
    (major, minor, patch)
}

/// Returns true if `remote` is strictly newer than `current`
pub fn is_newer_version(remote: &str, current: &str) -> bool {
    let remote_t = parse_version_tuple(remote);
    let cur_t = parse_version_tuple(current);
    remote_t > cur_t
}

/// Queries the GitHub Releases API for the latest release of NativeVideoDownloader
pub async fn fetch_latest_release() -> Result<GitHubRelease> {
    let url = "https://api.github.com/repos/horrorbringer/NativeVideoDownloader/releases/latest";
    let client = reqwest::Client::builder()
        .user_agent("NativeVideoDownloader")
        .build()
        .map_err(|e| AppError::Generic(format!("Failed to create HTTP client: {}", e)))?;

    let res = client
        .get(url)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| AppError::Generic(format!("Could not connect to GitHub Releases API: {}", e)))?;

    if !res.status().is_success() {
        return Err(AppError::Generic(format!(
            "GitHub Releases API returned HTTP status {}: {}",
            res.status(),
            res.text().await.unwrap_or_default()
        )));
    }

    let body_bytes = res
        .bytes()
        .await
        .map_err(|e| AppError::Generic(format!("Failed to read release response body: {}", e)))?;

    let release: GitHubRelease = serde_json::from_slice(&body_bytes)
        .map_err(|e| AppError::Generic(format!("Failed to parse release JSON from GitHub: {}", e)))?;

    Ok(release)
}

/// Checks whether an update is available comparing the repository's latest release with `current_version`
pub async fn check_for_updates(current_version: &str) -> Result<Option<ReleaseInfo>> {
    let release = fetch_latest_release().await?;

    if release.draft {
        return Ok(None);
    }

    if !is_newer_version(&release.tag_name, current_version) {
        info!(
            "App is on current version {} (latest is {})",
            current_version, release.tag_name
        );
        return Ok(None);
    }

    info!("New version available: {} (current: {})", release.tag_name, current_version);

    let os = std::env::consts::OS; // "macos", "windows", "linux"
    let mut direct_download_url = None;
    let mut asset_name = None;
    let mut asset_size = None;
    let mut bundle_download_url = None;
    let mut bundle_name = None;

    for a in &release.assets {
        let lower = a.name.to_lowercase();
        match os {
            "macos" => {
                if a.name == "native_video_downloader" {
                    direct_download_url = Some(a.browser_download_url.clone());
                    asset_name = Some(a.name.clone());
                    asset_size = Some(a.size);
                } else if lower.ends_with(".dmg") || (lower.contains("macos") && lower.ends_with(".zip")) {
                    bundle_download_url = Some(a.browser_download_url.clone());
                    bundle_name = Some(a.name.clone());
                }
            }
            "windows" => {
                if a.name == "native_video_downloader.exe" {
                    direct_download_url = Some(a.browser_download_url.clone());
                    asset_name = Some(a.name.clone());
                    asset_size = Some(a.size);
                } else if lower.contains("windows") && lower.ends_with(".zip") {
                    bundle_download_url = Some(a.browser_download_url.clone());
                    bundle_name = Some(a.name.clone());
                }
            }
            _ => {
                if a.name == "native_video_downloader" {
                    direct_download_url = Some(a.browser_download_url.clone());
                    asset_name = Some(a.name.clone());
                    asset_size = Some(a.size);
                } else if lower.contains("linux") && (lower.ends_with(".tar.gz") || lower.ends_with(".zip")) {
                    bundle_download_url = Some(a.browser_download_url.clone());
                    bundle_name = Some(a.name.clone());
                }
            }
        }
    }

    // Fallback: if no direct single binary was matched, check if any asset matches OS
    if direct_download_url.is_none() && bundle_download_url.is_some() {
        direct_download_url = bundle_download_url.clone();
        asset_name = bundle_name.clone();
    }

    let title = release.name.unwrap_or_else(|| release.tag_name.clone());
    let release_notes = release.body.unwrap_or_else(|| "General bug fixes and performance improvements.".to_string());
    let published_at = release.published_at.unwrap_or_default();

    Ok(Some(ReleaseInfo {
        tag_name: release.tag_name,
        title,
        release_notes,
        release_url: release.html_url,
        published_at,
        direct_download_url,
        asset_name,
        asset_size,
        bundle_download_url,
        bundle_name,
    }))
}

/// Downloads the release asset with live progress and safely replaces the current running binary in-place
pub async fn install_binary_update<F>(
    download_url: &str,
    on_progress: F,
) -> Result<PathBuf>
where
    F: Fn(f32, u64, u64) + Send + Sync + 'static,
{
    let current_exe = std::env::current_exe()
        .map_err(|e| AppError::Generic(format!("Failed to determine current running executable path: {}", e)))?;

    let parent_dir = current_exe.parent().unwrap_or(Path::new("."));
    let temp_download_path = parent_dir.join(format!(
        ".native_video_downloader_update_{}.tmp",
        uuid::Uuid::new_v4()
    ));

    info!(
        "Downloading update from {} to temporary path {:?}",
        download_url, temp_download_path
    );

    let client = reqwest::Client::builder()
        .user_agent("NativeVideoDownloader")
        .build()
        .map_err(|e| AppError::Generic(format!("Failed to create HTTP client: {}", e)))?;

    let res = client
        .get(download_url)
        .send()
        .await
        .map_err(|e| AppError::Generic(format!("Failed to connect to update server: {}", e)))?;

    if !res.status().is_success() {
        return Err(AppError::Generic(format!(
            "Update download failed with HTTP status: {}",
            res.status()
        )));
    }

    let total_bytes = res.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut stream = res.bytes_stream();

    let mut file = tokio::fs::File::create(&temp_download_path)
        .await
        .map_err(|e| AppError::Generic(format!("Failed to create temporary update file: {}", e)))?;

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res
            .map_err(|e| AppError::Generic(format!("Error while downloading update stream: {}", e)))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| AppError::Generic(format!("Failed to write update chunk to disk: {}", e)))?;

        downloaded += chunk.len() as u64;
        let progress = if total_bytes > 0 {
            (downloaded as f32 / total_bytes as f32).clamp(0.0, 1.0)
        } else {
            0.5
        };
        on_progress(progress, downloaded, total_bytes);
    }

    file.flush()
        .await
        .map_err(|e| AppError::Generic(format!("Failed to flush update file: {}", e)))?;
    drop(file);

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&temp_download_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&temp_download_path, perms);
        }
    }

    // Safely swap with running executable
    let backup_path = parent_dir.join(format!(
        "{}.old_{}",
        current_exe.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));

    info!("Backing up current executable to {:?}", backup_path);

    // 1. Rename current running exe to backup
    std::fs::rename(&current_exe, &backup_path).map_err(|e| {
        AppError::Generic(format!(
            "Failed to rename current executable for update (check write permissions): {}",
            e
        ))
    })?;

    // 2. Move new exe to current exe location
    if let Err(e) = std::fs::rename(&temp_download_path, &current_exe) {
        // Rollback if move failed
        let _ = std::fs::rename(&backup_path, &current_exe);
        let _ = std::fs::remove_file(&temp_download_path);
        return Err(AppError::Generic(format!(
            "Failed to install new executable into place: {}",
            e
        )));
    }

    // 3. Remove backup if possible
    let _ = std::fs::remove_file(&backup_path);

    info!(
        "Successfully replaced binary with updated version at {:?}",
        current_exe
    );
    Ok(current_exe)
}

/// Restarts the application into the updated executable
pub fn restart_application() -> Result<()> {
    let current_exe = std::env::current_exe()
        .map_err(|e| AppError::Generic(format!("Failed to determine current executable path: {}", e)))?;

    info!("Relaunching application from {:?}", current_exe);

    #[cfg(target_os = "macos")]
    {
        let exe_str = current_exe.to_string_lossy();
        if let Some(app_idx) = exe_str.find(".app") {
            let app_path = &exe_str[..app_idx + 4];
            info!("Launching macOS .app bundle at {}", app_path);
            let _ = std::process::Command::new("open")
                .arg("-n")
                .arg(app_path)
                .spawn();
            std::process::exit(0);
        }
    }

    std::process::Command::new(&current_exe)
        .args(std::env::args().skip(1))
        .spawn()
        .map_err(|e| AppError::Generic(format!("Failed to relaunch new application binary: {}", e)))?;

    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_tuple() {
        assert_eq!(parse_version_tuple("0.1.0"), (0, 1, 0));
        assert_eq!(parse_version_tuple("v0.2.1"), (0, 2, 1));
        assert_eq!(parse_version_tuple("v1.10.4-beta"), (1, 10, 4));
        assert_eq!(parse_version_tuple("2"), (2, 0, 0));
    }

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("v0.2.2", "0.2.1"));
        assert!(is_newer_version("v0.3.0", "v0.2.1"));
        assert!(is_newer_version("v1.0.0", "v0.9.9"));
        assert!(!is_newer_version("v0.2.1", "0.2.1"));
        assert!(!is_newer_version("v0.2.0", "0.2.1"));
        assert!(!is_newer_version("0.1.9", "0.2.0"));
    }
}
