const { ext, getSettings, apiUrl, postTask } = self.dlsrv

async function init() {
  const settings = await getSettings()
  const status = document.getElementById('status')
  const sendBtn = document.getElementById('send')

  if (!settings.serverUrl || !settings.token) {
    status.textContent = 'Configure server URL and token in settings.'
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

  sendBtn.onclick = async () => {
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
  }

  document.getElementById('open').onclick = () => ext.tabs.create({ url: settings.serverUrl })
  document.getElementById('options').onclick = () => ext.runtime.openOptionsPage()
}

init()
