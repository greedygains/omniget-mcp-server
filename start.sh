#!/bin/sh
set -e
echo "Starting OmniGet MCP Server on port ${PORT:-8080}..."
exec /usr/local/bin/omniget-server
