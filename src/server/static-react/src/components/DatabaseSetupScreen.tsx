import { useState } from 'react'
import { applySetup } from '../api/clients/systemClient'
import { activateExemem } from '../api/clients/activateExemem'

const RESTORE_POLL_INTERVAL_MS = 2000
const RESTORE_POLL_MAX_MS = 5 * 60 * 1000

interface PollRestoreOptions {
  intervalMs?: number
  maxMs?: number
  fetchImpl?: typeof fetch
}

export async function pollRestoreStatus({
  intervalMs = RESTORE_POLL_INTERVAL_MS,
  maxMs = RESTORE_POLL_MAX_MS,
  fetchImpl = fetch,
}: PollRestoreOptions = {}): Promise<void> {
  const deadline = Date.now() + maxMs
  // eslint-disable-next-line no-constant-condition
  while (true) {
    const resp = await fetchImpl('/api/auth/restore/status')
    if (!resp.ok) {
      throw new Error(`Restore status request failed: HTTP ${resp.status}`)
    }
    const body = await resp.json() as { status?: string; error?: string }
    if (body.status === 'complete') {
      return
    }
    if (body.status === 'failed') {
      throw new Error(body.error || 'Restore failed')
    }
    if (body.status !== 'in_progress') {
      throw new Error(`Unknown restore status: ${JSON.stringify(body.status)}`)
    }
    if (Date.now() + intervalMs > deadline) {
      throw new Error(
        'Restore is still in progress after 5 minutes. Check the node logs and try again.'
      )
    }
    await new Promise((r) => setTimeout(r, intervalMs))
  }
}

interface DatabaseSetupScreenProps {
  onComplete: () => void
}

type RestoreStatus = 'idle' | 'registering' | 'in_progress' | 'complete' | 'failed'
type Mode = 'choose' | 'invite' | 'restore'

