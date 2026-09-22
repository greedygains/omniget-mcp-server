//! Adversarial Stress & Correctness Verification Suite for Milestone 1
//! Covers:
//! 1. Timing-safe constant-time Bearer verification (`check_bearer`).
//! 2. Scheme mutations (mixed case, tabs, spaces, invalid schemes, huge 64KB token, Unicode non-ASCII).
//! 3. Port parsing stress (boundary values, overflow, negative, whitespace, fullwidth Unicode).
//! 4. Fail-closed behavior on empty or blank secret.
//! 5. High-concurrency async load stress on /health and protected endpoints.
//! 6. RFC 6750 HTTP 401 error response contract compliance.
//! 7. Public health endpoint path and method boundaries.

mod common;

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use common::TestServer;
use omniget_server::auth::{check_bearer, AuthState};
use omniget_server::server::parse_port_from_str;
use reqwest::Client;
use std::hint::black_box;
use std::time::Instant;

/// Helper to construct a HeaderMap with a single Authorization header string.
fn auth_header(val: &str) -> HeaderMap {
    let mut map = HeaderMap::new();
    map.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(val).expect("valid header value string"),
    );
    map
}

/// Helper to construct a HeaderMap with raw bytes (for non-ASCII / Unicode testing).
fn auth_header_bytes(bytes: &[u8]) -> HeaderMap {
    let mut map = HeaderMap::new();
    map.insert(
        header::AUTHORIZATION,
        HeaderValue::from_bytes(bytes).expect("valid header value bytes"),
    );
    map
}

// =========================================================================
// 1. Timing Measurement Test (Constant-Time Verification)
// =========================================================================

#[test]
fn test_adversarial_constant_time_verification() {
    // Construct a 64-character secret token
    let secret = "A".repeat(64);

    // Candidate 1: 0 matching prefix characters ("B" followed by 63 "A"s)
    let candidate_prefix_0 = format!("B{}", "A".repeat(63));
    let headers_prefix_0 = auth_header(&format!("Bearer {candidate_prefix_0}"));

    // Candidate 2: 32 matching prefix characters (32 "A"s followed by 32 "B"s)
    let candidate_prefix_32 = format!("{}{}", "A".repeat(32), "B".repeat(32));
    let headers_prefix_32 = auth_header(&format!("Bearer {candidate_prefix_32}"));

    // Candidate 3: 63 matching prefix characters (63 "A"s followed by 1 "B")
    let candidate_prefix_63 = format!("{}B", "A".repeat(63));
    let headers_prefix_63 = auth_header(&format!("Bearer {candidate_prefix_63}"));

    // Candidate 4: Exact match (64 "A"s)
    let candidate_match = "A".repeat(64);
    let headers_match = auth_header(&format!("Bearer {candidate_match}"));

    // Sanity checks on correctness
    assert!(!check_bearer(&headers_prefix_0, &secret));
    assert!(!check_bearer(&headers_prefix_32, &secret));
    assert!(!check_bearer(&headers_prefix_63, &secret));
    assert!(check_bearer(&headers_match, &secret));

    // Benchmark using batched iterations to eliminate clock read overhead.
    // We run multiple batches alternating candidate 0 and candidate 63.
    const BATCH_SIZE: usize = 10_000;
    const NUM_BATCHES: usize = 10;
    let mut total_duration_0 = std::time::Duration::ZERO;
    let mut total_duration_63 = std::time::Duration::ZERO;

    // Warm up
    for _ in 0..BATCH_SIZE {
        black_box(check_bearer(black_box(&headers_prefix_0), black_box(&secret)));
        black_box(check_bearer(black_box(&headers_prefix_63), black_box(&secret)));
    }

    for batch_idx in 0..NUM_BATCHES {
        if batch_idx % 2 == 0 {
            // Measure candidate 0 first
            let start_0 = Instant::now();
            for _ in 0..BATCH_SIZE {
                let res = check_bearer(black_box(&headers_prefix_0), black_box(&secret));
                black_box(res);
            }
            total_duration_0 += start_0.elapsed();

            // Measure candidate 63 second
            let start_63 = Instant::now();
            for _ in 0..BATCH_SIZE {
                let res = check_bearer(black_box(&headers_prefix_63), black_box(&secret));
                black_box(res);
            }
            total_duration_63 += start_63.elapsed();
        } else {
            // Measure candidate 63 first
            let start_63 = Instant::now();
            for _ in 0..BATCH_SIZE {
                let res = check_bearer(black_box(&headers_prefix_63), black_box(&secret));
                black_box(res);
            }
            total_duration_63 += start_63.elapsed();

            // Measure candidate 0 second
            let start_0 = Instant::now();
            for _ in 0..BATCH_SIZE {
                let res = check_bearer(black_box(&headers_prefix_0), black_box(&secret));
                black_box(res);
            }
            total_duration_0 += start_0.elapsed();
        }
    }

    let total_calls = (BATCH_SIZE * NUM_BATCHES) as f64;
    let nanos_0 = total_duration_0.as_nanos() as f64 / total_calls;
    let nanos_63 = total_duration_63.as_nanos() as f64 / total_calls;
    let ratio = nanos_63 / nanos_0;

    println!(
        "[Timing Verification] Batched: 0-prefix: {nanos_0:.2} ns/op, 63-prefix: {nanos_63:.2} ns/op, Ratio (63/0): {ratio:.4}"
    );

    // In non-constant-time equality (`==`), comparing 64 bytes with 63 matching prefix bytes
    // takes significantly longer than terminating on byte 0.
    // With `constant_time_eq`, both take practically identical time.
    // Under unoptimized debug builds, OS scheduling jitter is higher; in release builds it is strictly bounded.
    let tolerance_range = if cfg!(debug_assertions) {
        0.60..=1.60
    } else {
        0.75..=1.25
    };

    assert!(
        tolerance_range.contains(&ratio),
        "Timing side-channel detected: ratio={ratio:.4} is outside tolerance [{:.2}, {:.2}]",
        tolerance_range.start(),
        tolerance_range.end()
    );
}

