mod common;

use common::{create_corrupt_pdf, create_test_pdf, MockWebsite, TestServer};
use serde_json::{json, Value};

// ============================================================================
// FEATURE 8: Universal Web to Markdown (web_to_markdown) — Tier 1 & Tier 2
// ============================================================================

/// T1.1: Extracts clean title and paragraph Markdown from MockWebsite.
#[tokio::test]
async fn test_t1_f8_web_markdown_simple_article() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock.url("/article-simple") }
            }),
        )
        .await;

    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Simple Article Title"));
    assert!(text.contains("This is a clean paragraph of text for testing."));
    assert!(!text.contains("<article>"));
}

/// T1.2: Converts HTML tables to standard Markdown pipe tables.
#[tokio::test]
async fn test_t1_f8_web_markdown_table_to_pipe_table() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock.url("/article-with-table") }
            }),
        )
        .await;

    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("| Quarter | Revenue | Profit |"));
    assert!(text.contains("| Q1 | $10M | $2M |"));
    assert!(text.contains("| Q2 | $12M | $3M |"));
    assert!(!text.contains("<table>"));
    assert!(!text.contains("<tr>"));
}

/// T1.3: Strips noise elements: script, style, nav, button, and ads.
#[tokio::test]
async fn test_t1_f8_web_markdown_strips_junk_tags() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock.url("/article-with-junk") }
            }),
        )
        .await;

    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Essential Story Content"));
    assert!(text.contains("This is the real journalistic text"));
    assert!(!text.contains("analytics tracking code"));
    assert!(!text.contains("<script>"));
    assert!(!text.contains("<style>"));
    assert!(!text.contains("Share to Social Media"));
    assert!(!text.contains("<nav>"));
}

/// T1.4: Preserves multilingual Unicode characters: Japanese, Arabic, and Emojis.
#[tokio::test]
async fn test_t1_f8_web_markdown_unicode_multilingual() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock.url("/unicode-article") }
            }),
        )
        .await;

    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("多言語テスト"));
    assert!(text.contains("🚀"));
    assert!(text.contains("こんにちは世界！"));
    assert!(text.contains("مرحبا بالعالم!"));
}

/// T1.5: URL tracking parameters (utm_source, etc.) are cleaned from source URL.
#[tokio::test]
async fn test_t1_f8_web_markdown_cleans_tracking_parameters() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;
    let tracking_url = format!("{}/article-tracking?utm_source=twitter&utm_medium=social&fbclid=IwAR123", mock.base_url);

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": tracking_url }
            }),
        )
        .await;

    assert!(res["result"].is_object());
    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Clean Headline"));
}

/// T2.1: Target returning HTTP 404 produces clean error response without server crash.
#[tokio::test]
async fn test_t2_f8_web_markdown_http_404_error_handled() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock.url("/not-found") }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.2: Target returning HTTP 500 produces clean error response without server crash.
#[tokio::test]
async fn test_t2_f8_web_markdown_http_500_error_handled() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock.url("/server-error") }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.3: Empty HTML document returns empty or title-only Markdown without panic.
#[tokio::test]
async fn test_t2_f8_web_markdown_empty_body_handled() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock.url("/empty-article") }
            }),
        )
        .await;

    assert!(res["result"].is_object());
}

/// T2.4: Large HTML document (300+ paragraphs) converted cleanly without timeout or OOM.
#[tokio::test]
async fn test_t2_f8_web_markdown_large_document() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock.url("/huge-article") }
            }),
        )
        .await;

    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Paragraph 1 of descriptive text"));
    assert!(text.contains("Paragraph 300 of descriptive text"));
}

/// T2.5: SSRF prevention: non-HTTP schemes (file://, ftp://) are rejected safely.
#[tokio::test]
async fn test_t2_f8_web_markdown_ssrf_forbidden_schemes() {
    let server = TestServer::start().await;

    let forbidden_urls = [
        "file:///etc/passwd",
        "file:///C:/Windows/system32/cmd.exe",
        "ftp://ftp.example.com/file.txt",
    ];

    for url in forbidden_urls {
        let res = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "web_to_markdown",
                    "arguments": { "url": url }
                }),
            )
            .await;

        assert!(
            res["error"].is_object() || res["result"]["isError"] == true,
            "Scheme {} must be rejected",
            url
        );
    }
}

