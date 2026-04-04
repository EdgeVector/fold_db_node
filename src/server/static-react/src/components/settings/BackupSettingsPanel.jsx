import { useState, useEffect, useRef, useCallback } from 'react'
import { getSyncStatus, triggerSync } from '../../api/clients/systemClient'

const POLL_INTERVAL_MS = 10_000

function BackupSettingsPanel() {
  const [status, setStatus] = useState(null)
  const [loading, setLoading] = useState(true)
  const [syncing, setSyncing] = useState(false)
  const [triggerResult, setTriggerResult] = useState(null)
  const [hasCredentials, setHasCredentials] = useState(false)
  const pollRef = useRef(null)
  const resultTimeoutRef = useRef(null)

  const fetchStatus = useCallback(async () => {
    try {
      const response = await getSyncStatus()
      if (response.success && response.data) {
        setStatus(response.data)
      }
    } catch (error) {
      console.error('Failed to fetch sync status:', error)
    } finally {
      setLoading(false)
    }
  }, [])

  // Detect if user has Exemem credentials (signed up but sync may not be running)
  useEffect(() => {
    const hasLocalCreds = localStorage.getItem('exemem_api_url') && localStorage.getItem('exemem_api_key')
    if (hasLocalCreds) {
      setHasCredentials(true)
      return
    }
    fetch('/api/auth/credentials')
      .then(r => r.json())
      .then(data => {
        if (data.ok && data.has_credentials) setHasCredentials(true)
      })
      .catch(() => {})
  }, [])

  useEffect(() => {
    fetchStatus()
    pollRef.current = setInterval(fetchStatus, POLL_INTERVAL_MS)
    return () => {
      if (pollRef.current) clearInterval(pollRef.current)
      if (resultTimeoutRef.current) clearTimeout(resultTimeoutRef.current)
    }
  }, [fetchStatus])

  const handleTriggerSync = async () => {
    setSyncing(true)
    setTriggerResult(null)
    try {
      const response = await triggerSync()
      if (response.success && response.data) {
        setTriggerResult({
          type: response.data.success ? 'success' : 'error',
          message: response.data.message,
        })
      } else {
        setTriggerResult({
          type: 'error',
          message: response.error || 'Sync trigger failed',
        })
      }
      // Refresh status after trigger
      await fetchStatus()
    } catch (error) {
      setTriggerResult({
        type: 'error',
        message: `Network error: ${error instanceof Error ? error.message : String(error)}`,
      })
    } finally {
      setSyncing(false)
      resultTimeoutRef.current = setTimeout(() => setTriggerResult(null), 5000)
    }
  }

  if (loading) {
    return (
      <div className="flex items-center gap-2 p-4 text-secondary">
        <span className="spinner" /> Loading backup status...
      </div>
    )
  }

  const enabled = status?.enabled ?? false
  const syncState = status?.state
  const pendingCount = status?.pending_count ?? 0
  const encryptionActive = status?.encryption_active ?? false
  // Cloud mode = sync engine running OR credentials exist (signed up but needs restart)
  const cloudMode = enabled || hasCredentials

  return (
    <div className="space-y-6">
      {/* Backup Enable/Disable Status */}
      <div>
        <div className="flex items-center justify-between">
          <div>
            <h4 className="text-sm font-semibold text-primary">Cloud Backup</h4>
            <p className="text-xs text-secondary mt-1">
              {enabled
                ? 'Your data is being synced to Exemem cloud storage with end-to-end encryption.'
                : hasCredentials
                  ? 'Cloud backup is enabled but sync is not active. Restart the server to start syncing.'
                  : 'Cloud backup is not configured. Sign up for Exemem above to enable backup sync.'}
            </p>
          </div>
          <div className={`px-3 py-1 rounded-full text-xs font-medium ${
            enabled
              ? 'bg-gruvbox-green/20 text-gruvbox-green'
              : hasCredentials
                ? 'bg-gruvbox-yellow/20 text-gruvbox-yellow'
                : 'bg-gruvbox-red/20 text-gruvbox-red'
          }`}>
            {enabled ? 'Enabled' : hasCredentials ? 'Restart Required' : 'Disabled'}
          </div>
        </div>
      </div>

      {/* Status Cards */}
      <div className="grid grid-cols-2 gap-4">
        {/* Sync State */}
        <div className="card p-4">
          <p className="text-xs text-secondary mb-1">Sync Status</p>
          <div className="flex items-center gap-2">
            <span className={`status-dot ${getStateDotClass(syncState, enabled)}`} />
            <span className="text-sm font-medium text-primary">
              {getStateLabel(syncState, enabled)}
            </span>
          </div>
        </div>

        {/* Pending Changes */}
        <div className="card p-4">
          <p className="text-xs text-secondary mb-1">Pending Changes</p>
          <span className="text-sm font-medium text-primary">
            {enabled ? `${pendingCount} entr${pendingCount === 1 ? 'y' : 'ies'}` : '--'}
          </span>
        </div>

        {/* Encryption Status */}
        <div className="card p-4">
          <p className="text-xs text-secondary mb-1">Encryption</p>
          <div className="flex items-center gap-2">
            <span className={`status-dot ${encryptionActive ? 'status-dot-success' : ''}`} />
            <span className="text-sm font-medium text-primary">
              {encryptionActive ? 'AES-256-GCM (E2E)' : 'Not active'}
            </span>
          </div>
        </div>

        {/* Sync Engine */}
        <div className="card p-4">
          <p className="text-xs text-secondary mb-1">Sync Engine</p>
          <span className="text-sm font-medium text-primary">
            {enabled ? 'Backblaze B2 (S3)' : 'Not configured'}
          </span>
        </div>
      </div>

      {/* Manual Sync Button */}
      <div className="pt-2">
        <button
          onClick={handleTriggerSync}
          disabled={!enabled || syncing}
          className="btn-primary flex items-center gap-2"
        >
          {syncing ? (
            <>
              <span className="spinner" />
              Syncing...
            </>
          ) : (
            <>
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                  d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
              </svg>
              Back Up Now
            </>
          )}
        </button>
        {!enabled && (
          <p className="text-xs text-secondary mt-2">
            {hasCredentials
              ? 'Restart the server to activate cloud sync.'
              : 'Enable cloud storage to use manual backup.'}
          </p>
        )}
      </div>

      {/* Trigger Result */}
      {triggerResult && (
        <div className={`p-3 text-sm card ${
          triggerResult.type === 'success'
            ? 'card-success text-gruvbox-green'
            : 'card-error text-gruvbox-red'
        }`}>
          {triggerResult.message}
        </div>
      )}
    </div>
  )
}

function getStateDotClass(state, enabled) {
  if (!enabled) return ''
  switch (state) {
    case 'idle': return 'status-dot-success'
    case 'syncing': return 'status-dot-success'
    case 'dirty': return '' // default yellow/neutral
    case 'offline': return ''
    default: return ''
  }
}

function getStateLabel(state, enabled) {
  if (!enabled) return 'Not configured'
  switch (state) {
    case 'idle': return 'Up to date'
    case 'syncing': return 'Syncing...'
    case 'dirty': return 'Changes pending'
    case 'offline': return 'Offline (will retry)'
    default: return 'Unknown'
  }
}

export default BackupSettingsPanel
