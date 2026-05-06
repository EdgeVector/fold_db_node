import { useState } from 'react'

interface RecoveryPhraseViewProps {
  words: string[]
  onContinue: () => void
}

export default function RecoveryPhraseView({ words, onContinue }: RecoveryPhraseViewProps) {
  const [savedConfirmed, setSavedConfirmed] = useState(false)

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

      <div
        data-testid="recovery-phrase-words"
        className="grid grid-cols-3 gap-2 p-3 border border-border rounded-md bg-surface-elevated font-mono text-xs"
      >
        {words.map((word, i) => (
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
          onClick={onContinue}
          disabled={!savedConfirmed}
          className="btn-primary flex-1 text-center"
        >
          Continue
        </button>
      </div>
    </div>
  )
}
