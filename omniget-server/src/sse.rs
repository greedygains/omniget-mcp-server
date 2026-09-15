//! MCP Server-Sent Events (SSE) transport implementation.
//!
//! Provides the `GET /sse` endpoint for establishing persistent event streams
//! and the `POST /messages?sessionId=<uuid>` endpoint for receiving JSON-RPC 2.0
//! requests and routing responses back down the active SSE stream.

use axum::{
    body::to_bytes,
    extract::Request,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
};
use serde_json::Value;
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

/// Maximum payload body size for POST /messages (10 MB).
const MAX_MESSAGE_BODY_BYTES: usize = 10 * 1024 * 1024;

/// Channel capacity for buffered events per SSE session.
const SESSION_CHANNEL_CAPACITY: usize = 256;

/// Thread-safe registry mapping active UUID session IDs to SSE event senders.
#[derive(Clone, Default)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, mpsc::Sender<Event>>>>,
}

impl SessionStore {
    /// Creates a new, empty session store.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Registers an active session with its channel sender.
    pub async fn register(&self, session_id: String, tx: mpsc::Sender<Event>) {
        let mut lock = self.sessions.write().await;
        lock.insert(session_id, tx);
    }

    /// Retrieves the channel sender for an active session.
    pub async fn get(&self, session_id: &str) -> Option<mpsc::Sender<Event>> {
        let lock = self.sessions.read().await;
        lock.get(session_id).cloned()
    }

    /// Removes a session from the registry upon disconnection.
    pub async fn remove(&self, session_id: &str) -> Option<mpsc::Sender<Event>> {
        let mut lock = self.sessions.write().await;
        lock.remove(session_id)
    }

    /// Returns the number of currently active sessions.
    #[allow(dead_code)]
    pub async fn count(&self) -> usize {
        let lock = self.sessions.read().await;
        lock.len()
    }
}

/// Global singleton session store for the process.
static GLOBAL_SESSION_STORE: std::sync::OnceLock<SessionStore> = std::sync::OnceLock::new();

/// Returns a reference to the global session store.
pub fn global_session_store() -> &'static SessionStore {
    GLOBAL_SESSION_STORE.get_or_init(SessionStore::new)
}

/// RAII guard that automatically removes a session from the store when the SSE stream drops.
struct SessionGuard {
    session_id: String,
    store: SessionStore,
}

impl SessionGuard {
    fn new(session_id: String, store: SessionStore) -> Self {
        Self { session_id, store }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let session_id = self.session_id.clone();
        let store = self.store.clone();
        tokio::spawn(async move {
            store.remove(&session_id).await;
            tracing::debug!("Cleaned up SSE session: {session_id}");
        });
    }
}

/// Handler for `GET /sse`: Establishes the SSE stream and emits the initial endpoint event.
pub async fn sse_get_handler() -> impl IntoResponse {
    let session_id = Uuid::new_v4().to_string();
    let store = global_session_store().clone();
    let (tx, mut rx) = mpsc::channel::<Event>(SESSION_CHANNEL_CAPACITY);

    store.register(session_id.clone(), tx).await;
    tracing::info!("Established new MCP SSE session: {session_id}");

    let endpoint_path = format!("/messages?sessionId={session_id}");
    let initial_event = Event::default()
        .event("endpoint")
        .data(endpoint_path);

    let guard = SessionGuard::new(session_id.clone(), store);

    let stream = async_stream::stream! {
        // Keep RAII guard in scope for the stream's lifetime
        let _guard = guard;

        // 1. Initial endpoint event
        yield Ok::<_, Infallible>(initial_event);

        // 2. Stream incoming message events from POST /messages
        while let Some(event) = rx.recv().await {
            yield Ok(event);
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::default()
            .interval(Duration::from_secs(15)),
    )
}

/// Handler for `POST /messages?sessionId=<uuid>`: Validates input, accepts immediately with 202,
/// and asynchronously executes JSON-RPC 2.0 payloads, routing responses over the SSE stream.
pub async fn messages_post_handler(req: Request) -> Response {
    let (parts, body) = req.into_parts();

    // 1. Read body bytes (limited to MAX_MESSAGE_BODY_BYTES)
    let body_bytes = match to_bytes(body, MAX_MESSAGE_BODY_BYTES).await {
        Ok(b) => b,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    // 2. Handle empty body (for auth/liveness verification without payload)
    if body_bytes.is_empty() {
        return StatusCode::ACCEPTED.into_response();
    }

    // 3. Parse query string and extract sessionId
    let query_str = parts.uri.query().unwrap_or("");
    let query_map: HashMap<String, String> = url::form_urlencoded::parse(query_str.as_bytes())
        .into_owned()
        .collect();

    let session_id = match query_map.get("sessionId") {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    // 4. Validate UUID format
    if Uuid::parse_str(&session_id).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    // 5. Verify session exists in store
    let store = global_session_store();
    let session_tx = match store.get(&session_id).await {
        Some(tx) => tx,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    // 6. Parse JSON-RPC 2.0 payload
    let payload: Value = match serde_json::from_slice(&body_bytes) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    // 6. Asynchronously execute JSON-RPC request and route response over SSE channel
    tokio::spawn(async move {
        let responses: Vec<Value> = if payload.is_array() {
            let arr = payload.as_array().unwrap();
            if arr.is_empty() {
                vec![crate::mcp::make_error_response(
                    Value::Null,
                    crate::mcp::INVALID_REQUEST,
                    "Invalid Request: empty batch array",
                )]
            } else {
                let mut res_list = Vec::new();
                for item in arr {
                    if let Some(res) = crate::mcp::dispatch_json_rpc(item.clone()).await {
                        res_list.push(res);
                    }
                }
                res_list
            }
        } else {
            match crate::mcp::dispatch_json_rpc(payload).await {
                Some(res) => vec![res],
                None => Vec::new(),
            }
        };

        for res in responses {
            let json_str = match serde_json::to_string(&res) {
                Ok(s) => s,
                Err(e) => {
                    let err_res = crate::mcp::make_error_response(
                        Value::Null,
                        crate::mcp::INTERNAL_ERROR,
                        format!("Internal serialization error: {e}"),
                    );
                    serde_json::to_string(&err_res).unwrap_or_default()
                }
            };

            let event = Event::default().event("message").data(json_str);
            if let Err(e) = session_tx.send(event).await {
                tracing::warn!("Failed to route response to SSE stream (client disconnected): {e}");
                break;
            }
        }
    });

    // 7. Return immediate HTTP 202 Accepted with empty body
    StatusCode::ACCEPTED.into_response()
}

