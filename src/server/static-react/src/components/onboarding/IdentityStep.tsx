interface IdentityStepProps {
  displayName: string
  contactHint: string
  birthday: string
  onChange: (fields: { displayName?: string; contactHint?: string; birthday?: string }) => void
  onNext: () => void
  onSkip: () => void
}

export default function IdentityStep({
  displayName,
  contactHint,
  birthday,
  onChange,
  onNext,
  onSkip,
}: IdentityStepProps) {
  const canContinue = displayName.trim().length > 0

  return (
    <div>
      <h2 className="text-sm font-bold mb-1">
        <span className="text-gruvbox-blue">YOUR IDENTITY</span>{' '}
        <span className="text-secondary">How others see you</span>
      </h2>
      <p className="text-primary mb-1">
        Set a display name so trusted contacts can recognize you.
      </p>
      <p className="text-xs text-secondary mb-4">
        This stays on your device. It&apos;s only shared with people you explicitly send trust invites to
        — never sent to Exemem or any server.
      </p>

      <div className="mb-3">
        <label className="text-xs text-secondary block mb-1">Display Name *</label>
        <input
          type="text"
          value={displayName}
          onChange={(e) => onChange({ displayName: e.target.value })}
          placeholder="Your name"
          className="input w-full"
        />
      </div>

      <div className="mb-3">
        <label className="text-xs text-secondary block mb-1">Contact Hint (optional)</label>
        <input
          type="text"
          value={contactHint}
          onChange={(e) => onChange({ contactHint: e.target.value })}
          placeholder="Email, phone, or handle for verification"
          className="input w-full"
        />
        <p className="text-xs text-tertiary mt-1">
          Helps others verify it&apos;s really you when they receive your trust invite.
        </p>
      </div>

      <div className="mb-3">
        <label className="text-xs text-secondary block mb-1">Birthday MM-DD (optional)</label>
        <input
          type="text"
          value={birthday}
          onChange={(e) => onChange({ birthday: e.target.value })}
          placeholder="03-15"
          className="input w-full"
        />
        <p className="text-xs text-tertiary mt-1">
          For peer verification when connecting.
        </p>
      </div>

      <div className="flex gap-2 mt-4">
        <button
          onClick={onNext}
          disabled={!canContinue}
          className="btn-primary flex-1 text-center"
        >
          Save & Continue
        </button>
        <button onClick={onSkip} className="btn-secondary flex-1 text-center">
          Skip
        </button>
      </div>
    </div>
  )
}
