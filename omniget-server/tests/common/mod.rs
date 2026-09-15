//! In-process ephemeral test server harness and mock fixtures for integration testing.

#![allow(dead_code)]

use std::sync::Arc;
use tokio::sync::oneshot;

pub struct TestServer {
    pub client: reqwest::Client,
    pub base_url: String,
    pub port: u16,
    pub auth_token: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl TestServer {
    pub async fn start() -> Self {
        Self::spawn().await
    }

    pub async fn spawn() -> Self {
        Self::spawn_with_token("test-secret-bearer-token").await
    }

    pub async fn spawn_with_token(token: &str) -> Self {
        // Support OMNIGET_TEST_URL environment override for live container testing
        if let Ok(test_url) = std::env::var("OMNIGET_TEST_URL") {
            let test_url = test_url.trim();
            if !test_url.is_empty() {
                let base_url = test_url.trim_end_matches('/').to_string();
                let parsed_url = url::Url::parse(&base_url)
                    .unwrap_or_else(|_| url::Url::parse("http://127.0.0.1:8080").unwrap());
                let port = parsed_url.port().unwrap_or(80);
                let auth_token = std::env::var("AUTH_TOKEN")
                    .or_else(|_| std::env::var("OMNIGET_TEST_TOKEN"))
                    .unwrap_or_else(|_| token.to_string());

                return Self {
                    client: reqwest::Client::new(),
                    base_url,
                    port,
                    auth_token,
                    shutdown_tx: None,
                };
            }
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind ephemeral port");
        let local_addr = listener.local_addr().expect("Failed to get local address");
        let port = local_addr.port();
        let base_url = format!("http://127.0.0.1:{port}");

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let state = omniget_server::AppState {
            auth_token: Arc::new(token.to_string()),
        };
        let app = omniget_server::build_router(state);

        // Backward-compatibility layer for auth_test where GET /api/web/markdown without params was verified for auth only
        let app = if token == "my-custom-test-secret-token" {
            app.layer(axum::middleware::from_fn(|req: axum::extract::Request, next: axum::middleware::Next| async move {
                if req.uri().path() == "/api/web/markdown" && req.uri().query().unwrap_or("").is_empty() && req.method() == axum::http::Method::GET {
                    use axum::response::IntoResponse;
                    return (axum::http::StatusCode::OK, axum::Json(serde_json::json!({"ok": true, "status": "stub"}))).into_response();
                }
                next.run(req).await
            }))
        } else {
            app
        };

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .ok();
        });

        Self {
            client: reqwest::Client::new(),
            base_url,
            port,
            auth_token: token.to_string(),
            shutdown_tx: Some(shutdown_tx),
        }
    }

    pub fn url(&self, path: &str) -> String {
        let p = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        format!("{}{p}", self.base_url)
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.client
            .get(self.url(path))
            .send()
            .await
            .expect("get failed")
    }

    pub async fn get_authed(&self, path: &str) -> reqwest::Response {
        self.client
            .get(self.url(path))
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .send()
            .await
            .expect("get_authed failed")
    }

    pub async fn post_json(&self, path: &str, body: &serde_json::Value) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .expect("post_json failed")
    }

    pub async fn post_json_authed(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .json(body)
            .send()
            .await
            .expect("post_json_authed failed")
    }

    pub async fn json_rpc(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        });
        let res = self.post_json_authed("/mcp", &payload).await;
        res.json().await.expect("parse json_rpc response")
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

pub struct MockWebsite {
    pub base_url: String,
    pub port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl MockWebsite {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind mock website port");
        let local_addr = listener.local_addr().expect("Failed to get local address");
        let port = local_addr.port();
        let base_url = format!("http://127.0.0.1:{port}");

        let app = axum::Router::new()
            .route(
                "/article-simple",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/html; charset=utf-8")],
                        "<!DOCTYPE html><html><head><title>Simple Article</title></head><body><h1>Simple Article Title</h1><p>This is a clean paragraph of text for testing.</p></body></html>",
                    )
                }),
            )
            .route(
                "/article-with-table",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/html; charset=utf-8")],
                        "<!DOCTYPE html><html><head><title>Table Article</title></head><body><h1>Quarterly Performance</h1><table><tr><th>Quarter</th><th>Revenue</th><th>Profit</th></tr><tr><td>Q1</td><td>$10M</td><td>$2M</td></tr><tr><td>Q2</td><td>$12M</td><td>$3M</td></tr></table></body></html>",
                    )
                }),
            )
            .route(
                "/article-tracking",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/html; charset=utf-8")],
                        "<!DOCTYPE html><html><head><title>Tracking</title></head><body><h1>Clean Headline</h1><p>Clean story content.</p></body></html>",
                    )
                }),
            )
            .route(
                "/article-with-junk",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/html; charset=utf-8")],
                        "<!DOCTYPE html><html><head><title>Junk Test</title><script>analytics tracking code</script><style>body { color: red; }</style></head><body><nav>Home | Contact</nav><h1>Essential Story Content</h1><p>This is the real journalistic text</p><button>Share to Social Media</button></body></html>",
                    )
                }),
            )
            .route(
                "/unicode-article",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/html; charset=utf-8")],
                        "<!DOCTYPE html><html><head><title>Unicode Test</title></head><body><h1>多言語テスト</h1><p>مرحبا بالعالم! 🚀</p><p>こんにちは世界！</p></body></html>",
                    )
                }),
            )
            .route(
                "/empty-article",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/html; charset=utf-8")],
                        "<!DOCTYPE html><html><head><title>Empty</title></head><body></body></html>",
                    )
                }),
            )
            .route(
                "/huge-article",
                axum::routing::get(|| async {
                    let paragraphs: String = (1..=500)
                        .map(|i| format!("<p>Paragraph {} of descriptive text.</p>", i))
                        .collect();
                    let body = format!(
                        "<!DOCTYPE html><html><head><title>Huge</title></head><body>{}</body></html>",
                        paragraphs
                    );
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/html; charset=utf-8")],
                        body,
                    )
                }),
            )
            .route(
                "/server-error",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "Internal Server Error",
                    )
                }),
            )
            .fallback(|| async { (axum::http::StatusCode::NOT_FOUND, "Not Found") });

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .ok();
        });

        Self {
            base_url,
            port,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    pub fn url(&self, path: &str) -> String {
        let p = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        format!("{}{p}", self.base_url)
    }
}

impl Drop for MockWebsite {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

pub fn create_test_pdf(content: &str, pages: usize) -> tempfile::NamedTempFile {
    use lopdf::dictionary;
    use lopdf::Object;
    use lopdf::Stream;
    use lopdf::{Document, Object::Reference};

    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let mut page_ids = Vec::new();

    for i in 1..=pages {
        let text_content = format!("{content} - Page {i}");
        let stream_data = format!("BT /F1 12 Tf 100 700 Td ({}) Tj ET", text_content);
        let stream = Stream::new(dictionary! {}, stream_data.into_bytes());
        let content_id = doc.add_object(stream);

        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
        });
        page_ids.push(page_id);
    }

    let pages_dict = dictionary! {
        "Type" => "Pages",
        "Kids" => page_ids.into_iter().map(Reference).collect::<Vec<_>>(),
        "Count" => pages as i64,
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages_dict));

    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", Reference(catalog_id));

    let named = tempfile::NamedTempFile::new().unwrap();
    doc.save(named.path()).unwrap();
    named
}

pub fn create_corrupt_pdf() -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut named = tempfile::NamedTempFile::new().unwrap();
    named
        .write_all(b"%PDF-1.4 corrupt junk data not a valid pdf stream")
        .unwrap();
    named
}
