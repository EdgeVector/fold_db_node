import { useState, useEffect, useCallback } from 'react'
import {
  listTrustGrants,
  grantTrust,
  revokeTrust,
  setTrustOverride,
  resolveTrustDistance,
  getAuditLog,
} from '../../api/clients/trustClient'

function TrustTab({ onResult }) {
  const [grants, setGrants] = useState([])
  const [auditEvents, setAuditEvents] = useState([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)
  const [activeSection, setActiveSection] = useState('grants')

  // Grant form
  const [newPublicKey, setNewPublicKey] = useState('')
  const [newDistance, setNewDistance] = useState('')
  const [granting, setGranting] = useState(false)

  // Override form
  const [overrideKey, setOverrideKey] = useState('')
  const [overrideDistance, setOverrideDistance] = useState('')
  const [settingOverride, setSettingOverride] = useState(false)

  // Resolve form
  const [resolveKey, setResolveKey] = useState('')
  const [resolveResult, setResolveResult] = useState(null)
  const [resolving, setResolving] = useState(false)

  // Revoke state
  const [revoking, setRevoking] = useState(null)

  const fetchGrants = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const response = await listTrustGrants()
      if (response.success && response.data) {
        setGrants(response.data.grants || [])
      } else {
        setError(response.error || 'Failed to load trust grants')
      }
    } catch (err) {
      setError(err.message || 'Failed to load trust grants')
    } finally {
      setLoading(false)
    }
  }, [])

  const fetchAuditLog = useCallback(async () => {
    try {
      const response = await getAuditLog(50)
      if (response.success && response.data) {
        setAuditEvents(response.data.events || [])
      }
    } catch {
      // Audit log is supplementary, don't block on errors
    }
  }, [])

  useEffect(() => {
    fetchGrants()
    fetchAuditLog()
  }, [fetchGrants, fetchAuditLog])

  const handleGrant = async (e) => {
    e.preventDefault()
    if (!newPublicKey.trim() || newDistance === '') return
    const dist = parseInt(newDistance, 10)
    if (isNaN(dist) || dist < 0) {
      setError('Distance must be a non-negative integer')
      return
    }
    setGranting(true)
    setError(null)
    try {
      const response = await grantTrust(newPublicKey.trim(), dist)
      if (response.success) {
        setNewPublicKey('')
        setNewDistance('')
        await fetchGrants()
        await fetchAuditLog()
        if (onResult) onResult({ success: true, data: { message: 'Trust granted' } })
      } else {
        setError(response.error || 'Failed to grant trust')
      }
    } catch (err) {
      setError(err.message || 'Failed to grant trust')
    } finally {
      setGranting(false)
    }
  }

  const handleRevoke = async (publicKey) => {
    setRevoking(publicKey)
    setError(null)
    try {
      const response = await revokeTrust(publicKey)
      if (response.success) {
        await fetchGrants()
        await fetchAuditLog()
        if (onResult) onResult({ success: true, data: { message: 'Trust revoked' } })
      } else {
        setError(response.error || 'Failed to revoke trust')
      }
    } catch (err) {
      setError(err.message || 'Failed to revoke trust')
    } finally {
      setRevoking(null)
    }
  }

  const handleOverride = async (e) => {
    e.preventDefault()
    if (!overrideKey.trim() || overrideDistance === '') return
    const dist = parseInt(overrideDistance, 10)
    if (isNaN(dist) || dist < 0) {
      setError('Distance must be a non-negative integer')
      return
    }
    setSettingOverride(true)
    setError(null)
    try {
      const response = await setTrustOverride(overrideKey.trim(), dist)
      if (response.success) {
        setOverrideKey('')
        setOverrideDistance('')
        await fetchGrants()
        if (onResult) onResult({ success: true, data: { message: 'Override set' } })
      } else {
        setError(response.error || 'Failed to set override')
      }
    } catch (err) {
      setError(err.message || 'Failed to set override')
    } finally {
      setSettingOverride(false)
    }
  }

  const handleResolve = async (e) => {
    e.preventDefault()
    if (!resolveKey.trim()) return
    setResolving(true)
    setResolveResult(null)
    setError(null)
    try {
      const response = await resolveTrustDistance(resolveKey.trim())
      if (response.success && response.data) {
        setResolveResult(response.data)
      } else {
        setError(response.error || 'Failed to resolve trust distance')
      }
    } catch (err) {
      setError(err.message || 'Failed to resolve trust distance')
    } finally {
      setResolving(false)
    }
  }

  const truncateKey = (key) => {
    if (!key) return ''
    if (key.length <= 20) return key
    return `${key.slice(0, 10)}...${key.slice(-10)}`
  }

  const formatTimestamp = (isoString) => {
    try {
      return new Date(isoString).toLocaleString()
    } catch {
      return isoString
    }
  }

  const formatAuditAction = (action) => {
    if (!action) return 'Unknown'
    if (action.TrustGrant) return `Grant trust to ${truncateKey(action.TrustGrant.user_id)} at distance ${action.TrustGrant.distance}`
    if (action.TrustRevoke) return `Revoke trust for ${truncateKey(action.TrustRevoke.user_id)}`
    if (action.Read) return `Read ${action.Read.schema_name} [${action.Read.fields?.join(', ')}]`
    if (action.Write) return `Write ${action.Write.schema_name} [${action.Write.fields?.join(', ')}]`
    if (action.AccessDenied) return `Access denied: ${action.AccessDenied.schema_name} — ${action.AccessDenied.reason}`
    return JSON.stringify(action)
  }

  return (
    <div className="p-6 max-w-4xl mx-auto">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h2 className="text-lg font-medium text-primary">Trust Graph</h2>
          <p className="text-sm text-secondary mt-1">
            Manage which public keys your node trusts and at what distance.
            Trust distance controls read/write access to your data.
          </p>
        </div>
        <button
          className="btn btn-sm"
          onClick={() => { fetchGrants(); fetchAuditLog() }}
          disabled={loading}
        >
          Refresh
        </button>
      </div>

      {/* Section tabs */}
      <div className="flex gap-1 mb-6 border-b border-border">
        {[
          { id: 'grants', label: 'Trust Grants' },
          { id: 'resolve', label: 'Resolve Distance' },
          { id: 'audit', label: 'Audit Log' },
        ].map(({ id, label }) => (
          <button
            key={id}
            className={`px-4 py-2 text-sm border-b-2 transition-colors ${
              activeSection === id
                ? 'border-gruvbox-blue text-gruvbox-blue font-medium'
                : 'border-transparent text-secondary hover:text-primary'
            }`}
            onClick={() => setActiveSection(id)}
          >
            {label}
          </button>
        ))}
      </div>

      {/* Error */}
      {error && (
        <div className="card card-error mb-4">
          <p className="text-sm">{error}</p>
          <button
            className="text-xs underline mt-1"
            onClick={() => setError(null)}
          >
            Dismiss
          </button>
        </div>
      )}

      {/* === GRANTS SECTION === */}
      {activeSection === 'grants' && (
        <>
          {/* Grant trust form */}
          <div className="border border-border rounded-lg p-4 mb-6 bg-surface">
            <h3 className="text-sm font-medium text-primary mb-3">Grant Trust</h3>
            <form onSubmit={handleGrant} className="flex gap-3 items-end">
              <div className="flex-1">
                <label className="block text-xs text-secondary mb-1">Public Key</label>
                <input
                  className="input w-full"
                  type="text"
                  placeholder="Base64-encoded public key..."
                  value={newPublicKey}
                  onChange={(e) => setNewPublicKey(e.target.value)}
                />
              </div>
              <div className="w-32">
                <label className="block text-xs text-secondary mb-1">Distance</label>
                <input
                  className="input w-full"
                  type="number"
                  min="0"
                  placeholder="1"
                  value={newDistance}
                  onChange={(e) => setNewDistance(e.target.value)}
                />
              </div>
              <button
                type="submit"
                className="btn"
                disabled={granting || !newPublicKey.trim() || newDistance === ''}
              >
                {granting ? 'Granting...' : 'Grant'}
              </button>
            </form>
          </div>

          {/* Override form */}
          <div className="border border-border rounded-lg p-4 mb-6 bg-surface">
            <h3 className="text-sm font-medium text-primary mb-1">Set Distance Override</h3>
            <p className="text-xs text-secondary mb-3">
              Overrides take precedence over graph-computed shortest path.
            </p>
            <form onSubmit={handleOverride} className="flex gap-3 items-end">
              <div className="flex-1">
                <label className="block text-xs text-secondary mb-1">Public Key</label>
                <input
                  className="input w-full"
                  type="text"
                  placeholder="Base64-encoded public key..."
                  value={overrideKey}
                  onChange={(e) => setOverrideKey(e.target.value)}
                />
              </div>
              <div className="w-32">
                <label className="block text-xs text-secondary mb-1">Distance</label>
                <input
                  className="input w-full"
                  type="number"
                  min="0"
                  placeholder="1"
                  value={overrideDistance}
                  onChange={(e) => setOverrideDistance(e.target.value)}
                />
              </div>
              <button
                type="submit"
                className="btn"
                disabled={settingOverride || !overrideKey.trim() || overrideDistance === ''}
              >
                {settingOverride ? 'Setting...' : 'Set Override'}
              </button>
            </form>
          </div>

          {/* Loading */}
          {loading && (
            <div className="text-center py-12">
              <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin mx-auto mb-3" />
              <p className="text-secondary text-sm">Loading trust grants...</p>
            </div>
          )}

          {/* Empty state */}
          {!loading && grants.length === 0 && (
            <div className="text-center py-12 border border-border rounded-lg">
              <p className="text-secondary text-lg mb-2">No trust grants</p>
              <p className="text-tertiary text-sm">
                Your node does not trust any other public keys yet.
                Use the form above to grant trust.
              </p>
            </div>
          )}

          {/* Grants list */}
          {!loading && grants.length > 0 && (
            <>
              <p className="text-sm text-secondary mb-3">
                {grants.length} trusted key{grants.length !== 1 ? 's' : ''}
              </p>
              <div className="space-y-2">
                {grants.map((grant) => (
                  <div
                    key={grant.public_key}
                    className="border border-border rounded-lg p-4 bg-surface flex items-center justify-between gap-4"
                  >
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-3">
                        <code
                          className="text-sm text-primary break-all"
                          title={grant.public_key}
                        >
                          {truncateKey(grant.public_key)}
                        </code>
                        <span className="badge badge-info text-xs flex-shrink-0">
                          distance: {grant.distance}
                        </span>
                      </div>
                    </div>
                    <button
                      className="btn btn-sm text-gruvbox-red border-gruvbox-red/30 hover:bg-gruvbox-red/10"
                      onClick={() => handleRevoke(grant.public_key)}
                      disabled={revoking === grant.public_key}
                    >
                      {revoking === grant.public_key ? 'Revoking...' : 'Revoke'}
                    </button>
                  </div>
                ))}
              </div>
            </>
          )}
        </>
      )}

      {/* === RESOLVE SECTION === */}
      {activeSection === 'resolve' && (
        <div>
          <div className="border border-border rounded-lg p-4 mb-6 bg-surface">
            <h3 className="text-sm font-medium text-primary mb-1">Resolve Trust Distance</h3>
            <p className="text-xs text-secondary mb-3">
              Compute the effective trust distance for a public key, considering
              graph paths and overrides.
            </p>
            <form onSubmit={handleResolve} className="flex gap-3 items-end">
              <div className="flex-1">
                <label className="block text-xs text-secondary mb-1">Public Key</label>
                <input
                  className="input w-full"
                  type="text"
                  placeholder="Base64-encoded public key..."
                  value={resolveKey}
                  onChange={(e) => setResolveKey(e.target.value)}
                />
              </div>
              <button
                type="submit"
                className="btn"
                disabled={resolving || !resolveKey.trim()}
              >
                {resolving ? 'Resolving...' : 'Resolve'}
              </button>
            </form>
          </div>

          {resolveResult && (
            <div className="border border-border rounded-lg p-4 bg-surface">
              <h3 className="text-sm font-medium text-primary mb-3">Result</h3>
              <div className="flex items-center gap-3">
                <code className="text-sm text-primary" title={resolveResult.public_key}>
                  {truncateKey(resolveResult.public_key)}
                </code>
                {resolveResult.distance !== null && resolveResult.distance !== undefined ? (
                  <span className="badge badge-success text-xs">
                    distance: {resolveResult.distance}
                  </span>
                ) : (
                  <span className="badge badge-error text-xs">
                    unreachable
                  </span>
                )}
              </div>
            </div>
          )}
        </div>
      )}

      {/* === AUDIT SECTION === */}
      {activeSection === 'audit' && (
        <div>
          {auditEvents.length === 0 && (
            <div className="text-center py-12 border border-border rounded-lg">
              <p className="text-secondary text-lg mb-2">No audit events</p>
              <p className="text-tertiary text-sm">
                Trust operations will appear here as they occur.
              </p>
            </div>
          )}

          {auditEvents.length > 0 && (
            <div className="space-y-2">
              {auditEvents.map((event, idx) => (
                <div
                  key={event.id || idx}
                  className="border border-border rounded-lg p-3 bg-surface"
                >
                  <div className="flex items-start justify-between gap-3">
                    <div className="flex-1 min-w-0">
                      <p className="text-sm text-primary">
                        {formatAuditAction(event.action)}
                      </p>
                      <div className="flex items-center gap-3 mt-1">
                        <span className="text-xs text-tertiary">
                          {formatTimestamp(event.timestamp)}
                        </span>
                        {event.trust_distance !== null && event.trust_distance !== undefined && (
                          <span className="text-xs text-secondary">
                            trust distance: {event.trust_distance}
                          </span>
                        )}
                      </div>
                    </div>
                    <span className={`badge text-xs ${event.decision_granted ? 'badge-success' : 'badge-warning'}`}>
                      {event.decision_granted ? 'granted' : 'denied'}
                    </span>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

export default TrustTab