// =========================================================================
// 2. Scheme Mutation Tests
// =========================================================================

#[test]
fn test_adversarial_scheme_mutations() {
    let secret = "your-adversarial-token-12345";

    // Valid mutations (should succeed)
    let valid_variations = [
        format!("Bearer {secret}"),
        format!("bearer {secret}"),
        format!("BEARER {secret}"),
        format!("BeArEr {secret}"),
        format!("bEaReR {secret}"),
        format!("  Bearer   {secret}  "),
        format!("  bearer   {secret}  "),
        format!("\tBearer {secret}\t"),
        format!("Bearer \t {secret} \t"),
    ];

    for val in &valid_variations {
        let headers = auth_header(val);
        assert!(
            check_bearer(&headers, secret),
            "Expected valid for header: '{val}'"
        );
    }

    // Invalid scheme mutations (should be rejected)
    let invalid_schemes = [
        format!("Basic {secret}"),
        format!("Token {secret}"),
        format!("Digest {secret}"),
        format!("OAuth {secret}"),
        format!("Mac {secret}"),
        format!("Bearer{secret}"),
        format!("Bearer1 {secret}"),
        format!("Bearer_{secret}"),
        format!("Bearer-Token {secret}"),
        format!("CustomBearer {secret}"),
        "Bearer".to_string(),
        "bearer".to_string(),
        "BEARER".to_string(),
        "Bearer ".to_string(),
        "Bearer      ".to_string(),
        "Bearer \t ".to_string(),
        "".to_string(),
        "   ".to_string(),
        format!("secret {secret}"),
        secret.to_string(),
    ];

    for val in &invalid_schemes {
        let headers = auth_header(val);
        assert!(
            !check_bearer(&headers, secret),
            "Expected rejection for header: '{val}'"
        );
    }
}

