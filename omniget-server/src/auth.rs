//! Constant-time Bearer token authentication guardrails and public health endpoint.

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use constant_time_eq::constant_time_eq;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Structured JSON error response for HTTP 401 Unauthorized.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthErrorResponse {
    pub ok: bool,
    pub code: String,
    pub message: String,
}

impl Default for AuthErrorResponse {
    fn default() -> Self {
        Self {
            ok: false,
            code: "UNAUTHORIZED".to_string(),
            message: "Invalid or missing bearer token".to_string(),
        }
    }
}

/// Public health check response payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthResponse {
    pub ok: bool,
}

/// Shared authentication state storing the configured Bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthState {
    pub expected_token: Arc<str>,
}

impl AuthState {
    /// Creates a new `AuthState` with the provided token, trimming surrounding whitespace.
    pub fn new(token: impl AsRef<str>) -> Self {
        Self {
            expected_token: Arc::from(token.as_ref().trim()),
        }
    }

    /// Loads the auth token from the `AUTH_TOKEN` environment variable.
    pub fn from_env() -> Self {
        let token = std::env::var("AUTH_TOKEN").unwrap_or_default();
        Self::new(token)
    }
}

/// Returns a structured HTTP 401 Unauthorized response with RFC 6750 Bearer challenge.
pub fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::WWW_AUTHENTICATE, "Bearer"),
        ],
        Json(AuthErrorResponse::default()),
    )
        .into_response()
}

/// Public health check handler returning HTTP 200 OK with `{ "ok": true }`.
pub async fn health_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        Json(HealthResponse { ok: true }),
    )
}

/// Validates the Authorization header against the expected token using constant-time byte comparison.
///
/// Rules:
/// 1. Fails closed if expected token is empty.
/// 2. Extracts `Authorization` header; fails if missing or non-UTF8.
/// 3. Validates scheme prefix (`Bearer ` or `bearer ` case-insensitively).
/// 4. Trims leading/trailing whitespace from provided token.
/// 5. Executes timing-safe comparison via `constant_time_eq::constant_time_eq`.
pub fn check_bearer(headers: &HeaderMap, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }

    let header_val = match headers.get(header::AUTHORIZATION) {
        Some(v) => v,
        None => return false,
    };

    let raw = match header_val.to_str() {
        Ok(s) => s.trim(),
        Err(_) => return false,
    };

    let provided = if raw.len() >= 7 && raw[..7].eq_ignore_ascii_case("bearer ") {
        raw[7..].trim()
    } else {
        return false;
    };

    if provided.is_empty() {
        return false;
    }

    constant_time_eq(provided.as_bytes(), expected.as_bytes())
}

/// Validates if the request is authenticated via Bearer header or URL query parameter (?token=, ?auth=, ?api_key=).
pub fn is_authenticated(request: &Request, expected: &str) -> bool {
    if check_bearer(request.headers(), expected) {
        return true;
    }

    if let Some(query) = request.uri().query() {
        for (key, val) in url::form_urlencoded::parse(query.as_bytes()) {
            if (key == "token" || key == "auth" || key == "api_key")
                && !expected.is_empty()
                && constant_time_eq(val.trim().as_bytes(), expected.as_bytes())
            {
                return true;
            }
        }
    }

    false
}

