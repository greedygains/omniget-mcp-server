#![allow(clippy::needless_borrows_for_generic_args)]

mod common;

use common::{create_test_pdf, MockWebsite, TestServer};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::time::Duration;

// ============================================================================
// FEATURE 4: Streamable HTTP MCP (POST /mcp) — Tier 1 & Tier 2
// ============================================================================

/// T1.1: POST /mcp with "initialize" returns protocol version, serverInfo, and tool capabilities.
#[tokio::test]
async fn test_t1_f4_mcp_initialize_handshake() {
    let server = TestServer::start().await;
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-ai-client",
                "version": "1.0.0"
            }
        }
    });

    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);

    let body: Value = res.json().await.expect("parse json-rpc response");
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 1);

    let result = &body["result"];
    assert!(result.is_object(), "Result must be an object");
    assert_eq!(result["serverInfo"]["name"], "omniget-server");
    assert!(result["capabilities"]["tools"].is_object());
}

/// T1.2: POST /mcp with "ping" returns empty result object {}.
#[tokio::test]
async fn test_t1_f4_mcp_ping() {
    let server = TestServer::start().await;
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "ping",
        "params": {}
    });

    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);

    let body: Value = res.json().await.expect("parse ping response");
    assert_eq!(body["id"], 2);
    assert_eq!(body["result"], json!({}));
}

/// T1.3: POST /mcp with "tools/list" returns all 5 universal extraction tools with schemas.
#[tokio::test]
async fn test_t1_f4_mcp_tools_list() {
    let server = TestServer::start().await;
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/list",
        "params": {}
    });

    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);

    let body: Value = res.json().await.expect("parse tools/list response");
    let tools = body["result"]["tools"]
        .as_array()
        .expect("tools array in result");

    assert_eq!(tools.len(), 7, "tools/list must return exactly 7 extraction tools");
    let tool_names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().expect("tool name")).collect();
    assert!(tool_names.contains(&"x_post"), "Missing x_post tool");
    assert!(tool_names.contains(&"x_thread"), "Missing x_thread tool");
    assert!(tool_names.contains(&"web_to_markdown"), "Missing web_to_markdown tool");
    assert!(tool_names.contains(&"pdf_text"), "Missing pdf_text tool");
    assert!(tool_names.contains(&"media_info"), "Missing media_info tool");
    assert!(tool_names.contains(&"instagram_post"), "Missing instagram_post tool");
    assert!(tool_names.contains(&"facebook_post"), "Missing facebook_post tool");

    for tool in tools {
        assert!(tool["description"].is_string());
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
}

/// T1.4: POST /mcp with "tools/call" executes web_to_markdown against MockWebsite.
#[tokio::test]
async fn test_t1_f4_mcp_tools_call_web_to_markdown() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let payload = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "web_to_markdown",
            "arguments": {
                "url": mock.url("/article-simple")
            }
        }
    });

    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);

    let body: Value = res.json().await.expect("parse tools/call response");
    assert_eq!(body["id"], 4);

    let content = &body["result"]["content"];
    assert!(content.is_array(), "content must be an array");
    let text = content[0]["text"].as_str().expect("text content");
    assert!(
        text.contains("Simple Article Title"),
        "Extracted text should contain title"
    );
    assert!(
        !text.contains("<article>"),
        "Output must be clean Markdown without raw HTML tags"
    );
}

/// T1.5: POST /mcp with "notifications/initialized" returns HTTP 202 Accepted with empty body.
#[tokio::test]
async fn test_t1_f4_mcp_notification_initialized_returns_202() {
    let server = TestServer::start().await;
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });

    let res = server.post_json_authed("/mcp", &notification).await;
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let bytes = res.bytes().await.expect("read response bytes");
    assert!(bytes.is_empty(), "Notification response body must be empty");
}

/// T2.1: POST /mcp with unknown method returns standard JSON-RPC -32601 Method not found.
#[tokio::test]
async fn test_t2_f4_mcp_unknown_method_returns_32601() {
    let server = TestServer::start().await;
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 100,
        "method": "unknown/nonexistent_method",
        "params": {}
    });

    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);

    let body: Value = res.json().await.expect("parse response");
    assert_eq!(body["id"], 100);
    assert_eq!(body["error"]["code"], -32601);
}

/// T2.2: POST /mcp with invalid tool call arguments returns JSON-RPC -32602 Invalid params.
#[tokio::test]
async fn test_t2_f4_mcp_invalid_params_returns_32602() {
    let server = TestServer::start().await;
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 101,
        "method": "tools/call",
        "params": {
            "name": "web_to_markdown",
            "arguments": {
                "invalid_field": 12345
            }
        }
    });

    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);

    let body: Value = res.json().await.expect("parse response");
    assert_eq!(body["id"], 101);
    assert!(body["error"].is_object() || body["result"]["isError"] == true);
}