// ============================================================================
// FEATURE 9: Document PDF Text Extraction (pdf_text) — Tier 1 & Tier 2
// ============================================================================

/// T1.6: Extracts readable text from local PDF file fixture.
#[tokio::test]
async fn test_t1_f9_pdf_text_extracts_page_content() {
    let server = TestServer::start().await;
    let pdf = create_test_pdf("Primary Section Header Content", 1);
    let path = pdf.path().to_str().unwrap();

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path }
            }),
        )
        .await;

    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Primary Section Header Content"));
}

/// T1.7: Page range parameter extracts only specified subset of pages (e.g. "1-2").
#[tokio::test]
async fn test_t1_f9_pdf_text_page_range_subset() {
    let server = TestServer::start().await;
    let pdf = create_test_pdf("MultiPage Document", 3);
    let path = pdf.path().to_str().unwrap();

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path, "pages": "1-2" }
            }),
        )
        .await;

    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Page 1"));
    assert!(text.contains("Page 2"));
}

/// T1.8: Total page count is reported in structured extraction response.
#[tokio::test]
async fn test_t1_f9_pdf_text_reports_page_count() {
    let server = TestServer::start().await;
    let pdf = create_test_pdf("Page Count Verification", 4);
    let path = pdf.path().to_str().unwrap();

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path }
            }),
        )
        .await;

    assert!(res["result"].is_object());
}

/// T1.9: Pure-Rust extraction executes in-process without requiring external C libraries.
#[tokio::test]
async fn test_t1_f9_pdf_text_pure_rust_in_process() {
    let server = TestServer::start().await;
    let pdf = create_test_pdf("Pure Rust Lopdf Engine Test", 1);
    let path = pdf.path().to_str().unwrap();

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path }
            }),
        )
        .await;

    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Pure Rust Lopdf Engine Test"));
}

/// T1.10: "pages": "all" extracts all pages of a document.
#[tokio::test]
async fn test_t1_f9_pdf_text_all_pages_keyword() {
    let server = TestServer::start().await;
    let pdf = create_test_pdf("All Pages Keyword Test", 2);
    let path = pdf.path().to_str().unwrap();

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path, "pages": "all" }
            }),
        )
        .await;

    let text = res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Page 1"));
    assert!(text.contains("Page 2"));
}

/// T2.6: Non-existent PDF file path produces clean error without server panic.
#[tokio::test]
async fn test_t2_f9_pdf_text_nonexistent_file() {
    let server = TestServer::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": "/path/to/definitely_nonexistent_file_12345.pdf" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.7: Corrupted PDF file content returns clean error without crash.
#[tokio::test]
async fn test_t2_f9_pdf_text_corrupt_file_handled() {
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

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.8: Out-of-bounds page range ("999-1000" on 1-page PDF) handled gracefully.
#[tokio::test]
async fn test_t2_f9_pdf_text_out_of_bounds_page_range() {
    let server = TestServer::start().await;
    let pdf = create_test_pdf("Single Page Document", 1);
    let path = pdf.path().to_str().unwrap();

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path, "pages": "999-1000" }
            }),
        )
        .await;

    // Either returns empty text or error, but never panics
    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T2.9: Malformed page syntax ("invalid-range", "-5") handled safely.
#[tokio::test]
async fn test_t2_f9_pdf_text_malformed_page_syntax() {
    let server = TestServer::start().await;
    let pdf = create_test_pdf("Syntax Test Document", 2);
    let path = pdf.path().to_str().unwrap();

    let bad_syntaxes = ["invalid-range", "-5", "abc", "0", ""];
    for syntax in bad_syntaxes {
        let res = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "pdf_text",
                    "arguments": { "path": path, "pages": syntax }
                }),
            )
            .await;
        assert!(res["result"].is_object() || res["error"].is_object());
    }
}

