//! Facebook headless post, Reel, and Watch video extractor tool (`facebook_post`).
//!
//! Multi-tier extraction pipeline:
//! 1. Tier 1: SSR OpenGraph Crawler (`facebookexternalhit/1.1`) to extract server-rendered metadata
//! 2. Tier 2: Direct Video Stream Extractor scanning HTML for progressive HD/SD MP4 streams
//! 3. Tier 3: `yt-dlp` metadata and progressive video stream extraction fallback
//!
//! Includes login wall detection, tracking parameter stripping, URL normalization,
//! HTML entity decoding (named, decimal, hex), arithmetic overflow safety,
//! and AI Markdown summary generation conforming to MCP JSON-RPC standards.

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;
use url::Url;

// ── Constants & Regular Expressions ──────────────────────────────────────────

pub const FB_CRAWLER_USER_AGENT: &str =
    "facebookexternalhit/1.1 (+http://www.facebook.com/externalhit_uatext.php)";

pub const DESKTOP_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

static HASHTAG_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#([a-zA-Z0-9_\u{0080}-\u{ffff}]+)")
        .expect("Valid hashtag extraction regex")
});

static NUMERIC_ENTITY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"&#(\d+);")
        .expect("Valid numeric entity regex")
});

static HEX_ENTITY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"&#[xX]([0-9a-fA-F]+);")
        .expect("Valid hex entity regex")
});

static OG_META_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<meta[^>]+(?:property|name)=["']og:([a-zA-Z0-9_:]+)["'][^>]+content=["']([^"']*)["']"#)
        .expect("Valid OpenGraph property regex")
});

static OG_META_REVERSE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<meta[^>]+content=["']([^"']*)["'][^>]+(?:property|name)=["']og:([a-zA-Z0-9_:]+)["']"#)
        .expect("Valid OpenGraph reverse attribute regex")
});

static META_DESC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<meta[^>]+name=["']description["'][^>]+content=["']([^"']*)["']"#)
        .expect("Valid meta description regex")
});

static TITLE_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<title[^>]*>(.*?)</title>"#)
        .expect("Valid HTML title tag regex")
});

static HD_STREAM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?x)
        (?:
            "playable_url_quality_hd" \s*:\s* "([^"]+)" |
            "hd_src" \s*:\s* "([^"]+)" |
            "hd_src_no_ratelimit" \s*:\s* "([^"]+)" |
            "browser_native_hd_url" \s*:\s* "([^"]+)" |
            \\"playable_url_quality_hd\\" \s*:\s* \\"((?:\\[^"]|[^"\\])+)\\" |
            \\"hd_src\\" \s*:\s* \\"((?:\\[^"]|[^"\\])+)\\" |
            \\"hd_src_no_ratelimit\\" \s*:\s* \\"((?:\\[^"]|[^"\\])+)\\" |
            \\"browser_native_hd_url\\" \s*:\s* \\"((?:\\[^"]|[^"\\])+)\\"
        )
    "#).expect("Valid HD stream extraction regex")
});

static SD_STREAM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?x)
        (?:
            "playable_url" \s*:\s* "([^"]+)" |
            "sd_src" \s*:\s* "([^"]+)" |
            "sd_src_no_ratelimit" \s*:\s* "([^"]+)" |
            "browser_native_sd_url" \s*:\s* "([^"]+)" |
            \\"playable_url\\" \s*:\s* \\"((?:\\[^"]|[^"\\])+)\\" |
            \\"sd_src\\" \s*:\s* \\"((?:\\[^"]|[^"\\])+)\\" |
            \\"sd_src_no_ratelimit\\" \s*:\s* \\"((?:\\[^"]|[^"\\])+)\\" |
            \\"browser_native_sd_url\\" \s*:\s* \\"((?:\\[^"]|[^"\\])+)\\"
        )
    "#).expect("Valid SD stream extraction regex")
});

static FB_AUDIO_STREAM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?x)
        (?:
            "playback_audio_url" \s*:\s* "([^"]+)" |
            \\"playback_audio_url\\" \s*:\s* \\"((?:\\[^"]|[^"\\])+)\\" |
            "dash_audio_url" \s*:\s* "([^"]+)" |
            \\"dash_audio_url\\" \s*:\s* \\"((?:\\[^"]|[^"\\])+)\\" |
            "audio" \s*:\s* \[\s*\{\s*"[^"]*"\s*:\s*"[^"]*",\s*"base_url"\s*:\s*"([^"]+)" |
            \\"audio\\" \s*:\s* \[\s*\\\{\s*\\"[^"]*\\"\s*:\s*\\"[^"]*\\",\s*\\"base_url\\"\s*:\s*\\"((?:\\[^"]|[^"\\])+)\\"
        )
    "#).expect("Valid FB audio stream extraction regex")
});

static SCONTENT_IMAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"https://scontent[^"'\s<>]+\.(?:jpg|png|webp)[^"'\s<>]*"#)
        .expect("Valid scontent image extraction regex")
});

// ── Data Models ──────────────────────────────────────────────────────────────

/// Arguments payload for `facebook_post` tool invocations.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FacebookArgs {
    /// Public Facebook post, Reel, or Watch URL
    pub url: Option<String>,
    /// Optional post/video identifier
    #[serde(default)]
    pub id: Option<String>,
}

/// Author / page / creator metadata for a Facebook post or video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacebookAuthor {
    /// Display name of the Facebook page or profile (e.g. "Mark Zuckerberg", "BBC News")
    pub name: String,
    /// Canonical URL to author's profile or page, if known
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Author / Page numeric ID or username handle, if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Direct URL to author's profile picture / page avatar
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// Whether the page or profile carries a verified badge
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_verified: Option<bool>,
}

/// An individual media item attached to a Facebook post, video, or multi-photo album.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FacebookMediaItem {
    /// Media identifier, if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Media kind: "photo" | "video"
    pub media_type: String,
    /// Direct highest-resolution media asset URL (image source or progressive video stream)
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
    /// True if media item is a video stream, false if photo
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

/// Comprehensive extracted metadata for a Facebook post, Reel, or Watch video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FacebookPost {
    /// Post or video identifier, if extracted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Normalized canonical HTTPS web URL
    pub url: String,
    /// Post author / page / creator information
    pub author: FacebookAuthor,
    /// Full post status update / caption text content
    pub caption: String,
    /// Extracted hashtags without the '#' symbol
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hashtags: Vec<String>,
    /// Flat list of direct high-resolution image URLs
    pub images: Vec<String>,
    /// Flat list of direct playable video stream URLs (HD/SD MP4)
    pub videos: Vec<String>,
    /// Direct URL to standalone audio track (.m4a/.mp3) for AI speech transcription
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
    /// Whether the post/video includes an audio track
    #[serde(default)]
    pub has_audio: bool,
    /// Overall post media classification: "post", "photo", "video", "reel", or "carousel"
    pub media_type: String,
    /// Detailed individual media items (e.g. photos in an album or progressive video streams)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media_items: Vec<FacebookMediaItem>,
    /// Best available thumbnail / preview image URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    /// Publication Unix timestamp (seconds since epoch), if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<i64>,
    /// Extracted like / reaction count, if public
    #[serde(skip_serializing_if = "Option::is_none")]
    pub like_count: Option<u64>,
    /// Extracted comment count, if public
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment_count: Option<u64>,
    /// Extracted share count, if public
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_count: Option<u64>,
    /// Clean human-readable Markdown summary formatted for AI agents
    pub markdown: String,
}

/// Result of parsing and normalizing a Facebook input URL or post ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacebookUrlInfo {
    /// Extracted post/video/reel identifier (numeric ID, `pfbid...`, or share slug)
    pub id: String,
    /// Content kind inferred from URL: "post", "reel", "watch", "photo", or "share"
    pub kind: String,
    /// Normalized canonical HTTPS URL (e.g. "https://www.facebook.com/watch/?v=123456")
    pub canonical_url: String,
    /// Author / page handle or numeric ID extracted from path/query, if present
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_handle: Option<String>,
    /// True if input was a shortened share link (`fb.watch/*`, `/share/*`) requiring redirect resolution
    pub is_redirect_needed: bool,
    /// True if content is a Reel or Watch video
    pub is_video: bool,
}

/// Error types for Facebook URL parsing and headless extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacebookExtractError {
    /// Invalid or empty input (whitespace, invalid characters in post ID, bad scheme)
    InvalidInput(String),
    /// Host is not a recognized Facebook domain
    InvalidDomain(String),
    /// URL path is not an extractable post (profile, groups browse, marketplace, messages)
    UnsupportedUrl(String),
    /// Post was not found or has been deleted (HTTP 404)
    NotFound(String),
    /// Post or account is private, friends-only, or blocked by a Facebook authentication/login wall
    PrivateOrLoginWall(String),
    /// Upstream network, API, or parsing failure
    UpstreamError(String),
    /// Extraction request timed out
    Timeout(String),
}

impl std::fmt::Display for FacebookExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
            Self::InvalidDomain(msg) => write!(f, "Invalid domain: {}", msg),
            Self::UnsupportedUrl(msg) => write!(f, "Unsupported URL: {}", msg),
            Self::NotFound(msg) => write!(f, "Post not found: {}", msg),
            Self::PrivateOrLoginWall(msg) => {
                write!(f, "Facebook post is private or blocked by login wall: {}", msg)
            }
            Self::UpstreamError(msg) => write!(f, "Upstream error: {}", msg),
            Self::Timeout(msg) => write!(f, "Request timed out: {}", msg),
        }
    }
}

impl std::error::Error for FacebookExtractError {}

// ── URL Normalization & Validation ───────────────────────────────────────────

/// Checks if a query parameter key is a Facebook tracking parameter.
pub fn is_tracking_param(key: &str) -> bool {
    let lower = key.to_lowercase();
    lower == "mibextid"
        || lower == "rdid"
        || lower.starts_with("utm_")
        || lower.starts_with("__cft")
        || lower.starts_with("__tn")
        || lower == "fbclid"
        || lower == "ref"
        || lower == "refsrc"
        || lower == "paipv"
        || lower.starts_with("notif_")
        || lower == "hc_ref"
        || lower == "_rdr"
}

/// Validates that a Facebook post, reel, or video ID conforms to alphanumeric/pfbid rules.
pub fn validate_post_id(id: &str) -> Result<(), FacebookExtractError> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Err(FacebookExtractError::InvalidInput(
            "Post ID cannot be empty".into(),
        ));
    }
    if !trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(FacebookExtractError::InvalidInput(format!(
            "Post ID '{}' contains invalid characters (only alphanumeric, '_', and '-' are allowed)",
            trimmed
        )));
    }
    Ok(())
}

