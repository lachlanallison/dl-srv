;(function () {
  const status = document.getElementById('status')
  const sendBtn = document.getElementById('send')
  const categorySelect = document.getElementById('category')
  const categoryCustom = document.getElementById('categoryCustom')

  if (!globalThis.dlsrv) {
    status.textContent = 'Failed to load shared.js'
    sendBtn.disabled = true
    return
  }

  const { ext, getSettings, saveSettings, apiUrl, postTask, CATEGORY_PRESETS } = globalThis.dlsrv

  function syncCustomField() {
    const custom = categorySelect.value === '_custom'
    categoryCustom.hidden = !custom
    categoryCustom.required = custom
  }

  function pickCategory(settings, stored) {
    const last = stored.lastCategory || settings.category || 'inbox'
    if (CATEGORY_PRESETS.includes(last)) {
      categorySelect.value = last
      categoryCustom.value = ''
    } else {
      categorySelect.value = '_custom'
      categoryCustom.value = last
    }
    syncCustomField()
  }

  function selectedCategory() {
    if (categorySelect.value === '_custom') {
      return categoryCustom.value.trim() || 'inbox'
    }
    return categorySelect.value
  }

  categorySelect.addEventListener('change', syncCustomField)

  document.getElementById('options').addEventListener('click', () => {
    ext.runtime.openOptionsPage()
  })

  getSettings()
    .then(async (settings) => {
      const stored = await ext.storage.local.get(['lastCategory'])
      pickCategory(settings, stored)

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
          const category = selectedCategory()
          await postTask({ url: tab.url, referer: tab.url, category })
          await saveSettings({ lastCategory: category })
          status.textContent = `Sent to ${category}/`
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
