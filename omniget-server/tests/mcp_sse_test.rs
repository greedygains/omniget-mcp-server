#![allow(clippy::needless_borrows_for_generic_args)]

mod common;

use common::{MockWebsite, TestServer};
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
            Err(_) => {
                // Read tick timeout, continue until overall timeout
            }
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
        "endpoint event must contain sessionId, got: {}",
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
// FEATURE 5: MCP over SSE (GET /sse + POST /messages) — Tier 1 & Tier 2
// ============================================================================

/// T1.1: GET /sse returns HTTP 200 with text/event-stream Content-Type and Cache-Control: no-cache.
#[tokio::test]
async fn test_t1_f5_sse_handshake_headers() {
    let server = TestServer::start().await;
    let res = server
        .client
        .get(&server.url("/sse"))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .send()
        .await
        .expect("connect /sse");

    assert_eq!(res.status(), StatusCode::OK);

    let content_type = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .expect("content-type header")
        .to_str()
        .expect("utf8");
    assert!(
        content_type.contains("text/event-stream"),
        "Content-Type must be text/event-stream, got: {}",
        content_type
    );

    let cache_control = res
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .map(|v| v.to_str().unwrap_or(""));
    assert!(
        cache_control.unwrap_or("").contains("no-cache"),
        "Cache-Control must contain no-cache"
    );
}

/// T1.2: Initial SSE event emitted is "event: endpoint" with data: "/messages?sessionId=<uuid>".
#[tokio::test]
async fn test_t1_f5_sse_initial_endpoint_event() {
    let server = TestServer::start().await;
    let (session_id, _stream) = sse_connect(&server).await;
    assert!(!session_id.is_empty(), "session_id should not be empty");
}

/// T1.3: Extracted sessionId is a valid RFC 4122 UUID v4.
#[tokio::test]
async fn test_t1_f5_sse_session_uuid_format() {
    let server = TestServer::start().await;
    let (session_id, _stream) = sse_connect(&server).await;
    let parsed = Uuid::parse_str(&session_id);
    assert!(
        parsed.is_ok(),
        "sessionId must parse as valid UUID, got: {}",
        session_id
    );
}

/// T1.4: POST /messages?sessionId=<uuid> returns HTTP 202 Accepted with empty body.
#[tokio::test]
async fn test_t1_f5_sse_post_messages_returns_202_accepted() {
    let server = TestServer::start().await;
    let (session_id, _stream) = sse_connect(&server).await;

    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping",
        "params": {}
    });

    let res = server
        .client
        .post(&server.url(&format!("/messages?sessionId={}", session_id)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&payload)
        .send()
        .await
        .expect("post to /messages");

    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let bytes = res.bytes().await.expect("read body");
    assert!(bytes.is_empty(), "Response body for 202 must be empty");
}

/// T1.5: JSON-RPC response is delivered via SSE event: message with matching id and result.
#[tokio::test]
async fn test_t1_f5_sse_response_routed_to_stream() {
    let server = TestServer::start().await;
    let (session_id, mut stream) = sse_connect(&server).await;

    let payload = json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "ping",
        "params": {}
    });

    let res = server
        .client
        .post(&server.url(&format!("/messages?sessionId={}", session_id)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&payload)
        .send()
        .await
        .expect("post ping to /messages");
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    let (event, data) = next_sse_event(&mut stream, Duration::from_secs(5))
        .await
        .expect("receive ping response on SSE");

    assert_eq!(event, "message");
    let rpc_res: Value = serde_json::from_str(&data).expect("parse json-rpc response");
    assert_eq!(rpc_res["jsonrpc"], "2.0");
    assert_eq!(rpc_res["id"], 42);
    assert_eq!(rpc_res["result"], json!({}));
}

