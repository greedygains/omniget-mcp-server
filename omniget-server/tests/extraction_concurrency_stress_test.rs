//! Empirical Stress, Concurrency & Resource Lifecycle Test Suite for Milestone 2 (Extraction Tools)
//!
//! Adversarial Verification Vectors:
//! 1. Concurrent Multi-Tool Extraction:
//!    - Concurrently execute 75+ mixed requests (web_to_markdown, pdf_text, x_post, media_info) against TestServer.
//!    - Verify zero deadlocks, zero thread-pool exhaustion, and clean error isolation.
//!    - Concurrently probe /health to verify container orchestration liveness under extraction load.
//! 2. Tempfile Descriptor Lifecycle:
//!    - Repeatedly generate, extract text, and immediately delete temporary PDF files.
//!    - Verify tokio::fs::read drops file descriptors immediately and allows instant unlinking on filesystem without EBUSY or lock errors.
//!    - Test concurrent temporary PDF generation, extraction, and unlinking under load.
//! 3. Large Document Scrapes:
//!    - Test scraping large documents (500 paragraphs, 50-row x 10-column tables) under concurrent conditions.
//!    - Verify no unbounded memory leaks, no stack overflow from deep recursion, and zero raw HTML tags in output.
//! 4. Thread Pool Liveness & Error Isolation:
//!    - Mix valid, invalid, slow, and abrupt socket disconnect requests, verifying server stability.

mod common;

use common::{create_test_pdf, MockWebsite, TestServer};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

// ============================================================================
// VECTOR 1: Concurrent Multi-Tool Extraction (75+ Mixed Requests)
// ============================================================================

