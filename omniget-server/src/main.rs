//! OmniGet Standalone Headless MCP Server daemon entrypoint.

use omniget_server::{build_router, AppState};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Structured logging setup
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "omniget_server=info,tower_http=info".into()),
        )
        .init();

    tracing::info!(
        "Starting OmniGet Standalone MCP Server v{}",
        env!("CARGO_PKG_VERSION")
    );

    // 2. Load and validate AUTH_TOKEN
    let auth_token = match std::env::var("AUTH_TOKEN") {
        Ok(token) if !token.trim().is_empty() => token.trim().to_string(),
        _ => {
            tracing::error!("FATAL: AUTH_TOKEN environment variable must be set and non-empty");
            eprintln!("Error: AUTH_TOKEN environment variable must be set and non-empty");
            std::process::exit(1);
        }
    };

    // 3. Dynamic port binding to 0.0.0.0:${PORT:-8080}
    let port = omniget_server::server::resolve_port()?;
    let bind_addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = omniget_server::server::bind_listener(bind_addr)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to bind TCP listener to {bind_addr}: {e}"))?;

    let local_addr = listener.local_addr()?;
    tracing::info!("Server listening on http://{local_addr} (binding 0.0.0.0:{port})");

    // 4. Assemble root router
    let state = AppState {
        auth_token: Arc::new(auth_token),
    };
    let app = build_router(state);

    // 5. Run Axum server with graceful shutdown signal
    tracing::info!("Ready to accept connections. Installing SIGINT/SIGTERM shutdown handlers...");
    axum::serve(listener, app)
        .with_graceful_shutdown(omniget_server::server::shutdown_signal())
        .await
        .map_err(|e| anyhow::anyhow!("Server runtime error: {e}"))?;

    tracing::info!("OmniGet Server terminated gracefully.");
    Ok(())
}
