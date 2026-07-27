#!/bin/sh
set -e

DOWNLOAD_DIR="${DOWNLOAD_DIR:-/media}"
CONFIG_DIR="${CONFIG_DIR:-/config}"
ARIA2_RPC_SECRET="${ARIA2_RPC_SECRET:-dl-srv-secret}"
ARIA2_RPC_PORT="${ARIA2_RPC_PORT:-6800}"

mkdir -p "$DOWNLOAD_DIR" "$CONFIG_DIR"

if ! aria2c \
  --enable-rpc \
  --rpc-listen-port="$ARIA2_RPC_PORT" \
  --rpc-listen-all=false \
  --rpc-secret="$ARIA2_RPC_SECRET" \
  --dir="$DOWNLOAD_DIR" \
  --continue=true \
  --max-concurrent-downloads=5 \
  --user-agent="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36" \
  --log-level=error &
then
  echo "aria2c failed to start" >&2
  exit 1
fi

export ARIA2_RPC_URL="http://127.0.0.1:${ARIA2_RPC_PORT}/jsonrpc"
export ARIA2_RPC_SECRET="$ARIA2_RPC_SECRET"

sleep 2
exec /usr/local/bin/dlsrv