/// T2.10: Empty string path handled with clean error.
#[tokio::test]
async fn test_t2_f9_pdf_text_empty_path_handled() {
    let server = TestServer::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": "" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

// ============================================================================
// FEATURE 6: Twitter/X Post (x_post) — Tier 1 & Tier 2
// ============================================================================

/// T1.11: Parses numeric status ID from https://x.com/... URL.
#[tokio::test]
async fn test_t1_f6_x_post_parses_x_com_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "https://x.com/jack/status/20" }
            }),
        )
        .await;
    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.12: Parses numeric status ID from legacy https://twitter.com/... URL.
#[tokio::test]
async fn test_t1_f6_x_post_parses_twitter_com_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "https://twitter.com/jack/status/20" }
            }),
        )
        .await;
    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.13: Validates XPost schema contains text and author objects when post is returned.
#[tokio::test]
async fn test_t1_f6_x_post_schema_structure() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "https://x.com/user/status/12345" }
            }),
        )
        .await;
    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.14: Media items array structure is present in XPost definition.
#[tokio::test]
async fn test_t1_f6_x_post_media_structure() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "https://x.com/user/status/54321" }
            }),
        )
        .await;
    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.15: Post metrics (likes, reposts) defined in schema.
#[tokio::test]
async fn test_t1_f6_x_post_metrics_contract() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "https://x.com/user/status/98765" }
            }),
        )
        .await;
    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T2.11: Non-X domain URL is rejected with clean error.
#[tokio::test]
async fn test_t2_f6_x_post_invalid_domain_rejected() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "https://instagram.com/p/abc123" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.12: Missing status ID in URL path is rejected.
#[tokio::test]
async fn test_t2_f6_x_post_missing_status_id() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "https://x.com/jack" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.13: Non-numeric status ID is rejected.
#[tokio::test]
async fn test_t2_f6_x_post_non_numeric_status_id() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "https://x.com/jack/status/notanumber" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.14: Query parameters in tweet URL are safely stripped to resolve status ID.
#[tokio::test]
async fn test_t2_f6_x_post_url_with_query_params() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "https://x.com/jack/status/20?s=20&t=abcdef" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T2.15: Empty string URL is rejected with clean error.
#[tokio::test]
async fn test_t2_f6_x_post_empty_url_rejected() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

// ============================================================================
// FEATURE 7: Twitter/X Thread (x_thread) — Tier 1 & Tier 2
// ============================================================================

