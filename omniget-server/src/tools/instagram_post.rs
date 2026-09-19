//! Instagram headless post, Reel, and carousel extractor tool (`instagram_post`).
//!
//! Multi-tier extraction pipeline:
//! 1. Primary: Anonymous Polaris GraphQL query (`doc_id: 8845758582119845` or `27130156389949648`)
//! 2. Secondary: Public captioned embed page scraper (`/p/{shortcode}/embed/captioned/`)
//! 3. Tertiary: `yt-dlp` metadata and progressive video stream extraction fallback

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;
use url::Url;

// ── Constants & Regular Expressions ──────────────────────────────────────────

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const IG_APP_ID: &str = "936619743392459";
const GQL_DOC_ID_PRIMARY: &str = "8845758582119845";
const GQL_DOC_ID_SECONDARY: &str = "27130156389949648";

static HASHTAG_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#([a-zA-Z0-9_\u{0080}-\u{ffff}]+)")
        .expect("Valid hashtag extraction regex")
});

static MENTION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"@([a-zA-Z0-9_.]+)")
        .expect("Valid mention extraction regex")
});

static NUMERIC_ENTITY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"&#(\d+);")
        .expect("Valid numeric entity regex")
});

static HEX_ENTITY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"&#[xX]([0-9a-fA-F]+);")
        .expect("Valid hex entity regex")
});

static DASH_AUDIO_SET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<AdaptationSet[^>]+(?:contentType=["']audio["']|mimeType=["']audio/[^"']*["'])[^>]*>(.*?)</AdaptationSet>"#)
        .expect("Valid DASH audio adaptation set regex")
});

static DASH_AUDIO_REP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<Representation[^>]+(?:contentType=["']audio["']|mimeType=["']audio/[^"']*["'])[^>]*>(.*?)</Representation>"#)
        .expect("Valid DASH audio representation regex")
});

static DASH_BASE_URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<BaseURL[^>]*>([^<]+)</BaseURL>"#)
        .expect("Valid DASH BaseURL regex")
});

static DASH_DIRECT_AUDIO_URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<BaseURL[^>]*>([^<]+\.(?:m4a|mp3|aac)(?:\?[^<]*)?)</BaseURL>"#)
        .expect("Valid DASH direct audio URL regex")
});

/// Cleans and unescapes media asset URLs from JSON or DASH manifests.
pub fn clean_media_url(raw: &str) -> String {
    let decoded = decode_html_entities(raw.trim().trim_matches('"').trim_matches('\\'));
    decoded
        .replace(r"\u0026", "&")
        .replace(r"\/", "/")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}


// ── Data Models ──────────────────────────────────────────────────────────────

/// Arguments payload for `instagram_post` tool invocations.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InstagramArgs {
    pub url: Option<String>,
    #[serde(default)]
    pub shortcode: Option<String>,
}

/// Author / creator metadata for an Instagram post, Reel, or IGTV video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstagramAuthor {
    /// Instagram username / handle (without '@')
    pub username: String,
    /// Display name or full name, if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    /// Canonical profile URL (e.g. "https://www.instagram.com/username/")
    pub profile_url: String,
    /// Direct URL to author's profile avatar picture
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// Whether the profile is verified with a blue badge
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_verified: Option<bool>,
}

/// An individual media item attached to an Instagram post, Reel, or carousel slide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstagramMediaItem {
    /// Media item identifier or shortcode, if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Media kind: "photo" | "video"
    pub media_type: String,
    /// Direct highest-resolution media asset URL (image source or video stream)
    pub url: String,
    /// Width in pixels, if known
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Height in pixels, if known
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Thumbnail / poster image URL (for video items)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// True if media item is a video, false if static image
    pub is_video: bool,
    /// Duration in seconds for video items, if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    /// Direct URL to standalone audio track (.m4a/.mp3), if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
    /// Whether this media item includes an audio track
    #[serde(default)]
    pub has_audio: bool,
}

/// Comprehensive extracted metadata for an Instagram post, Reel, or carousel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstagramPost {
    /// Post identifier (shortcode or numeric PK)
    pub id: String,
    /// Canonical shortcode (e.g. "C_abc123")
    pub shortcode: String,
    /// Normalized canonical web URL (e.g. "https://www.instagram.com/p/C_abc123/")
    pub url: String,
    /// Post author / creator information
    pub author: InstagramAuthor,
    /// Full post caption / text content
    pub caption: String,
    /// Extracted hashtags without the '#' symbol
    pub hashtags: Vec<String>,
    /// Extracted mentioned usernames without the '@' symbol
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
    /// Flat list of direct high-resolution image URLs
    pub images: Vec<String>,
    /// Flat list of direct playable video stream URLs
    pub videos: Vec<String>,
    /// Direct URL to standalone audio track (.m4a/.mp3) for AI speech transcription
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
    /// Whether the post/video includes an audio track
    #[serde(default)]
    pub has_audio: bool,
    /// Music title or sound name, if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_title: Option<String>,
    /// Music artist or sound creator, if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_artist: Option<String>,
    /// Overall post media classification: "photo", "video", or "carousel"
    pub media_type: String,
    /// Detailed individual media items (e.g. all slides in a carousel)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media_items: Vec<InstagramMediaItem>,
    /// Best available thumbnail / poster image URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// Publication Unix timestamp (seconds since epoch), if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taken_at: Option<i64>,
    /// Extracted like count, if public
    #[serde(skip_serializing_if = "Option::is_none")]
    pub like_count: Option<u64>,
    /// Extracted comment count, if public
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment_count: Option<u64>,
    /// Clean human-readable Markdown summary formatted for AI agents
    pub markdown: String,
}

/// Result of parsing and normalizing an Instagram input URL or shortcode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstagramUrlInfo {
    /// Extracted shortcode with exact case preserved (e.g. "C_abc123")
    pub shortcode: String,
    /// Post kind inferred from URL path: "post", "reel", "tv", or "share"
    pub kind: String,
    /// Normalized canonical HTTPS URL (e.g. "https://www.instagram.com/p/C_abc123/")
    pub canonical_url: String,
    /// True if input was a shortened share link (`/share/{id}/`) requiring redirect resolution
    pub is_share_link: bool,
    /// True if content is a Reel
    pub is_reel: bool,
}

/// Error types for Instagram URL parsing and headless extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstagramExtractError {
    /// Invalid or empty input (whitespace, invalid characters in shortcode, bad scheme)
    InvalidInput(String),
    /// Host is not a recognized Instagram domain
    InvalidDomain(String),
    /// URL path is not an extractable post (Stories, Direct, Accounts, Explore, Profiles)
    UnsupportedUrl(String),
    /// Post was not found or has been deleted (HTTP 404)
    NotFound(String),
    /// Post or account is private, or blocked by an authentication/login wall
    PrivateOrLoginRequired(String),
    /// Upstream network, API, or parsing failure
    UpstreamError(String),
    /// Extraction request timed out
    Timeout(String),
}

impl std::fmt::Display for InstagramExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
            Self::InvalidDomain(msg) => write!(f, "Invalid domain: {}", msg),
            Self::UnsupportedUrl(msg) => write!(f, "Unsupported URL: {}", msg),
            Self::NotFound(msg) => write!(f, "Post not found: {}", msg),
            Self::PrivateOrLoginRequired(msg) => write!(f, "Post is private or requires login: {}", msg),
            Self::UpstreamError(msg) => write!(f, "Upstream error: {}", msg),
            Self::Timeout(msg) => write!(f, "Request timed out: {}", msg),
        }
    }
}

impl std::error::Error for InstagramExtractError {}

// ── URL Normalization & Validation ───────────────────────────────────────────

/// Validates that a shortcode conforms to Instagram Base64 specifications.
pub fn validate_shortcode(code: &str) -> Result<(), InstagramExtractError> {
    let trimmed = code.trim();
    if trimmed.is_empty() {
        return Err(InstagramExtractError::InvalidInput(
            "Shortcode cannot be empty".into(),
        ));
    }
    if trimmed.len() < 3 || trimmed.len() > 40 {
        return Err(InstagramExtractError::InvalidInput(format!(
            "Shortcode '{}' has invalid length {} (expected between 3 and 40 characters)",
            trimmed,
            trimmed.len()
        )));
    }
    if !trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(InstagramExtractError::InvalidInput(format!(
            "Shortcode '{}' contains invalid characters (only alphanumeric, '_', and '-' are allowed)",
            trimmed
        )));
    }
    Ok(())
}

