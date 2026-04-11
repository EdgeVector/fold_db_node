import { useCallback, useEffect, useRef, useState } from 'react'
import { useApprovedSchemas } from '../../hooks/useApprovedSchemas.js'
import { discoveryClient } from '../../api/clients/discoveryClient'
import { listSharingRoles } from '../../api/clients/trustClient'
import { toErrorMessage } from '../../utils/schemaUtils'

/** Derive a category from schema field_interest_categories.
 *  Returns the most common interest category across the schema's fields,
 *  falling back to schema name if no interest categories are assigned. */
function inferCategory(schema) {
  if (schema.field_interest_categories) {
    const counts = {}
    for (const cat of Object.values(schema.field_interest_categories)) {
      counts[cat] = (counts[cat] || 0) + 1
    }
    const sorted = Object.entries(counts).sort((a, b) => b[1] - a[1])
    if (sorted.length > 0) return sorted[0][0]
  }

  return schema.descriptive_name || schema.name?.replace(/([A-Z])/g, ' $1').trim().toLowerCase() || 'general'
}

/** Group schemas by their inferred category. */
function groupByCategory(schemas) {
  const groups = {}
  for (const s of schemas) {
    const cat = inferCategory(s)
    if (!groups[cat]) groups[cat] = []
    groups[cat].push(s)
  }
  return groups
}

/** Count fields across schemas in a category. */
function fieldCount(schemas) {
  let count = 0
  for (const s of schemas) {
    if (s.fields) count += Object.keys(s.fields).length
  }
  return count
}

/** Build a preview of what will be shared for a set of schemas. */
function buildPreviewItems(schemas) {
  const items = []
  for (const s of schemas) {
    if (!s.fields) continue
    for (const [fieldName, fieldDef] of Object.entries(s.fields)) {
      const type = fieldDef?.field_type || 'unknown'
      items.push({ schema: s.name, field: fieldName, type })
    }
  }
  return items
}