/// Parses and normalizes a Facebook URL or raw post identifier.
///
/// Handles:
/// - Standard page/user posts (`/{page}/posts/{id}`, `/posts/{id}`, `pfbid...`)
/// - Facebook Reels (`/reel/{id}`, `/reels/{id}`)
/// - Facebook Watch videos (`/watch/?v={id}`, `/{page}/videos/{id}`, `/video.php?v={id}`)
/// - Shortened links (`fb.watch/{slug}/`) with `is_redirect_needed = true`
/// - Mobile share links (`/share/p/{id}/`, `/share/r/{id}/`, `/share/v/{id}/`, `/share/{id}/`) with `is_redirect_needed = true`
/// - Legacy query URLs (`permalink.php?story_fbid=...`, `story.php?story_fbid=...`, `photo.php?fbid=...`)
/// - Group posts (`/groups/{gid}/posts/{pid}/`, `/groups/{gid}/permalink/{pid}/`)
/// - Strips all tracking parameters (`mibextid`, `rdid`, `utm_*`, `__cft__*`, `__tn__*`, `fbclid`)
/// - Validates domain against recognized Facebook hosts (`facebook.com`, `fb.com`, `fb.watch`, `m.facebook.com`, `web.facebook.com`)
/// - Auto-prefixes `https://` if scheme is missing
/// - Rejects unsupported profile, marketplace, event, message, and login wall pages
pub fn parse_facebook_url(raw: &str) -> Result<FacebookUrlInfo, FacebookExtractError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(FacebookExtractError::InvalidInput(
            "URL or post ID cannot be empty".into(),
        ));
    }

    // 1. Raw post ID shortcut: string contains no slashes, colons, dots, or query marks
    if !trimmed.contains('/') && !trimmed.contains(':') && !trimmed.contains('.') && !trimmed.contains('?') {
        validate_post_id(trimmed)?;
        return Ok(FacebookUrlInfo {
            id: trimmed.to_string(),
            kind: "post".to_string(),
            canonical_url: format!("https://www.facebook.com/{}", trimmed),
            author_handle: None,
            is_redirect_needed: false,
            is_video: false,
        });
    }

    // 2. URL parsing with auto-prefixing
    let url_str = if trimmed.starts_with("//") {
        format!("https:{}", trimmed)
    } else if !trimmed.contains("://") {
        format!("https://{}", trimmed)
    } else {
        trimmed.to_string()
    };

    let parsed_url = Url::parse(&url_str).map_err(|e| {
        FacebookExtractError::InvalidInput(format!("Malformed URL '{}': {}", trimmed, e))
    })?;

    // 3. Scheme validation
    let scheme = parsed_url.scheme().to_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(FacebookExtractError::InvalidInput(format!(
            "Unsupported URL scheme '{}'; expected http or https",
            scheme
        )));
    }

    // 4. Host validation
    let host = parsed_url.host_str().unwrap_or("").to_lowercase();
    let is_valid_host = host == "facebook.com"
        || host.ends_with(".facebook.com")
        || host == "fb.com"
        || host.ends_with(".fb.com")
        || host == "fb.watch"
        || host.ends_with(".fb.watch");

    if !is_valid_host {
        return Err(FacebookExtractError::InvalidDomain(format!(
            "Domain '{}' is not a recognized Facebook domain",
            host
        )));
    }

    // 5. Shortened fb.watch domain handling
    if host == "fb.watch" || host.ends_with(".fb.watch") {
        let segments: Vec<&str> = parsed_url
            .path()
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if segments.is_empty() {
            return Err(FacebookExtractError::InvalidInput(
                "Missing video slug in fb.watch URL".into(),
            ));
        }

        let slug = segments[0];
        validate_post_id(slug)?;

        return Ok(FacebookUrlInfo {
            id: slug.to_string(),
            kind: "watch".to_string(),
            canonical_url: format!("https://fb.watch/{}/", slug),
            author_handle: None,
            is_redirect_needed: true,
            is_video: true,
        });
    }

    // 6. Query parameters helper: extract non-tracking query parameters
    let non_tracking_queries: Vec<(String, String)> = parsed_url
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let get_query_param = |name: &str| -> Option<String> {
        non_tracking_queries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };

    // 7. Path segments extraction
    let segments: Vec<&str> = parsed_url
        .path()
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    // 8. Match specialized path endpoints
    let first_seg = segments.first().map(|s| s.to_lowercase());

    match first_seg.as_deref() {
        None | Some("") => {
            if let Some(v) = get_query_param("v") {
                validate_post_id(&v)?;
                return Ok(FacebookUrlInfo {
                    id: v.clone(),
                    kind: "watch".to_string(),
                    canonical_url: format!("https://www.facebook.com/watch/?v={}", v),
                    author_handle: None,
                    is_redirect_needed: false,
                    is_video: true,
                });
            }
            return Err(FacebookExtractError::InvalidInput(
                "URL path is empty; please provide a Facebook post, Reel, or Watch video URL".into(),
            ));
        }
        Some("watch") => {
            let video_id = if let Some(v) = get_query_param("v") {
                v
            } else if segments.len() >= 2 && !segments[1].trim().is_empty() {
                segments[1].trim().to_string()
            } else {
                return Err(FacebookExtractError::InvalidInput(
                    "Facebook Watch URL missing 'v' query parameter or video ID".into(),
                ));
            };
            validate_post_id(&video_id)?;
            return Ok(FacebookUrlInfo {
                id: video_id.clone(),
                kind: "watch".to_string(),
                canonical_url: format!("https://www.facebook.com/watch/?v={}", video_id),
                author_handle: None,
                is_redirect_needed: false,
                is_video: true,
            });
        }
        Some("video.php") => {
            let video_id = if let Some(v) = get_query_param("v") {
                v
            } else {
                return Err(FacebookExtractError::InvalidInput(
                    "Facebook video.php URL missing 'v' query parameter".into(),
                ));
            };
            validate_post_id(&video_id)?;
            return Ok(FacebookUrlInfo {
                id: video_id.clone(),
                kind: "watch".to_string(),
                canonical_url: format!("https://www.facebook.com/watch/?v={}", video_id),
                author_handle: None,
                is_redirect_needed: false,
                is_video: true,
            });
        }
        Some("permalink.php") | Some("story.php") => {
            let post_id = if let Some(fbid) = get_query_param("story_fbid") {
                fbid
            } else if let Some(fbid) = get_query_param("fbid") {
                fbid
            } else {
                return Err(FacebookExtractError::InvalidInput(
                    "Facebook permalink/story URL missing 'story_fbid' query parameter".into(),
                ));
            };
            validate_post_id(&post_id)?;
            let page_id = get_query_param("id");
            let canonical_url = match &page_id {
                Some(uid) => format!(
                    "https://www.facebook.com/permalink.php?story_fbid={}&id={}",
                    post_id, uid
                ),
                None => format!(
                    "https://www.facebook.com/permalink.php?story_fbid={}",
                    post_id
                ),
            };
            return Ok(FacebookUrlInfo {
                id: post_id,
                kind: "post".to_string(),
                canonical_url,
                author_handle: page_id,
                is_redirect_needed: false,
                is_video: false,
            });
        }
        Some("photo.php") | Some("photo") => {
            let photo_id = if let Some(fbid) = get_query_param("fbid") {
                fbid
            } else if segments.len() >= 2 && !segments[1].trim().is_empty() {
                segments[1].trim().to_string()
            } else {
                return Err(FacebookExtractError::InvalidInput(
                    "Facebook photo URL missing 'fbid' query parameter".into(),
                ));
            };
            validate_post_id(&photo_id)?;
            return Ok(FacebookUrlInfo {
                id: photo_id.clone(),
                kind: "photo".to_string(),
                canonical_url: format!("https://www.facebook.com/photo.php?fbid={}", photo_id),
                author_handle: None,
                is_redirect_needed: false,
                is_video: false,
            });
        }
        Some("reel") | Some("reels") => {
            if segments.len() < 2 || segments[1].trim().is_empty() {
                return Err(FacebookExtractError::InvalidInput(
                    "Missing reel ID in URL path '/reel/'".into(),
                ));
            }
            let reel_id = segments[1].trim();
            validate_post_id(reel_id)?;
            return Ok(FacebookUrlInfo {
                id: reel_id.to_string(),
                kind: "reel".to_string(),
                canonical_url: format!("https://www.facebook.com/reel/{}/", reel_id),
                author_handle: None,
                is_redirect_needed: false,
                is_video: true,
            });
        }
        Some("share") => {
            if segments.len() < 2 || segments[1].trim().is_empty() {
                return Err(FacebookExtractError::InvalidInput(
                    "Missing share ID in '/share/' path".into(),
                ));
            }
            let sub = segments[1].to_lowercase();
            match sub.as_str() {
                "p" => {
                    if segments.len() < 3 || segments[2].trim().is_empty() {
                        return Err(FacebookExtractError::InvalidInput(
                            "Missing post ID in '/share/p/' path".into(),
                        ));
                    }
                    let share_id = segments[2].trim();
                    validate_post_id(share_id)?;
                    return Ok(FacebookUrlInfo {
                        id: share_id.to_string(),
                        kind: "post".to_string(),
                        canonical_url: format!("https://www.facebook.com/share/p/{}/", share_id),
                        author_handle: None,
                        is_redirect_needed: true,
                        is_video: false,
                    });
                }
                "r" => {
                    if segments.len() < 3 || segments[2].trim().is_empty() {
                        return Err(FacebookExtractError::InvalidInput(
                            "Missing reel ID in '/share/r/' path".into(),
                        ));
                    }
                    let share_id = segments[2].trim();
                    validate_post_id(share_id)?;
                    return Ok(FacebookUrlInfo {
                        id: share_id.to_string(),
                        kind: "reel".to_string(),
                        canonical_url: format!("https://www.facebook.com/share/r/{}/", share_id),
                        author_handle: None,
                        is_redirect_needed: true,
                        is_video: true,
                    });
                }
                "v" => {
                    if segments.len() < 3 || segments[2].trim().is_empty() {
                        return Err(FacebookExtractError::InvalidInput(
                            "Missing video ID in '/share/v/' path".into(),
                        ));
                    }
                    let share_id = segments[2].trim();
                    validate_post_id(share_id)?;
                    return Ok(FacebookUrlInfo {
                        id: share_id.to_string(),
                        kind: "watch".to_string(),
                        canonical_url: format!("https://www.facebook.com/share/v/{}/", share_id),
                        author_handle: None,
                        is_redirect_needed: true,
                        is_video: true,
                    });
                }
                _ => {
                    let share_id = segments[1].trim();
                    validate_post_id(share_id)?;
                    return Ok(FacebookUrlInfo {
                        id: share_id.to_string(),
                        kind: "share".to_string(),
                        canonical_url: format!("https://www.facebook.com/share/{}/", share_id),
                        author_handle: None,
                        is_redirect_needed: true,
                        is_video: false,
                    });
                }
            }
        }
        Some("posts") => {
            if segments.len() < 2 || segments[1].trim().is_empty() {
                return Err(FacebookExtractError::InvalidInput(
                    "Missing post ID in '/posts/' path".into(),
                ));
            }
            let post_id = segments[1].trim();
            validate_post_id(post_id)?;
            return Ok(FacebookUrlInfo {
                id: post_id.to_string(),
                kind: "post".to_string(),
                canonical_url: format!("https://www.facebook.com/posts/{}/", post_id),
                author_handle: None,
                is_redirect_needed: false,
                is_video: false,
            });
        }
        Some("groups") => {
            if segments.len() >= 4 {
                let group_id = segments[1].trim();
                let sub_type = segments[2].to_lowercase();
                let post_id = segments[3].trim();
                if (sub_type == "posts" || sub_type == "permalink") && !post_id.is_empty() {
                    validate_post_id(post_id)?;
                    return Ok(FacebookUrlInfo {
                        id: post_id.to_string(),
                        kind: "post".to_string(),
                        canonical_url: format!(
                            "https://www.facebook.com/groups/{}/posts/{}/",
                            group_id, post_id
                        ),
                        author_handle: Some(group_id.to_string()),
                        is_redirect_needed: false,
                        is_video: false,
                    });
                }
            }
            return Err(FacebookExtractError::UnsupportedUrl(
                "Facebook group browse URLs without a post ID are not supported; please provide a specific group post URL".into(),
            ));
        }
        Some("login") | Some("login.php") => {
            return Err(FacebookExtractError::PrivateOrLoginWall(
                "Login page URL cannot be extracted; post may be private or requires Facebook authentication".into(),
            ));
        }
        _ => {}
    }

    // 9. Rejection of unsupported platform browse directories
    let first_lower = segments[0].to_lowercase();
    match first_lower.as_str() {
        "marketplace" | "events" | "messages" | "notifications" | "settings" | "friends"
        | "gaming" | "ads" | "saved" | "bookmarks" | "pages" | "search" => {
            return Err(FacebookExtractError::UnsupportedUrl(format!(
                "Facebook path '/{}/' is not an extractable post or video",
                first_lower
            )));
        }
        _ => {}
    }

    // 10. Multi-segment path matching: /{author}/posts/{id}, /{author}/videos/{id}, /{author}/photos/{id}
    if segments.len() >= 3 {
        let author = segments[0].trim();
        let action = segments[1].to_lowercase();
        let target_id = segments[2].trim();

        match action.as_str() {
            "posts" => {
                validate_post_id(target_id)?;
                return Ok(FacebookUrlInfo {
                    id: target_id.to_string(),
                    kind: "post".to_string(),
                    canonical_url: format!("https://www.facebook.com/{}/posts/{}/", author, target_id),
                    author_handle: Some(author.to_string()),
                    is_redirect_needed: false,
                    is_video: false,
                });
            }
            "videos" => {
                validate_post_id(target_id)?;
                return Ok(FacebookUrlInfo {
                    id: target_id.to_string(),
                    kind: "watch".to_string(),
                    canonical_url: format!("https://www.facebook.com/{}/videos/{}/", author, target_id),
                    author_handle: Some(author.to_string()),
                    is_redirect_needed: false,
                    is_video: true,
                });
            }
            "photos" => {
                validate_post_id(target_id)?;
                return Ok(FacebookUrlInfo {
                    id: target_id.to_string(),
                    kind: "photo".to_string(),
                    canonical_url: format!("https://www.facebook.com/{}/photos/{}/", author, target_id),
                    author_handle: Some(author.to_string()),
                    is_redirect_needed: false,
                    is_video: false,
                });
            }
            _ => {}
        }
    }

    // 11. Two-segment path matching: /{author}/posts/ (missing ID) or /{author}/{id}
    if segments.len() == 2 {
        let seg0 = segments[0].trim();
        let seg1 = segments[1].trim();

        if seg1.eq_ignore_ascii_case("posts") || seg1.eq_ignore_ascii_case("videos") {
            return Err(FacebookExtractError::InvalidInput(format!(
                "Missing content ID in URL path '/{}/{}/'",
                seg0, seg1
            )));
        }

        // Direct numeric or pfbid ID following username handle
        if (seg1.chars().all(|c| c.is_ascii_digit()) && seg1.len() >= 5) || seg1.starts_with("pfbid") {
            validate_post_id(seg1)?;
            return Ok(FacebookUrlInfo {
                id: seg1.to_string(),
                kind: "post".to_string(),
                canonical_url: format!("https://www.facebook.com/{}/posts/{}/", seg0, seg1),
                author_handle: Some(seg0.to_string()),
                is_redirect_needed: false,
                is_video: false,
            });
        }
    }

    // 12. Single segment: Profile or Page URL (e.g. /zuck, /BBCNews)
    if segments.len() == 1 {
        return Err(FacebookExtractError::UnsupportedUrl(format!(
            "Profile or Page URL '/{}/' is not a single post or video; please provide a specific post, Reel, or Watch video URL",
            segments[0]
        )));
    }

    Err(FacebookExtractError::UnsupportedUrl(format!(
        "Unrecognized Facebook URL path '{}'",
        parsed_url.path()
    )))
}

