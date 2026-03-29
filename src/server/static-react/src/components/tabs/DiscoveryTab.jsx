import { useCallback, useEffect, useState } from 'react'
import { useApprovedSchemas } from '../../hooks/useApprovedSchemas.js'
import { discoveryClient } from '../../api/clients/discoveryClient'
import { toErrorMessage } from '../../utils/schemaUtils'

/** Derive a category from schema classification metadata.
 *  Prefers field_data_classifications domains, falls back to schema name. */
function inferCategory(schema) {
  // Collect unique data domains from field_data_classifications
  const domains = new Set()
  if (schema.field_data_classifications) {
    for (const cls of Object.values(schema.field_data_classifications)) {
      if (cls?.data_domain) domains.add(cls.data_domain)
    }
  }
  if (domains.size > 0) {
    // Use the most common domain, or join if mixed
    return [...domains].join(', ')
  }

  // Fallback: derive from field_classifications tags
  const tags = new Set()
  if (schema.field_classifications) {
    for (const fieldTags of Object.values(schema.field_classifications)) {
      if (Array.isArray(fieldTags)) {
        for (const tag of fieldTags) {
          // Extract domain from compound tags like "name:person" → "person"
          const parts = tag.split(':')
          if (parts.length > 1) tags.add(parts[1])
          else tags.add(tag)
        }
      }
    }
  }
  if (tags.size > 0) {
    // Pick the most descriptive tags (skip generic ones like "word")
    const descriptive = [...tags].filter(t => !['word', 'number', 'date'].includes(t))
    if (descriptive.length > 0) return descriptive.slice(0, 3).join(', ')
  }

  // Last resort: lowercase schema name
  return schema.name?.replace(/([A-Z])/g, ' $1').trim().toLowerCase() || 'general'
}