function ToggleSwitch({ enabled, onChange, disabled }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={enabled}
      disabled={disabled}
      onClick={() => onChange(!enabled)}
      className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
        disabled ? 'opacity-50 cursor-not-allowed' : 'cursor-pointer'
      } ${enabled ? 'bg-gruvbox-green' : 'bg-gruvbox-elevated border border-border'}`}
    >
      <span
        className={`inline-block h-3.5 w-3.5 rounded-full bg-primary transition-transform ${
          enabled ? 'translate-x-[18px]' : 'translate-x-[3px]'
        }`}
      />
    </button>
  )
}

function PrivacyGuarantees() {
  return (
    <div className="card-info p-3 rounded text-xs space-y-1.5">
      <div className="font-semibold text-gruvbox-blue">Privacy Guarantees</div>
      <ul className="space-y-1 text-secondary">
        <li>Only embedding vectors are shared — never raw text</li>
        <li>Each entry gets a unique pseudonym — your identity stays hidden</li>
        <li>Fields marked as sensitive are automatically excluded</li>
        <li>You can unpublish at any time to remove all shared data</li>
      </ul>
    </div>
  )
}

/** Map numeric trust tier to human-readable label. */
const TRUST_TIER_LABELS = {
  0: 'Public',
  1: 'Outer',
  2: 'Trusted',
  3: 'Inner',
  4: 'Owner',
}

function trustTierLabel(tier) {
  return TRUST_TIER_LABELS[tier] ?? `Tier ${tier}`
}

/** Inline role selector for the connect flow. */
function RoleSelect({ value, onChange }) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="input text-xs w-32"
    >
      <option value="acquaintance">Acquaintance</option>
      <option value="friend">Friend</option>
      <option value="inner_circle">Inner Circle</option>
    </select>
  )
}

function CategoryCard({
  category,
  schemas,
  optedInNames,
  publishedCategories,
  onToggle,
  toggling,
  expanded,
  onExpandToggle,
  publishFaces,
  onPublishFacesToggle,
}) {
  const allOptedIn = schemas.every(s => optedInNames.has(s.name))
  const someOptedIn = schemas.some(s => optedInNames.has(s.name))
  const isPublished = publishedCategories.has(category)
  const previewItems = buildPreviewItems(schemas)

  return (
    <div className="card rounded p-4 space-y-3">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <ToggleSwitch
            enabled={allOptedIn}
            onChange={(val) => onToggle(category, schemas, val)}
            disabled={toggling}
          />
          <div>
            <div className="flex items-center gap-2">
              <span className="font-semibold text-sm text-primary">{category}</span>
              {isPublished && (
                <span className="badge badge-success">published</span>
              )}
              {someOptedIn && !isPublished && (
                <span className="badge badge-warning">unpublished</span>
              )}
            </div>
            <div className="text-xs text-secondary mt-0.5">
              {schemas.length} schema{schemas.length !== 1 ? 's' : ''} &middot; {fieldCount(schemas)} field{fieldCount(schemas) !== 1 ? 's' : ''}
            </div>
          </div>
        </div>
        <button
          onClick={onExpandToggle}
          className="text-xs text-secondary hover:text-primary transition-colors"
        >
          {expanded ? 'Hide preview' : 'Show preview'}
        </button>
      </div>

      {expanded && (
        <div className="border-t border-border pt-3 space-y-2">
          <div className="text-xs text-secondary font-semibold">
            What will be shared:
          </div>
          {previewItems.length === 0 ? (
            <div className="text-xs text-tertiary">No fields detected</div>
          ) : (
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-1">
              {previewItems.map((item, i) => (
                <div key={i} className="flex items-center gap-2 text-xs">
                  <span className="text-gruvbox-blue font-mono">{item.field}</span>
                  <span className="text-tertiary">({item.type})</span>
                  <span className="text-tertiary">from {item.schema}</span>
                </div>
              ))}
            </div>
          )}
          <div className="text-xs text-tertiary mt-1">
            Embedding vectors of these fields will be published — raw text is never shared.
          </div>

          {/* Publish face embeddings opt-in */}
          <label className="flex items-center gap-2 mt-2 cursor-pointer">
            <input
              type="checkbox"
              checked={publishFaces}
              onChange={(e) => onPublishFacesToggle(category, e.target.checked)}
              disabled={toggling}
              className="accent-gruvbox-green"
            />
            <span className="text-xs text-secondary">
              Publish face embeddings (detected faces in photos will be searchable on the network)
            </span>
          </label>
        </div>
      )}
    </div>
  )
}

function EmptyState() {
  return (
    <div className="card p-8 text-center space-y-4 rounded">
      <div className="text-3xl">
        {/* Simple icon using unicode */}
        <span className="text-gruvbox-yellow">&#9776;</span>
      </div>
      <div>
        <h3 className="text-lg text-primary font-semibold">No data to discover yet</h3>
        <p className="text-secondary text-sm mt-2 max-w-md mx-auto">
          Ingest some data first using the Data tab. Once you have schemas with data,
          you can choose which categories to share on the discovery network.
        </p>
      </div>
      <div className="card-info p-3 rounded text-xs text-secondary max-w-sm mx-auto">
        Discovery lets others find your data by topic — without revealing your identity
        or the actual content. You stay in full control of what gets shared.
      </div>
    </div>
  )
}

function SearchPanel({ onResult }) {
  const [query, setQuery] = useState('')
  const [results, setResults] = useState([])
  const [searching, setSearching] = useState(false)
  const [error, setError] = useState(null)
  const [connectingTo, setConnectingTo] = useState(null)
  const [connectMessage, setConnectMessage] = useState('')
  const [connectRole, setConnectRole] = useState('acquaintance')

  const handleSearch = useCallback(async () => {
    if (!query.trim()) return
    setSearching(true)
    setError(null)
    try {
      const res = await discoveryClient.search(query)
      if (res.success) {
        setResults(res.data?.results || [])
        onResult({ success: true, data: res.data })
      } else {
        setError(res.error || 'Search failed')
        onResult({ error: res.error || 'Search failed' })
      }
    } catch (e) {
      const msg = toErrorMessage(e)
      setError(msg || 'Network error')
      onResult({ error: msg || 'Network error' })
    } finally {
      setSearching(false)
    }
  }, [query, onResult])

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

  return (
    <div className="space-y-3">
      <div className="flex gap-2">
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
          placeholder="Search the discovery network..."
          className="input flex-1"
        />
        <button onClick={handleSearch} disabled={searching || !query.trim()} className="btn-primary">
          {searching ? 'Searching...' : 'Search Network'}
        </button>
      </div>

      {error && <div className="text-sm text-gruvbox-red">{error}</div>}

      {results.length > 0 && (
        <div className="space-y-2">
          <div className="text-xs text-secondary">{results.length} results</div>
          {results.map((r, i) => (
            <div key={i} className="border border-border rounded p-3">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <span className="badge badge-info">{r.category}</span>
                  <span className="text-xs text-secondary">
                    similarity: {(r.similarity * 100).toFixed(1)}%
                  </span>
                </div>
                {connectingTo === r.pseudonym ? (
                  <div className="flex gap-1 items-center">
                    <RoleSelect value={connectRole} onChange={setConnectRole} />
                    <input
                      type="text"
                      value={connectMessage}
                      onChange={(e) => setConnectMessage(e.target.value)}
                      placeholder="Message..."
                      className="input text-xs w-48"
                    />
                    <button
                      onClick={() => handleConnect(r.pseudonym)}
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
                    onClick={() => setConnectingTo(r.pseudonym)}
                    className="btn-secondary btn-sm"
                  >
                    Connect
                  </button>
                )}
              </div>
              {r.content_preview && (
                <p className="text-xs text-secondary mt-1">{r.content_preview}</p>
              )}
              <div className="text-xs text-tertiary mt-1 font-mono truncate">
                {r.pseudonym}
              </div>
            </div>
          ))}
        </div>
      )}

      {results.length === 0 && !searching && !error && query && (
        <div className="text-sm text-secondary text-center py-4">No results found</div>
      )}
    </div>
  )
}

function FaceSearchPanel({ onResult }) {
  const [sourceSchema, setSourceSchema] = useState('')
  const [sourceKey, setSourceKey] = useState('')
  const [faceIndex, setFaceIndex] = useState(0)
  const [results, setResults] = useState([])
  const [searching, setSearching] = useState(false)
  const [error, setError] = useState(null)
  const [connectingTo, setConnectingTo] = useState(null)
  const [connectMessage, setConnectMessage] = useState('')
  const [connectRole, setConnectRole] = useState('acquaintance')

  const handleSearch = useCallback(async () => {
    if (!sourceSchema.trim() || !sourceKey.trim()) return
    setSearching(true)
    setError(null)
    try {
      const res = await discoveryClient.faceSearch(sourceSchema, sourceKey, faceIndex)
      if (res.success) {
        setResults(res.data?.results || [])
        onResult({ success: true, data: res.data })
      } else {
        setError(res.error || 'Face search failed')
        onResult({ error: res.error || 'Face search failed' })
      }
    } catch (e) {
      const msg = toErrorMessage(e)
      setError(msg || 'Network error')
      onResult({ error: msg || 'Network error' })
    } finally {
      setSearching(false)
    }
  }, [sourceSchema, sourceKey, faceIndex, onResult])

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

  return (
    <div className="space-y-3">
      <div className="card-info p-3 rounded text-xs space-y-1.5">
        <div className="font-semibold text-gruvbox-blue">Face Search</div>
        <p className="text-secondary">
          Search the discovery network by face. Enter the schema name and record key of a photo
          with detected faces, then specify which face to search by (0 = first face).
        </p>
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
        <div>
          <label className="text-xs text-secondary block mb-1">Schema Name</label>
          <input
            type="text"
            value={sourceSchema}
            onChange={(e) => setSourceSchema(e.target.value)}
            placeholder="e.g. photos"
            className="input w-full"
          />
        </div>
        <div>
          <label className="text-xs text-secondary block mb-1">Record Key</label>
          <input
            type="text"
            value={sourceKey}
            onChange={(e) => setSourceKey(e.target.value)}
            placeholder="e.g. IMG_1234"
            className="input w-full"
          />
        </div>
      </div>

      <div className="flex gap-2 items-end">
        <div>
          <label className="text-xs text-secondary block mb-1">Face Index</label>
          <input
            type="number"
            min={0}
            value={faceIndex}
            onChange={(e) => setFaceIndex(parseInt(e.target.value, 10) || 0)}
            className="input w-24"
          />
        </div>
        <button
          onClick={handleSearch}
          disabled={searching || !sourceSchema.trim() || !sourceKey.trim()}
          className="btn-primary"
        >
          {searching ? 'Searching...' : 'Search by Face'}
        </button>
      </div>

      {error && <div className="text-sm text-gruvbox-red">{error}</div>}

      {results.length > 0 && (
        <div className="space-y-2">
          <div className="text-xs text-secondary">{results.length} results</div>
          {results.map((r, i) => (
            <div key={i} className="border border-border rounded p-3">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <span className="badge badge-info">face #{r.face_index}</span>
                  <span className="text-xs text-secondary">
                    similarity: {(r.similarity * 100).toFixed(1)}%
                  </span>
                  {r.min_trust_tier != null && r.min_trust_tier > 0 && (
                    <span className="badge badge-warning text-xs">
                      requires {trustTierLabel(r.min_trust_tier)}
                    </span>
                  )}
                </div>
                {connectingTo === r.pseudonym ? (
                  <div className="flex gap-1 items-center">
                    <RoleSelect value={connectRole} onChange={setConnectRole} />
                    <input
                      type="text"
                      value={connectMessage}
                      onChange={(e) => setConnectMessage(e.target.value)}
                      placeholder="Message..."
                      className="input text-xs w-48"
                    />
                    <button
                      onClick={() => handleConnect(r.pseudonym)}
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
                    onClick={() => setConnectingTo(r.pseudonym)}
                    className="btn-secondary btn-sm"
                  >
                    Connect
                  </button>
                )}
              </div>
              <div className="text-xs text-secondary mt-1">
                {r.schema_name} / {r.record_key}
              </div>
              <div className="text-xs text-tertiary mt-1 font-mono truncate">
                {r.pseudonym}
              </div>
            </div>
          ))}
        </div>
      )}

      {results.length === 0 && !searching && !error && sourceSchema && sourceKey && (
        <div className="text-sm text-secondary text-center py-4">No results found</div>
      )}
    </div>
  )
}

function ConnectionRequestsPanel({ onResult }) {
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
    } catch { /* ignore */ }
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

function SentRequestsPanel() {
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

const REFRESH_INTERVAL_MS = 60_000

function PeopleLikeYouPanel({ onResult }) {
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

function InterestsPanel({ onResult }) {
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
            No interests detected yet. Ingest some data and click Re-detect to discover your interest categories.
          </p>
        </div>
      )}

      <button
        onClick={handleDetect}
        disabled={detecting}
        className="btn-secondary"
      >
        {detecting ? 'Detecting...' : 'Re-detect Interests'}
      </button>
    </div>
  )
}

function isLocalModeError(msg) {
  return msg && (msg.includes('503') || msg.includes('DISCOVERY_MASTER_KEY') || msg.includes('Service Unavailable') || msg.includes('Discovery not available') || msg.includes('Register with Exemem'))
}

function LocalModeNotice() {
  return (
    <div className="card p-6 text-center space-y-2 rounded">
      <h3 className="text-base text-primary font-medium">Discovery requires Exemem Cloud</h3>
      <p className="text-sm text-secondary">
        Discovery connects you with other FoldDB users who share similar interests.
        Enable cloud sync in Settings to use this feature.
      </p>
    </div>
  )
}

function SharedEventsPanel({ onResult }) {
  const [status, setStatus] = useState(null)
  const [sharedEvents, setSharedEvents] = useState([])
  const [loading, setLoading] = useState(true)
  const [toggling, setToggling] = useState(false)
  const [error, setError] = useState(null)

  const loadData = useCallback(async () => {
    try {
      const statusRes = await discoveryClient.getCalendarSharingStatus()
      if (statusRes.success) {
        setStatus(statusRes.data)
        setError(null)
        if (statusRes.data?.opted_in) {
          const eventsRes = await discoveryClient.getSharedEvents()
          if (eventsRes.success) {
            setSharedEvents(eventsRes.data?.shared_events || [])
          }
        }
      } else {
        setError(statusRes.error || 'Failed to load calendar sharing status')
      }
    } catch (e) {
      setError(toErrorMessage(e) || 'Network error')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { loadData() }, [loadData])

  const handleToggle = async (enable) => {
    setToggling(true)
    try {
      const res = enable
        ? await discoveryClient.calendarSharingOptIn()
        : await discoveryClient.calendarSharingOptOut()
      if (res.success) {
        setStatus(res.data)
        if (!enable) setSharedEvents([])
        onResult({
          success: true,
          data: { message: enable ? 'Calendar sharing enabled' : 'Calendar sharing disabled' },
        })
        if (enable) {
          const eventsRes = await discoveryClient.getSharedEvents()
          if (eventsRes.success) setSharedEvents(eventsRes.data?.shared_events || [])
        }
      } else {
        onResult({ error: res.error || 'Failed to toggle calendar sharing' })
      }
    } catch (e) {
      onResult({ error: toErrorMessage(e) || 'Network error' })
    } finally {
      setToggling(false)
    }
  }

  if (loading) return <p className="text-secondary text-sm">Loading calendar sharing...</p>

  if (error) {
    if (isLocalModeError(error)) return <LocalModeNotice />
    return (
      <div className="space-y-3">
        <div className="text-sm text-gruvbox-red">{error}</div>
        <button onClick={loadData} className="btn-secondary btn-sm">Retry</button>
      </div>
    )
  }

  const optedIn = status?.opted_in || false

  return (
    <div className="space-y-4">
      {/* Opt-in toggle */}
      <div className="card p-4 space-y-3">
        <div className="flex items-center justify-between">
          <div>
            <div className="text-sm font-medium text-primary">Share Calendar with Connections</div>
            <div className="text-xs text-secondary mt-1">
              Compare events with accepted connections to discover shared conferences, meetups, and events.
              Only overlap existence is revealed — never full calendar details.
            </div>
          </div>
          <button
            onClick={() => handleToggle(!optedIn)}
            disabled={toggling}
            className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
              toggling ? 'opacity-50 cursor-not-allowed' : 'cursor-pointer'
            } ${optedIn ? 'bg-gruvbox-green' : 'bg-gruvbox-elevated border border-border'}`}
          >
            <span
              className={`inline-block h-3.5 w-3.5 rounded-full bg-primary transition-transform ${
                optedIn ? 'translate-x-[18px]' : 'translate-x-[3px]'
              }`}
            />
          </button>
        </div>

        {optedIn && status && (
          <div className="text-xs text-tertiary">
            {status.local_event_count} events synced &middot; {status.peer_count} peer{status.peer_count !== 1 ? 's' : ''} connected
          </div>
        )}
      </div>

      {/* Privacy notice */}
      {optedIn && (
        <div className="card-info p-3 rounded text-xs space-y-1.5">
          <div className="font-semibold text-gruvbox-blue">Privacy</div>
          <ul className="space-y-1 text-secondary">
            <li>Only event overlap existence is shared — never full calendar details</li>
            <li>Both you and your connection must opt in for comparison</li>
            <li>Events are compared by date, title, and location similarity</li>
            <li>You can opt out at any time to stop sharing</li>
          </ul>
        </div>
      )}

      {/* Shared events */}
      {optedIn && sharedEvents.length > 0 && (
        <div className="space-y-2">
          <div className="text-sm font-medium text-primary">Shared Events</div>
          {sharedEvents.map((evt, i) => (
            <div key={i} className="card p-3 space-y-1">
              <div className="flex items-center justify-between">
                <div className="text-sm font-medium text-primary">{evt.event_title}</div>
                <span className="badge badge-info text-xs">
                  {evt.connection_count} connection{evt.connection_count !== 1 ? 's' : ''}
                </span>
              </div>
              <div className="text-xs text-secondary">
                {evt.start_time} — {evt.end_time}
              </div>
              {evt.location && (
                <div className="text-xs text-tertiary">{evt.location}</div>
              )}
              <div className="text-xs text-gruvbox-green">
                You and {evt.connection_count} connection{evt.connection_count !== 1 ? 's' : ''} are attending this event
              </div>
            </div>
          ))}
        </div>
      )}

      {/* Empty state */}
      {optedIn && sharedEvents.length === 0 && (
        <div className="card p-6 text-center">
          <p className="text-secondary text-sm">
            No shared events detected yet. Sync your calendar and connect with peers who also have calendar sharing enabled.
          </p>
        </div>
      )}

      {/* Not opted in */}
      {!optedIn && (
        <div className="card p-6 text-center">
          <p className="text-secondary text-sm">
            Enable calendar sharing to discover events you have in common with your connections.
          </p>
        </div>
      )}
    </div>
  )
}