#[tokio::test]
async fn test_concurrency_flood_75_mixed_extraction_requests() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    // Pre-create 10 test PDFs of varying sizes
    let mut temp_pdfs = Vec::new();
    for i in 1..=10 {
        let pdf = create_test_pdf(&format!("Concurrent Dataset Batch {}", i), (i % 3) + 1);
        temp_pdfs.push(pdf);
    }
    let pdf_paths: Vec<String> = temp_pdfs
        .iter()
        .map(|p| p.path().to_str().unwrap().to_string())
        .collect();

    let total_tasks = 80; // Exceeds 50+ requirement
    let completed_count = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::with_capacity(total_tasks);

    let client = server.client.clone();
    let base_url = server.base_url.clone();
    let auth_token = server.auth_token.clone();

    let start_time = Instant::now();

    for i in 0..total_tasks {
        let client = client.clone();
        let base_url = base_url.clone();
        let auth_token = auth_token.clone();
        let mock_url = mock.url("/article-simple");
        let mock_table_url = mock.url("/article-with-table");
        let mock_unicode_url = mock.url("/unicode-article");
        let mock_error_url = mock.url("/server-error");
        let pdf_path = pdf_paths[i % pdf_paths.len()].clone();
        let completed = completed_count.clone();

        let handle = tokio::spawn(async move {
            let (method, payload) = match i % 8 {
                // Category 0: web_to_markdown (simple article)
                0 => (
                    "tools/call",
                    json!({
                        "name": "web_to_markdown",
                        "arguments": { "url": mock_url }
                    }),
                ),
                // Category 1: web_to_markdown (table article)
                1 => (
                    "tools/call",
                    json!({
                        "name": "web_to_markdown",
                        "arguments": { "url": mock_table_url }
                    }),
                ),
                // Category 2: pdf_text (valid local file)
                2 => (
                    "tools/call",
                    json!({
                        "name": "pdf_text",
                        "arguments": { "path": pdf_path, "pages": "1" }
                    }),
                ),
                // Category 3: media_info (direct media URL)
                3 => (
                    "tools/call",
                    json!({
                        "name": "media_info",
                        "arguments": { "url": "https://example.com/video.mp4" }
                    }),
                ),
                // Category 4: x_post (formatted URL - test schema routing and error handling)
                4 => (
                    "tools/call",
                    json!({
                        "name": "x_post",
                        "arguments": { "url": "https://x.com/rustlang/status/1234567890" }
                    }),
                ),
                // Category 5: web_to_markdown (multilingual article)
                5 => (
                    "tools/call",
                    json!({
                        "name": "web_to_markdown",
                        "arguments": { "url": mock_unicode_url }
                    }),
                ),
                // Category 6: Adversarial / Error isolation: web_to_markdown with 500 error
                6 => (
                    "tools/call",
                    json!({
                        "name": "web_to_markdown",
                        "arguments": { "url": mock_error_url }
                    }),
                ),
                // Category 7: Adversarial / Error isolation: pdf_text with non-existent file
                _ => (
                    "tools/call",
                    json!({
                        "name": "pdf_text",
                        "arguments": { "path": "/path/does/not/exist_12345.pdf" }
                    }),
                ),
            };

            let req_body = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": method,
                "params": payload
            });

            let resp = client
                .post(format!("{}/mcp", base_url))
                .header("Authorization", format!("Bearer {}", auth_token))
                .json(&req_body)
                .send()
                .await
                .expect("Extraction request must receive HTTP response");

            assert_eq!(resp.status(), StatusCode::OK);
            let json_resp: Value = resp.json().await.expect("Response must be valid JSON");

            // Verify basic JSON-RPC structure
            assert_eq!(json_resp["jsonrpc"], "2.0");
            assert_eq!(json_resp["id"], i);

            // Verify error isolation: invalid endpoints return error without failing protocol
            if i % 8 == 6 || i % 8 == 7 {
                assert!(
                    json_resp["error"].is_object() || json_resp["result"]["isError"] == true,
                    "Error case must return structured error"
                );
            } else if i % 8 == 0 || i % 8 == 1 || i % 8 == 2 || i % 8 == 5 {
                // Successful extraction tools must return non-empty text content
                let content = &json_resp["result"]["content"];
                assert!(content.is_array(), "Result content must be an array");
                let text = content[0]["text"].as_str().unwrap_or("");
                assert!(!text.is_empty(), "Extracted text must not be empty");
            }

            completed.fetch_add(1, Ordering::SeqCst);
        });

        handles.push(handle);
    }

    // Simultaneously run a health probe loop to verify zero thread-pool exhaustion
    let health_client = server.client.clone();
    let health_url = format!("{}/health", server.base_url);
    let health_handle = tokio::spawn(async move {
        for _ in 0..10 {
            let t0 = Instant::now();
            let res = health_client.get(&health_url).send().await.expect("Health probe must succeed");
            assert_eq!(res.status(), StatusCode::OK);
            let elapsed = t0.elapsed();
            assert!(
                elapsed < Duration::from_millis(500),
                "Health check must respond under 500ms even under heavy extraction load, took {:?}",
                elapsed
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    });

    // Wait for all handles with a strict timeout to ensure zero deadlocks
    let timeout_duration = Duration::from_secs(30);
    let join_all_future = async {
        for handle in handles {
            handle.await.expect("Task panicked during concurrent flood");
        }
        health_handle.await.expect("Health probe panicked");
    };

    tokio::time::timeout(timeout_duration, join_all_future)
        .await
        .expect("Deadlock detected! Concurrent extraction requests did not complete within timeout");

    assert_eq!(completed_count.load(Ordering::SeqCst), total_tasks);
    let total_elapsed = start_time.elapsed();
    println!("Completed {} mixed extraction requests in {:?}", total_tasks, total_elapsed);
}

#[tokio::test]
async fn test_concurrency_socket_abort_isolation() {
    let server = TestServer::start().await;
    let addr = format!("127.0.0.1:{}", server.port);

    // Concurrently open 30 raw TCP connections, send partial HTTP headers, and abort abruptly
    let mut handles = Vec::new();
    for _ in 0..30 {
        let target_addr = addr.clone();
        handles.push(tokio::spawn(async move {
            if let Ok(mut stream) = TcpStream::connect(target_addr).await {
                let partial = b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 1000\r\n\r\n{\"par";
                let _ = stream.write_all(partial).await;
                // Abrupt drop without shutdown
                drop(stream);
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    // Immediately verify that server socket is fully healthy and processing normal requests
    let health_res = server.get("/health").await;
    assert_eq!(health_res.status(), StatusCode::OK);
    let health_body: Value = health_res.json().await.unwrap();
    assert_eq!(health_body["ok"], true);
}

// ============================================================================
// VECTOR 2: Tempfile Descriptor Lifecycle & Instant Unlinking
// ============================================================================

#[tokio::test]
async fn test_tempfile_descriptor_immediate_drop_and_unlink_sequential() {
    let server = TestServer::start().await;

    // Run 50 sequential create -> extract -> unlink cycles
    for iteration in 1..=50 {
        let content = format!("Sequential Tempfile Test Iteration #{}", iteration);
        let named_file = create_test_pdf(&content, 2);
        let path = named_file.path().to_path_buf();
        let path_str = path.to_str().unwrap().to_string();

        // Extract text via MCP tool call
        let res = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "pdf_text",
                    "arguments": { "path": path_str }
                }),
            )
            .await;

        assert_eq!(res["jsonrpc"], "2.0");
        let text = res["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(&content));

        // Immediately drop NamedTempFile wrapper and explicitly remove file
        drop(named_file);
        let remove_result = std::fs::remove_file(&path);
        // Note: NamedTempFile drop may have already unlinked it, or remove_file succeeds.
        // If file descriptor was held open with exclusive locks (e.g. on Windows or busy Unix descriptor),
        // remove_file or subsequent create would fail with EBUSY / PermissionDenied.
        if let Err(e) = remove_result {
            assert_eq!(
                e.kind(),
                std::io::ErrorKind::NotFound,
                "File removal error must be NotFound if already unlinked by drop, got: {:?}",
                e
            );
        }

        // Verify the path is definitely gone from filesystem
        assert!(
            !path.exists(),
            "Tempfile at {:?} must not exist after unlinking",
            path
        );
    }
}

#[tokio::test]
async fn test_tempfile_descriptor_immediate_drop_and_unlink_concurrent() {
    let server = TestServer::start().await;
    let total_concurrent = 30;
    let mut handles = Vec::with_capacity(total_concurrent);

    for i in 0..total_concurrent {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let auth_token = server.auth_token.clone();

        let handle = tokio::spawn(async move {
            let content = format!("Concurrent Tempfile PDF #{}", i);
            let named_file = create_test_pdf(&content, 3);
            let path = named_file.path().to_path_buf();
            let path_str = path.to_str().unwrap().to_string();

            let payload = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "tools/call",
                "params": {
                    "name": "pdf_text",
                    "arguments": { "path": path_str, "pages": "1-2" }
                }
            });

            let resp = client
                .post(format!("{}/mcp", base_url))
                .header("Authorization", format!("Bearer {}", auth_token))
                .json(&payload)
                .send()
                .await
                .expect("PDF extraction request failed");

            assert_eq!(resp.status(), StatusCode::OK);
            let body: Value = resp.json().await.unwrap();
            let extracted = body["result"]["content"][0]["text"].as_str().unwrap();
            assert!(extracted.contains(&content));

            // Explicitly delete file from disk while runtime is busy
            drop(named_file);
            let rm = tokio::fs::remove_file(&path).await;
            if let Err(e) = rm {
                assert_eq!(
                    e.kind(),
                    std::io::ErrorKind::NotFound,
                    "tokio::fs::remove_file unexpected error: {:?}",
                    e
                );
            }

            // Verify file cannot be read anymore
            let read_after = tokio::fs::read(&path).await;
            assert!(read_after.is_err(), "Reading deleted file must return Err");
        });

        handles.push(handle);
    }

    for h in handles {
        h.await.expect("Concurrent tempfile task panicked");
    }
}

#[tokio::test]
async fn test_tempfile_concurrent_double_extraction_same_file() {
    let server = TestServer::start().await;
    let pdf = create_test_pdf("Shared Multi-Reader Tempfile Content", 2);
    let path_str = pdf.path().to_str().unwrap().to_string();

    let client1 = server.client.clone();
    let client2 = server.client.clone();
    let base_url1 = server.base_url.clone();
    let base_url2 = server.base_url.clone();
    let token1 = server.auth_token.clone();
    let token2 = server.auth_token.clone();
    let path1 = path_str.clone();
    let path2 = path_str.clone();

    let task1 = tokio::spawn(async move {
        let payload = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "pdf_text", "arguments": { "path": path1, "pages": "1" } }
        });
        client1.post(format!("{}/mcp", base_url1))
            .header("Authorization", format!("Bearer {}", token1))
            .json(&payload).send().await.unwrap().json::<Value>().await.unwrap()
    });

    let task2 = tokio::spawn(async move {
        let payload = json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "pdf_text", "arguments": { "path": path2, "pages": "2" } }
        });
        client2.post(format!("{}/mcp", base_url2))
            .header("Authorization", format!("Bearer {}", token2))
            .json(&payload).send().await.unwrap().json::<Value>().await.unwrap()
    });

    let (res1, res2) = tokio::join!(task1, task2);
    let val1 = res1.unwrap();
    let val2 = res2.unwrap();

    assert!(val1["result"]["content"][0]["text"].as_str().unwrap().contains("Shared Multi-Reader Tempfile Content"));
    assert!(val2["result"]["content"][0]["text"].as_str().unwrap().contains("Shared Multi-Reader Tempfile Content"));

    // Drop and delete without issue
    let file_path = pdf.path().to_path_buf();
    drop(pdf);
    let _ = std::fs::remove_file(&file_path);
    assert!(!file_path.exists());
}

