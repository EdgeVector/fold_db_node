import { useState } from 'react'
import { migrateToCloud } from '../../api/clients/systemClient'

export default function CloudBackupStep({ onNext, onSkip }) {
  const [apiUrl, setApiUrl] = useState('https://api.exemem.com')
  const [apiKey, setApiKey] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(null)
  const [success, setSuccess] = useState(false)

  const handleEnable = async () => {
    if (!apiKey.trim()) {
      setError('API key is required')
      return
    }
    setLoading(true)
    setError(null)
    try {
      const resp = await migrateToCloud(apiUrl, apiKey)
      if (resp.success) {
        setSuccess(true)
      } else {
        setError(resp.data?.message || 'Failed to enable cloud backup')
      }
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
      </p>

      <div className="space-y-3">
        <div>
          <p className="label">Cloud API URL</p>
          <input
            type="text"
            value={apiUrl}
            onChange={(e) => setApiUrl(e.target.value)}
            className="input"
            placeholder="https://api.exemem.com"
          />
        </div>
        <div>
          <p className="label">API Key</p>
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            className="input"
            placeholder="Your Exemem API key"
          />
          <p className="text-xs text-secondary mt-1">
            Don't have an account? You can set this up later in Settings.
          </p>
        </div>
      </div>

      {error && (
        <p className="text-gruvbox-red text-sm mt-3">{error}</p>
      )}

      <div className="flex gap-2 mt-4">
        <button
          onClick={handleEnable}
          disabled={loading || !apiKey.trim()}
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