export default function DatabaseSetupScreen({ onComplete }: DatabaseSetupScreenProps) {
  const [loading, setLoading] = useState(false)
  const [loadingLabel, setLoadingLabel] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [mode, setMode] = useState<Mode>('choose')
  const [inviteCode, setInviteCode] = useState('')
  const [recoveryPhrase, setRecoveryPhrase] = useState('')
  const [restoreStatus, setRestoreStatus] = useState<RestoreStatus>('idle')

  const handleLocalSetup = async () => {
    if (loading) return
    setLoading(true)
    setLoadingLabel('Setting up local storage…')
    setError(null)
    const defaultPath = '~/.folddb/data'
    try {
      const response = await applySetup({
        storage: { type: 'local', path: defaultPath },
      })
      if (response.success) {
        onComplete()
      } else {
        const msg = (response.data as { message?: string } | undefined)?.message
        setError(msg || 'Setup failed')
        setLoading(false)
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      if (msg.includes('could not acquire lock') || msg.includes('WouldBlock')) {
        setError(
          'Another FoldDB instance is already running and holds the database lock. ' +
          'Please close the other instance and try again.'
        )
      } else {
        setError(msg)
      }
      setLoading(false)
    }
  }

  // Single-click commit on the Cloud card just opens the invite-code form.
  // The actual cloud setup (which needs the invite code) runs from the form's
  // submit button — we don't have the code yet at card-click time.
  const handleCloudCardClick = () => {
    if (loading) return
    setMode('invite')
    setError(null)
  }

  const submitCloudSetup = async () => {
    if (!inviteCode.trim()) {
      setError('Invite code is required')
      return
    }
    setLoading(true)
    setLoadingLabel('Connecting to Exemem Cloud…')
    setError(null)
    try {
      await activateExemem(inviteCode)
      onComplete()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setLoading(false)
    }
  }

  const handleRestore = async () => {
    const words = recoveryPhrase.trim().toLowerCase()
    const wordCount = words.split(/\s+/).length
    if (wordCount !== 24) {
      setError(`Recovery phrase must be 24 words (got ${wordCount})`)
      return
    }
    setLoading(true)
    setLoadingLabel('Restoring identity…')
    setError(null)
    setRestoreStatus('registering')
    try {
      const resp = await fetch('/api/auth/restore', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ words }),
      })
      const data = await resp.json() as { ok?: boolean; error?: string; api_url?: string; api_key?: string }

      if (!data.ok) {
        throw new Error(data.error || 'Restore failed')
      }
      if (!data.api_url || !data.api_key) {
        throw new Error('Restore response missing api_url/api_key')
      }

      localStorage.setItem('exemem_api_url', data.api_url)
      localStorage.setItem('exemem_api_key', data.api_key)

      setRestoreStatus('in_progress')
      setLoadingLabel('Restoring your data from the cloud. This may take a few minutes…')
      await pollRestoreStatus()
      setRestoreStatus('complete')

      const response = await applySetup({
        storage: {
          type: 'exemem',
          api_url: data.api_url,
          api_key: data.api_key,
        },
      })
      if (response.success) {
        onComplete()
      } else {
        setRestoreStatus('failed')
        const msg = (response.data as { message?: string } | undefined)?.message
        setError(msg || 'Setup failed')
        setLoading(false)
      }
    } catch (e) {
      setRestoreStatus('failed')
      setError(e instanceof Error ? e.message : String(e))
      setLoading(false)
    }
  }

  const resetRestore = () => {
    setRestoreStatus('idle')
    setError(null)
  }

  return (
    <div className="fixed inset-0 z-50 bg-surface text-primary flex items-center justify-center font-mono">
      <div className="max-w-[600px] w-[90%] text-center">
        <h1 className="text-2xl font-bold text-gruvbox-bright mb-2">FoldDB</h1>
        <p className="text-sm text-tertiary mb-8">Choose where to store your data</p>

        {mode === 'choose' && (
          <div className="flex gap-4 justify-center flex-wrap">
            <button
              type="button"
              onClick={handleLocalSetup}
              disabled={loading}
              className="bg-gruvbox-elevated border border-border w-[250px] p-6 text-left text-primary font-mono cursor-pointer transition-colors hover:border-gruvbox-green disabled:cursor-wait disabled:opacity-60"
            >
              <div className="text-xl mb-2">Local Storage</div>
              <div className="inline-block px-1.5 py-px text-[11px] font-bold bg-gruvbox-green text-surface mb-3">
                RECOMMENDED
              </div>
              <p className="text-[13px] text-tertiary leading-relaxed m-0">
                Store data on your device. Fast, private, works offline. Uses ~/.folddb/data.
              </p>
            </button>

            <button
              type="button"
              onClick={handleCloudCardClick}
              disabled={loading}
              className="bg-gruvbox-elevated border border-border w-[250px] p-6 text-left text-primary font-mono cursor-pointer transition-colors hover:border-gruvbox-blue disabled:cursor-wait disabled:opacity-60"
            >
              <div className="text-xl mb-2">Exemem Cloud</div>
              <div className="inline-block px-1.5 py-px text-[11px] font-bold bg-gruvbox-blue text-surface mb-3">
                CLOUD
              </div>
              <p className="text-[13px] text-tertiary leading-relaxed m-0">
                Store data in the cloud. Syncs across devices. Requires an Exemem account.
              </p>
            </button>
          </div>
        )}

        {mode === 'choose' && !loading && (
          <p className="mt-6 text-[13px]">
            <button
              type="button"
              onClick={() => { setMode('restore'); setError(null) }}
              className="bg-transparent border-none text-gruvbox-purple cursor-pointer font-mono text-[13px] underline"
            >
              Restore from recovery phrase
            </button>
          </p>
        )}

        {mode === 'invite' && !loading && (
          <div className="mt-6 max-w-[300px] mx-auto text-left">
            <label className="text-xs text-tertiary block mb-1">Invite Code</label>
            <input
              type="text"
              value={inviteCode}
              onChange={(e) => setInviteCode(e.target.value.toUpperCase())}
              placeholder="EXM-XXXX-XXXX"
              className="input tracking-[2px]"
              autoFocus
            />
            <p className="text-[11px] text-tertiary mt-1">
              Get an invite code from an existing Exemem user.
            </p>
            <div className="flex gap-2 mt-3">
              <button
                type="button"
                onClick={() => { setMode('choose'); setInviteCode(''); setError(null) }}
                className="btn btn-secondary flex-1"
              >
                Back
              </button>
              <button
                type="button"
                onClick={submitCloudSetup}
                disabled={!inviteCode.trim()}
                className="btn flex-1 bg-gruvbox-blue text-surface border border-gruvbox-blue disabled:opacity-50 disabled:cursor-not-allowed"
              >
                Continue
              </button>
            </div>
          </div>
        )}

        {mode === 'restore' && !loading && (
          <div className="mt-6 max-w-[400px] mx-auto text-left">
            <label className="text-xs text-tertiary block mb-1">
              Recovery Phrase (24 words)
            </label>
            <textarea
              value={recoveryPhrase}
              onChange={(e) => setRecoveryPhrase(e.target.value.toLowerCase())}
              placeholder="Enter your 24-word recovery phrase..."
              rows={4}
              className="textarea leading-relaxed"
              autoFocus
            />
            <p className="text-[11px] text-tertiary mt-1">
              Enter the 24 words you saved when you first set up your account.
            </p>
            <div className="flex gap-2 mt-3">
              <button
                type="button"
                onClick={() => { setMode('choose'); setRecoveryPhrase(''); setError(null) }}
                className="btn btn-secondary flex-1"
              >
                Back
              </button>
              <button
                type="button"
                onClick={handleRestore}
                disabled={recoveryPhrase.trim().split(/\s+/).length !== 24}
                className="btn flex-1 bg-gruvbox-purple text-surface border border-gruvbox-purple disabled:opacity-50 disabled:cursor-not-allowed"
              >
                Restore
              </button>
            </div>
          </div>
        )}

        {loading && (
          <div
            data-testid="restore-progress"
            className="fixed inset-0 z-[60] bg-surface/95 flex items-center justify-center"
          >
            <div className="text-center">
              <div className="w-8 h-8 mx-auto mb-4 border-2 border-border border-t-gruvbox-yellow rounded-full animate-spin" />
              <p className="text-primary text-sm">{loadingLabel}</p>
            </div>
          </div>
        )}

        {error && !loading && (
          <div className="mt-6 p-3 bg-gruvbox-red/10 border border-gruvbox-red text-[13px] text-gruvbox-red text-left">
            <div>{error}</div>
            {mode === 'restore' && restoreStatus === 'failed' && (
              <div className="flex gap-2 mt-3">
                <button
                  type="button"
                  onClick={resetRestore}
                  className="btn flex-1 bg-gruvbox-purple text-surface border border-gruvbox-purple"
                >
                  Try again
                </button>
                <button
                  type="button"
                  onClick={() => { setMode('choose'); setRecoveryPhrase(''); resetRestore() }}
                  className="btn btn-secondary flex-1"
                >
                  Cancel
                </button>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
