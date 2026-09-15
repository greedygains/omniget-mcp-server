#![allow(clippy::needless_borrows_for_generic_args)]

//! Tier 5 Adversarial & White-Box Hardening Test Suite (Milestone 5).
//!
//! Comprehensive test suite covering 18 adversarial and boundary scenarios:
//! - Group A: Twitter/X Tools (x_extract.rs)
//!   1. test_t5_x_extract_raw_numeric_id
//!   2. test_t5_x_extract_alternative_supported_domains
//!   3. test_t5_x_extract_legacy_statuses_path_and_subroutes
//!   4. test_t5_x_extract_argument_alias_id_and_empty_dispatch
//! - Group B: Web to Markdown (web_markdown.rs)
//!   5. test_t5_web_markdown_reverse_og_title_order
//!   6. test_t5_web_markdown_javascript_link_sanitization
//!   7. test_t5_web_markdown_raw_html_preserved_inside_fenced_code
//!   8. test_t5_web_markdown_pathological_dom_depth_bomb
//!   9. test_t5_web_markdown_non_utf8_iso8859_charset
//! - Group C: PDF Text Extraction (pdf_text.rs)
//!   10. test_t5_pdf_text_remote_http_fetching_happy_path
//!   11. test_t5_pdf_text_remote_http_errors_404_and_500
//!   12. test_t5_pdf_text_remote_non_pdf_content_type
//!   13. test_t5_pdf_text_argument_alias_url
//!   14. test_t5_pdf_text_wildcard_and_reverse_page_ranges
//!   15. test_t5_pdf_text_zero_page_document_boundary
//! - Group D: Universal Media Metadata (media_info.rs)
//!   16. test_t5_media_info_direct_audio_extensions_no_ytdlp
//!   17. test_t5_media_info_direct_hls_stream_no_ytdlp
//!   18. test_t5_media_info_http_head_probe_fallback

mod common;

use common::{create_test_pdf, MockWebsite, TestServer};
use omniget_core::models::media::MediaType;
use omniget_server::tools::{
    media_info::{extract_media_info, MediaInfoArgs},
    pdf_text::{extract_pdf_text, parse_page_ranges, PdfTextArgs},
    web_markdown::tidy_markdown,
    x_extract::{parse_status_id, XExtractError},
};
use reqwest::StatusCode;
use serde_json::json;

// ============================================================================
// Group A: Twitter/X Tools (x_extract.rs)
// ============================================================================

/// T5.1: Twitter/X extraction with raw numeric status IDs (without URL scheme).
#[tokio::test]
async fn test_t5_x_extract_raw_numeric_id() {
    let server = TestServer::start().await;

    // Direct unit parser verification
    assert_eq!(parse_status_id("20").unwrap(), "20");
    assert_eq!(
        parse_status_id("1894567890123456789").unwrap(),
        "1894567890123456789"
    );
    assert_eq!(parse_status_id("   42   ").unwrap(), "42");

    // Protocol MCP verification via tools/call with raw numeric ID in 'url'
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "20" }
            }),
        )
        .await;
    let is_err = res["result"]["isError"].as_bool().unwrap_or(false);
    if is_err {
        let err_text = res["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            !err_text.contains("Malformed URL"),
            "Raw numeric ID should not fail as Malformed URL: {err_text}"
        );
        assert!(
            !err_text.contains("not a recognized X/Twitter domain"),
            "Raw numeric ID should not fail domain check: {err_text}"
        );
    }

    // Protocol verification via arguments.id alias
    let res_id = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "id": "1894567890123456789" }
            }),
        )
        .await;
    let is_err_id = res_id["result"]["isError"].as_bool().unwrap_or(false);
    if is_err_id {
        let err_text = res_id["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            !err_text.contains("Malformed URL"),
            "Raw numeric ID alias should not fail as Malformed URL: {err_text}"
        );
        assert!(
            !err_text.contains("not a recognized X/Twitter domain"),
            "Raw numeric ID alias should not fail domain check: {err_text}"
        );
    }
}

