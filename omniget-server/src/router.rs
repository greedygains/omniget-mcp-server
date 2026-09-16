//! Axum router assembly, route definitions, and CORS configuration.

use axum::{
    http::Method,
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

/// Shared application state storing the expected auth token.
#[derive(Clone, Debug)]
pub struct AppState {
    pub auth_token: Arc<String>,
}

impl AppState {
    /// Create a new `AppState` with the given token string.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            auth_token: Arc::new(token.into()),
        }
    }

    /// Load the auth token from `AUTH_TOKEN` environment variable.
    pub fn from_env() -> Self {
        let token = std::env::var("AUTH_TOKEN").unwrap_or_default();
        Self::new(token)
    }
}

/// Assembles the complete application router with public bypass and protected routes.
pub fn build_router(state: AppState) -> Router {
    let auth_state = crate::auth::AuthState::new(&*state.auth_token);

    // Protected routes requiring timing-safe Bearer token authentication
    let protected = Router::new()
        .route("/mcp", post(crate::mcp::mcp_post_handler))
        .route("/sse", get(crate::sse::sse_get_handler))
        .route("/messages", post(crate::sse::messages_post_handler))
        .route("/openapi.json", get(crate::rest::openapi_json_handler))
        .route(
            "/api/x/post",
            get(crate::rest::x_post_get_handler).post(crate::rest::x_post_post_handler),
        )
        .route(
            "/api/x/thread",
            get(crate::rest::x_thread_get_handler).post(crate::rest::x_thread_post_handler),
        )
        .route(
            "/api/web/markdown",
            get(crate::rest::web_markdown_get_handler).post(crate::rest::web_markdown_post_handler),
        )
        .route(
            "/api/pdf/text",
            get(crate::rest::pdf_text_get_handler).post(crate::rest::pdf_text_post_handler),
        )
        .route(
            "/api/media/info",
            get(crate::rest::media_info_get_handler).post(crate::rest::media_info_post_handler),
        )
        .route(
            "/api/instagram/post",
            get(crate::rest::instagram_post_get_handler).post(crate::rest::instagram_post_post_handler),
        )
        .route(
            "/api/facebook/post",
            get(crate::rest::facebook_post_get_handler).post(crate::rest::facebook_post_post_handler),
        )
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            crate::auth::auth_middleware,
        ));

    // Public routes (no auth required)
    let public = Router::new().route("/health", get(crate::auth::health_handler));

    // Permissive CORS layer for cross-origin web/browser clients
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any);

    public.merge(protected).layer(cors).with_state(state)
}
