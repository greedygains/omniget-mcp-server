//! Adversarial Stress & Correctness Verification Suite for Milestone 2 (Extraction Tools)
//!
//! Vectors Covered:
//! 1. Adversarial URLs & SSRF Protection:
//!    - Non-HTTP schemes: file://, ftp://, gopher://, javascript:, data:, ldap://, dict://, blob:, ws://, wss://
//!    - Rejection across web_to_markdown, media_info, x_post, x_thread
//!    - Domain spoofing attacks against x_post and x_thread (subdomain manipulation, evil-x, lookalike domains)
//!    - Pathological X/Twitter URLs (missing status ID, non-numeric ID, 10KB query strings, path traversal attempts)
//! 2. Pathological HTML & Table Normalization:
//!    - Deeply nested tables (10 levels of recursion)
//!    - Tables without <th> headers (promoting <td> to pipe table headers)
//!    - Empty tables and tables with empty rows
//!    - Malformed and unclosed HTML tags (<span><div><p> without closing tags)
//!    - Stripping of junk elements, script, style, SVG, form, and button tags (ZERO raw HTML tags in output)
//!    - Unicode injection, RTL Arabic/Hebrew, Emojis, Zero-width spaces (\u{200B}, \u{FEFF})
//! 3. Corrupt & Extreme PDFs:
//!    - Zero-byte files (0 bytes)
//!    - Random noise/garbage bytes (2048 bytes of pseudorandom data)
//!    - Truncated PDF header with immediate EOF
//!    - Page range fuzzing: out of bounds ("999-1000", "999"), negative ("-5"), malformed ("1-2-3", "abc", "0"),
//!      reversed ranges ("3-1"), massive ranges ("1-1000000"), and empty/whitespace ranges
//! 4. Tool Router JSON-RPC Fuzzing:
//!    - Unknown tool names ("unknown_tool_9999")
//!    - Missing arguments object or missing required fields
//!    - Type confusion (numbers for string fields, nulls, arrays, booleans)
//!    - JSON-RPC protocol fuzzing (arbitrary ID types, unknown methods)
//!    - High-concurrency burst of mixed valid, fuzzed, and adversarial payloads

mod common;

use common::{create_corrupt_pdf, create_test_pdf, MockWebsite, TestServer};
use serde_json::{json, Value};
use std::io::Write;
use tempfile::NamedTempFile;

// ============================================================================
// VECTOR 1: Adversarial URLs & SSRF Protection
// ============================================================================

#[tokio::test]
async fn test_adversarial_ssrf_schemes_rejected_web_markdown() {
    let server = TestServer::start().await;

    let forbidden_schemes = [
        "file:///etc/passwd",
        "file:///C:/Windows/System32/drivers/etc/hosts",
        "ftp://attacker.com/malicious.sh",
        "gopher://127.0.0.1:6379/_INFO",
        "javascript:alert(document.domain)",
        "javascript://%0aalert(1)",
        "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==",
        "data:text/plain;charset=utf-8,malicious",
        "ldap://localhost:389/dc=example,dc=com",
        "dict://localhost:2628/d:test",
        "blob:https://example.com/uuid-blob",
        "ws://127.0.0.1:8080/ws",
        "wss://127.0.0.1:8080/wss",
        "ssh://git@github.com:user/repo.git",
        "telnet://127.0.0.1:23",
        "mailto:victim@example.com",
    ];

    for scheme_url in forbidden_schemes {
        let res = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "web_to_markdown",
                    "arguments": { "url": scheme_url }
                }),
            )
            .await;

        let is_error = res["error"].is_object() || res["result"]["isError"] == true;
        assert!(
            is_error,
            "SSRF scheme '{}' must be rejected with error, got: {:?}",
            scheme_url, res
        );
    }
}

