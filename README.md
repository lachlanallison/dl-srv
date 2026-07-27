# dl-srv

Remote download inbox for your NAS — HTTP, torrents, and video sites via **aria2** + **yt-dlp**, with a unified web UI and browser extension.

**Repository:** https://github.com/lachlanallison/dl-srv

## Stack



| Layer | Tech |

|-------|------|

| Daemon | Rust (Axum) |

| Web UI | Svelte + Vite |

| Extension | Chrome / Firefox MV3 |

| HTTP/torrent | aria2 (JSON-RPC sidecar) |

| Video sites | yt-dlp (+ aria2 external downloader) |



## Quick start (Docker)



```bash

cp .env.example .env   # edit HOST_MEDIA_PATH, HOST_CONFIG_PATH for your NAS

docker compose up --build -d

```



Open http://localhost:35778 — the **setup wizard** shows your API token on first visit.

### TrueNAS (Jellyfin-style layout)

Same pattern as Jellyfin: `apps/<app>/config` for state, shared `media/` for libraries.

```
/mnt/apps/
  jellyfin/config/     ← existing
  dl-srv/config/       ← dl-srv state
/mnt/tank/media/       ← movies/, tv/, … (same as Jellyfin)
```

See [deploy/truenas.md](deploy/truenas.md) for step-by-step UI and shell instructions.

In `.env`:

```bash
HOST_MEDIA_PATH=/mnt/tank/media
HOST_CONFIG_PATH=/mnt/apps/dl-srv/config
DOWNLOAD_DIR=/media
CONFIG_DIR=/config
```

Create config before first run: `mkdir -p /mnt/apps/dl-srv/config`



### Browser extension



See [extension/README.md](extension/README.md) for Chrome and Firefox install steps. For Mozilla Add-ons submission, see [extension/AMO_SUBMISSION.md](extension/AMO_SUBMISSION.md).



1. Load unpacked from `extension/` (Chrome) or `manifest.firefox.json` (Firefox)

2. Options → **Server URL** + **API token** from setup

3. Set **default category** to match library folders (`movies`, `tv`, …)

4. Browse — downloads go to the NAS instead of your PC



## Features by tier



### Tier 1 — daily workflow



- **First-run setup wizard** — token, copy button, extension instructions

- **Extension category mapping** — default folder per download

- **`.torrent` file handling** — fetch and add via aria2

- **Smarter video routing** — hostname list + yt-dlp simulate fallback



### Tier 2 — library integration



- **Quality presets** — best / 1080p / 720p / audio (global default + per-add override)

- **Cookies file** — Netscape cookies path for age-gated sites

- **Webhooks + Jellyfin refresh** — POST on complete, optional library scan URL

- **Queue filters** — all / active / completed / failed

- **Settings tab** — token regenerate, CORS origins display



### Tier 3 — automation & clients



- **RSS auto-fetch** — poll feeds, regex filter, dedupe, queue downloads

- **Sonarr / Radarr (qBittorrent API)** — use dl-srv as download client (see below)

- **Rate limiting** — configurable POST /tasks throttle

- **Caddy reverse proxy** — see [deploy/Caddyfile.example](deploy/Caddyfile.example)



## Sonarr / Radarr setup



dl-srv exposes a **qBittorrent Web API v2**-compatible endpoint so *arr apps can queue torrents without qBittorrent.



| Setting | Value |

|---------|-------|

| Download client | qBittorrent |

| Host | `dl-srv` (Docker network) or `http://your-nas:35778` |

| Port | `35778` (d-l-s-r-v on a phone keypad) |

| Username | `admin` |

| Password | `adminadmin` |

| Category | Maps to download subfolder (`tv`, `movies`, …) |



Point Sonarr/Radarr **completed download handling** at the same root path Jellyfin scans, e.g. `/data/downloads/tv` and `/data/downloads/movies`.



## RSS feeds



Web UI → **RSS** tab:



- Add feed URL, category, optional filter regex, poll interval

- New items matching the regex are queued automatically

- Duplicates are skipped by GUID



## Reverse proxy (Caddy)



Copy [deploy/Caddyfile.example](deploy/Caddyfile.example) and terminate TLS in front of dl-srv. Keep `flush_interval -1` for the SSE `/api/v1/events` stream.



## Updating yt-dlp in Docker



The image ships yt-dlp via pip. Upgrade without rebuilding:



- **Web UI** → Health → **Update yt-dlp now**

- Or: `docker compose exec dl-srv yt-dlp -U`

- Or mount a custom binary at `./config/bin/yt-dlp`



Health checks compare installed versions against GitHub releases and show alerts for **yt-dlp** and **ffmpeg**. ffmpeg is updated by rebuilding the image (`docker compose build --pull`).



## Development



```bash

# Terminal 1 — aria2 (required)

aria2c --enable-rpc --rpc-secret=dev --dir=./data/downloads



# Terminal 2 — API (needs built web UI for full UI)

cd web && npm install && npm run build

cd ../server && cargo run



# Terminal 3 — web dev with proxy

cd web && npm run dev

```



Copy `.env.example` to `.env` for local overrides.



Env vars: `DL_SRV_ADDR`, `DL_SRV_TOKEN`, `DOWNLOAD_DIR`, `CONFIG_DIR`, `ARIA2_RPC_URL`, `ARIA2_RPC_SECRET`, `YTDLP_PATH`, `FFMPEG_PATH`, `WEB_DIST`.



## API



Most routes under `/api/v1` require `Authorization: Bearer <token>` or `X-Api-Token`.



| Route | Description |

|-------|-------------|

| `GET /setup` | First-run info (no auth) |

| `POST /setup` | Mark setup complete (auth required) |

| `GET /health` | aria2 + yt-dlp + ffmpeg versions |

| `GET /events` | SSE task updates |

| `GET /tasks?status=` | List tasks (`active`, `completed`, `failed`) |

| `POST /tasks` | `{ "url", "category?", "quality?", "referer?", "force_ytdlp?" }` |

| `GET/PUT /settings` | Server settings |

| `POST /settings/regenerate-token` | New API token |

| `GET/POST /rss/feeds`, `DELETE /rss/feeds/{id}` | RSS management |

| `POST /binaries/ytdlp/update` | Run `yt-dlp -U` |



Downloads land in `DOWNLOAD_DIR/<category>/` (default category: `inbox`). On TrueNAS, point `DOWNLOAD_DIR` at your media root so categories match library folders (`movies`, `tv`, …).

## License

MIT — see [LICENSE](LICENSE).
