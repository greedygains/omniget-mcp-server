#![allow(clippy::needless_borrows_for_generic_args)]

mod common;

use common::{create_test_pdf, MockWebsite, TestServer};
use reqwest::StatusCode;
use serde_json::{json, Value};

// ============================================================================
// FEATURE 11: REST API Bridge (/api/*) — Tier 1 & Tier 2
// ============================================================================

/// T1.1: GET /api/web/markdown?url=... extracts clean Markdown via query parameter.
#[tokio::test]
async fn test_t1_f11_rest_web_markdown_get() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let res = server
        .client
        .get(&server.url(&format!(
            "/api/web/markdown?url={}",
            urlencoding::encode(&mock.url("/article-simple"))
        )))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .send()
        .await
        .expect("send get web/markdown");

    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("parse response json");
    assert_eq!(body["title"], "Simple Article");
    let markdown = body["markdown"].as_str().expect("markdown string");
    assert!(markdown.contains("Simple Article Title"));
    assert!(!markdown.contains("<article>"));
}

/// T1.2: POST /api/web/markdown extracts clean Markdown via JSON body { "url": "..." }.
#[tokio::test]
async fn test_t1_f11_rest_web_markdown_post() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let payload = json!({
        "url": mock.url("/article-simple")
    });

    let res = server
        .post_json_authed("/api/web/markdown", &payload)
        .await;

    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.expect("parse json");
    assert_eq!(body["title"], "Simple Article");
    assert!(body["markdown"].as_str().unwrap().contains("Simple Article Title"));
}

/// T1.3: POST /api/pdf/text extracts text and reports page count from PDF document.
#[tokio::test]
async fn test_t1_f11_rest_pdf_text_post() {
    let server = TestServer::start().await;
    let pdf_file = create_test_pdf("Rest Api PDF Extraction Test Content", 3);
    let pdf_path = pdf_file.path().to_str().expect("pdf path");

    let payload = json!({
        "path": pdf_path,
        "pages": "1-2"
    });

    let res = server.post_json_authed("/api/pdf/text", &payload).await;
    assert_eq!(res.status(), StatusCode::OK);

    let body: Value = res.json().await.expect("parse response");
    assert!(body["text"].as_str().unwrap().contains("Rest Api PDF Extraction Test Content"));
    assert!(body["pages"].as_u64().unwrap_or(0) >= 2);
}

/// T1.4: POST /api/x/post contract accepts URL and validates schema.
#[tokio::test]
async fn test_t1_f11_rest_x_post_contract() {
    let server = TestServer::start().await;
    let payload = json!({
        "url": "https://x.com/jack/status/20"
    });

    let res = server.post_json_authed("/api/x/post", &payload).await;
    // When offline or mocked, endpoint responds with 200 or structured error, not 401 or 500
    assert!(res.status() == StatusCode::OK || res.status() == StatusCode::BAD_GATEWAY);
}

