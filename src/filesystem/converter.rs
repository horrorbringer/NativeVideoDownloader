use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{info, error};
use crate::error::{Result, AppError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Mp3,
    Aac,
    Flac,
    Opus,
    Wav,
    Mp4Remux,
    Mp4Transcode,
}

impl OutputFormat {
    pub fn from_index(idx: i32) -> Self {
        match idx {
            0 => Self::Mp3,
            1 => Self::Aac,
            2 => Self::Flac,
            3 => Self::Opus,
            4 => Self::Wav,
            5 => Self::Mp4Remux,
            6 => Self::Mp4Transcode,
            _ => Self::Mp3,
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Aac => "m4a",
            Self::Flac => "flac",
            Self::Opus => "opus",
            Self::Wav => "wav",
            Self::Mp4Remux | Self::Mp4Transcode => "mp4",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Mp3 => "MP3 Audio (Universal)",
            Self::Aac => "AAC Audio (.m4a)",
            Self::Flac => "FLAC Lossless Audio",
            Self::Opus => "OPUS High-Efficiency Audio",
            Self::Wav => "WAV Uncompressed Audio",
            Self::Mp4Remux => "MP4 Fast Remux (Stream Copy)",
            Self::Mp4Transcode => "MP4 Universal Video (H.264/AAC)",
        }
    }

    #[allow(dead_code)]
    pub fn is_audio_only(&self) -> bool {
        !matches!(self, Self::Mp4Remux | Self::Mp4Transcode)
    }
}

/// Generates an appropriate output path for conversion given the input file and format
pub fn compute_converted_output_path(input_path: &Path, format: OutputFormat) -> PathBuf {
    let parent = input_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = input_path.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let ext = format.extension();

    // If input has the exact same extension, append a descriptor to avoid overwriting input
    let new_file_name = if input_path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case(ext)).unwrap_or(false) {
        match format {
            OutputFormat::Mp4Remux => format!("{}_remux.{}", stem, ext),
            OutputFormat::Mp4Transcode => format!("{}_h264.{}", stem, ext),
            _ => format!("{}_converted.{}", stem, ext),
        }
    } else {
        format!("{}.{}", stem, ext)
    };

    crate::filesystem::resolve_unique_path(parent, &new_file_name)
}

/// Executes FFmpeg conversion command asynchronously
pub async fn run_media_conversion(
    ffmpeg_bin: &Path,
    input_path: &Path,
    output_path: &Path,
    format: OutputFormat,
    bitrate_index: i32,
) -> Result<PathBuf> {
    if !input_path.exists() {
        return Err(AppError::Generic(format!("Input media file not found: {}", input_path.display())));
    }

    if let Some(parent) = output_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let audio_bitrate = match bitrate_index {
        0 => "320k",
        1 => "256k",
        2 => "192k",
        3 => "128k",
        _ => "256k",
    };

    info!(
        "Starting media conversion: {} -> {} (Format: {:?}, Bitrate: {})",
        input_path.display(),
        output_path.display(),
        format,
        audio_bitrate
    );

    let mut cmd = Command::new(ffmpeg_bin);
    cmd.arg("-y") // Overwrite output if already resolved
       .arg("-nostdin")
       .arg("-i").arg(input_path);

    match format {
        OutputFormat::Mp3 => {
            cmd.arg("-vn")
               .arg("-c:a").arg("libmp3lame")
               .arg("-b:a").arg(audio_bitrate);
        }
        OutputFormat::Aac => {
            cmd.arg("-vn")
               .arg("-c:a").arg("aac")
               .arg("-b:a").arg(audio_bitrate);
        }
        OutputFormat::Flac => {
            cmd.arg("-vn")
               .arg("-c:a").arg("flac");
        }
        OutputFormat::Opus => {
            let opus_bitrate = match bitrate_index {
                0 => "192k",
                1 => "160k",
                2 => "128k",
                3 => "96k",
                _ => "128k",
            };
            cmd.arg("-vn")
               .arg("-c:a").arg("libopus")
               .arg("-b:a").arg(opus_bitrate);
        }
        OutputFormat::Wav => {
            cmd.arg("-vn")
               .arg("-c:a").arg("pcm_s16le");
        }
        OutputFormat::Mp4Remux => {
            cmd.arg("-c").arg("copy")
               .arg("-movflags").arg("+faststart");
        }
        OutputFormat::Mp4Transcode => {
            cmd.arg("-c:v").arg("libx264")
               .arg("-crf").arg("23")
               .arg("-preset").arg("fast")
               .arg("-c:a").arg("aac")
               .arg("-b:a").arg("192k")
               .arg("-movflags").arg("+faststart");
        }
    }

    cmd.arg(output_path);

    let output = cmd.output().await.map_err(|e| {
        error!("Failed to spawn FFmpeg process: {}", e);
        AppError::Generic(format!("FFmpeg execution failed: {}", e))
    })?;

    if !output.status.success() {
        let err_text = String::from_utf8_lossy(&output.stderr);
        error!("FFmpeg conversion error: {}", err_text);
        return Err(AppError::Generic(format!("FFmpeg conversion failed: {}", err_text.trim())));
    }

    if !output_path.exists() {
        return Err(AppError::Generic("FFmpeg exited successfully but output file was not created".to_string()));
    }

    info!("Media conversion completed successfully: {}", output_path.display());
    Ok(output_path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_mappings() {
        assert_eq!(OutputFormat::from_index(0), OutputFormat::Mp3);
        assert_eq!(OutputFormat::from_index(1), OutputFormat::Aac);
        assert_eq!(OutputFormat::from_index(2), OutputFormat::Flac);
        assert_eq!(OutputFormat::from_index(3), OutputFormat::Opus);
        assert_eq!(OutputFormat::from_index(4), OutputFormat::Wav);
        assert_eq!(OutputFormat::from_index(5), OutputFormat::Mp4Remux);
        assert_eq!(OutputFormat::from_index(6), OutputFormat::Mp4Transcode);

        assert_eq!(OutputFormat::Mp3.extension(), "mp3");
        assert_eq!(OutputFormat::Aac.extension(), "m4a");
        assert_eq!(OutputFormat::Flac.extension(), "flac");
        assert_eq!(OutputFormat::Opus.extension(), "opus");
        assert_eq!(OutputFormat::Wav.extension(), "wav");
        assert_eq!(OutputFormat::Mp4Remux.extension(), "mp4");
        assert_eq!(OutputFormat::Mp4Transcode.extension(), "mp4");

        assert!(OutputFormat::Mp3.is_audio_only());
        assert!(!OutputFormat::Mp4Remux.is_audio_only());
        assert!(!OutputFormat::Mp4Transcode.is_audio_only());
    }

    #[test]
    fn test_compute_converted_output_path() {
        let video = Path::new("/downloads/series/episode_1.mp4");
        let mp3_path = compute_converted_output_path(video, OutputFormat::Mp3);
        assert_eq!(mp3_path.extension().unwrap(), "mp3");
        assert_eq!(mp3_path.file_stem().unwrap(), "episode_1");

        let remux_path = compute_converted_output_path(video, OutputFormat::Mp4Remux);
        assert_eq!(remux_path.extension().unwrap(), "mp4");
        assert!(remux_path.to_string_lossy().contains("_remux"));
    }
}