export default function DiscoveryTab({ onResult }) {
  const { approvedSchemas } = useApprovedSchemas()
  const [configs, setConfigs] = useState([])
  const [publishing, setPublishing] = useState(false)
  const [activeSection, setActiveSection] = useState('people')
  const [error, setError] = useState(null)
  const [serviceAvailable, setServiceAvailable] = useState(true)
  const [expandedCategories, setExpandedCategories] = useState(new Set())
  const [toggling, setToggling] = useState(false)
  const [lastPublishResult, setLastPublishResult] = useState(null)
  const [publishFacesCategories, setPublishFacesCategories] = useState(new Set())

  const optedInNames = new Set(configs.map(c => c.schema_name))

  // Categories that have been published (have at least one opted-in schema)
  // We track this via lastPublishResult — if publish was called, those categories are live
  const publishedCategories = new Set(
    lastPublishResult
      ? configs.filter(c => optedInNames.has(c.schema_name)).map(c => c.category)
      : []
  )

  const categoryGroups = groupByCategory(approvedSchemas || [])
  const categoryNames = Object.keys(categoryGroups).sort()
  const hasSchemas = (approvedSchemas || []).length > 0

  const loadConfigs = useCallback(async () => {
    try {
      const res = await discoveryClient.listOptIns()
      if (res.success) {
        setConfigs(res.data?.configs || [])
        setServiceAvailable(true)
        // If there are existing opt-ins, they may have been published before
        if ((res.data?.configs || []).length > 0) {
          setLastPublishResult({ existing: true })
        }
      } else if (res.status === 503) {
        setServiceAvailable(false)
      }
    } catch {
      setServiceAvailable(false)
    }
  }, [])

  useEffect(() => { loadConfigs() }, [loadConfigs])

  const handlePublishFacesToggle = (category, enabled) => {
    setPublishFacesCategories(prev => {
      const next = new Set(prev)
      if (enabled) next.add(category)
      else next.delete(category)
      return next
    })
  }

  const handleToggleCategory = async (category, schemas, enable) => {
    setToggling(true)
    setError(null)
    try {
      if (enable) {
        // Opt in all schemas in this category
        for (const s of schemas) {
          if (!optedInNames.has(s.name)) {
            const res = await discoveryClient.optIn({
              schema_name: s.name,
              category,
              include_preview: false,
              publish_faces: publishFacesCategories.has(category),
            })
            if (res.success) {
              setConfigs(res.data?.configs || [])
            } else {
              setError(res.error)
              break
            }
          }
        }
      } else {
        // Opt out all schemas in this category
        for (const s of schemas) {
          if (optedInNames.has(s.name)) {
            const res = await discoveryClient.optOut(s.name)
            if (res.success) {
              setConfigs(res.data?.configs || [])
            } else {
              setError(res.error)
              break
            }
          }
        }
      }
    } catch (e) {
      setError(toErrorMessage(e))
    } finally {
      setToggling(false)
    }
  }

  const handleBulkAction = async (action) => {
    setToggling(true)
    setError(null)
    try {
      if (action === 'publish-all') {
        for (const [cat, schemas] of Object.entries(categoryGroups)) {
          for (const s of schemas) {
            if (!optedInNames.has(s.name)) {
              const res = await discoveryClient.optIn({
                schema_name: s.name,
                category: cat,
                include_preview: false,
                publish_faces: publishFacesCategories.has(cat),
              })
              if (res.success) setConfigs(res.data?.configs || [])
            }
          }
        }
      } else if (action === 'unpublish-all') {
        for (const c of configs) {
          const res = await discoveryClient.optOut(c.schema_name)
          if (res.success) setConfigs(res.data?.configs || [])
        }
        setLastPublishResult(null)
      }
    } catch (e) {
      setError(toErrorMessage(e))
    } finally {
      setToggling(false)
    }
  }

  const handlePublish = async () => {
    setPublishing(true)
    setError(null)
    try {
      const res = await discoveryClient.publish()
      if (res.success) {
        setLastPublishResult(res.data)
        onResult({
          success: true,
          data: {
            message: `Published: ${res.data?.accepted} accepted, ${res.data?.quarantined} quarantined, ${res.data?.skipped} skipped`,
            ...res.data,
          },
        })
      } else {
        setError(res.error)
        onResult({ error: res.error })
      }
    } catch (e) {
      const msg = toErrorMessage(e)
      setError(msg)
      onResult({ error: msg })
    } finally {
      setPublishing(false)
    }
  }

  const toggleExpand = (cat) => {
    setExpandedCategories(prev => {
      const next = new Set(prev)
      if (next.has(cat)) next.delete(cat)
      else next.add(cat)
      return next
    })
  }

  if (!serviceAvailable) {
    return (
      <div className="space-y-4">
        <div className="card p-6 text-center rounded">
          <h3 className="text-lg text-primary mb-2">Discovery Not Configured</h3>
          <p className="text-secondary text-sm">
            Set <code className="text-gruvbox-yellow">DISCOVERY_SERVICE_URL</code> and{' '}
            <code className="text-gruvbox-yellow">DISCOVERY_MASTER_KEY</code> environment
            variables to enable network discovery.
          </p>
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-4">
      {/* Section Tabs */}
      <div className="flex gap-1 border-b border-border pb-1">
        {[
          { id: 'people', label: 'People Like You' },
          { id: 'shared-events', label: 'Shared Events' },
          { id: 'interests', label: 'Your Interests' },
          { id: 'manage', label: 'Interest Categories' },
          { id: 'search', label: 'Search Network' },
          { id: 'face-search', label: 'Face Search' },
          { id: 'requests', label: 'Received' },
          { id: 'sent', label: 'Sent' },
        ].map(s => (
          <button
            key={s.id}
            onClick={() => setActiveSection(s.id)}
            className={`px-3 py-1 text-sm rounded-t ${
              activeSection === s.id
                ? 'bg-surface text-primary border border-border border-b-surface'
                : 'text-secondary hover:text-primary'
            }`}
          >
            {s.label}
          </button>
        ))}
      </div>

      {error && <div className="text-sm text-gruvbox-red">{error}</div>}

      {/* People Like You Section */}
      {activeSection === 'people' && (
        <PeopleLikeYouPanel onResult={onResult} />
      )}

      {/* Shared Events Section */}
      {activeSection === 'shared-events' && (
        <SharedEventsPanel onResult={onResult} />
      )}

      {/* Interests Section */}
      {activeSection === 'interests' && (
        <InterestsPanel onResult={onResult} />
      )}

      {/* Manage Section — Category Cards */}
      {activeSection === 'manage' && (
        <div className="space-y-4">
          {!hasSchemas ? (
            <EmptyState />
          ) : (
            <>
              {/* Bulk Actions */}
              <div className="flex items-center justify-between">
                <div className="text-xs text-secondary">
                  {configs.length} of {(approvedSchemas || []).length} schemas opted in
                </div>
                <div className="flex gap-2">
                  <button
                    onClick={() => handleBulkAction('publish-all')}
                    disabled={toggling || configs.length === (approvedSchemas || []).length}
                    className="btn-secondary btn-sm"
                  >
                    Opt In All
                  </button>
                  <button
                    onClick={() => handleBulkAction('unpublish-all')}
                    disabled={toggling || configs.length === 0}
                    className="btn-secondary btn-sm text-gruvbox-red"
                  >
                    Opt Out All
                  </button>
                </div>
              </div>

              {/* Privacy Guarantees */}
              <PrivacyGuarantees />

              {/* Category Cards */}
              <div className="space-y-3">
                {categoryNames.map(cat => (
                  <CategoryCard
                    key={cat}
                    category={cat}
                    schemas={categoryGroups[cat]}
                    optedInNames={optedInNames}
                    publishedCategories={publishedCategories}
                    onToggle={handleToggleCategory}
                    toggling={toggling}
                    expanded={expandedCategories.has(cat)}
                    onExpandToggle={() => toggleExpand(cat)}
                    publishFaces={publishFacesCategories.has(cat)}
                    onPublishFacesToggle={handlePublishFacesToggle}
                  />
                ))}
              </div>

              {/* Publish Button */}
              {configs.length > 0 && (
                <button
                  onClick={handlePublish}
                  disabled={publishing}
                  className="btn-primary w-full"
                >
                  {publishing
                    ? 'Publishing...'
                    : `Publish ${configs.length} Schema${configs.length !== 1 ? 's' : ''} to Network`}
                </button>
              )}

              {/* Last Publish Result */}
              {lastPublishResult && lastPublishResult.accepted !== undefined && (
                <div className="card-success p-3 rounded text-xs text-secondary">
                  Last publish: {lastPublishResult.accepted} accepted, {lastPublishResult.quarantined} quarantined, {lastPublishResult.skipped} skipped of {lastPublishResult.total} total
                </div>
              )}
            </>
          )}
        </div>
      )}

      {/* Search Section */}
      {activeSection === 'search' && (
        <SearchPanel onResult={onResult} />
      )}

      {/* Face Search Section */}
      {activeSection === 'face-search' && (
        <FaceSearchPanel onResult={onResult} />
      )}

      {/* Received Connection Requests */}
      {activeSection === 'requests' && (
        <ConnectionRequestsPanel onResult={onResult} />
      )}

      {/* Sent Connection Requests */}
      {activeSection === 'sent' && (
        <SentRequestsPanel />
      )}
    </div>
  )
}