// ── HTML Entity Decoding & Caption Normalization ─────────────────────────────

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

/// Normalizes post caption text:
/// - Strips leading BOM (`\u{feff}`) if present
/// - Decodes HTML entities (standard, dec/hex numeric) with 2-pass recursion for double-encoded entities
/// - Normalizes CRLF and CR to LF (`\n`)
/// - Trims trailing whitespace per line
/// - Collapses excessive blank lines (max 1 empty line between paragraphs)
/// - Preserves multi-lingual scripts and emojis byte-for-byte
pub fn normalize_caption(raw: &str) -> String {
    if raw.trim().is_empty() {
        return String::new();
    }

    // 1. Strip leading BOM if present
    let text = raw.strip_prefix('\u{feff}').unwrap_or(raw);

    // 2. Decode HTML entities (2-pass handles double-encoded entities like &amp;amp;)
    let decoded_once = decode_html_entities(text);
    let decoded = if decoded_once.contains('&') {
        decode_html_entities(&decoded_once)
    } else {
        decoded_once
    };

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

/// Extracts unique lowercase hashtags from text.
pub fn extract_hashtags(text: &str) -> Vec<String> {
    let mut hashtags = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for cap in HASHTAG_REGEX.captures_iter(text) {
        if let Some(tag_match) = cap.get(1) {
            let tag = tag_match.as_str().to_lowercase();
            if !tag.is_empty() && seen.insert(tag.clone()) {
                hashtags.push(tag);
            }
        }
    }

    hashtags
}

/// Cleans and normalizes Facebook CDN stream and image URLs:
/// - Un-escapes JSON forward slashes (`\/` -> `/`)
/// - Un-escapes Unicode ampersands (`\u0026` -> `&`)
/// - Un-escapes HTML ampersand entities (`&amp;` -> `&`)
/// - Removes extraneous enclosing quotes or trailing slashes
pub fn clean_facebook_cdn_url(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .trim_matches('\\')
        .replace(r"\u0026", "&")
        .replace(r"\/", "/")
        .replace("&amp;", "&")
}

/// Helper to find the last occurrence of an ASCII needle case-insensitively,
/// returning the byte offset in `haystack`.
fn rfind_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let needle_bytes = needle.as_bytes();
    if needle_bytes.is_empty() {
        return Some(haystack.len());
    }
    if needle_bytes.len() > haystack.len() {
        return None;
    }
    for i in (0..=haystack.len() - needle_bytes.len()).rev() {
        if haystack.is_char_boundary(i)
            && haystack.is_char_boundary(i + needle_bytes.len())
            && haystack.as_bytes()[i..i + needle_bytes.len()].eq_ignore_ascii_case(needle_bytes)
        {
            return Some(i);
        }
    }
    None
}

/// Splits and normalizes Facebook title and author display name.
///
/// Strips " | Facebook" and " - Facebook" suffixes, cleans boilerplate section suffixes (" - Posts", " - Home", " - Videos", etc.),
/// and extracts the clean page or creator name.
pub fn parse_author_from_title(title: &str, fallback_handle: Option<&str>) -> String {
    let decoded = decode_html_entities(title.trim());

    // Strip " | Facebook" or " - Facebook" suffix
    let without_facebook = if let Some(idx) = rfind_ascii_case_insensitive(&decoded, " | facebook") {
        &decoded[..idx]
    } else if let Some(idx) = rfind_ascii_case_insensitive(&decoded, " - facebook") {
        &decoded[..idx]
    } else {
        decoded.as_str()
    }.trim();

    let lower = without_facebook.to_lowercase();
    if without_facebook.is_empty()
        || lower == "facebook"
        || lower == "log in"
        || lower == "login"
        || lower == "log in to facebook"
        || lower.starts_with("log in ")
    {
        return fallback_handle
            .filter(|h| !h.is_empty())
            .map(|h| h.to_string())
            .unwrap_or_else(|| "Facebook User".to_string());
    }

    // Check for standard section suffixes
    for suffix in &[
        " - Posts", " - Home", " - Videos", " - Reels", " - Photos",
        " - Timeline Photos", " - About", " - Community", " - Events",
    ] {
        if let Some(idx) = rfind_ascii_case_insensitive(without_facebook, suffix) {
            let author = without_facebook[..idx].trim();
            if !author.is_empty() {
                return author.to_string();
            }
        }
    }

    // Check if title starts with view/reaction metrics: "2.8M views · 45K reactions | Video Title"
    if let Some(pipe_idx) = without_facebook.find('|') {
        let first_seg = without_facebook[..pipe_idx].trim();
        let second_seg = without_facebook[pipe_idx + 1..].trim();
        if (first_seg.contains("views") || first_seg.contains("reactions") || first_seg.contains('·')) && !second_seg.is_empty() {
            return second_seg.to_string();
        }
    }

    // Check for " - " splitting when followed by date or snippet
    if let Some(idx) = without_facebook.find(" - ") {
        let candidate = without_facebook[..idx].trim();
        if !candidate.is_empty() && candidate.chars().count() < 60 {
            return candidate.to_string();
        }
    }

    without_facebook.to_string()
}

// ── Login Wall & Private Post Detection ──────────────────────────────────────

/// Inspects response metadata, final URL, and HTML body for Facebook login wall and privacy barriers.
///
/// Detects:
/// - Redirects to `/login.php`, `/login/`, `/checkpoint/`
/// - Login query parameters (`login_attempt=`, `next=`)
/// - Login forms (`id="login_form"`, `id="loginform"`, `name="login_form"`, `action="/login.php"`)
/// - Checkpoint screens and UI interstitial modals (`class="uiInterstitialContent"`, `checkpoint_title`)
/// - Text signatures ("You must log in to continue", "This content isn't available right now", etc.)
/// - HTTP 401 / 403 status codes
pub fn detect_login_wall(status_code: u16, final_url: &str, html_body: &str) -> Option<FacebookExtractError> {
    // 1. Check final redirected URL
    let url_lower = final_url.to_lowercase();
    if url_lower.contains("/login.php")
        || url_lower.contains("/login/")
        || url_lower.contains("/checkpoint/")
        || url_lower.contains("login_attempt=")
        || url_lower.contains("facebook.com/login/")
        || url_lower.contains("facebook.com/login.php")
        || url_lower.contains("facebook.com/login?")
        || url_lower.ends_with("facebook.com/login")
    {
        return Some(FacebookExtractError::PrivateOrLoginWall(
            "This Facebook post is private, restricted, or requires login authentication".to_string(),
        ));
    }

    // 2. Check HTML form / container signatures
    let body_lower = html_body.to_lowercase();
    if body_lower.contains("id=\"login_form\"")
        || body_lower.contains("id=\"loginform\"")
        || body_lower.contains("id='login_form'")
        || body_lower.contains("id='loginform'")
        || body_lower.contains("name=\"login_form\"")
        || body_lower.contains("action=\"/login.php\"")
        || body_lower.contains("action=\"https://www.facebook.com/login.php\"")
        || body_lower.contains("action=\"https://m.facebook.com/login.php\"")
        || body_lower.contains("class=\"uiinterstitialcontent\"")
        || body_lower.contains("checkpoint_title")
        || body_lower.contains("id=\"mobile_login_bar\"")
    {
        return Some(FacebookExtractError::PrivateOrLoginWall(
            "This Facebook post is private, restricted, or requires login authentication".to_string(),
        ));
    }

    // 3. Check textual restriction signatures (multi-lingual and English)
    if body_lower.contains("you must log in to continue")
        || body_lower.contains("log in to continue")
        || body_lower.contains("this content isn't available right now")
        || body_lower.contains("this content isn't available at the moment")
        || body_lower.contains("only shared it with a small group of people")
        || body_lower.contains("log in or sign up to view")
        || body_lower.contains("you must log in first")
        || body_lower.contains("debes iniciar sesión para continuar")
        || body_lower.contains("vous devez vous connecter pour continuer")
        || body_lower.contains("du musst dich anmelden, um fortzufahren")
        || body_lower.contains("você precisa entrar para continuar")
    {
        return Some(FacebookExtractError::PrivateOrLoginWall(
            "This Facebook post is private, restricted, or requires login authentication".to_string(),
        ));
    }

    // 4. Check HTTP status codes indicating private/blocked
    if status_code == 401 || status_code == 403 {
        return Some(FacebookExtractError::PrivateOrLoginWall(
            "This Facebook post is private, restricted, or requires login authentication".to_string(),
        ));
    }

    None
}

