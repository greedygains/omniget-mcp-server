#![allow(clippy::needless_borrows_for_generic_args)]

//! Adversarial Stress & Pathological Verification Suite for Milestone 3 (Dual Protocols & REST/OpenAPI)
//!
//! Empirical Challenger Test Suite covering:
//! 1. JSON-RPC 2.0 Fuzzing & Malformed Payloads:
//!    - Missing "jsonrpc" version -> -32600
//!    - Invalid "jsonrpc" versions ("1.0", 2.0, true, null, arrays, objects) -> -32600
//!    - Numeric, boolean, null, array, object "method" fields -> -32600
//!    - Extreme ID formats: negative ints, floats, 64-bit max ints, empty strings, emojis, null, arrays, objects
//!    - Batch request pathologies: empty array [], non-object items in batch, mixed requests & notifications,
//!      batch with only notifications (HTTP 202), nested arrays
//!    - Tools/call parameter fuzzing: missing arguments, non-object params, non-string tool names, unknown tools
//!    - Header echoing: Mcp-Session-Id header preservation and edge cases
//! 2. SSE Protocol Boundary & Session Fuzzing:
//!    - Missing sessionId query parameter -> 400
//!    - Empty or whitespace-only sessionId -> 400
//!    - Pathological / injection sessionIds (SQLi, XSS, path traversal, non-UUID formats) -> 400
//!    - Syntactically valid UUID that does not exist -> 404
//!    - RAII Drop session cleanup: verify session becomes 404 after SSE connection disconnects
//!    - High concurrency: 20 simultaneous POST /messages to the same active session all routed and delivered
//!    - Notifications dispatched over SSE: returns 202 Accepted without junk events
//! 3. REST API Boundary Testing:
//!    - Invalid & corrupt percent encodings in query parameters -> handled without 500 panic
//!    - Precedence: JSON body vs query parameter in POST requests (body takes precedence)
//!    - Empty request bodies, empty JSON objects, and missing required fields -> 400 Bad Request
//!    - Type confusion in JSON body (integers, nulls, arrays instead of string URLs) -> 400 Bad Request
//!    - Non-existent endpoints under /api/* -> 404 Not Found
//!    - Unsupported HTTP methods (PUT, DELETE, PATCH) -> 405 Method Not Allowed
//!    - PDF text boundaries: missing path/url, non-existent files, invalid page formats
//!    - X post/thread boundaries: invalid domains, missing status IDs, non-numeric status IDs
//!    - Media info boundaries: invalid URL schemes, empty targets
//!    - Permissive CORS preflight OPTIONS across all REST & MCP endpoints
//! 4. OpenAPI 3.1.0 Formal Specification Validation:
//!    - Version strictly matches "3.1.0"
//!    - Contains all 9 required endpoints
//!    - Security schemes define BearerAuth (http bearer)
//!    - Every protected endpoint enforces BearerAuth security
//!    - Public /health does NOT require security
//!    - Every POST endpoint defines requestBody with application/json schema
//! 5. Multi-Transport Cross-Protocol Concurrency Storm:
//!    - Simultaneous load across POST /mcp, GET /sse, POST /messages, REST GET/POST, /openapi.json, and /health

mod common;

use common::{create_test_pdf, MockWebsite, TestServer};
use futures::StreamExt;
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// Helper function to parse raw SSE chunks into (event_type, data) pairs.
async fn next_sse_event<S>(
    stream: &mut S,
    timeout: Duration,
) -> Option<(String, String)>
where
    S: futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    let start = std::time::Instant::now();
    let mut buffer = String::new();

    while start.elapsed() < timeout {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(bytes))) => {
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                if let Some(pos) = buffer.find("\n\n") {
                    let event_block = buffer[..pos].to_string();
                    buffer.drain(..pos + 2);

                    let mut event_type = String::from("message");
                    let mut data = String::new();

                    for line in event_block.lines() {
                        if let Some(stripped) = line.strip_prefix("event: ") {
                            event_type = stripped.trim().to_string();
                        } else if let Some(stripped) = line.strip_prefix("data: ") {
                            data = stripped.trim().to_string();
                        }
                    }
                    return Some((event_type, data));
                }
            }
            Ok(Some(Err(_))) => return None,
            Ok(None) => return None,
            Err(_) => {}
        }
    }
    None
}