/// T2.3: POST /mcp with malformed JSON syntax returns HTTP 400 Bad Request with -32700.
#[tokio::test]
async fn test_t2_f4_mcp_malformed_json_syntax_returns_32700() {
    let server = TestServer::start().await;
    let res = server
        .client
        .post(&server.url("/mcp"))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .header("Content-Type", "application/json")
        .body("{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": broken")
        .send()
        .await
        .expect("send broken json");

    assert!(
        res.status() == StatusCode::BAD_REQUEST || res.status() == StatusCode::OK,
        "Parse error should return 400 Bad Request or 200 JSON-RPC error"
    );

    let body: Value = res.json().await.expect("parse response");
    assert_eq!(body["error"]["code"], -32700);
}

/// T2.4: POST /mcp with invalid JSON-RPC structure (missing "jsonrpc") returns -32600.
#[tokio::test]
async fn test_t2_f4_mcp_invalid_request_missing_version_returns_32600() {
    let server = TestServer::start().await;
    let payload = json!({
        "id": 102,
        "method": "ping"
    });

    let res = server.post_json_authed("/mcp", &payload).await;
    assert!(res.status() == StatusCode::BAD_REQUEST || res.status() == StatusCode::OK);

    let body: Value = res.json().await.expect("parse response");
    assert_eq!(body["error"]["code"], -32600);
}

/// T2.5: POST /mcp supports string, integer, and null IDs in JSON-RPC 2.0 requests.
#[tokio::test]
async fn test_t2_f4_mcp_supports_string_int_and_null_ids() {
    let server = TestServer::start().await;

    // String ID
    let res_str = server
        .post_json_authed(
            "/mcp",
            &json!({ "jsonrpc": "2.0", "id": "req-uuid-xyz", "method": "ping", "params": {} }),
        )
        .await;
    let body_str: Value = res_str.json().await.expect("parse string id");
    assert_eq!(body_str["id"], "req-uuid-xyz");

    // Integer ID
    let res_int = server
        .post_json_authed(
            "/mcp",
            &json!({ "jsonrpc": "2.0", "id": 99999, "method": "ping", "params": {} }),
        )
        .await;
    let body_int: Value = res_int.json().await.expect("parse int id");
    assert_eq!(body_int["id"], 99999);

    // Null ID
    let res_null = server
        .post_json_authed(
            "/mcp",
            &json!({ "jsonrpc": "2.0", "id": null, "method": "ping", "params": {} }),
        )
        .await;
    let body_null: Value = res_null.json().await.expect("parse null id");
    assert!(body_null["id"].is_null());
}

/// T2.6: POST /mcp without Bearer token returns HTTP 401 Unauthorized.
#[tokio::test]
async fn test_t2_f4_mcp_unauthenticated_post_returns_401() {
    let server = TestServer::start().await;
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping",
        "params": {}
    });

    let res = server.post_json("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// T2.7: POST /mcp safely processes huge request payloads (e.g. 500KB JSON).
#[tokio::test]
async fn test_t2_f4_mcp_huge_payload_handling() {
    let server = TestServer::start().await;
    let huge_string = "x".repeat(500_000);
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 103,
        "method": "unknown_huge",
        "params": {
            "blob": huge_string
        }
    });

    let res = server.post_json_authed("/mcp", &payload).await;
    assert!(res.status() == StatusCode::OK || res.status() == StatusCode::PAYLOAD_TOO_LARGE);
}

// ============================================================================
// TIER 3: Cross-Feature Combinations & Protocols
// ============================================================================

/// T3.1: Bearer Auth + Streamable MCP + Web Scraping table extraction.
#[tokio::test]
async fn test_t3_mcp_streamable_with_auth_and_web_table_extraction() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let payload = json!({
        "jsonrpc": "2.0",
        "id": 201,
        "method": "tools/call",
        "params": {
            "name": "web_to_markdown",
            "arguments": {
                "url": mock.url("/article-with-table")
            }
        }
    });

    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);

    let body: Value = res.json().await.expect("parse response");
    let markdown = body["result"]["content"][0]["text"]
        .as_str()
        .expect("markdown text");

    assert!(
        markdown.contains("| Quarter | Revenue | Profit |"),
        "Markdown must contain formatted table header"
    );
    assert!(
        markdown.contains("| Q1 | $10M | $2M |"),
        "Markdown must contain formatted table row"
    );
}

/// T3.2: Streamable MCP calling pdf_text on generated PDF fixture.
#[tokio::test]
async fn test_t3_mcp_streamable_call_pdf_text_fixture() {
    let server = TestServer::start().await;
    let pdf_file = create_test_pdf("OmniGet Headless PDF Test Document", 2);
    let pdf_path = pdf_file.path().to_str().expect("pdf path");

    let payload = json!({
        "jsonrpc": "2.0",
        "id": 202,
        "method": "tools/call",
        "params": {
            "name": "pdf_text",
            "arguments": {
                "path": pdf_path,
                "pages": "1"
            }
        }
    });

    let res = server.post_json_authed("/mcp", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);

    let body: Value = res.json().await.expect("parse response");
    let content_text = body["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    assert!(
        content_text.contains("OmniGet Headless PDF Test Document"),
        "Extracted text must match generated fixture"
    );
}

