/* Shared dl-srv extension logic — background, popup, options. */

const ext = typeof globalThis.browser !== 'undefined' ? globalThis.browser : globalThis.chrome

const DEBUG_MAX = 80

const DEFAULTS = {
  serverUrl: '',
  token: '',
  category: 'inbox',
  lastCategory: '',
  enabled: true,
  interceptMagnets: true,
  askOnIntercept: true,
  minSize: 0,
  ignoreExt: ['html', 'htm', 'txt', 'css', 'js', 'json'],
  ignoreDomains: [],
  autoForceYtdlp: false,
  debugEnabled: true,
}

const CATEGORY_PRESETS = ['tv', 'movies', 'inbox', 'music']

function cleanFilenameHint(raw) {
  if (!raw) return undefined
  const name = String(raw).split(/[\\/]/).pop() || ''
  if (!name.includes('.')) return undefined
  try {
    const decoded = decodeURIComponent(name.replace(/\+/g, ' '))
    const m = /^[A-Za-z0-9_-]{8,}-(.+\.[A-Za-z0-9]{2,5})$/.exec(decoded)
    return m ? m[1] : decoded
  } catch {
    return name
  }
}

function magnetLabel(url) {
  if (!url?.startsWith('magnet:')) return url || ''
  try {
    const q = url.split('?')[1] || ''
    for (const part of q.split('&')) {
      const [key, val] = part.split('=')
      if (key === 'dn' && val) return decodeURIComponent(val.replace(/\+/g, ' '))
    }
  } catch {
    /* ignore */
  }
  return url.length > 72 ? `${url.slice(0, 69)}…` : url
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

function extensionVersion() {
  try {
    return ext.runtime.getManifest().version
  } catch {
    return ''
  }
}

async function backgroundFetch(settings, path, init = {}) {
  if (!ext.runtime?.sendMessage) return null
  try {
    const result = await ext.runtime.sendMessage({
      type: 'dlsrv-fetch',
      serverUrl: settings.serverUrl,
      path,
      method: init.method || 'GET',
      headers: init.headers || {},
      body: init.body,
    })
    if (result === undefined) {
      await debugLog('warn', 'Background script did not respond — reload the extension')
      return null
    }
    return result
  } catch (e) {
    const msg = String(e.message || e)
    if (msg.includes('Could not establish connection') || msg.includes('Receiving end does not exist')) {
      await debugLog('warn', 'Background script unavailable — reload the extension', msg)
      return null
    }
    throw e
  }
}

async function apiFetch(settings, path, init = {}) {
  const url = apiUrl(settings, path)
  await debugLog('info', `${init.method || 'GET'} ${url}`, init.body ? JSON.parse(init.body) : null)

  const bg = await backgroundFetch(settings, path, init)
  if (bg) {
    if (bg.error) {
      await debugLog('error', 'Network error (background)', { url, error: bg.error })
      throw new Error(
        `Cannot reach server at ${url}. (${bg.error}) Check: dl-srv is running, URL is http://YOUR-NAS-IP:35778, same network, no typo.`,
      )
    }
    await debugLog(bg.ok ? 'info' : 'error', `HTTP ${bg.status} (background)`, {
      url: bg.url || url,
      body: (bg.text || '').slice(0, 500),
    })
    return {
      res: { ok: bg.ok, status: bg.status },
      url: bg.url || url,
      text: bg.text || '',
    }
  }

  let res
  try {
    await debugLog('info', 'Trying direct fetch from extension page', { url })
    res = await fetch(url, init)
  } catch (e) {
    const msg = String(e.message || e)
    await debugLog('error', 'Network error (direct)', { url, error: msg })
      throw new Error(
        `Cannot reach server at ${url}. (${msg}) Check the IP is correct (PowerShell test used a different host?), dl-srv is running, and Firefox proxy is off.`,
      )
  }
  const text = await res.text()
  await debugLog(res.ok ? 'info' : 'error', `HTTP ${res.status} (direct)`, { url, body: text.slice(0, 500) })
  return { res, url, text }
}

async function cookiesForUrls(...urls) {
  if (!ext.cookies?.getAll) return ''
  const seen = new Set()
  const parts = []
  for (const url of urls) {
    if (!url) continue
    try {
      const list = await ext.cookies.getAll({ url })
      for (const c of list) {
        const key = `${c.name}\0${c.domain}`
        if (seen.has(key)) continue
        seen.add(key)
        parts.push(`${c.name}=${c.value}`)
      }
    } catch (e) {
      console.warn('[dl-srv] cookies.getAll', url, e)
    }
  }
  return parts.join('; ')
}

async function postTask(payload) {
  const settings = await getSettings()
  if (!settings.token) throw new Error('Set API token in extension options')
  const urls = [payload.url, payload.referer].filter(Boolean)
  const cookies =
    payload.cookies !== undefined ? payload.cookies : await cookiesForUrls(...urls)
  const body = {
    ...payload,
    category: payload.category || settings.category || 'inbox',
  }
  if (cookies) body.cookies = cookies
  else delete body.cookies
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
      return { ok: false, error: 'No API token — paste the token from dl-srv setup and click Save' }
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

async function pendingStorageGet(keys) {
  if (ext.storage.session) return ext.storage.session.get(keys)
  return ext.storage.local.get(keys)
}

async function pendingStorageSet(data) {
  if (ext.storage.session) return ext.storage.session.set(data)
  return ext.storage.local.set(data)
}

async function getPendingQueue() {
  const data = await pendingStorageGet(['pendingDownloads'])
  return Array.isArray(data.pendingDownloads) ? data.pendingDownloads : []
}

async function setPendingQueue(queue) {
  await pendingStorageSet({ pendingDownloads: queue })
}

async function getPendingDownload(id) {
  const queue = await getPendingQueue()
  return queue.find((p) => p.id === id) || null
}

async function enqueuePendingDownload(item) {
  const queue = await getPendingQueue()
  queue.push(item)
  await setPendingQueue(queue)
  return queue.length
}

globalThis.dlsrv = {
  ext,
  DEFAULTS,
  CATEGORY_PRESETS,
  cleanFilenameHint,
  magnetLabel,
  extensionVersion,
  getSettings,
  saveSettings,
  apiUrl,
  normalizeServerUrl,
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
  setBadge,
  getPendingQueue,
  setPendingQueue,
  getPendingDownload,
  enqueuePendingDownload,
}
