//! Comprehensive Integration Test Suite for Instagram & Facebook Social Extraction
//! (Milestone 4 — MCP tools/call, REST endpoints, OpenAPI contracts, and error boundaries)

mod common;

use common::TestServer;
use reqwest::StatusCode;
use serde_json::{json, Value};

// ============================================================================
// PART 1: Instagram Post — MCP tools/call
// ============================================================================

/// T1.1: MCP tools/call instagram_post rejects empty and whitespace arguments.
#[tokio::test]
async fn test_mcp_instagram_post_empty_args_rejected() {
    let server = TestServer::start().await;

    // Empty arguments object
    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "instagram_post",
                "arguments": {}
            }),
        )
        .await;

    assert_eq!(res["result"]["isError"], true);
    assert!(res["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("URL or shortcode cannot be empty"));

    // Whitespace URL
    let res_ws = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "instagram_post",
                "arguments": { "url": "   \t\n  " }
            }),
        )
        .await;

    assert_eq!(res_ws["result"]["isError"], true);
    assert!(res_ws["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("URL or shortcode cannot be empty"));
}

/// T1.2: MCP tools/call instagram_post rejects foreign and spoofed domains.
#[tokio::test]
async fn test_mcp_instagram_post_invalid_domains_rejected() {
    let server = TestServer::start().await;

    let bad_domains = [
        "https://twitter.com/jack/status/20",
        "https://facebook.com/posts/123",
        "https://instagram.com.attacker.com/p/C_abc123/",
        "https://notinstagram.com/p/C_abc123/",
        "https://evil.com/p/C_abc123",
    ];

    for bad_url in bad_domains {
        let res = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "instagram_post",
                    "arguments": { "url": bad_url }
                }),
            )
            .await;

        assert_eq!(res["result"]["isError"], true);
        assert!(
            res["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Invalid domain"),
            "URL '{}' must fail with Invalid domain",
            bad_url
        );
    }
}

/// T1.3: MCP tools/call instagram_post rejects unsupported ephemeral & browse paths.
#[tokio::test]
async fn test_mcp_instagram_post_unsupported_paths_rejected() {
    let server = TestServer::start().await;

    let unsupported_cases = [
        ("https://www.instagram.com/stories/natgeo/1234567890/", "Stories are ephemeral"),
        ("https://www.instagram.com/direct/t/123456/", "Direct Messages are private"),
        ("https://www.instagram.com/explore/tags/rust/", "Explore page cannot be extracted"),
        ("https://www.instagram.com/reels/audio/1234567890/", "Reel Audio pages are not supported"),
        ("https://www.instagram.com/reels/videos/12345/", "Reels video browse pages are not supported"),
        ("https://www.instagram.com/natgeo/", "Profile URL"),
        ("https://www.instagram.com/accounts/login/", "account management pages"),
    ];

    for (url, expected_err_fragment) in unsupported_cases {
        let res = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "instagram_post",
                    "arguments": { "url": url }
                }),
            )
            .await;

        assert_eq!(res["result"]["isError"], true);
        assert!(
            res["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(expected_err_fragment),
            "URL '{}' error text missing fragment '{}'",
            url,
            expected_err_fragment
        );
    }
}

/// T1.4: MCP tools/call instagram_post rejects malformed shortcodes.
#[tokio::test]
async fn test_mcp_instagram_post_malformed_shortcodes_rejected() {
    let server = TestServer::start().await;

    let malformed_urls = [
        "https://www.instagram.com/p/abc$xyz/",
        "https://www.instagram.com/p/a/", // too short (<3 chars)
        "https://www.instagram.com/p/",   // missing shortcode
    ];

    for url in malformed_urls {
        let res = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "instagram_post",
                    "arguments": { "url": url }
                }),
            )
            .await;

        assert_eq!(res["result"]["isError"], true);
    }
}

