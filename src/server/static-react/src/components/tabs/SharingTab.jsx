import { useState, useEffect, useCallback } from 'react'
import {
  listTrustGrants,
  grantTrust,
  revokeTrust,
  getAuditLog,
  getNodeInfo,
} from '../../api/clients/sharingClient'

const SECTIONS = ['trust', 'audit', 'node-info']

export default function SharingTab({ onResult }) {
  const [activeSection, setActiveSection] = useState('trust')

  return (
    <div className="space-y-4">
      {/* Section tabs */}
      <div className="flex gap-2 border-b border-border pb-2">
        {SECTIONS.map((section) => (
          <button
            key={section}
            onClick={() => setActiveSection(section)}
            className={`px-3 py-1.5 text-sm rounded-t transition-colors ${
              activeSection === section
                ? 'bg-gruvbox-elevated text-accent border-b-2 border-accent'
                : 'text-secondary hover:text-primary'
            }`}
          >
            {section === 'trust' && 'Trust Grants'}
            {section === 'audit' && 'Audit Log'}
            {section === 'node-info' && 'Your Node'}
          </button>
        ))}
      </div>

      {activeSection === 'trust' && <TrustSection onResult={onResult} />}
      {activeSection === 'audit' && <AuditSection />}
      {activeSection === 'node-info' && <NodeInfoSection />}
    </div>
  )
}

// ===== Trust Grants Section =====