/// T5.2: Twitter/X extraction with alternative supported mirror domains (fixupx.com, fxtwitter.com, vxtwitter.com).
#[tokio::test]
async fn test_t5_x_extract_alternative_supported_domains() {
    let mirrors = [
        "https://fixupx.com/user/status/12345678",
        "https://fxtwitter.com/user/status/12345678",
        "https://vxtwitter.com/user/status/12345678",
        "http://fixupx.com/user/status/12345678",
        "http://fxtwitter.com/user/status/12345678",
        "http://vxtwitter.com/user/status/12345678",
    ];

    for mirror_url in mirrors {
        let parsed = parse_status_id(mirror_url).expect("alternative domain must be accepted");
        assert_eq!(parsed, "12345678", "Failed for mirror: {mirror_url}");
    }

    // Contrast with an unsupported domain which must be rejected
    let rejected = parse_status_id("https://fake-x.com/user/status/12345678");
    assert!(
        matches!(rejected, Err(XExtractError::InvalidDomain(_))),
        "Expected InvalidDomain error for unsupported domain"
    );
}

/// T5.3: Twitter/X extraction with legacy /statuses/ path format and subroutes (/photo/1, /video/1, /quotes).
#[tokio::test]
async fn test_t5_x_extract_legacy_statuses_path_and_subroutes() {
    let cases = [
        ("https://twitter.com/user/statuses/987654321", "987654321"),
        ("https://x.com/user/status/987654321/photo/1", "987654321"),
        ("https://x.com/user/status/987654321/video/1", "987654321"),
        ("https://x.com/user/status/987654321/quotes", "987654321"),
        ("https://x.com/user/statuses/987654321/history/analytics", "987654321"),
        ("https://x.com/user/status/987654321/", "987654321"),
    ];

    for (url, expected_id) in cases {
        let parsed = parse_status_id(url).expect("URL with subroute or legacy path must be parsed");
        assert_eq!(parsed, expected_id, "Mismatch for URL {url}");
    }
}

/// T5.4: Twitter/X extraction with dual argument schema ("id" instead of "url") and empty/null dispatch.
#[tokio::test]
async fn test_t5_x_extract_argument_alias_id_and_empty_dispatch() {
    let server = TestServer::start().await;

    // Empty object
    let res_empty = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": {}
            }),
        )
        .await;
    assert_eq!(res_empty["result"]["isError"], true);
    assert!(res_empty["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("URL or status ID cannot be empty"));

    // Null url and null id
    let res_nulls = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_thread",
                "arguments": { "url": null, "id": null }
            }),
        )
        .await;
    assert_eq!(res_nulls["result"]["isError"], true);
    assert!(res_nulls["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("URL or status ID cannot be empty"));

    // Empty string url falling back to empty string id
    let res_empty_strings = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "   ", "id": "" }
            }),
        )
        .await;
    assert_eq!(res_empty_strings["result"]["isError"], true);

    // Valid id supplied via REST GET /api/x/post?url=&id=20 (testing empty string query parameter fallback)
    let res_rest = server.get_authed("/api/x/post?url=&id=20").await;
    assert_ne!(
        res_rest.status(),
        StatusCode::BAD_REQUEST,
        "Query parameter fallback from empty url to valid id must not return 400"
    );

    // Valid id supplied via REST POST /api/x/thread with {"url": "", "id": "20"}
    let res_rest_post = server
        .post_json_authed(
            "/api/x/thread",
            &json!({ "url": "", "id": "20" }),
        )
        .await;
    assert_ne!(
        res_rest_post.status(),
        StatusCode::BAD_REQUEST,
        "Body fallback from empty url to valid id must not return 400"
    );
}

// ============================================================================
// Group B: Web to Markdown (web_markdown.rs)
// ============================================================================

/// T5.5: Web to Markdown extraction with reverse <meta> attribute ordering (content before property).
#[tokio::test]
async fn test_t5_web_markdown_reverse_og_title_order() {
    let server = TestServer::start().await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    let app = axum::Router::new().route(
        "/reversed-meta",
        axum::routing::get(|| async {
            (
                axum::http::StatusCode::OK,
                [("content-type", "text/html; charset=utf-8")],
                r#"<!DOCTYPE html><html><head>
<meta content="Reversed OG Title Headline" property="og:title">
<meta content="Reversed Twitter Description" name="twitter:description">
</head><body><p>Article body content goes here.</p></body></html>"#,
            )
        }),
    );

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await
            .ok();
    });

    let mock_url = format!("http://127.0.0.1:{port}/reversed-meta");
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
    let title = res["result"]["title"].as_str().unwrap();
    assert_eq!(title, "Reversed OG Title Headline");

    let markdown = res["result"]["markdown"].as_str().unwrap();
    assert!(markdown.contains("# Reversed OG Title Headline"));
    assert!(markdown.contains("Article body content goes here."));
}

