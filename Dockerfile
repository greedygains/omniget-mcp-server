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
ARG BUILD_ID=0
COPY . .

# Build only the headless standalone server binary in release mode.
RUN cargo build --release --package omniget-server --bin omniget-server

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
    && chmod 755 /usr/local/bin/yt-dlp

WORKDIR /app
COPY --chmod=755 --from=builder /build/target/release/omniget-server /usr/local/bin/omniget-server
COPY --chmod=755 start.sh /app/start.sh

# Configure environment defaults
# PORT=8080 allows dynamic override by Railway or container orchestrators
ENV PORT=8080 \
    RUST_LOG=info,omniget_server=info \
    PATH="/usr/local/bin:/usr/bin:/bin:${PATH}"

# Expose default HTTP port
EXPOSE 8080

# Start command
CMD ["sh", "/app/start.sh"]