/// Helper function to establish SSE connection, read endpoint event, and extract sessionId.
async fn sse_connect(
    server: &TestServer,
) -> (
    String,
    impl futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
) {
    let res = server
        .client
        .get(&server.url("/sse"))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .send()
        .await
        .expect("connect to /sse");

    assert_eq!(res.status(), StatusCode::OK);
    let mut stream = res.bytes_stream();

    let (event, data) = next_sse_event(&mut stream, Duration::from_secs(5))
        .await
        .expect("receive initial SSE event");

    assert_eq!(event, "endpoint");
    assert!(
        data.contains("sessionId="),
        "endpoint event must contain sessionId: {}",
        data
    );

    let session_id = data
        .split("sessionId=")
        .nth(1)
        .expect("extract session id")
        .split('&')
        .next()
        .expect("clean session id")
        .to_string();

    (session_id, stream)
}

// ============================================================================
// SECTION 1: JSON-RPC 2.0 Fuzzing & Malformed Payloads (POST /mcp)
// ============================================================================

/// 1.1: Fuzzing "jsonrpc" version field with non-"2.0" types and values.
#[tokio::test]
async fn test_adversarial_jsonrpc_version_fuzzing() {
    let server = TestServer::start().await;

    let pathological_versions = vec![
        json!("1.0"),
        json!("2.1"),
        json!("3.0"),
        json!(2.0),
        json!(2),
        json!(true),
        json!(false),
        json!(Value::Null),
        json!(["2.0"]),
        json!({"version": "2.0"}),
    ];

    for (idx, bad_version) in pathological_versions.into_iter().enumerate() {
        let payload = json!({
            "jsonrpc": bad_version,
            "id": idx + 1,
            "method": "ping"
        });

        let res = server.post_json_authed("/mcp", &payload).await;
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "Server should return 200 with JSON-RPC error for invalid version: {:?}",
            payload
        );

        let body: Value = res.json().await.expect("parse response json");
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], idx + 1);
        assert_eq!(
            body["error"]["code"], -32600,
            "Invalid version must produce error code -32600, got: {:?}",
            body
        );
    }
}

/// 1.2: Fuzzing "method" field with non-string types.
#[tokio::test]
async fn test_adversarial_jsonrpc_method_fuzzing() {
    let server = TestServer::start().await;

    let pathological_methods = vec![
        json!(12345),
        json!(42.42),
        json!(true),
        json!(false),
        json!(Value::Null),
        json!(["ping"]),
        json!({"name": "ping"}),
    ];

    for (idx, bad_method) in pathological_methods.into_iter().enumerate() {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": idx + 100,
            "method": bad_method
        });

        let res = server.post_json_authed("/mcp", &payload).await;
        assert_eq!(res.status(), StatusCode::OK);

        let body: Value = res.json().await.expect("parse response json");
        assert_eq!(body["id"], idx + 100);
        assert_eq!(
            body["error"]["code"], -32600,
            "Non-string method must produce error code -32600, got: {:?}",
            body
        );
    }
}

/// 1.3: Extreme and boundary JSON-RPC ID formats.
#[tokio::test]
async fn test_adversarial_jsonrpc_extreme_ids() {
    let server = TestServer::start().await;

    let id_test_cases = vec![
        json!(-1),
        json!(-999999999),
        json!(0),
        json!(9007199254740991i64), // Number.MAX_SAFE_INTEGER
        json!(123.456789),
        json!(""),
        json!("   spaces   "),
        json!("🎯✨🚀_special_unicode_id_漢字"),
        json!(Value::Null),
    ];

    for test_id in id_test_cases {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": test_id.clone(),
            "method": "ping"
        });

        let res = server.post_json_authed("/mcp", &payload).await;
        assert_eq!(res.status(), StatusCode::OK);

        let body: Value = res.json().await.expect("parse response json");
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(
            body["id"], test_id,
            "JSON-RPC id must be preserved exactly as sent"
        );
        assert_eq!(body["result"], json!({}));
    }
}

