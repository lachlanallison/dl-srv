;(function () {
  const status = document.getElementById('status')
  const sendBtn = document.getElementById('send')

  if (!globalThis.dlsrv) {
    status.textContent = 'Failed to load shared.js'
    sendBtn.disabled = true
    return
  }

  const { ext, getSettings, apiUrl, postTask } = globalThis.dlsrv

  document.getElementById('options').addEventListener('click', () => {
    ext.runtime.openOptionsPage()
  })

  getSettings()
    .then(async (settings) => {
      document.getElementById('open').addEventListener('click', () => {
        const url = settings.serverUrl || 'http://127.0.0.1:35778'
        ext.tabs.create({ url })
      })

      if (!settings.serverUrl || !settings.token) {
        status.textContent = 'Set server URL and token in settings.'
        sendBtn.disabled = true
        return
      }

      try {
        const res = await fetch(apiUrl(settings, '/health'), {
          headers: { Authorization: `Bearer ${settings.token}` },
        })
        status.textContent = res.ok ? 'Connected to NAS' : `Server error (${res.status})`
      } catch {
        status.textContent = 'Cannot reach server'
      }

      sendBtn.addEventListener('click', async () => {
        sendBtn.disabled = true
        status.textContent = 'Sending…'
        try {
          const [tab] = await ext.tabs.query({ active: true, currentWindow: true })
          if (!tab?.url) throw new Error('No active tab URL')
          await postTask({ url: tab.url, referer: tab.url })
          status.textContent = 'Sent to dl-srv'
        } catch (e) {
          status.textContent = String(e.message || e)
        } finally {
          sendBtn.disabled = false
        }
      })
    })
    .catch((e) => {
      status.textContent = String(e.message || e)
      sendBtn.disabled = true
    })
})()