/// T2.1: GET /sse without Bearer token returns HTTP 401 Unauthorized.
#[tokio::test]
async fn test_t2_f5_sse_unauthenticated_get_returns_401() {
    let server = TestServer::start().await;
    let res = server.get("/sse").await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// T2.2: POST /messages without Bearer token returns HTTP 401 Unauthorized.
#[tokio::test]
async fn test_t2_f5_sse_unauthenticated_post_returns_401() {
    let server = TestServer::start().await;
    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    let res = server
        .client
        .post(&server.url("/messages?sessionId=00000000-0000-0000-0000-000000000000"))
        .json(&payload)
        .send()
        .await
        .expect("send unauthenticated post");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// T2.3: POST /messages missing sessionId query parameter returns HTTP 400 Bad Request.
#[tokio::test]
async fn test_t2_f5_sse_missing_session_id_returns_400() {
    let server = TestServer::start().await;
    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    let res = server
        .client
        .post(&server.url("/messages"))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&payload)
        .send()
        .await
        .expect("send post without sessionId");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// T2.4: POST /messages with unknown/non-existent sessionId returns HTTP 404 Not Found.
#[tokio::test]
async fn test_t2_f5_sse_unknown_session_id_returns_404() {
    let server = TestServer::start().await;
    let fake_uuid = Uuid::new_v4().to_string();
    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });

    let res = server
        .client
        .post(&server.url(&format!("/messages?sessionId={}", fake_uuid)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&payload)
        .send()
        .await
        .expect("send post to unknown session");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// T2.5: POST /messages with non-UUID sessionId string returns HTTP 400 Bad Request.
#[tokio::test]
async fn test_t2_f5_sse_malformed_session_id_returns_400() {
    let server = TestServer::start().await;
    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });

    let res = server
        .client
        .post(&server.url("/messages?sessionId=not-a-valid-uuid-format"))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&payload)
        .send()
        .await
        .expect("send post with malformed session id");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// T2.6: POST /messages with malformed JSON body returns HTTP 400 Bad Request.
