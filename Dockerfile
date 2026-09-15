# syntax=docker/dockerfile:1

# ==============================================================================
# Stage 1: Builder
# ==============================================================================
FROM rust:slim-bookworm AS builder

WORKDIR /build

# Install minimal build tools and SSL development headers.
# Zero GUI dependencies: no WebKitGTK, GTK3, X11, PipeWire, or ALSA packages.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy repository source files
COPY . .

# Build only the headless standalone server binary in release mode.
RUN cargo build --release --package omniget-server --bin omniget-server && \
    cp target/release/omniget-server /build/omniget-server

# ==============================================================================
# Stage 2: Minimal Production Runtime
# ==============================================================================
FROM debian:bookworm-slim AS runtime

# Install essential runtime dependencies:
# - ca-certificates: TLS root certificates for outbound HTTPS requests
# - curl: healthcheck probing and binary fetching
# - ffmpeg: media extraction, transcoding, and stream muxing
# - python3: execution environment for yt-dlp plugins and extractors
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    ffmpeg \
    python3 \
    && rm -rf /var/lib/apt/lists/*

# Install standalone yt-dlp binary (fulfills MIN_YTDLP_VERSION >= 2026.06.09)
RUN curl -fsSL https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp -o /usr/local/bin/yt-dlp \
    && chmod a+rx /usr/local/bin/yt-dlp

# Create unprivileged system user and group (appuser, UID 1000)
RUN groupadd -g 1000 appuser && \
    useradd -u 1000 -g appuser -m -d /home/appuser -s /bin/bash appuser

# Set up application directory and copy binary from builder
WORKDIR /app
COPY --from=builder --chown=appuser:appuser /build/omniget-server /app/omniget-server

# Switch to non-root user
USER appuser

# Configure environment defaults
# PORT=8080 allows dynamic override by Railway or container orchestrators
ENV PORT=8080 \
    RUST_LOG=info,omniget_server=info \
    PATH="/usr/local/bin:/usr/bin:/bin:${PATH}"

# Expose default HTTP port
EXPOSE 8080

# Native Docker healthcheck querying the public /health endpoint with dynamic port resolution
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -f "http://localhost:${PORT:-8080}/health" || exit 1

# Execute standalone headless MCP server daemon
ENTRYPOINT ["/app/omniget-server"]