#[tokio::test]
async fn test_adversarial_ssrf_schemes_rejected_media_info() {
    let server = TestServer::start().await;

    let forbidden_schemes = [
        "file:///etc/shadow",
        "ftp://mirror.example.com/video.mp4",
        "gopher://127.0.0.1:70/stream",
        "javascript:void(0)",
        "data:video/mp4;base64,AAAA",
        "ws://localhost:9000/live",
    ];

    for scheme_url in forbidden_schemes {
        let res = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "media_info",
                    "arguments": { "url": scheme_url }
                }),
            )
            .await;

        let is_error = res["error"].is_object() || res["result"]["isError"] == true;
        assert!(
            is_error,
            "media_info must reject scheme '{}', got: {:?}",
            scheme_url, res
        );
    }
}

#[tokio::test]
async fn test_adversarial_x_domain_spoofing_attacks() {
    let server = TestServer::start().await;

    let spoof_urls = [
        "https://evil-x.com/status/123456",
        "https://notx.com/user/status/123456",
        "https://x.com.attacker.com/status/123456",
        "https://sub.evil-x.com/status/123456",
        "https://nottwitter.com/status/123456",
        "https://twitter.com.attacker.com/status/123456",
        "https://attacker-fxtwitter.com/status/123456",
        "https://fxtwitter.com.evil.org/status/123456",
        "https://vxtwitter.com.attacker.com/status/123456",
        "https://fixupx.com.evil.net/status/123456",
        "https://instagram.com/p/C_123456",
        "https://facebook.com/posts/123456",
        "https://reddit.com/r/rust/comments/123456",
        "file:///etc/passwd",
        "javascript:alert(1)",
    ];

    for url in spoof_urls {
        // Test x_post
        let res_post = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "x_post",
                    "arguments": { "url": url }
                }),
            )
            .await;
        let is_error_post = res_post["error"].is_object() || res_post["result"]["isError"] == true;
        assert!(
            is_error_post,
            "x_post must reject spoofed/invalid domain '{}', got: {:?}",
            url, res_post
        );

        // Test x_thread
        let res_thread = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "x_thread",
                    "arguments": { "url": url }
                }),
            )
            .await;
        let is_error_thread = res_thread["error"].is_object() || res_thread["result"]["isError"] == true;
        assert!(
            is_error_thread,
            "x_thread must reject spoofed/invalid domain '{}', got: {:?}",
            url, res_thread
        );
    }
}

#[tokio::test]
async fn test_adversarial_x_pathological_urls() {
    let server = TestServer::start().await;

    // 10KB query string padding
    let long_query = "a".repeat(10_000);
    let giant_url = format!("https://x.com/jack/status/20?padding={}", long_query);

    let pathological_cases = vec![
        ("", "empty string"),
        ("   \t\n  ", "whitespace only"),
        ("https://x.com", "root domain without path"),
        ("https://x.com/", "root domain with slash"),
        ("https://x.com/jack", "user profile without status"),
        ("https://x.com/jack/status/", "status prefix without ID"),
        ("https://x.com/jack/status/notanumber", "alphanumeric ID"),
        ("https://x.com/jack/status/-12345", "negative status ID"),
        ("https://x.com/jack/status/123.456", "floating point ID"),
        ("https://x.com/jack/status/../../etc/passwd", "path traversal attempt"),
    ];

    for (bad_url, desc) in pathological_cases {
        let res = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "x_post",
                    "arguments": { "url": bad_url }
                }),
            )
            .await;

        let is_error = res["error"].is_object() || res["result"]["isError"] == true;
        assert!(
            is_error,
            "x_post must reject {}: '{}', got: {:?}",
            desc, bad_url, res
        );
    }

    // Giant 10KB URL must parse status ID safely without buffer overflow or panic
    let res_giant = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": giant_url }
            }),
        )
        .await;
    assert!(res_giant["result"].is_object() || res_giant["error"].is_object());
}

// ============================================================================
// VECTOR 2: Pathological HTML & Table Normalization
// ============================================================================

