;(function () {
  if (typeof importScripts === 'function' && !globalThis.dlsrv) {
    importScripts('shared.js')
  }
  if (!globalThis.dlsrv) {
    console.error('[dl-srv] shared.js did not load')
    return
  }
  globalThis.dlsrvInBackground = true

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
    shouldInterceptExt,
    debugLog,
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

  function log(level, message, detail) {
    console[level === 'error' ? 'error' : 'log']('[dl-srv]', message, detail ?? '')
    return debugLog(level, message, detail).catch(() => {})
  }

  function snapshotDownload(item) {
    if (!item) return null
    return {
      id: item.id,
      state: item.state,
      paused: item.paused,
      url: item.url,
      finalUrl: item.finalUrl,
      filename: item.filename,
      bytesReceived: item.bytesReceived,
      totalBytes: item.totalBytes,
      fileSize: item.fileSize,
      exists: item.exists,
      error: item.error,
      mime: item.mime,
      canResume: item.canResume,
    }
  }

  function claimBrowserRelease(pendingId) {
    if (browserReleaseIds.has(pendingId)) return false
    browserReleaseIds.add(pendingId)
    setTimeout(() => browserReleaseIds.delete(pendingId), 15000)
    return true
  }

  function downloadHasBytes(item) {
    if (!item) return false
    const received = Number(item.bytesReceived)
    if (Number.isFinite(received) && received > 0) return true
    const size = Number(item.fileSize)
    if (Number.isFinite(size) && size > 0) return true
    const total = Number(item.totalBytes)
    if (Number.isFinite(total) && total > 0) return true
    return false
  }

  function downloadTotalBytes(item) {
    const total = Number(item?.totalBytes)
    if (Number.isFinite(total) && total > 0) return total
    return 0
  }

  function downloadFullyReceived(item) {
    const total = downloadTotalBytes(item)
    const received = Number(item?.bytesReceived) || 0
    return total > 0 && received >= total
  }

  function downloadNearlyDone(item) {
    const total = downloadTotalBytes(item)
    if (!total) return false
    const received = Number(item.bytesReceived) || 0
    return total - received <= 65536 || received / total >= 0.95
  }

  /** No Content-Length yet and not much data — typical generated/session PDF. Don't pause. */
  function downloadUnknownAndSmall(item) {
    if (downloadTotalBytes(item)) return false
    return (Number(item.bytesReceived) || 0) < 1024 * 1024
  }

  function leaveDownloadInBrowser(item) {
    return downloadFullyReceived(item) || downloadNearlyDone(item) || downloadUnknownAndSmall(item)
  }

  async function releasePendingToBrowser(pending) {
    if (pending.url.startsWith('magnet:')) {
      markBypass(pending.url)
      ext.tabs.create({ url: pending.url })
      notify('Opened magnet link', globalThis.dlsrv.magnetLabel(pending.url))
      return
    }

    await log('info', 'releasing download to browser', {
      url: pending.url,
      referer: pending.referer || null,
      filename: pending.filename || null,
      heldId: pending.downloadId ?? null,
    })

    if (pending.downloadId == null) {
      await log('warn', 'no original download id — not starting a new GET')
      notify('Original download was lost — click the link again', pending.filename || pending.url)
      return
    }

    heldDownloadIds.add(pending.downloadId)
    const [item] = await ext.downloads.search({ id: pending.downloadId }).catch(() => [])
    await log('info', 'original download at browser-release', snapshotDownload(item))

    if (!item) {
      await log('error', 'original download vanished — not re-fetching (signed URLs return 403)')
      notify('Original download was lost — click the link again', pending.filename || pending.url)
      return
    }

    if (item.state === 'complete') {
      if (downloadHasBytes(item)) {
        notify('Download complete', pending.filename || item.filename || pending.url)
      } else {
        notify('Download was empty — click the link again', pending.filename || pending.url)
      }
      return
    }

    if (item.state === 'interrupted' || item.paused) {
      try {
        await ext.downloads.resume(pending.downloadId)
        await log('info', 'resumed original download', { id: pending.downloadId })
      } catch (e) {
        await log('error', 'resume original failed', {
          error: String(e.message || e),
          item: snapshotDownload(item),
        })
        if (downloadHasBytes(item)) {
          notify('Downloading in browser', pending.filename || pending.url)
        } else {
          notify('Could not resume download — click the link again', pending.filename || pending.url)
        }
        return
      }
    }

    notify('Downloading in browser', pending.filename || pending.url)
  }

  async function cancelHeldDownload(pending) {
    if (pending.downloadId == null) return
    const id = pending.downloadId
    await ext.downloads.cancel(id).catch(() => {})
    await ext.downloads.removeFile(id).catch(() => {})
    await ext.downloads.erase({ id }).catch(() => {})
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

  function shouldSkipInterceptDownload(item) {
    const url = item.finalUrl || item.url
    if (heldDownloadIds.has(item.id)) return 'held-id'
    if (bypassUrls.has(url) || (item.url && bypassUrls.has(item.url)) || (item.finalUrl && bypassUrls.has(item.finalUrl))) {
      return 'bypass-url'
    }
    if (browserReleaseUrls.has(url) || (item.finalUrl && browserReleaseUrls.has(item.finalUrl)) || (item.url && browserReleaseUrls.has(item.url))) {
      return 'release-url'
    }
    return null
  }

  const watchingIntercepts = new Map()
  const pausingIds = new Set()
  const WATCH_MS = 10 * 60 * 1000
  const POLL_MS = 200

  function stopWatching(downloadId) {
    const meta = watchingIntercepts.get(downloadId)
    if (meta?.timer) clearTimeout(meta.timer)
    if (meta?.poll) clearInterval(meta.poll)
    watchingIntercepts.delete(downloadId)
    pausingIds.delete(downloadId)
  }

  async function promptPausedDownload(item, meta) {
    if (meta.prompted) return
    meta.prompted = true
    const pending = {
      id: makePendingId(),
      downloadId: item.id,
      url: meta.url,
      referer: meta.referer,
      filename: meta.filename,
      createdAt: Date.now(),
    }
    stopWatching(item.id)
    heldDownloadIds.add(item.id)
    await log('info', 'paused resumable download for intercept prompt', snapshotDownload(item))
    const settings = await getSettings()
    await queueIntercept(pending, settings)
  }

  async function decideWatchedDownload(item) {
    const meta = watchingIntercepts.get(item.id)
    if (!meta) return

    const received = Number(item.bytesReceived) || 0
    const pausing = pausingIds.has(item.id) || meta.wePaused

    if (item.state === 'complete') {
      await log('info', 'skip intercept: completed before a safe pause', snapshotDownload(item))
      stopWatching(item.id)
      return
    }

    if (item.state === 'interrupted') {
      const looksPaused = received > 0 && (pausing || item.paused || item.canResume)
      if (looksPaused) {
        if (leaveDownloadInBrowser(item)) {
          await log('info', 'skip intercept: pause hit a small or finished download — resuming', snapshotDownload(item))
          stopWatching(item.id)
          await ext.downloads.resume(item.id).catch(() => {})
          return
        }
        await log('info', 'Firefox reported pause as interrupted — still prompting', snapshotDownload(item))
        await promptPausedDownload(item, meta)
        return
      }
      await log('info', 'skip intercept: finished before a safe pause', snapshotDownload(item))
      stopWatching(item.id)
      return
    }

    if (received <= 0) return
    if (pausing) return
    if (downloadNearlyDone(item)) {
      await log('info', 'skip intercept: already nearly complete', snapshotDownload(item))
      stopWatching(item.id)
      return
    }
    if (downloadUnknownAndSmall(item)) return

    pausingIds.add(item.id)
    meta.wePaused = true
    await log('info', 'first bytes received — trying pause', snapshotDownload(item))
    try {
      await ext.downloads.pause(item.id)
    } catch (e) {
      await log('info', 'skip intercept: pause failed (not resumable)', {
        error: String(e.message || e),
        item: snapshotDownload(item),
      })
      stopWatching(item.id)
      return
    }

    const [paused] = await ext.downloads.search({ id: item.id }).catch(() => [])
    const current = paused || item
    if (leaveDownloadInBrowser(current)) {
      await log('info', 'skip intercept: pause hit a small or finished download — resuming', snapshotDownload(current))
      stopWatching(item.id)
      await ext.downloads.resume(item.id).catch(() => {})
      return
    }
    await promptPausedDownload(current, meta)
  }

  function watchForResumableIntercept(item, filename) {
    heldDownloadIds.add(item.id)
    const meta = {
      url: item.finalUrl || item.url,
      referer: item.referrer || undefined,
      filename,
      prompted: false,
      wePaused: false,
      timer: null,
      poll: null,
    }
    meta.timer = setTimeout(() => {
      if (!watchingIntercepts.has(item.id)) return
      log('info', 'skip intercept: timed out waiting for first byte', { id: item.id })
      stopWatching(item.id)
    }, WATCH_MS)
    meta.poll = setInterval(() => {
      if (!watchingIntercepts.has(item.id)) return
      ext.downloads
        .search({ id: item.id })
        .then(([current]) => current && decideWatchedDownload(current))
        .catch((e) => log('error', 'watch poll failed', String(e.message || e)))
    }, POLL_MS)
    watchingIntercepts.set(item.id, meta)
    log('info', 'watching download until first byte — will pause only if resumable', snapshotDownload(item))
    decideWatchedDownload(item).catch((e) => log('error', 'watch intercept failed', String(e.message || e)))
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
        if (delta.id !== downloadId) return
        log('info', 'download changed', {
          id: downloadId,
          state: delta.state?.current,
          bytes: delta.bytesReceived?.current,
          error: delta.error?.current,
          exists: delta.exists?.current,
          filename: delta.filename?.current,
        })
        if (delta.state && wanted.has(delta.state.current)) finish(delta.state.current)
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
      height: 390,
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
      notify('Sent to NAS', label)
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
    finishingPendingId = id
    try {
      const pending = await getPendingDownload(id)
      if (!pending) return { error: 'Download no longer pending' }
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

        const skipReason = shouldSkipInterceptDownload(item)
        if (skipReason) {
          heldDownloadIds.add(item.id)
          await log('info', 'skip intercept', { reason: skipReason, item: snapshotDownload(item) })
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
        if (settings.ignoreDomains.includes(host)) {
          await log('info', 'skip intercept: ignored domain', { host, url })
          return
        }

        const name = item.filename || ''
        const fileExt = extOf(name) || extOf(url)
        if (!shouldInterceptExt(fileExt, settings)) {
          await log('info', 'skip intercept: file type', { ext: fileExt || null, url })
          return
        }
        if (settings.minSize > 0 && item.fileSize > 0 && item.fileSize < settings.minSize) {
          await log('info', 'skip intercept: min size', { fileSize: item.fileSize, minSize: settings.minSize })
          return
        }

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
          notify('Sent to NAS', cleaned || filename || url)
          return
        }

        await log('info', 'intercept candidate', {
          ext: fileExt || null,
          item: snapshotDownload(item),
        })
        watchForResumableIntercept(item, cleaned || filename)
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

  if (ext.downloads?.onChanged) {
    ext.downloads.onChanged.addListener((delta) => {
      if (!watchingIntercepts.has(delta.id)) return
      ext.downloads
        .search({ id: delta.id })
        .then(([item]) => item && decideWatchedDownload(item))
        .catch((e) => log('error', 'watch onChanged failed', String(e.message || e)))
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
          title: 'Send this page URL to NAS',
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
        force_ytdlp: false,
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
  if (ext.action?.setIcon) {
    ext.action
      .setIcon({ path: { 16: 'icons/icon16.png', 32: 'icons/icon32.png' } })
      .catch((e) => console.warn('[dl-srv] setIcon', e))
  }

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
            notify('Sent to NAS', `${msg.category}/ — ${pending.filename || pending.url}`)
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

    if (msg.type === 'dlsrv-intercept-cancel') {
      ;(async () => {
        sendResponse(
          await finishPending(msg.id, async (pending) => {
            await cancelHeldDownload(pending)
            if (pending.downloadId != null) heldDownloadIds.delete(pending.downloadId)
          }),
        )
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
