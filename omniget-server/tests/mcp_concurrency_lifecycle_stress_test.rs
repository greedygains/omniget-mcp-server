#![allow(clippy::needless_borrows_for_generic_args)]

//! Empirical Stress, Concurrency & Transport Lifecycle Test Suite for Milestone 3.
//!
//! Verifies:
//! 1. High-concurrency `POST /mcp` Streamable HTTP requests (batch arrays, notifications, interleaved initialize/ping/tools requests).
//! 2. MCP over SSE Lifecycle: Rapid stream disconnects, multiple concurrent SSE streams with distinct session IDs, concurrent `POST /messages` bursts routed to single or multiple SSE streams.
//! 3. Memory & Resource Leaks: Verifying `SessionStore` cleans up entries when clients disconnect, ensuring channels do not deadlock when receivers are dropped.
//! 4. Cross-transport saturation with uninterrupted sub-50ms container health probe.

mod common;

use common::{MockWebsite, TestServer};
use futures::StreamExt;
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Helper function to parse multiple raw SSE chunks from a stream into a list of (event_type, data) pairs.
async fn collect_sse_events<S>(
    stream: &mut S,
    expected_count: usize,
    timeout: Duration,
) -> Vec<(String, String)>
where
    S: futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    let start = Instant::now();
    let mut buffer = String::new();
    let mut events = Vec::new();

    while start.elapsed() < timeout && events.len() < expected_count {
        match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(bytes))) => {
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(pos) = buffer.find("\n\n") {
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
                    if !data.is_empty() || event_type == "endpoint" {
                        events.push((event_type, data));
                    }
                }
            }
            Ok(Some(Err(_))) => break,
            Ok(None) => break,
            Err(_) => {
                // Tick timeout, keep waiting until overall timeout
            }
        }
    }
    events
}

/// Helper function to connect to `/sse` and extract the session ID.
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

    let events = collect_sse_events(&mut stream, 1, Duration::from_secs(5)).await;
    assert!(
        !events.is_empty(),
        "Expected initial endpoint event from /sse"
    );

    let (event, data) = &events[0];
    assert_eq!(event, "endpoint");
    assert!(
        data.contains("sessionId="),
        "endpoint event must contain sessionId, got: {data}"
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
// SCENARIO 1: High-Concurrency POST /mcp Streamable HTTP
// ============================================================================

#[tokio::test]
async fn test_stress_mcp_streamable_100_concurrent_requests() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let total_tasks = 100;
    let success_count = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::with_capacity(total_tasks);

    let client = server.client.clone();
    let base_url = server.base_url.clone();
    let auth_token = server.auth_token.clone();
    let mock_url = mock.url("/article-simple");

    for i in 0..total_tasks {
        let client = client.clone();
        let base_url = base_url.clone();
        let auth_token = auth_token.clone();
        let mock_url = mock_url.clone();
        let success = success_count.clone();

        let handle = tokio::spawn(async move {
            let (payload, expected_status) = match i % 5 {
                0 => (
                    json!({
                        "jsonrpc": "2.0",
                        "id": i,
                        "method": "initialize",
                        "params": {
                            "protocolVersion": "2024-11-05",
                            "clientInfo": { "name": "stress-client", "version": "1.0" }
                        }
                    }),
                    StatusCode::OK,
                ),
                1 => (
                    json!({
                        "jsonrpc": "2.0",
                        "id": format!("ping-{i}"),
                        "method": "ping"
                    }),
                    StatusCode::OK,
                ),
                2 => (
                    json!({
                        "jsonrpc": "2.0",
                        "id": i,
                        "method": "tools/list"
                    }),
                    StatusCode::OK,
                ),
                3 => (
                    json!({
                        "jsonrpc": "2.0",
                        "id": i,
                        "method": "tools/call",
                        "params": {
                            "name": "web_to_markdown",
                            "arguments": { "url": mock_url }
                        }
                    }),
                    StatusCode::OK,
                ),
                _ => (
                    json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/initialized"
                    }),
                    StatusCode::ACCEPTED,
                ),
            };

            let res = client
                .post(format!("{base_url}/mcp"))
                .header("Authorization", format!("Bearer {auth_token}"))
                .json(&payload)
                .send()
                .await
                .expect("send mcp request");

            assert_eq!(res.status(), expected_status);
            if expected_status == StatusCode::OK {
                let body: Value = res.json().await.expect("parse response json");
                assert_eq!(body["jsonrpc"], "2.0");
                if payload.get("method") == Some(&Value::String("initialize".to_string())) {
                    assert_eq!(body["result"]["serverInfo"]["name"], "omniget-server");
                } else if payload.get("method") == Some(&Value::String("tools/list".to_string())) {
                    let tools = body["result"]["tools"].as_array().expect("tools array");
                    assert!(tools.len() >= 5);
                }
            }
            success.fetch_add(1, Ordering::SeqCst);
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.expect("task panicked");
    }

    assert_eq!(
        success_count.load(Ordering::SeqCst),
        total_tasks,
        "All 100 concurrent MCP requests must succeed"
    );
}

