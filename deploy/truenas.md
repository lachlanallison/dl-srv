# TrueNAS Scale setup

Match the same storage pattern as your Jellyfin app: one app folder under `apps/`, with a `config/` child for persistent state. Media uses the same host paths Jellyfin already reads.

## Target layout

```
/mnt/apps/
  jellyfin/
    config/          ← you already have this
  dl-srv/
    config/          ← dl-srv state (config.json, tasks.db, optional bin/yt-dlp)

/mnt/tank/media/     ← same library root as Jellyfin
  movies/
  tv/
  music/
  inbox/             ← optional; default category if you keep "inbox"
```

Downloads land in `media/<category>/`. Set categories to match Jellyfin library folders (`movies`, `tv`, etc.).

## 1. Storage (TrueNAS UI)

**Storage** → your pool → **Add Dataset** (or use an existing `apps` mount)

| Path | Purpose |
|------|---------|
| `/mnt/apps/dl-srv` | App root |
| `/mnt/apps/dl-srv/config` | Persistent config — **required** |

Or from **System Settings → Shell**:

```bash
mkdir -p /mnt/apps/dl-srv/config
```

You do **not** need a separate downloads dataset if files go straight into `media/`.

## 2. Configure paths

On the TrueNAS shell (or wherever you run compose):

```bash
cd /mnt/apps/dl-srv
git clone https://github.com/lachlanallison/dl-srv.git src && cd src
cp .env.example .env
```

Edit `.env`:

```bash
HOST_MEDIA_PATH=/mnt/tank/media
HOST_CONFIG_PATH=/mnt/apps/dl-srv/config
DOWNLOAD_DIR=/media
CONFIG_DIR=/config
```

Check Jellyfin’s mounts if unsure of the media path: **Apps → Installed Applications → Jellyfin → Edit → Storage**. Use the same host path Jellyfin uses for your libraries (often `/mnt/tank/media` or per-library paths).

If Jellyfin mounts libraries individually (e.g. `/mnt/tank/media/movies`, `/mnt/tank/media/tv`) rather than one parent, set:

```bash
HOST_MEDIA_PATH=/mnt/tank/media
```

Categories still map to subfolders (`movies`, `tv`) under that root.

## 3. Deploy

```bash
docker compose up --build -d
```

Open **http://&lt;truenas-ip&gt;:35778** → save the API token from the setup wizard.

## 4. Web UI

| Tab | Action |
|-----|--------|
| **Settings** | Default category → `movies`, `tv`, or your main library folder |
| **Settings** | Jellyfin refresh URL → e.g. `http://jellyfin:8096/Library/Refresh` if Jellyfin is on the same Docker/Apps network, or `http://<truenas-ip>:8096/Library/Refresh` |
| **Health** | Confirm aria2 / yt-dlp / ffmpeg are OK |

## 5. TrueNAS Custom App (alternative to compose)

If you prefer **Apps → Discover Apps → Custom App** instead of shell compose, mirror Jellyfin’s storage wiring:

| Setting | Value |
|---------|-------|
| **Container port** | `35778` → host `35778` |
| **Storage 1** | Host: `/mnt/apps/dl-srv/config` → Mount: `/config` |
| **Storage 2** | Host: `/mnt/tank/media` → Mount: `/media` |
| **Environment** | `DOWNLOAD_DIR=/media`, `CONFIG_DIR=/config`, `DL_SRV_ADDR=0.0.0.0:35778` |

Build or pull the image first — compose from git is the supported path until a published image exists.

## 6. Browser extension

Firefox → `about:debugging` → Load Temporary Add-on → `extension/manifest.json`

Options:

- **Server URL:** `http://<truenas-ip>:35778`
- **API token:** from setup wizard
- **Default category:** `movies` / `tv` / etc.

## What goes where

| Host path | Container | Contents |
|-----------|-----------|----------|
| `/mnt/apps/dl-srv/config` | `/config` | `config.json`, `tasks.db`, `bin/yt-dlp` |
| `/mnt/tank/media` | `/media` | Downloaded files in `<category>/` subfolders |

Same idea as Jellyfin: app state in `apps/<name>/config`, libraries on `media/`.