#[test]
fn test_adversarial_huge_64kb_token() {
    let secret = "your-adversarial-token";
    // Construct a 64KB token payload
    let huge_token = "X".repeat(65536);
    let huge_header = format!("Bearer {huge_token}");

    let headers = auth_header(&huge_header);
    // Must be rejected safely without crash, panic, or stack overflow
    assert!(!check_bearer(&headers, secret));
}

#[test]
fn test_adversarial_non_ascii_unicode_tokens() {
    let secret = "your-valid-token";

    // 1. Non-ASCII UTF-8 bytes in header scheme (e.g. Béarer with é = 0xC3 0xA9)
    let scheme_bytes = b"B\xc3\xa9arer your-valid-token";
    let headers = auth_header_bytes(scheme_bytes);
    // header_val.to_str() fails on non-ASCII bytes, safely returning false
    assert!(!check_bearer(&headers, secret));

    // 2. Non-ASCII UTF-8 bytes in token (e.g. Bearer 🦀 = 0xF0 0x9F 0xA6 0x80)
    let token_bytes = b"Bearer \xf0\x9f\xa6\x80";
    let headers = auth_header_bytes(token_bytes);
    assert!(!check_bearer(&headers, secret));

    // 3. Raw arbitrary non-UTF8 high bytes
    let corrupt_bytes = b"Bearer \xff\xfe\xfd\x80";
    let headers = auth_header_bytes(corrupt_bytes);
    assert!(!check_bearer(&headers, secret));
}

// =========================================================================
// 3. Port Parsing Stress Tests
// =========================================================================

#[test]
fn test_adversarial_port_parsing_stress() {
    // Valid boundary ports
    assert_eq!(parse_port_from_str(Some("0")).unwrap(), 0);
    assert_eq!(parse_port_from_str(Some("1")).unwrap(), 1);
    assert_eq!(parse_port_from_str(Some("80")).unwrap(), 80);
    assert_eq!(parse_port_from_str(Some("8080")).unwrap(), 8080);
    assert_eq!(parse_port_from_str(Some("65535")).unwrap(), 65535);

    // Default fallback ports (None, empty, whitespace)
    assert_eq!(parse_port_from_str(None).unwrap(), 8080);
    assert_eq!(parse_port_from_str(Some("")).unwrap(), 8080);
    assert_eq!(parse_port_from_str(Some("   ")).unwrap(), 8080);
    assert_eq!(parse_port_from_str(Some("\t\r\n ")).unwrap(), 8080);

    // Whitespace trimming
    assert_eq!(parse_port_from_str(Some(" 8080 ")).unwrap(), 8080);
    assert_eq!(parse_port_from_str(Some("  65535  ")).unwrap(), 65535);
    assert_eq!(parse_port_from_str(Some(" 0 ")).unwrap(), 0);
    assert_eq!(parse_port_from_str(Some("008080")).unwrap(), 8080);

    // Out-of-range ports (overflow u16)
    assert!(parse_port_from_str(Some("65536")).is_err());
    assert!(parse_port_from_str(Some("65537")).is_err());
    assert!(parse_port_from_str(Some("100000")).is_err());
    assert!(parse_port_from_str(Some("9999999999999999999999999999")).is_err());

    // Negative numbers
    assert!(parse_port_from_str(Some("-1")).is_err());
    assert!(parse_port_from_str(Some("-8080")).is_err());

    // Non-numeric strings
    assert!(parse_port_from_str(Some("abc")).is_err());
    assert!(parse_port_from_str(Some("8080abc")).is_err());
    assert!(parse_port_from_str(Some("abc8080")).is_err());
    assert!(parse_port_from_str(Some("8080\0")).is_err());
    assert!(parse_port_from_str(Some("3.14")).is_err());
    assert!(parse_port_from_str(Some("0x1F90")).is_err());

    // Fullwidth Unicode / Arabic numerals
    assert!(parse_port_from_str(Some("８０８０")).is_err());
    assert!(parse_port_from_str(Some("🚀")).is_err());
    assert!(parse_port_from_str(Some("١٢٣٤")).is_err());
}