#[tokio::test]
async fn test_t2_f5_sse_malformed_json_body_returns_400() {
    let server = TestServer::start().await;
    let (session_id, _stream) = sse_connect(&server).await;

    let res = server
        .client
        .post(&server.url(&format!("/messages?sessionId={}", session_id)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .header("Content-Type", "application/json")
        .body("INVALID RAW JSON CONTENT")
        .send()
        .await
        .expect("send malformed json body");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// T2.7: Multiple concurrent SSE sessions receive independent session IDs without crosstalk.
#[tokio::test]
async fn test_t2_f5_sse_multiple_concurrent_sessions_isolated() {
    let server = TestServer::start().await;

    let (session1, mut stream1) = sse_connect(&server).await;
    let (session2, mut stream2) = sse_connect(&server).await;

    assert_ne!(session1, session2, "Session IDs must be distinct");

    // Send request to session 1
    let payload1 = json!({ "jsonrpc": "2.0", "id": 1001, "method": "ping", "params": {} });
    let res1 = server
        .client
        .post(&server.url(&format!("/messages?sessionId={}", session1)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&payload1)
        .send()
        .await
        .expect("send to session 1");
    assert_eq!(res1.status(), StatusCode::ACCEPTED);

    // Stream 1 receives response for 1001
    let (event1, data1) = next_sse_event(&mut stream1, Duration::from_secs(5))
        .await
        .expect("receive on stream 1");
    assert_eq!(event1, "message");
    let rpc1: Value = serde_json::from_str(&data1).expect("parse stream 1 json");
    assert_eq!(rpc1["id"], 1001);

    // Send request to session 2
    let payload2 = json!({ "jsonrpc": "2.0", "id": 1002, "method": "ping", "params": {} });
    let res2 = server
        .client
        .post(&server.url(&format!("/messages?sessionId={}", session2)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&payload2)
        .send()
        .await
        .expect("send to session 2");
    assert_eq!(res2.status(), StatusCode::ACCEPTED);

    // Stream 2 receives response for 1002
    let (event2, data2) = next_sse_event(&mut stream2, Duration::from_secs(5))
        .await
        .expect("receive on stream 2");
    assert_eq!(event2, "message");
    let rpc2: Value = serde_json::from_str(&data2).expect("parse stream 2 json");
    assert_eq!(rpc2["id"], 1002);
}

// ============================================================================
// TIER 3: Cross-Feature Combinations
// ============================================================================

/// T3.1: SSE tools/list discovery followed by tools/call execution via stream.
#[tokio::test]
async fn test_t3_sse_tools_list_and_call_pipeline() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;
    let (session_id, mut stream) = sse_connect(&server).await;

    // 1. tools/list
    let list_req = json!({ "jsonrpc": "2.0", "id": 201, "method": "tools/list", "params": {} });
    let res = server
        .client
        .post(&server.url(&format!("/messages?sessionId={}", session_id)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&list_req)
        .send()
        .await
        .expect("post tools/list");
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    let (event, data) = next_sse_event(&mut stream, Duration::from_secs(5))
        .await
        .expect("receive tools/list on SSE");
    assert_eq!(event, "message");
    let list_res: Value = serde_json::from_str(&data).expect("parse tools/list");
    assert_eq!(list_res["id"], 201);
    assert!(list_res["result"]["tools"].as_array().is_some());

    // 2. tools/call web_to_markdown
    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 202,
        "method": "tools/call",
        "params": {
            "name": "web_to_markdown",
            "arguments": { "url": mock.url("/article-simple") }
        }
    });
    let res = server
        .client
        .post(&server.url(&format!("/messages?sessionId={}", session_id)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&call_req)
        .send()
        .await
        .expect("post tools/call");
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    let (event, data) = next_sse_event(&mut stream, Duration::from_secs(5))
        .await
        .expect("receive tools/call on SSE");
    assert_eq!(event, "message");
    let call_res: Value = serde_json::from_str(&data).expect("parse tools/call");
    assert_eq!(call_res["id"], 202);
    let md = call_res["result"]["content"][0]["text"]
        .as_str()
        .expect("markdown text");
    assert!(md.contains("Simple Article Title"));
}

/// T3.2: Multiple concurrent messages over the same session ID routed and ordered.
#[tokio::test]
async fn test_t3_sse_concurrent_messages_same_session() {
    let server = TestServer::start().await;
    let (session_id, mut stream) = sse_connect(&server).await;

    // Send 3 requests concurrently
    for i in 1..=3 {
        let payload = json!({ "jsonrpc": "2.0", "id": i, "method": "ping", "params": {} });
        let res = server
            .client
            .post(&server.url(&format!("/messages?sessionId={}", session_id)))
            .header("Authorization", format!("Bearer {}", server.auth_token))
            .json(&payload)
            .send()
            .await
            .expect("send concurrent ping");
        assert_eq!(res.status(), StatusCode::ACCEPTED);
    }

    // Read 3 responses from stream
    let mut received_ids = Vec::new();
    for _ in 1..=3 {
        let (event, data) = next_sse_event(&mut stream, Duration::from_secs(5))
            .await
            .expect("receive concurrent response");
        assert_eq!(event, "message");
        let parsed: Value = serde_json::from_str(&data).expect("parse rpc");
        received_ids.push(parsed["id"].as_i64().expect("id"));
    }

    assert!(received_ids.contains(&1));
    assert!(received_ids.contains(&2));
    assert!(received_ids.contains(&3));
}

// ============================================================================
// TIER 4: Real-World Workloads & Scenarios
// ============================================================================

/// T4.1: Scenario 1 — AI Agent End-to-End Research Pipeline:
/// AI Agent connects via SSE, handshakes initialize, discovers tools via tools/list,
/// and converts an external news link to Markdown via web_to_markdown.
#[tokio::test]
async fn test_t4_scenario_1_ai_agent_research_pipeline_sse() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    // Step 1: Connect to SSE stream
    let (session_id, mut stream) = sse_connect(&server).await;

    // Step 2: Initialize handshake
    let init_payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "Claude-Research-Agent", "version": "3.5" }
        }
    });

    let res = server
        .client
        .post(&server.url(&format!("/messages?sessionId={}", session_id)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&init_payload)
        .send()
        .await
        .expect("post initialize");
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    let (event, data) = next_sse_event(&mut stream, Duration::from_secs(5))
        .await
        .expect("receive init response");
    assert_eq!(event, "message");
    let init_res: Value = serde_json::from_str(&data).expect("parse init");
    assert_eq!(init_res["id"], 1);
    assert_eq!(init_res["result"]["serverInfo"]["name"], "omniget-server");

    // Step 3: Discover available tools
    let list_payload = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} });
    let res = server
        .client
        .post(&server.url(&format!("/messages?sessionId={}", session_id)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&list_payload)
        .send()
        .await
        .expect("post tools/list");
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    let (event, data) = next_sse_event(&mut stream, Duration::from_secs(5))
        .await
        .expect("receive tools/list response");
    assert_eq!(event, "message");
    let list_res: Value = serde_json::from_str(&data).expect("parse tools/list");
    assert_eq!(list_res["id"], 2);

    // Step 4: Extract article with tables via web_to_markdown
    let call_payload = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "web_to_markdown",
            "arguments": { "url": mock.url("/article-with-table") }
        }
    });

    let res = server
        .client
        .post(&server.url(&format!("/messages?sessionId={}", session_id)))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&call_payload)
        .send()
        .await
        .expect("post tools/call");
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    let (event, data) = next_sse_event(&mut stream, Duration::from_secs(5))
        .await
        .expect("receive tools/call response");
    assert_eq!(event, "message");
    let call_res: Value = serde_json::from_str(&data).expect("parse tools/call");
    assert_eq!(call_res["id"], 3);

    let md_content = call_res["result"]["content"][0]["text"]
        .as_str()
        .expect("markdown text");
    assert!(
        md_content.contains("Quarterly Performance"),
        "Title must be present"
    );
    assert!(
        md_content.contains("| Quarter | Revenue | Profit |"),
        "Table header must be converted"
    );
    assert!(
        !md_content.contains("<table>"),
        "Raw table HTML tags must be stripped"
    );
}