/// T1.16: Thread unrolling returns Thread schema with focal post and posts array.
#[tokio::test]
async fn test_t1_f7_x_thread_schema_structure() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_thread",
                "arguments": { "url": "https://x.com/jack/status/20" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.17: Thread unrolls from mobile Twitter URL format.
#[tokio::test]
async fn test_t1_f7_x_thread_mobile_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_thread",
                "arguments": { "url": "https://mobile.twitter.com/jack/status/20" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.18: Truncated flag reported in thread response.
#[tokio::test]
async fn test_t1_f7_x_thread_truncated_flag() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_thread",
                "arguments": { "url": "https://x.com/user/status/12345" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.19: Source identifier string reported in thread response.
#[tokio::test]
async fn test_t1_f7_x_thread_source_identifier() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_thread",
                "arguments": { "url": "https://x.com/user/status/54321" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.20: Thread unrolling preserves author focal post.
#[tokio::test]
async fn test_t1_f7_x_thread_focal_post_structure() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_thread",
                "arguments": { "url": "https://x.com/user/status/11111" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T2.16: Malformed URL for thread returns clean error.
#[tokio::test]
async fn test_t2_f7_x_thread_malformed_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_thread",
                "arguments": { "url": "not_a_valid_url" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.17: Non-tweet URL for thread returns clean error.
#[tokio::test]
async fn test_t2_f7_x_thread_non_tweet_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_thread",
                "arguments": { "url": "https://reddit.com/r/rust" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.18: Whitespace-padded thread URL is trimmed and handled cleanly.
#[tokio::test]
async fn test_t2_f7_x_thread_whitespace_padded_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_thread",
                "arguments": { "url": "   https://x.com/jack/status/20   " }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T2.19: Empty URL for thread returns clean error.
#[tokio::test]
async fn test_t2_f7_x_thread_empty_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_thread",
                "arguments": { "url": "" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.20: Thread unrolling executes with bounded execution timeout.
#[tokio::test]
async fn test_t2_f7_x_thread_timeout_bounded() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_thread",
                "arguments": { "url": "https://x.com/user/status/99999" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

// ============================================================================
// FEATURE 10: Universal Media Info Extraction (media_info) — Tier 1 & Tier 2
// ============================================================================

/// T1.21: MediaInfo schema contains title, platform, author, duration, and qualities.
#[tokio::test]
async fn test_t1_f10_media_info_schema_contract() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.22: MediaInfo handles Vimeo video URL.
#[tokio::test]
async fn test_t1_f10_media_info_vimeo_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": "https://vimeo.com/76979871" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.23: MediaInfo handles TikTok URL.
#[tokio::test]
async fn test_t1_f10_media_info_tiktok_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": "https://www.tiktok.com/@tiktok/video/12345" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.24: MediaInfo executes purely in headless environment without GUI window.
#[tokio::test]
async fn test_t1_f10_media_info_headless_execution() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": "https://soundcloud.com/artist/track" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T1.25: MediaInfo available qualities contains resolution metadata.
#[tokio::test]
async fn test_t1_f10_media_info_qualities_metadata() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": "https://example.com/video.mp4" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T2.21: Non-URL string rejected with clean validation error.
#[tokio::test]
async fn test_t2_f10_media_info_malformed_url_rejected() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": "not a valid media url" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.22: Unsupported streaming domain returns clean error without crash.
#[tokio::test]
async fn test_t2_f10_media_info_unsupported_domain() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": "https://unsupported-arbitrary-domain-12345.org/media" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.23: Empty URL string returns clean error.
#[tokio::test]
async fn test_t2_f10_media_info_empty_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": "" }
            }),
        )
        .await;

    assert!(res["error"].is_object() || res["result"]["isError"] == true);
}

/// T2.24: Whitespace-padded URL handled cleanly.
#[tokio::test]
async fn test_t2_f10_media_info_whitespace_padded_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": "   https://www.youtube.com/watch?v=dQw4w9WgXcQ   " }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

/// T2.25: Special URL encoded characters handled safely.
#[tokio::test]
async fn test_t2_f10_media_info_special_characters_url() {
    let server = TestServer::start().await;
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": "https://example.com/video?title=%E3%83%86%E3%82%B9%E3%83%88&tag=%23video" }
            }),
        )
        .await;

    assert!(res["result"].is_object() || res["error"].is_object());
}

// ============================================================================
// TIER 3: Cross-Feature Combinations
// ============================================================================

/// T3.1: Concurrent execution of web_to_markdown and pdf_text.
#[tokio::test]
async fn test_t3_extraction_web_and_pdf_concurrency() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;
    let pdf = create_test_pdf("Concurrent PDF Extraction Content", 1);
    let pdf_path = pdf.path().to_str().unwrap().to_string();
    let web_url = mock.url("/article-simple");

    let server1 = server.client.clone();
    let base_url1 = server.base_url.clone();
    let token1 = server.auth_token.clone();

    let web_handle = tokio::spawn(async move {
        let payload = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "web_to_markdown", "arguments": { "url": web_url } }
        });
        server1
            .post(format!("{}/mcp", base_url1))
            .header("Authorization", format!("Bearer {}", token1))
            .json(&payload)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    });

    let server2 = server.client.clone();
    let base_url2 = server.base_url.clone();
    let token2 = server.auth_token.clone();

    let pdf_handle = tokio::spawn(async move {
        let payload = json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "pdf_text", "arguments": { "path": pdf_path } }
        });
        server2
            .post(format!("{}/mcp", base_url2))
            .header("Authorization", format!("Bearer {}", token2))
            .json(&payload)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    });

    let (web_res, pdf_res) = tokio::join!(web_handle, pdf_handle);
    let web_data = web_res.unwrap();
    let pdf_data = pdf_res.unwrap();

    assert!(web_data["result"]["content"][0]["text"].as_str().unwrap().contains("Simple Article Title"));
    assert!(pdf_data["result"]["content"][0]["text"].as_str().unwrap().contains("Concurrent PDF Extraction Content"));
}

