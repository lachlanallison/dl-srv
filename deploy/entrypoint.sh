#!/bin/sh
set -e

DOWNLOAD_DIR="${DOWNLOAD_DIR:-/media}"
ARIA2_RPC_SECRET="${ARIA2_RPC_SECRET:-dl-srv-secret}"
ARIA2_RPC_PORT="${ARIA2_RPC_PORT:-6800}"

mkdir -p "$DOWNLOAD_DIR" "${CONFIG_DIR:-/config}"

aria2c \
  --enable-rpc \
  --rpc-listen-port="$ARIA2_RPC_PORT" \
  --rpc-listen-all=false \
  --rpc-secret="$ARIA2_RPC_SECRET" \
  --dir="$DOWNLOAD_DIR" \
  --continue=true \
  --max-concurrent-downloads=5 \
  --log=- \
  --log-level=warn &

export ARIA2_RPC_URL="http://127.0.0.1:${ARIA2_RPC_PORT}/jsonrpc"
export ARIA2_RPC_SECRET="$ARIA2_RPC_SECRET"

exec /usr/local/bin/dlsrv
