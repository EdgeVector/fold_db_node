import { useState, useEffect, useCallback } from 'react'
import { useApprovedSchemas } from '../../hooks/useApprovedSchemas.js'
import { discoveryClient } from '../../api/clients/discoveryClient'

function InterestTag({ label, selected, onToggle }) {
  return (
    <button
      type="button"
      onClick={onToggle}
      className={`px-3 py-1.5 text-xs border transition-colors ${
        selected
          ? 'border-gruvbox-green text-gruvbox-green bg-gruvbox-green/10'
          : 'border-border text-secondary hover:border-gruvbox-yellow hover:text-primary'
      }`}
    >
      {label}
    </button>
  )
}

const INTEREST_CATEGORIES = [
  'Personal Notes', 'Health & Fitness', 'Photos & Media',
  'Finance', 'Productivity', 'Education',
  'Contacts', 'Travel', 'Creative Writing',
  'Code & Development', 'Music', 'Recipes & Food',
]

export default function DiscoveryStep({ onNext, onSkip }) {
  const { approvedSchemas } = useApprovedSchemas()
  const [serviceAvailable, setServiceAvailable] = useState(null)
  const [interests, setInterests] = useState(new Set())
  const [optingIn, setOptingIn] = useState(false)
  const [done, setDone] = useState(false)
  const [error, setError] = useState(null)

  useEffect(() => {
    discoveryClient.listOptIns()
      .then(res => setServiceAvailable(res.success))
      .catch(() => setServiceAvailable(false))
  }, [])

  const toggleInterest = useCallback((label) => {
    setInterests(prev => {
      const next = new Set(prev)
      if (next.has(label)) next.delete(label)
      else next.add(label)
      return next
    })
  }, [])

  const handleOptIn = async () => {
    if (interests.size === 0) {
      onNext()
      return
    }
    setOptingIn(true)
    setError(null)

    // Opt in approved schemas whose inferred categories match selected interests
    const schemasToOptIn = (approvedSchemas || []).slice(0, 5) // limit to first 5
    let succeeded = 0
    let lastError = null
    for (const schema of schemasToOptIn) {
      try {
        const category = [...interests][0] // Use first interest as category
        await discoveryClient.optIn({
          schema_name: schema.name,
          category,
          include_preview: false,
        })
        succeeded++
      } catch (e) {
        lastError = e
      }
    }

    if (succeeded === 0 && schemasToOptIn.length > 0) {
      setError(`Failed to share schemas with the network: ${lastError?.message || 'Unknown error'}`)
      setOptingIn(false)
      return
    }

    setDone(true)
    setOptingIn(false)
  }

  if (serviceAvailable === null) {
    return <p className="text-secondary text-center py-6">Checking discovery service...</p>
  }

  if (!serviceAvailable) {
    return (
      <div>
        <h2 className="text-sm font-bold mb-1">
          <span className="text-gruvbox-orange">COMMUNITY</span>{' '}
          <span className="text-secondary">Join the network</span>
        </h2>
        <div className="card p-6 text-center mt-4">
          <p className="text-secondary text-sm">
            Discovery service is not configured. You can enable it later in Settings.
          </p>
        </div>
        <div className="flex gap-2 mt-4">
          <button onClick={onSkip} className="btn-primary flex-1 text-center">Continue</button>
        </div>
      </div>
    )
  }

  if (done) {
    return (
      <div>
        <h2 className="text-sm font-bold mb-1">
          <span className="text-gruvbox-green">COMMUNITY</span>{' '}
          <span className="text-secondary">You're in</span>
        </h2>
        <div className="card-success p-4 mt-4">
          <p className="text-primary">Your schemas have been shared with the discovery network.</p>
          <p className="text-xs text-secondary mt-2">
            Others with similar data can now find and connect with you. Manage in the Discovery tab.
          </p>
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
        <span className="text-gruvbox-orange">COMMUNITY</span>{' '}
        <span className="text-secondary">Join the discovery network</span>
      </h2>
      <p className="text-primary mb-1">
        Share your data types (not your data) so others with similar interests can find you.
      </p>
      <p className="text-xs text-secondary mb-4">
        Only schema structure is shared — your actual data never leaves your device.
      </p>

      <div className="mb-4">
        <p className="label">Select your interests</p>
        <div className="flex flex-wrap gap-2">
          {INTEREST_CATEGORIES.map(cat => (
            <InterestTag
              key={cat}
              label={cat}
              selected={interests.has(cat)}
              onToggle={() => toggleInterest(cat)}
            />
          ))}
        </div>
      </div>

      {interests.size > 0 && (
        <div className="card p-3 mb-4">
          <p className="text-xs text-secondary">
            Selected: <span className="text-gruvbox-green">{[...interests].join(', ')}</span>
          </p>
          {approvedSchemas && approvedSchemas.length > 0 && (
            <p className="text-xs text-secondary mt-1">
              {Math.min(approvedSchemas.length, 5)} schema{approvedSchemas.length !== 1 ? 's' : ''} will be shared with the network.
            </p>
          )}
        </div>
      )}

      {error && (
        <p className="text-gruvbox-red text-sm mb-4">{error}</p>
      )}

      <div className="flex gap-2 mt-4">
        {interests.size > 0 ? (
          <>
            <button
              onClick={handleOptIn}
              disabled={optingIn}
              className="btn-primary flex-1 text-center"
            >
              {optingIn ? 'Joining...' : 'Join & Continue'}
            </button>
            <button onClick={onSkip} className="btn-secondary flex-1 text-center">
              Skip
            </button>
          </>
        ) : (
          <button onClick={onSkip} className="btn-secondary flex-1 text-center">
            Skip
          </button>
        )}
      </div>
    </div>
  )
}