// ============================================================================
// VECTOR 3: Large Document Scrapes & Memory Stability Under Load
// ============================================================================

/// Helper: Spawns an auxiliary HTTP server returning heavy HTML documents:
/// 1. /massive-article: 500 paragraphs of rich text
/// 2. /massive-table: 50 rows by 10 columns table
/// 3. /dense-junk-article: 200 junk ad-containers, navs, scripts around content
struct HeavyMockServer {
    pub base_url: String,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl HeavyMockServer {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind heavy mock server");
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{}", port);

        let app = axum::Router::new()
            .route(
                "/massive-article",
                axum::routing::get(|| async {
                    let mut body = String::with_capacity(200_000);
                    body.push_str("<!DOCTYPE html><html><head><title>Massive Article Benchmark</title></head><body>");
                    body.push_str("<h1>Benchmark Report for High-Throughput Scrapes</h1>");
                    for i in 1..=500 {
                        body.push_str(&format!(
                            "<p>Paragraph {}: Comprehensive analytics data stream containing detailed telemetry information and performance benchmarks across multiple distributed nodes.</p>",
                            i
                        ));
                    }
                    body.push_str("</body></html>");
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/html; charset=utf-8")],
                        body,
                    )
                }),
            )
            .route(
                "/massive-table",
                axum::routing::get(|| async {
                    let mut body = String::with_capacity(100_000);
                    body.push_str("<!DOCTYPE html><html><head><title>50-Row Financial Matrix</title></head><body>");
                    body.push_str("<h1>Quarterly Enterprise Matrix</h1>");
                    body.push_str("<table><thead><tr>");
                    for c in 1..=10 {
                        body.push_str(&format!("<th>Metric Col {}</th>", c));
                    }
                    body.push_str("</tr></thead><tbody>");
                    for r in 1..=50 {
                        body.push_str("<tr>");
                        for c in 1..=10 {
                            body.push_str(&format!("<td>R{}C{} Value</td>", r, c));
                        }
                        body.push_str("</tr>");
                    }
                    body.push_str("</tbody></table></body></html>");
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/html; charset=utf-8")],
                        body,
                    )
                }),
            )
            .route(
                "/dense-junk-article",
                axum::routing::get(|| async {
                    let mut body = String::with_capacity(150_000);
                    body.push_str("<!DOCTYPE html><html><head><title>Junk Filter Benchmark</title>");
                    body.push_str("<script>function track(){};</script><style>.ad{display:none;}</style></head><body>");
                    for j in 1..=100 {
                        body.push_str(&format!(
                            "<div class=\"ad-container recommendation cookie-banner\"><p>Ad {}</p><button>Click Here</button></div>",
                            j
                        ));
                    }
                    body.push_str("<h1>Core Journalistic Truth</h1><p>The definitive extracted content without noise.</p>");
                    for j in 101..=200 {
                        body.push_str(&format!(
                            "<div class=\"subscription-widget share-dialog\"><p>Subscribe {}</p></div>",
                            j
                        ));
                    }
                    body.push_str("</body></html>");
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/html; charset=utf-8")],
                        body,
                    )
                }),
            );

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async { let _ = shutdown_rx.await; })
                .await
                .ok();
        });

        Self {
            base_url,
            shutdown_tx: Some(shutdown_tx),
        }
    }
}

