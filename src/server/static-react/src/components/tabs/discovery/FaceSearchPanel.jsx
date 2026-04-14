import { useCallback, useEffect, useState } from 'react'
import { discoveryClient } from '../../../api/clients/discoveryClient'
import { listContacts } from '../../../api/clients/trustClient'
import { toErrorMessage } from '../../../utils/schemaUtils'
import RoleSelect from './RoleSelect'

export default function FaceSearchPanel({ onResult }) {
  const [sourceSchema, setSourceSchema] = useState('')
  const [sourceKey, setSourceKey] = useState('')
  const [faceIndex, setFaceIndex] = useState(0)
  const [results, setResults] = useState([])
  const [searching, setSearching] = useState(false)
  const [error, setError] = useState(null)
  const [connectingTo, setConnectingTo] = useState(null)
  const [connectMessage, setConnectMessage] = useState('')
  const [connectRole, setConnectRole] = useState('acquaintance')
  const [faceSchemas, setFaceSchemas] = useState([])
  const [knownPseudonyms, setKnownPseudonyms] = useState(() => new Set())
  // Detected faces in the user's selected (schema, key). Populated lazily
  // when both fields are filled in, so the user can pick a face_index by
  // bbox + confidence instead of brute-forcing every index.
  const [detectedFaces, setDetectedFaces] = useState([])
  const [detectedFacesError, setDetectedFacesError] = useState(null)
  const [loadingFaces, setLoadingFaces] = useState(false)

  useEffect(() => {
    let cancelled = false
    async function loadContacts() {
      try {
        const resp = await listContacts()
        if (cancelled || !resp.success) return
        const contacts = resp.data?.contacts || []
        const set = new Set()
        for (const c of contacts) {
          if (c.pseudonym) set.add(c.pseudonym)
          if (c.messaging_pseudonym) set.add(c.messaging_pseudonym)
        }
        setKnownPseudonyms(set)
      } catch {
        // best-effort
      }
    }
    loadContacts()
    return () => { cancelled = true }
  }, [])

  useEffect(() => {
    async function loadSchemas() {
      try {
        const resp = await discoveryClient.listOptIns()
        if (resp.success && resp.data?.configs) {
          setFaceSchemas(resp.data.configs.filter(c => c.publish_faces))
        }
      } catch { /* ignore */ }
    }
    loadSchemas()
  }, [])

  // Fetch the list of detected faces for the current (schema, key). Runs
  // whenever either field changes and both are non-empty. Failure is silent
  // (the picker just shows "no faces detected") — the user can still type
  // a face index manually, so we don't want to block search on this call.
  useEffect(() => {
    if (!sourceSchema.trim() || !sourceKey.trim()) {
      setDetectedFaces([])
      setDetectedFacesError(null)
      return
    }
    let cancelled = false
    setLoadingFaces(true)
    setDetectedFacesError(null)
    discoveryClient
      .listFaces(sourceSchema, sourceKey)
      .then(res => {
        if (cancelled) return
        if (res.success && res.data?.faces) {
          setDetectedFaces(res.data.faces)
        } else {
          setDetectedFaces([])
          setDetectedFacesError(res.error || null)
        }
      })
      .catch(() => {
        if (!cancelled) {
          setDetectedFaces([])
          setDetectedFacesError(null)
        }
      })
      .finally(() => {
        if (!cancelled) setLoadingFaces(false)
      })
    return () => { cancelled = true }
  }, [sourceSchema, sourceKey])

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
          <select
            value={sourceSchema}
            onChange={(e) => setSourceSchema(e.target.value)}
            className="w-full px-3 py-2 rounded-lg bg-surface border border-border text-primary text-sm"
          >
            <option value="">Select schema...</option>
            {faceSchemas.map(s => (
              <option key={s.schema_name} value={s.schema_name}>{s.schema_name}</option>
            ))}
          </select>
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

      {/*
        Detected-faces picker. Without this, users had to brute-force every
        face_index when a photo had multiple faces (e.g. a Comic Con shot
        with the subject + 4 audience members in the background) — there
        was no way to tell which index was who. Now the API returns bbox +
        confidence for each face, and the picker lets the user click a face
        to set face_index. Click again to clear. Renders an informative
        empty/loading/legacy state so it never silently disappears.
      */}
      {sourceSchema.trim() && sourceKey.trim() && (
        <div className="text-xs">
          <div className="text-secondary mb-1">Detected faces in this record</div>
          {loadingFaces && (
            <div className="text-tertiary">Loading…</div>
          )}
          {!loadingFaces && detectedFacesError && (
            <div className="text-gruvbox-red">{detectedFacesError}</div>
          )}
          {!loadingFaces && !detectedFacesError && detectedFaces.length === 0 && (
            <div className="text-tertiary">No faces detected for this record.</div>
          )}
          {!loadingFaces && detectedFaces.length > 0 && (
            <div className="flex flex-wrap gap-1.5">
              {detectedFaces.map(f => {
                const selected = f.face_index === faceIndex
                const hasBbox = Array.isArray(f.bbox) && f.bbox.length === 4
                const conf = typeof f.confidence === 'number' ? f.confidence : null
                return (
                  <button
                    key={f.face_index}
                    type="button"
                    onClick={() => setFaceIndex(f.face_index)}
                    className={`px-2 py-1 rounded border font-mono text-[11px] transition-colors ${
                      selected
                        ? 'bg-gruvbox-blue/20 border-gruvbox-blue text-primary'
                        : 'bg-surface border-border text-secondary hover:border-gruvbox-blue/50'
                    }`}
                    title={
                      hasBbox
                        ? `bbox [${f.bbox.map(v => v.toFixed(3)).join(', ')}]${conf !== null ? ` · conf ${conf.toFixed(2)}` : ''}`
                        : 'legacy entry — no bbox metadata'
                    }
                  >
                    #{f.face_index}
                    {hasBbox && (
                      <span className="text-tertiary ml-1">
                        ({(f.bbox[2] - f.bbox[0]).toFixed(2)}×{(f.bbox[3] - f.bbox[1]).toFixed(2)})
                      </span>
                    )}
                    {conf !== null && (
                      <span className="text-tertiary ml-1">{(conf * 100).toFixed(0)}%</span>
                    )}
                  </button>
                )
              })}
            </div>
          )}
        </div>
      )}

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
                ) : knownPseudonyms.has(r.pseudonym) ? (
                  <span className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-green/10 text-gruvbox-green border border-gruvbox-green/30">
                    ✓ Already connected
                  </span>
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
                <div className="text-xs text-secondary mt-1">
                  {r.content_preview}
                </div>
              )}
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