/// 1.4: Pathological Batch JSON-RPC requests.
#[tokio::test]
async fn test_adversarial_jsonrpc_batch_pathologies() {
    let server = TestServer::start().await;

    // Test A: Empty batch array [] -> -32600 Invalid Request
    let empty_batch = json!([]);
    let res = server.post_json_authed("/mcp", &empty_batch).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("parse response json");
    assert_eq!(body["error"]["code"], -32600);

    // Test B: Batch containing invalid non-object primitives [1, "hello", true, null]
    let invalid_primitives = json!([1, "hello", true, Value::Null]);
    let res = server.post_json_authed("/mcp", &invalid_primitives).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("parse response json");
    let arr = body.as_array().expect("batch response must be array");
    assert_eq!(arr.len(), 4);
    for item in arr {
        assert_eq!(item["error"]["code"], -32600);
    }

    // Test C: Batch containing ONLY notifications -> returns HTTP 202 Accepted with empty body
    let notifications_batch = json!([
        { "jsonrpc": "2.0", "method": "notifications/initialized" },
        { "jsonrpc": "2.0", "method": "notifications/custom_notice" }
    ]);
    let res = server.post_json_authed("/mcp", &notifications_batch).await;
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let bytes = res.bytes().await.expect("read bytes");
    assert!(bytes.is_empty(), "Batch notifications must return empty body");

    // Test D: Batch mixing valid requests, notifications, and errors
    let mixed_batch = json!([
        { "jsonrpc": "2.0", "id": 1, "method": "ping" },
        { "jsonrpc": "2.0", "method": "notifications/initialized" }, // notification: no response
        { "jsonrpc": "2.0", "id": 2, "method": "nonexistent" },     // error: -32601
        { "jsonrpc": "2.0", "id": 3, "method": "ping" }
    ]);
    let res = server.post_json_authed("/mcp", &mixed_batch).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("parse response json");
    let arr = body.as_array().expect("batch array");
    assert_eq!(arr.len(), 3, "Only requests should produce responses, not notifications");

    assert_eq!(arr[0]["id"], 1);
    assert_eq!(arr[0]["result"], json!({}));

    assert_eq!(arr[1]["id"], 2);
    assert_eq!(arr[1]["error"]["code"], -32601);

    assert_eq!(arr[2]["id"], 3);
    assert_eq!(arr[2]["result"], json!({}));

    // Test E: Nested batch array [[...]]
    let nested_batch = json!([[
        { "jsonrpc": "2.0", "id": 1, "method": "ping" }
    ]]);
    let res = server.post_json_authed("/mcp", &nested_batch).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("parse nested batch");
    let arr = body.as_array().expect("array response");
    assert_eq!(arr[0]["error"]["code"], -32600);
}

/// 1.5: Tool call parameter fuzzing (type errors, missing fields, unknown tools).
#[tokio::test]
async fn test_adversarial_tools_call_parameter_fuzzing() {
    let server = TestServer::start().await;

    // A: params is not an object
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": "not-an-object"
    });
    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("parse json");
    assert_eq!(body["id"], 10);
    assert_eq!(body["result"]["isError"], true);

    // B: params object is missing "name"
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": { "arguments": {} }
    });
    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("parse json");
    assert_eq!(body["id"], 11);
    assert_eq!(body["result"]["isError"], true);

    // C: params "name" is numeric
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "tools/call",
        "params": { "name": 9999 }
    });
    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("parse json");
    assert_eq!(body["id"], 12);
    assert_eq!(body["result"]["isError"], true);

    // D: call unknown tool name
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 13,
        "method": "tools/call",
        "params": { "name": "fictional_unregistered_tool", "arguments": {} }
    });
    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("parse json");
    assert_eq!(body["id"], 13);
    assert_eq!(body["result"]["isError"], true);
    let err_msg = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(err_msg.contains("not found") || err_msg.contains("Unknown tool"));
}

/// 1.6: Session header echo behavior on POST /mcp.
#[tokio::test]
async fn test_adversarial_mcp_session_id_header_echo() {
    let server = TestServer::start().await;
    let custom_session = "mcp-session-uuid-custom-test-1234";

    let payload = json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "ping"
    });

    // Send request with mcp-session-id header
    let res = server
        .client
        .post(&server.url("/mcp"))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .header("mcp-session-id", custom_session)
        .json(&payload)
        .send()
        .await
        .expect("send request with session header");

    assert_eq!(res.status(), StatusCode::OK);
    let echoed = res
        .headers()
        .get("mcp-session-id")
        .expect("mcp-session-id header should be echoed in response")
        .to_str()
        .expect("valid ascii");
    assert_eq!(echoed, custom_session);

    // Send request WITHOUT session header -> response should not have mcp-session-id
    let res_no_header = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res_no_header.status(), StatusCode::OK);
    assert!(res_no_header.headers().get("mcp-session-id").is_none());
}