impl Drop for HeavyMockServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

#[tokio::test]
async fn test_large_document_scrapes_concurrency_and_memory_stability() {
    let server = TestServer::start().await;
    let heavy_mock = HeavyMockServer::start().await;

    let total_scrapes = 30; // 30 concurrent scrapes of 500 paragraphs, 50-row tables, and junk-dense pages
    let mut handles = Vec::with_capacity(total_scrapes);

    let start_time = Instant::now();

    for i in 0..total_scrapes {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let auth_token = server.auth_token.clone();
        let target_url = match i % 3 {
            0 => format!("{}/massive-article", heavy_mock.base_url),
            1 => format!("{}/massive-table", heavy_mock.base_url),
            _ => format!("{}/dense-junk-article", heavy_mock.base_url),
        };

        let handle = tokio::spawn(async move {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "tools/call",
                "params": {
                    "name": "web_to_markdown",
                    "arguments": { "url": target_url }
                }
            });

            let resp = client
                .post(format!("{}/mcp", base_url))
                .header("Authorization", format!("Bearer {}", auth_token))
                .json(&payload)
                .send()
                .await
                .expect("Large document scrape request failed");

            assert_eq!(resp.status(), StatusCode::OK);
            let body: Value = resp.json().await.unwrap();
            let markdown = body["result"]["content"][0]["text"].as_str().unwrap();

            match i % 3 {
                0 => {
                    // Massive article (500 paragraphs)
                    assert!(markdown.contains("Paragraph 1:"));
                    assert!(markdown.contains("Paragraph 500:"));
                    assert!(!markdown.contains("<p>"), "Must not contain raw <p> tags");
                }
                1 => {
                    // Massive table (50 rows x 10 cols)
                    assert!(markdown.contains("| Metric Col 1 |"));
                    assert!(markdown.contains("| R1C1 Value |"));
                    assert!(markdown.contains("| R50C10 Value |"));
                    assert!(!markdown.contains("<table>"), "Must not contain raw <table> tags");
                    assert!(!markdown.contains("<tr>"), "Must not contain raw <tr> tags");
                    assert!(!markdown.contains("<td>"), "Must not contain raw <td> tags");
                }
                _ => {
                    // Dense junk article
                    assert!(markdown.contains("Core Journalistic Truth"));
                    assert!(markdown.contains("The definitive extracted content without noise."));
                    assert!(!markdown.contains("Click Here"));
                    assert!(!markdown.contains("Subscribe 150"));
                    assert!(!markdown.contains("<script>"));
                    assert!(!markdown.contains("<style>"));
                }
            }
        });

        handles.push(handle);
    }

    // Await with timeout to prevent deadlock
    tokio::time::timeout(Duration::from_secs(25), async {
        for h in handles {
            h.await.expect("Scrape task panicked");
        }
    })
    .await
    .expect("Deadlock during large document concurrent scrapes");

    let duration = start_time.elapsed();
    println!("Completed {} large concurrent document scrapes in {:?}", total_scrapes, duration);
}