/// T5.6: Web to Markdown sanitizes javascript: pseudo-protocol and empty href links into plain text.
#[tokio::test]
async fn test_t5_web_markdown_javascript_link_sanitization() {
    let server = TestServer::start().await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    let app = axum::Router::new().route(
        "/malicious-links",
        axum::routing::get(|| async {
            (
                axum::http::StatusCode::OK,
                [("content-type", "text/html; charset=utf-8")],
                r#"<!DOCTYPE html><html><head><title>Link Sanitization</title></head>
<body>
<p>Check this <a href="javascript:alert(document.cookie)">XSS Payload</a> now.</p>
<p>Here is an <a href="">Empty Link</a> and a <a href="   ">Whitespace Link</a>.</p>
<p>And here is a <a href="https://example.com/safe">Safe Verified Link</a>.</p>
</body></html>"#,
            )
        }),
    );

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await
            .ok();
    });

    let mock_url = format!("http://127.0.0.1:{port}/malicious-links");
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
    let markdown = res["result"]["markdown"].as_str().unwrap();

    // javascript: link must NOT be formatted as a Markdown link [text](javascript:...)
    assert!(!markdown.contains("](javascript:"));
    assert!(markdown.contains("XSS Payload"));

    // Empty links must NOT be formatted as Markdown links []( or []()
    assert!(!markdown.contains("[Empty Link]()"));
    assert!(!markdown.contains("[Whitespace Link]()"));
    assert!(markdown.contains("Empty Link"));
    assert!(markdown.contains("Whitespace Link"));

    // Safe link MUST be preserved as a Markdown link
    assert!(markdown.contains("[Safe Verified Link](https://example.com/safe)"));
}

/// T5.7: Web to Markdown strips HTML tags outside code blocks, but strictly preserves them inside fenced code.
#[tokio::test]
async fn test_t5_web_markdown_raw_html_preserved_inside_fenced_code() {
    let raw_md = r#"# HTML Documentation

<div><span class="bad">This div and span outside should be stripped</span></div>

```html
<div class="container">
    <span id="target">Inside code snippet</span>
</div>
```

<p>Normal text after code block <font color="red">with font tag</font></p>
"#;

    let cleaned = tidy_markdown(raw_md);

    // Outside code block: tags must be stripped
    assert!(!cleaned.contains("<div class=\"bad\">"));
    assert!(!cleaned.contains("<span class=\"bad\">"));
    assert!(!cleaned.contains("<font color=\"red\">"));
    assert!(cleaned.contains("This div and span outside should be stripped"));
    assert!(cleaned.contains("with font tag"));

    // Inside fenced code block: HTML tags must be preserved verbatim
    assert!(cleaned.contains("<div class=\"container\">"));
    assert!(cleaned.contains("<span id=\"target\">Inside code snippet</span>"));
    assert!(cleaned.contains("</div>"));
}

/// T5.8: Web to Markdown recursion and depth limit testing under a 250-level deeply nested DOM.
#[tokio::test]
async fn test_t5_web_markdown_pathological_dom_depth_bomb() {
    let server = TestServer::start().await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    let app = axum::Router::new().route(
        "/depth-bomb",
        axum::routing::get(|| async {
            let mut html = String::from("<!DOCTYPE html><html><head><title>Depth Bomb</title></head><body>");
            for i in 0..250 {
                if i % 2 == 0 {
                    html.push_str("<div>");
                } else {
                    html.push_str("<blockquote>");
                }
            }
            html.push_str("<p>Core message deeply buried inside 250 layers of nesting.</p>");
            for i in (0..250).rev() {
                if i % 2 == 0 {
                    html.push_str("</div>");
                } else {
                    html.push_str("</blockquote>");
                }
            }
            html.push_str("</body></html>");
            (
                axum::http::StatusCode::OK,
                [("content-type", "text/html; charset=utf-8")],
                html,
            )
        }),
    );

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await
            .ok();
    });

    let mock_url = format!("http://127.0.0.1:{port}/depth-bomb");
    let start_time = std::time::Instant::now();
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock_url }
            }),
        )
        .await;
    let duration = start_time.elapsed();

    let _ = tx.send(());

    assert!(
        duration < std::time::Duration::from_millis(2000),
        "Depth bomb took too long: {:?}",
        duration
    );
    assert_eq!(res["result"]["isError"], false);
    let markdown = res["result"]["markdown"].as_str().unwrap();
    assert!(markdown.contains("Core message deeply buried inside 250 layers of nesting."));
}