/// Axum middleware enforcing Bearer token authentication on protected endpoints.
/// Unconditionally allows `GET /health`.
pub async fn auth_middleware(
    State(state): State<AuthState>,
    request: Request,
    next: Next,
) -> Response {
    // Defense-in-depth fast path for GET /health
    if request.method() == Method::GET && request.uri().path() == "/health" {
        return next.run(request).await;
    }

    if !is_authenticated(&request, &state.expected_token) {
        return unauthorized_response();
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{HeaderValue, Request},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    fn header_with(val: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(header::AUTHORIZATION, HeaderValue::from_str(val).unwrap());
        map
    }

    #[test]
    fn check_bearer_accepts_valid_capital_bearer() {
        let headers = header_with("Bearer secret-token-42");
        assert!(check_bearer(&headers, "secret-token-42"));
    }

    #[test]
    fn check_bearer_accepts_valid_lowercase_bearer() {
        let headers = header_with("bearer your-secure-token-42");
        assert!(check_bearer(&headers, "your-secure-token-42"));
    }

    #[test]
    fn check_bearer_accepts_mixed_case_bearer() {
        let headers = header_with("BeArEr your-secure-token-42");
        assert!(check_bearer(&headers, "your-secure-token-42"));
    }

    #[test]
    fn check_bearer_accepts_whitespace_padding() {
        let headers = header_with("  Bearer   your-secure-token-42   ");
        assert!(check_bearer(&headers, "your-secure-token-42"));
    }

    #[test]
    fn check_bearer_rejects_missing_header() {
        let headers = HeaderMap::new();
        assert!(!check_bearer(&headers, "your-secure-token-42"));
    }

    #[test]
    fn check_bearer_rejects_wrong_token() {
        let headers = header_with("Bearer wrong-token");
        assert!(!check_bearer(&headers, "your-secure-token-42"));
    }

    #[test]
    fn check_bearer_rejects_timing_safe_length_mismatch() {
        let token = "your-super-token-42";
        assert!(!check_bearer(&header_with("Bearer your-super"), token));
        assert!(!check_bearer(&header_with("Bearer your-super-token-42-extra"), token));
        assert!(!check_bearer(&header_with("Bearer your-super-token-43"), token));
    }

    #[test]
    fn check_bearer_rejects_invalid_scheme() {
        assert!(!check_bearer(&header_with("Basic dXNlcjpwYXNz"), "your-secure-token"));
        assert!(!check_bearer(&header_with("Token your-secure-token"), "your-secure-token"));
        assert!(!check_bearer(&header_with("your-secure-token"), "your-secure-token"));
        assert!(!check_bearer(&header_with("Beareryour-secure-token"), "your-secure-token"));
        assert!(!check_bearer(&header_with("Bearer "), "your-secure-token"));
    }

    #[test]
    fn check_bearer_fails_closed_on_empty_server_token() {
        assert!(!check_bearer(&header_with("Bearer "), ""));
        assert!(!check_bearer(&header_with("Bearer any-token"), ""));
    }

    #[tokio::test]
    async fn test_unauthorized_response_payload() {
        let res = unauthorized_response();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(res.headers().get(header::CONTENT_TYPE).unwrap(), "application/json");
        assert_eq!(res.headers().get(header::WWW_AUTHENTICATE).unwrap(), "Bearer");

        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let payload: AuthErrorResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload, AuthErrorResponse {
            ok: false,
            code: "UNAUTHORIZED".to_string(),
            message: "Invalid or missing bearer token".to_string(),
        });
    }

    #[tokio::test]
    async fn test_middleware_gating_and_health_bypass() {
        let auth_state = AuthState::new("your-secure-token-42");
        let app = Router::new()
            .route("/health", get(health_handler))
            .route("/protected", get(|| async { "sensitive data" }))
            .layer(axum::middleware::from_fn_with_state(
                auth_state,
                auth_middleware,
            ));

        // 1. GET /health with no headers -> 200 OK, {"ok": true}
        let req = Request::builder().uri("/health").body(Body::empty()).unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let health: HealthResponse = serde_json::from_slice(&body).unwrap();
        assert!(health.ok);

        // 2. Protected endpoint without auth -> 401
        let req = Request::builder().uri("/protected").body(Body::empty()).unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // 3. Protected endpoint with invalid auth -> 401
        let req = Request::builder()
            .uri("/protected")
            .header(header::AUTHORIZATION, "Bearer wrong")
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // 4. Protected endpoint with valid auth -> 200
        let req = Request::builder()
            .uri("/protected")
            .header(header::AUTHORIZATION, "Bearer your-secure-token-42")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }
}