#[tokio::test]
async fn test_stress_mcp_streamable_concurrent_batches_and_notifications() {
    let server = TestServer::start().await;
    let client = server.client.clone();
    let base_url = server.base_url.clone();
    let auth_token = server.auth_token.clone();

    let num_batches = 20;
    let mut handles = Vec::with_capacity(num_batches);

    for b in 0..num_batches {
        let client = client.clone();
        let base_url = base_url.clone();
        let auth_token = auth_token.clone();

        let handle = tokio::spawn(async move {
            if b % 3 == 0 {
                // Pure notification batch -> should return 202 Accepted
                let batch = json!([
                    { "jsonrpc": "2.0", "method": "notifications/initialized" },
                    { "jsonrpc": "2.0", "method": "notifications/progress" },
                    { "jsonrpc": "2.0", "method": "notifications/cancelled" }
                ]);

                let res = client
                    .post(format!("{base_url}/mcp"))
                    .header("Authorization", format!("Bearer {auth_token}"))
                    .json(&batch)
                    .send()
                    .await
                    .expect("send batch notifications");

                assert_eq!(res.status(), StatusCode::ACCEPTED);
            } else {
                // Mixed batch of 20 requests and notifications
                let mut batch_items = Vec::new();
                for i in 0..20 {
                    if i % 4 == 0 {
                        batch_items.push(json!({
                            "jsonrpc": "2.0",
                            "method": "notifications/ping"
                        }));
                    } else {
                        batch_items.push(json!({
                            "jsonrpc": "2.0",
                            "id": format!("batch-{b}-req-{i}"),
                            "method": "ping"
                        }));
                    }
                }

                let res = client
                    .post(format!("{base_url}/mcp"))
                    .header("Authorization", format!("Bearer {auth_token}"))
                    .json(&batch_items)
                    .send()
                    .await
                    .expect("send mixed batch");

                assert_eq!(res.status(), StatusCode::OK);
                let body: Value = res.json().await.expect("parse batch response");
                let arr = body.as_array().expect("batch response is array");
                // 20 items minus 5 notifications = 15 responses
                assert_eq!(arr.len(), 15);
                for item in arr {
                    assert_eq!(item["jsonrpc"], "2.0");
                    assert!(item.get("result").is_some());
                }
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.expect("batch task panicked");
    }
}

#[tokio::test]
async fn test_stress_mcp_streamable_session_header_concurrency() {
    let server = TestServer::start().await;
    let client = server.client.clone();
    let base_url = server.base_url.clone();
    let auth_token = server.auth_token.clone();

    let total = 40;
    let mut handles = Vec::with_capacity(total);

    for i in 0..total {
        let client = client.clone();
        let base_url = base_url.clone();
        let auth_token = auth_token.clone();
        let session_id = format!("custom-session-uuid-{}", Uuid::new_v4());

        let handle = tokio::spawn(async move {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "ping"
            });

            let res = client
                .post(format!("{base_url}/mcp"))
                .header("Authorization", format!("Bearer {auth_token}"))
                .header("mcp-session-id", &session_id)
                .json(&payload)
                .send()
                .await
                .expect("send mcp request");

            assert_eq!(res.status(), StatusCode::OK);
            let echoed = res
                .headers()
                .get("mcp-session-id")
                .expect("mcp-session-id header present")
                .to_str()
                .expect("utf8 session id");

            assert_eq!(echoed, session_id);
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.expect("task panicked");
    }
}

// ============================================================================
// SCENARIO 2: MCP over SSE Lifecycle, Disconnects & Isolation
// ============================================================================

#[tokio::test]
async fn test_stress_sse_session_store_clean_lifecycle_and_zero_leak() {
    let server = TestServer::start().await;
    let store = omniget_server::sse::global_session_store();

    // Connect 20 SSE streams concurrently
    let total_streams = 20;
    let mut streams = Vec::with_capacity(total_streams);
    let mut session_ids = Vec::with_capacity(total_streams);

    for _ in 0..total_streams {
        let (session_id, stream) = sse_connect(&server).await;
        session_ids.push(session_id.clone());
        streams.push((session_id, stream));
    }

    // Verify all 20 sessions exist in the store
    for sid in &session_ids {
        assert!(
            store.get(sid).await.is_some(),
            "Session {sid} must exist in SessionStore while stream is open"
        );
    }

    // Explicitly drop all client streams (simulates client abrupt disconnect)
    drop(streams);

    // Allow background drop tasks to run
    let mut all_cleaned_up = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut still_present = 0;
        for sid in &session_ids {
            if store.get(sid).await.is_some() {
                still_present += 1;
            }
        }
        if still_present == 0 {
            all_cleaned_up = true;
            break;
        }
    }

    assert!(
        all_cleaned_up,
        "All 20 sessions must be cleaned up from SessionStore on stream drop (no memory leak)"
    );
}

#[tokio::test]
async fn test_stress_sse_rapid_connect_disconnect_churn() {
    let server = TestServer::start().await;
    let store = omniget_server::sse::global_session_store();
    let total_churn = 40;
    let mut session_ids = Vec::with_capacity(total_churn);

    // 40 sequential rapid connect-and-drops
    for _ in 0..total_churn {
        let (session_id, _stream) = sse_connect(&server).await;
        session_ids.push(session_id);
        // _stream dropped at end of loop iteration
    }

    // Wait for cleanup of all 40 churned sessions
    let mut all_cleaned_up = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut still_present = 0;
        for sid in &session_ids {
            if store.get(sid).await.is_some() {
                still_present += 1;
            }
        }
        if still_present == 0 {
            all_cleaned_up = true;
            break;
        }
    }

    assert!(
        all_cleaned_up,
        "All 40 churned sessions must be completely cleaned up from SessionStore"
    );
}

#[tokio::test]
async fn test_stress_sse_multi_session_isolation() {
    let server = TestServer::start().await;
    let num_sessions = 6;
    let messages_per_session = 5;

    let mut session_data = Vec::new();

    // Establish 6 concurrent SSE sessions
    for _ in 0..num_sessions {
        let (session_id, stream) = sse_connect(&server).await;
        session_data.push((session_id, stream));
    }

    // Fire messages concurrently to each session
    let mut post_handles = Vec::new();
    for (s_idx, (session_id, _)) in session_data.iter().enumerate() {
        for m_idx in 0..messages_per_session {
            let client = server.client.clone();
            let base_url = server.base_url.clone();
            let auth_token = server.auth_token.clone();
            let session_id = session_id.clone();

            let handle = tokio::spawn(async move {
                let payload = json!({
                    "jsonrpc": "2.0",
                    "id": format!("sess-{s_idx}-msg-{m_idx}"),
                    "method": "ping"
                });

                let res = client
                    .post(format!("{base_url}/messages?sessionId={session_id}"))
                    .header("Authorization", format!("Bearer {auth_token}"))
                    .json(&payload)
                    .send()
                    .await
                    .expect("send to /messages");

                assert_eq!(res.status(), StatusCode::ACCEPTED);
            });
            post_handles.push(handle);
        }
    }

    for h in post_handles {
        h.await.expect("post handle panicked");
    }

    // Read and verify responses from each stream
    for (s_idx, (_session_id, mut stream)) in session_data.into_iter().enumerate() {
        let events = collect_sse_events(&mut stream, messages_per_session, Duration::from_secs(5)).await;
        assert_eq!(
            events.len(),
            messages_per_session,
            "Stream {s_idx} must receive exactly {messages_per_session} messages"
        );

        let mut received_ids = HashSet::new();
        for (event_type, data) in events {
            assert_eq!(event_type, "message");
            let parsed: Value = serde_json::from_str(&data).expect("parse json response");
            assert_eq!(parsed["jsonrpc"], "2.0");
            let id = parsed["id"].as_str().expect("string id").to_string();
            assert!(
                id.starts_with(&format!("sess-{s_idx}-")),
                "Message id '{id}' routed to wrong session stream (expected sess-{s_idx}-*)"
            );
            received_ids.insert(id);
        }

        assert_eq!(
            received_ids.len(),
            messages_per_session,
            "All message IDs for session {s_idx} must be unique and present"
        );
    }
}

#[tokio::test]
async fn test_stress_sse_burst_messages_single_session() {
    let server = TestServer::start().await;
    let (session_id, mut stream) = sse_connect(&server).await;

    let burst_count = 60;
    let mut handles = Vec::with_capacity(burst_count);

    for i in 0..burst_count {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let auth_token = server.auth_token.clone();
        let session_id = session_id.clone();

        let handle = tokio::spawn(async move {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "ping"
            });

            let res = client
                .post(format!("{base_url}/messages?sessionId={session_id}"))
                .header("Authorization", format!("Bearer {auth_token}"))
                .json(&payload)
                .send()
                .await
                .expect("send to /messages");

            assert_eq!(res.status(), StatusCode::ACCEPTED);
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.expect("burst handle panicked");
    }

    let events = collect_sse_events(&mut stream, burst_count, Duration::from_secs(8)).await;
    assert_eq!(
        events.len(),
        burst_count,
        "Must receive all {burst_count} events from SSE stream"
    );

    let mut received_ids = HashSet::new();
    for (event_type, data) in events {
        assert_eq!(event_type, "message");
        let parsed: Value = serde_json::from_str(&data).expect("parse json");
        let id = parsed["id"].as_u64().expect("u64 id") as usize;
        received_ids.insert(id);
    }

    assert_eq!(
        received_ids.len(),
        burst_count,
        "Every single message in the burst must arrive intact without loss"
    );
}

#[tokio::test]
async fn test_stress_sse_channel_saturation_300_messages() {
    let server = TestServer::start().await;
    let (session_id, mut stream) = sse_connect(&server).await;

    // SESSION_CHANNEL_CAPACITY is 256. We burst 300 messages to test buffer backpressure.
    let total_messages = 300;
    let mut post_handles = Vec::with_capacity(total_messages);

    for i in 0..total_messages {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let auth_token = server.auth_token.clone();
        let session_id = session_id.clone();

        let handle = tokio::spawn(async move {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "ping"
            });

            let res = client
                .post(format!("{base_url}/messages?sessionId={session_id}"))
                .header("Authorization", format!("Bearer {auth_token}"))
                .json(&payload)
                .send()
                .await
                .expect("send to /messages");

            assert_eq!(res.status(), StatusCode::ACCEPTED);
        });
        post_handles.push(handle);
    }

    for h in post_handles {
        h.await.expect("post handle panicked");
    }

    // Read all 300 messages from the SSE stream
    let events = collect_sse_events(&mut stream, total_messages, Duration::from_secs(12)).await;
    assert_eq!(
        events.len(),
        total_messages,
        "All 300 messages must be received from stream even when exceeding channel capacity of 256"
    );

    let mut received_ids = HashSet::new();
    for (event_type, data) in events {
        assert_eq!(event_type, "message");
        let parsed: Value = serde_json::from_str(&data).expect("parse json");
        let id = parsed["id"].as_u64().expect("u64 id") as usize;
        received_ids.insert(id);
    }

    assert_eq!(
        received_ids.len(),
        total_messages,
        "Every single message ID (0..300) must be present"
    );
}

