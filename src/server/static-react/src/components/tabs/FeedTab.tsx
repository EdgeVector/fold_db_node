import { useState, useCallback } from 'react'
import { useAppSelector } from '../../store/hooks'
import { createApiClient } from '../../api/core/client'
import { API_ENDPOINTS } from '../../api/endpoints'

const apiClient = createApiClient()

function relativeTime(timestamp) {
  const now = Date.now()
  const then = new Date(timestamp).getTime()
  const seconds = Math.floor((now - then) / 1000)

  if (seconds < 60) return 'just now'
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ago`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h ago`
  const days = Math.floor(hours / 24)
  if (days < 30) return `${days}d ago`
  const months = Math.floor(days / 30)
  return `${months}mo ago`
}

export default function FeedTab() {
  const { user } = useAppSelector(state => state.auth)
  const [items, setItems] = useState([])
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState(null)
  const [fetched, setFetched] = useState(false)
  const [friendInput, setFriendInput] = useState('')
  const [keyCopied, setKeyCopied] = useState(false)

  const handleCopyKey = async () => {
    if (user?.id) {
      await navigator.clipboard.writeText(user.id)
      setKeyCopied(true)
      setTimeout(() => setKeyCopied(false), 2000)
    }
  }

  const fetchFeed = useCallback(async () => {
    const hashes = friendInput
      .split('\n')
      .map(s => s.trim())
      .filter(Boolean)

    if (hashes.length === 0) {
      setError('Add at least one friend\'s key to load the feed.')
      return
    }

    setLoading(true)
    setError(null)
    try {
      const response = await apiClient.post(API_ENDPOINTS.SOCIAL_FEED, {
        friend_hashes: hashes,
        limit: 50,
      })

      if (!response.success) {
        throw new Error(response.error || 'Failed to fetch feed')
      }

      setItems(response.data?.items || [])
      setTotal(response.data?.total || 0)
      setFetched(true)
    } catch (err) {
      setError(err.message || 'Network error')
    } finally {
      setLoading(false)
    }
  }, [friendInput])

  return (
    <div className="max-w-5xl mx-auto space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-semibold text-primary">Social Feed</h2>
        {fetched && (
          <span className="text-sm text-secondary">
            {items.length} of {total} items
          </span>
        )}
      </div>

      {/* Share your key */}
      <div className="card p-4 space-y-2">
        <div className="flex items-center justify-between">
          <div>
            <h3 className="text-sm font-medium text-primary">Your node key</h3>
            <p className="text-xs text-tertiary mt-0.5">Share this with friends so they can add you to their feed</p>
          </div>
          <button
            onClick={handleCopyKey}
            className={`btn text-sm px-3 py-1.5 ${keyCopied ? 'btn-primary' : 'btn-secondary'}`}
          >
            {keyCopied ? 'Copied!' : 'Copy key'}
          </button>
        </div>
        {user?.id && (
          <div className="bg-gruvbox-elevated rounded px-3 py-2 font-mono text-xs text-secondary break-all select-all">
            {user.id}
          </div>
        )}
      </div>

      {/* Add friends */}
      <div className="card p-4 space-y-3">
        <div>
          <h3 className="text-sm font-medium text-primary">Friends</h3>
          <p className="text-xs text-tertiary mt-0.5">Paste node keys from friends to see their shared content</p>
        </div>
        <textarea
          className="textarea w-full"
          rows={3}
          placeholder="Paste friend node keys, one per line..."
          value={friendInput}
          onChange={e => setFriendInput(e.target.value)}
        />
        <button
          className="btn btn-primary"
          onClick={fetchFeed}
          disabled={loading}
        >
          {loading ? 'Loading...' : 'Load Feed'}
        </button>
      </div>

      {/* Error state */}
      {error && (
        <div className="card card-error p-4">
          <p className="text-gruvbox-red text-sm">{error}</p>
        </div>
      )}

      {/* Loading state */}
      {loading && (
        <div className="flex items-center justify-center py-12">
          <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin" />
        </div>
      )}

      {/* Empty state */}
      {fetched && !loading && items.length === 0 && !error && (
        <div className="card p-8 text-center">
          <p className="text-secondary text-sm">No items in your feed.</p>
        </div>
      )}

      {/* Feed grid */}
      {items.length > 0 && !loading && (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {items.map((item, idx) => (
            <div key={item.key?.range || idx} className="card overflow-hidden">
              {item.fields?.photo_url && (
                <img
                  src={item.fields.photo_url}
                  alt={item.fields?.caption || 'Photo'}
                  className="w-full h-48 object-cover"
                  loading="lazy"
                />
              )}
              <div className="p-3 space-y-1">
                {item.fields?.caption && (
                  <p className="text-primary text-sm truncate">
                    {item.fields.caption}
                  </p>
                )}
                <div className="flex items-center justify-between text-xs text-secondary">
                  <span className="truncate max-w-[60%]">
                    {item.fields?.author_name || item.author}
                  </span>
                  <span>{relativeTime(item.timestamp)}</span>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