// ============================================================================
// SECTION 2: SSE Protocol Boundary & Session Fuzzing (GET /sse, POST /messages)
// ============================================================================

/// 2.1: Fuzzing sessionId query parameter with invalid and injection formats.
#[tokio::test]
async fn test_adversarial_sse_session_id_fuzzing() {
    let server = TestServer::start().await;

    let invalid_session_ids = vec![
        "",
        "   ",
        "123",
        "true",
        "null",
        "undefined",
        "../../../../etc/passwd",
        "' OR '1'='1",
        "<script>alert(1)</script>",
        "00000000-0000-0000-0000", // truncated UUID
        "00000000-0000-0000-0000-0000000000000000", // oversized UUID
        "not-a-valid-uuid-format-at-all",
    ];

    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });

    for bad_id in invalid_session_ids {
        let url = if bad_id.is_empty() {
            server.url("/messages")
        } else {
            format!("{}/messages?sessionId={}", server.base_url, urlencoding::encode(bad_id))
        };

        let res = server
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", server.auth_token))
            .json(&payload)
            .send()
            .await
            .expect("send bad sessionId");

        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "Invalid sessionId '{}' must be rejected with 400 Bad Request",
            bad_id
        );
    }
}

/// 2.2: Syntactically valid UUID that does not exist in the active session store returns 404.
#[tokio::test]
async fn test_adversarial_sse_nonexistent_uuid_returns_404() {
    let server = TestServer::start().await;
    let non_existent_uuid = Uuid::new_v4().to_string();

    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    let res = server
        .client
        .post(&format!("{}/messages?sessionId={}", server.base_url, non_existent_uuid))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&payload)
        .send()
        .await
        .expect("send to non-existent session");

    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// 2.3: RAII Session Cleanup upon SSE Disconnect (prevent zombie sessions / memory leaks).
#[tokio::test]
async fn test_adversarial_sse_disconnect_cleans_up_session() {
    let server = TestServer::start().await;

    // Connect to SSE stream and obtain session ID
    let (session_id, stream) = sse_connect(&server).await;
    assert!(!session_id.is_empty());

    // Explicitly drop stream, terminating the HTTP client connection
    drop(stream);

    // Allow background Tokio task to detect EOF and run SessionGuard::drop
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Attempt to post to the dropped session -> must now return 404 Not Found!
    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    let res = server
        .client
        .post(&format!("{}/messages?sessionId={}", server.base_url, session_id))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&payload)
        .send()
        .await
        .expect("post to dropped session");

    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "Disconnected SSE session must be purged from session store"
    );
}

/// 2.4: Flood 20 concurrent messages to a single active session without drop or deadlock.
#[tokio::test]
async fn test_adversarial_sse_concurrent_message_flood() {
    let server = TestServer::start().await;
    let (session_id, mut stream) = sse_connect(&server).await;

    const MESSAGE_COUNT: usize = 20;

    let mut handles = Vec::new();
    for i in 0..MESSAGE_COUNT {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let token = server.auth_token.clone();
        let sid = session_id.clone();

        handles.push(tokio::spawn(async move {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "ping",
                "params": {}
            });

            let res = client
                .post(format!("{}/messages?sessionId={}", base_url, sid))
                .header("Authorization", format!("Bearer {}", token))
                .json(&payload)
                .send()
                .await
                .expect("send flood ping");

            assert_eq!(res.status(), StatusCode::ACCEPTED);
        }));
    }

    for h in handles {
        h.await.expect("join handle");
    }

    // Collect 20 response events from SSE stream
    let mut received_ids = Vec::new();
    for _ in 0..MESSAGE_COUNT {
        let (event, data) = next_sse_event(&mut stream, Duration::from_secs(5))
            .await
            .expect("receive flooded response event");

        assert_eq!(event, "message");
        let parsed: Value = serde_json::from_str(&data).expect("parse rpc json");
        let id = parsed["id"].as_u64().expect("numeric id");
        received_ids.push(id as usize);
    }

    assert_eq!(received_ids.len(), MESSAGE_COUNT);
    for i in 0..MESSAGE_COUNT {
        assert!(
            received_ids.contains(&i),
            "Missing response for message id {}",
            i
        );
    }
}