/// T5.9: Web to Markdown handling of non-UTF8 ISO-8859-1 (Latin-1) encoded European characters.
#[tokio::test]
async fn test_t5_web_markdown_non_utf8_iso8859_charset() {
    let server = TestServer::start().await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    let app = axum::Router::new().route(
        "/iso8859-page",
        axum::routing::get(|| async {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(b"<!DOCTYPE html><html><head><title>Latin1 Page</title></head><body><h1>Caf");
            bytes.push(0xE9); // 'é' in ISO-8859-1
            bytes.extend_from_slice(b" ");
            bytes.push(0xDC); // 'Ü' in ISO-8859-1
            bytes.extend_from_slice(b"ber Na");
            bytes.push(0xEF); // 'ï' in ISO-8859-1
            bytes.extend_from_slice(b"ve</h1><p>Cr");
            bytes.push(0xE8); // 'è' in ISO-8859-1
            bytes.extend_from_slice(b"me br");
            bytes.push(0xFB); // 'û' in ISO-8859-1
            bytes.extend_from_slice(b"l");
            bytes.push(0xE9); // 'é' in ISO-8859-1
            bytes.extend_from_slice(b"e</p></body></html>");

            (
                axum::http::StatusCode::OK,
                [("content-type", "text/html; charset=iso-8859-1")],
                bytes,
            )
        }),
    );

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await
            .ok();
    });

    let mock_url = format!("http://127.0.0.1:{port}/iso8859-page");
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
    let markdown = res["result"]["markdown"].as_str().unwrap();
    assert!(markdown.contains("Caf") && markdown.contains("ber"));
}

// ============================================================================
// Group C: PDF Text Extraction (pdf_text.rs)
// ============================================================================

/// T5.10: Pure-Rust in-memory extraction from a remote HTTP PDF document URL.
#[tokio::test]
async fn test_t5_pdf_text_remote_http_fetching_happy_path() {
    let server = TestServer::start().await;

    let pdf_temp = create_test_pdf("Remote Cloud Invoice", 2);
    let pdf_bytes = std::fs::read(pdf_temp.path()).expect("read test pdf");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    let app = axum::Router::new().route(
        "/documents/invoice.pdf",
        axum::routing::get(move || {
            let bytes = pdf_bytes.clone();
            async move {
                (
                    axum::http::StatusCode::OK,
                    [("content-type", "application/pdf")],
                    bytes,
                )
            }
        }),
    );

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await
            .ok();
    });

    let remote_pdf_url = format!("http://127.0.0.1:{port}/documents/invoice.pdf");
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": remote_pdf_url }
            }),
        )
        .await;

    let _ = tx.send(());

    assert_eq!(res["result"]["isError"], false);
    assert_eq!(res["result"]["pages"], 2);
    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Remote Cloud Invoice - Page 1"));
    assert!(text.contains("Remote Cloud Invoice - Page 2"));
}

/// T5.11: Remote PDF HTTP error handling (404 Not Found and 500 Server Error).
#[tokio::test]
async fn test_t5_pdf_text_remote_http_errors_404_and_500() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    // 404 Not Found
    let res_404 = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": mock.url("/nonexistent.pdf") }
            }),
        )
        .await;
    assert_eq!(res_404["result"]["isError"], true);
    assert!(res_404["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Remote PDF fetch failed with HTTP status 404"));

    // 500 Internal Server Error
    let res_500 = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": mock.url("/server-error") }
            }),
        )
        .await;
    assert_eq!(res_500["result"]["isError"], true);
    assert!(res_500["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Remote PDF fetch failed with HTTP status 500"));
}

