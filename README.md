# OmniGet MCP Server

Production-ready, headless Model Context Protocol (MCP) & REST server for universal social media, article, document, and media extraction. Built with high-performance Rust, Axum, and timing-safe Bearer token authorization.

## Features

- **MCP over Server-Sent Events (SSE)**: Standard `GET /sse` and `POST /messages` endpoints for AI Agents (Claude Desktop, Cursor, etc.).
- **Streamable HTTP MCP**: Direct JSON-RPC 2.0 interface at `POST /mcp`.
- **REST / OpenAPI Bridge**: Direct HTTP endpoints at `/api/*` and OpenAPI 3.1 specification at `GET /openapi.json`.
- **Timing-Safe Authentication**: Constant-time Bearer token comparison (`AUTH_TOKEN`) across all sensitive routes.
- **Extraction Tools**:
  - `x_post`: Extract single X/Twitter posts with author info, media, and metrics.
  - `x_thread`: Unroll entire X/Twitter conversation threads by author.
  - `web_to_markdown`: Fetch public web pages and convert to clean Markdown without HTML noise.
  - `pdf_text`: Extract readable text from local PDF files or remote PDF URLs.
  - `media_info`: Universal metadata extraction for 1,800+ media platforms via yt-dlp.

## Endpoints

| Endpoint | Method | Auth Required | Description |
|---|---|---|---|
| `/health` | GET | No | Liveness/health probe (`{"ok": true}`) |
| `/sse` | GET | Yes | SSE connection initiation (returns session endpoint) |
| `/messages` | POST | Yes | Post JSON-RPC messages to active SSE session |
| `/mcp` | POST | Yes | Streamable HTTP MCP JSON-RPC 2.0 handler |
| `/api/*` | GET/POST | Yes | REST extraction endpoints |
| `/openapi.json` | GET | Yes | OpenAPI 3.1 schema specification |

## Environment Variables

- `AUTH_TOKEN` (required): Secret Bearer token for authenticating requests.
- `PORT` (optional): Port to bind to (defaults to `8080`).

## Deployment

### Docker

```bash
docker build -t omniget-mcp-server .
docker run -p 8080:8080 -e AUTH_TOKEN=your-secure-token omniget-mcp-server
```

### Railway

1. Link repository to Railway.
2. Configure `AUTH_TOKEN` in Railway Environment Variables.
3. Deploy — Railway automatically provisions HTTPS and binds `$PORT`.

## Client Configuration

### Claude Desktop (`claude_desktop_config.json`)

```json
{
  "mcpServers": {
    "omniget": {
      "url": "https://<your-railway-domain>/sse",
      "headers": {
        "Authorization": "Bearer <AUTH_TOKEN>"
      }
    }
  }
}
```

### Cursor (`~/.cursor/mcp.json`)

```json
{
  "mcpServers": {
    "omniget": {
      "url": "https://<your-railway-domain>/sse",
      "headers": {
        "Authorization": "Bearer <AUTH_TOKEN>"
      }
    }
  }
}
```

## License

Apache-2.0 / MIT