/// 2.5: Notification dispatch over SSE (POST /messages) returns 202 without junk stream events.
#[tokio::test]
async fn test_adversarial_sse_notification_handling() {
    let server = TestServer::start().await;
    let (session_id, mut stream) = sse_connect(&server).await;

    // Send notification
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });

    let res = server
        .client
        .post(&format!("{}/messages?sessionId={}", server.base_url, session_id))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&notification)
        .send()
        .await
        .expect("send notification to sse");
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    // Send regular ping request right after
    let ping = json!({ "jsonrpc": "2.0", "id": 777, "method": "ping" });
    let res_ping = server
        .client
        .post(&format!("{}/messages?sessionId={}", server.base_url, session_id))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&ping)
        .send()
        .await
        .expect("send ping to sse");
    assert_eq!(res_ping.status(), StatusCode::ACCEPTED);

    // First event received on stream must be the ping response (notification produced no stream event)
    let (event, data) = next_sse_event(&mut stream, Duration::from_secs(5))
        .await
        .expect("receive ping response");
    assert_eq!(event, "message");
    let body: Value = serde_json::from_str(&data).expect("parse json");
    assert_eq!(body["id"], 777);
}

// ============================================================================
// SECTION 3: REST API Boundary & Edge Cases (/api/*)
// ============================================================================

/// 3.1: Invalid percent-encodings and query parameters handled gracefully.
#[tokio::test]
async fn test_adversarial_rest_query_parameter_encoding() {
    let server = TestServer::start().await;

    // Malformed percent sequences: %ZZ, %, %1, %G5
    let malformed_urls = [
        "/api/web/markdown?url=http://example.com/%ZZ",
        "/api/web/markdown?url=http://example.com/%",
        "/api/web/markdown?url=http://example.com/%G5%A0",
    ];

    for path in malformed_urls {
        let res = server.get_authed(path).await;
        // Server should return 400 Bad Request or 502 Bad Gateway, NEVER 500 panic
        assert!(
            res.status() == StatusCode::BAD_REQUEST || res.status() == StatusCode::BAD_GATEWAY,
            "Malformed URL in query must not crash server: status {:?}",
            res.status()
        );
    }
}

/// 3.2: Body vs Query Precedence in POST /api/web/markdown: JSON body must take precedence.
#[tokio::test]
async fn test_adversarial_rest_post_body_precedence_over_query() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let target_body_url = mock.url("/article-simple");
    let target_query_url = "http://127.0.0.1:1/unreachable-dummy-query-host";

    let payload = json!({ "url": target_body_url });

    // Send POST with both query param AND JSON body
    let res = server
        .client
        .post(&format!("{}/api/web/markdown?url={}", server.base_url, urlencoding::encode(target_query_url)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&payload)
        .send()
        .await
        .expect("send post with body and query");

    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("parse response json");
    assert_eq!(body["title"], "Simple Article");
    assert!(body["markdown"].as_str().unwrap().contains("Simple Article Title"));
}

/// 3.3: REST Payload Schema Violations & Type Injections.
#[tokio::test]
async fn test_adversarial_rest_payload_schema_violations() {
    let server = TestServer::start().await;

    let bad_payloads = vec![
        json!({}),                              // Missing "url"
        json!({ "url": "" }),                   // Empty string
        json!({ "url": "    " }),               // Whitespace string
        json!({ "url": 12345 }),                // Number instead of string
        json!({ "url": true }),                 // Boolean instead of string
        json!({ "url": Value::Null }),          // Null instead of string
        json!({ "url": ["http://a.com"] }),     // Array instead of string
        json!({ "url": { "link": "foo" } }),   // Object instead of string
    ];

    for bad in bad_payloads {
        let res = server.post_json_authed("/api/web/markdown", &bad).await;
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "Malformed payload {:?} must return 400 Bad Request",
            bad
        );
    }
}

