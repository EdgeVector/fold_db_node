import { useState } from 'react'

export interface CloudStepFields {
  inviteCode: string
  // 24-word phrase joined with single spaces, lowercase. Empty string when
  // the user is not restoring.
  recoveryPhrase: string
}

interface CloudBackupStepProps {
  fields: CloudStepFields
  onChange: (next: Partial<CloudStepFields>) => void
  // Submit triggers the wizard's bootstrap call with `enable_cloud=true` and
  // the collected invite code.
  onEnableCloud: () => void
  // Submit triggers the wizard's bootstrap call with the recovery phrase set.
  onRestore: () => void
  // Skip submits with `enable_cloud=false`.
  onSkip: () => void
  submitting: boolean
  error: string | null
}

export default function CloudBackupStep({
  fields,
  onChange,
  onEnableCloud,
  onRestore,
  onSkip,
  submitting,
  error,
}: CloudBackupStepProps) {
  const [showRestore, setShowRestore] = useState(false)

  if (showRestore) {
    const wordCount = fields.recoveryPhrase.trim().split(/\s+/).filter(Boolean).length
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
          value={fields.recoveryPhrase}
          onChange={(e) => onChange({ recoveryPhrase: e.target.value.toLowerCase() })}
          placeholder="Enter your 24-word recovery phrase..."
          rows={4}
          className="input w-full font-mono text-xs"
          disabled={submitting}
        />

        {error && <p className="text-gruvbox-red text-sm mt-3">{error}</p>}

        <div className="flex gap-2 mt-4">
          <button
            onClick={() => { setShowRestore(false); onChange({ recoveryPhrase: '' }) }}
            disabled={submitting}
            className="btn-secondary flex-1 text-center"
          >Back</button>
          <button
            onClick={onRestore}
            disabled={submitting || wordCount !== 24}
            className="btn-primary flex-1 text-center"
          >{submitting ? 'Restoring...' : 'Restore'}</button>
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
          value={fields.inviteCode}
          onChange={(e) => onChange({ inviteCode: e.target.value.toUpperCase() })}
          placeholder="EXM-XXXX-XXXX"
          className="input w-full font-mono tracking-wider"
          disabled={submitting}
        />
        <p className="text-xs text-secondary mt-1">Get an invite code from an existing Exemem user.</p>
      </div>

      {error && (
        <p className="text-gruvbox-red text-sm mt-3">{error}</p>
      )}

      <div className="flex gap-2 mt-4">
        <button
          onClick={onEnableCloud}
          disabled={submitting || !fields.inviteCode.trim()}
          className="btn-primary flex-1 text-center"
        >
          {submitting ? 'Enabling...' : 'Enable Cloud Backup'}
        </button>
        <button
          onClick={onSkip}
          disabled={submitting}
          className="btn-secondary flex-1 text-center"
        >
          Skip
        </button>
      </div>

      <p className="text-xs text-center mt-4">
        <button
          onClick={() => setShowRestore(true)}
          disabled={submitting}
          className="text-tertiary hover:text-secondary bg-transparent border-none cursor-pointer underline text-xs disabled:opacity-50"
        >
          Restore from recovery phrase
        </button>
      </p>
    </div>
  )
}