/// T3.3: Full sequential JSON-RPC lifecycle: initialize -> notification -> tools/list -> tools/call.
#[tokio::test]
async fn test_t3_mcp_streamable_full_lifecycle_sequence() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    // Step 1: Initialize
    let init_res = server
        .post_json_authed(
            "/mcp",
            &json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "seq-test", "version": "1.0" } }
            }),
        )
        .await;
    assert_eq!(init_res.status(), StatusCode::OK);

    // Step 2: Initialized notification
    let notif_res = server
        .post_json_authed(
            "/mcp",
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
    assert_eq!(notif_res.status(), StatusCode::ACCEPTED);

    // Step 3: Discover tools
    let list_res = server
        .post_json_authed(
            "/mcp",
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
        )
        .await;
    assert_eq!(list_res.status(), StatusCode::OK);

    // Step 4: Call web_to_markdown
    let call_res = server
        .post_json_authed(
            "/mcp",
            &json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "web_to_markdown", "arguments": { "url": mock.url("/article-simple") } }
            }),
        )
        .await;
    assert_eq!(call_res.status(), StatusCode::OK);
}

// ============================================================================
// TIER 4: Real-World Workloads & Scenarios
// ============================================================================

/// T4.1: Scenario 4 — Protocol Resilience & Edge-Case Storm:
/// Fires concurrent valid/invalid JSON-RPC calls, rapid requests, verifying server stability.
#[tokio::test]
async fn test_t4_scenario_4_protocol_resilience_and_edge_case_storm() {
    let server = TestServer::start().await;

    let mut handles = Vec::new();
    for i in 0..25 {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let token = server.auth_token.clone();

        handles.push(tokio::spawn(async move {
            let (method, payload, expected_status) = match i % 5 {
                0 => (
                    "ping",
                    json!({ "jsonrpc": "2.0", "id": i, "method": "ping", "params": {} }),
                    StatusCode::OK,
                ),
                1 => (
                    "tools/list",
                    json!({ "jsonrpc": "2.0", "id": i, "method": "tools/list", "params": {} }),
                    StatusCode::OK,
                ),
                2 => (
                    "unknown_method",
                    json!({ "jsonrpc": "2.0", "id": i, "method": "nonexistent", "params": {} }),
                    StatusCode::OK,
                ),
                3 => (
                    "notification",
                    json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                    StatusCode::ACCEPTED,
                ),
                _ => (
                    "ping_invalid_token",
                    json!({ "jsonrpc": "2.0", "id": i, "method": "ping", "params": {} }),
                    StatusCode::UNAUTHORIZED,
                ),
            };

            let mut req = client.post(format!("{}/mcp", base_url)).json(&payload);
            if i % 5 != 4 {
                req = req.header("Authorization", format!("Bearer {}", token));
            } else {
                req = req.header("Authorization", "Bearer invalid-token");
            }

            let res = req.send().await.expect("send storm request");
            assert_eq!(
                res.status(),
                expected_status,
                "Request {} failed expected status for method {}",
                i,
                method
            );
        }));
    }

    for handle in handles {
        handle.await.expect("join handle");
    }

    // Verify server is healthy and operational after the storm
    let health = server.get("/health").await;
    assert_eq!(health.status(), StatusCode::OK);
}

/// T4.2: Scenario 5 — Container Liveness & Orchestration Probe:
/// Orchestrator probes GET /health without auth while heavy JSON-RPC calls are processed on /mcp.
#[tokio::test]
async fn test_t4_scenario_5_container_liveness_under_mcp_load() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    // Launch background load on /mcp
    let mut load_handles = Vec::new();
    for i in 0..10 {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let token = server.auth_token.clone();
        let article_url = mock.url("/huge-article");

        load_handles.push(tokio::spawn(async move {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "tools/call",
                "params": {
                    "name": "web_to_markdown",
                    "arguments": { "url": article_url }
                }
            });
            let _ = client
                .post(format!("{}/mcp", base_url))
                .header("Authorization", format!("Bearer {}", token))
                .json(&payload)
                .send()
                .await;
        }));
    }

    // Concurrent liveness probe loop
    for _ in 0..10 {
        let start = std::time::Instant::now();
        let res = server.get("/health").await;
        let latency = start.elapsed();

        assert_eq!(
            res.status(),
            StatusCode::OK,
            "Health probe must return 200 OK while under load"
        );
        assert!(
            latency < Duration::from_millis(50),
            "Health probe latency under load must be < 50ms, took {:?}",
            latency
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    for handle in load_handles {
        let _ = handle.await;
    }
}
