# dl-srv extension — privacy policy

Last updated: July 2026

## Summary

**dl-srv** sends download URLs (and sometimes site cookies needed to fetch those files) **only to a server you configure** — typically your own dl-srv instance on your home NAS or LAN. **No data is sent to the extension developer or any third-party service.**

## What this extension does

- Optionally intercepts browser downloads and queues them on your dl-srv server instead of saving locally.
- Lets you send the current tab URL or a link to your server via the toolbar popup or context menu.
- Stores your server URL, API token, and preferences locally in the browser (`storage.local`).

## Data transmitted

When you use the extension, the following may be sent **to your configured dl-srv server only**:

| Data | When | Why |
|------|------|-----|
| Download URL | Intercept, popup, or context menu | Queue the file on your NAS |
| Page URL (referer) | Intercept or manual send | Help the server fetch the file |
| Site cookies | Intercept or manual send, when needed | Authenticated downloads from file hosts |
| API token | Every API request | Authenticate to your server (Bearer header) |

## Data not collected

- No analytics or telemetry to the developer.
- No accounts or sign-in with the developer.
- No selling or sharing of user data.
- Debug log entries (optional, off by default except local retention) stay **only in your browser** (`storage.local`).

## Data storage

All settings and the optional debug log are stored locally via the WebExtension `storage` API on your device.

## Your control

- **Intercept downloads** is off until you enable it in extension options.
- **Ask category when intercepting** lets you choose NAS vs browser for each download.
- You choose the server URL and can remove the extension at any time to delete local settings.

## Open source

Source code: [github.com/lachlanallison/dl-srv](https://github.com/lachlanallison/dl-srv)

## Contact

Issues and privacy questions: [GitHub Issues](https://github.com/lachlanallison/dl-srv/issues)
