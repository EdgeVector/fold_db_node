import { useState } from 'react'
import { systemClient } from '../../api/clients/systemClient'

export default function CloudBackupStep({ onNext, onSkip }) {
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(null)
  const [success, setSuccess] = useState(false)
  const [inviteCode, setInviteCode] = useState('')

  const handleEnable = async () => {
    if (!inviteCode.trim()) {
      setError('Invite code is required')
      return
    }

    setLoading(true)
    setError(null)
    try {
      // Register this node's public key with Exemem (one-click, no email)
      const resp = await fetch('/api/auth/register', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ invite_code: inviteCode.trim() }),
      })
      const data = await resp.json()

      if (!data.ok) {
        throw new Error(data.error || 'Registration failed')
      }

      // Store cloud credentials for subscription client
      localStorage.setItem('exemem_api_url', data.api_url)
      localStorage.setItem('exemem_api_key', data.api_key)

      // Switch database to Exemem mode
      await systemClient.applySetup({
        storage: {
          type: 'exemem',
          api_url: data.api_url,
          api_key: data.api_key,
        }
      })

      setSuccess(true)
    } catch (e) {
      setError(e?.message || String(e))
    } finally {
      setLoading(false)
    }
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
          maxLength={13}
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
    </div>
  )
}