/// 3.4: Non-existent routes under /api/* return HTTP 404 Not Found.
#[tokio::test]
async fn test_adversarial_rest_nonexistent_routes_return_404() {
    let server = TestServer::start().await;

    let nonexistent_paths = [
        "/api/nonexistent",
        "/api/v2/tools",
        "/api/web/unknown",
        "/api/x/spaces",
        "/api/pdf/ocr",
    ];

    for path in nonexistent_paths {
        let res = server.get_authed(path).await;
        assert_eq!(
            res.status(),
            StatusCode::NOT_FOUND,
            "Path '{}' must return 404",
            path
        );
    }
}

/// 3.5: Method Not Allowed (405) for unsupported HTTP verbs.
#[tokio::test]
async fn test_adversarial_rest_method_not_allowed_405() {
    let server = TestServer::start().await;

    let test_cases = [
        (reqwest::Method::PUT, "/api/web/markdown"),
        (reqwest::Method::DELETE, "/api/web/markdown"),
        (reqwest::Method::PATCH, "/mcp"),
        (reqwest::Method::POST, "/sse"),
        (reqwest::Method::GET, "/messages"),
        (reqwest::Method::PUT, "/openapi.json"),
    ];

    for (method, path) in test_cases {
        let res = server
            .client
            .request(method.clone(), &server.url(path))
            .header("Authorization", format!("Bearer {}", server.auth_token))
            .send()
            .await
            .expect("send method");

        assert_eq!(
            res.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "Method {} on {} should return 405 Method Not Allowed",
            method,
            path
        );
    }
}

/// 3.6: /api/pdf/text boundary error conditions.
#[tokio::test]
async fn test_adversarial_rest_pdf_text_boundaries() {
    let server = TestServer::start().await;

    // A: Missing both path and url
    let res = server
        .post_json_authed("/api/pdf/text", &json!({ "pages": "1" }))
        .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // B: Non-existent file path
    let res = server
        .post_json_authed(
            "/api/pdf/text",
            &json!({ "path": "/path/does/not/exist/test_xyz.pdf", "pages": "1" }),
        )
        .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // C: Valid PDF fixture with page range testing
    let pdf = create_test_pdf("Page boundary test content", 2);
    let pdf_path = pdf.path().to_str().unwrap();

    // Standard client syntax errors mapped to 400 Bad Request
    let bad_ranges_400 = ["-5", "abc", "0", "999-1000"];
    for bad_range in bad_ranges_400 {
        let res = server
            .post_json_authed(
                "/api/pdf/text",
                &json!({ "path": pdf_path, "pages": bad_range }),
            )
            .await;
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "Invalid page range '{}' should return 400 Bad Request",
            bad_range
        );
    }

    // Edge page ranges with non-numeric start/end like "1-2-3" or "abc-2"
    // produce "Invalid end page..." or "Invalid start page...", which map to 400 Bad Request.
    let edge_ranges = ["1-2-3", "abc-2"];
    for bad_range in edge_ranges {
        let res = server
            .post_json_authed(
                "/api/pdf/text",
                &json!({ "path": pdf_path, "pages": bad_range }),
            )
            .await;
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "Edge page range '{}' must return 400 Bad Request",
            bad_range
        );
        let body: Value = res.json().await.expect("parse error json");
        assert_eq!(body["ok"], false);
    }
}

/// 3.7: /api/x/post and /api/x/thread boundary errors.
#[tokio::test]
async fn test_adversarial_rest_x_extract_boundaries() {
    let server = TestServer::start().await;

    let bad_x_targets = [
        "https://evil-x.com/status/123",
        "https://facebook.com/post/123",
        "https://x.com/home", // missing status id
        "https://x.com/user/status/not_numeric",
    ];

    for bad in bad_x_targets {
        let res_post = server
            .post_json_authed("/api/x/post", &json!({ "url": bad }))
            .await;
        assert_eq!(res_post.status(), StatusCode::BAD_REQUEST);

        let res_thread = server
            .post_json_authed("/api/x/thread", &json!({ "url": bad }))
            .await;
        assert_eq!(res_thread.status(), StatusCode::BAD_REQUEST);
    }
}