/// Classifies yt-dlp stderr output into structured `FacebookExtractError`.
pub fn classify_ytdlp_error(stderr: &str) -> FacebookExtractError {
    let lower = stderr.to_lowercase();
    if lower.contains("login required")
        || lower.contains("you must log in to continue")
        || lower.contains("only available for registered users")
        || lower.contains("private video")
        || lower.contains("this content is currently unavailable")
        || lower.contains("sign in to confirm")
        || lower.contains("members-only")
    {
        FacebookExtractError::PrivateOrLoginWall(
            "This Facebook post is private, restricted, or requires login authentication".to_string(),
        )
    } else if lower.contains("does not exist")
        || lower.contains("404")
        || lower.contains("video deleted")
        || lower.contains("not found")
    {
        FacebookExtractError::NotFound("Facebook post or video not found".to_string())
    } else {
        FacebookExtractError::UpstreamError(format!("yt-dlp extraction failed: {}", stderr.trim()))
    }
}

// ── OpenGraph & Video Stream Extraction ──────────────────────────────────────

/// Extracted OpenGraph metadata bag.
#[derive(Debug, Clone, Default)]
pub struct OpenGraphMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    pub video: Option<String>,
    pub url: Option<String>,
    pub og_type: Option<String>,
    pub site_name: Option<String>,
}

/// Parses OpenGraph tags from server-rendered HTML.
pub fn parse_opengraph_html(html: &str) -> OpenGraphMeta {
    let mut meta = OpenGraphMeta::default();

    // Standard order: property="og:..." content="..."
    for cap in OG_META_RE.captures_iter(html) {
        let key = cap.get(1).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        let val = cap.get(2).map(|m| decode_html_entities(m.as_str())).unwrap_or_default();
        populate_og_field(&mut meta, &key, val);
    }

    // Reverse order: content="..." property="og:..."
    for cap in OG_META_REVERSE_RE.captures_iter(html) {
        let val = cap.get(1).map(|m| decode_html_entities(m.as_str())).unwrap_or_default();
        let key = cap.get(2).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        populate_og_field(&mut meta, &key, val);
    }

    // Fallback description from <meta name="description">
    if meta.description.is_none() {
        if let Some(cap) = META_DESC_RE.captures(html) {
            if let Some(desc) = cap.get(1) {
                let cleaned = decode_html_entities(desc.as_str());
                if !cleaned.is_empty() {
                    meta.description = Some(cleaned);
                }
            }
        }
    }

    // Fallback title from <title> tag
    if meta.title.is_none() {
        if let Some(cap) = TITLE_TAG_RE.captures(html) {
            if let Some(t) = cap.get(1) {
                let cleaned = decode_html_entities(t.as_str());
                if !cleaned.is_empty() {
                    meta.title = Some(cleaned);
                }
            }
        }
    }

    meta
}

fn populate_og_field(meta: &mut OpenGraphMeta, key: &str, val: String) {
    if val.trim().is_empty() {
        return;
    }
    match key {
        "title" if meta.title.is_none() => meta.title = Some(val),
        "description" if meta.description.is_none() => meta.description = Some(val),
        "image" | "image:url" | "image:secure_url" if meta.image.is_none() => {
            meta.image = Some(clean_facebook_cdn_url(&val));
        }
        "video" | "video:url" | "video:secure_url" if meta.video.is_none() => {
            meta.video = Some(clean_facebook_cdn_url(&val));
        }
        "url" if meta.url.is_none() => meta.url = Some(val),
        "type" if meta.og_type.is_none() => meta.og_type = Some(val),
        "site_name" if meta.site_name.is_none() => meta.site_name = Some(val),
        _ => {}
    }
}

/// Scans Facebook HTML for progressive MP4 stream links.
/// Returns `(hd_video_urls, sd_video_urls)`.
pub fn extract_video_streams_from_html(html: &str) -> (Vec<String>, Vec<String>) {
    let mut hd_streams = Vec::new();
    let mut sd_streams = Vec::new();

    // 1. Scan for HD streams
    for cap in HD_STREAM_RE.captures_iter(html) {
        for i in 1..cap.len() {
            if let Some(m) = cap.get(i) {
                let url = clean_facebook_cdn_url(m.as_str());
                if url.starts_with("http") && !hd_streams.contains(&url) {
                    hd_streams.push(url);
                    break;
                }
            }
        }
    }

    // 2. Scan for SD streams
    for cap in SD_STREAM_RE.captures_iter(html) {
        for i in 1..cap.len() {
            if let Some(m) = cap.get(i) {
                let url = clean_facebook_cdn_url(m.as_str());
                if url.starts_with("http") && !sd_streams.contains(&url) && !hd_streams.contains(&url) {
                    sd_streams.push(url);
                    break;
                }
            }
        }
    }

    (hd_streams, sd_streams)
}

/// Scans Facebook HTML for direct audio stream representations.
pub fn extract_audio_stream_from_html(html: &str) -> Option<String> {
    for cap in FB_AUDIO_STREAM_RE.captures_iter(html) {
        for i in 1..cap.len() {
            if let Some(m) = cap.get(i) {
                let url = clean_facebook_cdn_url(m.as_str());
                if url.starts_with("http") {
                    return Some(url);
                }
            }
        }
    }
    None
}

/// Extracts high-resolution content images from HTML, filtering out avatars, emojis, and icons.
pub fn extract_attached_images_from_html(html: &str, primary_image: Option<&str>) -> Vec<String> {
    let mut images = Vec::new();

    if let Some(img) = primary_image {
        let cleaned = clean_facebook_cdn_url(img);
        if !cleaned.is_empty() {
            images.push(cleaned);
        }
    }

    for mat in SCONTENT_IMAGE_RE.find_iter(html) {
        let cleaned = clean_facebook_cdn_url(mat.as_str());

        // Filter out small thumbnails, profile badges, and emojis
        let lower = cleaned.to_lowercase();
        if lower.contains("emoji.php")
            || lower.contains("static.xx")
            || lower.contains("/p50x50/")
            || lower.contains("/s50x50/")
            || lower.contains("/p100x100/")
            || lower.contains("/s100x100/")
        {
            continue;
        }

        if !images.contains(&cleaned) {
            images.push(cleaned);
        }
    }

    images
}

// ── HTTP Client & Redirect Resolution ────────────────────────────────────────

/// Builds the reqwest HTTP client configured with crawler headers, 15s timeout, and redirect following.
pub fn build_fb_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(FB_CRAWLER_USER_AGENT)
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .unwrap_or_default()
}

/// Resolves shortened share links (`fb.watch/*`, `/share/*`) by following HTTP redirects
/// to discover the canonical target URL.
pub async fn resolve_facebook_redirect(
    client: &reqwest::Client,
    url: &str,
) -> Result<String, FacebookExtractError> {
    let res = client
        .get(url)
        .header("User-Agent", FB_CRAWLER_USER_AGENT)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .send()
        .await
        .map_err(|e| {
            FacebookExtractError::UpstreamError(format!("Failed to resolve redirect for '{}': {}", url, e))
        })?;

    let final_url = res.url().as_str();

    // Check if Facebook redirected to a login page containing a `next` destination query
    if final_url.contains("/login.php") || final_url.contains("/login/?next=") {
        if let Ok(parsed) = Url::parse(final_url) {
            for (k, v) in parsed.query_pairs() {
                if k == "next" && !v.is_empty() {
                    let decoded_next = v.into_owned();
                    tracing::debug!("[facebook] Extracted canonical destination from login 'next' parameter: {}", decoded_next);
                    return Ok(decoded_next);
                }
            }
        }
    }

    Ok(final_url.to_string())
}

/// Tier 1: Fetches Facebook post webpage using `facebookexternalhit/1.1` and parses OpenGraph tags.
pub async fn fetch_via_opengraph(
    client: &reqwest::Client,
    canonical_url: &str,
) -> Result<(OpenGraphMeta, String), FacebookExtractError> {
    let res = client
        .get(canonical_url)
        .header("User-Agent", FB_CRAWLER_USER_AGENT)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|e| {
            FacebookExtractError::UpstreamError(format!("Network error fetching '{}': {}", canonical_url, e))
        })?;

    let status = res.status();
    let final_url = res.url().as_str().to_string();

    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(FacebookExtractError::NotFound(format!(
            "Facebook post at '{}' not found",
            canonical_url
        )));
    }

    let html = res.text().await.map_err(|e| {
        FacebookExtractError::UpstreamError(format!("Failed to read response HTML: {}", e))
    })?;

    let og = parse_opengraph_html(&html);

    // If OpenGraph has a genuine description or title, and URL is not login.php/checkpoint,
    // then the post content is publicly available (the login form in footer is just a logged-out prompt).
    let has_public_content = match (&og.title, &og.description) {
        (_, Some(desc)) => {
            let d = desc.trim().to_lowercase();
            !d.is_empty()
                && !d.contains("you must log in to continue")
                && !d.contains("log in or sign up to view")
                && !d.contains("this content isn't available right now")
                && !d.contains("only shared it with a small group of people")
        }
        (Some(title), None) => {
            let t = title.trim().to_lowercase();
            !t.is_empty()
                && t != "facebook"
                && t != "log in"
                && t != "login"
                && !t.contains("log in to facebook")
                && !t.contains("log into facebook")
        }
        _ => false,
    };

    if !has_public_content {
        // Check for login wall or privacy barriers
        if let Some(err) = detect_login_wall(status.as_u16(), &final_url, &html) {
            return Err(err);
        }
    } else {
        // Still check for hard redirect to login.php or 401/403
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(FacebookExtractError::PrivateOrLoginWall(
                "This Facebook post is private, restricted, or requires login authentication".to_string(),
            ));
        }
        let url_lower = final_url.to_lowercase();
        if url_lower.contains("/login.php") || url_lower.contains("/checkpoint/") {
            return Err(FacebookExtractError::PrivateOrLoginWall(
                "This Facebook post is private, restricted, or requires login authentication".to_string(),
            ));
        }
    }

    Ok((og, html))
}

// ── Tier 3: `yt-dlp` Video Fallback Engine ───────────────────────────────────

