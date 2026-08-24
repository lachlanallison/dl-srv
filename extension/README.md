# dl-srv browser extension

Sends browser downloads (and the current tab URL) to your [dl-srv](../README.md) NAS instead of downloading locally.

**Privacy:** URLs and cookies (when needed) are sent only to the server URL you configure. See [PRIVACY.md](PRIVACY.md).

## Firefox (development)

**Primary target browser** — develop and test here first. WebExtension APIs differ from Chromium (e.g. `downloads.download` referer is `headers: [{ name: 'Referer', … }]`, not `referrer`).

1. Open `about:debugging#/runtime/this-firefox`
2. Click **Load Temporary Add-on…**
3. Choose `extension/manifest.json`

## Firefox (Mozilla Add-ons)

See [AMO_SUBMISSION.md](AMO_SUBMISSION.md) for store packaging, listing text, and reviewer notes.

```bash
cd extension
npm install
npm run lint
npm run build:amo
# Upload dist/dl-srv-firefox.zip to addons.mozilla.org
```

## Chrome / Edge (Chromium)

1. Copy `manifest.chromium.json` over `manifest.json`
2. Open `chrome://extensions` → **Load unpacked** → select this folder

## Configuration

Open extension **Options** (right-click toolbar icon → Options):

| Setting | Description |
|---------|-------------|
| Server URL | dl-srv base URL, e.g. `http://192.168.1.50:35778` |
| API token | From first-run setup or `config/config.json` |
| Default category | Subfolder for downloads (`movies`, `tv`, `inbox`, …) |
| Intercept downloads | Pause local download and ask NAS vs browser |
| Ask category when intercepting | Category prompt on each intercept |
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