/// Parses and normalizes an Instagram URL or raw shortcode.
///
/// Features:
/// - Handles `/p/`, `/reel/`, `/reels/`, `/tv/`, `/share/`, `/share/p/`, `/share/reel/`
/// - Preserves exact case of base64 shortcode
/// - Strips all query parameters (`igsh`, `utm_*`) and hash fragments
/// - Validates recognized host domains (`instagram.com`, `ddinstagram.com`)
/// - Auto-prefixes missing schemes (e.g. `instagram.com/...` -> `https://instagram.com/...`)
/// - Rejects unsupported paths (Stories, Direct, Accounts, Explore, Highlights, Live, Guides, Reel audio, non-post Reel subpaths, Profiles)
/// - Rejects non-HTTP schemes and foreign/spoofed domains
pub fn parse_instagram_url(raw: &str) -> Result<InstagramUrlInfo, InstagramExtractError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(InstagramExtractError::InvalidInput(
            "URL or shortcode cannot be empty".into(),
        ));
    }

    // 1. Raw shortcode shortcut: string contains no slashes, colons, or dots
    if !trimmed.contains('/') && !trimmed.contains(':') && !trimmed.contains('.') {
        validate_shortcode(trimmed)?;
        return Ok(InstagramUrlInfo {
            shortcode: trimmed.to_string(),
            kind: "post".to_string(),
            canonical_url: format!("https://www.instagram.com/p/{}/", trimmed),
            is_share_link: false,
            is_reel: false,
        });
    }

    // 2. URL parsing with auto-prefixing
    let parsed_url = if trimmed.contains("://") {
        Url::parse(trimmed).map_err(|e| {
            InstagramExtractError::InvalidInput(format!("Malformed URL '{}': {}", trimmed, e))
        })?
    } else {
        Url::parse(&format!("https://{}", trimmed)).map_err(|e| {
            InstagramExtractError::InvalidInput(format!("Malformed URL 'https://{}': {}", trimmed, e))
        })?
    };

    // 3. Scheme validation
    let scheme = parsed_url.scheme().to_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(InstagramExtractError::InvalidInput(format!(
            "Unsupported URL scheme '{}'; expected http or https",
            scheme
        )));
    }

    // 4. Host validation
    let host = parsed_url.host_str().unwrap_or("").to_lowercase();
    let is_valid_host = host == "instagram.com"
        || host.ends_with(".instagram.com")
        || host == "ddinstagram.com"
        || host.ends_with(".ddinstagram.com");

    if !is_valid_host {
        return Err(InstagramExtractError::InvalidDomain(format!(
            "Domain '{}' is not a recognized Instagram domain",
            host
        )));
    }

    // 5. Path segments extraction
    let segments: Vec<&str> = parsed_url
        .path()
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    if segments.is_empty() {
        return Err(InstagramExtractError::InvalidInput(
            "URL path is empty; please provide a post, Reel, or IGTV URL".into(),
        ));
    }

    let first_lower = segments[0].to_lowercase();

    // 6. Explicitly reject unsupported Instagram paths
    match first_lower.as_str() {
        "stories" => {
            return Err(InstagramExtractError::UnsupportedUrl(
                "Instagram Stories are ephemeral and require authentication; headless extraction is not supported".into(),
            ));
        }
        "direct" => {
            return Err(InstagramExtractError::UnsupportedUrl(
                "Instagram Direct Messages are private and cannot be extracted".into(),
            ));
        }
        "accounts" => {
            return Err(InstagramExtractError::UnsupportedUrl(
                "Instagram account management pages cannot be extracted as posts".into(),
            ));
        }
        "explore" => {
            return Err(InstagramExtractError::UnsupportedUrl(
                "Instagram Explore page cannot be extracted as a post".into(),
            ));
        }
        "highlights" | "highlight" => {
            return Err(InstagramExtractError::UnsupportedUrl(
                "Instagram Story Highlights require authentication; headless extraction is not supported".into(),
            ));
        }
        "live" => {
            return Err(InstagramExtractError::UnsupportedUrl(
                "Instagram Live streams cannot be extracted as static posts".into(),
            ));
        }
        "guides" | "guide" => {
            return Err(InstagramExtractError::UnsupportedUrl(
                "Instagram Guides are not individual posts and cannot be extracted".into(),
            ));
        }
        _ => {}
    }

    // Reject non-post subpaths under /reel/ or /reels/ (e.g. /reels/audio/<id>/, /reels/videos/)
    if first_lower == "reel" || first_lower == "reels" {
        if let Some(subpath) = segments.get(1).map(|s| s.to_lowercase()) {
            match subpath.as_str() {
                "audio" => {
                    return Err(InstagramExtractError::UnsupportedUrl(
                        "Instagram Reel Audio pages are not supported; please provide a post or Reel URL".into(),
                    ));
                }
                "videos" => {
                    return Err(InstagramExtractError::UnsupportedUrl(
                        "Instagram Reels video browse pages are not supported; please provide a post or Reel URL".into(),
                    ));
                }
                _ => {}
            }
        }
    }

    // 7. Match supported post paths
    match first_lower.as_str() {
        "p" | "reel" | "reels" | "tv" => {
            let is_reel = first_lower == "reel" || first_lower == "reels";
            let kind = match first_lower.as_str() {
                "reel" | "reels" => "reel",
                "tv" => "tv",
                _ => "post",
            };
            let raw_shortcode = match segments.get(1) {
                Some(sc) if !sc.trim().is_empty() => *sc,
                _ => {
                    return Err(InstagramExtractError::InvalidInput(format!(
                        "Missing shortcode in URL path '/{}/'",
                        first_lower
                    )));
                }
            };
            validate_shortcode(raw_shortcode)?;
            let canonical_url = match kind {
                "reel" => format!("https://www.instagram.com/reel/{}/", raw_shortcode),
                "tv" => format!("https://www.instagram.com/tv/{}/", raw_shortcode),
                _ => format!("https://www.instagram.com/p/{}/", raw_shortcode),
            };
            Ok(InstagramUrlInfo {
                shortcode: raw_shortcode.to_string(),
                kind: kind.to_string(),
                canonical_url,
                is_share_link: false,
                is_reel,
            })
        }
        "share" => {
            if segments.len() < 2 {
                return Err(InstagramExtractError::InvalidInput(
                    "Missing share ID in '/share/' path".into(),
                ));
            }
            let second_lower = segments[1].to_lowercase();
            match second_lower.as_str() {
                "p" | "reel" | "reels" => {
                    let is_reel = second_lower != "p";
                    let kind = if second_lower == "p" { "post" } else { "reel" };
                    if is_reel {
                        if let Some(sub) = segments.get(2).map(|s| s.to_lowercase()) {
                            match sub.as_str() {
                                "audio" => {
                                    return Err(InstagramExtractError::UnsupportedUrl(
                                        "Instagram Reel Audio pages are not supported; please provide a post or Reel URL".into(),
                                    ));
                                }
                                "videos" => {
                                    return Err(InstagramExtractError::UnsupportedUrl(
                                        "Instagram Reels video browse pages are not supported; please provide a post or Reel URL".into(),
                                    ));
                                }
                                _ => {}
                            }
                        }
                    }
                    let raw_shortcode = match segments.get(2) {
                        Some(sc) if !sc.trim().is_empty() => *sc,
                        _ => {
                            return Err(InstagramExtractError::InvalidInput(format!(
                                "Missing shortcode in URL path '/share/{}/'",
                                second_lower
                            )));
                        }
                    };
                    validate_shortcode(raw_shortcode)?;
                    let canonical_url = match kind {
                        "reel" => format!("https://www.instagram.com/reel/{}/", raw_shortcode),
                        _ => format!("https://www.instagram.com/p/{}/", raw_shortcode),
                    };
                    Ok(InstagramUrlInfo {
                        shortcode: raw_shortcode.to_string(),
                        kind: kind.to_string(),
                        canonical_url,
                        is_share_link: false,
                        is_reel,
                    })
                }
                _ => {
                    let share_id = segments[1];
                    validate_shortcode(share_id)?;
                    Ok(InstagramUrlInfo {
                        shortcode: share_id.to_string(),
                        kind: "share".to_string(),
                        canonical_url: format!("https://www.instagram.com/share/{}/", share_id),
                        is_share_link: true,
                        is_reel: false,
                    })
                }
            }
        }
        _ => {
            // If there is only one segment, it is a profile handle (e.g. /natgeo/)
            if segments.len() == 1 {
                Err(InstagramExtractError::UnsupportedUrl(format!(
                    "Profile URL '/{}/' is not supported; please provide a post, Reel, or IGTV URL",
                    segments[0]
                )))
            } else {
                Err(InstagramExtractError::UnsupportedUrl(format!(
                    "Unrecognized Instagram URL path '{}'",
                    parsed_url.path()
                )))
            }
        }
    }
}

// ── Caption & Hashtag Normalization ──────────────────────────────────────────

/// Extracts unique lowercase hashtags from caption text using `#([a-zA-Z0-9_\u0080-\uffff]+)`.
pub fn extract_hashtags(caption: &str) -> Vec<String> {
    let mut hashtags = Vec::new();
    let mut seen = HashSet::new();

    for cap in HASHTAG_REGEX.captures_iter(caption) {
        if let Some(tag_match) = cap.get(1) {
            let tag = tag_match.as_str().to_lowercase();
            if !tag.is_empty() && seen.insert(tag.clone()) {
                hashtags.push(tag);
            }
        }
    }

    hashtags
}

/// Extracts unique lowercase mentions from caption text.
pub fn extract_mentions(caption: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    let mut seen = HashSet::new();

    for cap in MENTION_REGEX.captures_iter(caption) {
        if let Some(mention_match) = cap.get(1) {
            let m = mention_match.as_str().to_lowercase();
            if !m.is_empty() && seen.insert(m.clone()) {
                mentions.push(m);
            }
        }
    }

    mentions
}

/// Decodes standard, decimal, and hexadecimal HTML entities.
pub fn decode_html_entities(input: &str) -> String {
    let s = input
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ");

    let s = if s.contains("&#") {
        NUMERIC_ENTITY_REGEX.replace_all(&s, |caps: &regex::Captures| {
            if let Ok(code) = caps[1].parse::<u32>() {
                if let Some(c) = char::from_u32(code) {
                    return c.to_string();
                }
            }
            caps[0].to_string()
        }).to_string()
    } else {
        s
    };

    if s.contains("&#x") || s.contains("&#X") {
        HEX_ENTITY_REGEX.replace_all(&s, |caps: &regex::Captures| {
            if let Ok(code) = u32::from_str_radix(&caps[1], 16) {
                if let Some(c) = char::from_u32(code) {
                    return c.to_string();
                }
            }
            caps[0].to_string()
        }).to_string()
    } else {
        s
    }
}

/// Normalizes caption text: strips BOM, decodes HTML entities, standardizes CRLF/CR to LF,
/// trims trailing spaces per line, collapses excessive vertical space, preserves emojis and RTL byte-for-byte.
pub fn normalize_caption(raw: &str) -> String {
    if raw.trim().is_empty() {
        return String::new();
    }

    // 1. Strip leading BOM if present
    let text = raw.strip_prefix('\u{feff}').unwrap_or(raw);

    // 2. Decode HTML entities
    let decoded = decode_html_entities(text);

    // 3. Unify newlines
    let unified = decoded.replace("\r\n", "\n").replace('\r', "\n");

    // 4. Line-by-line processing: trim trailing spaces and collapse excessive empty lines
    let lines: Vec<&str> = unified.lines().collect();
    let mut cleaned_lines: Vec<String> = Vec::with_capacity(lines.len());
    let mut consecutive_blank_lines = 0;

    for line in lines {
        let trimmed_line = line.trim_end();
        if trimmed_line.is_empty() {
            consecutive_blank_lines += 1;
            if consecutive_blank_lines <= 1 {
                cleaned_lines.push(String::new());
            }
        } else {
            consecutive_blank_lines = 0;
            cleaned_lines.push(trimmed_line.to_string());
        }
    }

    let result = cleaned_lines.join("\n");
    result.trim().to_string()
}

// ── Media Resolution & Carousel Unrolling ────────────────────────────────────

