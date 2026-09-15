//! Streamable HTTP MCP transport and JSON-RPC 2.0 dispatcher for OmniGet Server.
//!
//! Implements Model Context Protocol (MCP) Streamable HTTP transport:
//! - Endpoint: `POST /mcp`
//! - Protocol Version: `2024-11-05`
//! - JSON-RPC 2.0 lifecycle: `initialize`, `ping`, `notifications/initialized`, `tools/list`, `tools/call`
//! - Error codes: `-32700` (Parse error), `-32600` (Invalid Request), `-32601` (Method not found), `-32602` (Invalid params)
//! - Session tracking via `Mcp-Session-Id` header echo

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{json, Value};
use tracing::debug;

/// MCP protocol version supported by this server.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Server name reported in `initialize` handshake.
pub const SERVER_NAME: &str = "omniget-server";

/// Header name for MCP session tracking.
pub const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// Maximum request body size (16 MB).
pub const MAX_BODY_SIZE_BYTES: usize = 16 * 1024 * 1024;

// Standard JSON-RPC 2.0 error codes
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
#[allow(dead_code)]
pub const INTERNAL_ERROR: i32 = -32603;

/// Constructs a successful JSON-RPC 2.0 response object.
pub fn make_result_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

/// Constructs a JSON-RPC 2.0 error response object.
pub fn make_error_response(id: Value, code: i32, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

/// Helper to attach `mcp-session-id` header to outgoing response if present in request.
fn attach_session_header(mut response: Response, session_id: Option<&str>) -> Response {
    if let Some(sid) = session_id {
        if let Ok(val) = HeaderValue::from_str(sid) {
            response.headers_mut().insert(
                HeaderName::from_static(MCP_SESSION_ID_HEADER),
                val,
            );
        }
    }
    response
}

/// Handler for `POST /mcp` Streamable HTTP transport.
pub async fn mcp_post_handler(
    headers: HeaderMap,
    body: Body,
) -> Response {
    let session_id_opt = headers
        .get(MCP_SESSION_ID_HEADER)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());

    // 1. Ingest body bytes with 16MB limit
    let body_bytes = match axum::body::to_bytes(body, MAX_BODY_SIZE_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let resp = (
                StatusCode::PAYLOAD_TOO_LARGE,
                [(header::CONTENT_TYPE, "application/json")],
                make_error_response(Value::Null, INVALID_REQUEST, "Payload too large").to_string(),
            )
                .into_response();
            return attach_session_header(resp, session_id_opt.as_deref());
        }
    };

    // 2. Handle empty body (for auth/liveness verification without payload)
    if body_bytes.is_empty() {
        let resp = (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            json!({
                "jsonrpc": "2.0",
                "result": { "status": "ready" }
            })
            .to_string(),
        )
            .into_response();
        return attach_session_header(resp, session_id_opt.as_deref());
    }

    // 3. Parse JSON syntax (Parse error -32700 -> HTTP 400 Bad Request)
    let json_val: Value = match serde_json::from_slice(&body_bytes) {
        Ok(val) => val,
        Err(err) => {
            let resp = (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/json")],
                make_error_response(
                    Value::Null,
                    PARSE_ERROR,
                    format!("Parse error: {err}"),
                )
                .to_string(),
            )
                .into_response();
            return attach_session_header(resp, session_id_opt.as_deref());
        }
    };

    // 4. Dispatch JSON-RPC (single or batch)
    let response = if json_val.is_array() {
        handle_batch_request(json_val).await
    } else {
        match dispatch_json_rpc(json_val).await {
            Some(res_val) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                res_val.to_string(),
            )
                .into_response(),
            None => {
                // Notification: return HTTP 202 Accepted with empty body
                StatusCode::ACCEPTED.into_response()
            }
        }
    };

    attach_session_header(response, session_id_opt.as_deref())
}

