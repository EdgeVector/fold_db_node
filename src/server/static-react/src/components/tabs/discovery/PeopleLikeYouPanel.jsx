import { useCallback, useEffect, useRef, useState } from 'react'
import { discoveryClient } from '../../../api/clients/discoveryClient'
import { toErrorMessage } from '../../../utils/schemaUtils'
import RoleSelect from './RoleSelect'
import LocalModeNotice from './LocalModeNotice'
import { isLocalModeError } from './discoveryUtils'

const REFRESH_INTERVAL_MS = 60_000

export default function PeopleLikeYouPanel({ onResult }) {
  const [profiles, setProfiles] = useState([])
  const [categoriesUsed, setCategoriesUsed] = useState(0)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)
  const [connectingTo, setConnectingTo] = useState(null)
  const [connectMessage, setConnectMessage] = useState('')
  const [connectRole, setConnectRole] = useState('acquaintance')
  const intervalRef = useRef(null)
  // Track whether we've hit a local-mode error to stop polling
  const localModeRef = useRef(false)

  const fetchProfiles = useCallback(async () => {
    try {
      const res = await discoveryClient.getSimilarProfiles()
      if (res.success) {
        setProfiles(res.data?.profiles || [])
        setCategoriesUsed(res.data?.user_categories_used || 0)
        setError(null)
      } else {
        const msg = res.error || 'Failed to load similar profiles'
        setError(msg)
        if (isLocalModeError(msg)) {
          localModeRef.current = true
          clearInterval(intervalRef.current)
        }
      }
    } catch (e) {
      const msg = toErrorMessage(e) || 'Network error'
      setError(msg)
      if (isLocalModeError(msg)) {
        localModeRef.current = true
        clearInterval(intervalRef.current)
      }
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    fetchProfiles().then(() => {
      // Only start polling if not in local mode
      if (!localModeRef.current) {
        intervalRef.current = setInterval(fetchProfiles, REFRESH_INTERVAL_MS)
      }
    })
    return () => clearInterval(intervalRef.current)
  }, [fetchProfiles])

  const handleConnect = async (pseudonym) => {
    if (!connectMessage.trim()) return
    try {
      const res = await discoveryClient.connect(pseudonym, connectMessage, connectRole !== 'acquaintance' ? connectRole : undefined)
      if (res.success) {
        setConnectingTo(null)
        setConnectMessage('')
        setConnectRole('acquaintance')
        onResult({ success: true, data: { message: 'Connection request sent' } })
      } else {
        onResult({ error: res.error || 'Connect failed' })
      }
    } catch (e) {
      onResult({ error: toErrorMessage(e) || 'Network error' })
    }
  }

  if (loading) return <p className="text-secondary text-sm">Finding people like you...</p>

  if (error) {
    if (isLocalModeError(error)) return <LocalModeNotice />
    return (
      <div className="space-y-3">
        <div className="text-sm text-gruvbox-red">{error}</div>
        <button onClick={fetchProfiles} className="btn-secondary btn-sm">Retry</button>
      </div>
    )
  }

  if (categoriesUsed === 0) {
    return (
      <div className="card p-8 text-center space-y-4 rounded">
        <h3 className="text-lg text-primary font-semibold">Opt into discovery first</h3>
        <p className="text-secondary text-sm max-w-md mx-auto">
          To find people with similar interests, you need to detect your interest categories
          and publish your embeddings to the network. Visit the <strong>Your Interests</strong> tab
          to get started.
        </p>
      </div>
    )
  }

  if (profiles.length === 0) {
    return (
      <div className="card p-8 text-center space-y-4 rounded">
        <h3 className="text-lg text-primary font-semibold">No matches yet</h3>
        <p className="text-secondary text-sm max-w-md mx-auto">
          We searched across {categoriesUsed} of your interest categories but haven't found
          similar profiles yet. As more people join the discovery network, matches will
          appear here automatically.
        </p>
        <div className="text-xs text-tertiary">Refreshes every 60 seconds</div>
      </div>
    )
  }

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <div className="text-xs text-secondary">
          {profiles.length} profile{profiles.length !== 1 ? 's' : ''} matched across {categoriesUsed} interest categories
        </div>
        <button onClick={fetchProfiles} className="btn-secondary btn-sm">Refresh</button>
      </div>

      {profiles.map(p => (
        <div key={p.pseudonym} className="card rounded p-4 space-y-2">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-3">
              <div className="flex items-center justify-center w-10 h-10 rounded-full bg-surface-secondary border border-border text-sm font-bold text-gruvbox-blue">
                {Math.round(p.match_percentage)}%
              </div>
              <div>
                <div className="text-sm text-primary font-medium">
                  {Math.round(p.match_percentage)}% match
                </div>
                <div className="text-xs text-secondary">
                  Top similarity: {(p.top_similarity * 100).toFixed(1)}%
                </div>
              </div>
            </div>
            {connectingTo === p.pseudonym ? (
              <div className="flex gap-1 items-center">
                <RoleSelect value={connectRole} onChange={setConnectRole} />
                <input
                  type="text"
                  value={connectMessage}
                  onChange={(e) => setConnectMessage(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && handleConnect(p.pseudonym)}
                  placeholder="Message..."
                  className="input text-xs w-48"
                />
                <button
                  onClick={() => handleConnect(p.pseudonym)}
                  disabled={!connectMessage.trim()}
                  className="btn-primary btn-sm"
                >
                  Send
                </button>
                <button
                  onClick={() => { setConnectingTo(null); setConnectMessage(''); setConnectRole('acquaintance') }}
                  className="btn-secondary btn-sm"
                >
                  Cancel
                </button>
              </div>
            ) : (
              <button
                onClick={() => setConnectingTo(p.pseudonym)}
                className="btn-primary btn-sm"
              >
                Connect
              </button>
            )}
          </div>

          <div className="flex flex-wrap gap-1.5">
            {p.shared_categories.map(cat => (
              <span key={cat} className="badge badge-info">{cat}</span>
            ))}
          </div>

          <div className="text-xs text-tertiary font-mono truncate">
            {p.pseudonym}
          </div>
        </div>
      ))}
    </div>
  )
}