/// T1.5: MCP tools/call instagram_post contract handles standard post URLs and shortcode args.
#[tokio::test]
async fn test_mcp_instagram_post_contract_handling() {
    let server = TestServer::start().await;

    // Test with URL
    let res_url = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "instagram_post",
                "arguments": { "url": "https://www.instagram.com/p/C_abc123/" }
            }),
        )
        .await;

    assert_eq!(res_url["jsonrpc"], "2.0");
    assert!(res_url["result"]["content"].is_array());
    assert!(res_url["result"]["isError"].is_boolean());

    // Test with shortcode argument
    let res_sc = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "instagram_post",
                "arguments": { "shortcode": "C_abc123" }
            }),
        )
        .await;

    assert_eq!(res_sc["jsonrpc"], "2.0");
    assert!(res_sc["result"]["content"].is_array());
    assert!(res_sc["result"]["isError"].is_boolean());
}

// ============================================================================
// PART 2: Facebook Post — MCP tools/call
// ============================================================================

/// T2.1: MCP tools/call facebook_post rejects empty and whitespace arguments.
#[tokio::test]
async fn test_mcp_facebook_post_empty_args_rejected() {
    let server = TestServer::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "facebook_post",
                "arguments": {}
            }),
        )
        .await;

    assert_eq!(res["result"]["isError"], true);
    assert!(res["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("URL cannot be empty"));

    let res_ws = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "facebook_post",
                "arguments": { "url": "   " }
            }),
        )
        .await;

    assert_eq!(res_ws["result"]["isError"], true);
    assert!(res_ws["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("URL cannot be empty"));
}

/// T2.2: MCP tools/call facebook_post rejects foreign and spoofed domains.
#[tokio::test]
async fn test_mcp_facebook_post_invalid_domains_rejected() {
    let server = TestServer::start().await;

    let bad_domains = [
        "https://youtube.com/watch?v=123",
        "https://twitter.com/jack/status/20",
        "https://facebook.com.evil.com/posts/123",
        "https://evil-facebook.com/posts/123",
        "https://notfacebook.com/posts/123",
    ];

    for bad in bad_domains {
        let res = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "facebook_post",
                    "arguments": { "url": bad }
                }),
            )
            .await;

        assert_eq!(res["result"]["isError"], true);
        assert!(res["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Invalid domain"));
    }
}

/// T2.3: MCP tools/call facebook_post rejects unsupported platform paths & missing IDs.
#[tokio::test]
async fn test_mcp_facebook_post_unsupported_paths_and_missing_ids() {
    let server = TestServer::start().await;

    let test_cases = [
        "https://www.facebook.com/marketplace",
        "https://www.facebook.com/messages",
        "https://www.facebook.com/settings",
        "https://www.facebook.com/friends",
        "https://www.facebook.com/zuck", // Profile page
        "https://www.facebook.com/posts/", // Missing ID
        "https://www.facebook.com/reel/",  // Missing Reel ID
        "https://www.facebook.com/watch/", // Missing Watch ID
        "https://fb.watch/",              // Missing slug
    ];

    for url in test_cases {
        let res = server
            .json_rpc(
                "tools/call",
                json!({
                    "name": "facebook_post",
                    "arguments": { "url": url }
                }),
            )
            .await;

        assert_eq!(
            res["result"]["isError"], true,
            "URL '{}' must return isError: true",
            url
        );
    }
}

/// T2.4: MCP tools/call facebook_post detects login page URLs and reports login wall error.
#[tokio::test]
async fn test_mcp_facebook_post_login_wall_rejection() {
    let server = TestServer::start().await;

    let res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "facebook_post",
                "arguments": { "url": "https://www.facebook.com/login.php?next=..." }
            }),
        )
        .await;

    assert_eq!(res["result"]["isError"], true);
    assert!(res["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("login"));
}

/// T2.5: MCP tools/call facebook_post contract handles standard post and Watch URLs.
#[tokio::test]
async fn test_mcp_facebook_post_contract_handling() {
    let server = TestServer::start().await;

    // Test with standard post
    let res_post = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "facebook_post",
                "arguments": { "url": "https://www.facebook.com/zuck/posts/10115432729910941" }
            }),
        )
        .await;

    assert_eq!(res_post["jsonrpc"], "2.0");
    assert!(res_post["result"]["content"].is_array());
    assert!(res_post["result"]["isError"].is_boolean());

    // Test with Watch URL containing tracking parameters
    let res_watch = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "facebook_post",
                "arguments": { "url": "https://www.facebook.com/watch/?v=10153231379946729&mibextid=oXZqq4" }
            }),
        )
        .await;

    assert_eq!(res_watch["jsonrpc"], "2.0");
    assert!(res_watch["result"]["content"].is_array());
    assert!(res_watch["result"]["isError"].is_boolean());
}

