import { useCallback, useEffect, useState } from 'react'
import { discoveryClient } from '../../../api/clients/discoveryClient'
import { toErrorMessage } from '../../../utils/schemaUtils'
import LocalModeNotice from './LocalModeNotice'
import { isLocalModeError } from './discoveryUtils'

export default function InterestsPanel({ onResult }) {
  const [profile, setProfile] = useState(null)
  const [loading, setLoading] = useState(true)
  const [detecting, setDetecting] = useState(false)
  const [loadError, setLoadError] = useState(null)

  const loadInterests = useCallback(async () => {
    try {
      const res = await discoveryClient.getInterests()
      if (res.success) {
        setProfile(res.data)
        setLoadError(null)
      } else {
        setLoadError(res.error || 'Failed to load interests')
      }
    } catch (e) {
      setLoadError(toErrorMessage(e) || 'Network error')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { loadInterests() }, [loadInterests])

  const handleToggle = async (categoryName, enabled) => {
    try {
      const res = await discoveryClient.toggleInterest(categoryName, enabled)
      if (res.success) {
        setProfile(res.data)
      } else {
        onResult({ error: res.error || 'Toggle failed' })
      }
    } catch (e) {
      onResult({ error: toErrorMessage(e) || 'Network error' })
    }
  }

  const handleDetect = async () => {
    setDetecting(true)
    try {
      const res = await discoveryClient.detectInterests()
      if (res.success) {
        setProfile(res.data)
        onResult({ success: true, data: { message: `Detected ${res.data?.categories?.length || 0} interest categories` } })
      } else {
        onResult({ error: res.error || 'Detection failed' })
      }
    } catch (e) {
      onResult({ error: toErrorMessage(e) || 'Network error' })
    } finally {
      setDetecting(false)
    }
  }

  if (loading) return <p className="text-secondary text-sm">Loading interests...</p>

  if (loadError) {
    if (isLocalModeError(loadError)) return <LocalModeNotice />
    return (
      <div className="space-y-3">
        <div className="text-sm text-gruvbox-red">{loadError}</div>
        <button onClick={loadInterests} className="btn-secondary btn-sm">Retry</button>
      </div>
    )
  }

  const categories = profile?.categories || []
  const hasProfile = profile && profile.seed_version > 0

  return (
    <div className="space-y-4">
      {hasProfile && (
        <div className="text-xs text-tertiary">
          {profile.total_embeddings_scanned} items scanned &middot;{' '}
          {profile.unmatched_count} uncategorized &middot;{' '}
          detected {new Date(profile.detected_at).toLocaleDateString()}
        </div>
      )}

      <p className="text-sm text-secondary max-w-2xl">
        Topics we spotted in your data. Click any to enable or disable it — only
        enabled topics affect the people we match you with.
      </p>

      {categories.length > 0 ? (
        <div className="flex flex-wrap gap-2">
          {categories.map(cat => (
            <button
              key={cat.name}
              onClick={() => handleToggle(cat.name, !cat.enabled)}
              className={`px-3 py-2 rounded border text-sm transition-colors ${
                cat.enabled
                  ? 'bg-surface-secondary border-border text-primary'
                  : 'bg-transparent border-border text-tertiary'
              }`}
            >
              <span className="font-medium">{cat.name}</span>
              <span className={`ml-2 text-xs ${cat.enabled ? 'text-secondary' : 'text-tertiary'}`}>
                {cat.count}
              </span>
            </button>
          ))}
        </div>
      ) : (
        <div className="card p-6 text-center">
          <p className="text-secondary text-sm">
            No topics detected yet. Import some data first, then click Detect to
            find the topics you care about.
          </p>
        </div>
      )}

      <button
        onClick={handleDetect}
        disabled={detecting}
        className="btn-secondary"
      >
        {detecting ? 'Detecting...' : hasProfile ? 'Re-detect topics' : 'Detect topics'}
      </button>
    </div>
  )
}
