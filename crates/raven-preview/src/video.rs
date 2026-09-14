use std::path::Path;
use std::process::{Command, Stdio};

use async_trait::async_trait;
use raven_core::error::{RavenError, RavenResult};
use raven_core::events::PreviewData;
use raven_core::path::RavenPath;

use crate::thumbnail::{self, VIDEO_EXTENSIONS};
use crate::{require_local, PreviewProvider};

/// Size of the still frame shown in the preview panel.
const PANEL_THUMBNAIL_SIZE: u32 = 640;

/// Video previews: a representative frame and the stream's dimensions and
/// duration, both from ffmpeg when it is installed.
pub struct VideoPreview;

impl VideoPreview {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VideoPreview {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PreviewProvider for VideoPreview {
    async fn generate(&self, path: &RavenPath) -> RavenResult<PreviewData> {
        let local = require_local(path)?.clone();
        let display = path.to_string();
        tokio::task::spawn_blocking(move || {
            let info = probe(&local).unwrap_or_default();
            let thumbnail = thumbnail::generate(&local, PANEL_THUMBNAIL_SIZE, || true);
            PreviewData::Video {
                path: local,
                thumbnail,
                width: info.width,
                height: info.height,
                duration_secs: info.duration_secs,
            }
        })
        .await
        .map_err(|e| RavenError::Preview {
            message: format!("video preview task failed for {}: {}", display, e),
        })
    }

    fn supports(&self, extension: &str) -> bool {
        VIDEO_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    }
}

#[derive(Debug, Default, PartialEq)]
struct VideoInfo {
    width: u32,
    height: u32,
    duration_secs: Option<f64>,
}

fn probe(path: &Path) -> Option<VideoInfo> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height:format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| parse_probe(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse ffprobe's `key=value` lines. Unknown or "N/A" values are left unset.
fn parse_probe(text: &str) -> VideoInfo {
    let mut info = VideoInfo::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "width" => info.width = value.trim().parse().unwrap_or(0),
            "height" => info.height = value.trim().parse().unwrap_or(0),
            "duration" => info.duration_secs = value.trim().parse().ok(),
            _ => {}
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffprobe_output() {
        let info = parse_probe("width=1920\nheight=1080\nduration=61.500000\n");
        assert_eq!(
            info,
            VideoInfo {
                width: 1920,
                height: 1080,
                duration_secs: Some(61.5)
            }
        );
    }

    #[test]
    fn missing_values_stay_unset() {
        let info = parse_probe("width=640\nduration=N/A\n");
        assert_eq!(info.width, 640);
        assert_eq!(info.height, 0);
        assert_eq!(info.duration_secs, None);
    }

    #[test]
    fn supports_common_containers() {
        let preview = VideoPreview::new();
        assert!(preview.supports("MP4"));
        assert!(preview.supports("mkv"));
        assert!(!preview.supports("png"));
    }
}
