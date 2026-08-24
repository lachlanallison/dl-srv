<script>

  import { onMount, onDestroy } from 'svelte'

  import {

    api,

    formatBytes,

    formatSpeed,

    isTorrentUrl,

    isMetadataPhase,

    formatShareRatio,

    WEB_VERSION,

    getToken,

    setToken,

    parseError,

    QUALITY_PRESETS,

  } from './lib/api.js'



  let tab = 'queue'

  let token = getToken()

  let tasks = []

  let health = null

  let settings = null

  let feeds = []

  let error = ''

  let errorDetail = ''

  let queueFilter = 'all'

  let queueLoading = false

  let addingDownload = false



  // Setup wizard

  let showSetup = false

  let setupToken = ''

  let setupSaving = false

  let copied = false



  // Add form

  let url = ''

  let referer = ''

  let category = 'inbox'

  let forceYtdlp = false

  let quality = 'best'



  // Settings form

  let settingsForm = {}

  let settingsSaving = false

  let regenerating = false



  // RSS form

  let rssForm = { url: '', category: 'inbox', filter_regex: '', poll_interval_secs: 3600 }

  let rssSaving = false



  let updatingYtdlp = false

  let unsubscribe = null

  let queuePoll = null

  const QUEUE_POLL_MS = 4000



  function taskMatchesFilter(task, filter) {

    if (filter === 'all') return true

    if (filter === 'active') return ['pending', 'downloading', 'paused'].includes(task.status) || task.seeding

    return task.status === filter

  }



  function setError(e) {

    if (!e) {

      error = ''

      errorDetail = ''

      return

    }

    error = e.message || String(e)

    errorDetail = e.status ? `HTTP ${e.status}` : ''

  }



  async function loadSetup() {

    try {

      const setup = await api.setup()

      if (!setup.setup_complete) {

        showSetup = true

        setupToken = setup.token || ''

        if (setupToken) {

          token = setupToken

          setToken(setupToken)

        }

        return true

      }

      showSetup = false

      return false

    } catch (e) {

      setError(e)

      return false

    }

  }



  async function refreshTasks() {

    queueLoading = true

    try {

      tasks = await api.listTasks(queueFilter)

    } finally {

      queueLoading = false

    }

  }



  async function refresh() {

    setError('')

    try {

      await refreshTasks()

      health = await api.health()

      settings = await api.settings()

      syncSettingsForm()

      if (tab === 'rss') feeds = await api.listRssFeeds()

    } catch (e) {

      setError(e)

    }

  }



  function syncSettingsForm() {

    if (!settings) return

    settingsForm = {

      default_category: settings.default_category || 'inbox',

      ytdlp_quality: settings.ytdlp_quality || 'best',

      ytdlp_cookies_file: settings.ytdlp_cookies_file || '',

      webhook_url: settings.webhook_url || '',

      webhook_enabled: !!settings.webhook_enabled,

      jellyfin_refresh_url: settings.jellyfin_refresh_url || '',

      bt_seed_ratio: settings.bt_seed_ratio ?? 0,

      bt_seed_time: settings.bt_seed_time ?? 0,

      bt_max_peers: settings.bt_max_peers ?? 55,

      max_upload_kbps: settings.max_upload_kbps ?? 0,

      max_download_kbps: settings.max_download_kbps ?? 0,

    }

    if (!category || category === 'inbox') category = settingsForm.default_category

    quality = settings.ytdlp_quality || 'best'

  }



  let sseFlush = null

  function flushSseUpdates(pending) {

    const idx = tasks.findIndex((t) => t.id === pending.id)

    if (taskMatchesFilter(pending, queueFilter)) {

      if (idx >= 0) tasks[idx] = pending

      else tasks = [pending, ...tasks]

    } else if (idx >= 0) {

      tasks = tasks.filter((t) => t.id !== pending.id)

    }

    tasks = [...tasks]

  }



  async function connectEvents() {

    if (unsubscribe) unsubscribe()

    if (!getToken()) return

    const pending = new Map()

    unsubscribe = api.events((task) => {

      pending.set(task.id, task)

      if (sseFlush) return

      sseFlush = setTimeout(() => {

        sseFlush = null

        for (const t of pending.values()) flushSseUpdates(t)

        pending.clear()

      }, 250)

    })

  }



  function startQueuePoll() {

    stopQueuePoll()

    queuePoll = setInterval(() => {

      if (tab === 'queue' && getToken() && !showSetup) {

        refreshTasks().catch(() => {})

      }

    }, QUEUE_POLL_MS)

  }



  function stopQueuePoll() {

    if (queuePoll) {

      clearInterval(queuePoll)

      queuePoll = null

    }

  }



  async function saveToken() {

    setToken(token)

    await refresh()

    await connectEvents()

    startQueuePoll()

  }



  async function completeSetup() {

    setupSaving = true

    setError('')

    try {

      setToken(setupToken || token)

      token = getToken()

      await api.completeSetup()

      showSetup = false

      await saveToken()

    } catch (e) {

      setError(e)

    } finally {

      setupSaving = false

    }

  }



  async function copyToken() {

    const t = setupToken || token

    if (!t) {

      setError(new Error('No token to copy'))

      return

    }

    try {

      await navigator.clipboard.writeText(t)

    } catch {

      const ta = document.createElement('textarea')

      ta.value = t

      ta.style.position = 'fixed'

      ta.style.left = '-9999px'

      document.body.appendChild(ta)

      ta.select()

      document.execCommand('copy')

      document.body.removeChild(ta)

    }

    copied = true

    setTimeout(() => (copied = false), 2000)

  }



  async function addDownload() {

    if (!url.trim() || addingDownload) return

    setError('')

    addingDownload = true

    const payload = {

      url: url.trim(),

      referer: referer.trim() || undefined,

      category: category || settingsForm.default_category || 'inbox',

      force_ytdlp: forceYtdlp,

      quality: quality || undefined,

    }

    url = ''

    referer = ''

    tab = 'queue'

    try {

      const task = await api.createTask(payload)

      if (taskMatchesFilter(task, queueFilter)) tasks = [task, ...tasks]

    } catch (e) {

      setError(e)

    } finally {

      addingDownload = false

    }

  }



  async function saveSettings() {

    settingsSaving = true

    setError('')

    try {

      settings = await api.updateSettings({

        default_category: settingsForm.default_category,

        ytdlp_quality: settingsForm.ytdlp_quality,

        ytdlp_cookies_file: settingsForm.ytdlp_cookies_file || null,

        webhook_url: settingsForm.webhook_url || null,

        webhook_enabled: settingsForm.webhook_enabled,

        jellyfin_refresh_url: settingsForm.jellyfin_refresh_url || null,

        bt_seed_ratio: Number(settingsForm.bt_seed_ratio),

        bt_seed_time: Number(settingsForm.bt_seed_time),

        bt_max_peers: Number(settingsForm.bt_max_peers),

        max_upload_kbps: Number(settingsForm.max_upload_kbps),

        max_download_kbps: Number(settingsForm.max_download_kbps),

      })

      syncSettingsForm()

    } catch (e) {

      setError(e)

    } finally {

      settingsSaving = false

    }

  }



  async function regenerateToken() {

    if (!confirm('Regenerate API token? Update the browser extension and any clients.')) return

    regenerating = true

    setError('')

    try {

      const res = await api.regenerateToken()

      token = res.token

      setToken(token)

    } catch (e) {

      setError(e)

    } finally {

      regenerating = false

    }

  }



  async function addRssFeed() {

    if (!rssForm.url.trim()) return

    rssSaving = true

    setError('')

    try {

      await api.createRssFeed({

        url: rssForm.url.trim(),

        category: rssForm.category || 'inbox',

        filter_regex: rssForm.filter_regex || null,

        poll_interval_secs: Number(rssForm.poll_interval_secs) || 3600,

      })

      rssForm = { url: '', category: settingsForm.default_category || 'inbox', filter_regex: '', poll_interval_secs: 3600 }

      feeds = await api.listRssFeeds()

    } catch (e) {

      setError(e)

    } finally {

      rssSaving = false

    }

  }



  async function deleteFeed(id) {

    setError('')

    try {

      await api.deleteRssFeed(id)

      feeds = feeds.filter((f) => f.id !== id)

    } catch (e) {

      setError(e)

    }

  }



  async function changeQueueFilter(f) {

    queueFilter = f

    setError('')

    try {

      await refreshTasks()

    } catch (e) {

      setError(e)

    }

  }



  async function runUpdateYtdlp() {

    updatingYtdlp = true

    setError('')

    try {

      await api.updateYtdlp()

      health = await api.health()

    } catch (e) {

      setError(e)

    } finally {

      updatingYtdlp = false

    }

  }



  async function switchTab(t) {

    tab = t

    if (t === 'rss') {

      try {

        feeds = await api.listRssFeeds()

      } catch (e) {

        setError(e)

      }

    }

  }



  onMount(async () => {

    const inSetup = await loadSetup()

    if (!inSetup && token) await saveToken()

    else if (!inSetup) await refresh()

    startQueuePoll()

  })



  onDestroy(() => {

    if (unsubscribe) unsubscribe()

    stopQueuePoll()

  })