#[tokio::test]
async fn test_adversarial_deeply_nested_html_tables() {
    let server = TestServer::start().await;

    // Generate 10 levels of nested tables
    let mut nested_html = String::from("<!DOCTYPE html><html><body><h1>Nested Tables</h1>");
    for i in 1..=10 {
        nested_html.push_str(&format!(
            "<table><tr><td>Level {} Table",
            i
        ));
    }
    nested_html.push_str("<strong>Deepest Core Content</strong>");
    for _ in 1..=10 {
        nested_html.push_str("</td></tr></table>");
    }
    nested_html.push_str("</body></html>");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind nested test server");
    let port = listener.local_addr().unwrap().port();
    let mock_url = format!("http://127.0.0.1:{port}/nested");

    let app = axum::Router::new().route(
        "/nested",
        axum::routing::get(move || {
            let html = nested_html.clone();
            async move {
                (
                    axum::http::StatusCode::OK,
                    [("content-type", "text/html; charset=utf-8")],
                    html,
                )
            }
        }),
    );

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await
            .ok();
    });

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock_url }
            }),
        )
        .await;

    let _ = tx.send(());

    assert_eq!(res["result"]["isError"], false);
    let md = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(md.contains("Deepest Core Content"));
    assert!(!md.contains("<table>"), "Markdown must not contain <table> tags");
    assert!(!md.contains("<td>"), "Markdown must not contain <td> tags");
}

#[tokio::test]
async fn test_adversarial_tables_without_th_promoted_to_pipe_table() {
    let server = TestServer::start().await;

    // Table strictly using <td> without any <th> elements
    let html = r#"<!DOCTYPE html>
<html>
<head><title>No-TH Table</title></head>
<body>
<h1>Product Catalog</h1>
<table>
  <tr><td>Item</td><td>Price</td><td>Quantity</td></tr>
  <tr><td>Widget A</td><td>$19.99</td><td>50</td></tr>
  <tr><td>Widget B</td><td>$29.99</td><td>25</td></tr>
</table>
</body>
</html>"#;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind no-th server");
    let port = listener.local_addr().unwrap().port();
    let mock_url = format!("http://127.0.0.1:{port}/no-th");

    let app = axum::Router::new().route(
        "/no-th",
        axum::routing::get(move || async move {
            (
                axum::http::StatusCode::OK,
                [("content-type", "text/html; charset=utf-8")],
                html,
            )
        }),
    );

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await
            .ok();
    });

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock_url }
            }),
        )
        .await;

    let _ = tx.send(());

    assert_eq!(res["result"]["isError"], false);
    let md = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(md.contains("| Item | Price | Quantity |"));
    assert!(md.contains("| Widget A | $19.99 | 50 |"));
    assert!(!md.contains("<table>"));
    assert!(!md.contains("<tr>"));
    assert!(!md.contains("<td>"));
}

#[tokio::test]
async fn test_adversarial_empty_and_broken_tables() {
    let server = TestServer::start().await;

    let html = r#"<!DOCTYPE html>
<html>
<body>
<h1>Edge Tables</h1>
<table></table>
<table><tr></tr></table>
<table><tr><td>Only One Cell</td></tr></table>
<table>Non-tagged text inside table</table>
</body>
</html>"#;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mock_url = format!("http://127.0.0.1:{port}/edge-tables");

    let app = axum::Router::new().route(
        "/edge-tables",
        axum::routing::get(move || async move {
            (
                axum::http::StatusCode::OK,
                [("content-type", "text/html; charset=utf-8")],
                html,
            )
        }),
    );

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await
            .ok();
    });

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock_url }
            }),
        )
        .await;

    let _ = tx.send(());

    assert_eq!(res["result"]["isError"], false);
    let md = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(md.contains("Only One Cell"));
    assert!(!md.contains("<table>"));
    assert!(!md.contains("</table>"));
}

