/* Shared dl-srv extension logic — background, popup, options. */

const ext = typeof globalThis.browser !== 'undefined' ? globalThis.browser : globalThis.chrome

const DEBUG_MAX = 80

const DEFAULTS = {
  serverUrl: 'http://192.168.1.100:35778',
  token: '',
  category: 'inbox',
  enabled: true,
  minSize: 0,
  ignoreExt: ['html', 'htm', 'txt', 'css', 'js', 'json'],
  ignoreDomains: [],
  autoForceYtdlp: false,
  debugEnabled: true,
}

function actionApi() {
  return ext.action || ext.browserAction
}

function setBadge(text, bgColor) {
  const action = actionApi()
  if (!action?.setBadgeText) return
  try {
    action.setBadgeText({ text })
    if (bgColor && action.setBadgeBackgroundColor) {
      action.setBadgeBackgroundColor({ color: bgColor })
    }
  } catch (e) {
    console.warn('[dl-srv] setBadge', e)
  }
}

function normalizeServerUrl(raw) {
  let url = String(raw || '').trim()
  if (!url) return ''
  url = url.replace(/\/+$/, '')
  url = url.replace(/\/api\/v1\/?$/i, '')
  return url
}

async function getSettings() {
  const keys = Object.keys(DEFAULTS)
  let data = {}
  try {
    data = await ext.storage.local.get([...keys, 'debugLog'])
  } catch (e) {
    console.warn('[dl-srv] storage.local.get', e)
  }
  const merged = {
    ...DEFAULTS,
    ...data,
    serverUrl: normalizeServerUrl(data.serverUrl ?? DEFAULTS.serverUrl),
    ignoreExt: Array.isArray(data.ignoreExt) ? data.ignoreExt : DEFAULTS.ignoreExt,
    ignoreDomains: Array.isArray(data.ignoreDomains) ? data.ignoreDomains : DEFAULTS.ignoreDomains,
  }
  return merged
}

async function saveSettings(payload) {
  if (payload.serverUrl !== undefined) {
    payload.serverUrl = normalizeServerUrl(payload.serverUrl)
  }
  await ext.storage.local.set(payload)
}

async function debugLog(level, message, detail) {
  const entry = {
    ts: new Date().toISOString(),
    level,
    message,
    detail: detail ?? null,
  }
  console.log('[dl-srv]', level, message, detail ?? '')
  try {
    const settings = await getSettings()
    if (!settings.debugEnabled) return
    const stored = await ext.storage.local.get(['debugLog'])
    const logs = Array.isArray(stored.debugLog) ? stored.debugLog : []
    logs.push(entry)
    if (logs.length > DEBUG_MAX) logs.splice(0, logs.length - DEBUG_MAX)
    await ext.storage.local.set({ debugLog: logs })
  } catch (e) {
    console.warn('[dl-srv] debugLog', e)
  }
}

async function getDebugLog() {
  const stored = await ext.storage.local.get(['debugLog'])
  return Array.isArray(stored.debugLog) ? stored.debugLog : []
}

async function clearDebugLog() {
  await ext.storage.local.set({ debugLog: [] })
}

function apiUrl(settings, path) {
  const base = normalizeServerUrl(settings.serverUrl)
  if (!base) throw new Error('Set server URL in extension options')
  return `${base}/api/v1${path}`
}

function formatHttpError(status, url, body) {
  if (status === 401) return `Unauthorized (401) — check API token. ${url}`
  if (status === 404) return `Not found (404) — is dl-srv running at ${url}? Check server URL includes port :35778 and no /api/v1 suffix.`
  const snippet = body ? body.slice(0, 200) : ''
  return `HTTP ${status} from ${url}${snippet ? `: ${snippet}` : ''}`
}

async function apiFetch(settings, path, init = {}) {
  const url = apiUrl(settings, path)
  await debugLog('info', `${init.method || 'GET'} ${url}`, init.body ? JSON.parse(init.body) : null)
  let res
  try {
    res = await fetch(url, init)
  } catch (e) {
    const msg = String(e.message || e)
    await debugLog('error', 'Network error', { url, error: msg })
    throw new Error(
      `Cannot reach server at ${url}. (${msg}) Check: dl-srv is running, URL is http://YOUR-NAS-IP:35778, same network, no typo.`,
    )
  }
  const text = await res.text()
  await debugLog(res.ok ? 'info' : 'error', `HTTP ${res.status}`, { url, body: text.slice(0, 500) })
  return { res, url, text }
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
  const { res, url, text } = await apiFetch(settings, '/tasks', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${settings.token}`,
    },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(formatHttpError(res.status, url, text))
  try {
    return JSON.parse(text)
  } catch {
    return { ok: true }
  }
}

async function pingServer() {
  try {
    const settings = await getSettings()
    if (!settings.serverUrl) {
      setBadge('?')
      await debugLog('warn', 'No server URL configured')
      return { ok: false, error: 'No server URL' }
    }
    if (!settings.token) {
      setBadge('?')
      await debugLog('warn', 'No API token configured')
      return { ok: false, error: 'No API token' }
    }
    const { res, url, text } = await apiFetch(settings, '/health', {
      headers: { Authorization: `Bearer ${settings.token}` },
    })
    if (res.ok) {
      setBadge('')
      return { ok: true, url, data: JSON.parse(text) }
    }
    const err = formatHttpError(res.status, url, text)
    setBadge('!', '#ef4444')
    return { ok: false, error: err }
  } catch (e) {
    const err = String(e.message || e)
    await debugLog('error', 'Health check failed', err)
    setBadge('!', '#ef4444')
    return { ok: false, error: err }
  }
}

async function testConnection() {
  return pingServer()
}

async function testQueue() {
  const settings = await getSettings()
  const { res, url, text } = await apiFetch(settings, '/tasks', {
    headers: { Authorization: `Bearer ${settings.token}` },
  })
  if (!res.ok) throw new Error(formatHttpError(res.status, url, text))
  return JSON.parse(text)
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

function formatDebugLog(logs) {
  if (!logs.length) return '(no log entries yet)'
  return logs
    .map((e) => `[${e.ts}] ${e.level.toUpperCase()} ${e.message}${e.detail ? '\n  ' + JSON.stringify(e.detail) : ''}`)
    .join('\n')
}

function notify(title, message) {
  try {
    ext.notifications.create({
      type: 'basic',
      iconUrl: ext.runtime.getURL('icons/icon128.png'),
      title,
      message: message.slice(0, 250),
    })
  } catch (e) {
    console.warn('[dl-srv] notify', e)
  }
}

globalThis.dlsrv = {
  ext,
  DEFAULTS,
  getSettings,
  saveSettings,
  apiUrl,
  apiFetch,
  postTask,
  pingServer,
  testConnection,
  testQueue,
  debugLog,
  getDebugLog,
  clearDebugLog,
  formatDebugLog,
  extOf,
  hostOf,
  parseCommaList,
  commaList,
  notify,
}
