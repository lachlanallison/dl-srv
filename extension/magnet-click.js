;(function () {
  const runtime = globalThis.browser?.runtime || globalThis.chrome?.runtime
  document.addEventListener(
    'click',
    (event) => {
      const link = event.target.closest?.('a[href^="magnet:"]')
      if (!link || !runtime) return
      event.preventDefault()
      event.stopImmediatePropagation()
      runtime
        .sendMessage({
          type: 'dlsrv-magnet-click',
          url: link.href,
          pageUrl: location.href,
        })
        .catch(() => {})
    },
    true,
  )
})()