#[tokio::test]
async fn test_adversarial_unclosed_tags_and_heavy_injection() {
    let server = TestServer::start().await;

    // Malformed HTML with unclosed tags, script tags, style, SVG, button, comments, and zero-width spaces
    let html = "<!DOCTYPE html>
<html>
<head>
<title>Unclosed & Injected</title>
<script>alert('xss'); while(1){}</script>
<style>body { background: black !important; } @media print { * { display: none; } }</style>
</head>
<body>
<!-- Secret comment that should not be visible -->
<svg width='100' height='100'><circle cx='50' cy='50' r='40'/></svg>
<form action='/login'><input type='password' name='pass'/><button type='submit'>Submit</button></form>
<div class='advertisement'>Ad Content</div>
<div><span><p>Unclosed paragraph with <b>bold and <i>italic text
<p>Second unclosed paragraph with zero-width spaces: \u{200B}secret\u{FEFF}
<p>RTL Multilingual: مرحبا بالعالم! שלום עולם 🚀🔥
</body>";

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mock_url = format!("http://127.0.0.1:{port}/unclosed");

    let app = axum::Router::new().route(
        "/unclosed",
        axum::routing::get(move || async move {
            (
                axum::http::StatusCode::OK,
                [("content-type", "text/html; charset=utf-8")],
                html,
            )
        }),
    );

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await
            .ok();
    });

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock_url }
            }),
        )
        .await;

    let _ = tx.send(());

    assert_eq!(res["result"]["isError"], false);
    let md = res["result"]["content"][0]["text"].as_str().unwrap();

    // Check content extraction
    assert!(md.contains("Unclosed paragraph with"));
    assert!(md.contains("bold and"));
    assert!(md.contains("italic text"));
    assert!(md.contains("مرحبا بالعالم!"));
    assert!(md.contains("שלום עולם"));
    assert!(md.contains("🚀🔥"));

    // Check that NO raw HTML tags survive in the Markdown
    assert!(!md.contains("<script>"), "Must strip <script>");
    assert!(!md.contains("alert('xss')"), "Must strip script body");
    assert!(!md.contains("<style>"), "Must strip <style>");
    assert!(!md.contains("<svg>"), "Must strip <svg>");
    assert!(!md.contains("<form>"), "Must strip <form>");
    assert!(!md.contains("<button>"), "Must strip <button>");
    assert!(!md.contains("<div>"), "Must strip <div>");
    assert!(!md.contains("<span>"), "Must strip <span>");
    assert!(!md.contains("<p>"), "Must strip <p>");
    assert!(!md.contains("Ad Content"), "Must strip advertisement class");
}

// ============================================================================
// VECTOR 3: Corrupt & Extreme PDFs
// ============================================================================

#[tokio::test]
async fn test_adversarial_zero_byte_pdf() {
    let server = TestServer::start().await;

    let mut empty_file = NamedTempFile::new().unwrap();
    empty_file.write_all(b"").unwrap();
    let path = empty_file.path().to_str().unwrap();

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path }
            }),
        )
        .await;

    let is_error = res["error"].is_object() || res["result"]["isError"] == true;
    assert!(is_error, "0-byte PDF must return error, got: {:?}", res);
}

#[tokio::test]
async fn test_adversarial_random_garbage_bytes_pdf() {
    let server = TestServer::start().await;

    // Create a 2KB file with pseudo-random non-PDF binary bytes
    let mut garbage_file = NamedTempFile::new().unwrap();
    let garbage: Vec<u8> = (0..2048).map(|i| ((i * 37 + 13) % 256) as u8).collect();
    garbage_file.write_all(&garbage).unwrap();
    let path = garbage_file.path().to_str().unwrap();

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path }
            }),
        )
        .await;

    let is_error = res["error"].is_object() || res["result"]["isError"] == true;
    assert!(is_error, "Random garbage PDF must return error without crash, got: {:?}", res);
}

