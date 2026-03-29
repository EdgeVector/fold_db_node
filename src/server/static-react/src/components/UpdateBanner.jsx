import { useState, useEffect } from 'react'

/**
 * Non-intrusive update notification banner.
 * Listens for `update-available` events from the Tauri backend (emitted on
 * startup) and provides a button to download + install.
 *
 * In a browser (non-Tauri) environment the banner is never shown.
 */
function UpdateBanner() {
  const [updateInfo, setUpdateInfo] = useState(null)
  const [installing, setInstalling] = useState(false)
  const [dismissed, setDismissed] = useState(false)

  useEffect(() => {
    // Only run inside Tauri desktop shell
    if (!window.__TAURI_INTERNALS__) return

    let unlisten

    async function listen() {
      const { listen: tauriListen } = await import('@tauri-apps/api/event')
      unlisten = await tauriListen('update-available', (event) => {
        setUpdateInfo(event.payload)
      })
    }

    listen()

    return () => {
      if (unlisten) unlisten()
    }
  }, [])

  if (!updateInfo || dismissed) return null

  async function handleInstall() {
    setInstalling(true)
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('install_update')
    } catch (err) {
      console.error('Update install failed:', err)
      setInstalling(false)
    }
  }

  return (
    <div className="bg-accent/10 border-b border-accent px-4 py-2 flex items-center justify-between text-sm">
      <span className="text-primary">
        <strong>Update available:</strong> FoldDB v{updateInfo.version}
        {updateInfo.body && <span className="ml-2 text-secondary">— {updateInfo.body}</span>}
      </span>
      <div className="flex items-center gap-2">
        <button
          onClick={handleInstall}
          disabled={installing}
          className="px-3 py-1 rounded bg-accent text-white text-xs font-medium hover:bg-accent/90 disabled:opacity-50"
        >
          {installing ? 'Installing...' : 'Update & Restart'}
        </button>
        <button
          onClick={() => setDismissed(true)}
          className="text-tertiary hover:text-primary text-xs"
          aria-label="Dismiss update notification"
        >
          Dismiss
        </button>
      </div>
    </div>
  )
}

export default UpdateBanner