// ============================================================================
// PART 3: REST API Integration Tests (/api/instagram/post & /api/facebook/post)
// ============================================================================

/// T3.1: Unauthenticated requests to Instagram and Facebook REST endpoints return HTTP 401.
#[tokio::test]
async fn test_rest_social_endpoints_unauthenticated_rejected() {
    let server = TestServer::start().await;

    let endpoints = [
        ("GET", "/api/instagram/post?url=https://www.instagram.com/p/C_abc123/"),
        ("POST", "/api/instagram/post"),
        ("GET", "/api/facebook/post?url=https://www.facebook.com/zuck/posts/10115432729910941"),
        ("POST", "/api/facebook/post"),
    ];

    for (method, path) in endpoints {
        let res = if method == "GET" {
            server.get(path).await
        } else {
            server.post_json(path, &json!({ "url": "https://example.com" })).await
        };

        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "Endpoint {} {} without Bearer token must return 401",
            method,
            path
        );

        let body: Value = res.json().await.expect("parse 401 json");
        assert_eq!(body["code"], "UNAUTHORIZED");
    }
}

/// T3.2: Missing URL/ID parameters on REST endpoints return HTTP 400 Bad Request.
#[tokio::test]
async fn test_rest_social_endpoints_missing_params_return_400() {
    let server = TestServer::start().await;

    // GET /api/instagram/post without query params
    let res_ig_get = server.get_authed("/api/instagram/post").await;
    assert_eq!(res_ig_get.status(), StatusCode::BAD_REQUEST);

    // POST /api/instagram/post with empty JSON body
    let res_ig_post = server.post_json_authed("/api/instagram/post", &json!({})).await;
    assert_eq!(res_ig_post.status(), StatusCode::BAD_REQUEST);

    // GET /api/facebook/post without query params
    let res_fb_get = server.get_authed("/api/facebook/post").await;
    assert_eq!(res_fb_get.status(), StatusCode::BAD_REQUEST);

    // POST /api/facebook/post with empty JSON body
    let res_fb_post = server.post_json_authed("/api/facebook/post", &json!({})).await;
    assert_eq!(res_fb_post.status(), StatusCode::BAD_REQUEST);
}

/// T3.3: Invalid domain on REST endpoints returns HTTP 400 Bad Request.
#[tokio::test]
async fn test_rest_social_endpoints_invalid_domain_returns_400() {
    let server = TestServer::start().await;

    // Instagram endpoint receiving Facebook URL
    let res_ig = server
        .post_json_authed(
            "/api/instagram/post",
            &json!({ "url": "https://www.facebook.com/posts/123" }),
        )
        .await;
    assert_eq!(res_ig.status(), StatusCode::BAD_REQUEST);

    // Facebook endpoint receiving YouTube URL
    let res_fb = server
        .post_json_authed(
            "/api/facebook/post",
            &json!({ "url": "https://www.youtube.com/watch?v=123" }),
        )
        .await;
    assert_eq!(res_fb.status(), StatusCode::BAD_REQUEST);
}