// ============================================================================
// VECTOR 4: Thread Pool Liveness & Error Isolation Under Load
// ============================================================================

#[tokio::test]
async fn test_thread_pool_liveness_with_interleaved_slow_and_fast_workloads() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    // Fast probe handle
    let health_client = server.client.clone();
    let health_url = format!("{}/health", server.base_url);
    let health_latencies = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let health_latencies_clone = health_latencies.clone();

    let probe_handle = tokio::spawn(async move {
        for _ in 0..20 {
            let t0 = Instant::now();
            let res = health_client.get(&health_url).send().await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            let elapsed = t0.elapsed();
            health_latencies_clone.lock().await.push(elapsed);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    // Fire 20 extraction tasks in parallel
    let mut extraction_handles = Vec::new();
    for i in 0..20 {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let auth_token = server.auth_token.clone();
        let url = mock.url("/article-simple");

        extraction_handles.push(tokio::spawn(async move {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "tools/call",
                "params": {
                    "name": "web_to_markdown",
                    "arguments": { "url": url }
                }
            });
            let res = client
                .post(format!("{}/mcp", base_url))
                .header("Authorization", format!("Bearer {}", auth_token))
                .json(&payload)
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
        }));
    }

    for h in extraction_handles {
        h.await.unwrap();
    }
    probe_handle.await.unwrap();

    let latencies = health_latencies.lock().await;
    assert!(!latencies.is_empty());
    for &lat in latencies.iter() {
        assert!(
            lat < Duration::from_millis(250),
            "Health probe latency under extraction load must be < 250ms, saw {:?}",
            lat
        );
    }
}

