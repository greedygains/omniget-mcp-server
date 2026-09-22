//! Integration tests for security guardrails, Bearer token authentication, and health check.

mod common;

use common::TestServer;
use reqwest::{header, StatusCode};
use serde_json::Value;

#[tokio::test]
async fn test_health_check_unauthenticated_returns_200() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/health", server.base_url))
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(res.status(), StatusCode::OK);
    let json: Value = res.json().await.expect("Failed to parse JSON");
    assert_eq!(json, serde_json::json!({ "ok": true }));
}

#[tokio::test]
async fn test_health_check_with_invalid_token_still_returns_200() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();

    let res = client
        .get(format!("{}/health", server.base_url))
        .header(header::AUTHORIZATION, "Bearer invalid-token-12345")
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(res.status(), StatusCode::OK);
    let json: Value = res.json().await.expect("Failed to parse JSON");
    assert_eq!(json, serde_json::json!({ "ok": true }));
}

#[tokio::test]
async fn test_protected_routes_unauthenticated_return_401() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();

    let endpoints = vec![
        ("POST", format!("{}/mcp", server.base_url)),
        ("GET", format!("{}/sse", server.base_url)),
        ("POST", format!("{}/messages", server.base_url)),
        ("GET", format!("{}/openapi.json", server.base_url)),
        ("GET", format!("{}/api/x/post", server.base_url)),
        ("GET", format!("{}/api/x/thread", server.base_url)),
        ("GET", format!("{}/api/web/markdown", server.base_url)),
        ("GET", format!("{}/api/pdf/text", server.base_url)),
        ("GET", format!("{}/api/media/info", server.base_url)),
    ];

    for (method, url) in endpoints {
        let req = match method {
            "POST" => client.post(&url),
            _ => client.get(&url),
        };

        let res = req.send().await.expect("Failed to execute request");
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "Endpoint {method} {url} should return 401 when unauthenticated"
        );

        let www_auth = res.headers().get(header::WWW_AUTHENTICATE);
        assert_eq!(
            www_auth.map(|v| v.to_str().unwrap()),
            Some("Bearer"),
            "Expected WWW-Authenticate: Bearer header on {url}"
        );

        let json: Value = res.json().await.expect("Failed to parse 401 JSON");
        assert_eq!(
            json,
            serde_json::json!({
                "ok": false,
                "code": "UNAUTHORIZED",
                "message": "Invalid or missing bearer token"
            }),
            "401 JSON body mismatch on {url}"
        );
    }
}

#[tokio::test]
async fn test_protected_routes_invalid_token_returns_401() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();

    let res = client
        .post(format!("{}/mcp", server.base_url))
        .header(header::AUTHORIZATION, "Bearer wrong-secret")
        .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let json: Value = res.json().await.expect("Failed to parse 401 JSON");
    assert_eq!(
        json,
        serde_json::json!({
            "ok": false,
            "code": "UNAUTHORIZED",
            "message": "Invalid or missing bearer token"
        })
    );
}

#[tokio::test]
async fn test_protected_routes_invalid_scheme_returns_401() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();

    let bad_auth_headers = [
        "Basic dXNlcjpwYXNz",
        "Token your-secure-token",
        "your-secure-token",
        "Bearer",
        "Bearer ",
    ];

    for auth_val in bad_auth_headers {
        let res = client
            .get(format!("{}/openapi.json", server.base_url))
            .header(header::AUTHORIZATION, auth_val)
            .send()
            .await
            .expect("Failed to send request");

        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "Expected 401 for Authorization: '{auth_val}'"
        );
    }
}

#[tokio::test]
async fn test_protected_routes_valid_token_returns_success() {
    let token = "your-custom-test-token";
    let server = TestServer::spawn_with_token(token).await;
    let client = reqwest::Client::new();
    let auth_token = &server.auth_token;

    // 1. POST /mcp with Bearer token -> 200 OK
    let res = client
        .post(format!("{}/mcp", server.base_url))
        .header(header::AUTHORIZATION, format!("Bearer {auth_token}"))
        .send()
        .await
        .expect("Failed to send request");
    assert_eq!(res.status(), StatusCode::OK);

    // 2. GET /openapi.json with lowercase bearer -> 200 OK
    let res = client
        .get(format!("{}/openapi.json", server.base_url))
        .header(header::AUTHORIZATION, format!("bearer {auth_token}"))
        .send()
        .await
        .expect("Failed to send request");
    assert_eq!(res.status(), StatusCode::OK);

    // 3. GET /sse with padded Bearer -> 200 OK
    let res = client
        .get(format!("{}/sse", server.base_url))
        .header(header::AUTHORIZATION, format!("  Bearer   {auth_token}  "))
        .send()
        .await
        .expect("Failed to send request");
    assert_eq!(res.status(), StatusCode::OK);

    // 4. POST /messages with Bearer token -> 202 Accepted
    let res = client
        .post(format!("{}/messages", server.base_url))
        .header(header::AUTHORIZATION, format!("Bearer {auth_token}"))
        .send()
        .await
        .expect("Failed to send request");
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    // 5. GET /api/web/markdown with Bearer token -> auth accepted (not 401)
    let res = client
        .get(format!("{}/api/web/markdown", server.base_url))
        .header(header::AUTHORIZATION, format!("Bearer {auth_token}"))
        .send()
        .await
        .expect("Failed to send request");
    assert_ne!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(res.status() == StatusCode::OK || res.status() == StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_cors_options_preflight_unauthenticated_returns_200() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();

    let res = client
        .request(reqwest::Method::OPTIONS, format!("{}/mcp", server.base_url))
        .header(header::ORIGIN, "https://chatgpt.com")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization,content-type")
        .send()
        .await
        .expect("Failed to send OPTIONS preflight request");

    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
}