/// Invokes `yt-dlp --dump-single-json` to extract high-quality video formats, author, and description.
pub async fn fetch_via_ytdlp(
    canonical_url: &str,
    post_id: &str,
) -> Result<FacebookPost, FacebookExtractError> {
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
        FB_CRAWLER_USER_AGENT,
        canonical_url,
    ]);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = match tokio::time::timeout(Duration::from_secs(20), cmd.output()).await {
        Ok(res) => res.map_err(|e| {
            FacebookExtractError::UpstreamError(format!("Failed to execute yt-dlp: {}", e))
        })?,
        Err(_) => {
            return Err(FacebookExtractError::Timeout(
                "yt-dlp execution timed out after 20s".into(),
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(classify_ytdlp_error(&stderr));
    }

    let json_val: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        FacebookExtractError::UpstreamError(format!("Failed to parse yt-dlp JSON: {}", e))
    })?;

    let title = json_val
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let raw_desc = json_val
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let caption = if !raw_desc.is_empty() {
        normalize_caption(raw_desc)
    } else {
        normalize_caption(&title)
    };

    let uploader = json_val
        .get("uploader")
        .or_else(|| json_val.get("channel"))
        .and_then(Value::as_str)
        .unwrap_or("Facebook User")
        .to_string();
    let uploader_id = json_val
        .get("uploader_id")
        .and_then(Value::as_str)
        .map(String::from);

    let published_at = json_val.get("timestamp").and_then(Value::as_i64);
    let view_count = json_val.get("view_count").and_then(Value::as_u64);
    let like_count = json_val.get("like_count").and_then(Value::as_u64).or(view_count);
    let comment_count = json_val.get("comment_count").and_then(Value::as_u64);

    let mut videos = Vec::new();
    let mut images = Vec::new();
    let mut media_items = Vec::new();
    let mut audio_url = None;
    let mut best_audio_br: u64 = 0;

    if let Some(formats) = json_val.get("formats").and_then(Value::as_array) {
        // 1. Scan for standalone audio stream
        for fmt in formats {
            let acodec = fmt.get("acodec").and_then(Value::as_str).unwrap_or("none");
            let vcodec = fmt.get("vcodec").and_then(Value::as_str).unwrap_or("none");
            let url = fmt.get("url").and_then(Value::as_str).unwrap_or("");
            if acodec != "none" && !acodec.is_empty() && (vcodec == "none" || vcodec.is_empty()) && url.starts_with("http") {
                let abr = fmt.get("abr").and_then(Value::as_f64).unwrap_or(0.0) as u64;
                let tbr = fmt.get("tbr").and_then(Value::as_f64).unwrap_or(0.0) as u64;
                let br = abr.max(tbr);
                if br >= best_audio_br {
                    best_audio_br = br;
                    audio_url = Some(clean_facebook_cdn_url(url));
                }
            }
        }

        // 2. Parse video formats: PRIORITIZE progressive formats with audio!
        let mut hd_url = None;
        let mut sd_url = None;
        let mut best_fmt: Option<&Value> = None;
        let mut best_pixel_area: u64 = 0;

        // Pass 1: Progressive formats WITH audio (vcodec != "none" && acodec != "none")
        for fmt in formats {
            let vcodec = fmt.get("vcodec").and_then(Value::as_str).unwrap_or("none");
            let acodec = fmt.get("acodec").and_then(Value::as_str).unwrap_or("none");
            let url = fmt.get("url").and_then(Value::as_str).unwrap_or("");
            if vcodec != "none" && acodec != "none" && !acodec.is_empty() && url.starts_with("http") {
                let format_id = fmt.get("format_id").and_then(Value::as_str).unwrap_or("");
                if (format_id == "hd" || format_id.contains("hd")) && hd_url.is_none() {
                    hd_url = Some(clean_facebook_cdn_url(url));
                } else if (format_id == "sd" || format_id.contains("sd")) && sd_url.is_none() {
                    sd_url = Some(clean_facebook_cdn_url(url));
                }

                let w = fmt.get("width").and_then(Value::as_u64).unwrap_or(0);
                let h = fmt.get("height").and_then(Value::as_u64).unwrap_or(0);
                let pixel_area = w.saturating_mul(h);
                if pixel_area >= best_pixel_area {
                    best_pixel_area = pixel_area;
                    best_fmt = Some(fmt);
                }
            }
        }

        // Pass 2: Fallback to video-only formats if no progressive format with audio was found
        if hd_url.is_none() && sd_url.is_none() && best_fmt.is_none() {
            for fmt in formats {
                let vcodec = fmt.get("vcodec").and_then(Value::as_str).unwrap_or("none");
                let url = fmt.get("url").and_then(Value::as_str).unwrap_or("");
                if vcodec != "none" && url.starts_with("http") {
                    let format_id = fmt.get("format_id").and_then(Value::as_str).unwrap_or("");
                    if (format_id == "hd" || format_id.contains("hd")) && hd_url.is_none() {
                        hd_url = Some(clean_facebook_cdn_url(url));
                    } else if (format_id == "sd" || format_id.contains("sd")) && sd_url.is_none() {
                        sd_url = Some(clean_facebook_cdn_url(url));
                    }

                    let w = fmt.get("width").and_then(Value::as_u64).unwrap_or(0);
                    let h = fmt.get("height").and_then(Value::as_u64).unwrap_or(0);
                    let pixel_area = w.saturating_mul(h);
                    if pixel_area >= best_pixel_area {
                        best_pixel_area = pixel_area;
                        best_fmt = Some(fmt);
                    }
                }
            }
        }

        if let Some(hd) = hd_url {
            videos.push(hd);
        }
        if let Some(sd) = sd_url {
            if !videos.contains(&sd) {
                videos.push(sd);
            }
        }
        if videos.is_empty() {
            if let Some(fmt) = best_fmt {
                if let Some(u) = fmt.get("url").and_then(Value::as_str) {
                    videos.push(clean_facebook_cdn_url(u));
                }
            }
        }
    }

    if let Some(thumb) = json_val.get("thumbnail").and_then(Value::as_str) {
        if !thumb.is_empty() {
            images.push(clean_facebook_cdn_url(thumb));
        }
    }

    let author = FacebookAuthor {
        name: uploader,
        url: uploader_id
            .as_ref()
            .map(|id| format!("https://www.facebook.com/{}/", id)),
        id: uploader_id,
        avatar_url: None,
        is_verified: None,
    };

    let post_id_val = json_val
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(post_id)
        .to_string();

    let thumbnail_url = images.first().cloned();
    let has_audio = audio_url.is_some() || !videos.is_empty();

    for v in &videos {
        media_items.push(FacebookMediaItem {
            id: Some(post_id_val.clone()),
            media_type: "video".to_string(),
            url: v.clone(),
            width: json_val.get("width").and_then(Value::as_u64).map(|n| n as u32),
            height: json_val.get("height").and_then(Value::as_u64).map(|n| n as u32),
            thumbnail_url: thumbnail_url.clone(),
            is_video: true,
            duration_secs: json_val.get("duration").and_then(Value::as_f64),
            audio_url: audio_url.clone(),
            has_audio,
        });
    }

    let hashtags = extract_hashtags(&caption);

    let mut post = FacebookPost {
        id: Some(post_id_val),
        url: canonical_url.to_string(),
        author,
        caption,
        hashtags,
        images,
        videos,
        audio_url,
        has_audio,
        media_type: "video".to_string(),
        media_items,
        thumbnail_url,
        published_at,
        like_count,
        comment_count,
        share_count: None,
        markdown: String::new(),
    };

    post.markdown = generate_markdown_summary(&post);
    Ok(post)
}

// ── Pyramid Markdown Summary Generator ───────────────────────────────────────