/// Extracts highest resolution image candidate from display_resources or candidates (max width * height).
pub fn select_highest_resolution_image(node: &Value) -> Option<String> {
    // 1. GraphQL display_resources
    if let Some(resources) = node.get("display_resources").and_then(|v| v.as_array()) {
        let best = resources
            .iter()
            .filter_map(|r| {
                let src = r.get("src")?.as_str()?;
                let w = r.get("config_width").and_then(|v| v.as_u64()).unwrap_or(0);
                let h = r.get("config_height").and_then(|v| v.as_u64()).unwrap_or(0);
                Some((src, w.saturating_mul(h)))
            })
            .max_by_key(|(_, area)| *area);

        if let Some((src, _)) = best {
            return Some(src.to_string());
        }
    }

    // 2. REST image_versions2 candidates
    if let Some(candidates) = node
        .get("image_versions2")
        .and_then(|i| i.get("candidates"))
        .and_then(|v| v.as_array())
    {
        let best = candidates
            .iter()
            .filter_map(|c| {
                let url = c.get("url")?.as_str()?;
                let w = c.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                let h = c.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
                Some((url, w.saturating_mul(h)))
            })
            .max_by_key(|(_, area)| *area);

        if let Some((url, _)) = best {
            return Some(url.to_string());
        }
    }

    // 3. Fallback to display_url
    node.get("display_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Helper to parse DASH MPD manifest and extract audio stream BaseURL
pub fn extract_audio_from_dash_manifest(manifest: &str) -> Option<String> {
    let unescaped_manifest = manifest.replace(r"\/", "/");
    let decoded_manifest = if unescaped_manifest.contains("&lt;") {
        decode_html_entities(&unescaped_manifest)
    } else {
        unescaped_manifest
    };

    // 1. Check AdaptationSet with audio content/mime type
    if let Some(cap) = DASH_AUDIO_SET_RE.captures(&decoded_manifest) {
        if let Some(inner) = cap.get(1) {
            if let Some(b_cap) = DASH_BASE_URL_RE.captures(inner.as_str()) {
                if let Some(u) = b_cap.get(1) {
                    let cleaned = clean_media_url(u.as_str());
                    if cleaned.starts_with("http") {
                        return Some(cleaned);
                    }
                }
            }
        }
    }

    // 2. Check Representation with audio content/mime type
    if let Some(cap) = DASH_AUDIO_REP_RE.captures(&decoded_manifest) {
        if let Some(inner) = cap.get(1) {
            if let Some(b_cap) = DASH_BASE_URL_RE.captures(inner.as_str()) {
                if let Some(u) = b_cap.get(1) {
                    let cleaned = clean_media_url(u.as_str());
                    if cleaned.starts_with("http") {
                        return Some(cleaned);
                    }
                }
            }
        }
    }

    // 3. Check direct audio extensions in BaseURL
    if let Some(cap) = DASH_DIRECT_AUDIO_URL_RE.captures(&decoded_manifest) {
        if let Some(u) = cap.get(1) {
            let cleaned = clean_media_url(u.as_str());
            if cleaned.starts_with("http") {
                return Some(cleaned);
            }
        }
    }

    None
}

/// Extracts audio stream and music metadata from GraphQL / REST media JSON nodes.
/// Returns `(audio_url, has_audio, audio_title, audio_artist)`.
pub fn extract_audio_from_node(node: &Value) -> (Option<String>, bool, Option<String>, Option<String>) {
    // 1. Check clips_metadata
    if let Some(clips) = node.get("clips_metadata") {
        if let Some(music_info) = clips.get("music_info") {
            if let Some(asset) = music_info.get("music_asset_info") {
                let audio_url = asset
                    .get("progressive_download_url")
                    .and_then(Value::as_str)
                    .filter(|s| s.starts_with("http"))
                    .map(String::from);
                let title = asset.get("title").and_then(Value::as_str).map(String::from);
                let artist = asset.get("display_artist").and_then(Value::as_str).map(String::from);
                if audio_url.is_some() {
                    return (audio_url, true, title, artist);
                }
            }
        }
        if let Some(orig) = clips.get("original_sound_info") {
            let audio_url = orig
                .get("progressive_download_url")
                .and_then(Value::as_str)
                .filter(|s| s.starts_with("http"))
                .map(String::from);
            let title = orig
                .get("original_audio_title")
                .or_else(|| orig.get("audio_asset_id"))
                .and_then(Value::as_str)
                .map(String::from);
            let artist = orig
                .pointer("/ig_artist/username")
                .or_else(|| orig.pointer("/ig_artist/full_name"))
                .and_then(Value::as_str)
                .map(String::from);
            if audio_url.is_some() {
                return (audio_url, true, title, artist);
            }
        }
    }

    // 2. Check music_metadata
    if let Some(mm) = node.get("music_metadata") {
        if let Some(music_info) = mm.get("music_info") {
            if let Some(asset) = music_info.get("music_asset_info") {
                let audio_url = asset
                    .get("progressive_download_url")
                    .and_then(Value::as_str)
                    .filter(|s| s.starts_with("http"))
                    .map(String::from);
                let title = asset.get("title").and_then(Value::as_str).map(String::from);
                let artist = asset.get("display_artist").and_then(Value::as_str).map(String::from);
                if audio_url.is_some() {
                    return (audio_url, true, title, artist);
                }
            }
        }
        if let Some(orig) = mm.get("original_sound_info") {
            let audio_url = orig
                .get("progressive_download_url")
                .and_then(Value::as_str)
                .filter(|s| s.starts_with("http"))
                .map(String::from);
            if audio_url.is_some() {
                return (audio_url, true, None, None);
            }
        }
    }

    // 3. Check dash_info / video_dash_manifest / dash_manifest for audio representation BaseURL
    let manifest = node.pointer("/dash_info/video_dash_manifest")
        .or_else(|| node.get("video_dash_manifest"))
        .or_else(|| node.get("dash_manifest"))
        .or_else(|| node.pointer("/video_dash_manifest"))
        .and_then(Value::as_str);
    if let Some(manifest) = manifest {
        if let Some(audio_url) = extract_audio_from_dash_manifest(manifest) {
            return (Some(audio_url), true, None, None);
        }
    }

    // 4. Fallback: check has_audio boolean flag
    let has_audio_flag = node.get("has_audio").and_then(Value::as_bool);
    let is_video = node.get("is_video").and_then(Value::as_bool).unwrap_or(false)
        || node.get("video_url").is_some()
        || node.get("video_versions").is_some();

    let has_audio = match has_audio_flag {
        Some(b) => b,
        None => is_video,
    };

    (None, has_audio, None, None)
}

/// Unrolls multi-image carousels or single media nodes into structured items and flat URL lists.
/// Returns `(images, videos, media_items, media_type, thumbnail_url)`.
pub fn unroll_media_from_node(
    node: &Value,
) -> (
    Vec<String>,
    Vec<String>,
    Vec<InstagramMediaItem>,
    String,
    Option<String>,
) {
    let mut images = Vec::new();
    let mut videos = Vec::new();
    let mut media_items = Vec::new();

    // Case 1: GraphQL edge_sidecar_to_children
    if let Some(sidecar) = node.get("edge_sidecar_to_children") {
        if let Some(edges) = sidecar.get("edges").and_then(|v| v.as_array()) {
            if !edges.is_empty() {
                for edge in edges {
                    if let Some(child) = edge.get("node") {
                        let child_id = child
                            .get("shortcode")
                            .or_else(|| child.get("id"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        let is_video = child
                            .get("is_video")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                            || child.get("video_url").is_some();

                        let img_url = select_highest_resolution_image(child);
                        let vid_url = child
                            .get("video_url")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        let width = child
                            .get("dimensions")
                            .and_then(|d| d.get("width"))
                            .and_then(|v| v.as_u64())
                            .map(|n| n as u32);
                        let height = child
                            .get("dimensions")
                            .and_then(|d| d.get("height"))
                            .and_then(|v| v.as_u64())
                            .map(|n| n as u32);

                        if is_video {
                            let (child_audio, child_has_audio, _, _) = extract_audio_from_node(child);
                            if let Some(ref v_url) = vid_url {
                                if !videos.contains(v_url) {
                                    videos.push(v_url.clone());
                                }
                            }
                            if let Some(ref i_url) = img_url {
                                if !images.contains(i_url) {
                                    images.push(i_url.clone());
                                }
                            }
                            media_items.push(InstagramMediaItem {
                                id: child_id,
                                media_type: "video".to_string(),
                                url: vid_url.unwrap_or_else(|| img_url.clone().unwrap_or_default()),
                                width,
                                height,
                                thumbnail_url: img_url,
                                is_video: true,
                                duration_secs: child.get("video_duration").and_then(|v| v.as_f64()),
                                audio_url: child_audio,
                                has_audio: child_has_audio,
                            });
                        } else if let Some(i_url) = img_url {
                            if !images.contains(&i_url) {
                                images.push(i_url.clone());
                            }
                            media_items.push(InstagramMediaItem {
                                id: child_id,
                                media_type: "photo".to_string(),
                                url: i_url,
                                width,
                                height,
                                thumbnail_url: None,
                                is_video: false,
                                duration_secs: None,
                                audio_url: None,
                                has_audio: false,
                            });
                        }
                    }
                }
                let thumbnail_url = images.first().cloned();
                return (images, videos, media_items, "carousel".to_string(), thumbnail_url);
            }
        }
    }

    // Case 2: REST carousel_media
    if let Some(children) = node.get("carousel_media").and_then(|v| v.as_array()) {
        if !children.is_empty() {
            for child in children {
                let child_id = child
                    .get("id")
                    .or_else(|| child.get("pk"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let mut best_video = None;
                if let Some(v_arr) = child.get("video_versions").and_then(|v| v.as_array()) {
                    best_video = v_arr
                        .iter()
                        .max_by_key(|v| {
                            let w = v.get("width").and_then(|x| x.as_u64()).unwrap_or(0);
                            let h = v.get("height").and_then(|x| x.as_u64()).unwrap_or(0);
                            w.saturating_mul(h)
                        })
                        .and_then(|v| v.get("url"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }

                let best_image = select_highest_resolution_image(child);

                let is_video = best_video.is_some()
                    || child.get("is_video").and_then(|v| v.as_bool()).unwrap_or(false);

                if is_video {
                    let (child_audio, child_has_audio, _, _) = extract_audio_from_node(child);
                    if let Some(ref v_url) = best_video {
                        if !videos.contains(v_url) {
                            videos.push(v_url.clone());
                        }
                    }
                    if let Some(ref i_url) = best_image {
                        if !images.contains(i_url) {
                            images.push(i_url.clone());
                        }
                    }
                    media_items.push(InstagramMediaItem {
                        id: child_id,
                        media_type: "video".to_string(),
                        url: best_video.unwrap_or_else(|| best_image.clone().unwrap_or_default()),
                        width: child.get("original_width").and_then(|v| v.as_u64()).map(|n| n as u32),
                        height: child.get("original_height").and_then(|v| v.as_u64()).map(|n| n as u32),
                        thumbnail_url: best_image,
                        is_video: true,
                        duration_secs: child.get("video_duration").and_then(|v| v.as_f64()),
                        audio_url: child_audio,
                        has_audio: child_has_audio,
                    });
                } else if let Some(i_url) = best_image {
                    if !images.contains(&i_url) {
                        images.push(i_url.clone());
                    }
                    media_items.push(InstagramMediaItem {
                        id: child_id,
                        media_type: "photo".to_string(),
                        url: i_url,
                        width: child.get("original_width").and_then(|v| v.as_u64()).map(|n| n as u32),
                        height: child.get("original_height").and_then(|v| v.as_u64()).map(|n| n as u32),
                        thumbnail_url: None,
                        is_video: false,
                        duration_secs: None,
                        audio_url: None,
                        has_audio: false,
                    });
                }
            }
            let thumbnail_url = images.first().cloned();
            return (images, videos, media_items, "carousel".to_string(), thumbnail_url);
        }
    }

    // Case 3: Single item handling (Photo, Video, or Reel)
    let is_video = node
        .get("is_video")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || node.get("video_url").is_some()
        || node.get("video_versions").is_some();

    let node_id = node
        .get("shortcode")
        .or_else(|| node.get("id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let img_url = select_highest_resolution_image(node);

    let vid_url = if let Some(v) = node.get("video_url").and_then(|v| v.as_str()) {
        Some(v.to_string())
    } else if let Some(v_arr) = node.get("video_versions").and_then(|v| v.as_array()) {
        v_arr
            .iter()
            .max_by_key(|v| {
                let w = v.get("width").and_then(|x| x.as_u64()).unwrap_or(0);
                let h = v.get("height").and_then(|x| x.as_u64()).unwrap_or(0);
                w.saturating_mul(h)
            })
            .and_then(|v| v.get("url"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    };

    let width = node
        .get("dimensions")
        .and_then(|d| d.get("width"))
        .or_else(|| node.get("original_width"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let height = node
        .get("dimensions")
        .and_then(|d| d.get("height"))
        .or_else(|| node.get("original_height"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    if is_video {
        let (audio_url, has_audio, _, _) = extract_audio_from_node(node);
        if let Some(ref v_url) = vid_url {
            videos.push(v_url.clone());
        }
        if let Some(ref i_url) = img_url {
            images.push(i_url.clone());
        }
        let thumbnail_url = img_url.clone();
        media_items.push(InstagramMediaItem {
            id: node_id,
            media_type: "video".to_string(),
            url: vid_url.unwrap_or_else(|| img_url.clone().unwrap_or_default()),
            width,
            height,
            thumbnail_url: img_url,
            is_video: true,
            duration_secs: node.get("video_duration").and_then(|v| v.as_f64()),
            audio_url,
            has_audio,
        });
        (images, videos, media_items, "video".to_string(), thumbnail_url)
    } else {
        if let Some(ref i_url) = img_url {
            images.push(i_url.clone());
        }
        let thumbnail_url = img_url.clone();
        if let Some(i_url) = img_url {
            media_items.push(InstagramMediaItem {
                id: node_id,
                media_type: "photo".to_string(),
                url: i_url,
                width,
                height,
                thumbnail_url: None,
                is_video: false,
                duration_secs: None,
                audio_url: None,
                has_audio: false,
            });
        }
        (images, videos, media_items, "photo".to_string(), thumbnail_url)
    }
}

// ── Markdown Summary Generator (Pyramid Layout) ──────────────────────────────

/// Converts a Unix timestamp (seconds since epoch) into a UTC formatted string `YYYY-MM-DD HH:MM:SS UTC`.
fn format_timestamp_utc(ts: i64) -> String {
    let secs_in_day = 86400;
    let mut days = ts / secs_in_day;
    let mut rem_secs = ts % secs_in_day;
    if rem_secs < 0 {
        rem_secs += secs_in_day;
        days -= 1;
    }
    let hours = rem_secs / 3600;
    let minutes = (rem_secs % 3600) / 60;
    let seconds = rem_secs % 60;

    // Civil date algorithm (Howard Hinnant Euclidean Affine algorithm)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", y, m, d, hours, minutes, seconds)
}

/// Formats integer numbers with standard comma groupings (e.g. 12450 -> "12,450").
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + (len / 3));

    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

/// Generates a clean human-readable Markdown summary formatted for AI agents (Pyramid structure).
pub fn generate_markdown_summary(post: &InstagramPost) -> String {
    let mut md = String::with_capacity(1024);

    let profile_link = &post.author.profile_url;
    let author_display = match &post.author.full_name {
        Some(name) if !name.is_empty() && name != &post.author.username => {
            format!("{} ([@{}]({}))", name, post.author.username, profile_link)
        }
        _ => {
            format!("[@{}]({})", post.author.username, profile_link)
        }
    };

    // 1. Apex: Header & Metadata
    md.push_str(&format!("# Instagram Post by @{}\n\n", post.author.username));
    md.push_str(&format!("- **Author**: {}\n", author_display));
    md.push_str(&format!("- **Post URL**: [{}]({})\n", post.url, post.url));
    md.push_str(&format!("- **Type**: {}\n", capitalize(&post.media_type)));

    if let Some(ts) = post.taken_at {
        md.push_str(&format!("- **Published**: {}\n", format_timestamp_utc(ts)));
    }

    if post.like_count.is_some() || post.comment_count.is_some() {
        let likes = post.like_count.map(|l| format!("{} likes", format_number(l))).unwrap_or_default();
        let comments = post.comment_count.map(|c| format!("{} comments", format_number(c))).unwrap_or_default();
        let stats_str = match (!likes.is_empty(), !comments.is_empty()) {
            (true, true) => format!("{}, {}", likes, comments),
            (true, false) => likes,
            (false, true) => comments,
            (false, false) => String::new(),
        };
        if !stats_str.is_empty() {
            md.push_str(&format!("- **Engagement**: {}\n", stats_str));
        }
    }

    md.push_str("\n---\n\n");

    // 2. Body: Caption
    md.push_str("### Caption\n\n");
    if post.caption.is_empty() {
        md.push_str("*(No caption provided)*\n\n");
    } else {
        md.push_str(&post.caption);
        md.push_str("\n\n");
    }

    // 3. Context: Hashtags
    if !post.hashtags.is_empty() {
        md.push_str("### Hashtags\n\n");
        let tags_formatted: Vec<String> = post.hashtags.iter().map(|t| format!("`#{}`", t)).collect();
        md.push_str(&tags_formatted.join(" "));
        md.push_str("\n\n");
    }

    // 4. Foundation: Media Gallery
    md.push_str("### Media Gallery\n\n");
    let total_images = post.images.len();
    let total_videos = post.videos.len();

    if total_images == 0 && total_videos == 0 && post.audio_url.is_none() {
        md.push_str("*(No attached media found)*\n");
    } else {
        // Direct audio stream track for AI speech processing
        if let Some(ref audio) = post.audio_url {
            let music_info = match (&post.audio_title, &post.audio_artist) {
                (Some(title), Some(artist)) => format!(" (Track: \"{}\" by {})", title, artist),
                (Some(title), None) => format!(" (Track: \"{}\")", title),
                _ => String::new(),
            };
            md.push_str(&format!(
                "- **Audio Track**: [Direct Audio Stream (.m4a/.mp3)]({}) 🎵 *(Optimized for AI speech transcription & translation{})*\n",
                audio, music_info
            ));
        }

        let mut item_num = 1;

        // Display individual items from media_items if present
        if !post.media_items.is_empty() {
            for item in &post.media_items {
                if item.is_video {
                    let audio_badge = if item.has_audio || post.has_audio {
                        " 🎬 *(Includes audio track)*"
                    } else {
                        " 🔇 *(Muted / Video-only)*"
                    };
                    md.push_str(&format!(
                        "- **Item {} (Video)**: [Direct Video Stream (.mp4)]({}){}\n",
                        item_num, item.url, audio_badge
                    ));
                    if let Some(ref thumb) = item.thumbnail_url {
                        md.push_str(&format!("  - Poster Image: [Thumbnail]({})\n", thumb));
                    }
                } else {
                    md.push_str(&format!(
                        "- **Item {} (Photo)**: [High-Resolution Image]({})\n",
                        item_num, item.url
                    ));
                }
                item_num += 1;
            }
        } else {
            for img_url in &post.images {
                md.push_str(&format!(
                    "- **Item {} (Photo)**: [High-Resolution Image]({})\n",
                    item_num, img_url
                ));
                item_num += 1;
            }
            for vid_url in &post.videos {
                let audio_badge = if post.has_audio {
                    " 🎬 *(Includes audio track)*"
                } else {
                    " 🔇 *(Muted / Video-only)*"
                };
                md.push_str(&format!(
                    "- **Item {} (Video)**: [Direct Video Stream (.mp4)]({}){}\n",
                    item_num, vid_url, audio_badge
                ));
                item_num += 1;
            }
        }
    }

    md
}

// ── Headless Extraction Layer ────────────────────────────────────────────────

/// Builds reqwest client configured with browser headers and compression.
fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .unwrap_or_default()
}

/// Parses GraphQL media node into an `InstagramPost`.
pub fn parse_gql_media_node(media: &Value, shortcode: &str) -> Result<InstagramPost, InstagramExtractError> {
    let id = media.get("id").and_then(|v| v.as_str()).unwrap_or(shortcode).to_string();

    let owner = media.get("owner");
    let username = owner
        .and_then(|o| o.get("username"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let full_name = owner
        .and_then(|o| o.get("full_name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let avatar_url = owner
        .and_then(|o| o.get("profile_pic_url"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let is_verified = owner
        .and_then(|o| o.get("is_verified"))
        .and_then(|v| v.as_bool());
    let profile_url = format!("https://www.instagram.com/{}/", username);

    let author = InstagramAuthor {
        username,
        full_name,
        profile_url,
        avatar_url,
        is_verified,
    };

    let raw_caption = media
        .get("edge_media_to_caption")
        .and_then(|e| e.get("edges"))
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|edge| edge.get("node"))
        .and_then(|n| n.get("text"))
        .and_then(|t| t.as_str())
        .or_else(|| {
            media
                .get("caption")
                .and_then(|c| c.get("text"))
                .and_then(|t| t.as_str())
        })
        .unwrap_or("");

    let caption = normalize_caption(raw_caption);
    let hashtags = extract_hashtags(&caption);
    let mentions = extract_mentions(&caption);

    let taken_at = media
        .get("taken_at_timestamp")
        .or_else(|| media.get("taken_at"))
        .and_then(|v| v.as_i64());

    let like_count = media
        .get("edge_media_preview_like")
        .or_else(|| media.get("edge_liked_by"))
        .and_then(|l| l.get("count"))
        .and_then(|v| v.as_u64())
        .or_else(|| media.get("like_count").and_then(|v| v.as_u64()));

    let comment_count = media
        .get("edge_media_to_parent_comment")
        .or_else(|| media.get("edge_media_to_comment"))
        .and_then(|c| c.get("count"))
        .and_then(|v| v.as_u64())
        .or_else(|| media.get("comment_count").and_then(|v| v.as_u64()));

    let (images, videos, media_items, media_type, thumbnail_url) = unroll_media_from_node(media);
    let (audio_url, has_audio, audio_title, audio_artist) = extract_audio_from_node(media);

    let mut post = InstagramPost {
        id,
        shortcode: shortcode.to_string(),
        url: format!("https://www.instagram.com/p/{}/", shortcode),
        author,
        caption,
        hashtags,
        mentions,
        images,
        videos,
        audio_url,
        has_audio,
        audio_title,
        audio_artist,
        media_type,
        media_items,
        thumbnail_url,
        taken_at,
        like_count,
        comment_count,
        markdown: String::new(),
    };

    post.markdown = generate_markdown_summary(&post);
    Ok(post)
}

/// Tier 1: Anonymous Polaris GraphQL query.
pub async fn fetch_via_graphql(client: &reqwest::Client, shortcode: &str) -> Result<InstagramPost, InstagramExtractError> {
    let lsd_token = "AVrX1234";
    let csrf_token = "csrftoken_anon_1234567890abcdef";
    let device_id = uuid::Uuid::new_v4().to_string();
    let machine_id = "mid_anon_12345";

    let cookie = format!(
        "csrftoken={}; ig_did={}; mid={}; ig_nrcb=1; wd=1280x720; dpr=2",
        csrf_token, device_id, machine_id
    );

    let variables = json!({
        "shortcode": shortcode,
        "fetch_tagged_user_count": null,
        "hoisted_comment_id": null,
        "hoisted_reply_id": null
    }).to_string();

    let doc_ids = [GQL_DOC_ID_PRIMARY, GQL_DOC_ID_SECONDARY];

    for doc_id in doc_ids {
        let params = [
            ("doc_id", doc_id),
            ("variables", &variables),
            ("fb_api_caller_class", "RelayModern"),
            ("fb_api_req_friendly_name", "PolarisPostActionLoadPostQueryQuery"),
            ("server_timestamps", "true"),
            ("__d", "www"),
            ("__user", "0"),
            ("__a", "1"),
            ("__req", "b"),
            ("lsd", lsd_token),
        ];

        let res = match client
            .post("https://www.instagram.com/graphql/query")
            .header("Accept", "*/*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-origin")
            .header("X-Requested-With", "XMLHttpRequest")
            .header("X-IG-App-ID", IG_APP_ID)
            .header("X-FB-LSD", lsd_token)
            .header("X-CSRFToken", csrf_token)
            .header("X-FB-Friendly-Name", "PolarisPostActionLoadPostQueryQuery")
            .header("Referer", "https://www.instagram.com/")
            .header("Origin", "https://www.instagram.com")
            .header("Cookie", &cookie)
            .form(&params)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("[instagram] GraphQL network error with doc_id {}: {}", doc_id, e);
                continue;
            }
        };

        if res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
            || res.status() == reqwest::StatusCode::FORBIDDEN
            || res.status() == reqwest::StatusCode::UNAUTHORIZED
        {
            tracing::debug!("[instagram] GraphQL rate limited (status {})", res.status());
            continue;
        }

        if !res.status().is_success() {
            continue;
        }

        let val: Value = match res.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };

        let media = val
            .get("data")
            .and_then(|d| d.get("xdt_shortcode_media").or_else(|| d.get("shortcode_media")));

        if let Some(media_node) = media {
            if !media_node.is_null() {
                return parse_gql_media_node(media_node, shortcode);
            }
        }
    }

    Err(InstagramExtractError::NotFound(format!(
        "Post '{}' not found via GraphQL queries",
        shortcode
    )))
}

/// Determines whether an Instagram embed HTML indicates a private post or login requirement,
/// ensuring that user captions mentioning private keywords do not trigger false-positive rejections.
fn is_embed_private_or_login_required(html: &str) -> bool {
    // 1. Check for explicit Instagram embed error container classes
    if html.contains("EmbedEmpty")
        || html.contains("EmbedPrivatePost")
        || html.contains("EmbedError")
        || html.contains("EmbedEmptyMessage")
    {
        return true;
    }

    // 2. Strip user caption containers so caption text mentioning private keywords is ignored
    let stripped = if let Ok(caption_wrapper_re) = Regex::new(r#"(?s)<div[^>]+class="[^"]*Caption[^"]*"[^>]*>.*?</div>\s*</div>"#) {
        caption_wrapper_re.replace_all(html, "")
    } else {
        std::borrow::Cow::Borrowed(html)
    };
    let stripped = if let Ok(caption_text_re) = Regex::new(r#"(?s)<div[^>]+class="[^"]*Caption(?:Text)?[^"]*"[^>]*>.*?</div>"#) {
        caption_text_re.replace_all(&stripped, "").to_string()
    } else {
        stripped.into_owned()
    };

    // 3. Strip JSON caption text if present
    let stripped = if let Ok(json_caption_re) = Regex::new(r#""text"\s*:\s*"((?:\\.|[^"\\])*)""#) {
        json_caption_re.replace_all(&stripped, "").to_string()
    } else {
        stripped
    };

    // 4. Check for private post / login barrier markers in the non-caption markup
    stripped.contains("This post is private")
        || stripped.contains("/accounts/login")
        || stripped.contains("accounts/login")
}

/// Parses public captioned embed page HTML into an `InstagramPost`.
pub fn parse_embed_html(html: &str, shortcode: &str) -> Result<InstagramPost, InstagramExtractError> {

    // Pattern 1: Direct "contextJSON" string extraction from init script
    if let Ok(re) = Regex::new(r#""contextJSON"\s*:\s*"((?:\\.|[^"\\])*)""#) {
        if let Some(cap) = re.captures(html) {
            if let Some(escaped_str) = cap.get(1) {
                let full_json_str = format!("\"{}\"", escaped_str.as_str());
                if let Ok(context_json) = serde_json::from_str::<String>(&full_json_str) {
                    if let Ok(context) = serde_json::from_str::<Value>(&context_json) {
                        if let Some(media) = context
                            .get("shortcode_media")
                            .or_else(|| context.get("xdt_shortcode_media"))
                            .or_else(|| context.pointer("/graphql/shortcode_media"))
                            .or_else(|| context.pointer("/data/xdt_shortcode_media"))
                            .or_else(|| context.pointer("/data/shortcode_media"))
                            .filter(|m| !m.is_null())
                            .or_else(|| {
                                if context.get("owner").is_some() || context.get("display_url").is_some() {
                                    Some(&context)
                                } else {
                                    None
                                }
                            })
                        {
                            return parse_gql_media_node(media, shortcode);
                        }
                    }
                }
            }
        }
    }

    // Pattern 2: window.__additionalDataLoaded
    if let Ok(re) = Regex::new(r#"window\.__additionalDataLoaded\('(?:extra|[^']+)',\s*(\{.*?\})\s*\);?"#) {
        if let Some(cap) = re.captures(html) {
            if let Some(json_slice) = cap.get(1) {
                if let Ok(val) = serde_json::from_str::<Value>(json_slice.as_str()) {
                    if let Some(media) = val
                        .get("graphql")
                        .and_then(|g| g.get("shortcode_media"))
                        .or_else(|| val.get("shortcode_media"))
                        .or_else(|| val.get("xdt_shortcode_media"))
                    {
                        return parse_gql_media_node(media, shortcode);
                    }
                }
            }
        }
    }

    // Pattern 3: Embedded script tag data-sjs
    if let Ok(re) = Regex::new(r#"<script[^>]+data-sjs[^>]*>(.*?)</script>"#) {
        for cap in re.captures_iter(html) {
            if let Some(script_content) = cap.get(1) {
                if let Ok(val) = serde_json::from_str::<Value>(script_content.as_str()) {
                    if let Some(media) = val
                        .pointer("/require/0/3/0/__bbox/result/data/xdt_shortcode_media")
                        .or_else(|| val.pointer("/require/0/3/0/__bbox/result/data/shortcode_media"))
                    {
                        return parse_gql_media_node(media, shortcode);
                    }
                }
            }
        }
    }

    // Pattern 4: Fallback DOM Extraction from embed HTML
    let username_re = Regex::new(r#"<a[^>]+class="[^"]*CaptionUsername[^"]*"[^>]*>([^<]+)</a>"#).ok();
    let username = username_re
        .and_then(|r| r.captures(html))
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
        .unwrap_or_else(|| "instagram_user".to_string());

    let caption_re = Regex::new(r#"<div[^>]+class="[^"]*Caption(?:Text)?[^"]*"[^>]*>(.*?)</div>"#).ok();
    let raw_caption = caption_re
        .and_then(|r| r.captures(html))
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .unwrap_or("");
    let caption = normalize_caption(raw_caption);

    let img_re = Regex::new(r#"<img[^>]+class="[^"]*EmbeddedMediaImage[^"]*"[^>]+src="([^"]+)""#).ok();
    let img_url = img_re
        .and_then(|r| r.captures(html))
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    let vid_re = Regex::new(r#"<video[^>]+src="([^"]+)""#).ok();
    let vid_url = vid_re
        .and_then(|r| r.captures(html))
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    let mut images = Vec::new();
    let mut videos = Vec::new();
    let mut media_items = Vec::new();

    if let Some(ref v_url) = vid_url {
        videos.push(v_url.clone());
    }
    if let Some(ref i_url) = img_url {
        images.push(i_url.clone());
    }

    let is_video = !videos.is_empty();
    let media_type = if is_video { "video".to_string() } else { "photo".to_string() };

    if is_video {
        media_items.push(InstagramMediaItem {
            id: Some(shortcode.to_string()),
            media_type: "video".to_string(),
            url: vid_url.unwrap_or_else(|| img_url.clone().unwrap_or_default()),
            width: None,
            height: None,
            thumbnail_url: img_url.clone(),
            is_video: true,
            duration_secs: None,
            audio_url: None,
            has_audio: true,
        });
    } else if let Some(i_url) = img_url.clone() {
        media_items.push(InstagramMediaItem {
            id: Some(shortcode.to_string()),
            media_type: "photo".to_string(),
            url: i_url,
            width: None,
            height: None,
            thumbnail_url: None,
            is_video: false,
            duration_secs: None,
            audio_url: None,
            has_audio: false,
        });
    }

    let author = InstagramAuthor {
        username: username.clone(),
        full_name: None,
        profile_url: format!("https://www.instagram.com/{}/", username),
        avatar_url: None,
        is_verified: None,
    };

    let hashtags = extract_hashtags(&caption);
    let mentions = extract_mentions(&caption);

    if images.is_empty() && videos.is_empty() {
        if is_embed_private_or_login_required(html) {
            return Err(InstagramExtractError::PrivateOrLoginRequired(
                "This Instagram post is private, restricted, or requires user login.".into(),
            ));
        }
        return Err(InstagramExtractError::NotFound(
            "Could not parse media details from embed page".into(),
        ));
    }

    let mut post = InstagramPost {
        id: shortcode.to_string(),
        shortcode: shortcode.to_string(),
        url: format!("https://www.instagram.com/p/{}/", shortcode),
        author,
        caption,
        hashtags,
        mentions,
        images,
        videos,
        audio_url: None,
        has_audio: is_video,
        audio_title: None,
        audio_artist: None,
        media_type,
        media_items,
        thumbnail_url: img_url,
        taken_at: None,
        like_count: None,
        comment_count: None,
        markdown: String::new(),
    };

    post.markdown = generate_markdown_summary(&post);
    Ok(post)
}

/// Tier 2: Public captioned embed page scraper.
pub async fn fetch_via_embed(client: &reqwest::Client, shortcode: &str) -> Result<InstagramPost, InstagramExtractError> {
    let url = format!("https://www.instagram.com/p/{}/embed/captioned/", shortcode);
    let res = client
        .get(&url)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Sec-Fetch-Dest", "iframe")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "cross-site")
        .header("Referer", "https://www.instagram.com/")
        .send()
        .await
        .map_err(|e| InstagramExtractError::UpstreamError(e.to_string()))?;

    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(InstagramExtractError::NotFound(format!("Post '{}' not found", shortcode)));
    }

    let html = res
        .text()
        .await
        .map_err(|e| InstagramExtractError::UpstreamError(e.to_string()))?;

    parse_embed_html(&html, shortcode)
}

/// Tier 3: `yt-dlp` metadata extraction fallback.
pub async fn fetch_via_ytdlp(url: &str, shortcode: &str) -> Result<InstagramPost, InstagramExtractError> {
    let ytdlp_path = match omniget_core::core::ytdlp::ensure_ytdlp().await {
        Ok(p) => p,
        Err(_) => PathBuf::from("yt-dlp"),
    };

    let mut cmd = tokio::process::Command::new(&ytdlp_path);
    cmd.args([
        "--dump-single-json",
        "--no-download",
        "--skip-download",
        "--no-warnings",
        "--socket-timeout",
        "15",
        "--retries",
        "2",
        "--user-agent",
        USER_AGENT,
        url,
    ]);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = match tokio::time::timeout(Duration::from_secs(20), cmd.output()).await {
        Ok(res) => res.map_err(|e| InstagramExtractError::UpstreamError(format!("Failed to execute yt-dlp: {}", e)))?,
        Err(_) => return Err(InstagramExtractError::Timeout("yt-dlp execution timed out after 20s".into())),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lower_stderr = stderr.to_lowercase();
        if lower_stderr.contains("login") || lower_stderr.contains("private") {
            return Err(InstagramExtractError::PrivateOrLoginRequired("Post is private or requires login".into()));
        }
        if lower_stderr.contains("does not exist") || lower_stderr.contains("404") {
            return Err(InstagramExtractError::NotFound(format!("Post '{}' not found", shortcode)));
        }
        return Err(InstagramExtractError::UpstreamError(format!("yt-dlp failed: {}", stderr.trim())));
    }

    let json_val: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| InstagramExtractError::UpstreamError(format!("Failed to parse yt-dlp JSON: {}", e)))?;

    let post_id = json_val.get("id").and_then(|v| v.as_str()).unwrap_or(shortcode).to_string();
    let raw_caption = json_val.get("description").and_then(|v| v.as_str()).unwrap_or("");
    let caption = normalize_caption(raw_caption);
    let username = json_val
        .get("channel")
        .or_else(|| json_val.get("uploader_id"))
        .or_else(|| json_val.get("uploader"))
        .and_then(|v| v.as_str())
        .unwrap_or("instagram_user")
        .to_string();
    let full_name = json_val.get("uploader").and_then(|v| v.as_str()).map(|s| s.to_string());
    let taken_at = json_val.get("timestamp").and_then(|v| v.as_i64());
    let like_count = json_val.get("like_count").and_then(|v| v.as_u64());
    let comment_count = json_val.get("comment_count").and_then(|v| v.as_u64());

    let mut images: Vec<String> = Vec::new();
    let mut videos: Vec<String> = Vec::new();
    let mut media_items: Vec<InstagramMediaItem> = Vec::new();

    // Scan all formats for standalone audio track
    let mut audio_url = None;
    let mut best_audio_br: u64 = 0;
    if let Some(formats) = json_val.get("formats").and_then(Value::as_array) {
        for f in formats {
            let acodec = f.get("acodec").and_then(Value::as_str).unwrap_or("none");
            let vcodec = f.get("vcodec").and_then(Value::as_str).unwrap_or("none");
            let u = f.get("url").and_then(Value::as_str).unwrap_or("");
            if acodec != "none" && !acodec.is_empty() && u.starts_with("http") {
                let is_audio_only = vcodec == "none" || vcodec.is_empty();
                let abr = f.get("abr").and_then(Value::as_f64).unwrap_or(0.0) as u64;
                let tbr = f.get("tbr").and_then(Value::as_f64).unwrap_or(0.0) as u64;
                let br = abr.max(tbr);
                if is_audio_only && br >= best_audio_br {
                    best_audio_br = br;
                    audio_url = Some(u.to_string());
                } else if audio_url.is_none() && is_audio_only {
                    audio_url = Some(u.to_string());
                }
            }
        }
    }
    let mut has_audio = audio_url.is_some();

    // Check for carousel playlist entries
    if let Some(entries) = json_val.get("entries").and_then(|v| v.as_array()) {
        for entry in entries {
            let mut slide_has_video = false;
            if let Some(formats) = entry.get("formats").and_then(|v| v.as_array()) {
                // Check if entry has standalone audio
                let mut entry_audio_url = None;
                let mut entry_best_abr = 0u64;
                for f in formats {
                    let acodec = f.get("acodec").and_then(Value::as_str).unwrap_or("none");
                    let vcodec = f.get("vcodec").and_then(Value::as_str).unwrap_or("none");
                    let u = f.get("url").and_then(Value::as_str).unwrap_or("");
                    if acodec != "none" && !acodec.is_empty() && (vcodec == "none" || vcodec.is_empty()) && u.starts_with("http") {
                        let abr = f.get("abr").and_then(Value::as_f64).unwrap_or(0.0) as u64;
                        if abr >= entry_best_abr {
                            entry_best_abr = abr;
                            entry_audio_url = Some(u.to_string());
                        }
                    }
                }

                // 1. Prioritize progressive muxed format with video AND audio
                let best_muxed = formats
                    .iter()
                    .filter(|f| {
                        let vcodec = f.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
                        let acodec = f.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
                        let u = f.get("url").and_then(|v| v.as_str()).unwrap_or("");
                        vcodec != "none" && !vcodec.is_empty()
                            && acodec != "none" && !acodec.is_empty()
                            && u.starts_with("http")
                    })
                    .max_by_key(|f| {
                        let w = f.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                        let h = f.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
                        let tbr = f.get("tbr").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64;
                        (w.saturating_mul(h), tbr)
                    });

                // 2. Fallback to video-only format
                let best_f = best_muxed.or_else(|| {
                    formats
                        .iter()
                        .filter(|f| {
                            let vcodec = f.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
                            let u = f.get("url").and_then(|v| v.as_str()).unwrap_or("");
                            vcodec != "none" && !vcodec.is_empty() && u.starts_with("http")
                        })
                        .max_by_key(|f| {
                            let w = f.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                            let h = f.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
                            let tbr = f.get("tbr").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64;
                            (w.saturating_mul(h), tbr)
                        })
                });

                if let Some(f) = best_f {
                    if let Some(vurl) = f.get("url").and_then(|v| v.as_str()) {
                        let acodec = f.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
                        let slide_has_audio = (acodec != "none" && !acodec.is_empty()) || entry_audio_url.is_some();
                        if slide_has_audio {
                            has_audio = true;
                        }
                        videos.push(vurl.to_string());
                        media_items.push(InstagramMediaItem {
                            id: entry.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()),
                            media_type: "video".into(),
                            url: vurl.to_string(),
                            width: entry.get("width").and_then(|v| v.as_u64()).map(|n| n as u32),
                            height: entry.get("height").and_then(|v| v.as_u64()).map(|n| n as u32),
                            thumbnail_url: entry.get("thumbnail").and_then(|v| v.as_str()).map(|s| s.to_string()),
                            is_video: true,
                            duration_secs: entry.get("duration").and_then(|v| v.as_f64()),
                            audio_url: entry_audio_url.or_else(|| audio_url.clone()),
                            has_audio: slide_has_audio,
                        });
                        slide_has_video = true;
                    }
                }
            }
            if !slide_has_video {
                if let Some(thumb) = entry.get("thumbnail").and_then(|v| v.as_str()) {
                    if !thumb.is_empty() && !images.contains(&thumb.to_string()) {
                        images.push(thumb.to_string());
                        media_items.push(InstagramMediaItem {
                            id: entry.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()),
                            media_type: "photo".into(),
                            url: thumb.to_string(),
                            width: entry.get("width").and_then(|v| v.as_u64()).map(|n| n as u32),
                            height: entry.get("height").and_then(|v| v.as_u64()).map(|n| n as u32),
                            thumbnail_url: None,
                            is_video: false,
                            duration_secs: None,
                            audio_url: None,
                            has_audio: false,
                        });
                    }
                }
            }
        }
    } else {
        // Single item / Reel
        if let Some(formats) = json_val.get("formats").and_then(|v| v.as_array()) {
            // 1. Prioritize progressive muxed format with video AND audio
            let best_muxed = formats
                .iter()
                .filter(|f| {
                    let vcodec = f.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
                    let acodec = f.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
                    let u = f.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    vcodec != "none" && !vcodec.is_empty()
                        && acodec != "none" && !acodec.is_empty()
                        && u.starts_with("http")
                })
                .max_by_key(|f| {
                    let w = f.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                    let h = f.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
                    let tbr = f.get("tbr").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64;
                    (w.saturating_mul(h), tbr)
                });

            // 2. Fallback to video-only format
            let best_f = best_muxed.or_else(|| {
                formats
                    .iter()
                    .filter(|f| {
                        let vcodec = f.get("vcodec").and_then(|v| v.as_str()).unwrap_or("none");
                        let u = f.get("url").and_then(|v| v.as_str()).unwrap_or("");
                        vcodec != "none" && !vcodec.is_empty() && u.starts_with("http")
                    })
                    .max_by_key(|f| {
                        let w = f.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                        let h = f.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
                        let tbr = f.get("tbr").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64;
                        (w.saturating_mul(h), tbr)
                    })
            });

            if let Some(f) = best_f {
                if let Some(vurl) = f.get("url").and_then(|v| v.as_str()) {
                    let acodec = f.get("acodec").and_then(|v| v.as_str()).unwrap_or("none");
                    let item_has_audio = (acodec != "none" && !acodec.is_empty()) || audio_url.is_some();
                    if item_has_audio {
                        has_audio = true;
                    }
                    videos.push(vurl.to_string());
                    media_items.push(InstagramMediaItem {
                        id: Some(post_id.clone()),
                        media_type: "video".into(),
                        url: vurl.to_string(),
                        width: json_val.get("width").and_then(|v| v.as_u64()).map(|n| n as u32),
                        height: json_val.get("height").and_then(|v| v.as_u64()).map(|n| n as u32),
                        thumbnail_url: json_val.get("thumbnail").and_then(|v| v.as_str()).map(|s| s.to_string()),
                        is_video: true,
                        duration_secs: json_val.get("duration").and_then(|v| v.as_f64()),
                        audio_url: audio_url.clone(),
                        has_audio: item_has_audio,
                    });
                }
            }
        }

        if let Some(thumb) = json_val.get("thumbnail").and_then(|v| v.as_str()) {
            if !thumb.is_empty() {
                images.push(thumb.to_string());
                if videos.is_empty() {
                    media_items.push(InstagramMediaItem {
                        id: Some(post_id.clone()),
                        media_type: "photo".into(),
                        url: thumb.to_string(),
                        width: json_val.get("width").and_then(|v| v.as_u64()).map(|n| n as u32),
                        height: json_val.get("height").and_then(|v| v.as_u64()).map(|n| n as u32),
                        thumbnail_url: None,
                        is_video: false,
                        duration_secs: None,
                        audio_url: None,
                        has_audio: false,
                    });
                }
            }
        }
    }

    let media_type = if images.len() + videos.len() > 1 {
        "carousel".to_string()
    } else if !videos.is_empty() {
        "video".to_string()
    } else {
        "photo".to_string()
    };

    let author = InstagramAuthor {
        username: username.clone(),
        full_name,
        profile_url: format!("https://www.instagram.com/{}/", username),
        avatar_url: None,
        is_verified: None,
    };

    let hashtags = extract_hashtags(&caption);
    let mentions = extract_mentions(&caption);
    let thumbnail_url = images.first().cloned();

    let mut post = InstagramPost {
        id: post_id,
        shortcode: shortcode.to_string(),
        url: format!("https://www.instagram.com/p/{}/", shortcode),
        author,
        caption,
        hashtags,
        mentions,
        images,
        videos,
        audio_url,
        has_audio,
        audio_title: None,
        audio_artist: None,
        media_type,
        media_items,
        thumbnail_url,
        taken_at,
        like_count,
        comment_count,
        markdown: String::new(),
    };

    post.markdown = generate_markdown_summary(&post);
    Ok(post)
}

/// Resolves shortened share links (`/share/{id}/`) by following redirects to obtain final canonical post URL.
async fn resolve_share_redirect(
    client: &reqwest::Client,
    share_url: &str,
) -> Result<InstagramUrlInfo, InstagramExtractError> {
    let res = client
        .get(share_url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .send()
        .await
        .map_err(|e| InstagramExtractError::UpstreamError(format!("Failed to resolve share link: {}", e)))?;

    let final_url = res.url().as_str();
    parse_instagram_url(final_url)
}

/// Orchestrates multi-tier headless Instagram extraction:
/// Tier 1: Anonymous Polaris GraphQL -> Tier 2: Public Captioned Embed -> Tier 3: `yt-dlp` Video Stream Fallback.
pub async fn extract_instagram_post(input: &str) -> Result<InstagramPost, InstagramExtractError> {
    let mut url_info = parse_instagram_url(input)?;
    let client = build_http_client();

    // If input is an unresolved share redirect link, resolve destination first
    if url_info.is_share_link {
        if let Ok(resolved) = resolve_share_redirect(&client, &url_info.canonical_url).await {
            url_info = resolved;
        }
    }

    // Tier 1: Anonymous GraphQL Query
    match tokio::time::timeout(Duration::from_secs(10), fetch_via_graphql(&client, &url_info.shortcode)).await {
        Ok(Ok(post)) => return Ok(post),
        Ok(Err(e)) => tracing::debug!("[instagram] GraphQL tier failed: {}. Falling back to embed.", e),
        Err(_) => tracing::debug!("[instagram] GraphQL tier timed out. Falling back to embed."),
    }

    // Tier 2: Captioned Embed Page Scraper
    match tokio::time::timeout(Duration::from_secs(10), fetch_via_embed(&client, &url_info.shortcode)).await {
        Ok(Ok(post)) => {
            // If Reel was extracted without video stream, advance to yt-dlp to recover mp4 link
            if !(url_info.is_reel && post.videos.is_empty()) {
                return Ok(post);
            }
            tracing::debug!("[instagram] Reel embed lacked video URL. Falling back to yt-dlp.");
        }
        Ok(Err(e)) => match e {
            InstagramExtractError::PrivateOrLoginRequired(_) => return Err(e),
            _ => tracing::debug!("[instagram] Embed tier failed: {}. Falling back to yt-dlp.", e),
        },
        Err(_) => tracing::debug!("[instagram] Embed tier timed out. Falling back to yt-dlp."),
    }

    // Tier 3: yt-dlp Video Stream Fallback
    match tokio::time::timeout(Duration::from_secs(20), fetch_via_ytdlp(&url_info.canonical_url, &url_info.shortcode)).await {
        Ok(Ok(post)) => Ok(post),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(InstagramExtractError::Timeout("All extraction tiers timed out".into())),
    }
}

// ── MCP JSON-RPC Adapter ─────────────────────────────────────────────────────

/// JSON-RPC 2.0 `tools/call` handler for `instagram_post`.
///
/// Conforms to MCP tool result schema:
/// `{ content: [{ type: "text", text: markdown }], post: InstagramPost, isError: bool }`
pub async fn call_instagram_post(arguments: Value) -> Result<Value, anyhow::Error> {
    let args: InstagramArgs = match serde_json::from_value(arguments.clone()) {
        Ok(a) => a,
        Err(_) => {
            let url_str = arguments.get("url").and_then(|v| v.as_str()).map(|s| s.to_string());
            InstagramArgs {
                url: url_str,
                shortcode: None,
            }
        }
    };

    let input = args
        .url
        .as_deref()
        .or(args.shortcode.as_deref())
        .unwrap_or("")
        .trim();

    if input.is_empty() {
        return Ok(json!({
            "content": [
                {
                    "type": "text",
                    "text": "Failed to extract Instagram post: URL or shortcode cannot be empty"
                }
            ],
            "isError": true
        }));
    }

    match extract_instagram_post(input).await {
        Ok(post) => {
            let markdown = post.markdown.clone();
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": markdown
                    }
                ],
                "post": post,
                "isError": false
            }))
        }
        Err(e) => Ok(json!({
            "content": [
                {
                    "type": "text",
                    "text": format!("Failed to extract Instagram post: {}", e)
                }
            ],
            "isError": true
        })),
    }
}

// ── Unit Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── URL Parsing Tests (24 Matrix Scenarios) ──────────────────────────────

    #[test]
    fn test_standard_post_url() {
        let info = parse_instagram_url("https://www.instagram.com/p/C_abc123/").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.kind, "post");
        assert_eq!(info.canonical_url, "https://www.instagram.com/p/C_abc123/");
        assert!(!info.is_share_link);
        assert!(!info.is_reel);
    }

    #[test]
    fn test_post_no_trailing_slash() {
        let info = parse_instagram_url("https://instagram.com/p/C_abc123").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.canonical_url, "https://www.instagram.com/p/C_abc123/");
    }

    #[test]
    fn test_reel_singular_url() {
        let info = parse_instagram_url("https://www.instagram.com/reel/D-9_xZ01/").unwrap();
        assert_eq!(info.shortcode, "D-9_xZ01");
        assert_eq!(info.kind, "reel");
        assert_eq!(info.canonical_url, "https://www.instagram.com/reel/D-9_xZ01/");
        assert!(info.is_reel);
    }

    #[test]
    fn test_reels_plural_url() {
        let info = parse_instagram_url("https://www.instagram.com/reels/D-9_xZ01/").unwrap();
        assert_eq!(info.shortcode, "D-9_xZ01");
        assert_eq!(info.kind, "reel");
        assert_eq!(info.canonical_url, "https://www.instagram.com/reel/D-9_xZ01/");
        assert!(info.is_reel);
    }

    #[test]
    fn test_tv_url() {
        let info = parse_instagram_url("https://www.instagram.com/tv/B_123xyz/").unwrap();
        assert_eq!(info.shortcode, "B_123xyz");
        assert_eq!(info.kind, "tv");
        assert_eq!(info.canonical_url, "https://www.instagram.com/tv/B_123xyz/");
        assert!(!info.is_reel);
    }

    #[test]
    fn test_http_scheme() {
        let info = parse_instagram_url("http://www.instagram.com/p/C_abc123/").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.canonical_url, "https://www.instagram.com/p/C_abc123/");
    }

    #[test]
    fn test_mobile_subdomain() {
        let info = parse_instagram_url("https://m.instagram.com/p/C_abc123/").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.canonical_url, "https://www.instagram.com/p/C_abc123/");
    }

    #[test]
    fn test_ddinstagram_domain() {
        let info = parse_instagram_url("https://ddinstagram.com/p/C_abc123/").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.canonical_url, "https://www.instagram.com/p/C_abc123/");
    }

    #[test]
    fn test_ddinstagram_www_reel() {
        let info = parse_instagram_url("https://www.ddinstagram.com/reel/C_abc123/").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.canonical_url, "https://www.instagram.com/reel/C_abc123/");
        assert!(info.is_reel);
    }

    #[test]
    fn test_scheme_auto_prefix_www() {
        let info = parse_instagram_url("www.instagram.com/p/C_abc123/").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.canonical_url, "https://www.instagram.com/p/C_abc123/");
    }

    #[test]
    fn test_scheme_auto_prefix_apex() {
        let info = parse_instagram_url("instagram.com/reel/C_abc123").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.canonical_url, "https://www.instagram.com/reel/C_abc123/");
        assert!(info.is_reel);
    }

    #[test]
    fn test_query_param_stripping() {
        let info = parse_instagram_url("https://www.instagram.com/p/C_abc123/?igsh=MW0yd2==&utm_source=ig_web_copy_link").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.canonical_url, "https://www.instagram.com/p/C_abc123/");
    }

    #[test]
    fn test_fragment_stripping() {
        let info = parse_instagram_url("https://www.instagram.com/reel/C_abc123?utm_medium=copy_link#react-root").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.canonical_url, "https://www.instagram.com/reel/C_abc123/");
    }

    #[test]
    fn test_case_sensitivity_preserved() {
        let info = parse_instagram_url("https://www.instagram.com/p/C_aBc123XYZ/").unwrap();
        assert_eq!(info.shortcode, "C_aBc123XYZ");
        assert_eq!(info.canonical_url, "https://www.instagram.com/p/C_aBc123XYZ/");
    }

    #[test]
    fn test_share_p_path() {
        let info = parse_instagram_url("https://www.instagram.com/share/p/C_abc123/?igsh=123").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.kind, "post");
        assert_eq!(info.canonical_url, "https://www.instagram.com/p/C_abc123/");
        assert!(!info.is_share_link);
    }

    #[test]
    fn test_share_reel_path() {
        let info = parse_instagram_url("https://www.instagram.com/share/reel/D-9_xZ01/").unwrap();
        assert_eq!(info.shortcode, "D-9_xZ01");
        assert_eq!(info.kind, "reel");
        assert_eq!(info.canonical_url, "https://www.instagram.com/reel/D-9_xZ01/");
        assert!(!info.is_share_link);
        assert!(info.is_reel);
    }

    #[test]
    fn test_share_redirect_id() {
        let info = parse_instagram_url("https://www.instagram.com/share/BC123xyz/?igsh=MW0").unwrap();
        assert_eq!(info.shortcode, "BC123xyz");
        assert_eq!(info.kind, "share");
        assert!(info.is_share_link);
    }

    #[test]
    fn test_raw_shortcode() {
        let info = parse_instagram_url("C_abc123").unwrap();
        assert_eq!(info.shortcode, "C_abc123");
        assert_eq!(info.kind, "post");
        assert_eq!(info.canonical_url, "https://www.instagram.com/p/C_abc123/");
        assert!(!info.is_share_link);
    }

    #[test]
    fn test_empty_input() {
        assert!(matches!(
            parse_instagram_url("   \n\t"),
            Err(InstagramExtractError::InvalidInput(_))
        ));
    }

    #[test]
    fn test_missing_shortcode() {
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/p/"),
            Err(InstagramExtractError::InvalidInput(_))
        ));
    }

    #[test]
    fn test_invalid_characters() {
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/p/abc$def/"),
            Err(InstagramExtractError::InvalidInput(_))
        ));
    }

    #[test]
    fn test_foreign_domain() {
        assert!(matches!(
            parse_instagram_url("https://twitter.com/jack/status/20"),
            Err(InstagramExtractError::InvalidDomain(_))
        ));
    }

    #[test]
    fn test_spoofed_subdomain() {
        assert!(matches!(
            parse_instagram_url("https://instagram.com.attacker.com/p/C_abc123/"),
            Err(InstagramExtractError::InvalidDomain(_))
        ));
    }

    #[test]
    fn test_stories_unsupported() {
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/stories/natgeo/1234567890/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
    }

    #[test]
    fn test_direct_and_explore_unsupported() {
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/direct/t/123456/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/explore/tags/rust/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/natgeo/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
    }

    #[test]
    fn test_reel_audio_unsupported() {
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/reels/audio/1234567890/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/reel/audio/1234567890/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/reels/audio/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/reel/audio/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/reels/audio/1234567890/?igsh=MW0yd2=="),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/share/reel/audio/1234567890/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/share/reels/audio/1234567890/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
    }

    #[test]
    fn test_reel_non_post_subpaths_unsupported() {
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/reels/videos/12345/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/reels/videos/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/reel/videos/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
    }

    #[test]
    fn test_additional_unsupported_paths() {
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/highlights/natgeo/12345/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/highlight/12345/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/live/12345/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
        assert!(matches!(
            parse_instagram_url("https://www.instagram.com/guides/12345/"),
            Err(InstagramExtractError::UnsupportedUrl(_))
        ));
    }

    // ── Resolution Selection & Carousel Tests ────────────────────────────────

    #[test]
    fn test_carousel_resolution_maximization() {
        let node = json!({
            "display_url": "https://cdn.instagram.com/640.jpg",
            "display_resources": [
                { "src": "https://cdn.instagram.com/640.jpg", "config_width": 640, "config_height": 640 },
                { "src": "https://cdn.instagram.com/1080.jpg", "config_width": 1080, "config_height": 1080 },
                { "src": "https://cdn.instagram.com/1440.jpg", "config_width": 1440, "config_height": 1440 }
            ]
        });
        let best = select_highest_resolution_image(&node);
        assert_eq!(best, Some("https://cdn.instagram.com/1440.jpg".to_string()));
    }

    #[test]
    fn test_carousel_unrolling_graphql_payload() {
        let media_node = json!({
            "id": "123456789",
            "shortcode": "C_car123",
            "owner": {
                "username": "photographer",
                "full_name": "Photo Pro",
                "profile_pic_url": "https://cdn.instagram.com/avatar.jpg",
                "is_verified": true
            },
            "edge_media_to_caption": {
                "edges": [{ "node": { "text": "Carousel trip #travel #photography @friend" } }]
            },
            "taken_at_timestamp": 1726338600,
            "edge_media_preview_like": { "count": 2540 },
            "edge_media_to_parent_comment": { "count": 88 },
            "edge_sidecar_to_children": {
                "edges": [
                    {
                        "node": {
                            "id": "slide1",
                            "shortcode": "slide1",
                            "is_video": false,
                            "display_resources": [
                                { "src": "https://cdn.instagram.com/s1_640.jpg", "config_width": 640, "config_height": 640 },
                                { "src": "https://cdn.instagram.com/s1_1080.jpg", "config_width": 1080, "config_height": 1080 }
                            ]
                        }
                    },
                    {
                        "node": {
                            "id": "slide2",
                            "shortcode": "slide2",
                            "is_video": true,
                            "video_url": "https://cdn.instagram.com/video_slide2.mp4",
                            "display_resources": [
                                { "src": "https://cdn.instagram.com/poster2.jpg", "config_width": 1080, "config_height": 1920 }
                            ]
                        }
                    }
                ]
            }
        });

        let post = parse_gql_media_node(&media_node, "C_car123").unwrap();
        assert_eq!(post.media_type, "carousel");
        assert_eq!(post.media_items.len(), 2);
        assert_eq!(post.images.len(), 2);
        assert_eq!(post.videos.len(), 1);
        assert_eq!(post.images[0], "https://cdn.instagram.com/s1_1080.jpg");
        assert_eq!(post.videos[0], "https://cdn.instagram.com/video_slide2.mp4");
        assert_eq!(post.hashtags, vec!["travel", "photography"]);
        assert_eq!(post.mentions, vec!["friend"]);
        assert_eq!(post.author.username, "photographer");
        assert_eq!(post.author.is_verified, Some(true));
        assert_eq!(post.like_count, Some(2540));
        assert_eq!(post.comment_count, Some(88));
    }

    #[test]
    fn test_single_video_graphql_payload() {
        let media_node = json!({
            "id": "987654321",
            "shortcode": "D_reel123",
            "owner": { "username": "reeler" },
            "is_video": true,
            "video_url": "https://cdn.instagram.com/reel.mp4",
            "display_url": "https://cdn.instagram.com/reel_thumb.jpg",
            "edge_media_to_caption": { "edges": [] },
            "taken_at_timestamp": 1726338600
        });

        let post = parse_gql_media_node(&media_node, "D_reel123").unwrap();
        assert_eq!(post.media_type, "video");
        assert_eq!(post.videos, vec!["https://cdn.instagram.com/reel.mp4"]);
        assert_eq!(post.images, vec!["https://cdn.instagram.com/reel_thumb.jpg"]);
        assert_eq!(post.media_items.len(), 1);
        assert!(post.media_items[0].is_video);
    }

    #[test]
    fn test_select_highest_resolution_image_display_resources_overflow() {
        let node = json!({
            "display_resources": [
                { "src": "https://cdn.instagram.com/normal.jpg", "config_width": 1080u64, "config_height": 1080u64 },
                { "src": "https://cdn.instagram.com/extreme_5b.jpg", "config_width": 5_000_000_000u64, "config_height": 5_000_000_000u64 }
            ]
        });
        // 5B x 5B = 2.5e19 > u64::MAX (~1.84e19). Must not panic with overflow, and must pick largest resolution.
        let best = select_highest_resolution_image(&node);
        assert_eq!(best, Some("https://cdn.instagram.com/extreme_5b.jpg".to_string()));
    }

    #[test]
    fn test_select_highest_resolution_image_candidates_overflow() {
        let node = json!({
            "image_versions2": {
                "candidates": [
                    { "url": "https://cdn.instagram.com/normal.jpg", "width": 1080u64, "height": 1080u64 },
                    { "url": "https://cdn.instagram.com/extreme_candidates.jpg", "width": 5_000_000_000u64, "height": 5_000_000_000u64 }
                ]
            }
        });
        let best = select_highest_resolution_image(&node);
        assert_eq!(best, Some("https://cdn.instagram.com/extreme_candidates.jpg".to_string()));
    }

    #[test]
    fn test_select_highest_resolution_image_u64_max_boundary() {
        let node = json!({
            "display_resources": [
                { "src": "https://cdn.instagram.com/u64_max.jpg", "config_width": u64::MAX, "config_height": u64::MAX }
            ]
        });
        let best = select_highest_resolution_image(&node);
        assert_eq!(best, Some("https://cdn.instagram.com/u64_max.jpg".to_string()));
    }

    #[test]
    fn test_unroll_media_carousel_video_versions_overflow() {
        let node = json!({
            "carousel_media": [
                {
                    "id": "carousel_vid_1",
                    "is_video": true,
                    "video_versions": [
                        { "url": "https://cdn.instagram.com/vid_low.mp4", "width": 720u64, "height": 1280u64 },
                        { "url": "https://cdn.instagram.com/vid_overflow.mp4", "width": 5_000_000_000u64, "height": 5_000_000_000u64 }
                    ]
                }
            ]
        });
        let (_, videos, media_items, media_type, _) = unroll_media_from_node(&node);
        assert_eq!(media_type, "carousel");
        assert_eq!(videos, vec!["https://cdn.instagram.com/vid_overflow.mp4"]);
        assert_eq!(media_items.len(), 1);
        assert_eq!(media_items[0].url, "https://cdn.instagram.com/vid_overflow.mp4");
    }

    #[test]
    fn test_unroll_media_single_video_versions_overflow() {
        let node = json!({
            "id": "single_vid_1",
            "is_video": true,
            "video_versions": [
                { "url": "https://cdn.instagram.com/vid_standard.mp4", "width": 1080u64, "height": 1920u64 },
                { "url": "https://cdn.instagram.com/vid_extreme.mp4", "width": 5_000_000_000u64, "height": 5_000_000_000u64 }
            ]
        });
        let (_, videos, media_items, media_type, _) = unroll_media_from_node(&node);
        assert_eq!(media_type, "video");
        assert_eq!(videos, vec!["https://cdn.instagram.com/vid_extreme.mp4"]);
        assert_eq!(media_items.len(), 1);
    }

    // ── Caption & Hashtag Normalization Tests ────────────────────────────────

    #[test]
    fn test_extract_hashtags_multilingual_and_dedup() {
        let caption = "Exploring #RustLang with #async and #RUSTLANG! Also #東京 and #مرحبا #café, plus #한국 and #москва.";
        let tags = extract_hashtags(caption);
        assert_eq!(tags, vec!["rustlang", "async", "東京", "مرحبا", "café", "한국", "москва"]);
    }

    #[test]
    fn test_caption_normalization_entities_and_spacing() {
        let raw = "\u{feff}Tom &amp; Jerry &quot;Classic&quot;&#39;s &lt;show&gt;&#x21;\r\n\r\n\r\n\r\nLine 2   \r\nLine 3";
        let normalized = normalize_caption(raw);
        assert_eq!(normalized, "Tom & Jerry \"Classic\"'s <show>!\n\nLine 2\nLine 3");
    }

    #[test]
    fn test_caption_unicode_emojis_and_rtl_preservation() {
        let raw = "Amazing launch 🚀 with 🧑‍💻 team! 🇺🇸 مرحبا بالعالم";
        let normalized = normalize_caption(raw);
        assert_eq!(normalized, raw);
    }

    // ── Embed Parsing Tests ──────────────────────────────────────────────────

    #[test]
    fn test_parse_embed_context_json() {
        let inner_context = json!({
            "shortcode_media": {
                "id": "11223344",
                "shortcode": "E_embed1",
                "owner": { "username": "embed_user", "full_name": "Embed User" },
                "display_url": "https://cdn.instagram.com/embed_img.jpg",
                "edge_media_to_caption": {
                    "edges": [{ "node": { "text": "Embed caption #awesome" } }]
                }
            }
        }).to_string();

        let json_slice = json!({ "contextJSON": inner_context }).to_string();
        let html = format!(r#"<html><body><script>"init",[],[{}],</script></body></html>"#, json_slice);

        let post = parse_embed_html(&html, "E_embed1").unwrap();
        assert_eq!(post.shortcode, "E_embed1");
        assert_eq!(post.author.username, "embed_user");
        assert_eq!(post.images, vec!["https://cdn.instagram.com/embed_img.jpg"]);
        assert_eq!(post.hashtags, vec!["awesome"]);
    }

    #[test]
    fn test_parse_embed_dom_fallback() {
        let html = r#"
        <div class="Caption">
            <a class="CaptionUsername">nature_hub</a>
            <div class="CaptionText">Majestic mountain sunrise &amp; clear air #nature #mountains</div>
        </div>
        <img class="EmbeddedMediaImage" src="https://cdn.instagram.com/mountain.jpg" />
        "#;

        let post = parse_embed_html(html, "E_dom1").unwrap();
        assert_eq!(post.author.username, "nature_hub");
        assert_eq!(post.caption, "Majestic mountain sunrise & clear air #nature #mountains");
        assert_eq!(post.images, vec!["https://cdn.instagram.com/mountain.jpg"]);
        assert_eq!(post.hashtags, vec!["nature", "mountains"]);
    }

    #[test]
    fn test_parse_embed_private_detection() {
        let html = "<html><body><div>This post is private</div></body></html>";
        let err = parse_embed_html(html, "private1").unwrap_err();
        assert!(matches!(err, InstagramExtractError::PrivateOrLoginRequired(_)));
    }

    #[test]
    fn test_parse_embed_dom_caption_mentions_private_post() {
        let html = r#"
        <div class="Caption">
            <a class="CaptionUsername">nature_fan</a>
            <div class="CaptionText">This post is private joke between friends #nature</div>
        </div>
        <img class="EmbeddedMediaImage" src="https://cdn.instagram.com/nature.jpg" />
        "#;
        let post = parse_embed_html(html, "E_dom_priv").unwrap();
        assert_eq!(post.author.username, "nature_fan");
        assert_eq!(post.caption, "This post is private joke between friends #nature");
        assert_eq!(post.images, vec!["https://cdn.instagram.com/nature.jpg"]);
        assert_eq!(post.hashtags, vec!["nature"]);
    }

    #[test]
    fn test_parse_embed_context_json_caption_mentions_private_post() {
        let inner_context = serde_json::json!({
            "shortcode_media": {
                "shortcode": "E_embed_priv",
                "owner": { "username": "embed_user", "full_name": "Embed User" },
                "display_url": "https://cdn.instagram.com/embed_priv.jpg",
                "edge_media_to_caption": {
                    "edges": [{ "node": { "text": "This post is private thoughts #life" } }]
                }
            }
        }).to_string();

        let json_slice = serde_json::json!({ "contextJSON": inner_context }).to_string();
        let html = format!(r#"<html><body><script>"init",[],[{}],</script></body></html>"#, json_slice);

        let post = parse_embed_html(&html, "E_embed_priv").unwrap();
        assert_eq!(post.shortcode, "E_embed_priv");
        assert_eq!(post.author.username, "embed_user");
        assert_eq!(post.images, vec!["https://cdn.instagram.com/embed_priv.jpg"]);
        assert_eq!(post.hashtags, vec!["life"]);
    }

    #[test]
    fn test_parse_embed_dom_caption_mentions_login_path() {
        let html = r#"
        <div class="Caption">
            <a class="CaptionUsername">support</a>
            <div class="CaptionText">Recover your account at /accounts/login/ today</div>
        </div>
        <img class="EmbeddedMediaImage" src="https://cdn.instagram.com/support.jpg" />
        "#;
        let post = parse_embed_html(html, "E_dom_login").unwrap();
        assert_eq!(post.author.username, "support");
        assert_eq!(post.caption, "Recover your account at /accounts/login/ today");
        assert_eq!(post.images, vec!["https://cdn.instagram.com/support.jpg"]);
    }

    #[test]
    fn test_parse_embed_explicit_embed_empty_container() {
        let html = r#"<div class="EmbedEmpty"><div class="EmbedEmptyMessage">This post is private</div></div>"#;
        let err = parse_embed_html(html, "empty_priv").unwrap_err();
        assert!(matches!(err, InstagramExtractError::PrivateOrLoginRequired(_)));
    }

    // ── Markdown Summary Formatting Test ─────────────────────────────────────

    #[test]
    fn test_markdown_summary_formatting() {
        let post = InstagramPost {
            id: "post123".into(),
            shortcode: "C_post123".into(),
            url: "https://www.instagram.com/p/C_post123/".into(),
            author: InstagramAuthor {
                username: "techie".into(),
                full_name: Some("Techie Creator".into()),
                profile_url: "https://www.instagram.com/techie/".into(),
                avatar_url: None,
                is_verified: Some(true),
            },
            caption: "Hello AI agents from Instagram!".into(),
            hashtags: vec!["tech".into(), "ai".into()],
            mentions: vec!["agent".into()],
            images: vec!["https://cdn.instagram.com/photo.jpg".into()],
            videos: vec![],
            audio_url: None,
            has_audio: false,
            audio_title: None,
            audio_artist: None,
            media_type: "photo".into(),
            media_items: vec![InstagramMediaItem {
                id: Some("post123".into()),
                media_type: "photo".into(),
                url: "https://cdn.instagram.com/photo.jpg".into(),
                width: Some(1080),
                height: Some(1080),
                thumbnail_url: None,
                is_video: false,
                duration_secs: None,
                audio_url: None,
                has_audio: false,
            }],
            thumbnail_url: Some("https://cdn.instagram.com/photo.jpg".into()),
            taken_at: Some(1726338600),
            like_count: Some(12450),
            comment_count: Some(342),
            markdown: String::new(),
        };

        let md = generate_markdown_summary(&post);
        assert!(md.contains("# Instagram Post by @techie"));
        assert!(md.contains("Techie Creator ([@techie](https://www.instagram.com/techie/))"));
        assert!(md.contains("**Type**: Photo"));
        assert!(md.contains("12,450 likes, 342 comments"));
        assert!(md.contains("Hello AI agents from Instagram!"));
        assert!(md.contains("`#tech` `#ai`"));
        assert!(md.contains("- **Item 1 (Photo)**: [High-Resolution Image](https://cdn.instagram.com/photo.jpg)"));
    }

    #[test]
    fn test_markdown_summary_with_audio_track() {
        let post = InstagramPost {
            id: "reel123".into(),
            shortcode: "C_reel123".into(),
            url: "https://www.instagram.com/reel/C_reel123/".into(),
            author: InstagramAuthor {
                username: "creator".into(),
                full_name: Some("Creator Name".into()),
                profile_url: "https://www.instagram.com/creator/".into(),
                avatar_url: None,
                is_verified: Some(false),
            },
            caption: "Amazing reel with voice".into(),
            hashtags: vec!["reel".into()],
            mentions: vec![],
            images: vec!["https://cdn.instagram.com/poster.jpg".into()],
            videos: vec!["https://cdn.instagram.com/video.mp4".into()],
            audio_url: Some("https://cdn.instagram.com/audio.m4a".into()),
            has_audio: true,
            audio_title: Some("Viral Track".into()),
            audio_artist: Some("Top Artist".into()),
            media_type: "video".into(),
            media_items: vec![InstagramMediaItem {
                id: Some("reel123".into()),
                media_type: "video".into(),
                url: "https://cdn.instagram.com/video.mp4".into(),
                width: Some(720),
                height: Some(1280),
                thumbnail_url: Some("https://cdn.instagram.com/poster.jpg".into()),
                is_video: true,
                duration_secs: Some(30.0),
                audio_url: Some("https://cdn.instagram.com/audio.m4a".into()),
                has_audio: true,
            }],
            thumbnail_url: Some("https://cdn.instagram.com/poster.jpg".into()),
            taken_at: Some(1726338600),
            like_count: Some(500),
            comment_count: Some(25),
            markdown: String::new(),
        };

        let md = generate_markdown_summary(&post);
        assert!(md.contains("- **Audio Track**: [Direct Audio Stream (.m4a/.mp3)](https://cdn.instagram.com/audio.m4a) 🎵 *(Optimized for AI speech transcription & translation (Track: \"Viral Track\" by Top Artist))*"));
        assert!(md.contains("- **Item 1 (Video)**: [Direct Video Stream (.mp4)](https://cdn.instagram.com/video.mp4) 🎬 *(Includes audio track)*"));
        assert!(md.contains("Poster Image: [Thumbnail](https://cdn.instagram.com/poster.jpg)"));
    }

    #[test]
    fn test_extract_audio_from_node_music_info() {
        let node = json!({
            "is_video": true,
            "clips_metadata": {
                "music_info": {
                    "music_asset_info": {
                        "progressive_download_url": "https://cdn.instagram.com/audio/music.mp3",
                        "title": "Summer Song",
                        "display_artist": "Cool Singer"
                    }
                }
            }
        });

        let (url, has_audio, title, artist) = extract_audio_from_node(&node);
        assert_eq!(url.as_deref(), Some("https://cdn.instagram.com/audio/music.mp3"));
        assert_eq!(has_audio, true);
        assert_eq!(title.as_deref(), Some("Summer Song"));
        assert_eq!(artist.as_deref(), Some("Cool Singer"));
    }

    #[test]
    fn test_extract_audio_from_node_original_sound() {
        let node = json!({
            "is_video": true,
            "clips_metadata": {
                "original_sound_info": {
                    "progressive_download_url": "https://cdn.instagram.com/audio/orig.m4a",
                    "original_audio_title": "Original voice",
                    "ig_artist": {
                        "username": "voice_artist"
                    }
                }
            }
        });

        let (url, has_audio, title, artist) = extract_audio_from_node(&node);
        assert_eq!(url.as_deref(), Some("https://cdn.instagram.com/audio/orig.m4a"));
        assert_eq!(has_audio, true);
        assert_eq!(title.as_deref(), Some("Original voice"));
        assert_eq!(artist.as_deref(), Some("voice_artist"));
    }

    #[test]
    fn test_extract_audio_from_dash_manifest() {
        let manifest = r#"
            <MPD xmlns="urn:mpeg:dash:schema:mpd:2011">
                <Period>
                    <AdaptationSet contentType="video" mimeType="video/mp4">
                        <Representation id="1" bandwidth="1000000">
                            <BaseURL>https://cdn.instagram.com/video_stream.mp4</BaseURL>
                        </Representation>
                    </AdaptationSet>
                    <AdaptationSet contentType="audio" mimeType="audio/mp4">
                        <Representation id="2" bandwidth="128000">
                            <BaseURL>https://cdn.instagram.com/audio_stream.m4a</BaseURL>
                        </Representation>
                    </AdaptationSet>
                </Period>
            </MPD>
        "#;

        let audio = extract_audio_from_dash_manifest(manifest);
        assert_eq!(audio.as_deref(), Some("https://cdn.instagram.com/audio_stream.m4a"));
    }

    #[test]
    fn test_extract_audio_from_dash_manifest_representation_level() {
        let manifest = r#"
            <MPD xmlns="urn:mpeg:dash:schema:mpd:2011">
                <Period>
                    <AdaptationSet>
                        <Representation id="audio_1" mimeType="audio/mp4" codecs="mp4a.40.2">
                            <BaseURL>https://instagram.fsin9-1.fna.fbcdn.net/v/t50.2886-16/audio.mp4?_nc_cat=100&amp;oh=123</BaseURL>
                        </Representation>
                    </AdaptationSet>
                </Period>
            </MPD>
        "#;
        let audio = extract_audio_from_dash_manifest(manifest);
        assert_eq!(audio.as_deref(), Some("https://instagram.fsin9-1.fna.fbcdn.net/v/t50.2886-16/audio.mp4?_nc_cat=100&oh=123"));
    }

    #[test]
    fn test_extract_audio_from_dash_manifest_escaped_json_and_html() {
        let manifest = r#"&lt;MPD&gt;&lt;AdaptationSet contentType=&quot;audio&quot;&gt;&lt;BaseURL&gt;https:\/\/cdn.instagram.com\/v\/audio.m4a\u0026tag=test&lt;/BaseURL&gt;&lt;/AdaptationSet&gt;&lt;/MPD&gt;"#;
        let audio = extract_audio_from_dash_manifest(manifest);
        assert_eq!(audio.as_deref(), Some("https://cdn.instagram.com/v/audio.m4a&tag=test"));
    }

    #[test]
    fn test_extract_audio_from_node_root_video_dash_manifest() {
        let node = json!({
            "is_video": true,
            "video_dash_manifest": "<MPD><Period><AdaptationSet contentType=\"audio\"><BaseURL>https://cdn.instagram.com/dash_audio.m4a</BaseURL></AdaptationSet></Period></MPD>"
        });
        let (url, has_audio, _, _) = extract_audio_from_node(&node);
        assert_eq!(url.as_deref(), Some("https://cdn.instagram.com/dash_audio.m4a"));
        assert!(has_audio);
    }

    // ── MCP JSON-RPC Adapter Schema Test ─────────────────────────────────────

    #[tokio::test]
    async fn test_mcp_adapter_empty_args() {
        let res = call_instagram_post(json!({})).await.unwrap();
        assert_eq!(res["isError"], true);
        let text = res["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("URL or shortcode cannot be empty"));
    }

    #[tokio::test]
    async fn test_mcp_adapter_invalid_domain() {
        let res = call_instagram_post(json!({ "url": "https://notinstagram.com/p/123" })).await.unwrap();
        assert_eq!(res["isError"], true);
        let text = res["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Invalid domain"));
    }
}
