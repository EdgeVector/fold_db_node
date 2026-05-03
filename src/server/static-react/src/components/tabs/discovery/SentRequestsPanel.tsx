import { useCallback, useEffect, useState } from 'react'
import { discoveryClient } from '../../../api/clients/discoveryClient'

export default function SentRequestsPanel() {
  const [requests, setRequests] = useState([])
  const [loading, setLoading] = useState(true)

  const fetchRequests = useCallback(async () => {
    try {
      const res = await discoveryClient.getSentRequests()
      if (res.success) {
        setRequests(res.data?.requests || [])
      }
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { fetchRequests() }, [fetchRequests])

  if (loading) return <p className="text-secondary text-sm">Loading sent requests...</p>

  if (requests.length === 0) {
    return (
      <div className="card p-6 text-center rounded">
        <p className="text-secondary text-sm">No sent connection requests.</p>
        <p className="text-tertiary text-xs mt-1">
          When you send a connection request from search results or similar profiles, it will appear here.
        </p>
      </div>
    )
  }

  return (
    <div className="space-y-2">
      <div className="text-xs text-secondary">
        {requests.length} request{requests.length !== 1 ? 's' : ''} sent
      </div>
      {requests.map(r => (
        <div key={r.request_id} className="card rounded p-3 space-y-1">
          <div className="flex items-center gap-2 text-xs">
            <span className={`badge ${
              r.status === 'pending' ? 'badge-warning' :
              r.status === 'accepted' ? 'badge-success' : 'badge-error'
            }`}>
              {r.status}
            </span>
            <span className="text-secondary">
              {new Date(r.created_at).toLocaleDateString()}
            </span>
          </div>
          <p className="text-sm text-primary">{r.message}</p>
          <div className="text-xs text-tertiary font-mono truncate">
            to: {r.target_pseudonym}
          </div>
        </div>
      ))}
      <button onClick={fetchRequests} className="btn-secondary btn-sm">
        Refresh
      </button>
    </div>
  )
}
