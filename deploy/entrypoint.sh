#!/bin/sh
set -e

DOWNLOAD_DIR="${DOWNLOAD_DIR:-/media}"
CONFIG_DIR="${CONFIG_DIR:-/config}"
ARIA2_RPC_SECRET="${ARIA2_RPC_SECRET:-dl-srv-secret}"
ARIA2_RPC_PORT="${ARIA2_RPC_PORT:-6800}"

mkdir -p "$DOWNLOAD_DIR" "$CONFIG_DIR"

# Keep aria2 cache/DHT under /config (not /root/.cache).
export HOME="$CONFIG_DIR"
mkdir -p "$HOME/.cache/aria2"

ARIA2_LOG="${CONFIG_DIR}/aria2.log"
SESSION_FILE="${CONFIG_DIR}/aria2.session"

# --input-file must be a regular file; a missing path or directory prevents aria2 from starting.
if [ -d "$SESSION_FILE" ]; then
  echo "aria2.session is a directory — removing so aria2 can start" >&2
  rm -rf "$SESSION_FILE"
fi

ARIA2_ARGS=""
if [ -f "$SESSION_FILE" ]; then
  ARIA2_ARGS="$ARIA2_ARGS --input-file=$SESSION_FILE"
fi

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
  --save-session="$SESSION_FILE" \
  $ARIA2_ARGS \
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
