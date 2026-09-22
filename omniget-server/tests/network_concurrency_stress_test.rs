//! Empirical Stress & Adversarial Concurrency Test Suite for Milestone 1.
//!
//! Covers:
//! 1. High-concurrency flood (50+ to 150+ simultaneous requests mixing /health and protected routes)
//! 2. Unconditional CORS preflight OPTIONS requests across public and protected endpoints
//! 3. Edge-case HTTP headers (huge headers, unusual Unicode, multiple Authorization headers, null bytes)
//! 4. Graceful server drop and socket lifecycle (no orphaned listening sockets, shutdown signals)
//! 5. Error isolation (abrupt socket drops, malformed byte streams, raw TCP pipelining)

mod common;

use common::TestServer;
use reqwest::{header, StatusCode};
use serde_json::Value;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// =========================================================================
// 1. HIGH-CONCURRENCY REQUEST FLOOD (50+ SIMULTANEOUS REQUESTS)
// =========================================================================

#[tokio::test]
async fn test_concurrency_flood_100_mixed_requests() {
    let token = "your-concurrency-token";
    let server = TestServer::spawn_with_token(token).await;
    let base_url = server.base_url.clone();

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(100)
        .timeout(Duration::from_secs(5))
        .build()
        .expect("Failed to create reqwest client");

    let total_tasks = 120; // Well above the 50+ requirement
    let mut handles = Vec::with_capacity(total_tasks);

    for i in 0..total_tasks {
        let client = client.clone();
        let base_url = base_url.clone();
        let token = token.to_string();

        let handle = tokio::spawn(async move {
            match i % 4 {
                // Category 0: Public /health without auth -> must return 200 OK
                0 => {
                    let res = client
                        .get(format!("{base_url}/health"))
                        .send()
                        .await
                        .expect("Health request failed");
                    assert_eq!(res.status(), StatusCode::OK);
                    let body: Value = res.json().await.expect("Parse health json");
                    assert_eq!(body, serde_json::json!({ "ok": true }));
                }
                // Category 1: Protected route with valid token -> must return success (200 or 202)
                1 => {
                    let res = client
                        .get(format!("{base_url}/openapi.json"))
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .send()
                        .await
                        .expect("Openapi request failed");
                    assert_eq!(res.status(), StatusCode::OK);
                }
                // Category 2: Protected route without auth -> must return 401 UNAUTHORIZED
                2 => {
                    let res = client
                        .post(format!("{base_url}/mcp"))
                        .send()
                        .await
                        .expect("Unauthenticated mcp request failed");
                    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
                    let body: Value = res.json().await.expect("Parse 401 json");
                    assert_eq!(body["ok"], false);
                    assert_eq!(body["code"], "UNAUTHORIZED");
                }
                // Category 3: Protected route with invalid token -> must return 401 UNAUTHORIZED
                _ => {
                    let res = client
                        .get(format!("{base_url}/api/web/markdown"))
                        .header(header::AUTHORIZATION, "Bearer bogus-token-bad")
                        .send()
                        .await
                        .expect("Bogus token request failed");
                    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
                    assert_eq!(
                        res.headers()
                            .get(header::WWW_AUTHENTICATE)
                            .unwrap()
                            .to_str()
                            .unwrap(),
                        "Bearer"
                    );
                }
            }
        });
        handles.push(handle);
    }

    // Await all concurrent tasks; none should fail or panic
    for handle in handles {
        handle.await.expect("Concurrent task panicked or failed");
    }
}

#[tokio::test]
async fn test_concurrency_sustained_health_probe_under_load() {
    let token = "health-under-load-token";
    let server = TestServer::spawn_with_token(token).await;
    let base_url = server.base_url.clone();
    let client = reqwest::Client::new();

    // 50 parallel requests exclusively to /health
    let handles: Vec<_> = (0..50)
        .map(|_| {
            let client = client.clone();
            let base_url = base_url.clone();
            tokio::spawn(async move {
                let res = client
                    .get(format!("{base_url}/health"))
                    .send()
                    .await
                    .expect("Failed to send health request");
                assert_eq!(res.status(), StatusCode::OK);
            })
        })
        .collect();

    for h in handles {
        h.await.unwrap();
    }
}

// =========================================================================
// 2. UNCONDITIONAL CORS PREFLIGHT (OPTIONS) REQUESTS
// =========================================================================

