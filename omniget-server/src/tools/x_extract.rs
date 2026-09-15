//! Twitter/X extraction tools (`x_post` and `x_thread`) using FxTwitter v2 and thread unrolling.

use omniget_core::core::tools::x::{
    fx,
    thread::{self, Thread},
    XPost,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct XArgs {
    pub url: Option<String>,
    pub id: Option<String>,
}

#[derive(Debug)]
pub enum XExtractError {
    InvalidInput(String),
    InvalidDomain(String),
    NotFound(String),
    UpstreamError(String),
    Timeout(String),
}

impl std::fmt::Display for XExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(s) => write!(f, "Invalid input: {}", s),
            Self::InvalidDomain(s) => write!(f, "Invalid domain: {}", s),
            Self::NotFound(s) => write!(f, "Not found: {}", s),
            Self::UpstreamError(s) => write!(f, "Upstream error: {}", s),
            Self::Timeout(s) => write!(f, "Request timed out: {}", s),
        }
    }
}

impl std::error::Error for XExtractError {}

/// Parses and validates an X/Twitter post status ID from a raw ID or URL.
pub fn parse_status_id(input: &str) -> Result<String, XExtractError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(XExtractError::InvalidInput("URL or status ID cannot be empty".into()));
    }

    // Pure numeric string check
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Ok(trimmed.to_string());
    }

    // URL parsing with auto-prefixing if scheme missing
    let parsed_url = match url::Url::parse(trimmed) {
        Ok(u) => u,
        Err(_) => {
            if !trimmed.contains("://") {
                url::Url::parse(&format!("https://{}", trimmed))
                    .map_err(|_| XExtractError::InvalidInput(format!("Malformed URL: {}", trimmed)))?
            } else {
                return Err(XExtractError::InvalidInput(format!("Malformed URL: {}", trimmed)));
            }
        }
    };

    // Domain validation
    let host = parsed_url.host_str().unwrap_or("").to_lowercase();
    let is_valid = host == "x.com"
        || host.ends_with(".x.com")
        || host == "twitter.com"
        || host.ends_with(".twitter.com")
        || host == "fxtwitter.com"
        || host == "vxtwitter.com"
        || host == "fixupx.com";

    if !is_valid {
        return Err(XExtractError::InvalidDomain(format!(
            "Domain '{}' is not a recognized X/Twitter domain",
            host
        )));
    }

    // Path pattern matching
    let path = parsed_url.path();
    for prefix in &["/status/", "/statuses/"] {
        if let Some(pos) = path.find(prefix) {
            let after = &path[pos + prefix.len()..];
            let segment = after.split('/').next().unwrap_or("");
            if segment.is_empty() {
                return Err(XExtractError::InvalidInput("Missing status ID in URL path".into()));
            }
            if !segment.chars().all(|c| c.is_ascii_digit()) {
                return Err(XExtractError::InvalidInput(format!(
                    "Non-numeric status ID '{}' in URL",
                    segment
                )));
            }
            return Ok(segment.to_string());
        }
    }

    Err(XExtractError::InvalidInput("Missing status ID in URL path".into()))
}

/// Extracts a single X/Twitter post by URL or status ID.
pub async fn extract_post(input: &str) -> Result<XPost, XExtractError> {
    let id = parse_status_id(input)?;
    let fetch_fut = fx::status(&id);

    match tokio::time::timeout(Duration::from_secs(15), fetch_fut).await {
        Ok(Ok(post)) => Ok(post),
        Ok(Err(e)) => {
            let msg = e.to_string();
            if msg.contains("nao encontrado") || msg.contains("privado") || msg.contains("indisponivel") {
                Err(XExtractError::NotFound(msg))
            } else {
                Err(XExtractError::UpstreamError(msg))
            }
        }
        Err(_) => Err(XExtractError::Timeout("FxTwitter request timed out after 15s".into())),
    }
}

/// Unrolls a full X/Twitter thread by URL or status ID.
pub async fn extract_thread(input: &str) -> Result<Thread, XExtractError> {
    let id = parse_status_id(input)?;
    let fetch_fut = thread::unroll(&id);

    match tokio::time::timeout(Duration::from_secs(15), fetch_fut).await {
        Ok(Ok(thread)) => Ok(thread),
        Ok(Err(e)) => {
            let msg = e.to_string();
            if msg.contains("nao encontrado") || msg.contains("privado") || msg.contains("indisponivel") {
                Err(XExtractError::NotFound(msg))
            } else {
                Err(XExtractError::UpstreamError(msg))
            }
        }
        Err(_) => Err(XExtractError::Timeout("Thread unrolling timed out after 15s".into())),
    }
}

/// JSON-RPC `tools/call` handler for `x_post`.
pub async fn call_x_post(arguments: Value) -> Result<Value, anyhow::Error> {
    let args: XArgs = serde_json::from_value(arguments)
        .map_err(|e| anyhow::anyhow!("Invalid arguments for x_post: {}", e))?;
    let input = args.url.as_deref().or(args.id.as_deref()).unwrap_or("");
    match extract_post(input).await {
        Ok(post) => {
            let serialized = serde_json::to_string_pretty(&post)?;
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": serialized
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
                        "text": e.to_string()
                    }
                ],
                "isError": true
            }))
        }
    }
}

/// JSON-RPC `tools/call` handler for `x_thread`.
pub async fn call_x_thread(arguments: Value) -> Result<Value, anyhow::Error> {
    let args: XArgs = serde_json::from_value(arguments)
        .map_err(|e| anyhow::anyhow!("Invalid arguments for x_thread: {}", e))?;
    let input = args.url.as_deref().or(args.id.as_deref()).unwrap_or("");
    match extract_thread(input).await {
        Ok(thread) => {
            let serialized = serde_json::to_string_pretty(&thread)?;
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": serialized
                    }
                ],
                "thread": thread,
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