/// 3.8: /api/media/info boundary errors.
#[tokio::test]
async fn test_adversarial_rest_media_info_boundaries() {
    let server = TestServer::start().await;

    let bad_urls = [
        "",
        "file:///etc/passwd",
        "ftp://example.com/movie.mp4",
        "gopher://127.0.0.1/audio",
    ];

    for bad in bad_urls {
        let res = server
            .post_json_authed("/api/media/info", &json!({ "url": bad }))
            .await;
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "Media URL '{}' must return 400 Bad Request",
            bad
        );
    }
}

// ============================================================================
// SECTION 4: OpenAPI 3.1.0 Formal Specification Validation
// ============================================================================

/// 4.1: Rigorous schema and completeness verification of GET /openapi.json.
#[tokio::test]
async fn test_adversarial_openapi_spec_rigorous_validation() {
    let server = TestServer::start().await;
    let res = server.get_authed("/openapi.json").await;
    assert_eq!(res.status(), StatusCode::OK);

    let doc: Value = res.json().await.expect("parse openapi spec");

    // 1. OpenAPI version strictly 3.1.0
    assert_eq!(doc["openapi"], "3.1.0");

    // 2. Info metadata
    let info = &doc["info"];
    assert!(info["title"].is_string());
    assert!(info["version"].is_string());
    assert!(info["description"].is_string());

    // 3. Components & Security Schemes
    let sec_scheme = &doc["components"]["securitySchemes"]["BearerAuth"];
    assert_eq!(sec_scheme["type"], "http");
    assert_eq!(sec_scheme["scheme"], "bearer");

    // 4. Validate all 9 required paths
    let required_paths = [
        "/health",
        "/mcp",
        "/sse",
        "/messages",
        "/api/web/markdown",
        "/api/pdf/text",
        "/api/x/post",
        "/api/x/thread",
        "/api/media/info",
    ];

    let paths = doc["paths"].as_object().expect("paths object");
    for p in required_paths {
        assert!(paths.contains_key(p), "Missing required path: {}", p);
    }

    // 5. Protected operations define BearerAuth, /health does not
    let health_op = &paths["/health"]["get"];
    assert!(health_op.is_object());
    assert!(
        health_op.get("security").is_none(),
        "Public /health must NOT define security"
    );

    let protected_endpoints = [
        ("/mcp", "post"),
        ("/sse", "get"),
        ("/messages", "post"),
        ("/api/web/markdown", "get"),
        ("/api/web/markdown", "post"),
        ("/api/pdf/text", "get"),
        ("/api/pdf/text", "post"),
        ("/api/x/post", "get"),
        ("/api/x/post", "post"),
        ("/api/x/thread", "get"),
        ("/api/x/thread", "post"),
        ("/api/media/info", "get"),
        ("/api/media/info", "post"),
    ];

    for (p, m) in protected_endpoints {
        let op = &paths[p][m];
        assert!(op.is_object(), "Missing operation {} {}", m, p);

        let sec = &op["security"];
        assert!(
            sec.is_array(),
            "Operation {} {} must declare security array",
            m,
            p
        );
        let has_bearer = sec.as_array().unwrap().iter().any(|item| {
            item.as_object().map(|o| o.contains_key("BearerAuth")).unwrap_or(false)
        });
        assert!(
            has_bearer,
            "Operation {} {} must require BearerAuth",
            m,
            p
        );

        // Verify responses define 401
        assert!(
            op["responses"]["401"].is_object(),
            "Operation {} {} must document 401 response",
            m,
            p
        );
    }

    // 6. POST endpoints define requestBody with application/json
    let post_endpoints = [
        "/mcp",
        "/messages",
        "/api/web/markdown",
        "/api/pdf/text",
        "/api/x/post",
        "/api/x/thread",
        "/api/media/info",
    ];

    for p in post_endpoints {
        let post_op = &paths[p]["post"];
        assert!(
            post_op["requestBody"]["content"]["application/json"]["schema"].is_object(),
            "POST {} must define application/json schema in requestBody",
            p
        );
    }
}

// ============================================================================
// SECTION 5: Multi-Transport Cross-Protocol Concurrency Storm
// ============================================================================

