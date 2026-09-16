//! Universal extraction tools registry and dispatch router for omniget-server.

pub mod facebook_post;
pub mod instagram_post;
pub mod media_info;
pub mod pdf_text;
pub mod web_markdown;
pub mod x_extract;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// MCP Tool Definition schema matching JSON-RPC 2.0 `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Returns the 7 universal extraction tools provided by OmniGet Server.
pub fn list_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "x_post".to_string(),
            description: "Extract single X/Twitter post content, author details, metrics, and media via FxTwitter v2.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The X/Twitter post URL (e.g. https://x.com/user/status/123456789) or numeric status ID"
                    }
                },
                "required": ["url"]
            }),
        },
        ToolDefinition {
            name: "x_thread".to_string(),
            description: "Unroll full conversation thread by author returning focal post and sequential tweets.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The X/Twitter post URL in the thread (e.g. https://x.com/user/status/123456789) or numeric status ID"
                    }
                },
                "required": ["url"]
            }),
        },
        ToolDefinition {
            name: "web_to_markdown".to_string(),
            description: "Fetch a public webpage, strip junk/ads/scripts, and convert article content to clean Markdown with table formatting.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Public HTTP/HTTPS URL of the web page to scrape and convert"
                    }
                },
                "required": ["url"]
            }),
        },
        ToolDefinition {
            name: "pdf_text".to_string(),
            description: "Extract readable plain text and page counts from a local PDF file or remote HTTP/HTTPS URL using pure-Rust lopdf parser.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Local filesystem path or remote HTTP/HTTPS URL to the PDF document"
                    },
                    "pages": {
                        "type": "string",
                        "description": "Optional page range filter (e.g. '1-3', '1, 3, 5', 'all'). If omitted, extracts all pages."
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "media_info".to_string(),
            description: "Extract media metadata (title, author, platform, duration, available qualities, streaming format) for 1,800+ sites and direct media streams.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Public URL of the media item or direct streaming/audio/video URL"
                    }
                },
                "required": ["url"]
            }),
        },
        ToolDefinition {
            name: "instagram_post".to_string(),
            description: "Extract public Instagram post, Reel, or carousel metadata, author, caption, hashtags, images, and video streams.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Public Instagram post, Reel, or carousel URL (e.g. https://www.instagram.com/p/C_abc123/)"
                    }
                },
                "required": ["url"]
            }),
        },
        ToolDefinition {
            name: "facebook_post".to_string(),
            description: "Extract public Facebook post, Reel, or Watch video metadata, author, caption, images, and direct video streams.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Public Facebook post, Reel, or Watch video URL (e.g. https://www.facebook.com/page/posts/123456789)"
                    }
                },
                "required": ["url"]
            }),
        },
    ]
}

/// Dispatches a tool invocation by name and arguments.
///
/// Returns an MCP tool result containing a `content` array with text and optional structured fields.
pub async fn call_tool(name: &str, arguments: Value) -> Result<Value> {
    match name {
        "x_post" => x_extract::call_x_post(arguments).await,
        "x_thread" => x_extract::call_x_thread(arguments).await,
        "web_to_markdown" => web_markdown::call_web_to_markdown(arguments).await,
        "pdf_text" => pdf_text::call_pdf_text(arguments).await,
        "media_info" => media_info::call_media_info(arguments).await,
        "instagram_post" => instagram_post::call_instagram_post(arguments).await,
        "facebook_post" => facebook_post::call_facebook_post(arguments).await,
        unknown => Err(anyhow!("Unknown tool: {}", unknown)),
    }
}

/// Helper to handle JSON-RPC 2.0 requests for `tools/list` and `tools/call`.
pub async fn handle_json_rpc(payload: Value) -> Value {
    let id = payload.get("id").cloned().unwrap_or(json!(1));
    let method = payload.get("method").and_then(Value::as_str).unwrap_or("");

    match method {
        "tools/list" => {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": list_tools()
                }
            })
        }
        "tools/call" => {
            let params = payload.get("params").cloned().unwrap_or(json!({}));
            let tool_name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            match call_tool(tool_name, arguments).await {
                Ok(res) => {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": res
                    })
                }
                Err(e) => {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [
                                {
                                    "type": "text",
                                    "text": e.to_string()
                                }
                            ],
                            "isError": true
                        }
                    })
                }
            }
        }
        _ => {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("Method '{}' not found", method)
                }
            })
        }
    }
}
