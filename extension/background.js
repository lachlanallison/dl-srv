;(function () {
  if (typeof importScripts === 'function' && !globalThis.dlsrv) {
    importScripts('shared.js')
  }
  if (!globalThis.dlsrv) {
    console.error('[dl-srv] shared.js did not load')
    return
  }

  const { ext, getSettings, postTask, pingServer, extOf, hostOf, notify } = globalThis.dlsrv

  if (ext.downloads?.onCreated) {
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
        await postTask({ url, referer: item.referrer || undefined })
        notify('Sent to dl-srv', filename || url)
      } catch (e) {
        console.error('[dl-srv] download intercept', e)
        notify('dl-srv failed', String(e.message || e))
      }
    })
  }

  ext.runtime.onInstalled.addListener(() => {
    ext.contextMenus
      .removeAll()
      .then(() => {
        ext.contextMenus.create({
          id: 'dlsrv-link',
          title: 'Download on NAS (dl-srv)',
          contexts: ['link'],
        })
        ext.contextMenus.create({
          id: 'dlsrv-page',
          title: 'Download page on NAS (video)',
          contexts: ['page'],
        })
      })
      .catch((e) => console.error('[dl-srv] context menus', e))
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
      console.error('[dl-srv] context menu', e)
    }
  })

  ext.alarms.onAlarm.addListener((a) => {
    if (a.name === 'ping') pingServer()
  })

  ext.alarms.create('ping', { periodInMinutes: 1 }).catch((e) => {
    console.warn('[dl-srv] alarm create', e)
  })
  pingServer().catch((e) => console.warn('[dl-srv] initial ping', e))
})()
