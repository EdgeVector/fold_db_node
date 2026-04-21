import React, { useState, useEffect, useRef, useCallback } from 'react'
import { getSyncStatus, triggerSync } from '../../api/clients/systemClient'
import { getSubscriptionStatus, CloudApiError } from '../../api/clients/subscriptionClient'

const POLL_INTERVAL_MS = 10_000

// Validated against Exemem, not just checked for presence. "Starting…" is only
// shown while credentials are 'valid' but sync hasn't reported enabled yet;
// when they're 'stale' we surface that honestly instead of looping forever.
type CredentialsState = 'unknown' | 'none' | 'valid' | 'stale'

interface SyncStatusData {
  enabled?: boolean
  state?: string
  pending_count?: number
  encryption_active?: boolean
}

interface TriggerResult {
  type: 'success' | 'error'
  message: string
}

function BackupSettingsPanel(): React.ReactElement {
  const [status, setStatus] = useState<SyncStatusData | null>(null)
  const [loading, setLoading] = useState<boolean>(true)
  const [syncing, setSyncing] = useState<boolean>(false)
  const [triggerResult, setTriggerResult] = useState<TriggerResult | null>(null)
  const [credentialsState, setCredentialsState] = useState<CredentialsState>('unknown')
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const resultTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)

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

  // Validate Exemem credentials against the cloud API (not just check local
  // presence). We previously drove "Starting…" off mere presence, which meant
  // stale/revoked credentials left users stuck in an eternal "Starting…"
  // state. Now: 401/403 → 'stale', success → 'valid', absent → 'none',
  // transient failure → leave 'unknown' so the rest of the UI falls back to
  // neutral copy instead of claiming sync is about to start.
  useEffect(() => {
    const hasLocalCreds = !!(
      localStorage.getItem('exemem_api_url') && localStorage.getItem('exemem_api_key')
    )
    const validate = async () => {
      let hasCreds = hasLocalCreds
      if (!hasCreds) {
        try {
          const resp = await fetch('/api/auth/credentials')
          if (resp.ok) {
            const data = await resp.json()
            hasCreds = !!(data.ok && data.has_credentials)
          }
        } catch {
          // Local node not reachable — treat as 'unknown', default below.
        }
      }
      if (!hasCreds) {
        setCredentialsState('none')
        return
      }
      try {
        await getSubscriptionStatus()
        setCredentialsState('valid')
      } catch (err: unknown) {
        if (err instanceof CloudApiError && (err.status === 401 || err.status === 403)) {
          setCredentialsState('stale')
        } else {
          // Transient cloud outage — don't falsely show "Starting…" or
          // "Disabled"; leave credentialsState === 'unknown' so the panel
          // shows neutral copy.
          setCredentialsState('unknown')
        }
      }
    }
    validate()
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
  return (
    <div className="space-y-6">
      {/* Backup Enable/Disable Status */}
      <div>
        <div className="flex items-center justify-between">
          <div>
            <h4 className="text-sm font-semibold text-primary">Cloud Backup</h4>
            <p className="text-xs text-secondary mt-1">
              {enabled && 'Your data is being synced to Exemem cloud storage with end-to-end encryption.'}
              {!enabled && credentialsState === 'valid' && 'Cloud backup is registered. Sync should activate shortly.'}
              {!enabled && credentialsState === 'stale' && 'Cloud backup credentials were rejected by Exemem. Re-enter an invite code in Cloud Storage above to restore sync.'}
              {!enabled && credentialsState === 'none' && 'Cloud backup is not configured. Sign up for Exemem above to enable backup sync.'}
              {!enabled && credentialsState === 'unknown' && 'Cloud backup status is unavailable right now. Check your Exemem connection above.'}
            </p>
          </div>
          <div className={`px-3 py-1 rounded-full text-xs font-medium ${
            enabled
              ? 'bg-gruvbox-green/20 text-gruvbox-green'
              : credentialsState === 'valid'
                ? 'bg-gruvbox-yellow/20 text-gruvbox-yellow'
                : credentialsState === 'stale'
                  ? 'bg-gruvbox-red/20 text-gruvbox-red'
                  : credentialsState === 'unknown'
                    ? 'bg-gruvbox-dim/20 text-gruvbox-dim'
                    : 'bg-gruvbox-red/20 text-gruvbox-red'
          }`}>
            {enabled && 'Enabled'}
            {!enabled && credentialsState === 'valid' && 'Starting...'}
            {!enabled && credentialsState === 'stale' && 'Rejected'}
            {!enabled && credentialsState === 'none' && 'Disabled'}
            {!enabled && credentialsState === 'unknown' && 'Unknown'}
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
        {!enabled && credentialsState === 'none' && (
          <p className="text-xs text-secondary mt-2">
            Enable cloud storage to use manual backup.
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

function getStateDotClass(state: string | undefined, enabled: boolean): string {
  if (!enabled) return ''
  switch (state) {
    case 'idle': return 'status-dot-success'
    case 'syncing': return 'status-dot-success'
    case 'dirty': return '' // default yellow/neutral
    case 'offline': return ''
    default: return ''
  }
}

function getStateLabel(state: string | undefined, enabled: boolean): string {
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