#[tokio::test]
async fn test_adversarial_fixture_corrupt_pdf() {
    let server = TestServer::start().await;
    let corrupt = create_corrupt_pdf();
    let path = corrupt.path().to_str().unwrap();

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path }
            }),
        )
        .await;

    let is_error = res["error"].is_object() || res["result"]["isError"] == true;
    assert!(is_error, "Corrupt PDF fixture must return error, got: {:?}", res);
}

#[tokio::test]
async fn test_adversarial_truncated_pdf_header() {
    let server = TestServer::start().await;

    let mut truncated_file = NamedTempFile::new().unwrap();
    truncated_file.write_all(b"%PDF-1.4\n%%EOF\n").unwrap();
    let path = truncated_file.path().to_str().unwrap();

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path }
            }),
        )
        .await;

    // Lopdf will either report 0 pages or return a parse error; both are clean and non-panicking
    assert!(res["result"].is_object() || res["error"].is_object());
}

#[tokio::test]
async fn test_adversarial_pdf_extreme_page_ranges() {
    let server = TestServer::start().await;
    let pdf = create_test_pdf("Extreme Page Range Document", 3);
    let path = pdf.path().to_str().unwrap();

    // 1. Reversed range ("3-1")
    let res_rev = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path, "pages": "3-1" }
            }),
        )
        .await;
    assert_eq!(res_rev["result"]["isError"], false);
    let text_rev = res_rev["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text_rev.contains("Page 3"));
    assert!(text_rev.contains("Page 1"));

    // 2. Out-of-bounds start & end ("999-1000" on 3-page PDF)
    let res_oob = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path, "pages": "999-1000" }
            }),
        )
        .await;
    assert_eq!(res_oob["result"]["isError"], true);

    // 3. Negative number syntax ("-5")
    let res_neg = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path, "pages": "-5" }
            }),
        )
        .await;
    assert_eq!(res_neg["result"]["isError"], true);

    // 4. Invalid range syntax ("1-2-3")
    let res_syntax = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path, "pages": "1-2-3" }
            }),
        )
        .await;
    assert_eq!(res_syntax["result"]["isError"], true);

    // 5. Zero page number ("0" or "0-2")
    let res_zero = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path, "pages": "0" }
            }),
        )
        .await;
    assert_eq!(res_zero["result"]["isError"], true);

    // 6. Massive range ("1-1000000" on 3-page PDF)
    let res_massive = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path, "pages": "1-1000000" }
            }),
        )
        .await;
    assert_eq!(res_massive["result"]["isError"], true);

    // 7. Empty / whitespace pages string defaults gracefully to all pages
    let res_empty_pages = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path, "pages": "   " }
            }),
        )
        .await;
    assert_eq!(res_empty_pages["result"]["isError"], false);
    assert_eq!(res_empty_pages["result"]["pages"], 3);

    // 8. Wildcard asterisk "*" extracts all pages
    let res_star = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path, "pages": "*" }
            }),
        )
        .await;
    assert_eq!(res_star["result"]["isError"], false);
    assert_eq!(res_star["result"]["pages"], 3);
}

// ============================================================================
// VECTOR 4: Tool Router JSON-RPC Fuzzing
// ============================================================================

