import { useState } from 'react'
import { systemClient } from '../../api/clients/systemClient'

export default function CloudBackupStep({ onNext, onSkip }) {
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(null)
  const [success, setSuccess] = useState(false)
  const [inviteCode, setInviteCode] = useState('')
  const [recoveryWords, setRecoveryWords] = useState(null)
  const [savedConfirmed, setSavedConfirmed] = useState(false)
  const [showRestore, setShowRestore] = useState(false)
  const [restorePhrase, setRestorePhrase] = useState('')

  const handleEnable = async () => {
    if (!inviteCode.trim()) {
      setError('Invite code is required')
      return
    }

    setLoading(true)
    setError(null)
    try {
      const resp = await fetch('/api/auth/register', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ invite_code: inviteCode.trim() }),
      })
      if (!resp.ok) {
        const errBody = await resp.text().catch(() => '')
        throw new Error(errBody || `Registration failed (HTTP ${resp.status})`)
      }
      const data = await resp.json()

      if (!data.ok) {
        throw new Error(data.error || 'Registration failed')
      }

      localStorage.setItem('exemem_api_url', data.api_url)
      localStorage.setItem('exemem_api_key', data.api_key)

      await systemClient.applySetup({
        storage: {
          type: 'exemem',
          api_url: data.api_url,
          api_key: data.api_key,
        }
      })

      // Fetch recovery phrase
      const phraseResp = await fetch('/api/auth/recovery-phrase')
      if (!phraseResp.ok) {
        throw new Error(`Failed to fetch recovery phrase (HTTP ${phraseResp.status})`)
      }
      const phraseData = await phraseResp.json()
      if (phraseData.ok) {
        setRecoveryWords(phraseData.words)
      }

      setSuccess(true)
    } catch (e) {
      setError(e?.message || String(e))
    } finally {
      setLoading(false)
    }
  }

  const handleRestore = async () => {
    const words = restorePhrase.trim().toLowerCase()
    const wordCount = words.split(/\s+/).length
    if (wordCount !== 24) {
      setError(`Recovery phrase must be 24 words (got ${wordCount})`)
      return
    }
    setLoading(true)
    setError(null)
    try {
      const resp = await fetch('/api/auth/restore', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ words }),
      })
      if (!resp.ok) {
        const errBody = await resp.text().catch(() => '')
        throw new Error(errBody || `Restore failed (HTTP ${resp.status})`)
      }
      const data = await resp.json()
      if (!data.ok) throw new Error(data.error || 'Restore failed')

      localStorage.setItem('exemem_api_url', data.api_url)
      localStorage.setItem('exemem_api_key', data.api_key)

      await systemClient.applySetup({
        storage: { type: 'exemem', api_url: data.api_url, api_key: data.api_key }
      })

      setSuccess(true)
    } catch (e) {
      setError(e?.message || String(e))
    } finally {
      setLoading(false)
    }
  }

  if (showRestore) {
    return (
      <div>
        <h2 className="text-sm font-bold mb-1">
          <span className="text-gruvbox-purple">RESTORE</span>{' '}
          <span className="text-secondary">From recovery phrase</span>
        </h2>
        <p className="text-xs text-secondary mb-3">
          Enter the 24 words you saved when you first set up your account.
        </p>

        <textarea
          value={restorePhrase}
          onChange={(e) => setRestorePhrase(e.target.value.toLowerCase())}
          placeholder="Enter your 24-word recovery phrase..."
          rows={4}
          className="input-field w-full font-mono text-xs"
          disabled={loading}
        />

        {error && <p className="text-gruvbox-red text-sm mt-3">{error}</p>}

        <div className="flex gap-2 mt-4">
          <button
            onClick={() => { setShowRestore(false); setRestorePhrase(''); setError(null) }}
            className="btn-secondary flex-1 text-center"
          >Back</button>
          <button
            onClick={handleRestore}
            disabled={loading || restorePhrase.trim().split(/\s+/).length !== 24}
            className="btn-primary flex-1 text-center"
          >{loading ? 'Restoring...' : 'Restore'}</button>
        </div>
      </div>
    )
  }

  if (success && recoveryWords) {
    return (
      <div>
        <h2 className="text-sm font-bold mb-1">
          <span className="text-gruvbox-green">RECOVERY PHRASE</span>{' '}
          <span className="text-secondary">Save these 24 words</span>
        </h2>
        <p className="text-xs text-secondary mb-3">
          This is the only way to restore your account on a new device.
          Write it down and store it somewhere safe.
        </p>

        <div className="grid grid-cols-3 gap-2 p-3 border border-border rounded-md bg-surface-elevated font-mono text-xs">
          {recoveryWords.map((word, i) => (
            <div key={i} className="flex items-center gap-1">
              <span className="text-tertiary w-5 text-right">{i + 1}.</span>
              <span className="text-primary">{word}</span>
            </div>
          ))}
        </div>

        <label className="flex items-center gap-2 mt-4 text-xs text-secondary cursor-pointer">
          <input
            type="checkbox"
            checked={savedConfirmed}
            onChange={(e) => setSavedConfirmed(e.target.checked)}
            className="accent-gruvbox-green"
          />
          I have saved my recovery phrase
        </label>

        <div className="flex gap-2 mt-4">
          <button
            onClick={onNext}
            disabled={!savedConfirmed}
            className="btn-primary flex-1 text-center"
          >
            Continue
          </button>
        </div>
      </div>
    )
  }

  if (success) {
    return (
      <div>
        <h2 className="text-sm font-bold mb-1">
          <span className="text-gruvbox-green">CLOUD BACKUP</span>{' '}
          <span className="text-secondary">Enabled</span>
        </h2>
        <div className="card-success p-4 mt-4">
          <p className="text-primary">Cloud backup is now enabled. Your data will sync to Exemem cloud storage.</p>
          <p className="text-xs text-secondary mt-2">Your local database is preserved as a backup.</p>
        </div>
        <div className="flex gap-2 mt-4">
          <button onClick={onNext} className="btn-primary flex-1 text-center">Continue</button>
        </div>
      </div>
    )
  }

  return (
    <div>
      <h2 className="text-sm font-bold mb-1">
        <span className="text-gruvbox-purple">CLOUD BACKUP</span>{' '}
        <span className="text-secondary">Sync across devices</span>
      </h2>
      <p className="text-primary mb-1">Enable encrypted cloud backup to sync your data across devices.</p>
      <p className="text-xs text-secondary mb-4">
        Your data is encrypted before leaving your device. The local database remains your primary store.
        You get 1 GB of free storage to start.
      </p>

      <div className="mb-3">
        <label className="text-xs text-secondary block mb-1">Invite Code</label>
        <input
          type="text"
          value={inviteCode}
          onChange={(e) => setInviteCode(e.target.value.toUpperCase())}
          placeholder="EXM-XXXX-XXXX"
          className="input-field w-full font-mono tracking-wider"
          disabled={loading}
        />
        <p className="text-xs text-secondary mt-1">Get an invite code from an existing Exemem user.</p>
      </div>

      {error && (
        <p className="text-gruvbox-red text-sm mt-3">{error}</p>
      )}

      <div className="flex gap-2 mt-4">
        <button
          onClick={handleEnable}
          disabled={loading || !inviteCode.trim()}
          className="btn-primary flex-1 text-center"
        >
          {loading ? 'Enabling...' : 'Enable Cloud Backup'}
        </button>
        <button onClick={onSkip} className="btn-secondary flex-1 text-center">
          Skip
        </button>
      </div>

      <p className="text-xs text-center mt-4">
        <button
          onClick={() => { setShowRestore(true); setError(null) }}
          className="text-tertiary hover:text-secondary bg-transparent border-none cursor-pointer underline text-xs"
        >
          Restore from recovery phrase
        </button>
      </p>
    </div>
  )
}
