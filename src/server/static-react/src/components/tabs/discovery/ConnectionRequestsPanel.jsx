import { useCallback, useEffect, useState } from 'react'
import { discoveryClient } from '../../../api/clients/discoveryClient'
import { listSharingRoles } from '../../../api/clients/trustClient'
import { toErrorMessage } from '../../../utils/schemaUtils'

export default function ConnectionRequestsPanel({ onResult }) {
  const [requests, setRequests] = useState([])
  const [loading, setLoading] = useState(true)
  const [responding, setResponding] = useState(null)
  const [availableRoles, setAvailableRoles] = useState({})
  const [selectedRoles, setSelectedRoles] = useState({})

  const fetchRequests = useCallback(async () => {
    try {
      const res = await discoveryClient.getConnectionRequests()
      if (res.success) {
        setRequests(res.data?.requests || [])
      }
    } finally {
      setLoading(false)
    }
  }, [])

  const fetchRoles = useCallback(async () => {
    try {
      const response = await listSharingRoles()
      if (response.success && response.data) {
        setAvailableRoles(response.data.roles || {})
      }
    } catch (err) {
      console.error('Failed to load sharing roles:', err)
    }
  }, [])

  useEffect(() => { fetchRequests(); fetchRoles() }, [fetchRequests, fetchRoles])

  const handleRespond = async (requestId, action) => {
    setResponding(requestId)
    try {
      const role = action === 'accept' ? (selectedRoles[requestId] || 'acquaintance') : undefined
      const res = await discoveryClient.respondToRequest(requestId, action, undefined, role)
      if (res.success) {
        setRequests(prev =>
          prev.map(r => r.request_id === requestId ? res.data.request : r)
        )
        onResult({ success: true, data: { message: `Connection ${action}ed` } })
      } else {
        onResult({ error: res.error || `Failed to ${action}` })
      }
    } catch (e) {
      onResult({ error: toErrorMessage(e) || 'Network error' })
    } finally {
      setResponding(null)
    }
  }

  if (loading) return <p className="text-secondary text-sm">Loading connection requests...</p>

  const pending = requests.filter(r => r.status === 'pending')
  const responded = requests.filter(r => r.status !== 'pending')

  return (
    <div className="space-y-4">
      {pending.length === 0 && responded.length === 0 && (
        <div className="card p-6 text-center rounded">
          <p className="text-secondary text-sm">No connection requests yet.</p>
          <p className="text-tertiary text-xs mt-1">
            When someone discovers your data and wants to connect, their requests will appear here.
          </p>
        </div>
      )}

      {pending.length > 0 && (
        <div className="space-y-2">
          <div className="text-xs text-secondary font-semibold">
            Pending ({pending.length})
          </div>
          {pending.map(r => (
            <div key={r.request_id} className="card rounded p-4 space-y-2 border-l-2 border-gruvbox-yellow">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2 text-xs">
                  <span className="badge badge-warning">pending</span>
                  <span className="text-secondary">
                    {new Date(r.created_at).toLocaleDateString()}
                  </span>
                </div>
                <div className="flex gap-2 items-center">
                  <select
                    className="input input-sm text-xs"
                    value={selectedRoles[r.request_id] || 'acquaintance'}
                    onChange={(e) => setSelectedRoles(prev => ({ ...prev, [r.request_id]: e.target.value }))}
                  >
                    {Object.values(availableRoles).length > 0
                      ? Object.values(availableRoles).map((role) => (
                          <option key={role.name} value={role.name}>{role.name.replace(/_/g, ' ')}</option>
                        ))
                      : ['acquaintance', 'friend', 'close_friend', 'family', 'trainer', 'doctor', 'financial_advisor'].map(r => (
                          <option key={r} value={r}>{r.replace(/_/g, ' ')}</option>
                        ))
                    }
                  </select>
                  {!r.referral_query_id && (!r.mutual_contacts || r.mutual_contacts.length === 0) && (
                    <button
                      type="button"
                      className="btn btn-sm"
                      onClick={async () => {
                        try {
                          await discoveryClient.checkNetwork(r.request_id);
                          fetchRequests();
                        } catch (e) {
                          console.error('Check network failed:', e);
                        }
                      }}
                    >
                      Check network
                    </button>
                  )}
                  <button
                    onClick={() => handleRespond(r.request_id, 'accept')}
                    disabled={responding === r.request_id}
                    className="btn-primary btn-sm"
                  >
                    {responding === r.request_id ? '...' : 'Accept'}
                  </button>
                  <button
                    onClick={() => handleRespond(r.request_id, 'decline')}
                    disabled={responding === r.request_id}
                    className="btn-secondary btn-sm text-gruvbox-red"
                  >
                    Decline
                  </button>
                </div>
              </div>
              <p className="text-sm text-primary">{r.message}</p>
              {/* Mutual contacts (instant, from network intersection) */}
              {r.mutual_contacts && r.mutual_contacts.length > 0 && (
                <div className="mt-2 p-2 bg-gruvbox-green/10 rounded border border-gruvbox-green/30">
                  <p className="text-xs font-medium text-gruvbox-green mb-1">
                    {r.mutual_contacts.length} mutual contact{r.mutual_contacts.length !== 1 ? 's' : ''}:
                  </p>
                  <div className="flex flex-wrap gap-1">
                    {r.mutual_contacts.map((mc, i) => (
                      <span key={i} className="px-2 py-0.5 text-xs rounded-full bg-gruvbox-green/20 text-gruvbox-green font-medium">
                        {mc.display_name}
                      </span>
                    ))}
                  </div>
                </div>
              )}
              {/* Vouches from trusted contacts */}
              {r.vouches && r.vouches.length > 0 && (
                <div className="mt-2 space-y-1">
                  {r.vouches.map((v, i) => (
                    <div key={i} className="text-xs flex items-center gap-1 text-gruvbox-green">
                      <span className="font-semibold">{v.voucher_display_name}</span>
                      <span className="text-secondary">knows this person as</span>
                      <span className="font-semibold">&ldquo;{v.known_as}&rdquo;</span>
                    </div>
                  ))}
                </div>
              )}
              {/* Referral query progress */}
              {r.referral_query_id && (!r.vouches || r.vouches.length === 0) && (
                <p className="text-xs text-secondary mt-1">
                  Checking network... (queried {r.referral_contacts_queried} contacts)
                </p>
              )}
              <div className="text-xs text-tertiary font-mono truncate">
                from: {r.sender_pseudonym}
              </div>
            </div>
          ))}
        </div>
      )}

      {responded.length > 0 && (
        <div className="space-y-2">
          <div className="text-xs text-secondary font-semibold">
            History ({responded.length})
          </div>
          {responded.map(r => (
            <div key={r.request_id} className="card rounded p-3 space-y-1 opacity-75">
              <div className="flex items-center gap-2 text-xs">
                <span className={`badge ${
                  r.status === 'accept' ? 'badge-success' : 'badge-error'
                }`}>
                  {r.status === 'accept' ? 'accepted' : 'declined'}
                </span>
                <span className="text-secondary">
                  {r.responded_at ? new Date(r.responded_at).toLocaleDateString() : ''}
                </span>
              </div>
              <p className="text-sm text-secondary">{r.message}</p>
              <div className="text-xs text-tertiary font-mono truncate">
                from: {r.sender_pseudonym}
              </div>
            </div>
          ))}
        </div>
      )}

      <button onClick={fetchRequests} className="btn-secondary btn-sm">
        Refresh
      </button>
    </div>
  )
}
