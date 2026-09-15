//! OmniGet Standalone Headless MCP Server library.

pub mod auth;
pub mod mcp;
pub mod rest;
pub mod router;
pub mod server;
pub mod sse;
pub mod tools;

pub use auth::{AuthState, HealthResponse};
pub use router::{build_router, AppState};