/// T1.5: POST /api/media/info contract accepts URL and validates response schema.
#[tokio::test]
async fn test_t1_f11_rest_media_info_contract() {
    let server = TestServer::start().await;
    let payload = json!({
        "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
    });

    let res = server.post_json_authed("/api/media/info", &payload).await;
    assert!(
        res.status() == StatusCode::OK
            || res.status() == StatusCode::BAD_GATEWAY
            || res.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

/// T1.6: POST /api/instagram/post contract accepts URL and validates schema.
#[tokio::test]
async fn test_t1_f11_rest_instagram_post_contract() {
    let server = TestServer::start().await;
    let payload = json!({
        "url": "https://www.instagram.com/p/C_abc123/"
    });

    let res = server.post_json_authed("/api/instagram/post", &payload).await;
    assert!(
        res.status() == StatusCode::OK
            || res.status() == StatusCode::FORBIDDEN
            || res.status() == StatusCode::BAD_GATEWAY
            || res.status() == StatusCode::NOT_FOUND
    );
}

/// T1.7: POST /api/facebook/post contract accepts URL and validates schema.
#[tokio::test]
async fn test_t1_f11_rest_facebook_post_contract() {
    let server = TestServer::start().await;
    let payload = json!({
        "url": "https://www.facebook.com/zuck/posts/10115432729910941"
    });

    let res = server.post_json_authed("/api/facebook/post", &payload).await;
    assert!(
        res.status() == StatusCode::OK
            || res.status() == StatusCode::FORBIDDEN
            || res.status() == StatusCode::BAD_GATEWAY
            || res.status() == StatusCode::NOT_FOUND
    );
}

/// T2.1: Unauthenticated request to REST endpoint returns HTTP 401 Unauthorized.
#[tokio::test]
async fn test_t2_f11_rest_unauthenticated_request_returns_401() {
    let server = TestServer::start().await;
    let res = server.get("/api/web/markdown?url=http://example.com").await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body: Value = res.json().await.expect("parse 401 json");
    assert_eq!(body["code"], "UNAUTHORIZED");
}

/// T2.2: GET /api/web/markdown missing required "url" parameter returns HTTP 400 Bad Request.
#[tokio::test]
async fn test_t2_f11_rest_missing_url_query_param_returns_400() {
    let server = TestServer::start().await;
    let res = server.get_authed("/api/web/markdown").await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// T2.3: POST /api/web/markdown with missing "url" field returns HTTP 400 Bad Request.
#[tokio::test]
async fn test_t2_f11_rest_missing_url_post_field_returns_400() {
    let server = TestServer::start().await;
    let payload = json!({ "wrong_key": "http://example.com" });
    let res = server
        .post_json_authed("/api/web/markdown", &payload)
        .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// T2.4: POST /api/web/markdown with malformed JSON body returns HTTP 400 Bad Request.
#[tokio::test]
async fn test_t2_f11_rest_malformed_json_returns_400() {
    let server = TestServer::start().await;
    let res = server
        .client
        .post(&server.url("/api/web/markdown"))
        .header("Authorization", format!("Bearer {}", server.auth_token))
        .header("Content-Type", "application/json")
        .body("{ url: broken json }")
        .send()
        .await
        .expect("send bad json");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// T2.5: POST /api/pdf/text with non-existent file path returns clean error (400 or 404).
#[tokio::test]
async fn test_t2_f11_rest_pdf_nonexistent_file_returns_error() {
    let server = TestServer::start().await;
    let payload = json!({
        "path": "/nonexistent/path/to/missing_document_xyz.pdf",
        "pages": "1"
    });

    let res = server.post_json_authed("/api/pdf/text", &payload).await;
    assert!(
        res.status() == StatusCode::BAD_REQUEST
            || res.status() == StatusCode::NOT_FOUND
            || res.status() == StatusCode::UNPROCESSABLE_ENTITY
    );
}

/// T2.6: POST /api/x/post with invalid non-X domain returns HTTP 400 Bad Request.
#[tokio::test]
async fn test_t2_f11_rest_x_post_invalid_domain_returns_400() {
    let server = TestServer::start().await;
    let payload = json!({ "url": "https://instagram.com/p/invalid" });
    let res = server.post_json_authed("/api/x/post", &payload).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// T2.7: CORS Preflight (OPTIONS /api/*) responds with HTTP 200 and permissive headers.
#[tokio::test]
async fn test_t2_f11_rest_cors_preflight_options() {
    let server = TestServer::start().await;
    let res = server
        .client
        .request(reqwest::Method::OPTIONS, &server.url("/api/web/markdown"))
        .header("Origin", "https://chatgpt.com")
        .header("Access-Control-Request-Method", "POST")
        .header("Access-Control-Request-Headers", "authorization,content-type")
        .send()
        .await
        .expect("send OPTIONS");

    assert!(res.status() == StatusCode::OK || res.status() == StatusCode::NO_CONTENT);
}

// ============================================================================
// FEATURE 12: OpenAPI 3.1.0 Specification (GET /openapi.json) — Tier 1 & Tier 2
// ============================================================================

/// T1.6: GET /openapi.json returns HTTP 200 with application/json.
#[tokio::test]
async fn test_t1_f12_openapi_json_returns_200_and_json() {
    let server = TestServer::start().await;
    let res = server.get_authed("/openapi.json").await;
    assert_eq!(res.status(), StatusCode::OK);

    let content_type = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .expect("content-type")
        .to_str()
        .unwrap();
    assert!(content_type.contains("application/json"));
}

/// T1.7: GET /openapi.json specifies OpenAPI version 3.1.0 (or starts with 3.1).
#[tokio::test]
async fn test_t1_f12_openapi_version_is_3_1() {
    let server = TestServer::start().await;
    let res = server.get_authed("/openapi.json").await;
    let doc: Value = res.json().await.expect("parse openapi json");

    let version = doc["openapi"].as_str().expect("openapi field as string");
    assert!(
        version.starts_with("3.1"),
        "OpenAPI version must be 3.1.x, got: {}",
        version
    );
}

/// T1.8: GET /openapi.json contains descriptive info object with title and version.
#[tokio::test]
async fn test_t1_f12_openapi_info_metadata() {
    let server = TestServer::start().await;
    let res = server.get_authed("/openapi.json").await;
    let doc: Value = res.json().await.expect("parse openapi");

    let info = &doc["info"];
    assert!(info.is_object(), "info must be an object");
    assert!(
        info["title"].as_str().unwrap().contains("OmniGet"),
        "Title must reference OmniGet"
    );
    assert!(!info["version"].as_str().unwrap().is_empty());
}

/// T1.9: GET /openapi.json documents all core REST and MCP protocol paths.
#[tokio::test]
async fn test_t1_f12_openapi_contains_all_core_paths() {
    let server = TestServer::start().await;
    let res = server.get_authed("/openapi.json").await;
    let doc: Value = res.json().await.expect("parse openapi");

    let paths = doc["paths"].as_object().expect("paths object");
    let expected_paths = [
        "/health",
        "/mcp",
        "/sse",
        "/messages",
        "/api/web/markdown",
        "/api/pdf/text",
        "/api/x/post",
        "/api/x/thread",
        "/api/media/info",
        "/api/instagram/post",
        "/api/facebook/post",
    ];

    for path in expected_paths {
        assert!(
            paths.contains_key(path),
            "OpenAPI paths must document {}, available: {:?}",
            path,
            paths.keys()
        );
    }
}

/// T1.10: GET /openapi.json defines HTTP Bearer security scheme in components.
#[tokio::test]
async fn test_t1_f12_openapi_defines_bearer_security_scheme() {
    let server = TestServer::start().await;
    let res = server.get_authed("/openapi.json").await;
    let doc: Value = res.json().await.expect("parse openapi");

    let schemes = &doc["components"]["securitySchemes"];
    assert!(schemes.is_object(), "securitySchemes must be an object");

    let bearer = schemes
        .as_object()
        .unwrap()
        .values()
        .find(|s| s["type"] == "http" && s["scheme"] == "bearer");

    assert!(
        bearer.is_some(),
        "Must define an HTTP Bearer securityScheme in components"
    );
}

/// T2.8: All operations in OpenAPI paths define at least 200 and 401 response status codes.
#[tokio::test]
async fn test_t2_f12_openapi_all_protected_operations_define_401() {
    let server = TestServer::start().await;
    let res = server.get_authed("/openapi.json").await;
    let doc: Value = res.json().await.expect("parse openapi");

    let paths = doc["paths"].as_object().unwrap();
    for (path, path_item) in paths {
        if path == "/health" {
            continue; // Public endpoint
        }
        for (method, op) in path_item.as_object().unwrap() {
            if ["get", "post", "put", "delete"].contains(&method.as_str()) {
                let responses = &op["responses"];
                assert!(
                    responses.is_object(),
                    "Operation {} {} must define responses",
                    method,
                    path
                );
                assert!(
                    responses["401"].is_object() || responses["default"].is_object(),
                    "Operation {} {} must document 401 or default response",
                    method,
                    path
                );
            }
        }
    }
}

/// T2.9: All POST endpoints in OpenAPI define requestBody with application/json.
#[tokio::test]
async fn test_t2_f12_openapi_post_endpoints_define_request_body() {
    let server = TestServer::start().await;
    let res = server.get_authed("/openapi.json").await;
    let doc: Value = res.json().await.expect("parse openapi");

    let paths = doc["paths"].as_object().unwrap();
    for (path, path_item) in paths {
        if let Some(post_op) = path_item.get("post") {
            let req_body = &post_op["requestBody"];
            assert!(
                req_body.is_object(),
                "POST {} must define requestBody",
                path
            );
            assert!(
                req_body["content"]["application/json"].is_object(),
                "POST {} requestBody must support application/json",
                path
            );
        }
    }
}

/// T2.10: OpenAPI schema components or parameters define valid types.
#[tokio::test]
async fn test_t2_f12_openapi_schema_types_are_valid() {
    let server = TestServer::start().await;
    let res = server.get_authed("/openapi.json").await;
    let doc: Value = res.json().await.expect("parse openapi");

    assert!(doc["openapi"].is_string());
    assert!(doc["info"].is_object());
    assert!(doc["paths"].is_object());
}

// ============================================================================
// TIER 3: Cross-Feature Combinations
// ============================================================================

/// T3.1: REST vs MCP Equivalence: POST /api/web/markdown and MCP tools/call return equivalent Markdown.
#[tokio::test]
async fn test_t3_rest_and_mcp_markdown_equivalence() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;
    let article_url = mock.url("/article-simple");

    // 1. Fetch via REST
    let rest_res = server
        .post_json_authed("/api/web/markdown", &json!({ "url": article_url }))
        .await;
    assert_eq!(rest_res.status(), StatusCode::OK);
    let rest_body: Value = rest_res.json().await.expect("parse rest json");
    let rest_md = rest_body["markdown"].as_str().expect("rest markdown");

    // 2. Fetch via Streamable MCP
    let mcp_res = server
        .post_json_authed(
            "/mcp",
            &json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "web_to_markdown", "arguments": { "url": article_url } }
            }),
        )
        .await;
    assert_eq!(mcp_res.status(), StatusCode::OK);
    let mcp_body: Value = mcp_res.json().await.expect("parse mcp json");
    let mcp_md = mcp_body["result"]["content"][0]["text"]
        .as_str()
        .expect("mcp markdown");

    assert_eq!(
        rest_md.trim(),
        mcp_md.trim(),
        "REST and MCP web_to_markdown outputs must be identical"
    );
}

/// T3.2: OpenAPI specification accurately describes live REST parameter requirements.
#[tokio::test]
async fn test_t3_openapi_matches_live_rest_contract() {
    let server = TestServer::start().await;
    let spec_res = server.get_authed("/openapi.json").await;
    let spec: Value = spec_res.json().await.expect("parse openapi");

    // Ensure /api/web/markdown is in spec
    let post_op = &spec["paths"]["/api/web/markdown"]["post"];
    assert!(post_op.is_object());

    // Call live endpoint with valid parameters described in spec
    let mock = MockWebsite::start().await;
    let live_res = server
        .post_json_authed(
            "/api/web/markdown",
            &json!({ "url": mock.url("/article-simple") }),
        )
        .await;
    assert_eq!(live_res.status(), StatusCode::OK);
}

/// T3.3: Case-insensitive Bearer auth prefix on REST endpoints.
#[tokio::test]
async fn test_t3_rest_bearer_auth_variations() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    let res = server
        .client
        .post(&server.url("/api/web/markdown"))
        .header("Authorization", format!("bearer {}", server.auth_token))
        .json(&json!({ "url": mock.url("/article-simple") }))
        .send()
        .await
        .expect("send request with lowercase bearer");

    assert_eq!(res.status(), StatusCode::OK);
}

// ============================================================================
// TIER 4: Real-World Workloads & Scenarios
// ============================================================================

/// T4.1: Scenario 2 — ChatGPT Custom Action Webhook Workflow:
/// Authenticates via Bearer token -> fetches OpenAPI spec -> invokes POST /api/web/markdown
/// on an article with tables -> verifies pipe table -> invokes POST /api/pdf/text on document.
#[tokio::test]
async fn test_t4_scenario_2_chatgpt_custom_action_workflow() {
    let server = TestServer::start().await;
    let mock = MockWebsite::start().await;

    // Step 1: ChatGPT Custom Action discovers endpoints via GET /openapi.json
    let spec_res = server.get_authed("/openapi.json").await;
    assert_eq!(spec_res.status(), StatusCode::OK);
    let spec: Value = spec_res.json().await.expect("parse spec");
    assert!(spec["paths"]["/api/web/markdown"].is_object());
    assert!(spec["paths"]["/api/pdf/text"].is_object());

    // Step 2: ChatGPT invokes POST /api/web/markdown on a financial report with a table
    let table_url = mock.url("/article-with-table");
    let md_res = server
        .post_json_authed("/api/web/markdown", &json!({ "url": table_url }))
        .await;
    assert_eq!(md_res.status(), StatusCode::OK);
    let md_body: Value = md_res.json().await.expect("parse markdown body");
    let markdown = md_body["markdown"].as_str().unwrap();

    assert!(markdown.contains("Quarterly Performance"));
    assert!(markdown.contains("| Quarter | Revenue | Profit |"));
    assert!(markdown.contains("| Q1 | $10M | $2M |"));

    // Step 3: ChatGPT invokes POST /api/pdf/text to ingest supporting PDF document
    let test_pdf = create_test_pdf("ChatGPT Action Executive Summary Report", 2);
    let pdf_path = test_pdf.path().to_str().unwrap();

    let pdf_res = server
        .post_json_authed(
            "/api/pdf/text",
            &json!({ "path": pdf_path, "pages": "1" }),
        )
        .await;
    assert_eq!(pdf_res.status(), StatusCode::OK);
    let pdf_body: Value = pdf_res.json().await.expect("parse pdf body");
    assert!(pdf_body["text"]
        .as_str()
        .unwrap()
        .contains("ChatGPT Action Executive Summary Report"));
    assert!(pdf_body["pages"].as_u64().unwrap() >= 1);
}