#[tokio::test]
async fn test_cors_options_preflight_unconditional_across_all_routes() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();

    let routes = [
        "/health",
        "/mcp",
        "/sse",
        "/messages",
        "/openapi.json",
        "/api/x/post",
        "/api/x/thread",
        "/api/web/markdown",
        "/api/pdf/text",
        "/api/media/info",
    ];

    let test_origins = [
        "https://chatgpt.com",
        "https://claude.ai",
        "http://localhost:3000",
        "https://example.org",
    ];

    for route in routes {
        for origin in test_origins {
            let res = client
                .request(reqwest::Method::OPTIONS, format!("{}{route}", server.base_url))
                .header(header::ORIGIN, origin)
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "authorization, content-type, x-custom-trace",
                )
                .send()
                .await
                .unwrap_or_else(|e| panic!("OPTIONS {route} from {origin} failed: {e}"));

            // Must succeed with 200 OK unconditionally (WITHOUT ANY AUTHORIZATION HEADER)
            assert_eq!(
                res.status(),
                StatusCode::OK,
                "OPTIONS {route} must return 200 OK without auth"
            );

            // Access-Control-Allow-Origin must be present
            let allow_origin = res
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap_or_else(|| panic!("Missing Access-Control-Allow-Origin on {route}"));
            assert!(
                allow_origin == "*" || allow_origin == origin,
                "Unexpected Access-Control-Allow-Origin: {allow_origin:?}"
            );

            // Access-Control-Allow-Methods must be present and contain POST or GET
            assert!(
                res.headers().contains_key(header::ACCESS_CONTROL_ALLOW_METHODS),
                "Missing Access-Control-Allow-Methods on {route}"
            );
        }
    }
}

// =========================================================================
// 3. EDGE-CASE HTTP HEADERS
// =========================================================================

#[tokio::test]
async fn test_edge_case_huge_authorization_header() {
    let token = "expected-secret-token";
    let server = TestServer::spawn_with_token(token).await;
    let client = reqwest::Client::new();

    // 8KB huge token header
    let huge_token = "A".repeat(8192);
    let auth_val = format!("Bearer {huge_token}");

    let res = client
        .get(format!("{}/openapi.json", server.base_url))
        .header(header::AUTHORIZATION, auth_val)
        .send()
        .await
        .expect("Huge header request failed");

    // Server must reject cleanly with 401 Unauthorized or 431, never panic
    assert!(
        res.status() == StatusCode::UNAUTHORIZED || res.status() == StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
        "Expected 401 or 431 for huge token, got {}",
        res.status()
    );
}

#[tokio::test]
async fn test_edge_case_unusual_characters_in_authorization() {
    let token = "valid-secret-42";
    let server = TestServer::spawn_with_token(token).await;
    let client = reqwest::Client::new();

    let adversarial_headers = [
        "Bearer   ",                                      // whitespace only after Bearer
        "Bearer \t\t  ",                                  // tab characters
        "Bearer valid-secret-42\x00extra",                // embedded null byte if parsed
        "Bearer valid-secret-42-suffix",                  // longer prefix match
        "Bearer valid-secre",                             // shorter prefix match
        "Bearer ~!@#$%^&*()_+`-={}|[]\\:\";'<>?,./",      // special symbols
        "Bearer   valid-secret-42   extra",               // interior whitespace
        "Bearer\tvalid-secret-42",                        // tab separator instead of space
        "Bearer\r\nvalid-secret-42",                      // crlf injection attempt (reqwest rejects or server rejects)
    ];

    for val in adversarial_headers {
        // reqwest header value parsing might reject crlf, which is also a safe outcome
        if let Ok(hdr) = header::HeaderValue::from_str(val) {
            let res = client
                .get(format!("{}/openapi.json", server.base_url))
                .header(header::AUTHORIZATION, hdr)
                .send()
                .await
                .expect("Request with adversarial header failed");

            assert_eq!(
                res.status(),
                StatusCode::UNAUTHORIZED,
                "Adversarial header '{val}' must be rejected with 401"
            );
        }
    }
}