#[tokio::test]
async fn test_stress_sse_post_message_to_dropped_client_no_deadlock() {
    let server = TestServer::start().await;
    let (session_id, stream) = sse_connect(&server).await;

    // Drop the SSE stream to simulate client disconnect
    drop(stream);

    // Wait a brief moment for drop guard
    tokio::time::sleep(Duration::from_millis(150)).await;

    // POST /messages for the dropped session
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping"
    });

    let res = server
        .client
        .post(format!("{}/messages?sessionId={session_id}", server.base_url))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .json(&payload)
        .send()
        .await
        .expect("send post messages");

    // Once cleaned up, should return 404 NOT_FOUND
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_stress_sse_concurrent_invalid_session_ids() {
    let server = TestServer::start().await;
    let total = 50;
    let mut handles = Vec::with_capacity(total);

    for i in 0..total {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let auth_token = server.auth_token.clone();

        let handle = tokio::spawn(async move {
            let url = match i % 4 {
                0 => format!("{base_url}/messages"), // missing query string
                1 => format!("{base_url}/messages?sessionId=not-a-uuid"), // malformed uuid
                2 => format!("{base_url}/messages?sessionId={}", Uuid::new_v4()), // valid uuid but doesn't exist
                _ => format!("{base_url}/messages?sessionId="), // empty sessionId
            };

            let payload = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "ping"
            });

            let res = client
                .post(url)
                .header("Authorization", format!("Bearer {auth_token}"))
                .json(&payload)
                .send()
                .await
                .expect("send invalid session request");

            let status = res.status();
            match i % 4 {
                0 | 1 | 3 => assert_eq!(status, StatusCode::BAD_REQUEST),
                2 => assert_eq!(status, StatusCode::NOT_FOUND),
                _ => unreachable!(),
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.expect("task panicked");
    }
}

// ============================================================================
// SCENARIO 3: Cross-Transport Mixed Load & Container Orchestration Health
// ============================================================================

#[tokio::test]
async fn test_stress_cross_transport_saturation_with_health_probe() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let total_tasks = 80;
    let mut handles = Vec::with_capacity(total_tasks);

    let client = server.client.clone();
    let base_url = server.base_url.clone();
    let auth_token = server.auth_token.clone();
    let mock_url = mock.url("/article-simple");

    // Launch heavy mixed operations across MCP, REST, and OpenAPI
    for i in 0..total_tasks {
        let client = client.clone();
        let base_url = base_url.clone();
        let auth_token = auth_token.clone();
        let mock_url = mock_url.clone();

        let handle = tokio::spawn(async move {
            match i % 4 {
                0 => {
                    // Streamable HTTP MCP
                    let payload = json!({
                        "jsonrpc": "2.0",
                        "id": i,
                        "method": "tools/list"
                    });
                    let res = client
                        .post(format!("{base_url}/mcp"))
                        .header("Authorization", format!("Bearer {auth_token}"))
                        .json(&payload)
                        .send()
                        .await
                        .expect("mcp request");
                    assert_eq!(res.status(), StatusCode::OK);
                }
                1 => {
                    // REST Web to Markdown
                    let res = client
                        .post(format!("{base_url}/api/web/markdown"))
                        .header("Authorization", format!("Bearer {auth_token}"))
                        .json(&json!({ "url": mock_url }))
                        .send()
                        .await
                        .expect("rest markdown request");
                    assert_eq!(res.status(), StatusCode::OK);
                }
                2 => {
                    // GET OpenAPI spec
                    let res = client
                        .get(format!("{base_url}/openapi.json"))
                        .header("Authorization", format!("Bearer {auth_token}"))
                        .send()
                        .await
                        .expect("openapi request");
                    assert_eq!(res.status(), StatusCode::OK);
                }
                _ => {
                    // MCP ping
                    let payload = json!({
                        "jsonrpc": "2.0",
                        "id": i,
                        "method": "ping"
                    });
                    let res = client
                        .post(format!("{base_url}/mcp"))
                        .header("Authorization", format!("Bearer {auth_token}"))
                        .json(&payload)
                        .send()
                        .await
                        .expect("ping request");
                    assert_eq!(res.status(), StatusCode::OK);
                }
            }
        });
        handles.push(handle);
    }

    // Simultaneously probe /health 30 times and measure latency
    let mut health_handles = Vec::new();
    for _ in 0..30 {
        let client = client.clone();
        let base_url = base_url.clone();

        let h = tokio::spawn(async move {
            let start = Instant::now();
            let res = client
                .get(format!("{base_url}/health"))
                .send()
                .await
                .expect("health probe");
            let latency = start.elapsed();

            assert_eq!(res.status(), StatusCode::OK);
            let body: Value = res.json().await.expect("health json");
            assert_eq!(body, json!({ "ok": true }));
            assert!(
                latency < Duration::from_millis(50),
                "Health probe latency must remain strictly under 50ms (got: {:?})",
                latency
            );
        });
        health_handles.push(h);
    }

    for h in handles {
        h.await.expect("load task panicked");
    }
    for h in health_handles {
        h.await.expect("health task panicked");
    }
}
