//! Server runtime utilities: port resolution, signal handling, and listener binding.

use std::net::SocketAddr;
use tokio::net::TcpListener;

/// Parse port string into a valid port number. Defaults to 8080 if input is None or empty.
pub fn parse_port_from_str(port_str: Option<&str>) -> Result<u16, anyhow::Error> {
    match port_str {
        Some(val) => {
            let trimmed = val.trim();
            if trimmed.is_empty() {
                Ok(8080)
            } else {
                trimmed.parse::<u16>().map_err(|e| {
                    anyhow::anyhow!(
                        "Invalid PORT '{trimmed}': must be an integer between 0 and 65535: {e}"
                    )
                })
            }
        }
        None => Ok(8080),
    }
}

/// Resolve port from the `PORT` environment variable, defaulting to 8080.
pub fn resolve_port() -> Result<u16, anyhow::Error> {
    match std::env::var("PORT") {
        Ok(val) => parse_port_from_str(Some(&val)),
        Err(std::env::VarError::NotPresent) => Ok(8080),
        Err(e) => Err(anyhow::anyhow!("Failed to read PORT environment variable: {e}")),
    }
}

/// Bind a TCP listener to the specified address.
pub async fn bind_listener(addr: SocketAddr) -> Result<TcpListener, std::io::Error> {
    TcpListener::bind(addr).await
}

/// Asynchronously listens for shutdown signals: SIGINT (Ctrl+C) and SIGTERM (container stop).
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!("Failed to install Ctrl+C signal handler: {err}");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::error!("Failed to install SIGTERM signal handler: {err}");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received SIGINT (Ctrl+C), initiating graceful shutdown");
        },
        _ = terminate => {
            tracing::info!("Received SIGTERM, initiating graceful shutdown");
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_port_defaults() {
        assert_eq!(parse_port_from_str(None).unwrap(), 8080);
        assert_eq!(parse_port_from_str(Some("")).unwrap(), 8080);
        assert_eq!(parse_port_from_str(Some("   ")).unwrap(), 8080);
    }

    #[test]
    fn test_parse_port_custom() {
        assert_eq!(parse_port_from_str(Some("3000")).unwrap(), 3000);
        assert_eq!(parse_port_from_str(Some("80")).unwrap(), 80);
        assert_eq!(parse_port_from_str(Some("0")).unwrap(), 0);
    }

    #[test]
    fn test_parse_port_invalid() {
        assert!(parse_port_from_str(Some("abc")).is_err());
        assert!(parse_port_from_str(Some("-1")).is_err());
        assert!(parse_port_from_str(Some("70000")).is_err());
    }
}