function OptInForm({ schemas, optedInNames, onOptIn }) {
  const [schemaName, setSchemaName] = useState('')
  const [category, setCategory] = useState('')
  const [includePreview, setIncludePreview] = useState(false)
  const [submitting, setSubmitting] = useState(false)

  const availableSchemas = (schemas || []).filter(s => !optedInNames.has(s.name))

  // Auto-derive category when schema selection changes
  const handleSchemaChange = (name) => {
    setSchemaName(name)
    if (name) {
      const schema = availableSchemas.find(s => s.name === name)
      if (schema) {
        setCategory(inferCategory(schema))
      }
    } else {
      setCategory('')
    }
  }

  const handleSubmit = async (e) => {
    e.preventDefault()
    if (!schemaName || !category) return
    setSubmitting(true)
    try {
      await onOptIn({ schema_name: schemaName, category, include_preview: includePreview })
      setSchemaName('')
      setCategory('')
      setIncludePreview(false)
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <form onSubmit={handleSubmit} className="flex flex-wrap gap-2 items-end">
      <div className="flex-1 min-w-[150px]">
        <label className="text-xs text-secondary block mb-1">Schema</label>
        <select
          value={schemaName}
          onChange={(e) => handleSchemaChange(e.target.value)}
          className="input w-full"
        >
          <option value="">Select schema...</option>
          {availableSchemas.map(s => (
            <option key={s.name} value={s.name}>{s.name}</option>
          ))}
        </select>
      </div>
      <div className="flex-1 min-w-[120px]">
        <label className="text-xs text-secondary block mb-1">Category <span className="text-tertiary">(auto-detected)</span></label>
        <input
          type="text"
          value={category}
          onChange={(e) => setCategory(e.target.value)}
          placeholder="auto-detected from schema"
          className="input w-full"
        />
      </div>
      <label className="flex items-center gap-1 text-xs text-secondary cursor-pointer">
        <input
          type="checkbox"
          checked={includePreview}
          onChange={(e) => setIncludePreview(e.target.checked)}
        />
        Preview
      </label>
      <button
        type="submit"
        disabled={submitting || !schemaName || !category}
        className="btn-primary"
      >
        {submitting ? 'Opting in...' : 'Opt In'}
      </button>
    </form>
  )
}

function OptInList({ configs, onOptOut }) {
  if (!configs || configs.length === 0) {
    return <p className="text-secondary text-sm">No schemas opted in for discovery.</p>
  }

  return (
    <div className="space-y-1">
      {configs.map(c => (
        <div key={c.schema_name} className="flex items-center justify-between px-3 py-2 bg-surface-secondary rounded">
          <div className="flex items-center gap-3">
            <span className="font-mono text-sm text-primary">{c.schema_name}</span>
            <span className="badge badge-info">{c.category}</span>
            {c.include_preview && <span className="badge badge-warning">preview</span>}
          </div>
          <button
            onClick={() => onOptOut(c.schema_name)}
            className="btn-secondary btn-sm text-gruvbox-red"
          >
            Opt Out
          </button>
        </div>
      ))}
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
      const res = await discoveryClient.connect(pseudonym, connectMessage)
      if (res.success) {
        setConnectingTo(null)
        setConnectMessage('')
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
                      onClick={() => { setConnectingTo(null); setConnectMessage('') }}
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

function IncomingRequests() {
  const [requests, setRequests] = useState([])
  const [loading, setLoading] = useState(false)

  const fetchRequests = useCallback(async () => {
    setLoading(true)
    try {
      const res = await discoveryClient.pollRequests()
      if (res.success) {
        setRequests(res.data?.requests || [])
      }
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { fetchRequests() }, [fetchRequests])

  if (loading) return <p className="text-secondary text-sm">Loading requests...</p>
  if (requests.length === 0) return <p className="text-secondary text-sm">No pending connection requests.</p>

  return (
    <div className="space-y-2">
      {requests.map(r => (
        <div key={r.request_id} className="border border-border rounded p-3">
          <div className="flex items-center gap-2 text-xs">
            <span className="badge badge-warning">{r.status}</span>
            <span className="text-secondary">{r.created_at}</span>
          </div>
          <p className="text-sm text-primary mt-1">{r.message}</p>
          <div className="text-xs text-tertiary font-mono mt-1 truncate">
            from: {r.requester_pseudonym}
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

  const loadInterests = useCallback(async () => {
    try {
      const res = await discoveryClient.getInterests()
      if (res.success) {
        setProfile(res.data)
      }
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

export default function DiscoveryTab({ onResult }) {
  const { approvedSchemas } = useApprovedSchemas()
  const [configs, setConfigs] = useState([])
  const [publishing, setPublishing] = useState(false)
  const [activeSection, setActiveSection] = useState('interests')
  const [error, setError] = useState(null)
  const [serviceAvailable, setServiceAvailable] = useState(true)

  const optedInNames = new Set(configs.map(c => c.schema_name))

  const loadConfigs = useCallback(async () => {
    try {
      const res = await discoveryClient.listOptIns()
      if (res.success) {
        setConfigs(res.data?.configs || [])
        setServiceAvailable(true)
      } else if (res.status === 503) {
        setServiceAvailable(false)
      }
    } catch {
      setServiceAvailable(false)
    }
  }, [])

  useEffect(() => { loadConfigs() }, [loadConfigs])

  const handleOptIn = async (req) => {
    setError(null)
    try {
      const res = await discoveryClient.optIn(req)
      if (res.success) {
        setConfigs(res.data?.configs || [])
        onResult({ success: true, data: { message: `Opted in ${req.schema_name}` } })
      } else {
        setError(res.error)
        onResult({ error: res.error })
      }
    } catch (e) {
      const msg = toErrorMessage(e)
      setError(msg)
      onResult({ error: msg })
    }
  }

  const handleOptOut = async (schemaName) => {
    setError(null)
    try {
      const res = await discoveryClient.optOut(schemaName)
      if (res.success) {
        setConfigs(res.data?.configs || [])
        onResult({ success: true, data: { message: `Opted out ${schemaName}` } })
      } else {
        setError(res.error)
        onResult({ error: res.error })
      }
    } catch (e) {
      const msg = toErrorMessage(e)
      setError(msg)
      onResult({ error: msg })
    }
  }

  const handlePublish = async () => {
    setPublishing(true)
    setError(null)
    try {
      const res = await discoveryClient.publish()
      if (res.success) {
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

  if (!serviceAvailable) {
    return (
      <div className="space-y-4">
        <div className="card p-6 text-center">
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
          { id: 'interests', label: 'Your Interests' },
          { id: 'manage', label: 'Manage Opt-Ins' },
          { id: 'search', label: 'Search Network' },
          { id: 'requests', label: 'Incoming Requests' },
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

      {/* Interests Section */}
      {activeSection === 'interests' && (
        <InterestsPanel onResult={onResult} />
      )}

      {/* Manage Section */}
      {activeSection === 'manage' && (
        <div className="space-y-4">
          <div className="card p-4 space-y-3">
            <h3 className="text-sm font-semibold text-primary">Opted-In Schemas</h3>
            <OptInList configs={configs} onOptOut={handleOptOut} />
          </div>

          <div className="card p-4 space-y-3">
            <h3 className="text-sm font-semibold text-primary">Add Schema</h3>
            <OptInForm
              schemas={approvedSchemas}
              optedInNames={optedInNames}
              onOptIn={handleOptIn}
            />
          </div>

          {configs.length > 0 && (
            <button
              onClick={handlePublish}
              disabled={publishing}
              className="btn-primary w-full"
            >
              {publishing ? 'Publishing...' : `Publish ${configs.length} Schema${configs.length !== 1 ? 's' : ''} to Network`}
            </button>
          )}
        </div>
      )}

      {/* Search Section */}
      {activeSection === 'search' && (
        <SearchPanel onResult={onResult} />
      )}

      {/* Incoming Requests Section */}
      {activeSection === 'requests' && (
        <IncomingRequests />
      )}
    </div>
  )
}