// =========================================================================
// 4. Fail-Closed Behavior on Empty/Blank Secret
// =========================================================================

#[test]
fn test_adversarial_fail_closed_on_empty_secret() {
    let empty_secret = "";
    let blank_secret = "   ";

    let state_blank = AuthState::new(blank_secret);
    // Blank secret trimmed to empty string
    assert_eq!(&*state_blank.expected_token, "");

    // Test with various authorization headers against empty secret
    let tests = [
        auth_header("Bearer "),
        auth_header("Bearer test"),
        auth_header("Bearer secret"),
        auth_header("Bearer   "),
        auth_header(""),
    ];

    for h in &tests {
        assert!(
            !check_bearer(h, empty_secret),
            "Must fail closed on empty expected secret"
        );
        assert!(
            !check_bearer(h, &state_blank.expected_token),
            "Must fail closed on blank expected secret"
        );
    }
}

// =========================================================================
// 5. High-Concurrency Async Load Stress
// =========================================================================

#[tokio::test]
async fn test_adversarial_high_concurrency_stress() {
    let token = "your-stress-test-token-9876543210";
    let server = TestServer::spawn_with_token(token).await;
    let client = Client::new();

    const CONCURRENT_REQUESTS_PER_TYPE: usize = 50;
    let mut handles = Vec::new();

    // 1. Batch of unauthenticated GET /health requests (must return 200 OK)
    for _ in 0..CONCURRENT_REQUESTS_PER_TYPE {
        let client = client.clone();
        let url = format!("{}/health", server.base_url);
        handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let res = client.get(&url).send().await.expect("health send");
            let latency = start.elapsed();
            assert_eq!(res.status(), StatusCode::OK);
            let body: serde_json::Value = res.json().await.expect("health json");
            assert_eq!(body, serde_json::json!({ "ok": true }));
            ("health", latency)
        }));
    }

    // 2. Batch of unauthenticated protected GET /openapi.json requests (must return 401 Unauthorized)
    for _ in 0..CONCURRENT_REQUESTS_PER_TYPE {
        let client = client.clone();
        let url = format!("{}/openapi.json", server.base_url);
        handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let res = client.get(&url).send().await.expect("unauthed send");
            let latency = start.elapsed();
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
            ("unauthed", latency)
        }));
    }

    // 3. Batch of invalid Bearer token POST /mcp requests (must return 401 Unauthorized)
    for _ in 0..CONCURRENT_REQUESTS_PER_TYPE {
        let client = client.clone();
        let url = format!("{}/mcp", server.base_url);
        handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let res = client
                .post(&url)
                .header(header::AUTHORIZATION, "Bearer wrong-token-candidate")
                .send()
                .await
                .expect("wrong token send");
            let latency = start.elapsed();
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
            ("wrong_token", latency)
        }));
    }

    // 4. Batch of valid Bearer token POST /mcp requests (must return 200 OK)
    for _ in 0..CONCURRENT_REQUESTS_PER_TYPE {
        let client = client.clone();
        let url = format!("{}/mcp", server.base_url);
        let auth = format!("Bearer {token}");
        handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let res = client
                .post(&url)
                .header(header::AUTHORIZATION, auth)
                .send()
                .await
                .expect("valid token send");
            let latency = start.elapsed();
            assert_eq!(res.status(), StatusCode::OK);
            ("valid_token", latency)
        }));
    }

    // Await all 200 concurrent tasks
    let results = futures::future::join_all(handles).await;
    let mut health_latencies = Vec::new();

    for res in results {
        let (req_type, latency) = res.expect("task join failed");
        if req_type == "health" {
            health_latencies.push(latency);
        }
    }

    assert_eq!(health_latencies.len(), CONCURRENT_REQUESTS_PER_TYPE);
    let avg_health_latency = health_latencies.iter().sum::<std::time::Duration>()
        / health_latencies.len() as u32;

    println!(
        "[Concurrency Stress] Completed 200 parallel requests without errors. Average /health latency: {:?}",
        avg_health_latency
    );

    // Verify /health latency remains under 200ms even under concurrent storm
    assert!(
        avg_health_latency < std::time::Duration::from_millis(200),
        "Average health latency exceeded 200ms: {:?}",
        avg_health_latency
    );
}