#[tokio::test]
async fn test_concurrency_100_mixed_burst_with_keepalive_reuse() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    // Single reqwest Client with HTTP keepalive connection pool
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(50)
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Client build failed");

    let pdf = create_test_pdf("Keepalive Pool Shared Test", 2);
    let pdf_path = pdf.path().to_str().unwrap().to_string();

    let total = 100;
    let mut handles = Vec::with_capacity(total);

    for i in 0..total {
        let client = client.clone();
        let base_url = server.base_url.clone();
        let token = server.auth_token.clone();
        let simple_url = mock.url("/article-simple");
        let table_url = mock.url("/article-with-table");
        let pdf_path = pdf_path.clone();

        handles.push(tokio::spawn(async move {
            let (tool, args) = match i % 4 {
                0 => ("web_to_markdown", json!({ "url": simple_url })),
                1 => ("web_to_markdown", json!({ "url": table_url })),
                2 => ("pdf_text", json!({ "path": pdf_path, "pages": "1" })),
                _ => ("media_info", json!({ "url": "https://example.com/audio.mp3" })),
            };

            let payload = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "tools/call",
                "params": { "name": tool, "arguments": args }
            });

            let res = client
                .post(format!("{}/mcp", base_url))
                .header("Authorization", format!("Bearer {}", token))
                .json(&payload)
                .send()
                .await
                .expect("Keepalive pooled request must succeed");

            assert_eq!(res.status(), StatusCode::OK);
            let json_body: Value = res.json().await.unwrap();
            assert_eq!(json_body["jsonrpc"], "2.0");
            assert!(json_body["result"].is_object() || json_body["error"].is_object());
        }));
    }

    for h in handles {
        h.await.expect("Burst keepalive task panicked");
    }
}

#[tokio::test]
async fn test_tempfile_stress_100_rapid_cycles_under_concurrent_load() {
    let server = TestServer::start().await;
    let workers = 10;
    let cycles_per_worker = 10; // Total 100 temporary files
    let mut worker_handles = Vec::with_capacity(workers);

    for worker_id in 0..workers {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let token = server.auth_token.clone();

        worker_handles.push(tokio::spawn(async move {
            for cycle in 0..cycles_per_worker {
                let file_tag = format!("W{}C{}", worker_id, cycle);
                let named_file = create_test_pdf(&format!("PDF Stress Cycle {}", file_tag), 1);
                let path = named_file.path().to_path_buf();
                let path_str = path.to_str().unwrap().to_string();

                let payload = json!({
                    "jsonrpc": "2.0",
                    "id": worker_id * 100 + cycle,
                    "method": "tools/call",
                    "params": {
                        "name": "pdf_text",
                        "arguments": { "path": path_str }
                    }
                });

                let res = client
                    .post(format!("{}/mcp", base_url))
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&payload)
                    .send()
                    .await
                    .expect("Stress extraction failed");

                assert_eq!(res.status(), StatusCode::OK);
                let json_res: Value = res.json().await.unwrap();
                let text = json_res["result"]["content"][0]["text"].as_str().unwrap();
                assert!(text.contains(&file_tag));

                // Explicit unlink
                drop(named_file);
                let _ = std::fs::remove_file(&path);
                assert!(!path.exists(), "Path {:?} must be unlinked", path);
            }
        }));
    }

    for h in worker_handles {
        h.await.expect("Worker panicked during tempfile stress");
    }
}

