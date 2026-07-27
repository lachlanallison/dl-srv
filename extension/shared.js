/* Shared dl-srv extension logic — loaded via importScripts (SW) or <script src> (popup/options). */

const ext = typeof browser !== 'undefined' ? browser : chrome

const DEFAULTS = {
  serverUrl: 'http://192.168.1.100:35778',
  token: '',
  enabled: true,
  category: 'inbox',
  minSize: 0,
  ignoreExt: ['html', 'htm', 'txt', 'css', 'js', 'json'],
  ignoreDomains: [],
  autoForceYtdlp: false,
}

async function getSettings() {
  const data = await ext.storage.sync.get(DEFAULTS)
  return { ...DEFAULTS, ...data }
}

function apiUrl(settings, path) {
  return `${settings.serverUrl.replace(/\/$/, '')}/api/v1${path}`
}

async function postTask(payload) {
  const settings = await getSettings()
  if (!settings.token) throw new Error('Set API token in extension options')
  const body = {
    ...payload,
    category: payload.category || settings.category || 'inbox',
  }
  if (settings.autoForceYtdlp && body.force_ytdlp === undefined) {
    body.force_ytdlp = true
  }
  const res = await fetch(apiUrl(settings, '/tasks'), {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${settings.token}`,
    },
    body: JSON.stringify(body),
  })
  if (!res.ok) {
    const text = await res.text()
    throw new Error(text || res.statusText)
  }
  return res.json()
}

async function pingServer() {
  try {
    const settings = await getSettings()
    if (!settings.token) {
      ext.action.setBadgeText({ text: '?' })
      return false
    }
    const res = await fetch(apiUrl(settings, '/health'), {
      headers: { Authorization: `Bearer ${settings.token}` },
    })
    if (res.ok) {
      ext.action.setBadgeText({ text: '' })
      return true
    }
  } catch {}
  ext.action.setBadgeText({ text: '!' })
  ext.action.setBadgeBackgroundColor({ color: '#ef4444' })
  return false
}

function extOf(name) {
  const m = /\.([^./?#]+)(?:[?#]|$)/.exec(name || '')
  return m ? m[1].toLowerCase() : ''
}

function hostOf(url) {
  try {
    return new URL(url).hostname
  } catch {
    return ''
  }
}

function parseCommaList(s) {
  if (!s || !String(s).trim()) return []
  return String(s)
    .split(',')
    .map((x) => x.trim())
    .filter(Boolean)
}

function commaList(arr) {
  return Array.isArray(arr) ? arr.join(', ') : ''
}

// Export for service worker (importScripts) and page scripts
if (typeof self !== 'undefined') {
  self.dlsrv = {
    ext,
    DEFAULTS,
    getSettings,
    apiUrl,
    postTask,
    pingServer,
    extOf,
    hostOf,
    parseCommaList,
    commaList,
  }
}
