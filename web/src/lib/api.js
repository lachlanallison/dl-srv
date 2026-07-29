const TOKEN_KEY = 'dl-srv-token'

export function getToken() {
  return localStorage.getItem(TOKEN_KEY) || ''
}

export function setToken(token) {
  localStorage.setItem(TOKEN_KEY, token.trim())
}

function headers(auth = true) {
  const h = { 'Content-Type': 'application/json' }
  if (auth) {
    const token = getToken()
    if (token) h['Authorization'] = `Bearer ${token}`
  }
  return h
}

export function parseError(text, fallback) {
  if (!text) return fallback
  try {
    const j = JSON.parse(text)
    if (typeof j === 'string') return j
    if (j.error) return j.error
    if (j.message) return j.message
  } catch {}
  return text.length > 200 ? `${text.slice(0, 200)}…` : text
}

async function request(path, options = {}) {
  const auth = options.auth !== false
  const res = await fetch(`/api/v1${path}`, {
    ...options,
    headers: { ...headers(auth), ...(options.headers || {}) },
  })
  if (!res.ok) {
    const text = await res.text()
    const err = new Error(parseError(text, res.statusText))
    err.status = res.status
    throw err
  }
  if (res.status === 204) return null
  return res.json()
}

export const api = {
  setup: () => request('/setup', { auth: false }),
  completeSetup: () => request('/setup', { method: 'POST', body: JSON.stringify({}) }),

  listTasks: (status) => {
    const q = status && status !== 'all' ? `?status=${encodeURIComponent(status)}` : ''
    return request(`/tasks${q}`)
  },
  createTask: (body) => request('/tasks', { method: 'POST', body: JSON.stringify(body) }),
  pauseTask: (id) => request(`/tasks/${id}/pause`, { method: 'POST' }),
  resumeTask: (id) => request(`/tasks/${id}/resume`, { method: 'POST' }),
  removeTask: (id) => request(`/tasks/${id}`, { method: 'DELETE' }),

  health: () => request('/health'),
  settings: () => request('/settings'),
  updateSettings: (body) => request('/settings', { method: 'PUT', body: JSON.stringify(body) }),
  regenerateToken: () => request('/settings/regenerate-token', { method: 'POST' }),
  updateYtdlp: () => request('/binaries/ytdlp/update', { method: 'POST' }),

  listRssFeeds: () => request('/rss/feeds'),
  createRssFeed: (body) => request('/rss/feeds', { method: 'POST', body: JSON.stringify(body) }),
  deleteRssFeed: (id) => request(`/rss/feeds/${id}`, { method: 'DELETE' }),

  events(onTask) {
    const token = getToken()
    return subscribeEvents(token, onTask)
  },
}

function subscribeEvents(token, onTask) {
  let closed = false
  let retryMs = 1000

  async function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms))
  }

  async function connect() {
    if (closed || !token) return
    try {
      const res = await fetch('/api/v1/events', {
        headers: { Authorization: `Bearer ${token}`, Accept: 'text/event-stream' },
      })
      if (!res.ok || !res.body) {
        await sleep(retryMs)
        retryMs = Math.min(retryMs * 2, 30000)
        return connect()
      }
      retryMs = 1000
      const reader = res.body.getReader()
      const decoder = new TextDecoder()
      let buffer = ''
      while (!closed) {
        const { value, done } = await reader.read()
        if (done) break
        buffer += decoder.decode(value, { stream: true })
        const parts = buffer.split('\n\n')
        buffer = parts.pop() || ''
        for (const part of parts) {
          for (const line of part.split('\n')) {
            if (line.startsWith('data:')) {
              try {
                onTask(JSON.parse(line.slice(5).trim()))
              } catch {}
            }
          }
        }
      }
    } catch {
      /* stream dropped */
    }
    if (!closed) {
      await sleep(retryMs)
      retryMs = Math.min(retryMs * 2, 30000)
      connect()
    }
  }

  connect()
  return () => {
    closed = true
  }
}

export function formatBytes(n) {
  if (!n) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  let v = n
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(i ? 1 : 0)} ${units[i]}`
}

export function formatSpeed(n) {
  return `${formatBytes(n)}/s`
}

export function isTorrentUrl(url) {
  const u = (url || '').trim().toLowerCase()
  return u.startsWith('magnet:') || u.endsWith('.torrent')
}

export function formatShareRatio(uploaded, downloaded) {
  if (!downloaded || downloaded <= 0) return '0.00'
  return (uploaded / downloaded).toFixed(2)
}

export const QUALITY_PRESETS = [
  { value: 'best', label: 'Best available' },
  { value: '1080p', label: '1080p max' },
  { value: '720p', label: '720p max' },
  { value: 'audio', label: 'Audio only' },
]
