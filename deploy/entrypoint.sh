#!/bin/sh
set -e

DOWNLOAD_DIR="${DOWNLOAD_DIR:-/media}"
CONFIG_DIR="${CONFIG_DIR:-/config}"
ARIA2_RPC_SECRET="${ARIA2_RPC_SECRET:-dl-srv-secret}"
ARIA2_RPC_PORT="${ARIA2_RPC_PORT:-6800}"

mkdir -p "$DOWNLOAD_DIR" "$CONFIG_DIR"

ARIA2_LOG="${CONFIG_DIR}/aria2.log"

# Log aria2 to a file — --log=- spams blank lines into `docker compose logs`.
aria2c --enable-rpc \
  --rpc-listen-port="$ARIA2_RPC_PORT" \
  --rpc-listen-all=false \
  --rpc-secret="$ARIA2_RPC_SECRET" \
  --dir="$DOWNLOAD_DIR" \
  --continue=true \
  --max-concurrent-downloads=5 \
  --enable-dht=true \
  --enable-peer-exchange=true \
  --bt-enable-lpd=true \
  --listen-port=6881 \
  --dht-listen-port=6882 \
  --dht-file-path="$CONFIG_DIR/dht.dat" \
  --save-session="$CONFIG_DIR/aria2.session" \
  --input-file="$CONFIG_DIR/aria2.session" \
  --bt-tracker="udp://tracker.opentrackr.org:1337/announce,udp://open.stealth.si:80/announce,udp://tracker.torrent.eu.org:451/announce" \
  --seed-ratio=0 \
  --bt-save-metadata=true \
  --disable-ipv6=true \
  --daemon=false \
  --quiet=true \
  --summary-interval=0 \
  --user-agent="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36" \
  --log="$ARIA2_LOG" \
  --log-level=warn &

export ARIA2_RPC_URL="http://127.0.0.1:${ARIA2_RPC_PORT}/jsonrpc"
export ARIA2_RPC_SECRET="$ARIA2_RPC_SECRET"

sleep 2

export RUST_LOG="${RUST_LOG:-dlsrv=info}"
echo "dl-srv starting (RUST_LOG=$RUST_LOG, aria2 log=$ARIA2_LOG)" >&2
exec /usr/local/bin/dlsrv