#[tokio::test]
async fn test_edge_case_multiple_authorization_headers() {
    let token = "valid-secret-42";
    let server = TestServer::spawn_with_token(token).await;

    // Send raw HTTP request with duplicate Authorization headers
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", server.port))
        .await
        .expect("Failed to connect via raw TCP");

    // Two headers: first invalid, second valid
    let req = format!(
        "GET /openapi.json HTTP/1.1\r\n\
         Host: 127.0.0.1:{}\r\n\
         Authorization: Bearer invalid-first-token\r\n\
         Authorization: Bearer {}\r\n\
         Connection: close\r\n\r\n",
        server.port, token
    );

    stream.write_all(req.as_bytes()).await.unwrap();

    let mut response_bytes = Vec::new();
    stream.read_to_end(&mut response_bytes).await.unwrap();
    let response_str = String::from_utf8_lossy(&response_bytes);

    // Axum/HeaderMap::get returns the first header value, which is invalid, so it must return 401
    assert!(
        response_str.contains("401 Unauthorized"),
        "Duplicate headers with invalid first must reject with 401. Got:\n{response_str}"
    );

    // Now test with first valid, second invalid
    let mut stream2 = TcpStream::connect(format!("127.0.0.1:{}", server.port))
        .await
        .expect("Failed to connect via raw TCP");

    let req2 = format!(
        "GET /openapi.json HTTP/1.1\r\n\
         Host: 127.0.0.1:{}\r\n\
         Authorization: Bearer {}\r\n\
         Authorization: Bearer invalid-second-token\r\n\
         Connection: close\r\n\r\n",
        server.port, token
    );

    stream2.write_all(req2.as_bytes()).await.unwrap();

    let mut response_bytes2 = Vec::new();
    stream2.read_to_end(&mut response_bytes2).await.unwrap();
    let response_str2 = String::from_utf8_lossy(&response_bytes2);

    // Should return 200 OK or 401 without crashing or panicking
    assert!(
        response_str2.contains("200 OK") || response_str2.contains("401 Unauthorized"),
        "Duplicate headers must not cause internal server errors. Got:\n{response_str2}"
    );
}

// =========================================================================
// 4. GRACEFUL SERVER DROP & SOCKET LIFECYCLE
// =========================================================================

#[tokio::test]
async fn test_socket_lifecycle_clean_shutdown_on_drop() {
    let server = TestServer::spawn().await;
    let port = server.port;

    // Verify it is responding
    let res = reqwest::get(format!("http://127.0.0.1:{port}/health"))
        .await
        .expect("Initial health check failed");
    assert_eq!(res.status(), StatusCode::OK);

    // Explicitly drop server, which fires shutdown_tx
    drop(server);

    // Allow graceful shutdown future to resolve and close the socket
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Subsequent connection attempt MUST fail with ConnectionRefused or reset
    let connect_res = TcpStream::connect(format!("127.0.0.1:{port}")).await;
    assert!(
        connect_res.is_err(),
        "Socket on port {port} must be closed after server drop"
    );
}

// =========================================================================
// 5. ERROR ISOLATION & RAW SOCKET ROBUSTNESS
// =========================================================================

#[tokio::test]
async fn test_error_isolation_malformed_bytes_do_not_affect_concurrent_clients() {
    let token = "error-isolation-token";
    let server = TestServer::spawn_with_token(token).await;
    let port = server.port;

    // Client A: Sends malformed junk data on a raw TCP socket
    let junk_task = tokio::spawn(async move {
        if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{port}")).await {
            let garbage = b"INVALID_PROTOCOL_JUNK_0123456789\xff\xfe\x00\x01\r\n\r\n";
            let _ = stream.write_all(garbage).await;
            let mut buf = [0u8; 256];
            let _ = stream.read(&mut buf).await;
        }
    });

    // Client B: Concurrently sends valid requests to /health and protected routes
    let client = reqwest::Client::new();
    let health_url = format!("http://127.0.0.1:{port}/health");
    let openapi_url = format!("http://127.0.0.1:{port}/openapi.json");

    for _ in 0..10 {
        let res = client
            .get(&health_url)
            .send()
            .await
            .expect("Health check failed during junk attack");
        assert_eq!(res.status(), StatusCode::OK);

        let res_authed = client
            .get(&openapi_url)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .send()
            .await
            .expect("Authed check failed during junk attack");
        assert_eq!(res_authed.status(), StatusCode::OK);
    }

    junk_task.await.unwrap();
}

