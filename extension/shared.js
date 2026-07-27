/* Shared dl-srv extension logic — background, popup, options. */

const ext = typeof globalThis.browser !== 'undefined' ? globalThis.browser : globalThis.chrome

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

async function getSettings() {
  const keys = Object.keys(DEFAULTS)
  let data = {}
  try {
    data = await ext.storage.local.get(keys)
  } catch (e) {
    console.warn('[dl-srv] storage.local.get', e)
  }
  return {
    ...DEFAULTS,
    ...data,
    ignoreExt: Array.isArray(data.ignoreExt) ? data.ignoreExt : DEFAULTS.ignoreExt,
    ignoreDomains: Array.isArray(data.ignoreDomains) ? data.ignoreDomains : DEFAULTS.ignoreDomains,
  }
}

async function saveSettings(payload) {
  await ext.storage.local.set(payload)
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
      setBadge('?')
      return false
    }
    const res = await fetch(apiUrl(settings, '/health'), {
      headers: { Authorization: `Bearer ${settings.token}` },
    })
    if (res.ok) {
      setBadge('')
      return true
    }
  } catch (e) {
    console.warn('[dl-srv] ping', e)
  }
  setBadge('!', '#ef4444')
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

function notify(title, message) {
  try {
    ext.notifications.create({
      type: 'basic',
      iconUrl: ext.runtime.getURL('icons/icon128.png'),
      title,
      message,
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
  postTask,
  pingServer,
  extOf,
  hostOf,
  parseCommaList,
  commaList,
  notify,
}