// =========================================================================
// 6. RFC 6750 HTTP 401 Error Response Contract Compliance
// =========================================================================

#[tokio::test]
async fn test_adversarial_http_401_rfc6750_contract() {
    let server = TestServer::spawn().await;
    let client = Client::new();

    let invalid_auth_headers = [
        None,
        Some("Basic user:pass"),
        Some("Token abc"),
        Some("Bearer"),
        Some("Bearer "),
        Some("Bearer wrong-token"),
        Some("Bearer   "),
    ];

    let target_paths = [
        ("GET", format!("{}/openapi.json", server.base_url)),
        ("POST", format!("{}/mcp", server.base_url)),
        ("GET", format!("{}/sse", server.base_url)),
        ("POST", format!("{}/messages", server.base_url)),
        ("GET", format!("{}/api/x/post", server.base_url)),
        ("POST", format!("{}/api/web/markdown", server.base_url)),
    ];

    for (method, url) in &target_paths {
        for auth_hdr in &invalid_auth_headers {
            let mut req = match *method {
                "POST" => client.post(url),
                _ => client.get(url),
            };

            if let Some(val) = auth_hdr {
                req = req.header(header::AUTHORIZATION, *val);
            }

            let res = req.send().await.expect("send 401 test request");

            // 1. Status code must be 401
            assert_eq!(
                res.status(),
                StatusCode::UNAUTHORIZED,
                "URL {url} with auth {auth_hdr:?} did not return 401"
            );

            // 2. WWW-Authenticate must be Bearer (RFC 6750 Section 3)
            let www_auth = res
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .expect("WWW-Authenticate header must be present");
            assert_eq!(
                www_auth.to_str().unwrap(),
                "Bearer",
                "WWW-Authenticate header must be 'Bearer'"
            );

            // 3. Content-Type must be application/json
            let content_type = res
                .headers()
                .get(header::CONTENT_TYPE)
                .expect("Content-Type header must be present");
            assert!(
                content_type
                    .to_str()
                    .unwrap()
                    .contains("application/json"),
                "Content-Type header must be application/json"
            );

            // 4. JSON body contract
            let body: serde_json::Value = res.json().await.expect("parse 401 JSON");
            assert_eq!(
                body,
                serde_json::json!({
                    "ok": false,
                    "code": "UNAUTHORIZED",
                    "message": "Invalid or missing bearer token"
                }),
                "401 JSON response body mismatch"
            );
        }
    }
}

// =========================================================================
// 7. Public Health Path and Method Boundaries
// =========================================================================

#[tokio::test]
async fn test_adversarial_health_path_and_method_boundaries() {
    let server = TestServer::spawn().await;
    let client = Client::new();

    // 1. GET /health?query=123&test=true -> should return 200 OK
    let res = client
        .get(format!("{}/health?query=123&test=true", server.base_url))
        .send()
        .await
        .expect("health query send");
    assert_eq!(res.status(), StatusCode::OK);
    let json: serde_json::Value = res.json().await.unwrap();
    assert_eq!(json, serde_json::json!({ "ok": true }));

    // 2. POST /health without auth -> should be rejected (405 Method Not Allowed or 404/401)
    let res = client
        .post(format!("{}/health", server.base_url))
        .send()
        .await
        .expect("post health send");
    // Axum routes GET /health; POST is not registered on public router, so it falls through or returns 405
    assert_ne!(res.status(), StatusCode::OK);
}