function TrustSection({ onResult }) {
  const [grants, setGrants] = useState([])
  const [loading, setLoading] = useState(true)
  const [newKey, setNewKey] = useState('')
  const [newDistance, setNewDistance] = useState(1)

  const fetchGrants = useCallback(async () => {
    try {
      setLoading(true)
      const result = await listTrustGrants()
      setGrants(result)
    } catch (err) {
      onResult?.({ error: err.message })
    } finally {
      setLoading(false)
    }
  }, [onResult])

  useEffect(() => {
    fetchGrants()
  }, [fetchGrants])

  const handleGrant = async (e) => {
    e.preventDefault()
    if (!newKey.trim()) return
    try {
      await grantTrust(newKey.trim(), newDistance)
      setNewKey('')
      setNewDistance(1)
      onResult?.({ success: true, data: { message: 'Trust granted' } })
      fetchGrants()
    } catch (err) {
      onResult?.({ error: err.message })
    }
  }

  const handleRevoke = async (publicKey) => {
    try {
      await revokeTrust(publicKey)
      onResult?.({ success: true, data: { message: 'Trust revoked' } })
      fetchGrants()
    } catch (err) {
      onResult?.({ error: err.message })
    }
  }

  return (
    <div className="space-y-4">
      {/* Grant form */}
      <form onSubmit={handleGrant} className="bg-gruvbox-elevated rounded-lg p-4 space-y-3">
        <h3 className="text-sm font-semibold text-primary">Grant Trust</h3>
        <div className="flex gap-2">
          <input
            type="text"
            value={newKey}
            onChange={(e) => setNewKey(e.target.value)}
            placeholder="Public key (base64)"
            className="flex-1 bg-surface-primary border border-border rounded px-3 py-2 text-sm text-primary placeholder-secondary"
          />
          <input
            type="number"
            value={newDistance}
            onChange={(e) => setNewDistance(parseInt(e.target.value) || 1)}
            min={1}
            className="w-20 bg-surface-primary border border-border rounded px-3 py-2 text-sm text-primary"
          />
          <button
            type="submit"
            disabled={!newKey.trim()}
            className="px-4 py-2 bg-accent text-surface-primary rounded text-sm font-medium disabled:opacity-50 hover:bg-accent/80"
          >
            Grant
          </button>
        </div>
        <p className="text-xs text-secondary">
          Distance: lower = more trusted. 0 = owner, 1 = direct trust.
        </p>
      </form>

      {/* Grants list */}
      <div className="bg-gruvbox-elevated rounded-lg p-4">
        <h3 className="text-sm font-semibold text-primary mb-3">
          Active Trust Grants ({grants.length})
        </h3>
        {loading ? (
          <p className="text-secondary text-sm">Loading...</p>
        ) : grants.length === 0 ? (
          <p className="text-secondary text-sm">No trust grants yet.</p>
        ) : (
          <div className="space-y-2">
            {grants.map((grant) => (
              <div
                key={grant.public_key}
                className="flex items-center justify-between bg-surface-primary rounded px-3 py-2"
              >
                <div className="flex-1 min-w-0">
                  <code className="text-xs text-primary truncate block">
                    {grant.public_key}
                  </code>
                  <span className="text-xs text-secondary">
                    Distance: {grant.distance}
                  </span>
                </div>
                <button
                  onClick={() => handleRevoke(grant.public_key)}
                  className="ml-2 px-3 py-1 text-xs bg-red-900/30 text-red-400 rounded hover:bg-red-900/50"
                >
                  Revoke
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

// ===== Audit Log Section =====

function AuditSection() {
  const [events, setEvents] = useState([])
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    async function fetch() {
      try {
        const result = await getAuditLog(50)
        setEvents(result)
      } catch {
        // silently fail — audit may not have events yet
      } finally {
        setLoading(false)
      }
    }
    fetch()
  }, [])

  const formatAction = (action) => {
    if (action.Read) return `Read ${action.Read.schema_name} (${action.Read.fields?.length || 0} fields)`
    if (action.Write) return `Write ${action.Write.schema_name}`
    if (action.AccessDenied) return `Denied: ${action.AccessDenied.schema_name}`
    if (action.TrustGrant) return `Granted trust to ${action.TrustGrant.user_id?.slice(0, 16)}...`
    if (action.TrustRevoke) return `Revoked trust for ${action.TrustRevoke.user_id?.slice(0, 16)}...`
    return JSON.stringify(action)
  }

  return (
    <div className="bg-gruvbox-elevated rounded-lg p-4">
      <h3 className="text-sm font-semibold text-primary mb-3">
        Recent Access Events ({events.length})
      </h3>
      {loading ? (
        <p className="text-secondary text-sm">Loading...</p>
      ) : events.length === 0 ? (
        <p className="text-secondary text-sm">No audit events recorded yet.</p>
      ) : (
        <div className="space-y-1 max-h-96 overflow-y-auto">
          {[...events].reverse().map((event) => (
            <div
              key={event.id}
              className="flex items-center gap-3 px-3 py-2 text-xs bg-surface-primary rounded"
            >
              <span
                className={`w-2 h-2 rounded-full flex-shrink-0 ${
                  event.decision_granted ? 'bg-green-500' : 'bg-red-500'
                }`}
              />
              <span className="text-secondary w-36 flex-shrink-0">
                {new Date(event.timestamp).toLocaleString()}
              </span>
              <span className="text-primary flex-1 truncate">
                {formatAction(event.action)}
              </span>
              <code className="text-secondary truncate max-w-[120px]">
                {event.user_id?.slice(0, 12)}...
              </code>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

// ===== Node Info Section =====

function NodeInfoSection() {
  const [info, setInfo] = useState(null)
  const [loading, setLoading] = useState(true)
  const [copied, setCopied] = useState(null)

  useEffect(() => {
    async function fetch() {
      try {
        const result = await getNodeInfo()
        setInfo(result)
      } catch {
        // may fail if no user context
      } finally {
        setLoading(false)
      }
    }
    fetch()
  }, [])

  const copyToClipboard = (text, label) => {
    navigator.clipboard.writeText(text)
    setCopied(label)
    setTimeout(() => setCopied(null), 2000)
  }

  if (loading) return <p className="text-secondary text-sm">Loading...</p>
  if (!info) return <p className="text-secondary text-sm">Could not load node info.</p>

  return (
    <div className="space-y-4">
      <div className="bg-gruvbox-elevated rounded-lg p-4 space-y-3">
        <h3 className="text-sm font-semibold text-primary">Your Node</h3>

        <div>
          <label className="text-xs text-secondary block mb-1">Node ID</label>
          <div className="flex items-center gap-2">
            <code className="text-xs text-primary bg-surface-primary px-2 py-1 rounded flex-1 truncate">
              {info.node_id}
            </code>
            <button
              onClick={() => copyToClipboard(info.node_id, 'node_id')}
              className="px-2 py-1 text-xs bg-surface-primary border border-border rounded hover:bg-gruvbox-elevated"
            >
              {copied === 'node_id' ? 'Copied' : 'Copy'}
            </button>
          </div>
        </div>

        <div>
          <label className="text-xs text-secondary block mb-1">Public Key</label>
          <div className="flex items-center gap-2">
            <code className="text-xs text-primary bg-surface-primary px-2 py-1 rounded flex-1 truncate">
              {info.public_key}
            </code>
            <button
              onClick={() => copyToClipboard(info.public_key, 'public_key')}
              className="px-2 py-1 text-xs bg-surface-primary border border-border rounded hover:bg-gruvbox-elevated"
            >
              {copied === 'public_key' ? 'Copied' : 'Copy'}
            </button>
          </div>
        </div>

        <div>
          <label className="text-xs text-secondary block mb-1">
            Shared Schemas ({info.shared_schemas.length})
          </label>
          {info.shared_schemas.length === 0 ? (
            <p className="text-xs text-secondary">
              No schemas are shared yet. Set field access policies with read_max {'>'} 0 to share.
            </p>
          ) : (
            <div className="flex flex-wrap gap-1">
              {info.shared_schemas.map((name) => (
                <span
                  key={name}
                  className="px-2 py-0.5 text-xs bg-accent/20 text-accent rounded"
                >
                  {name}
                </span>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