#[tokio::test]
async fn test_error_isolation_abrupt_client_socket_disconnect() {
    let server = TestServer::spawn().await;
    let port = server.port;

    // Connect raw socket, send partial HTTP request header, then immediately close connection
    for _ in 0..15 {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .expect("Connect failed");
        stream
            .write_all(b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 100\r\n")
            .await
            .unwrap();
        // Drop stream abruptly without sending remaining headers or body
        drop(stream);
    }

    // Verify server remains 100% healthy and handles subsequent normal requests
    let client = reqwest::Client::new();
    let res = client
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await
        .expect("Server failed to respond after abrupt disconnects");
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_http_keep_alive_connection_reuse_and_pipelining() {
    let token = "pipelining-test-token";
    let server = TestServer::spawn_with_token(token).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", server.port))
        .await
        .expect("TCP connect failed");

    // Send two sequential requests over the same persistent HTTP/1.1 connection
    let pipelined_reqs = format!(
        "GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n\
         GET /openapi.json HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n",
        port = server.port
    );

    stream.write_all(pipelined_reqs.as_bytes()).await.unwrap();

    let mut response_buf = vec![0u8; 4096];
    let n = stream.read(&mut response_buf).await.unwrap();
    let response_text = String::from_utf8_lossy(&response_buf[..n]);

    // Should receive first response (200 OK for /health)
    assert!(
        response_text.contains("200 OK"),
        "First request must be 200 OK: {response_text}"
    );

    // Read remaining bytes if second response wasn't in first read
    let mut full_text = response_text.to_string();
    if !full_text.contains("401 Unauthorized") {
        let n2 = stream.read(&mut response_buf).await.unwrap();
        full_text.push_str(&String::from_utf8_lossy(&response_buf[..n2]));
    }

    // Second request was unauthenticated to /openapi.json -> must be 401 Unauthorized
    assert!(
        full_text.contains("401 Unauthorized"),
        "Second request must be 401 Unauthorized: {full_text}"
    );
}

#[tokio::test]
async fn test_concurrency_burst_200_parallel_tasks() {
    let token = "token-burst-200";
    let server = TestServer::spawn_with_token(token).await;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(200)
        .build()
        .unwrap();

    let mut handles = Vec::with_capacity(200);
    for i in 0..200 {
        let client = client.clone();
        let base_url = server.base_url.clone();
        let token = token.to_string();
        handles.push(tokio::spawn(async move {
            if i % 2 == 0 {
                let res = client
                    .get(format!("{base_url}/health"))
                    .send()
                    .await
                    .expect("Health request in burst failed");
                assert_eq!(res.status(), StatusCode::OK);
            } else {
                let res = client
                    .get(format!("{base_url}/openapi.json"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .send()
                    .await
                    .expect("Protected request in burst failed");
                assert_eq!(res.status(), StatusCode::OK);
            }
        }));
    }

    for h in handles {
        h.await.expect("Burst task panicked");
    }
}

#[tokio::test]
async fn test_http_10_compatibility() {
    let server = TestServer::spawn().await;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", server.port))
        .await
        .expect("Connect failed");

    // HTTP/1.0 request without Host header
    let req = "GET /health HTTP/1.0\r\n\r\n";
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let res = String::from_utf8_lossy(&buf);
    assert!(res.contains("200 OK"), "HTTP/1.0 request must receive 200 OK. Got:\n{res}");
}

#[tokio::test]
async fn test_port_collision_error_handling() {
    // Bind a listener first to hold a port
    let first_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind first listener");
    let occupied_addr = first_listener.local_addr().expect("local addr");

    // Attempt to bind another listener on the exact same port
    let collision_result = omniget_server::server::bind_listener(occupied_addr).await;
    assert!(
        collision_result.is_err(),
        "Binding to an occupied port must return an Err"
    );
    let err = collision_result.err().unwrap();
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::AddrInUse,
        "Expected AddrInUse error, got: {err:?}"
    );
}

#[tokio::test]
async fn test_active_connection_during_server_drop() {
    let server = TestServer::spawn().await;
    let port = server.port;

    // Connect raw socket and leave it idle
    let _idle_stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("Connect failed");

    // Drop server while connection is held
    drop(server);

    // Ensure dropping server completes quickly and does not hang
    tokio::time::sleep(Duration::from_millis(100)).await;

    // New connection attempts should fail
    let attempt = TcpStream::connect(format!("127.0.0.1:{port}")).await;
    assert!(attempt.is_err(), "Server socket should be closed after drop");
}

#[tokio::test]
async fn test_cors_options_with_null_and_custom_origins() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();

    for origin in ["null", "chrome-extension://xyz", "vscode-webview://abc"] {
        let res = client
            .request(reqwest::Method::OPTIONS, format!("{}/health", server.base_url))
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    }
}

#[tokio::test]
async fn test_non_ascii_header_bytes_rejected_safely() {
    let server = TestServer::spawn().await;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", server.port))
        .await
        .unwrap();

    // Raw bytes containing non-ASCII / non-UTF8 in Authorization header
    let raw_req = b"GET /openapi.json HTTP/1.1\r\n\
                    Host: 127.0.0.1\r\n\
                    Authorization: Bearer \xff\xfe\xfd\r\n\
                    Connection: close\r\n\r\n";
    stream.write_all(raw_req).await.unwrap();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let res = String::from_utf8_lossy(&buf);

    // Must return 401 Unauthorized or 400 Bad Request, never 500 or crash
    assert!(
        res.contains("401 Unauthorized") || res.contains("400 Bad Request"),
        "Invalid header bytes must reject cleanly. Got:\n{res}"
    );
}