/// T3.4: GET and POST parameter equivalence on REST endpoints.
#[tokio::test]
async fn test_rest_social_endpoints_get_post_parity() {
    let server = TestServer::start().await;

    // Instagram: GET with ?url= vs POST with {"url": "..."}
    let bad_url = "https://invalid-domain.com/p/123";
    let res_get = server
        .get_authed(&format!("/api/instagram/post?url={}", urlencoding::encode(bad_url)))
        .await;
    let res_post = server
        .post_json_authed("/api/instagram/post", &json!({ "url": bad_url }))
        .await;

    assert_eq!(res_get.status(), StatusCode::BAD_REQUEST);
    assert_eq!(res_post.status(), StatusCode::BAD_REQUEST);

    // Facebook: GET with ?url= vs POST with {"url": "..."}
    let res_fb_get = server
        .get_authed(&format!("/api/facebook/post?url={}", urlencoding::encode(bad_url)))
        .await;
    let res_fb_post = server
        .post_json_authed("/api/facebook/post", &json!({ "url": bad_url }))
        .await;

    assert_eq!(res_fb_get.status(), StatusCode::BAD_REQUEST);
    assert_eq!(res_fb_post.status(), StatusCode::BAD_REQUEST);
}

/// T3.5: CORS Preflight (OPTIONS) on social REST endpoints.
#[tokio::test]
async fn test_rest_social_endpoints_cors_preflight() {
    let server = TestServer::start().await;

    for path in ["/api/instagram/post", "/api/facebook/post"] {
        let res = server
            .client
            .request(reqwest::Method::OPTIONS, &server.url(path))
            .header("Origin", "https://chatgpt.com")
            .header("Access-Control-Request-Method", "POST")
            .header("Access-Control-Request-Headers", "authorization,content-type")
            .send()
            .await
            .expect("send OPTIONS");

        assert!(
            res.status() == StatusCode::OK || res.status() == StatusCode::NO_CONTENT,
            "CORS OPTIONS {} failed with status: {:?}",
            path,
            res.status()
        );
    }
}

// ============================================================================
// PART 4: Concurrency & Cross-Transport Storm
// ============================================================================

/// T4.1: Concurrent multi-platform tool calls via MCP tools/call.
#[tokio::test]
async fn test_social_multi_tool_concurrency() {
    let server = TestServer::start().await;

    let server1 = server.client.clone();
    let url1 = server.base_url.clone();
    let token1 = server.auth_token.clone();
    let ig_handle = tokio::spawn(async move {
        server1
            .post(format!("{}/mcp", url1))
            .header("Authorization", format!("Bearer {}", token1))
            .json(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "instagram_post", "arguments": { "url": "https://notinstagram.com/1" } }
            }))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    });

    let server2 = server.client.clone();
    let url2 = server.base_url.clone();
    let token2 = server.auth_token.clone();
    let fb_handle = tokio::spawn(async move {
        server2
            .post(format!("{}/mcp", url2))
            .header("Authorization", format!("Bearer {}", token2))
            .json(&json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "facebook_post", "arguments": { "url": "https://notfacebook.com/1" } }
            }))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    });

    let (ig_res, fb_res) = tokio::join!(ig_handle, fb_handle);
    let ig_data = ig_res.unwrap();
    let fb_data = fb_res.unwrap();

    assert_eq!(ig_data["result"]["isError"], true);
    assert_eq!(fb_data["result"]["isError"], true);
}

#[tokio::test]
async fn test_user_live_facebook_url() {
    let url = "https://www.facebook.com/share/p/19jf8yJG86/";
    let server = TestServer::start().await;
    let mcp_res = server
        .json_rpc(
            "tools/call",
            json!({
                "name": "facebook_post",
                "arguments": { "url": url }
            }),
        )
        .await;

    assert!(mcp_res["result"]["isError"].is_null() || mcp_res["result"]["isError"] == false);
    assert_eq!(mcp_res["result"]["post"]["author"]["name"], "SanookAi");
    assert!(mcp_res["result"]["post"]["caption"].as_str().unwrap().contains("4.3 ล้านดาวน์โหลด"));
    assert!(!mcp_res["result"]["post"]["images"].as_array().unwrap().is_empty());
}

