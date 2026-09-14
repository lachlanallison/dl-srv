;(function () {
  const filenameEl = document.getElementById('filename')
  const statusEl = document.getElementById('status')
  const sendNasBtn = document.getElementById('sendNas')
  const sendBrowserBtn = document.getElementById('sendBrowser')
  const categorySelect = document.getElementById('category')
  const categoryCustom = document.getElementById('categoryCustom')

  if (!globalThis.dlsrv) {
    filenameEl.textContent = 'Extension failed to load'
    return
  }

  const {
    ext,
    getSettings,
    saveSettings,
    CATEGORY_PRESETS,
    cleanFilenameHint,
    magnetLabel,
    getPendingDownload,
  } = globalThis.dlsrv

  const params = new URLSearchParams(location.search)
  const pendingId = params.get('id')
  let pending = null
  let closing = false

  function syncCustomField() {
    const custom = categorySelect.value === '_custom'
    categoryCustom.hidden = !custom
  }

  function selectedCategory() {
    if (categorySelect.value === '_custom') {
      return categoryCustom.value.trim() || 'inbox'
    }
    return categorySelect.value
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

  function setBusy(busy) {
    sendNasBtn.disabled = busy
    sendBrowserBtn.disabled = busy
  }

  function closePrompt() {
    closing = true
    window.close()
  }

  async function act(type) {
    if (!pending) return
    setBusy(true)
    closing = true
    statusEl.textContent = type === 'dlsrv-intercept-send' ? 'Sending…' : 'Starting browser download…'
    statusEl.className = 'status'
    try {
      const category = selectedCategory()
      if (type === 'dlsrv-intercept-send') {
        await saveSettings({ lastCategory: category })
      }
      const res = await ext.runtime.sendMessage({
        type,
        id: pending.id,
        category: type === 'dlsrv-intercept-send' ? category : undefined,
      })
      if (res?.error) {
        closing = false
        setBusy(false)
        statusEl.textContent = res.error
        statusEl.className = 'status err'
        return
      }
      window.close()
    } catch (e) {
      closing = false
      setBusy(false)
      statusEl.textContent = String(e.message || e)
      statusEl.className = 'status err'
    }
  }

  categorySelect.addEventListener('change', syncCustomField)
  sendNasBtn.addEventListener('click', () => act('dlsrv-intercept-send'))
  sendBrowserBtn.addEventListener('click', () => act('dlsrv-intercept-browser'))

  window.addEventListener('beforeunload', () => {
    if (closing || !pending) return
    ext.runtime.sendMessage({ type: 'dlsrv-intercept-dismiss', id: pending.id }).catch(() => {})
  })

  ;(async () => {
    if (!pendingId) {
      filenameEl.textContent = 'Missing download id'
      setBusy(true)
      return
    }

    pending = await getPendingDownload(pendingId)
    if (!pending) {
      filenameEl.textContent = 'Download no longer pending'
      setBusy(true)
      setTimeout(closePrompt, 1200)
      return
    }

    const displayName = pending.url.startsWith('magnet:')
      ? magnetLabel(pending.url)
      : cleanFilenameHint(pending.filename || pending.url) ||
        pending.filename ||
        pending.url
    filenameEl.textContent = displayName

    const settings = await getSettings()
    const stored = await ext.storage.local.get(['lastCategory'])
    pickCategory(settings, stored)

    if (!settings.serverUrl || !settings.token) {
      statusEl.textContent = 'Set server URL and token in extension settings to use NAS.'
      statusEl.className = 'status err'
      sendNasBtn.disabled = true
    }
  })().catch((e) => {
    statusEl.textContent = String(e.message || e)
    statusEl.className = 'status err'
  })
})()