</script>



<div class="layout">

  <header>

    <div>

      <h1>dl-srv</h1>

      <div class="muted">

        Remote download inbox

        · UI {WEB_VERSION}

        {#if health?.dlsrv_version}

          · server {health.dlsrv_version}

        {/if}

      </div>

    </div>

    {#if !showSetup}

      <div class="row header-auth">

        <input style="width: 220px" bind:value={token} placeholder="API token" />

        <button class="primary" onclick={saveToken}>Connect</button>

      </div>

    {/if}

  </header>



  {#if error}

    <div class="alert err banner" role="alert">

      <div class="banner-row">

        <strong>Error</strong>

        {#if errorDetail}<span class="badge err-code">{errorDetail}</span>{/if}

        <button class="banner-dismiss" onclick={() => setError('')} aria-label="Dismiss">×</button>

      </div>

      <div class="banner-msg">{error}</div>

    </div>

  {/if}



  {#if showSetup}

    <div class="panel setup-wizard">

      <h2>Welcome — first-time setup</h2>

      <p class="muted">Save this API token. You'll need it for the browser extension and API clients.</p>



      <div class="token-box">

        <code class="mono">{setupToken}</code>

        <button onclick={copyToken}>{copied ? 'Copied!' : 'Copy token'}</button>

      </div>



      <div class="setup-steps">

        <h3>Browser extension</h3>

        <ol>

          <li>Firefox: open <code>about:debugging</code> → This Firefox → Load Temporary Add-on → pick <code>manifest.json</code> in the <code>extension/</code> folder</li>

          <li>Chrome: copy <code>manifest.chromium.json</code> over <code>manifest.json</code>, then load unpacked from <code>extension/</code></li>

          <li>Open extension options → paste the server URL and API token above</li>

          <li>Set a default category (e.g. <code>movies</code>, <code>tv</code>) to match your library folders</li>

        </ol>

      </div>



      <div class="row" style="margin-top: 1rem">

        <button class="primary" disabled={setupSaving} onclick={completeSetup}>

          {setupSaving ? 'Saving…' : "I've saved the token — continue"}

        </button>

      </div>

    </div>

  {:else}

    <div class="tabs">

      <button class:active={tab === 'queue'} onclick={() => switchTab('queue')}>Queue</button>

      <button class:active={tab === 'add'} onclick={() => switchTab('add')}>Add</button>

      <button class:active={tab === 'settings'} onclick={() => switchTab('settings')}>Settings</button>

      <button class:active={tab === 'rss'} onclick={() => switchTab('rss')}>RSS</button>

      <button class:active={tab === 'health'} onclick={() => switchTab('health')}>Health</button>

    </div>



    {#if tab === 'queue'}

      <div class="panel">

        <div class="row toolbar">

          <div class="filter-group">

            {#each ['all', 'active', 'completed', 'failed'] as f}

              <button class:active={queueFilter === f} onclick={() => changeQueueFilter(f)}>{f}</button>

            {/each}

          </div>

          <button onclick={refresh} disabled={queueLoading}>{queueLoading ? 'Loading…' : 'Refresh'}</button>

        </div>

        {#if tasks.length === 0}

          <p class="muted">No downloads{queueFilter !== 'all' ? ` (${queueFilter})` : ''}.</p>

        {:else}

          <div class="task-list">

            {#each tasks as task (task.id)}

              <div class="task">

                <div class="task-head">

                  <div>

                    <div class="mono">{task.filename || task.url}</div>

                    <div class="muted">{task.type} · {task.category}{task.quality ? ` · ${task.quality}` : ''}</div>

                  </div>

                  <span class="badge {task.seeding ? 'seeding' : task.status}">{task.seeding ? 'seeding' : task.status}</span>

                </div>

                <div class="progress"><span style="width: {Math.min(task.progress, 100)}%"></span></div>

                <div class="muted">

                  {formatBytes(task.done_bytes)} / {formatBytes(task.total_bytes)}

                  {#if isTorrentUrl(task.url)}

                    · ↓ {formatSpeed(task.speed)} · ↑ {formatSpeed(task.upload_speed)}

                    · {task.connections ?? 0} {task.seeding ? 'peer(s)' : 'peers'}

                    {#if !task.seeding && !isMetadataPhase(task)}

                      · {task.num_seeders ?? 0} seeders

                    {/if}

                    · ratio {formatShareRatio(task.uploaded_bytes ?? 0, task.done_bytes)}

                  {:else}

                    · {formatSpeed(task.speed)}

                  {/if}

                </div>

                {#if task.seeding}

                  <div class="muted">Uploading to leechers in aria2 · seeders shows 0 while you are seeding (aria2 only counts seeders when downloading)</div>

                {:else if task.status === 'completed' && isTorrentUrl(task.url)}

                  <div class="muted">Download complete · not seeding (ratio/time met or torrent removed from aria2)</div>

                {/if}

                {#if isMetadataPhase(task)}

                  <div class="muted">Fetching torrent metadata via DHT… (can take a few minutes; peers often show 0 until this finishes)</div>

                {/if}

                {#if task.error}

                  <div class="muted err-text">{task.error}</div>

                {/if}

                <div class="row" style="margin-top: 0.5rem">

                  {#if task.status === 'downloading' || task.status === 'pending'}

                    <button onclick={() => api.pauseTask(task.id).then(refresh)}>Pause</button>

                  {/if}

                  {#if task.status === 'paused' || task.status === 'failed'}

                    <button onclick={() => api.resumeTask(task.id).then(refresh)}>Resume</button>

                  {/if}

                  <button class="danger" onclick={() => api.removeTask(task.id).then(refresh)}>Remove</button>

                </div>

              </div>

            {/each}

          </div>

        {/if}

      </div>



    {:else if tab === 'add'}

      <div class="panel grid">

        <label>

          <div class="muted">URL, magnet, or video page</div>

          <input bind:value={url} placeholder="https://..." />

        </label>

        <label>

          <div class="muted">Referer (optional — file host page URL)</div>

          <input bind:value={referer} placeholder="https://akirabox.to/f/..." />

        </label>

        <label>

          <div class="muted">Category folder</div>

          <input bind:value={category} placeholder="inbox" />

        </label>

        <label>

          <div class="muted">Quality preset</div>

          <select bind:value={quality}>

            {#each QUALITY_PRESETS as p}

              <option value={p.value}>{p.label}</option>

            {/each}

          </select>

        </label>

        <label class="row check-row">

          <input type="checkbox" bind:checked={forceYtdlp} />

          <span>Force yt-dlp (video sites)</span>

        </label>

        <button class="primary" disabled={addingDownload} onclick={addDownload}>{addingDownload ? 'Adding…' : 'Start download'}</button>

      </div>



    {:else if tab === 'settings'}

      <div class="panel grid">

        <label>

          <div class="muted">Default category</div>

          <input bind:value={settingsForm.default_category} />

        </label>

        <h3>BitTorrent</h3>

        <label>

          <div class="muted">Seed ratio (0 = no ratio limit)</div>

          <input type="number" min="0" step="0.1" bind:value={settingsForm.bt_seed_ratio} />

        </label>

        <label>

          <div class="muted">Seed time after complete (minutes, 0 = until ratio is met)</div>

          <input type="number" min="0" step="1" bind:value={settingsForm.bt_seed_time} />

        </label>

        <label>

          <div class="muted">Max peers per torrent</div>

          <input type="number" min="1" max="1000" step="1" bind:value={settingsForm.bt_max_peers} />

        </label>

        <label>

          <div class="muted">Max upload speed (KiB/s, 0 = unlimited)</div>

          <input type="number" min="0" step="1" bind:value={settingsForm.max_upload_kbps} />

        </label>

        <label>

          <div class="muted">Max download speed (KiB/s, 0 = unlimited)</div>

          <input type="number" min="0" step="1" bind:value={settingsForm.max_download_kbps} />

        </label>

        <label>

          <div class="muted">yt-dlp quality preset</div>

          <select bind:value={settingsForm.ytdlp_quality}>

            {#each QUALITY_PRESETS as p}

              <option value={p.value}>{p.label}</option>

            {/each}

          </select>

        </label>

        <label>

          <div class="muted">Cookies file path (Netscape format)</div>

          <input bind:value={settingsForm.ytdlp_cookies_file} placeholder="/config/cookies.txt" />

        </label>

        <label>

          <div class="muted">Webhook URL</div>

          <input bind:value={settingsForm.webhook_url} placeholder="https://..." />

        </label>

        <label class="row check-row">

          <input type="checkbox" bind:checked={settingsForm.webhook_enabled} />

          <span>Webhook enabled</span>

        </label>

        <label>

          <div class="muted">Jellyfin library refresh URL</div>

          <input bind:value={settingsForm.jellyfin_refresh_url} placeholder="http://jellyfin:8096/Library/Refresh" />

        </label>

        <div class="row">

          <button class="primary" disabled={settingsSaving} onclick={saveSettings}>

            {settingsSaving ? 'Saving…' : 'Save settings'}

          </button>

          <button disabled={regenerating} onclick={regenerateToken}>

            {regenerating ? 'Regenerating…' : 'Regenerate token'}

          </button>

        </div>

        {#if settings?.cors_origins}

          <div class="readonly-block">

            <div class="muted">CORS origins</div>

            {#if settings.cors_origins.length === 0}

              <div class="mono muted">Any (default)</div>

            {:else}

              <ul class="origin-list">

                {#each settings.cors_origins as origin}

                  <li class="mono">{origin}</li>

                {/each}

              </ul>

            {/if}

          </div>

        {/if}

        {#if health?.dlsrv_version}

          <p class="muted" style="margin: 0">Running dl-srv {health.dlsrv_version}</p>

        {/if}

      </div>



    {:else if tab === 'rss'}

      <div class="panel grid">

        <h3 style="margin: 0">Add feed</h3>

        <label>

          <div class="muted">Feed URL</div>

          <input bind:value={rssForm.url} placeholder="https://example.com/feed.xml" />

        </label>

        <div class="row">

          <label style="flex: 1">

            <div class="muted">Category</div>

            <input bind:value={rssForm.category} />

          </label>

          <label style="flex: 1">

            <div class="muted">Poll interval (seconds)</div>

            <input type="number" bind:value={rssForm.poll_interval_secs} min="300" />

          </label>

        </div>

        <label>

          <div class="muted">Filter regex (optional)</div>

          <input bind:value={rssForm.filter_regex} placeholder="\.mkv$|1080p" />

        </label>

        <button class="primary" disabled={rssSaving} onclick={addRssFeed}>

          {rssSaving ? 'Adding…' : 'Add feed'}

        </button>



        <h3>Feeds</h3>

        {#if feeds.length === 0}

          <p class="muted">No RSS feeds configured.</p>

        {:else}

          <div class="feed-list">

            {#each feeds as feed (feed.id)}

              <div class="feed-item">

                <div class="mono">{feed.url}</div>

                <div class="muted">

                  {feed.category} · every {feed.poll_interval_secs}s

                  {#if feed.filter_regex} · filter: {feed.filter_regex}{/if}

                </div>

                <button class="danger" onclick={() => deleteFeed(feed.id)}>Delete</button>

              </div>

            {/each}

          </div>

        {/if}

      </div>



    {:else if tab === 'health'}

      {#if health && !health.aria2_ok}

        <div class="alert err" role="alert">

          <strong>aria2 unreachable</strong> — downloads cannot start until the aria2 RPC sidecar is running.

        </div>

      {/if}

      <div class="panel health-grid">

        <button onclick={refresh}>Refresh health</button>

        {#if health}

          <div class="health-item">

            <strong>dl-srv</strong>

            <div class="mono">{health.dlsrv_version}</div>

          </div>

          <div class="health-item">

            <strong>aria2</strong>

            <div class="muted">{health.aria2_ok ? 'connected' : 'unreachable'}</div>

            {#if health.aria2_version}

              <div class="mono">{health.aria2_version}</div>

            {/if}

          </div>

          {#each health.binaries as bin}

            <div class="health-item">

              <strong>{bin.name}</strong>

              {#if bin.update_available}

                <div class="alert warn inline">Update available — latest {bin.latest}</div>

              {/if}

              {#if bin.error}

                <div class="muted err-text">{bin.error}</div>

              {:else}

                <div class="mono">{bin.installed}</div>

                {#if bin.latest}

                  <div class="muted">Latest: {bin.latest}</div>

                {/if}

              {/if}

              {#if bin.name === 'yt-dlp'}

                <button style="margin-top: 0.5rem" disabled={updatingYtdlp} onclick={runUpdateYtdlp}>

                  {updatingYtdlp ? 'Updating…' : 'Update yt-dlp now'}

                </button>

              {/if}

            </div>

          {/each}

        {/if}

        {#if settings}

          <div class="health-item">

            <strong>Paths</strong>

            <div class="mono">{settings.download_dir}</div>

            <div class="muted">Default category: {settings.default_category}</div>

          </div>

        {/if}

      </div>

    {/if}

  {/if}

</div>