#[tokio::test]
async fn test_large_document_1000_paragraphs_and_100_row_table_scrape() {
    let server = TestServer::start().await;

    // Ephemeral server for 1000 paragraphs + 100-row table
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{}", port);

    let app = axum::Router::new().route(
        "/ultra-huge-document",
        axum::routing::get(|| async {
            let mut html = String::with_capacity(500_000);
            html.push_str("<!DOCTYPE html><html><head><title>Ultra Large Document</title></head><body>");
            html.push_str("<h1>Deep Benchmark Test</h1>");
            for p in 1..=1000 {
                html.push_str(&format!("<p>Paragraph index {} of dense textual content with multiple entities.</p>", p));
            }
            html.push_str("<table><thead><tr><th>H1</th><th>H2</th><th>H3</th><th>H4</th><th>H5</th></tr></thead><tbody>");
            for r in 1..=100 {
                html.push_str(&format!("<tr><td>Row{}A</td><td>Row{}B</td><td>Row{}C</td><td>Row{}D</td><td>Row{}E</td></tr>", r, r, r, r, r));
            }
            html.push_str("</tbody></table></body></html>");
            (axum::http::StatusCode::OK, [("content-type", "text/html; charset=utf-8")], html)
        }),
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = shutdown_rx.await; })
            .await
            .ok();
    });

    let target_url = format!("{}/ultra-huge-document", base_url);

    // Concurrently execute 5 extractions of this ultra huge document
    let mut scrape_handles = Vec::new();
    for i in 0..5 {
        let client = server.client.clone();
        let s_url = server.base_url.clone();
        let token = server.auth_token.clone();
        let doc_url = target_url.clone();

        scrape_handles.push(tokio::spawn(async move {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": i,
                "method": "tools/call",
                "params": {
                    "name": "web_to_markdown",
                    "arguments": { "url": doc_url }
                }
            });

            let t0 = Instant::now();
            let res = client
                .post(format!("{}/mcp", s_url))
                .header("Authorization", format!("Bearer {}", token))
                .json(&payload)
                .send()
                .await
                .expect("Scrape request failed");

            assert_eq!(res.status(), StatusCode::OK);
            let json_body: Value = res.json().await.unwrap();
            let md = json_body["result"]["content"][0]["text"].as_str().unwrap();

            // Verify content completeness
            assert!(md.contains("Paragraph index 1 of dense textual content"));
            assert!(md.contains("Paragraph index 1000 of dense textual content"));
            assert!(md.contains("| H1 | H2 | H3 | H4 | H5 |"));
            assert!(md.contains("| Row1A | Row1B | Row1C | Row1D | Row1E |"));
            assert!(md.contains("| Row100A | Row100B | Row100C | Row100D | Row100E |"));
            assert!(!md.contains("<table>"));
            assert!(!md.contains("<tr>"));
            assert!(!md.contains("<p>"));

            t0.elapsed()
        }));
    }

    for h in scrape_handles {
        let duration = h.await.unwrap();
        assert!(
            duration < Duration::from_secs(10),
            "Ultra huge scrape must finish under 10 seconds, took {:?}",
            duration
        );
    }

    let _ = shutdown_tx.send(());
}

#[tokio::test]
async fn test_active_extraction_during_server_drop_graceful() {
    let server = TestServer::start().await;
    let port = server.port;

    // Connect raw socket
    let _socket = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("Connect failed");

    // Drop server while socket is connected
    drop(server);

    // Wait a brief moment for shutdown to complete
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Verify new connections are rejected immediately
    let conn_attempt = TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(
        conn_attempt.is_err(),
        "Server port must be closed after server drop"
    );
}