/// T5.12: Remote PDF extraction when URL returns an HTML page instead of PDF binary.
#[tokio::test]
async fn test_t5_pdf_text_remote_non_pdf_content_type() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": mock.url("/article-simple") }
            }),
        )
        .await;

    assert_eq!(res["result"]["isError"], true);
    let error_text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(error_text.contains("Failed to parse PDF document"));
}

/// T5.13: PDF extraction accepting "url" parameter key as an alias for "path".
#[tokio::test]
async fn test_t5_pdf_text_argument_alias_url() {
    let server = TestServer::start().await;

    let pdf_temp = create_test_pdf("Alias URL Test Document", 1);
    let pdf_bytes = std::fs::read(pdf_temp.path()).expect("read test pdf");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    let app = axum::Router::new().route(
        "/doc.pdf",
        axum::routing::get(move || {
            let bytes = pdf_bytes.clone();
            async move {
                (
                    axum::http::StatusCode::OK,
                    [("content-type", "application/pdf")],
                    bytes,
                )
            }
        }),
    );

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await
            .ok();
    });

    let remote_url = format!("http://127.0.0.1:{port}/doc.pdf");
    // Passing "url" instead of "path"
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "url": remote_url.clone() }
            }),
        )
        .await;

    assert_eq!(res["result"]["isError"], false);
    assert_eq!(res["result"]["pages"], 1);
    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Alias URL Test Document - Page 1"));

    // Also verify in REST POST /api/pdf/text with {"url": "..."} and empty path {"path": ""}
    let rest_res = server
        .post_json_authed(
            "/api/pdf/text",
            &json!({ "path": "", "url": remote_url }),
        )
        .await;
    assert_eq!(rest_res.status(), StatusCode::OK);

    let _ = tx.send(());
}

/// T5.14: PDF text extraction wildcard ("*") and reverse ranges ("3-1").
#[tokio::test]
async fn test_t5_pdf_text_wildcard_and_reverse_page_ranges() {
    let pdf_temp = create_test_pdf("Sequential Content", 3);
    let path = pdf_temp.path().to_str().unwrap().to_string();

    // 1. Wildcard range "*"
    let ranges_wildcard = parse_page_ranges(Some("*"), 3).unwrap();
    assert_eq!(ranges_wildcard, vec![1, 2, 3]);

    let res_wildcard = extract_pdf_text(PdfTextArgs {
        path: Some(path.clone()),
        url: None,
        pages: Some("*".to_string()),
    })
    .await
    .unwrap();
    assert_eq!(res_wildcard.pages, 3);
    let pos_p1 = res_wildcard.text.find("Page 1").unwrap();
    let pos_p2 = res_wildcard.text.find("Page 2").unwrap();
    let pos_p3 = res_wildcard.text.find("Page 3").unwrap();
    assert!(pos_p1 < pos_p2 && pos_p2 < pos_p3);

    // 2. Reverse range "3-1"
    let ranges_reverse = parse_page_ranges(Some("3-1"), 3).unwrap();
    assert_eq!(ranges_reverse, vec![3, 2, 1]);

    let res_reverse = extract_pdf_text(PdfTextArgs {
        path: Some(path),
        url: None,
        pages: Some("3-1".to_string()),
    })
    .await
    .unwrap();
    assert_eq!(res_reverse.pages, 3);
    let rpos_p3 = res_reverse.text.find("Page 3").unwrap();
    let rpos_p1 = res_reverse.text.find("Page 1").unwrap();
    assert!(
        rpos_p3 < rpos_p1,
        "Page 3 text must appear before Page 1 text in reverse range '3-1'"
    );
}

/// T5.15: Pure-Rust PDF extractor boundary test for a 0-page PDF document.
#[tokio::test]
async fn test_t5_pdf_text_zero_page_document_boundary() {
    use lopdf::dictionary;
    use lopdf::{Document, Object};

    // Construct a syntactically valid PDF catalog with Count = 0 and empty Kids
    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let pages_dict = dictionary! {
        "Type" => "Pages",
        "Kids" => Object::Array(vec![]),
        "Count" => 0,
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages_dict));

    let catalog_id = doc.new_object_id();
    let catalog = dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    };
    doc.objects.insert(catalog_id, Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut temp = tempfile::NamedTempFile::new().unwrap();
    doc.save(&mut temp).unwrap();

    let res = extract_pdf_text(PdfTextArgs {
        path: Some(temp.path().to_str().unwrap().to_string()),
        url: None,
        pages: None,
    })
    .await
    .unwrap();

    assert_eq!(res.pages, 0);
    assert_eq!(res.text, "");
}