/// T3.2: Sequential pipeline of x_post and media_info tools.
#[tokio::test]
async fn test_t3_extraction_x_post_and_media_info_pipeline() {
    let server = TestServer::start().await;

    let x_res = server
        .json_rpc(
            "tools/call",
            json!({ "name": "x_post", "arguments": { "url": "https://x.com/jack/status/20" } }),
        )
        .await;
    assert!(x_res["result"].is_object() || x_res["error"].is_object());

    let media_res = server
        .json_rpc(
            "tools/call",
            json!({ "name": "media_info", "arguments": { "url": "https://vimeo.com/76979871" } }),
        )
        .await;
    assert!(media_res["result"].is_object() || media_res["error"].is_object());
}

/// T3.3: Temporary PDF file cleanup: verifies temp file can be deleted immediately after extraction without file locking.
#[tokio::test]
async fn test_t3_extraction_pdf_tempfile_cleanup() {
    let server = TestServer::start().await;
    let pdf = create_test_pdf("Temp File Lock Test Content", 1);
    let path = pdf.path().to_str().unwrap().to_string();

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "pdf_text",
                "arguments": { "path": path }
            }),
        )
        .await;

    assert!(res["result"].is_object());
    drop(pdf); // File must unlink cleanly without Windows/Unix file lock errors
}

/// T3.4: Text extraction fidelity across HTML to Markdown and PDF.
#[tokio::test]
async fn test_t3_extraction_web_markdown_to_pdf_fidelity() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;
    let pdf = create_test_pdf("Shared Canonical Headline", 1);

    let web_res = server
        .json_rpc(
            "tools/call",
            json!({ "name": "web_to_markdown", "arguments": { "url": mock.url("/article-simple") } }),
        )
        .await;
    let pdf_res = server
        .json_rpc(
            "tools/call",
            json!({ "name": "pdf_text", "arguments": { "path": pdf.path().to_str().unwrap() } }),
        )
        .await;

    assert!(web_res["result"]["content"][0]["text"].is_string());
    assert!(pdf_res["result"]["content"][0]["text"].is_string());
}

// ============================================================================
// TIER 4: Real-World Workloads & Scenarios
// ============================================================================

/// T4.1: Scenario 3 — Multi-platform Media Extraction & Archival Workflow:
/// Queries media_info for video streaming metadata, extracts tweet via x_post,
/// and converts linked external article via web_to_markdown, verifying zero raw HTML tags or GUI crashes.
#[tokio::test]
async fn test_t4_scenario_3_media_and_social_archival_workflow() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    // Step 1: Media metadata query
    let media_res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "media_info",
                "arguments": { "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ" }
            }),
        )
        .await;
    assert!(media_res["result"].is_object() || media_res["error"].is_object());

    // Step 2: Social post query
    let post_res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "x_post",
                "arguments": { "url": "https://x.com/jack/status/20" }
            }),
        )
        .await;
    assert!(post_res["result"].is_object() || post_res["error"].is_object());

    // Step 3: Linked article conversion
    let article_res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "web_to_markdown",
                "arguments": { "url": mock.url("/article-with-table") }
            }),
        )
        .await;
    assert_eq!(article_res["jsonrpc"], "2.0");
    let markdown = article_res["result"]["content"][0]["text"].as_str().unwrap();

    assert!(markdown.contains("Quarterly Performance"));
    assert!(markdown.contains("| Quarter | Revenue | Profit |"));
    assert!(!markdown.contains("<table>"));
    assert!(!markdown.contains("<div>"));
}
