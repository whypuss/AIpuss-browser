#!/bin/bash
# AIpuss-browser Dashboard split mode launcher
# Runs Rust daemon + Next.js dev server independently

set -e

cd "$(dirname "$0")"

# 1. Kill any existing aipuss-browser processes
pkill -f "aipuss-browser" 2>/dev/null || true
sleep 1

# 2. Clean up stale socket files
rm -rf ~/.agent-browser/default.sock ~/.agent-browser/default.stream ~/.agent-browser/default.pid 2>/dev/null || true

# 3. Start Rust daemon (stream server only, no dashboard)
echo "Starting Rust daemon..."
AI_PROVIDER=nvidia \
AI_API_KEY=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 \
AI_BASE_URL=https://integrate.api.nvidia.com/v1 \
AI_MODEL=meta/llama-3.2-11b-vision-instruct \
~/.local/bin/aipuss-browser stream enable &
sleep 6

# 4. Get stream port
STREAM_PORT=$(cat ~/.agent-browser/default.stream 2>/dev/null)
if [ -z "$STREAM_PORT" ]; then
    echo "ERROR: Failed to get stream port"
    exit 1
fi
echo "Stream server running on port $STREAM_PORT"

# 5. Start Next.js dev server
echo "Starting Next.js dev server..."
DAEMON_URL="http://localhost:$STREAM_PORT" \
NEXT_PUBLIC_DAEMON_URL="http://localhost:$STREAM_PORT" \
npm run dev