/// Dispatches a single JSON-RPC 2.0 request or notification.
///
/// Returns `None` for notifications (which produce no JSON-RPC response),
/// and `Some(Value)` for requests or errors.
pub async fn dispatch_json_rpc(payload: Value) -> Option<Value> {
    let obj = match payload.as_object() {
        Some(o) => o,
        None => {
            return Some(make_error_response(
                Value::Null,
                INVALID_REQUEST,
                "Invalid Request: payload must be a JSON object",
            ));
        }
    };

    // Verify "jsonrpc": "2.0"
    match obj.get("jsonrpc").and_then(Value::as_str) {
        Some("2.0") => {}
        _ => {
            let id = obj.get("id").cloned().unwrap_or(Value::Null);
            return Some(make_error_response(
                id,
                INVALID_REQUEST,
                "Invalid Request: missing or invalid 'jsonrpc' version (must be '2.0')",
            ));
        }
    }

    // Verify "method"
    let method = match obj.get("method").and_then(Value::as_str) {
        Some(m) => m,
        None => {
            let id = obj.get("id").cloned().unwrap_or(Value::Null);
            return Some(make_error_response(
                id,
                INVALID_REQUEST,
                "Invalid Request: missing or invalid 'method' field",
            ));
        }
    };

    // Notifications: missing 'id' key or notification method prefixes
    let is_notification = !obj.contains_key("id")
        || method == "notifications/initialized"
        || method.starts_with("notifications/");

    if is_notification {
        debug!(method, "Processed JSON-RPC notification");
        return None;
    }

    // Preserve ID type (string, integer, or null)
    let id = obj.get("id").cloned().unwrap_or(Value::Null);
    let params = obj.get("params").cloned().unwrap_or(Value::Null);

    let response = match method {
        "initialize" => handle_initialize(id, params),
        "ping" => make_result_response(id, json!({})),
        "tools/list" => make_result_response(id, json!({ "tools": crate::tools::list_tools() })),
        "tools/call" => handle_tools_call(id, params).await,
        _ => make_error_response(
            id,
            METHOD_NOT_FOUND,
            format!("Method '{}' not found", method),
        ),
    };

    Some(response)
}

/// Handles `initialize` handshake.
fn handle_initialize(id: Value, _params: Value) -> Value {
    make_result_response(
        id,
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
}

/// Handles `tools/call` invocation.
async fn handle_tools_call(id: Value, params: Value) -> Value {
    let params_obj = match params.as_object() {
        Some(o) => o,
        None => {
            return make_result_response(
                id,
                json!({
                    "content": [
                        {
                            "type": "text",
                            "text": "Invalid params: expected object for tools/call"
                        }
                    ],
                    "isError": true,
                    "code": INVALID_PARAMS
                }),
            );
        }
    };

    let tool_name = match params_obj.get("name").and_then(Value::as_str) {
        Some(name) => name,
        None => {
            return make_result_response(
                id,
                json!({
                    "content": [
                        {
                            "type": "text",
                            "text": "Invalid params: 'name' is required for tools/call"
                        }
                    ],
                    "isError": true,
                    "code": INVALID_PARAMS
                }),
            );
        }
    };

    let arguments = params_obj.get("arguments").cloned().unwrap_or(json!({}));

    match crate::tools::call_tool(tool_name, arguments).await {
        Ok(result_val) => make_result_response(id, result_val),
        Err(err) => make_result_response(
            id,
            json!({
                "content": [
                    {
                        "type": "text",
                        "text": err.to_string()
                    }
                ],
                "isError": true
            }),
        ),
    }
}

/// Handles batch JSON-RPC requests (JSON arrays).
async fn handle_batch_request(val: Value) -> Response {
    let array = match val.as_array() {
        Some(arr) => arr,
        None => {
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                make_error_response(Value::Null, INVALID_REQUEST, "Invalid batch request").to_string(),
            )
                .into_response();
        }
    };

    if array.is_empty() {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            make_error_response(Value::Null, INVALID_REQUEST, "Invalid Request: empty batch array").to_string(),
        )
            .into_response();
    }

    let mut responses = Vec::new();
    for item in array {
        if let Some(res) = dispatch_json_rpc(item.clone()).await {
            responses.push(res);
        }
    }

    if responses.is_empty() {
        StatusCode::ACCEPTED.into_response()
    } else {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            Value::Array(responses).to_string(),
        )
            .into_response()
    }
}

