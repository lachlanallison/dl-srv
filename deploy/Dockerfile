# syntax=docker/dockerfile:1

FROM node:22-alpine AS web
WORKDIR /app/web
COPY web/package.json web/package-lock.json* ./
RUN npm install
COPY web/ ./
RUN npm run build

FROM rust:alpine AS server
RUN apk add --no-cache musl-dev pkgconfig
WORKDIR /app
COPY server/Cargo.toml server/Cargo.lock* ./server/
COPY server/src ./server/src
COPY --from=web /app/web/dist /app/web/dist
WORKDIR /app/server
ENV WEB_DIST=/app/web/dist
RUN cargo build --release

FROM alpine:3.21
RUN apk add --no-cache aria2 ffmpeg python3 py3-pip ca-certificates wget \
  && pip3 install --break-system-packages --upgrade pip yt-dlp

COPY --from=server /app/server/target/release/dlsrv /usr/local/bin/dlsrv
COPY --from=web /app/web/dist /app/web/dist
COPY deploy/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

ENV DL_SRV_ADDR=0.0.0.0:35778 \
    DOWNLOAD_DIR=/media \
    CONFIG_DIR=/config \
    WEB_DIST=/app/web/dist \
    ARIA2_RPC_SECRET=dl-srv-secret \
    YTDLP_PATH=yt-dlp \
    FFMPEG_PATH=ffmpeg

VOLUME ["/media", "/config"]
EXPOSE 35778

ENTRYPOINT ["/entrypoint.sh"]