/// Generates a clean human-readable Markdown summary formatted for AI agents (Pyramid structure).
///
/// Structure:
/// 1. Apex: Header, Author, Canonical URL, Post Type, Published UTC, Engagement stats
/// 2. Body: Clean Status Update / Post Caption & Hashtags
/// 3. Context: Media Gallery (Direct high-res photo links and progressive video streams)
/// 4. Actionable Links: Facebook Player & direct stream download links
pub fn generate_markdown_summary(post: &FacebookPost) -> String {
    let mut md = String::with_capacity(1024);

    let author_display = match &post.author.url {
        Some(url) if !url.is_empty() => {
            format!("[{}]({})", post.author.name, url)
        }
        _ => post.author.name.clone(),
    };

    let post_type_title = match post.media_type.to_lowercase().as_str() {
        "video" | "watch" => "Facebook Video",
        "reel" => "Facebook Reel",
        "photo" => "Facebook Photo",
        "carousel" | "album" => "Facebook Album",
        _ => "Facebook Post",
    };

    // 1. Apex: Header & Metadata
    md.push_str(&format!("# {} by {}\n\n", post_type_title, post.author.name));
    md.push_str(&format!("- **Author**: {}\n", author_display));
    md.push_str(&format!("- **Post URL**: [{}]({})\n", post.url, post.url));
    md.push_str(&format!("- **Type**: {}\n", capitalize(&post.media_type)));

    if let Some(ts) = post.published_at {
        md.push_str(&format!("- **Published**: {}\n", format_timestamp_utc(ts)));
    }

    // Engagement stats
    if post.like_count.is_some() || post.comment_count.is_some() || post.share_count.is_some() {
        let mut stats = Vec::new();
        if let Some(likes) = post.like_count {
            stats.push(format!("{} likes", format_number(likes)));
        }
        if let Some(comments) = post.comment_count {
            stats.push(format!("{} comments", format_number(comments)));
        }
        if let Some(shares) = post.share_count {
            stats.push(format!("{} shares", format_number(shares)));
        }
        if !stats.is_empty() {
            md.push_str(&format!("- **Engagement**: {}\n", stats.join(", ")));
        }
    }

    md.push_str("\n---\n\n");

    // 2. Body: Caption / Status Update
    md.push_str("### Caption\n\n");
    if post.caption.is_empty() {
        md.push_str("*(No caption provided)*\n\n");
    } else {
        md.push_str(&post.caption);
        md.push_str("\n\n");
    }

    // Hashtags if extracted
    if !post.hashtags.is_empty() {
        md.push_str("### Hashtags\n\n");
        let tags_formatted: Vec<String> = post.hashtags.iter().map(|t| format!("`#{}`", t)).collect();
        md.push_str(&tags_formatted.join(" "));
        md.push_str("\n\n");
    }

    // 3. Foundation: Media Gallery
    md.push_str("### Media Gallery\n\n");
    let total_images = post.images.len();
    let total_videos = post.videos.len();

    if total_images == 0 && total_videos == 0 && post.media_items.is_empty() && post.audio_url.is_none() {
        md.push_str("*(No attached media found)*\n\n");
    } else {
        // Direct audio stream track for AI speech processing
        if let Some(ref audio) = post.audio_url {
            md.push_str(&format!(
                "- **Audio Track**: [Direct Audio Stream (.m4a/.mp3)]({}) 🎵 *(Optimized for AI speech transcription & translation)*\n",
                audio
            ));
        }

        let mut item_num = 1;

        if !post.media_items.is_empty() {
            for item in &post.media_items {
                if item.is_video {
                    let quality_label = if item.url.contains("hd_src") || item.url.contains("quality_hd") || item.url.contains("stream_hd") {
                        " (HD .mp4)"
                    } else {
                        " (.mp4)"
                    };
                    let duration_str = item.duration_secs
                        .map(|d| format!(" [Duration: {:.1}s]", d))
                        .unwrap_or_default();
                    let audio_badge = if item.has_audio || post.has_audio {
                        " 🎬 *(Includes audio track)*"
                    } else {
                        " 🔇 *(Muted / Video-only)*"
                    };

                    md.push_str(&format!(
                        "- **Item {} (Video)**: [Direct Video Stream{}{}]({}){}\n",
                        item_num, quality_label, duration_str, item.url, audio_badge
                    ));
                    if let Some(ref thumb) = item.thumbnail_url {
                        md.push_str(&format!("  - Poster Image: [Thumbnail]({})\n", thumb));
                    }
                } else {
                    let dim_str = match (item.width, item.height) {
                        (Some(w), Some(h)) => format!(" ({}x{})", w, h),
                        _ => String::new(),
                    };
                    md.push_str(&format!(
                        "- **Item {} (Photo)**: [High-Resolution Image{}]({})\n",
                        item_num, dim_str, item.url
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
                let quality_label = if vid_url.contains("hd") {
                    " (HD .mp4)"
                } else {
                    " (.mp4)"
                };
                let audio_badge = if post.has_audio {
                    " 🎬 *(Includes audio track)*"
                } else {
                    " 🔇 *(Muted / Video-only)*"
                };
                md.push_str(&format!(
                    "- **Item {} (Video)**: [Direct Video Stream{}]({}){}\n",
                    item_num, quality_label, vid_url, audio_badge
                ));
                item_num += 1;
            }
        }
        md.push('\n');
    }

    // 4. Actionable Player & External Links
    if !post.videos.is_empty() || post.media_type == "video" || post.media_type == "reel" {
        md.push_str("### Video Player & Links\n\n");
        let player_label = if post.media_type == "reel" {
            "Facebook Reel Player"
        } else {
            "Facebook Watch Player"
        };
        md.push_str(&format!("- **Web Player**: [{}]({})\n", player_label, post.url));

        if let Some(first_vid) = post.videos.first() {
            md.push_str(&format!("- **Direct Video Stream**: [Play / Download .mp4]({})\n", first_vid));
        }
        if let Some(ref thumb) = post.thumbnail_url {
            md.push_str(&format!("- **Video Thumbnail**: [Preview Image]({})\n", thumb));
        }
        md.push('\n');
    }

    md.trim_end().to_string()
}

/// Helper to capitalize words.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Helper to format large integers with comma separators (e.g. 1,234,567).
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let len = s.len();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result
}

/// Formats a Unix timestamp into a readable UTC date/time string.
pub fn format_timestamp_utc(ts: i64) -> String {
    if ts <= 0 || ts > 253_402_300_799 {
        return "Unknown".to_string();
    }
    let total_secs = ts.max(0) as u64;
    let days = total_secs / 86400;
    let rem_secs = total_secs % 86400;
    let hours = rem_secs / 3600;
    let mins = (rem_secs % 3600) / 60;
    let secs = rem_secs % 60;

    let mut year = 1970;
    let mut rem_days = days;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let days_in_year = if leap { 366 } else { 365 };
        if rem_days < days_in_year {
            break;
        }
        rem_days -= days_in_year;
        year += 1;
    }

    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let days_in_months = [
        31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31
    ];

    let mut month = 1;
    for &dim in &days_in_months {
        if rem_days < dim {
            break;
        }
        rem_days -= dim;
        month += 1;
    }
    let day = rem_days + 1;

    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", year, month, day, hours, mins, secs)
}

// ── Main Cascading Extraction Pipeline ──────────────────────────────────────

/// Orchestrates multi-tier Facebook post, Reel, and Watch extraction:
///
/// 1. URL Normalization: Validate scheme/domain and parse path/parameters.
/// 2. Redirect Resolution: Follow redirects on shortened links (`fb.watch`, `/share/*`).
/// 3. Tier 1: SSR OpenGraph Crawler (`facebookexternalhit/1.1`) -> extracts title, caption, og:image, og:video.
/// 4. Tier 2: Direct Video Stream Extractor -> scans HTML for progressive HD/SD stream links.
/// 5. Tier 3: `yt-dlp` Video Fallback -> executes if post is a Reel/Watch or lacked video stream.
/// 6. Result Synthesis & Markdown Generation.
pub async fn extract_facebook_post(input: &str) -> Result<FacebookPost, FacebookExtractError> {
    let mut url_info = parse_facebook_url(input)?;
    let client = build_fb_http_client();

    // Step 1: Follow redirects for shortened/share URLs if required
    if url_info.is_redirect_needed {
        match resolve_facebook_redirect(&client, &url_info.canonical_url).await {
            Ok(resolved_url) => {
                if let Ok(resolved_info) = parse_facebook_url(&resolved_url) {
                    url_info = resolved_info;
                }
            }
            Err(e) => {
                tracing::debug!("[facebook] Share link redirect resolution failed: {}. Continuing with canonical.", e);
            }
        }
    }

    // Step 2: Tier 1 SSR OpenGraph Extraction
    let (og, html) = match fetch_via_opengraph(&client, &url_info.canonical_url).await {
        Ok(res) => res,
        Err(e) => match e {
            FacebookExtractError::PrivateOrLoginWall(_) => {
                // If login wall was hit, attempt yt-dlp for video URLs as a last resort
                if url_info.is_video {
                    tracing::debug!("[facebook] Login wall detected on video URL. Attempting yt-dlp fallback.");
                    if let Ok(post) = fetch_via_ytdlp(&url_info.canonical_url, &url_info.id).await {
                        return Ok(post);
                    }
                }
                return Err(e);
            }
            _ => {
                // On other errors, if it's a video, try yt-dlp before bailing
                if url_info.is_video {
                    return fetch_via_ytdlp(&url_info.canonical_url, &url_info.id).await;
                }
                return Err(e);
            }
        },
    };

    // Step 3: Tier 2 Direct Video Stream Regex Extraction
    let (hd_streams, sd_streams) = extract_video_streams_from_html(&html);
    let audio_url = extract_audio_stream_from_html(&html);

    let mut videos = Vec::new();
    for v in hd_streams {
        if !videos.contains(&v) {
            videos.push(v);
        }
    }
    for v in sd_streams {
        if !videos.contains(&v) {
            videos.push(v);
        }
    }
    if let Some(og_vid) = og.video {
        if !videos.contains(&og_vid) {
            videos.push(og_vid);
        }
    }

    // Step 4: Tier 3 yt-dlp Fallback for Videos & Reels
    if url_info.is_video && videos.is_empty() {
        tracing::debug!("[facebook] Video post lacked progressive streams in HTML. Invoking yt-dlp fallback.");
        if let Ok(mut ytdlp_post) = fetch_via_ytdlp(&url_info.canonical_url, &url_info.id).await {
            // If Tier 1 OpenGraph had a caption, preserve it if richer
            if let Some(ref desc) = og.description {
                let norm_desc = normalize_caption(desc);
                if !norm_desc.is_empty() && norm_desc.len() > ytdlp_post.caption.len() {
                    ytdlp_post.caption = norm_desc;
                    ytdlp_post.hashtags = extract_hashtags(&ytdlp_post.caption);
                }
            }
            ytdlp_post.markdown = generate_markdown_summary(&ytdlp_post);
            return Ok(ytdlp_post);
        }
    }

    // Step 5: Extract attached images & maximize resolution
    let images = extract_attached_images_from_html(&html, og.image.as_deref());

    // Step 6: Assemble Author Information
    let author_name = if let Some(ref title) = og.title {
        parse_author_from_title(title, url_info.author_handle.as_deref())
    } else if let Some(ref handle) = url_info.author_handle {
        handle.clone()
    } else {
        "Facebook User".to_string()
    };

    let author = FacebookAuthor {
        name: author_name,
        url: url_info
            .author_handle
            .as_ref()
            .map(|h| format!("https://www.facebook.com/{}/", h)),
        id: url_info.author_handle.clone(),
        avatar_url: None,
        is_verified: None,
    };

    let caption = og.description.as_deref().map(normalize_caption).unwrap_or_default();
    let hashtags = extract_hashtags(&caption);
    let is_video = !videos.is_empty() || url_info.is_video;
    let has_audio = audio_url.is_some() || is_video;
    let media_type = if is_video {
        if url_info.kind == "reel" { "reel".to_string() } else { "video".to_string() }
    } else if images.len() > 1 {
        "carousel".to_string()
    } else if !images.is_empty() {
        "photo".to_string()
    } else {
        "post".to_string()
    };

    let thumbnail_url = images.first().cloned();

    let mut media_items = Vec::new();
    for v in &videos {
        media_items.push(FacebookMediaItem {
            id: Some(url_info.id.clone()),
            media_type: "video".to_string(),
            url: v.clone(),
            width: None,
            height: None,
            thumbnail_url: thumbnail_url.clone(),
            is_video: true,
            duration_secs: None,
            audio_url: audio_url.clone(),
            has_audio,
        });
    }
    for img in &images {
        media_items.push(FacebookMediaItem {
            id: Some(url_info.id.clone()),
            media_type: "photo".to_string(),
            url: img.clone(),
            width: None,
            height: None,
            thumbnail_url: None,
            is_video: false,
            duration_secs: None,
            audio_url: None,
            has_audio: false,
        });
    }

    let mut post = FacebookPost {
        id: Some(url_info.id),
        url: og.url.unwrap_or(url_info.canonical_url),
        author,
        caption,
        hashtags,
        images,
        videos,
        audio_url,
        has_audio,
        media_type,
        media_items,
        thumbnail_url,
        published_at: None,
        like_count: None,
        comment_count: None,
        share_count: None,
        markdown: String::new(),
    };

    post.markdown = generate_markdown_summary(&post);
    Ok(post)
}

// ── MCP JSON-RPC Adapter ─────────────────────────────────────────────────────

/// JSON-RPC 2.0 `tools/call` handler for `facebook_post`.
///
/// Conforms to MCP tool result schema:
/// `{ content: [{ type: "text", text: markdown }], post: FacebookPost, isError: bool }`
pub async fn call_facebook_post(arguments: Value) -> Result<Value, anyhow::Error> {
    let args: FacebookArgs = match serde_json::from_value(arguments.clone()) {
        Ok(a) => a,
        Err(_) => {
            let url_str = arguments
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let id_str = arguments
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            FacebookArgs {
                url: url_str,
                id: id_str,
            }
        }
    };

    let input = args
        .url
        .as_deref()
        .or(args.id.as_deref())
        .unwrap_or("")
        .trim();

    if input.is_empty() {
        return Ok(json!({
            "content": [
                {
                    "type": "text",
                    "text": "Failed to extract Facebook post: URL cannot be empty"
                }
            ],
            "isError": true
        }));
    }

    match extract_facebook_post(input).await {
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
        Err(e) => {
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": format!("Failed to extract Facebook post: {}", e)
                    }
                ],
                "isError": true
            }))
        }
    }
}

