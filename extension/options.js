const { DEFAULTS, getSettings, parseCommaList, commaList } = self.dlsrv

async function load() {
  const data = await getSettings()
  document.getElementById('serverUrl').value = data.serverUrl
  document.getElementById('token').value = data.token
  document.getElementById('category').value = data.category
  document.getElementById('enabled').checked = data.enabled
  document.getElementById('autoForceYtdlp').checked = data.autoForceYtdlp
  document.getElementById('minSize').value = data.minSize
  document.getElementById('ignoreExt').value = commaList(data.ignoreExt)
  document.getElementById('ignoreDomains').value = commaList(data.ignoreDomains)
}

document.getElementById('save').addEventListener('click', async () => {
  const ext = self.dlsrv.ext
  await ext.storage.sync.set({
    serverUrl: document.getElementById('serverUrl').value.trim(),
    token: document.getElementById('token').value.trim(),
    category: document.getElementById('category').value.trim() || 'inbox',
    enabled: document.getElementById('enabled').checked,
    autoForceYtdlp: document.getElementById('autoForceYtdlp').checked,
    minSize: parseInt(document.getElementById('minSize').value, 10) || 0,
    ignoreExt: parseCommaList(document.getElementById('ignoreExt').value),
    ignoreDomains: parseCommaList(document.getElementById('ignoreDomains').value),
  })
  document.getElementById('status').textContent = 'Saved.'
})

load()
