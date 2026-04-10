import { useState, useCallback } from 'react'
import { systemClient } from '../api/clients/systemClient'
import { usePolling } from '../hooks/usePolling'

function formatTimestamp(unixSecs) {
  if (!unixSecs) return null
  const date = new Date(unixSecs * 1000)
  const now = new Date()
  const diffMs = now - date
  const diffMins = Math.floor(diffMs / 60000)
  if (diffMins < 1) return 'just now'
  if (diffMins < 60) return `${diffMins}m ago`
  const diffHours = Math.floor(diffMins / 60)
  if (diffHours < 24) return `${diffHours}h ago`
  return date.toLocaleDateString()
}

const POLL_INTERVAL_MS = 10_000

function SyncStatusIndicator({ onCloudSettingsClick }) {
  const [syncStatus, setSyncStatus] = useState(null)

  const pollFn = useCallback(async () => {
    const res = await systemClient.getSyncStatus()
    if (res.data) setSyncStatus(res.data)
  }, [])

  usePolling({
    key: true,
    pollFn,
    intervalMs: POLL_INTERVAL_MS,
    maxFailures: 5,
    onMaxFailures: () => {},
  })

  if (!syncStatus) return null

  // Disabled state (local mode, no sync engine)
  if (!syncStatus.enabled) {
    return (
      <button
        onClick={onCloudSettingsClick}
        className="bg-transparent border-none cursor-pointer p-0 font-mono text-sm text-tertiary hover:text-primary flex items-center gap-1"
        title="Cloud sync disabled (local mode)"
      >
        <span className="text-base leading-none">—</span>
        <span>Sync: off</span>
      </button>
    )
  }

  const { state, last_sync_at, last_error, pending_count } = syncStatus
  const lastSyncText = formatTimestamp(last_sync_at)

  // Error state
  if (state === 'offline' || last_error) {
    return (
      <button
        onClick={onCloudSettingsClick}
        className="bg-transparent border-none cursor-pointer p-0 font-mono text-sm text-gruvbox-red hover:text-primary flex items-center gap-1"
        title={last_error || 'Sync offline'}
      >
        <span className="text-base leading-none">✕</span>
        <span>Sync: error</span>
      </button>
    )
  }

  // Syncing state
  if (state === 'syncing' || state === 'dirty') {
    const label = state === 'syncing' ? 'syncing' : `${pending_count || 0} pending`
    return (
      <button
        onClick={onCloudSettingsClick}
        className="bg-transparent border-none cursor-pointer p-0 font-mono text-sm text-gruvbox-yellow hover:text-primary flex items-center gap-1"
        title={lastSyncText ? `Last synced: ${lastSyncText}` : 'Syncing...'}
      >
        <span className="text-base leading-none animate-spin inline-block" style={{ animationDuration: '2s' }}>⟳</span>
        <span>Sync: {label}</span>
      </button>
    )
  }

  // Idle (synced) state
  return (
    <button
      onClick={onCloudSettingsClick}
      className="bg-transparent border-none cursor-pointer p-0 font-mono text-sm text-gruvbox-green hover:text-primary flex items-center gap-1"
      title={lastSyncText ? `Last synced: ${lastSyncText}` : 'Cloud sync active'}
    >
      <span className="text-base leading-none">✓</span>
      <span>Synced</span>
    </button>
  )
}

export default SyncStatusIndicator