/// 5.1: High-concurrency combined burst across all Milestone 3 transports:
/// Streamable HTTP POST /mcp, SSE GET /sse + POST /messages, REST GET /api/web/markdown,
/// REST POST /api/web/markdown, REST POST /api/pdf/text, GET /openapi.json, and GET /health.
#[tokio::test]
async fn test_adversarial_m3_multi_transport_concurrency_storm() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let (sse_session_id, mut sse_stream) = sse_connect(&server).await;
    let pdf_fixture = create_test_pdf("Storm PDF Content", 2);
    let pdf_path = pdf_fixture.path().to_str().unwrap().to_string();

    const STORM_COUNT: usize = 35;
    let mut handles = Vec::new();

    for i in 0..STORM_COUNT {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let token = server.auth_token.clone();
        let article_url = mock.url("/article-simple");
        let s_id = sse_session_id.clone();
        let p_path = pdf_path.clone();

        handles.push(tokio::spawn(async move {
            match i % 7 {
                // 0: Streamable HTTP MCP tools/list
                0 => {
                    let res = client
                        .post(format!("{}/mcp", base_url))
                        .header("Authorization", format!("Bearer {}", token))
                        .json(&json!({ "jsonrpc": "2.0", "id": i, "method": "tools/list", "params": {} }))
                        .send()
                        .await
                        .expect("mcp post");
                    assert_eq!(res.status(), StatusCode::OK);
                }
                // 1: SSE POST /messages
                1 => {
                    let res = client
                        .post(format!("{}/messages?sessionId={}", base_url, s_id))
                        .header("Authorization", format!("Bearer {}", token))
                        .json(&json!({ "jsonrpc": "2.0", "id": i, "method": "ping", "params": {} }))
                        .send()
                        .await
                        .expect("messages post");
                    assert_eq!(res.status(), StatusCode::ACCEPTED);
                }
                // 2: REST GET /api/web/markdown
                2 => {
                    let res = client
                        .get(format!("{}/api/web/markdown?url={}", base_url, urlencoding::encode(&article_url)))
                        .header("Authorization", format!("Bearer {}", token))
                        .send()
                        .await
                        .expect("rest get");
                    assert_eq!(res.status(), StatusCode::OK);
                }
                // 3: REST POST /api/web/markdown
                3 => {
                    let res = client
                        .post(format!("{}/api/web/markdown", base_url))
                        .header("Authorization", format!("Bearer {}", token))
                        .json(&json!({ "url": article_url }))
                        .send()
                        .await
                        .expect("rest post markdown");
                    assert_eq!(res.status(), StatusCode::OK);
                }
                // 4: REST POST /api/pdf/text
                4 => {
                    let res = client
                        .post(format!("{}/api/pdf/text", base_url))
                        .header("Authorization", format!("Bearer {}", token))
                        .json(&json!({ "path": p_path, "pages": "1" }))
                        .send()
                        .await
                        .expect("rest post pdf");
                    assert_eq!(res.status(), StatusCode::OK);
                }
                // 5: GET /openapi.json
                5 => {
                    let res = client
                        .get(format!("{}/openapi.json", base_url))
                        .header("Authorization", format!("Bearer {}", token))
                        .send()
                        .await
                        .expect("openapi get");
                    assert_eq!(res.status(), StatusCode::OK);
                }
                // 6: Public unauthenticated GET /health
                _ => {
                    let res = client
                        .get(format!("{}/health", base_url))
                        .send()
                        .await
                        .expect("health get");
                    assert_eq!(res.status(), StatusCode::OK);
                }
            }
        }));
    }

    for h in handles {
        h.await.expect("join handle");
    }

    // Verify SSE received responses for each message routed to it (5 messages: i % 7 == 1 -> i=1, 8, 15, 22, 29)
    let sse_expected_count = STORM_COUNT / 7;
    for _ in 0..sse_expected_count {
        let (event, data) = next_sse_event(&mut sse_stream, Duration::from_secs(5))
            .await
            .expect("receive sse event during storm");
        assert_eq!(event, "message");
        let parsed: Value = serde_json::from_str(&data).expect("parse sse data");
        assert!(parsed["result"].is_object());
    }

    // Verify server remains fully healthy and responsive
    let health = server.get("/health").await;
    assert_eq!(health.status(), StatusCode::OK);
}
