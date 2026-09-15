//! Universal Media Info Extraction Tool via `GenericYtdlpDownloader`.
//!
//! Features:
//! - Queries metadata for 1,800+ supported sites using `omniget_core::platforms::generic_ytdlp::GenericYtdlpDownloader`.
//! - Resolves direct media streams (.mp4, .m3u8, .mp3, etc.) instantly in pure Rust without invoking yt-dlp.
//! - Graceful fallback probe via HTTP HEAD when yt-dlp is not installed in the runtime environment or when the URL is an unparseable generic stream.
//! - Strict input validation rejecting empty strings, non-URL inputs, or malformed protocols.
//! - Cleans whitespace-padded URLs.

use anyhow::{anyhow, Result};
use omniget_core::models::media::{MediaInfo, MediaType, VideoQuality};
use omniget_core::platforms::generic_ytdlp::GenericYtdlpDownloader;
use omniget_core::platforms::traits::PlatformDownloader;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;

/// Arguments for the `media_info` extraction tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInfoArgs {
    /// Public URL of the media item or direct streaming/audio/video URL.
    pub url: String,
}

/// Fallback extraction of a filename from a URL path.
fn filename_from_url(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| {
            let path = u.path();
            let last = path.rsplit('/').next()?;
            if last.is_empty() || !last.contains('.') {
                return None;
            }
            Some(last.to_string())
        })
        .unwrap_or_else(|| "media_stream".to_string())
}

/// Probe a remote URL with an HTTP HEAD request to synthesize MediaInfo when
/// `yt-dlp` is unavailable and the URL does not have a recognizable file extension.
async fn probe_direct_stream(url: &str) -> Option<MediaInfo> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .ok()?;

    let resp = client.head(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())?
        .to_lowercase();

    let is_video = content_type.starts_with("video/");
    let is_audio = content_type.starts_with("audio/");
    let is_hls = content_type.contains("mpegurl") || content_type.contains("m3u8");

    if !is_video && !is_audio && !is_hls {
        return None;
    }

    let media_type = if is_audio {
        MediaType::Audio
    } else {
        MediaType::Video
    };

    let format = if is_hls {
        "hls".to_string()
    } else if is_audio {
        "direct_audio".to_string()
    } else {
        "direct_video".to_string()
    };

    let file_size_bytes = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let title = filename_from_url(url);

    Some(MediaInfo {
        title,
        author: String::new(),
        platform: "generic".to_string(),
        duration_seconds: None,
        thumbnail_url: None,
        available_qualities: vec![VideoQuality {
            label: "original".to_string(),
            width: 0,
            height: 0,
            url: url.to_string(),
            format,
        }],
        media_type,
        file_size_bytes,
    })
}

/// Extract media metadata using GenericYtdlpDownloader with graceful fallback.
pub async fn extract_media_info(args: MediaInfoArgs) -> Result<MediaInfo> {
    let trimmed = args.url.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Media URL cannot be empty");
    }

    let parsed = Url::parse(trimmed).map_err(|e| anyhow!("Malformed URL: {}", e))?;

    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        anyhow::bail!(
            "Unsupported URL scheme: '{}' (must be http or https)",
            parsed.scheme()
        );
    }

    let host = parsed.host_str().unwrap_or("");
    if host.is_empty() {
        anyhow::bail!("Malformed URL: missing hostname");
    }

    let downloader = GenericYtdlpDownloader::new();

    // 1. Primary path: GenericYtdlpDownloader.
    // Handles direct media URLs (.mp4, .m3u8, .mp3, etc.) immediately in pure Rust without yt-dlp,
    // and queries yt-dlp for supported platforms (YouTube, Vimeo, TikTok, etc.).
    match downloader.get_media_info(trimmed).await {
        Ok(info) => Ok(info),
        Err(err) => {
            tracing::warn!(
                "GenericYtdlpDownloader failed for {}: {}. Attempting direct stream HTTP HEAD probe.",
                trimmed,
                err
            );

            // 2. Fallback: probe via HTTP HEAD to detect if the endpoint serves a media stream
            // without a standard file extension or when yt-dlp is missing in the container/host.
            if let Some(fallback_info) = probe_direct_stream(trimmed).await {
                return Ok(fallback_info);
            }

            // 3. Fallback exhausted; propagate the original error.
            Err(err)
        }
    }
}

/// JSON-RPC `tools/call` handler for `media_info`.
pub async fn call_media_info(arguments: Value) -> Result<Value, anyhow::Error> {
    let args: MediaInfoArgs = serde_json::from_value(arguments)
        .map_err(|e| anyhow!("Invalid arguments for media_info: {}", e))?;
    match extract_media_info(args).await {
        Ok(info) => {
            let serialized = serde_json::to_string_pretty(&info)?;
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": serialized
                    }
                ],
                "media_info": info,
                "isError": false
            }))
        }
        Err(e) => {
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": e.to_string()
                    }
                ],
                "isError": true
            }))
        }
    }
}