#[tokio::test]
async fn test_adversarial_json_rpc_tool_fuzzing() {
    let server = TestServer::start().await;

    // 1. Unknown tool name
    let res_unknown = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "completely_unknown_tool_xyz_9999",
                "arguments": { "url": "https://example.com" }
            }),
        )
        .await;
    assert_eq!(res_unknown["result"]["isError"], true);
    assert!(res_unknown["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Unknown tool"));

    // 2. Missing name in params
    let res_no_name = server
        .json_rpc("tools/call", json!({ "arguments": {} }))
        .await;
    assert_eq!(res_no_name["result"]["isError"], true);

    // 3. Type confusion: number instead of string for web_to_markdown URL
    let res_wrong_type_url = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": 12345 }
            }),
        )
        .await;
    assert_eq!(res_wrong_type_url["result"]["isError"], true);

    // 4. Type confusion: boolean instead of string
    let res_bool_url = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": true }
            }),
        )
        .await;
    assert_eq!(res_bool_url["result"]["isError"], true);

    // 5. Type confusion: null URL
    let res_null_url = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": null }
            }),
        )
        .await;
    assert_eq!(res_null_url["result"]["isError"], true);

    // 6. Type confusion: number instead of string for pdf_text path
    let res_wrong_type_path = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": 9999 }
            }),
        )
        .await;
    assert_eq!(res_wrong_type_path["result"]["isError"], true);

    // 7. Type confusion: number instead of string for pdf_text pages
    let res_wrong_type_pages = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": "/valid/path.pdf", "pages": 12 }
            }),
        )
        .await;
    assert_eq!(res_wrong_type_pages["result"]["isError"], true);

    // 8. Type confusion: number instead of string for media_info url
    let res_wrong_type_media = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": 8888 }
            }),
        )
        .await;
    assert_eq!(res_wrong_type_media["result"]["isError"], true);

    // 9. Extra unexpected fields in arguments should be tolerated gracefully
    let mock = MockWebsite::start().await;
    let res_extra_fields = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": {
                    "url": mock.url("/article-simple"),
                    "unexpected_field_1": 12345,
                    "malicious_code": "SELECT * FROM users",
                    "nested": { "a": [1, 2, 3] }
                }
            }),
        )
        .await;
    assert_eq!(res_extra_fields["result"]["isError"], false);
    assert!(res_extra_fields["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Simple Article Title"));
}

#[tokio::test]
async fn test_adversarial_high_concurrency_fuzz_burst() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let valid_url = mock.url("/article-simple");
    let pdf = create_test_pdf("Concurrency Burst PDF", 1);
    let pdf_path = pdf.path().to_str().unwrap().to_string();

    let mut handles = Vec::new();

    // Spawn 50 concurrent mixed tasks: valid requests, SSRF attempts, type fuzzing, non-existent files
    for i in 0..50 {
        let client = server.client.clone();
        let base_url = server.base_url.clone();
        let token = server.auth_token.clone();
        let valid_url = valid_url.clone();
        let pdf_path = pdf_path.clone();

        handles.push(tokio::spawn(async move {
            let payload = match i % 5 {
                0 => json!({
                    "jsonrpc": "2.0", "id": i, "method": "tools/call",
                    "params": { "name": "web_to_markdown", "arguments": { "url": valid_url } }
                }),
                1 => json!({
                    "jsonrpc": "2.0", "id": i, "method": "tools/call",
                    "params": { "name": "web_to_markdown", "arguments": { "url": "file:///etc/passwd" } }
                }),
                2 => json!({
                    "jsonrpc": "2.0", "id": i, "method": "tools/call",
                    "params": { "name": "pdf_text", "arguments": { "path": pdf_path } }
                }),
                3 => json!({
                    "jsonrpc": "2.0", "id": i, "method": "tools/call",
                    "params": { "name": "pdf_text", "arguments": { "path": 12345 } }
                }),
                _ => json!({
                    "jsonrpc": "2.0", "id": i, "method": "tools/call",
                    "params": { "name": "unknown_tool", "arguments": {} }
                }),
            };

            let res = client
                .post(format!("{}/mcp", base_url))
                .header("Authorization", format!("Bearer {}", token))
                .json(&payload)
                .send()
                .await
                .expect("send request")
                .json::<Value>()
                .await
                .expect("parse response");

            assert_eq!(res["jsonrpc"], "2.0");
            assert_eq!(res["id"], i);
        }));
    }

    for handle in handles {
        handle.await.expect("task completed without panic");
    }
}