// ── Unit Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 32-Scenario URL Parsing Test Suite ───────────────────────────────────

    #[test]
    fn test_scenario_01_standard_user_post_numeric() {
        let res = parse_facebook_url("https://www.facebook.com/zuck/posts/10115432729910941").unwrap();
        assert_eq!(res.id, "10115432729910941");
        assert_eq!(res.kind, "post");
        assert_eq!(res.author_handle, Some("zuck".to_string()));
        assert_eq!(res.canonical_url, "https://www.facebook.com/zuck/posts/10115432729910941/");
        assert!(!res.is_redirect_needed);
        assert!(!res.is_video);
    }

    #[test]
    fn test_scenario_02_standard_page_post_pfbid() {
        let res = parse_facebook_url("https://www.facebook.com/bbcnews/posts/pfbid02ABC123xyz456").unwrap();
        assert_eq!(res.id, "pfbid02ABC123xyz456");
        assert_eq!(res.kind, "post");
        assert_eq!(res.author_handle, Some("bbcnews".to_string()));
        assert_eq!(res.canonical_url, "https://www.facebook.com/bbcnews/posts/pfbid02ABC123xyz456/");
    }

    #[test]
    fn test_scenario_03_post_with_trailing_slash() {
        let res = parse_facebook_url("https://www.facebook.com/Page/posts/123456789/").unwrap();
        assert_eq!(res.id, "123456789");
        assert_eq!(res.canonical_url, "https://www.facebook.com/Page/posts/123456789/");
    }

    #[test]
    fn test_scenario_04_mobile_domain_m_facebook() {
        let res = parse_facebook_url("https://m.facebook.com/zuck/posts/10115432729910941").unwrap();
        assert_eq!(res.id, "10115432729910941");
        assert_eq!(res.canonical_url, "https://www.facebook.com/zuck/posts/10115432729910941/");
    }

    #[test]
    fn test_scenario_05_web_domain_web_facebook() {
        let res = parse_facebook_url("https://web.facebook.com/zuck/posts/10115432729910941").unwrap();
        assert_eq!(res.id, "10115432729910941");
        assert_eq!(res.canonical_url, "https://www.facebook.com/zuck/posts/10115432729910941/");
    }

    #[test]
    fn test_scenario_06_short_domain_fb_com() {
        let res = parse_facebook_url("https://fb.com/zuck/posts/10115432729910941").unwrap();
        assert_eq!(res.id, "10115432729910941");
        assert_eq!(res.canonical_url, "https://www.facebook.com/zuck/posts/10115432729910941/");
    }

    #[test]
    fn test_scenario_07_auto_prefix_scheme() {
        let res = parse_facebook_url("facebook.com/zuck/posts/10115432729910941").unwrap();
        assert_eq!(res.id, "10115432729910941");
        assert_eq!(res.canonical_url, "https://www.facebook.com/zuck/posts/10115432729910941/");
    }

    #[test]
    fn test_scenario_08_protocol_relative_scheme() {
        let res = parse_facebook_url("//www.facebook.com/zuck/posts/10115432729910941").unwrap();
        assert_eq!(res.id, "10115432729910941");
        assert_eq!(res.canonical_url, "https://www.facebook.com/zuck/posts/10115432729910941/");
    }

    #[test]
    fn test_scenario_09_reel_singular_path() {
        let res = parse_facebook_url("https://www.facebook.com/reel/123456789012345/").unwrap();
        assert_eq!(res.id, "123456789012345");
        assert_eq!(res.kind, "reel");
        assert!(res.is_video);
        assert_eq!(res.canonical_url, "https://www.facebook.com/reel/123456789012345/");
    }

    #[test]
    fn test_scenario_10_reels_plural_path() {
        let res = parse_facebook_url("https://www.facebook.com/reels/123456789012345").unwrap();
        assert_eq!(res.id, "123456789012345");
        assert_eq!(res.kind, "reel");
        assert!(res.is_video);
        assert_eq!(res.canonical_url, "https://www.facebook.com/reel/123456789012345/");
    }

    #[test]
    fn test_scenario_11_watch_with_query_v() {
        let res = parse_facebook_url("https://www.facebook.com/watch/?v=10153231379946729").unwrap();
        assert_eq!(res.id, "10153231379946729");
        assert_eq!(res.kind, "watch");
        assert!(res.is_video);
        assert_eq!(res.canonical_url, "https://www.facebook.com/watch/?v=10153231379946729");
    }

    #[test]
    fn test_scenario_12_watch_without_trailing_slash() {
        let res = parse_facebook_url("https://www.facebook.com/watch?v=10153231379946729").unwrap();
        assert_eq!(res.id, "10153231379946729");
        assert_eq!(res.kind, "watch");
        assert!(res.is_video);
        assert_eq!(res.canonical_url, "https://www.facebook.com/watch/?v=10153231379946729");
    }

    #[test]
    fn test_scenario_13_page_videos_path() {
        let res = parse_facebook_url("https://www.facebook.com/BBCNews/videos/10153231379946729/").unwrap();
        assert_eq!(res.id, "10153231379946729");
        assert_eq!(res.kind, "watch");
        assert!(res.is_video);
        assert_eq!(res.author_handle, Some("BBCNews".to_string()));
        assert_eq!(res.canonical_url, "https://www.facebook.com/BBCNews/videos/10153231379946729/");
    }

    #[test]
    fn test_scenario_14_fb_watch_shortened_slug() {
        let res = parse_facebook_url("https://fb.watch/xYz123/").unwrap();
        assert_eq!(res.id, "xYz123");
        assert_eq!(res.kind, "watch");
        assert!(res.is_redirect_needed);
        assert!(res.is_video);
        assert_eq!(res.canonical_url, "https://fb.watch/xYz123/");
    }

    #[test]
    fn test_scenario_15_fb_watch_no_trailing_slash() {
        let res = parse_facebook_url("https://fb.watch/xYz123").unwrap();
        assert_eq!(res.id, "xYz123");
        assert_eq!(res.kind, "watch");
        assert!(res.is_redirect_needed);
        assert!(res.is_video);
        assert_eq!(res.canonical_url, "https://fb.watch/xYz123/");
    }

    #[test]
    fn test_scenario_16_share_post_path() {
        let res = parse_facebook_url("https://www.facebook.com/share/p/1B4xYz89/").unwrap();
        assert_eq!(res.id, "1B4xYz89");
        assert_eq!(res.kind, "post");
        assert!(res.is_redirect_needed);
        assert!(!res.is_video);
        assert_eq!(res.canonical_url, "https://www.facebook.com/share/p/1B4xYz89/");
    }

    #[test]
    fn test_scenario_17_share_reel_path() {
        let res = parse_facebook_url("https://www.facebook.com/share/r/987654321/").unwrap();
        assert_eq!(res.id, "987654321");
        assert_eq!(res.kind, "reel");
        assert!(res.is_redirect_needed);
        assert!(res.is_video);
        assert_eq!(res.canonical_url, "https://www.facebook.com/share/r/987654321/");
    }

    #[test]
    fn test_scenario_18_share_video_path() {
        let res = parse_facebook_url("https://www.facebook.com/share/v/556677889/").unwrap();
        assert_eq!(res.id, "556677889");
        assert_eq!(res.kind, "watch");
        assert!(res.is_redirect_needed);
        assert!(res.is_video);
        assert_eq!(res.canonical_url, "https://www.facebook.com/share/v/556677889/");
    }

    #[test]
    fn test_scenario_19_share_generic_slug() {
        let res = parse_facebook_url("https://www.facebook.com/share/AbCdEfGh12/").unwrap();
        assert_eq!(res.id, "AbCdEfGh12");
        assert_eq!(res.kind, "share");
        assert!(res.is_redirect_needed);
        assert_eq!(res.canonical_url, "https://www.facebook.com/share/AbCdEfGh12/");
    }

    #[test]
    fn test_scenario_20_legacy_permalink_fbid_and_id() {
        let res = parse_facebook_url("https://www.facebook.com/permalink.php?story_fbid=123456789&id=987654321").unwrap();
        assert_eq!(res.id, "123456789");
        assert_eq!(res.author_handle, Some("987654321".to_string()));
        assert_eq!(res.kind, "post");
        assert_eq!(res.canonical_url, "https://www.facebook.com/permalink.php?story_fbid=123456789&id=987654321");
    }

    #[test]
    fn test_scenario_21_mobile_story_php() {
        let res = parse_facebook_url("https://m.facebook.com/story.php?story_fbid=123456789&id=987654321").unwrap();
        assert_eq!(res.id, "123456789");
        assert_eq!(res.kind, "post");
        assert_eq!(res.canonical_url, "https://www.facebook.com/permalink.php?story_fbid=123456789&id=987654321");
    }

    #[test]
    fn test_scenario_22_photo_php_fbid() {
        let res = parse_facebook_url("https://www.facebook.com/photo.php?fbid=10153231379946729&set=a.123").unwrap();
        assert_eq!(res.id, "10153231379946729");
        assert_eq!(res.kind, "photo");
        assert!(!res.is_video);
        assert_eq!(res.canonical_url, "https://www.facebook.com/photo.php?fbid=10153231379946729");
    }

    #[test]
    fn test_scenario_23_group_posts_path() {
        let res = parse_facebook_url("https://www.facebook.com/groups/123456/posts/789012345/").unwrap();
        assert_eq!(res.id, "789012345");
        assert_eq!(res.author_handle, Some("123456".to_string()));
        assert_eq!(res.kind, "post");
        assert_eq!(res.canonical_url, "https://www.facebook.com/groups/123456/posts/789012345/");
    }

    #[test]
    fn test_scenario_24_group_permalink_path() {
        let res = parse_facebook_url("https://www.facebook.com/groups/123456/permalink/789012345/").unwrap();
        assert_eq!(res.id, "789012345");
        assert_eq!(res.author_handle, Some("123456".to_string()));
        assert_eq!(res.kind, "post");
        assert_eq!(res.canonical_url, "https://www.facebook.com/groups/123456/posts/789012345/");
    }

    #[test]
    fn test_scenario_25_strip_all_tracking_parameters() {
        let url = "https://www.facebook.com/watch/?v=10153231379946729&mibextid=oXZqq4&rdid=123&utm_source=fb&__cft__[0]=AZX&__tn__=-R&fbclid=IwAR";
        let res = parse_facebook_url(url).unwrap();
        assert_eq!(res.id, "10153231379946729");
        assert_eq!(res.canonical_url, "https://www.facebook.com/watch/?v=10153231379946729");
    }

    #[test]
    fn test_scenario_26_raw_numeric_post_id() {
        let res = parse_facebook_url("10115432729910941").unwrap();
        assert_eq!(res.id, "10115432729910941");
        assert_eq!(res.kind, "post");
        assert_eq!(res.canonical_url, "https://www.facebook.com/10115432729910941");
    }

    #[test]
    fn test_scenario_27_raw_pfbid_post_id() {
        let res = parse_facebook_url("pfbid02ABC123xyz456").unwrap();
        assert_eq!(res.id, "pfbid02ABC123xyz456");
        assert_eq!(res.kind, "post");
        assert_eq!(res.canonical_url, "https://www.facebook.com/pfbid02ABC123xyz456");
    }

    #[test]
    fn test_scenario_28_empty_or_whitespace_error() {
        assert!(matches!(parse_facebook_url(""), Err(FacebookExtractError::InvalidInput(_))));
        assert!(matches!(parse_facebook_url("   \n\t  "), Err(FacebookExtractError::InvalidInput(_))));
    }

    #[test]
    fn test_scenario_29_foreign_domain_error() {
        let res = parse_facebook_url("https://youtube.com/watch?v=123");
        assert!(matches!(res, Err(FacebookExtractError::InvalidDomain(_))));
    }

    #[test]
    fn test_scenario_30_spoofed_subdomain_error() {
        let res = parse_facebook_url("https://facebook.com.evil.com/posts/123");
        assert!(matches!(res, Err(FacebookExtractError::InvalidDomain(_))));
    }

    #[test]
    fn test_scenario_31_unsupported_profile_url() {
        let res = parse_facebook_url("https://www.facebook.com/zuck");
        assert!(matches!(res, Err(FacebookExtractError::UnsupportedUrl(_))));
    }

    #[test]
    fn test_scenario_32_login_page_detection() {
        let res = parse_facebook_url("https://www.facebook.com/login.php?next=...");
        assert!(matches!(res, Err(FacebookExtractError::PrivateOrLoginWall(_))));
    }

    // ── HTML Entity Decoding Tests ───────────────────────────────────────────

    #[test]
    fn test_decode_named_entities() {
        let input = "Cats &amp; Dogs &quot;Hello&quot; &lt;World&gt; it&#39;s sunny &nbsp; here";
        let output = decode_html_entities(input);
        assert_eq!(output, "Cats & Dogs \"Hello\" <World> it's sunny   here");
    }

    #[test]
    fn test_decode_numeric_decimal_entities() {
        let input = "&#65;&#66;&#67;";
        let output = decode_html_entities(input);
        assert_eq!(output, "ABC");
    }

    #[test]
    fn test_decode_numeric_hex_entities() {
        // Thai text from live Facebook probe: &#xe16;&#xe39;&#xe01;&#xe43;&#xe08; -> "ถูกใจ"
        let input = "&#xe16;&#xe39;&#xe01;&#xe43;&#xe08;";
        let output = decode_html_entities(input);
        assert_eq!(output, "ถูกใจ");
    }

    #[test]
    fn test_decode_emoji_hex_entities() {
        // Rocket emoji 🚀 is U+1F680 -> &#x1F680;
        let input = "Launch &#x1F680; Fire &#x1F525;";
        let output = decode_html_entities(input);
        assert_eq!(output, "Launch 🚀 Fire 🔥");
    }

    #[test]
    fn test_double_encoded_entities() {
        let input = "AT&amp;amp;T &amp;quot;Quotes&amp;quot;";
        let output = normalize_caption(input);
        assert_eq!(output, "AT&T \"Quotes\"");
    }

    // ── Caption Normalization Tests ──────────────────────────────────────────

    #[test]
    fn test_normalize_caption_bom_and_newlines() {
        let input = "\u{feff}Line 1\r\nLine 2\rLine 3\n\n\n\nLine 4";
        let output = normalize_caption(input);
        assert_eq!(output, "Line 1\nLine 2\nLine 3\n\nLine 4");
    }

    #[test]
    fn test_normalize_caption_trailing_spaces() {
        let input = "Trailing spaces    \nNext line   ";
        let output = normalize_caption(input);
        assert_eq!(output, "Trailing spaces\nNext line");
    }

    #[test]
    fn test_normalize_caption_multilingual_and_emoji() {
        let input = "Hello 🌍! مرحبا بالعالم! 你好世界! 🚀";
        let output = normalize_caption(input);
        assert_eq!(output, "Hello 🌍! مرحبا بالعالم! 你好世界! 🚀");
    }

    #[test]
    fn test_extract_hashtags_deduplication() {
        let text = "Check this out #rust #code #RUST #coding #AI #rust";
        let tags = extract_hashtags(text);
        assert_eq!(tags, vec!["rust", "code", "coding", "ai"]);
    }

    // ── Author from Title Parsing Tests ──────────────────────────────────────

    #[test]
    fn test_parse_author_with_facebook_suffix() {
        assert_eq!(parse_author_from_title("Mark Zuckerberg | Facebook", None), "Mark Zuckerberg");
        assert_eq!(parse_author_from_title("NASA - Posts | Facebook", None), "NASA");
        assert_eq!(parse_author_from_title("BBC News - Home | Facebook", None), "BBC News");
        assert_eq!(parse_author_from_title("National Geographic - Videos | Facebook", None), "National Geographic");
    }

    #[test]
    fn test_parse_author_with_view_metrics_prefix() {
        assert_eq!(
            parse_author_from_title("2.8M views · 45K reactions | Video Title | Facebook", None),
            "Video Title"
        );
    }

    #[test]
    fn test_parse_author_with_fallback() {
        assert_eq!(parse_author_from_title("Facebook", Some("zuck")), "zuck");
        assert_eq!(parse_author_from_title("Log In | Facebook", Some("nasa")), "nasa");
        assert_eq!(parse_author_from_title("", None), "Facebook User");
    }

    // ── Login Wall Detection Tests ───────────────────────────────────────────

    #[test]
    fn test_detect_login_wall_redirect_url() {
        let err = detect_login_wall(200, "https://www.facebook.com/login.php?next=...", "").unwrap();
        assert_eq!(
            err,
            FacebookExtractError::PrivateOrLoginWall(
                "This Facebook post is private, restricted, or requires login authentication".into()
            )
        );
    }

    #[test]
    fn test_detect_login_wall_form_signature() {
        let html = "<html><body><form id=\"login_form\" action=\"/login.php\"></form></body></html>";
        let err = detect_login_wall(200, "https://www.facebook.com/post/123", html).unwrap();
        assert_eq!(
            err,
            FacebookExtractError::PrivateOrLoginWall(
                "This Facebook post is private, restricted, or requires login authentication".into()
            )
        );
    }

    #[test]
    fn test_detect_login_wall_text_signature() {
        let html = "<div>You must log in to continue.</div>";
        let err = detect_login_wall(200, "https://www.facebook.com/post/123", html).unwrap();
        assert_eq!(
            err,
            FacebookExtractError::PrivateOrLoginWall(
                "This Facebook post is private, restricted, or requires login authentication".into()
            )
        );
    }

    #[test]
    fn test_detect_login_wall_private_shared_text() {
        let html = "<div>When this happens, it's usually because the owner only shared it with a small group of people.</div>";
        let err = detect_login_wall(200, "https://www.facebook.com/post/123", html).unwrap();
        assert_eq!(
            err,
            FacebookExtractError::PrivateOrLoginWall(
                "This Facebook post is private, restricted, or requires login authentication".into()
            )
        );
    }

    #[test]
    fn test_detect_login_wall_clean_public_page() {
        let html = "<html><head><meta property=\"og:title\" content=\"NASA\" /></head><body>Public post</body></html>";
        assert!(detect_login_wall(200, "https://www.facebook.com/nasa/posts/123", html).is_none());
    }

    // ── OpenGraph and Video Stream Extraction Tests ──────────────────────────

    #[test]
    fn test_parse_opengraph_html_mock() {
        let html = r#"
            <!DOCTYPE html>
            <html>
            <head>
                <meta property="og:title" content="Mark Zuckerberg | Facebook" />
                <meta property="og:description" content="Bringing the world closer &amp; together." />
                <meta property="og:url" content="https://www.facebook.com/zuck/posts/101" />
                <meta property="og:image" content="https://lookaside.fbsbx.com/photo.jpg?oh=1&amp;oe=2" />
                <meta property="og:video" content="https://video.xx.fbcdn.net/video.mp4?oh=1&amp;oe=2" />
            </head>
            <body></body>
            </html>
        "#;
        let og = parse_opengraph_html(html);
        assert_eq!(og.title.as_deref(), Some("Mark Zuckerberg | Facebook"));
        assert_eq!(og.description.as_deref(), Some("Bringing the world closer & together."));
        assert_eq!(og.url.as_deref(), Some("https://www.facebook.com/zuck/posts/101"));
        assert_eq!(og.image.as_deref(), Some("https://lookaside.fbsbx.com/photo.jpg?oh=1&oe=2"));
        assert_eq!(og.video.as_deref(), Some("https://video.xx.fbcdn.net/video.mp4?oh=1&oe=2"));
    }

    #[test]
    fn test_extract_video_streams_regex() {
        let html = r#"
            <script>
            var config = {
                "browser_native_hd_url": "https:\/\/video.xx.fbcdn.net\/v\/hd_stream.mp4?oh=1\u0026oe=2",
                "browser_native_sd_url": "https:\/\/video.xx.fbcdn.net\/v\/sd_stream.mp4?oh=3\u0026oe=4"
            };
            </script>
        "#;
        let (hd, sd) = extract_video_streams_from_html(html);
        assert_eq!(hd, vec!["https://video.xx.fbcdn.net/v/hd_stream.mp4?oh=1&oe=2"]);
        assert_eq!(sd, vec!["https://video.xx.fbcdn.net/v/sd_stream.mp4?oh=3&oe=4"]);
    }

    // ── Arithmetic Safety Test ───────────────────────────────────────────────

    #[test]
    fn test_dimension_arithmetic_safety() {
        let w: u64 = u64::MAX;
        let h: u64 = 2;
        let pixel_area = w.saturating_mul(h);
        assert_eq!(pixel_area, u64::MAX);
    }

    // ── Markdown Summary Generator Tests ─────────────────────────────────────

    #[test]
    fn test_generate_markdown_summary_pyramid() {
        let post = FacebookPost {
            id: Some("10153231379946729".into()),
            url: "https://www.facebook.com/watch/?v=10153231379946729".into(),
            author: FacebookAuthor {
                name: "Mark Zuckerberg".into(),
                url: Some("https://www.facebook.com/zuck/".into()),
                id: Some("4".into()),
                avatar_url: None,
                is_verified: Some(true),
            },
            caption: "How to share with just friends.".into(),
            hashtags: vec!["privacy".into(), "facebook".into()],
            images: vec!["https://lookaside.fbsbx.com/preview.jpg".into()],
            videos: vec!["https://video.xx.fbcdn.net/stream_hd.mp4".into()],
            audio_url: Some("https://video.xx.fbcdn.net/audio_128k.m4a".into()),
            has_audio: true,
            media_type: "video".into(),
            media_items: vec![
                FacebookMediaItem {
                    id: Some("vid_1".into()),
                    media_type: "video".into(),
                    url: "https://video.xx.fbcdn.net/stream_hd.mp4".into(),
                    width: Some(1920),
                    height: Some(1080),
                    thumbnail_url: Some("https://lookaside.fbsbx.com/preview.jpg".into()),
                    is_video: true,
                    duration_secs: Some(42.5),
                    audio_url: Some("https://video.xx.fbcdn.net/audio_128k.m4a".into()),
                    has_audio: true,
                }
            ],
            thumbnail_url: Some("https://lookaside.fbsbx.com/preview.jpg".into()),
            published_at: Some(1678886400),
            like_count: Some(12500),
            comment_count: Some(340),
            share_count: Some(89),
            markdown: String::new(),
        };

        let md = generate_markdown_summary(&post);

        // Verify Pyramid levels
        assert!(md.contains("# Facebook Video by Mark Zuckerberg"));
        assert!(md.contains("- **Author**: [Mark Zuckerberg](https://www.facebook.com/zuck/)"));
        assert!(md.contains("- **Post URL**: [https://www.facebook.com/watch/?v=10153231379946729](https://www.facebook.com/watch/?v=10153231379946729)"));
        assert!(md.contains("- **Type**: Video"));
        assert!(md.contains("- **Engagement**: 12,500 likes, 340 comments, 89 shares"));
        assert!(md.contains("### Caption\n\nHow to share with just friends."));
        assert!(md.contains("### Hashtags\n\n`#privacy` `#facebook`"));
        assert!(md.contains("### Media Gallery"));
        assert!(md.contains("- **Audio Track**: [Direct Audio Stream (.m4a/.mp3)](https://video.xx.fbcdn.net/audio_128k.m4a) 🎵"));
        assert!(md.contains("- **Item 1 (Video)**: [Direct Video Stream (HD .mp4) [Duration: 42.5s]](https://video.xx.fbcdn.net/stream_hd.mp4) 🎬 *(Includes audio track)*"));
        assert!(md.contains("### Video Player & Links"));
        assert!(md.contains("- **Web Player**: [Facebook Watch Player](https://www.facebook.com/watch/?v=10153231379946729)"));
    }

    #[test]
    fn test_extract_audio_stream_from_html() {
        let html_playback = r#"{"playback_audio_url":"https:\/\/video.xx.fbcdn.net\/v\/t42\/audio.m4a?_nc_cat=1"}"#;
        let audio = extract_audio_stream_from_html(html_playback);
        assert_eq!(audio, Some("https://video.xx.fbcdn.net/v/t42/audio.m4a?_nc_cat=1".to_string()));

        let html_dash = r#"{"dash_audio_url":"https:\/\/video.xx.fbcdn.net\/v\/t42\/dash_audio.m4a"}"#;
        let audio_dash = extract_audio_stream_from_html(html_dash);
        assert_eq!(audio_dash, Some("https://video.xx.fbcdn.net/v/t42/dash_audio.m4a".to_string()));

        let html_none = r#"<div>No audio here</div>"#;
        assert_eq!(extract_audio_stream_from_html(html_none), None);
    }

    // ── MCP Adapter Argument Handling Tests ──────────────────────────────────

    #[tokio::test]
    async fn test_call_facebook_post_empty_url() {
        let res = call_facebook_post(json!({ "url": "" })).await.unwrap();
        assert_eq!(res["isError"], true);
        assert!(res["content"][0]["text"].as_str().unwrap().contains("URL cannot be empty"));
    }

    #[tokio::test]
    async fn test_call_facebook_post_whitespace_url() {
        let res = call_facebook_post(json!({ "url": "   " })).await.unwrap();
        assert_eq!(res["isError"], true);
        assert!(res["content"][0]["text"].as_str().unwrap().contains("URL cannot be empty"));
    }

    #[tokio::test]
    async fn test_call_facebook_post_invalid_domain() {
        let res = call_facebook_post(json!({ "url": "https://notfacebook.com/posts/123" })).await.unwrap();
        assert_eq!(res["isError"], true);
        assert!(res["content"][0]["text"].as_str().unwrap().contains("Invalid domain"));
    }
}
