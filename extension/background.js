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
  /** Download IDs we paused for the intercept prompt — never intercept again. */
  const heldDownloadIds = new Set()
  /** URLs currently being released to the browser (handles redirects vs pending.url). */
  const browserReleaseUrls = new Set()
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

  function isNotResumableError(err) {
    const s = String(err?.message || err)
    return /NOT_RESUMABLE|not resumable/i.test(s)
  }

  /** Chrome: `referrer`. Firefox: Referer via `headers` (referrer key is rejected). */
  function browserDownloadOptions(pending, conflictAction) {
    const opts = { url: pending.url, conflictAction }
    if (pending.filename) opts.filename = pending.filename
    if (pending.referer) {
      if (typeof globalThis.browser !== 'undefined') {
        opts.headers = [{ name: 'Referer', value: pending.referer }]
      } else {
        opts.referrer = pending.referer
      }
    }
    return opts
  }

  async function releaseToBrowser(pending, conflictAction = 'uniquify') {
    markBypass(pending.url)
    return ext.downloads.download(browserDownloadOptions(pending, conflictAction))
  }

  async function cancelHeldDownload(pending) {
    if (pending.downloadId == null) return
    await ext.downloads.cancel(pending.downloadId).catch(() => {})
    await ext.downloads.erase({ id: pending.downloadId }).catch(() => {})
  }

  /** Non-resumable HTTP downloads cannot resume after pause — re-fetch instead. */
  async function restartBrowserDownload(pending, conflictAction = 'overwrite') {
    await cancelHeldDownload(pending)
    if (pending.downloadId != null) heldDownloadIds.delete(pending.downloadId)
    const next = { ...pending, downloadId: null }
    const newId = await releaseToBrowser(next, conflictAction)
    if (newId != null) heldDownloadIds.add(newId)
    await waitForDownloadState(newId, ['complete', 'interrupted']).catch(() => {})
    const [done] = await ext.downloads.search({ id: newId }).catch(() => [])
    if (done?.state === 'complete') {
      notify('Download complete', pending.filename || pending.url)
    }
    return newId
  }

  /** Let the user finish in Firefox — resume a held download or start a clean one. */
  async function releasePendingToBrowser(pending) {
    if (pending.url.startsWith('magnet:')) {
      markBypass(pending.url)
      ext.tabs.create({ url: pending.url })
      notify('Opened magnet link', globalThis.dlsrv.magnetLabel(pending.url))
      return
    }

    trackBrowserRelease(pending)
    try {
      notify('Downloading in browser', pending.filename || pending.url)
      markBypass(pending.url)

      if (pending.downloadId == null) {
        const newId = await releaseToBrowser(pending)
        if (newId != null) heldDownloadIds.add(newId)
        await waitForDownloadState(newId, ['complete', 'interrupted']).catch(() => {})
        return
      }

      const [item] = await ext.downloads.search({ id: pending.downloadId }).catch(() => [])
      if (!item) {
        const newId = await releaseToBrowser(pending)
        if (newId != null) heldDownloadIds.add(newId)
        await waitForDownloadState(newId, ['complete', 'interrupted']).catch(() => {})
        return
      }

      heldDownloadIds.add(pending.downloadId)

      if (item.state === 'complete') {
        notify('Download complete', pending.filename || pending.url)
        return
      }

      if (item.state === 'in_progress' && !item.paused) {
        try {
          await ext.downloads.pause(pending.downloadId)
        } catch (e) {
          console.warn('[dl-srv] pause before release failed:', e)
        }
      }

      try {
        await ext.downloads.resume(pending.downloadId)
      } catch (err) {
        console.warn('[dl-srv] resume held download failed:', err)
        const [after] = await ext.downloads.search({ id: pending.downloadId }).catch(() => [])
        if (after?.state === 'complete') {
          notify('Download complete', pending.filename || pending.url)
          return
        }
        if (isNotResumableError(err)) {
          await restartBrowserDownload(pending, 'overwrite')
          return
        }
        await restartBrowserDownload(pending, 'uniquify')
        return
      }

      const finalState = await waitForDownloadState(pending.downloadId, [
        'complete',
        'interrupted',
      ]).catch((e) => {
        console.warn('[dl-srv] waiting for held download:', e)
        return null
      })

      if (finalState === 'complete') {
        notify('Download complete', pending.filename || pending.url)
        return
      }

      if (finalState === 'interrupted') {
        try {
          await ext.downloads.resume(pending.downloadId)
          await waitForDownloadState(pending.downloadId, ['complete', 'interrupted'])
          notify('Download complete', pending.filename || pending.url)
        } catch (e) {
          console.warn('[dl-srv] interrupted download could not finish:', e)
          if (isNotResumableError(e)) {
            await restartBrowserDownload(pending, 'overwrite')
          } else {
            notify('Browser download may be incomplete', pending.filename || pending.url)
          }
        }
      }
    } catch (e) {
      console.error('[dl-srv] browser download', e)
      notify('Browser download failed', String(e.message || e))
    } finally {
      untrackBrowserRelease(pending)
    }
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
    setTimeout(() => bypassUrls.delete(url), 60000)
  }

  function trackBrowserRelease(pending) {
    if (pending.url) browserReleaseUrls.add(pending.url)
    if (pending.downloadId != null) heldDownloadIds.add(pending.downloadId)
  }

  function untrackBrowserRelease(pending) {
    if (pending.url) browserReleaseUrls.delete(pending.url)
    if (pending.downloadId != null) heldDownloadIds.delete(pending.downloadId)
  }

  function shouldSkipInterceptDownload(item) {
    const url = item.finalUrl || item.url
    if (heldDownloadIds.has(item.id)) return true
    if (bypassUrls.has(url) || browserReleaseUrls.has(url)) return true
    if (item.finalUrl && browserReleaseUrls.has(item.finalUrl)) return true
    if (item.url && browserReleaseUrls.has(item.url)) return true
    return false
  }

  function waitForDownloadState(downloadId, states, timeoutMs = 180000) {
    const wanted = new Set(states)
    return new Promise((resolve, reject) => {
      let done = false
      const finish = (state) => {
        if (done) return
        done = true
        clearTimeout(timer)
        ext.downloads.onChanged.removeListener(onChanged)
        resolve(state)
      }
      const fail = (err) => {
        if (done) return
        done = true
        clearTimeout(timer)
        ext.downloads.onChanged.removeListener(onChanged)
        reject(err)
      }
      const onChanged = (delta) => {
        if (delta.id !== downloadId || !delta.state) return
        if (wanted.has(delta.state.current)) finish(delta.state.current)
      }
      const timer = setTimeout(() => fail(new Error('download timed out')), timeoutMs)
      ext.downloads.onChanged.addListener(onChanged)
      ext.downloads.search({ id: downloadId }).then(([item]) => {
        if (item && wanted.has(item.state)) finish(item.state)
      }).catch((e) => fail(e))
    })
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

  async function holdDownload(item) {
    const url = item.finalUrl || item.url
    const name = item.filename || ''
    const filename = name ? name.split(/[\\/]/).pop() : undefined
    const cleaned = cleanFilenameHint(filename || url)
    const base = {
      id: makePendingId(),
      url,
      referer: item.referrer || undefined,
      filename: cleaned || filename,
      createdAt: Date.now(),
    }

    try {
      await ext.downloads.pause(item.id)
      heldDownloadIds.add(item.id)
      return { ...base, downloadId: item.id }
    } catch {
      const [current] = await ext.downloads.search({ id: item.id }).catch(() => [])
      if (!current) {
        return { ...base, downloadId: null }
      }
      if (current.state === 'complete') {
        heldDownloadIds.add(item.id)
        return { ...base, downloadId: item.id }
      }
      if (current.state === 'in_progress' && !current.paused) {
        try {
          await ext.downloads.pause(item.id)
        } catch {
          /* keep downloadId — release path will try again */
        }
      }
      heldDownloadIds.add(item.id)
      return { ...base, downloadId: item.id }
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

        if (shouldSkipInterceptDownload(item)) {
          heldDownloadIds.add(item.id)
          return
        }

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
      await releasePendingToBrowser(pending)
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
            if (pending.downloadId != null) heldDownloadIds.delete(pending.downloadId)
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
        await releasePendingToBrowser(pending)
        finishingPendingId = null
        await showNextPrompt()
      })()
      return true
    }

    if (msg.type === 'dlsrv-intercept-dismiss') {
      sendResponse({ ok: true })
      return false
    }

    if (msg.type === 'dlsrv-magnet-click') {
      watchMagnet(msg.url, null, msg.pageUrl)
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
