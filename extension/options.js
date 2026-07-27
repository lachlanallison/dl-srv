;(function () {
  const status = document.getElementById('status')

  if (!globalThis.dlsrv) {
    status.textContent = 'Failed to load shared.js'
    return
  }

  const { getSettings, saveSettings, parseCommaList, commaList } = globalThis.dlsrv

  async function load() {
    const data = await getSettings()
    document.getElementById('serverUrl').value = data.serverUrl || ''
    document.getElementById('token').value = data.token || ''
    document.getElementById('category').value = data.category || 'inbox'
    document.getElementById('enabled').checked = data.enabled !== false
    document.getElementById('autoForceYtdlp').checked = !!data.autoForceYtdlp
    document.getElementById('minSize').value = data.minSize ?? 0
    document.getElementById('ignoreExt').value = commaList(data.ignoreExt)
    document.getElementById('ignoreDomains').value = commaList(data.ignoreDomains)
  }

  document.getElementById('save').addEventListener('click', async () => {
    try {
      await saveSettings({
        serverUrl: document.getElementById('serverUrl').value.trim(),
        token: document.getElementById('token').value.trim(),
        category: document.getElementById('category').value.trim() || 'inbox',
        enabled: document.getElementById('enabled').checked,
        autoForceYtdlp: document.getElementById('autoForceYtdlp').checked,
        minSize: parseInt(document.getElementById('minSize').value, 10) || 0,
        ignoreExt: parseCommaList(document.getElementById('ignoreExt').value),
        ignoreDomains: parseCommaList(document.getElementById('ignoreDomains').value),
      })
      status.textContent = 'Saved.'
    } catch (e) {
      status.textContent = String(e.message || e)
    }
  })

  load().catch((e) => {
    status.textContent = String(e.message || e)
  })
})()
