;(function () {
  if (typeof importScripts === 'function' && !globalThis.dlsrv) {
    importScripts('shared.js')
  }
  if (!globalThis.dlsrv) {
    console.error('[dl-srv] shared.js did not load')
    return
  }

  const {
    ext,
    getSettings,
    postTask,
    pingServer,
    extOf,
    hostOf,
    notify,
    normalizeServerUrl,
    cleanFilenameHint,
    getPendingQueue,
    setPendingQueue,
    getPendingDownload,
    enqueuePendingDownload,
    setBadge,
  } = globalThis.dlsrv

  const bypassUrls = new Set()
  const browserReleaseIds = new Set()
  const recentMagnets = new Set()
  const tabLastHttpUrl = new Map()
  let promptWindowId = null
  let promptPendingId = null
  let finishingPendingId = null

  function claimBrowserRelease(pendingId) {
    if (browserReleaseIds.has(pendingId)) return false
    browserReleaseIds.add(pendingId)
    setTimeout(() => browserReleaseIds.delete(pendingId), 15000)
    return true
  }

  function releasePendingToBrowser(pending) {
    if (pending.url.startsWith('magnet:')) {
      markBypass(pending.url)
      ext.tabs.create({ url: pending.url })
      notify('Opened magnet link', globalThis.dlsrv.magnetLabel(pending.url))
      return
    }

    notify('Downloading in browser', pending.filename || pending.url)

    if (pending.downloadId == null) {
      releaseToBrowser(pending).catch((e) => {
        console.error('[dl-srv] browser download', e)
        notify('Browser download failed', String(e.message || e))
      })
      return
    }

    ext.downloads
      .search({ id: pending.downloadId })
      .then(([item]) => {
        if (item?.state === 'complete') return
        if (item?.state === 'in_progress' && !item?.paused) return
        return ext.downloads.resume(pending.downloadId).catch((err) => {
          console.warn('[dl-srv] resume rejected:', err)
          if (item?.state === 'in_progress') return
          return releaseToBrowser(pending)
        })
      })
      .catch((e) => {
        console.error('[dl-srv] release search', e)
        releaseToBrowser(pending).catch(() => {})
      })
  }

  function makePendingId() {
    if (crypto.randomUUID) return crypto.randomUUID()
    return `p-${Date.now()}-${Math.random().toString(16).slice(2)}`
  }

  async function removePending(id) {
    const queue = await getPendingQueue()
    await setPendingQueue(queue.filter((p) => p.id !== id))
    return (await getPendingQueue()).length
  }

  async function updatePendingBadge() {
    const n = (await getPendingQueue()).length
    if (n > 0) setBadge(String(n), '#2563eb')
    else setBadge('')
  }

  function markBypass(url) {
    if (!url) return
    bypassUrls.add(url)
    setTimeout(() => bypassUrls.delete(url), 15000)
  }

  async function closePromptForPending(id) {
    if (promptPendingId !== id) return
    promptPendingId = null
    const winId = promptWindowId
    promptWindowId = null
    if (winId != null) {
      try {
        await ext.windows.remove(winId)
      } catch {
        /* already closed */
      }
    }
  }

  function releaseToBrowser(pending) {
    markBypass(pending.url)
    const opts = { url: pending.url, conflictAction: 'uniquify' }
    if (pending.referer) opts.referrer = pending.referer
    if (pending.filename) opts.filename = pending.filename
    return ext.downloads.download(opts)
  }

  async function cancelHeldDownload(pending) {
    if (pending.downloadId == null) return
    await ext.downloads.cancel(pending.downloadId).catch(() => {})
    await ext.downloads.erase({ id: pending.downloadId }).catch(() => {})
  }

  async function holdDownload(item) {
    const url = item.finalUrl || item.url
    const name = item.filename || ''
    const filename = name ? name.split(/[\\/]/).pop() : undefined
    const cleaned = cleanFilenameHint(filename || url)

    try {
      await ext.downloads.pause(item.id)
      return {
        id: makePendingId(),
        downloadId: item.id,
        url,
        referer: item.referrer || undefined,
        filename: cleaned || filename,
        createdAt: Date.now(),
      }
    } catch {
      await ext.downloads.cancel(item.id).catch(() => {})
      await ext.downloads.erase({ id: item.id }).catch(() => {})
      return {
        id: makePendingId(),
        downloadId: null,
        url,
        referer: item.referrer || undefined,
        filename: cleaned || filename,
        createdAt: Date.now(),
      }
    }
  }

  async function openPromptFor(id) {
    promptPendingId = id
    const url = ext.runtime.getURL(`prompt.html?id=${encodeURIComponent(id)}`)
    if (promptWindowId != null) {
      try {
        const tabs = await ext.tabs.query({ windowId: promptWindowId })
        if (tabs[0]?.id) {
          await ext.tabs.update(tabs[0].id, { url })
          await ext.windows.update(promptWindowId, { focused: true })
          return
        }
      } catch {
        promptWindowId = null
      }
    }
    const win = await ext.windows.create({
      url,
      type: 'popup',
      width: 380,
      height: 340,
      focused: true,
    })
    promptWindowId = win.id
  }

  async function queueIntercept(pending, settings) {
    if (!settings.askOnIntercept) {
      await postTask({
        url: pending.url,
        referer: pending.referer,
        filename: pending.filename,
        category: settings.category,
      })
      const label =
        pending.url.startsWith('magnet:') ?
          globalThis.dlsrv.magnetLabel(pending.url)
        : pending.filename || pending.url
      notify('Sent to dl-srv', label)
      return
    }
    await enqueuePendingDownload(pending)
    await updatePendingBadge()
    await openPromptFor(pending.id)
  }

  async function handleMagnetNavigation(url, tabId, referer) {
    if (!url?.startsWith('magnet:') || bypassUrls.has(url)) return
    if (recentMagnets.has(url)) return

    const settings = await getSettings()
    if (!settings.enabled || settings.interceptMagnets === false) return

    recentMagnets.add(url)
    setTimeout(() => recentMagnets.delete(url), 5000)

    if (tabId != null) {
      ext.tabs.goBack(tabId).catch(() => {
        ext.tabs.update(tabId, { url: 'about:blank' }).catch(() => {})
      })
    }

    const pending = {
      id: makePendingId(),
      downloadId: null,
      url,
      referer,
      filename: undefined,
      createdAt: Date.now(),
    }
    await queueIntercept(pending, settings)
  }

  function watchMagnet(url, tabId, referer) {
    handleMagnetNavigation(url, tabId, referer).catch((e) => {
      console.error('[dl-srv] magnet intercept', e)
    })
  }

  if (ext.tabs?.onUpdated) {
    ext.tabs.onUpdated.addListener((tabId, changeInfo) => {
      const url = changeInfo.url
      if (url && !url.startsWith('magnet:')) {
        tabLastHttpUrl.set(tabId, url)
        return
      }
      if (!url?.startsWith('magnet:')) return
      watchMagnet(url, tabId, tabLastHttpUrl.get(tabId))
    })
    ext.tabs.onRemoved.addListener((tabId) => tabLastHttpUrl.delete(tabId))
  }

  if (ext.tabs?.onCreated) {
    ext.tabs.onCreated.addListener((tab) => {
      const url = tab.pendingUrl || tab.url
      if (!url?.startsWith('magnet:') || tab.id == null) return
      watchMagnet(url, tab.id, tabLastHttpUrl.get(tab.openerTabId))
    })
  }

  if (ext.webNavigation?.onCommitted) {
    ext.webNavigation.onCommitted.addListener((details) => {
      if (details.frameId !== 0) return
      if (!details.url?.startsWith('magnet:')) return
      watchMagnet(details.url, details.tabId, tabLastHttpUrl.get(details.tabId))
    })
  }

  async function showNextPrompt() {
    const queue = await getPendingQueue()
    await updatePendingBadge()
    if (!queue.length) {
      promptPendingId = null
      return
    }
    await openPromptFor(queue[0].id)
  }

  async function finishPending(id, handler) {
    const pending = await getPendingDownload(id)
    if (!pending) return { error: 'Download no longer pending' }
    finishingPendingId = id
    try {
      await closePromptForPending(id)
      await handler(pending)
      await removePending(id)
      await showNextPrompt()
      return { ok: true }
    } finally {
      finishingPendingId = null
    }
  }

  if (ext.downloads?.onCreated) {
    ext.downloads.onCreated.addListener(async (item) => {
      try {
        const url = item.finalUrl || item.url
        if (!url) return

        if (bypassUrls.has(url)) return

        const settings = await getSettings()
        if (!settings.enabled) return

        if (url.startsWith('data:') || url.startsWith('blob:')) return

        if (url.startsWith('magnet:')) {
          await ext.downloads.cancel(item.id).catch(() => {})
          await ext.downloads.erase({ id: item.id }).catch(() => {})
          if (settings.interceptMagnets !== false) {
            const pending = {
              id: makePendingId(),
              downloadId: null,
              url,
              referer: item.referrer || undefined,
              filename: undefined,
              createdAt: Date.now(),
            }
            await queueIntercept(pending, settings)
          }
          return
        }

        const host = hostOf(url)
        if (settings.ignoreDomains.includes(host)) return

        const name = item.filename || ''
        const fileExt = extOf(name) || extOf(url)
        if (fileExt && settings.ignoreExt.includes(fileExt)) return
        if (settings.minSize > 0 && item.fileSize > 0 && item.fileSize < settings.minSize) return

        const filename = name ? name.split(/[\\/]/).pop() : undefined
        const cleaned = cleanFilenameHint(filename || url)

        if (!settings.askOnIntercept) {
          await ext.downloads.cancel(item.id).catch(() => {})
          await ext.downloads.erase({ id: item.id }).catch(() => {})
          await postTask({
            url,
            referer: item.referrer || undefined,
            filename: cleaned,
          })
          notify('Sent to dl-srv', cleaned || filename || url)
          return
        }

        const pending = await holdDownload(item)
        await queueIntercept(pending, settings)
      } catch (e) {
        console.error('[dl-srv] download intercept', e)
        const { debugLog } = globalThis.dlsrv
        if (debugLog) {
          await debugLog('error', 'Download intercept failed', {
            url: item.finalUrl || item.url,
            error: String(e.message || e),
          })
        }
        notify('dl-srv failed', String(e.message || e))
      }
    })
  }

  if (ext.windows?.onRemoved) {
    ext.windows.onRemoved.addListener(async (windowId) => {
      if (windowId !== promptWindowId) return
      promptWindowId = null
      const id = promptPendingId
      promptPendingId = null
      if (!id || id === finishingPendingId) return
      if (!claimBrowserRelease(id)) return
      const pending = await getPendingDownload(id)
      if (!pending) return
      await removePending(id)
      releasePendingToBrowser(pending)
      await showNextPrompt()
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
      const referer = info.pageUrl || undefined
      await postTask({
        url: target,
        referer,
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

  ext.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
    if (msg.type === 'dlsrv-ping') {
      ;(async () => {
        sendResponse(await pingServer())
      })()
      return true
    }

    if (msg.type === 'dlsrv-intercept-send') {
      ;(async () => {
        sendResponse(
          await finishPending(msg.id, async (pending) => {
            await cancelHeldDownload(pending)
            await postTask({
              url: pending.url,
              referer: pending.referer,
              filename: pending.filename,
              category: msg.category,
            })
            notify('Sent to dl-srv', `${msg.category}/ — ${pending.filename || pending.url}`)
          }),
        )
      })()
      return true
    }

    if (msg.type === 'dlsrv-intercept-browser') {
      ;(async () => {
        const pending = await getPendingDownload(msg.id)
        if (!pending) {
          sendResponse({ error: 'Download no longer pending' })
          return
        }
        if (!claimBrowserRelease(msg.id)) {
          sendResponse({ ok: true })
          return
        }
        finishingPendingId = msg.id
        await closePromptForPending(msg.id)
        await removePending(msg.id)
        sendResponse({ ok: true })
        releasePendingToBrowser(pending)
        finishingPendingId = null
        await showNextPrompt()
      })()
      return true
    }

    if (msg.type === 'dlsrv-intercept-dismiss') {
      sendResponse({ ok: true })
      return false
    }

    if (msg.type !== 'dlsrv-fetch') return undefined
    ;(async () => {
      try {
        const settings = await getSettings()
        const base = normalizeServerUrl(msg.serverUrl || settings.serverUrl)
        if (!base) {
          sendResponse({ error: 'Set server URL in extension options' })
          return
        }
        const url = `${base}/api/v1${msg.path}`
        console.log('[dl-srv] background fetch', url)
        const res = await fetch(url, {
          method: msg.method || 'GET',
          headers: msg.headers || {},
          body: msg.body,
        })
        const text = await res.text()
        sendResponse({ ok: res.ok, status: res.status, url, text })
      } catch (e) {
        console.error('[dl-srv] background fetch failed', e)
        sendResponse({ error: String(e.message || e) })
      }
    })()
    return true
  })
})()
