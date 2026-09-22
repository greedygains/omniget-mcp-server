# OmniGet MCP Server (Headless Rust Edition)

> **Universal Social Media, Web & Document Extraction Engine for AI Agents**  
> *Originally created by [tonhowtf](https://github.com/tonhowtf/omniget) | Re-architected, enhanced & maintained as a standalone headless Rust MCP server by **Greedy Gains**.*

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL_3.0-blue.svg)](LICENSE)
[![Rust: 1.80+](https://img.shields.io/badge/Rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)
[![MCP Protocol](https://img.shields.io/badge/MCP-SSE%20%2F%20HTTP-green.svg)](https://modelcontextprotocol.io/)
[![Maintainer](https://img.shields.io/badge/Enhanced%20by-Greedy%20Gains-purple.svg)](#acknowledgments--attribution)
[![Docker](https://img.shields.io/badge/Docker-Ready-2496ED.svg)](Dockerfile)

---

## 💡 Overview

**OmniGet MCP Server** is an ultra-fast, headless [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) server and REST API built in **Native Rust**. It equips AI Agents (such as Claude Desktop, Cursor, and ChatGPT) with the ability to extract, parse, and ingest rich content from social networks, modern web articles, and documents in real time.

Unlike traditional scrapers that rely on heavy browser automation (Chromium / Puppeteer / Playwright), OmniGet MCP operates **100% headlessly** using asynchronous Rust HTTP engines and embedded state decoders. It responds in **1–2 seconds**, consumes less than **30 MB of RAM**, and runs seamlessly in minimal container environments.

---

## ✨ Key Features

- **🚀 100% Headless Native Rust**: Zero browser overhead. No Chromium, No WebKit, and No X11 libraries required.
- **📱 Facebook Post & Media Extraction (Breakthrough)**:
  - Extracts **100% complete, untruncated captions** without text truncation (`...`).
  - Decodes embedded Relay/GraphQL state and Bloks data structures directly from raw HTML.
  - Automatically resolves share links and slugged URLs into canonical formats, **bypassing cloud datacenter login walls**.
  - Retrieves high-resolution photo galleries, video streams, and hashtags.
- **📸 Instagram Posts, Reels & Carousels**:
  - Full caption and hashtag extraction.
  - Multi-image carousel parsing with direct CDN asset URLs.
- **🐦 X / Twitter Thread Unrolling**:
  - Extracts single posts (`x_post`) with media and metrics.
  - Unrolls full multi-tweet threads (`x_thread`) into continuous, structured reading material.
- **🌐 Universal Web-to-Markdown**:
  - Strips ads, navigation, scripts, and clutter from any public article.
  - Converts web pages into clean, token-efficient Markdown optimized for LLMs.
- **📄 PDF Document Extraction**:
  - Ingests and extracts clean plain-text from remote URLs or local PDF files.
- **🎬 Universal Media Info**:
  - Powered by `yt-dlp` integration supporting metadata extraction for 1,800+ media sites.
- **🔒 Enterprise-Grade Security**:
  - Timing-safe Bearer token authentication (`AUTH_TOKEN`) using constant-time byte comparison (`constant_time_eq`) to protect all operational endpoints.
- **⚡ Dual Protocol Support**:
  - **MCP over SSE** (`GET /sse` + `POST /messages`)
  - **Streamable HTTP MCP** (`POST /mcp` JSON-RPC 2.0)
  - **REST API + OpenAPI 3.1** (`GET /openapi.json` + `POST /api/*`)

---

## 🛠 Available MCP Tools

When connected to an MCP client (Claude Desktop, Cursor, etc.), the server exposes 7 powerful extraction tools:

| Tool | Parameters | Description |
| :--- | :--- | :--- |
| `facebook_post` | `url` *(string, required)* | Extracts Facebook post content: full untruncated caption, author, high-res photos, video URLs, and hashtags. |
| `instagram_post` | `url` *(string, required)* | Extracts Instagram post, Reel, or carousel: full caption, author, and all images/videos. |
| `x_post` | `url` *(string, required)* | Extracts a single X/Twitter post: author, full text, attached media, and engagement metrics. |
| `x_thread` | `url` *(string, required)* | Unrolls an entire X/Twitter thread into a single, cohesive Markdown document. |
| `web_to_markdown` | `url` *(string, required)* | Converts any public web page or news article into clean, clutter-free Markdown for LLMs. |
| `pdf_text` | `url_or_path` *(string, required)* | Extracts readable text from a remote PDF link or local filesystem path. |
| `media_info` | `url` *(string, required)* | Universal video/audio metadata extraction across 1,800+ supported video platforms. |

---

## 🚀 Quickstart

### 1. Run with Docker (Recommended)

```bash
# Build the Docker image
docker build -t omniget-mcp-server .

# Run the container
docker run -d \
  -p 8080:8080 \
  -e AUTH_TOKEN="your-secure-secret-token" \
  --name omniget-mcp \
  omniget-mcp-server
```

Check health status:
```bash
curl http://localhost:8080/health
# Response: {"ok": true}
```

---

### 2. Run Locally with Cargo

Ensure you have **Rust 1.80+** installed:

```bash
# Clone the repository
git clone https://github.com/greedygains/omniget-mcp-server.git
cd omniget-mcp-server

# Set your authentication token and run
export AUTH_TOKEN="your-secure-secret-token"
cargo run -p omniget-server --release
```

The server will listen on `http://0.0.0.0:8080`.

---

### 3. Deploy to Railway

[![Deploy on Railway](https://railway.com/button.svg)](https://railway.com)

1. Fork or push this repository to your GitHub account.
2. Create a **New Project** in [Railway](https://railway.com) and select **Deploy from GitHub repo**.
3. Under **Variables**, add:
   - `AUTH_TOKEN`: A secure random token (e.g. `openssl rand -hex 16`).
4. Railway will automatically build the `Dockerfile`, assign an HTTPS domain, and manage the `$PORT` variable.

---

## 🔌 Connecting to AI Clients

### Claude Desktop

Edit your `claude_desktop_config.json`:
- **macOS**: `~/Library/Application Support/Claude/claude_desktop_config.json`
- **Windows**: `%APPDATA%\Claude\claude_desktop_config.json`

```json
{
  "mcpServers": {
    "omniget": {
      "url": "https://<your-domain>.up.railway.app/sse?token=<YOUR_AUTH_TOKEN>"
    }
  }
}
```

*Or using Bearer header syntax:*
```json
{
  "mcpServers": {
    "omniget": {
      "url": "https://<your-domain>.up.railway.app/sse",
      "headers": {
        "Authorization": "Bearer <YOUR_AUTH_TOKEN>"
      }
    }
  }
}
```

---

### Cursor IDE

Add to `~/.cursor/mcp.json` or project-level `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "omniget": {
      "url": "https://<your-domain>.up.railway.app/sse?token=<YOUR_AUTH_TOKEN>"
    }
  }
}
```

---

### ChatGPT (Custom Actions)

1. In ChatGPT GPT Builder, go to **Configure > Actions > Create new action**.
2. Set Authentication to **Bearer** and input your `<YOUR_AUTH_TOKEN>`.
3. Import the OpenAPI schema from:
   `https://<your-domain>.up.railway.app/openapi.json?token=<YOUR_AUTH_TOKEN>`

---

## 📡 API Endpoints

| Method | Endpoint | Auth | Description |
| :--- | :--- | :---: | :--- |
| `GET` | `/health` | No | Orchestrator health check (`{"ok": true}`) |
| `GET` | `/sse` | Yes | Model Context Protocol SSE stream initiation |
| `POST` | `/messages` | Yes | Post JSON-RPC 2.0 messages to an active SSE session |
| `POST` | `/mcp` | Yes | Streamable HTTP MCP JSON-RPC 2.0 endpoint |
| `GET` | `/openapi.json`| Yes | OpenAPI 3.1 schema specification |
| `POST` | `/api/facebook`| Yes | Direct REST endpoint for Facebook extraction |
| `POST` | `/api/instagram`| Yes | Direct REST endpoint for Instagram extraction |
| `POST` | `/api/x/post` | Yes | Direct REST endpoint for X/Twitter extraction |
| `POST` | `/api/x/thread` | Yes | Direct REST endpoint for X thread unrolling |
| `POST` | `/api/markdown`| Yes | Direct REST endpoint for Web-to-Markdown |
| `POST` | `/api/pdf` | Yes | Direct REST endpoint for PDF text extraction |

---

## 🤝 Acknowledgments & Attribution

This open-source project is built on the foundations of the open-source community:

- **Upstream Project**: The extraction core was originally derived from [OmniGet](https://github.com/tonhowtf/omniget) by **[tonhowtf](https://github.com/tonhowtf)**, a multi-platform downloader application. We express our immense gratitude for their pioneering work on universal media extraction logic.
- **Greedy Gains Enhancements**:
  - Decoupled the engine into a standalone, headless microservice free of desktop/Tauri dependencies.
  - Implemented the full **Model Context Protocol (MCP)** specification over Server-Sent Events (SSE) and Streamable HTTP.
  - Architected the **Native Rust Facebook & Instagram extraction engine**, enabling full 100% untruncated caption recovery and cloud datacenter login wall bypass.
  - Engineered timing-safe authentication, containerization, and the REST/OpenAPI 3.1 bridge.

---

## 📄 License

This project is licensed under the **GNU General Public License v3.0 (GPL-3.0)** in accordance with the upstream OmniGet project.  
See the [LICENSE](LICENSE) file for complete details.
