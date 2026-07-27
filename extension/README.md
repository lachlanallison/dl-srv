# dl-srv browser extension

Sends browser downloads (and the current tab URL) to your [dl-srv](../README.md) NAS instead of downloading locally.

## Chrome / Edge (Chromium)

1. Open `chrome://extensions` (or `edge://extensions`)
2. Enable **Developer mode**
3. Click **Load unpacked**
4. Select this `extension/` folder (uses `manifest.json`)

## Firefox

1. Open `about:debugging#/runtime/this-firefox`
2. Click **Load Temporary Add-on…**
3. Choose `extension/manifest.json` in this folder

   `manifest.json` is currently the Firefox manifest. For Chrome, copy `manifest.chromium.json` over `manifest.json` first.

## Configuration

Open extension **Options** (right-click toolbar icon → Options):

| Setting | Description |
|---------|-------------|
| Server URL | dl-srv base URL, e.g. `http://192.168.1.50:35778` |
| API token | From first-run setup or `config/config.json` |
| Default category | Subfolder for downloads (`movies`, `tv`, `inbox`, …) |
| Intercept downloads | Cancel local download and queue on NAS |
| Auto force yt-dlp | Use yt-dlp for intercepted links |
| Min file size | Skip files smaller than N bytes |
| Ignore extensions | Comma list, e.g. `html, htm, txt` |
| Ignore domains | Comma list of hostnames to skip |

## Popup

- **Send current tab URL** — queue the active tab (video pages, etc.)
- **Open web UI** — open dl-srv in a new tab
- **Extension settings** — options page

## Context menu

- Right-click a link → **Download on NAS (dl-srv)**
- Right-click a page → **Download page on NAS (video)** (forces yt-dlp)

## Icons

Generate placeholder icons if missing:

```bash
python extension/icons/gen.py
```
