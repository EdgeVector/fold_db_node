import { useState } from 'react'
import { systemClient } from '../../api/clients/systemClient'

export default function CloudBackupStep({ onNext, onSkip }) {
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState(null)
  const [created, setCreated] = useState(false)

  const canCreate = email && password && password === confirmPassword && !creating

  const handleCreateAccount = async () => {
    if (password !== confirmPassword) {
      setError('Passwords do not match.')
      return
    }
    setCreating(true)
    setError(null)
    try {
      const response = await systemClient.applySetup({
        storage: {
          type: 'exemem',
          api_url: 'https://api.exemem.com',
          api_key: '',
        },
        exemem_account: { email, password },
      })
      if (response.success) {
        setCreated(true)
      } else {
        setError(response.data?.message || 'Account creation failed. Please try again.')
      }
    } catch (err) {
      setError(err?.message || 'Failed to create account. Please try again.')
    } finally {
      setCreating(false)
    }
  }

  const handleSkip = () => {
    localStorage.setItem('folddb_cloud_backup_skipped', '1')
    onSkip()
  }

  if (created) {
    return (
      <div>
        <h2 className="text-sm font-bold mb-1">
          <span className="text-gruvbox-green">CLOUD BACKUP</span>{' '}
          <span className="text-secondary">Enabled</span>
        </h2>
        <div className="card-success p-3 mt-3">
          <p><span className="badge badge-success">ACCOUNT CREATED</span></p>
          <p className="text-primary mt-1">
            Your Exemem cloud backup account is ready. Encrypted backups will sync automatically.
          </p>
        </div>
        <div className="mt-4">
          <button className="btn-primary w-full text-center" onClick={onNext}>
            Continue
          </button>
        </div>
      </div>
    )
  }

  return (
    <div>
      <h2 className="text-sm font-bold mb-1">
        <span className="text-gruvbox-blue">CLOUD BACKUP</span>{' '}
        <span className="text-secondary">Encrypted backup to Exemem</span>
      </h2>

      <p className="text-primary mt-2">
        Keep your data safe with automatic encrypted cloud backups.
      </p>

      {/* Privacy guarantees */}
      <div className="card p-3 mt-3">
        <p className="font-bold text-primary text-sm mb-2">How it works</p>
        <ul className="text-secondary text-sm space-y-2">
          <li className="flex items-start gap-2">
            <span className="text-gruvbox-green mt-0.5 flex-shrink-0">*</span>
            <span>Your data is <strong className="text-primary">encrypted on your device</strong> before it leaves — the cloud only stores opaque bytes.</span>
          </li>
          <li className="flex items-start gap-2">
            <span className="text-gruvbox-green mt-0.5 flex-shrink-0">*</span>
            <span>Encryption keys <strong className="text-primary">never leave your device</strong>. Not even Exemem can read your data.</span>
          </li>
          <li className="flex items-start gap-2">
            <span className="text-gruvbox-green mt-0.5 flex-shrink-0">*</span>
            <span>Backups sync automatically in the background. Restore to any device with your keys.</span>
          </li>
          <li className="flex items-start gap-2">
            <span className="text-gruvbox-green mt-0.5 flex-shrink-0">*</span>
            <span>Your local database remains the primary copy — the cloud is a backup, not a replacement.</span>
          </li>
        </ul>
      </div>

      {/* Account creation form */}
      <div className="mt-4">
        <p className="label">Email</p>
        <input
          type="email"
          value={email}
          onChange={e => setEmail(e.target.value)}
          placeholder="you@example.com"
          className="input"
          data-testid="backup-email-input"
        />
      </div>

      <div className="mt-3">
        <p className="label">Password</p>
        <input
          type="password"
          value={password}
          onChange={e => setPassword(e.target.value)}
          placeholder="Choose a password"
          className="input"
          data-testid="backup-password-input"
        />
      </div>

      <div className="mt-3">
        <p className="label">Confirm Password</p>
        <input
          type="password"
          value={confirmPassword}
          onChange={e => setConfirmPassword(e.target.value)}
          placeholder="Confirm your password"
          className="input"
          data-testid="backup-confirm-password-input"
        />
        {confirmPassword && password !== confirmPassword && (
          <p className="text-gruvbox-red text-xs mt-1">Passwords do not match</p>
        )}
      </div>

      {error && (
        <p className="text-gruvbox-red mt-2 text-sm">{error}</p>
      )}

      <div className="flex gap-2 mt-4">
        <button
          onClick={handleCreateAccount}
          disabled={!canCreate}
          className="btn-primary flex-1 text-center"
        >
          {creating ? 'Creating Account...' : 'Create Account & Enable Backup'}
        </button>
        <button onClick={handleSkip} className="btn-secondary flex-1 text-center">
          Skip for Now
        </button>
      </div>

      <p className="text-secondary text-xs mt-3 text-center">
        You can enable cloud backup later in Settings &gt; Cloud DB.
      </p>
    </div>
  )
}