// ============================================================================
// Group D: Universal Media Metadata (media_info.rs)
// ============================================================================

/// T5.16: Direct audio stream extensions (.mp3, .m4a, .wav) resolved purely in Rust without yt-dlp.
#[tokio::test]
async fn test_t5_media_info_direct_audio_extensions_no_ytdlp() {
    let audio_urls = [
        "https://cdn.example.com/audio/podcast-episode.mp3",
        "https://audio.example.org/recordings/speech.m4a",
        "http://files.example.net/samples/sound.wav",
        "https://music.example.com/tracks/song.flac",
        "https://stream.example.com/audio/stream.aac",
    ];

    for url in audio_urls {
        let info = extract_media_info(MediaInfoArgs { url: url.to_string() })
            .await
            .expect("direct audio must be resolved in pure Rust");
        assert_eq!(info.platform, "generic");
        assert_eq!(info.available_qualities[0].format, "direct_audio");
        assert!(matches!(info.media_type, MediaType::Audio));
        assert_eq!(info.available_qualities.len(), 1);
        assert_eq!(info.available_qualities[0].url, url);
    }
}

/// T5.17: Direct HLS streaming playlist (.m3u8) resolved in pure Rust without yt-dlp.
#[tokio::test]
async fn test_t5_media_info_direct_hls_stream_no_ytdlp() {
    let hls_urls = [
        "https://live.example.com/hls/master.m3u8",
        "https://cdn.stream.org/feed/index.m3u8?token=xyz123",
    ];

    for url in hls_urls {
        let info = extract_media_info(MediaInfoArgs { url: url.to_string() })
            .await
            .expect("direct HLS must be resolved in pure Rust");
        assert_eq!(info.platform, "generic");
        assert_eq!(info.available_qualities[0].format, "hls");
        assert!(matches!(info.media_type, MediaType::Video));
        assert_eq!(info.available_qualities.len(), 1);
        assert_eq!(info.available_qualities[0].url, url);
    }
}

/// T5.18: HTTP HEAD probe fallback synthesizes MediaInfo for extensionless media streams.
#[tokio::test]
async fn test_t5_media_info_http_head_probe_fallback() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    let app = axum::Router::new()
        .route(
            "/stream/live-video",
            axum::routing::head(|| async {
                (
                    axum::http::StatusCode::OK,
                    [
                        ("content-type", "video/mp4"),
                        ("content-length", "52428800"),
                    ],
                )
            }),
        )
        .route(
            "/stream/live-audio",
            axum::routing::head(|| async {
                (
                    axum::http::StatusCode::OK,
                    [
                        ("content-type", "audio/aac"),
                        ("content-length", "10485760"),
                    ],
                )
            }),
        )
        .route(
            "/stream/not-media",
            axum::routing::head(|| async {
                (
                    axum::http::StatusCode::OK,
                    [("content-type", "text/html; charset=utf-8")],
                )
            }),
        );

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await
            .ok();
    });

    let video_url = format!("http://127.0.0.1:{port}/stream/live-video");
    let audio_url = format!("http://127.0.0.1:{port}/stream/live-audio");
    let non_media_url = format!("http://127.0.0.1:{port}/stream/not-media");

    // 1. Direct video stream probe
    let video_info = extract_media_info(MediaInfoArgs { url: video_url })
        .await
        .expect("video probe fallback");
    assert_eq!(video_info.available_qualities[0].format, "direct_video");
    assert!(matches!(video_info.media_type, MediaType::Video));
    assert_eq!(video_info.file_size_bytes, Some(52428800));

    // 2. Direct audio stream probe
    let audio_info = extract_media_info(MediaInfoArgs { url: audio_url })
        .await
        .expect("audio probe fallback");
    assert_eq!(audio_info.available_qualities[0].format, "direct_audio");
    assert!(matches!(audio_info.media_type, MediaType::Audio));
    assert_eq!(audio_info.file_size_bytes, Some(10485760));

    // 3. Non-media endpoint probe fails
    let non_media_result = extract_media_info(MediaInfoArgs { url: non_media_url }).await;
    assert!(non_media_result.is_err());

    let _ = tx.send(());
}
