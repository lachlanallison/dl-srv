importScripts('shared.js')

const { ext, getSettings, postTask, pingServer, extOf, hostOf } = self.dlsrv

ext.downloads.onCreated.addListener(async (item) => {
  try {
    const settings = await getSettings()
    if (!settings.enabled) return

    const url = item.finalUrl || item.url
    if (!url || url.startsWith('data:') || url.startsWith('blob:')) return

    const host = hostOf(url)
    if (settings.ignoreDomains.includes(host)) return

    const name = item.filename || ''
    const fileExt = extOf(name) || extOf(url)
    if (fileExt && settings.ignoreExt.includes(fileExt)) return
    if (settings.minSize > 0 && item.fileSize > 0 && item.fileSize < settings.minSize) return

    await ext.downloads.cancel(item.id).catch(() => {})
    await ext.downloads.erase({ id: item.id }).catch(() => {})

    const filename = name ? name.split(/[\\/]/).pop() : undefined
    await postTask({
      url,
      referer: item.referrer || undefined,
    })

    ext.notifications.create({
      type: 'basic',
      iconUrl: 'icons/icon128.png',
      title: 'Sent to dl-srv',
      message: filename || url,
    })
  } catch (e) {
    console.error('[dl-srv]', e)
    ext.notifications.create({
      type: 'basic',
      iconUrl: 'icons/icon128.png',
      title: 'dl-srv failed',
      message: String(e.message || e),
      priority: 1,
    })
  }
})

ext.runtime.onInstalled.addListener(() => {
  ext.contextMenus.create({ id: 'dlsrv-link', title: 'Download on NAS (dl-srv)', contexts: ['link'] })
  ext.contextMenus.create({ id: 'dlsrv-page', title: 'Download page on NAS (video)', contexts: ['page'] })
})

ext.contextMenus.onClicked.addListener(async (info) => {
  try {
    const target = info.menuItemId === 'dlsrv-link' ? info.linkUrl : info.pageUrl
    if (!target) return
    await postTask({
      url: target,
      force_ytdlp: info.menuItemId === 'dlsrv-page',
    })
  } catch (e) {
    console.error('[dl-srv]', e)
  }
})

pingServer()
ext.alarms.create('ping', { periodInMinutes: 1 })
ext.alarms.onAlarm.addListener((a) => {
  if (a.name === 'ping') pingServer()
})
