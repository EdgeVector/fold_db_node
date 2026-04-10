import { useState, useEffect } from 'react'
import { getIdentityCard, setIdentityCard } from '../../api/clients/trustClient'

export default function IdentityStep({ onNext, onSkip }) {
  const [displayName, setDisplayName] = useState('')
  const [contactHint, setContactHint] = useState('')
  const [birthday, setBirthday] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(null)
  const [existingCard, setExistingCard] = useState(null)

  useEffect(() => {
    getIdentityCard().then((resp) => {
      if (resp.success && resp.data?.identity_card) {
        const card = resp.data.identity_card
        setExistingCard(card)
        setDisplayName(card.display_name)
        setContactHint(card.contact_hint || '')
        setBirthday(card.birthday || '')
      }
    }).catch(() => {})
  }, [])

  const handleSave = async () => {
    if (!displayName.trim()) {
      setError('Display name is required')
      return
    }
    setLoading(true)
    setError(null)
    try {
      const resp = await setIdentityCard(displayName.trim(), contactHint.trim() || null, birthday.trim() || null)
      if (resp.success) {
        onNext()
      } else {
        setError(resp.error || 'Failed to save identity')
      }
    } catch (e) {
      setError(e?.message || String(e))
    } finally {
      setLoading(false)
    }
  }

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
        This stays on your device. It's only shared with people you explicitly send trust invites to
        — never sent to Exemem or any server.
      </p>

      <div className="mb-3">
        <label className="text-xs text-secondary block mb-1">Display Name *</label>
        <input
          type="text"
          value={displayName}
          onChange={(e) => setDisplayName(e.target.value)}
          placeholder="Your name"
          className="input w-full"
          disabled={loading}
        />
      </div>

      <div className="mb-3">
        <label className="text-xs text-secondary block mb-1">Contact Hint (optional)</label>
        <input
          type="text"
          value={contactHint}
          onChange={(e) => setContactHint(e.target.value)}
          placeholder="Email, phone, or handle for verification"
          className="input w-full"
          disabled={loading}
        />
        <p className="text-xs text-tertiary mt-1">
          Helps others verify it's really you when they receive your trust invite.
        </p>
      </div>

      <div className="mb-3">
        <label className="text-xs text-secondary block mb-1">Birthday MM-DD (optional)</label>
        <input
          type="text"
          value={birthday}
          onChange={(e) => setBirthday(e.target.value)}
          placeholder="03-15"
          className="input w-full"
          disabled={loading}
        />
        <p className="text-xs text-tertiary mt-1">
          For peer verification when connecting.
        </p>
      </div>

      {error && <p className="text-gruvbox-red text-sm mt-3">{error}</p>}

      <div className="flex gap-2 mt-4">
        <button
          onClick={handleSave}
          disabled={loading || !displayName.trim()}
          className="btn-primary flex-1 text-center"
        >
          {loading ? 'Saving...' : (existingCard ? 'Update & Continue' : 'Save & Continue')}
        </button>
        <button onClick={onSkip} className="btn-secondary flex-1 text-center">
          Skip
        </button>
      </div>
    </div>
  )
}
