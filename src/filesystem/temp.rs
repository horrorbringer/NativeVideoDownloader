use std::path::{Path, PathBuf};
use crate::error::Result;

/// Returns the `.part` file path for a destination file
pub fn part_path_for(final_path: &Path) -> PathBuf {
    let mut os_string = final_path.as_os_str().to_os_string();
    os_string.push(".part");
    PathBuf::from(os_string)
}

/// Atomically moves the finished `.part` file to the final destination path
pub fn finalize_part_file(part_path: &Path, final_path: &Path) -> Result<()> {
    if part_path.exists() {
        std::fs::rename(part_path, final_path)?;
    }
    Ok(())
}

/// Cleans up any incomplete `.part` file upon cancellation or error
pub fn cleanup_part_file(part_path: &Path) -> Result<()> {
    if part_path.exists() {
        let _ = std::fs::remove_file(part_path);
    }
    Ok(())
}
