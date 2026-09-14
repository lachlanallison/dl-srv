;(function () {
  const status = document.getElementById('status')
  const debugOut = document.getElementById('debugOut')

  if (!globalThis.dlsrv) {
    status.textContent = 'Failed to load shared.js'
    debugOut.textContent = 'shared.js missing'
    return
  }

  const {
    extensionVersion,
    getSettings,
    saveSettings,
    parseCommaList,
    commaList,
    testConnection,
    testQueue,
    getDebugLog,
    clearDebugLog,
    formatDebugLog,
    apiUrl,
    debugLog,
  } = globalThis.dlsrv

  async function refreshDebugLog() {
    const logs = await getDebugLog()
    debugOut.textContent = formatDebugLog(logs)
    debugOut.scrollTop = debugOut.scrollHeight
  }

  function readForm() {
    return {
      serverUrl: document.getElementById('serverUrl').value.trim(),
      token: document.getElementById('token').value.trim(),
      category: document.getElementById('category').value.trim() || 'inbox',
      enabled: document.getElementById('enabled').checked,
      interceptMagnets: document.getElementById('interceptMagnets').checked,
      askOnIntercept: document.getElementById('askOnIntercept').checked,
      autoForceYtdlp: document.getElementById('autoForceYtdlp').checked,
      minSize: parseInt(document.getElementById('minSize').value, 10) || 0,
      allowExt: parseCommaList(document.getElementById('allowExt').value),
      ignoreExt: parseCommaList(document.getElementById('ignoreExt').value),
      ignoreDomains: parseCommaList(document.getElementById('ignoreDomains').value),
      debugEnabled: document.getElementById('debugEnabled').checked,
    }
  }

  async function load() {
    document.getElementById('extVersion').textContent = extensionVersion() || 'unknown'
    const data = await getSettings()
    document.getElementById('serverUrl').value = data.serverUrl || ''
    document.getElementById('token').value = data.token || ''
    document.getElementById('category').value = data.category || 'inbox'
    document.getElementById('enabled').checked = data.enabled !== false
    document.getElementById('interceptMagnets').checked = data.interceptMagnets !== false
    document.getElementById('askOnIntercept').checked = data.askOnIntercept !== false
    document.getElementById('autoForceYtdlp').checked = !!data.autoForceYtdlp
    document.getElementById('minSize').value = data.minSize ?? 0
    document.getElementById('allowExt').value = commaList(data.allowExt)
    document.getElementById('ignoreExt').value = commaList(data.ignoreExt)
    document.getElementById('ignoreDomains').value = commaList(data.ignoreDomains)
    document.getElementById('debugEnabled').checked = data.debugEnabled !== false
    await refreshDebugLog()
  }

  document.getElementById('save').addEventListener('click', async () => {
    try {
      await saveSettings(readForm())
      status.textContent = 'Saved.'
      status.className = 'ok'
      await debugLog('info', 'Settings saved')
      await refreshDebugLog()
    } catch (e) {
      status.textContent = String(e.message || e)
      status.className = 'err'
    }
  })

  document.getElementById('testHealth').addEventListener('click', async () => {
    status.textContent = 'Testing…'
    status.className = ''
    try {
      await saveSettings(readForm())
      const settings = await getSettings()
      const healthUrl = apiUrl(settings, '/health')
      await debugLog('info', 'Manual health test', { url: healthUrl })
      const result = await testConnection()
      await refreshDebugLog()
      if (result.ok) {
        const h = result.data || {}
        const parts = [`OK — ${healthUrl}`]
        if (h.dlsrv_version) parts.push(`dl-srv ${h.dlsrv_version}`)
        if (h.aria2_ok) parts.push(`aria2 ${h.aria2_version || 'connected'}`)
        status.textContent = parts.join(' · ')
        status.className = 'ok'
      } else {
        status.textContent = result.error || 'Failed'
        status.className = 'err'
      }
    } catch (e) {
      status.textContent = String(e.message || e)
      status.className = 'err'
      await refreshDebugLog()
    }
  })

  document.getElementById('testQueue').addEventListener('click', async () => {
    status.textContent = 'Testing queue…'
    status.className = ''
    try {
      await saveSettings(readForm())
      const tasks = await testQueue()
      const n = Array.isArray(tasks) ? tasks.length : 0
      status.textContent = `Queue OK — ${n} task(s) listed`
      status.className = 'ok'
      await refreshDebugLog()
    } catch (e) {
      status.textContent = String(e.message || e)
      status.className = 'err'
      await refreshDebugLog()
    }
  })

  document.getElementById('clearLog').addEventListener('click', async () => {
    await clearDebugLog()
    await refreshDebugLog()
    status.textContent = 'Log cleared.'
    status.className = ''
  })

  load().catch((e) => {
    status.textContent = String(e.message || e)
    status.className = 'err'
  })
})()
