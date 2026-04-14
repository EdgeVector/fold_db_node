import { useCallback, useEffect, useState } from 'react'
import { listPersonas, getPersona } from '../../../api/clients/fingerprintsClient'

/**
 * Personas sub-tab content. Shows a list of every Persona on the node,
 * with click-through to a detail view that runs the resolver and
 * surfaces any ResolveDiagnostics the backend returned.
 *
 * Data shape comes from GET /api/fingerprints/personas and
 * /api/fingerprints/personas/:id (see fingerprintsClient.ts and
 * src/handlers/fingerprints/personas.rs).
 *
 * Threshold slider is read-only in this first cut. Mutation support
 * (editing threshold, excluding mentions, merging/splitting personas)
 * lands in a follow-up once the backend exposes a mutate path.
 */
export default function PersonasPanel() {
  const [personas, setPersonas] = useState([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)
  const [selectedId, setSelectedId] = useState(null)
  const [detail, setDetail] = useState(null)
  const [detailLoading, setDetailLoading] = useState(false)
  const [detailError, setDetailError] = useState(null)

  const fetchList = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const res = await listPersonas()
      if (res.success) {
        setPersonas(res.data?.personas ?? [])
      } else {
        setError(res.error ?? 'Failed to load personas')
      }
    } catch (e) {
      setError(e?.message ?? 'Network error')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    fetchList()
  }, [fetchList])

  useEffect(() => {
    if (!selectedId) {
      setDetail(null)
      return
    }
    let cancelled = false
    setDetailLoading(true)
    setDetailError(null)
    getPersona(selectedId)
      .then(res => {
        if (cancelled) return
        if (res.success) {
          setDetail(res.data ?? null)
        } else {
          setDetailError(res.error ?? 'Failed to load persona')
          setDetail(null)
        }
      })
      .catch(e => {
        if (!cancelled) {
          setDetailError(e?.message ?? 'Network error')
          setDetail(null)
        }
      })
      .finally(() => {
        if (!cancelled) setDetailLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [selectedId])

  return (
    <div className="flex flex-col gap-4 lg:flex-row">
      <PersonaList
        personas={personas}
        loading={loading}
        error={error}
        selectedId={selectedId}
        onSelect={setSelectedId}
        onRefresh={fetchList}
      />
      <PersonaDetail
        selectedId={selectedId}
        detail={detail}
        loading={detailLoading}
        error={detailError}
      />
    </div>
  )
}

function PersonaList({ personas, loading, error, selectedId, onSelect, onRefresh }) {
  return (
    <div className="lg:w-1/2 card p-3" data-testid="persona-list">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-base font-semibold">Personas</h3>
        <button
          type="button"
          className="btn-secondary text-xs"
          onClick={onRefresh}
          disabled={loading}
          data-testid="persona-list-refresh"
        >
          {loading ? 'Loading…' : 'Refresh'}
        </button>
      </div>

      {error && (
        <div className="text-sm text-gruvbox-red" data-testid="persona-list-error">
          {error}
        </div>
      )}

      {!loading && !error && personas.length === 0 && (
        <div className="text-sm text-secondary" data-testid="persona-list-empty">
          No personas yet. The Me persona is created at node signup;
          other personas appear as you group fingerprints together.
        </div>
      )}

      <ul className="space-y-1">
        {personas.map(p => (
          <li key={p.id}>
            <button
              type="button"
              onClick={() => onSelect(p.id)}
              className={`w-full text-left px-3 py-2 rounded border transition-colors ${
                selectedId === p.id
                  ? 'bg-gruvbox-blue/20 border-gruvbox-blue text-primary'
                  : 'bg-surface border-border text-secondary hover:border-gruvbox-blue/50'
              }`}
              data-testid={`persona-row-${p.id}`}
            >
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <span className="font-medium">{p.name || '(unnamed)'}</span>
                  {p.built_in && (
                    <span
                      className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-blue/10 text-gruvbox-blue border border-gruvbox-blue/30"
                      data-testid="badge-built-in"
                    >
                      built-in
                    </span>
                  )}
                  {p.identity_linked && (
                    <span
                      className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-green/10 text-gruvbox-green border border-gruvbox-green/30"
                      data-testid="badge-verified"
                    >
                      verified
                    </span>
                  )}
                </div>
                <span className="text-[10px] text-tertiary font-mono">
                  thr {p.threshold.toFixed(2)}
                </span>
              </div>
              <div className="text-[11px] text-tertiary mt-0.5">
                {p.relationship} · {p.fingerprint_count} fps · {p.edge_count} edges
                · {p.mention_count} mentions
              </div>
            </button>
          </li>
        ))}
      </ul>
    </div>
  )
}

function PersonaDetail({ selectedId, detail, loading, error }) {
  if (!selectedId) {
    return (
      <div
        className="lg:w-1/2 card p-3 flex items-center justify-center text-sm text-secondary"
        data-testid="persona-detail-placeholder"
      >
        Select a persona on the left to see its cluster.
      </div>
    )
  }

  return (
    <div className="lg:w-1/2 card p-3 space-y-3" data-testid="persona-detail">
      {loading && <div className="text-sm text-secondary">Loading…</div>}
      {error && (
        <div className="text-sm text-gruvbox-red" data-testid="persona-detail-error">
          {error}
        </div>
      )}
      {!loading && !error && detail && <PersonaDetailBody detail={detail} />}
    </div>
  )
}

function PersonaDetailBody({ detail }) {
  return (
    <>
      <header>
        <div className="flex items-center gap-2">
          <h3 className="text-base font-semibold">{detail.name || '(unnamed)'}</h3>
          {detail.built_in && (
            <span className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-blue/10 text-gruvbox-blue border border-gruvbox-blue/30">
              built-in
            </span>
          )}
          {detail.identity_id && (
            <span className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-green/10 text-gruvbox-green border border-gruvbox-green/30">
              verified
            </span>
          )}
        </div>
        <div className="text-xs text-tertiary mt-1 font-mono break-all">{detail.id}</div>
        <div className="text-xs text-secondary mt-1">
          {detail.relationship} · tier {detail.trust_tier}
        </div>
      </header>

      {/* Threshold slider is read-only in v1 — no PATCH endpoint yet. */}
      <div data-testid="persona-detail-threshold">
        <label className="text-xs text-secondary block mb-1">
          Threshold: <span className="font-mono">{detail.threshold.toFixed(2)}</span>
        </label>
        <input
          type="range"
          min="0"
          max="1"
          step="0.01"
          value={detail.threshold}
          disabled
          readOnly
          className="w-full opacity-60 cursor-not-allowed"
          data-testid="persona-detail-threshold-input"
        />
      </div>

      {detail.diagnostics && <Diagnostics diagnostics={detail.diagnostics} />}

      <CountRow label="Fingerprints" items={detail.fingerprint_ids} testId="persona-fingerprints" />
      <CountRow label="Edges" items={detail.edge_ids} testId="persona-edges" />
      <CountRow label="Mentions" items={detail.mention_ids} testId="persona-mentions" />

      {detail.identity_id && (
        <div className="text-xs text-secondary">
          <span className="text-tertiary">Identity: </span>
          <span className="font-mono break-all">{detail.identity_id}</span>
        </div>
      )}
    </>
  )
}

function CountRow({ label, items, testId }) {
  return (
    <div data-testid={testId}>
      <div className="text-xs text-secondary mb-1">
        {label}: <span className="font-mono text-tertiary">{items.length}</span>
      </div>
      {items.length > 0 && (
        <ul className="max-h-32 overflow-y-auto text-[11px] font-mono text-tertiary space-y-0.5">
          {items.slice(0, 20).map(id => (
            <li key={id} className="truncate">
              {id}
            </li>
          ))}
          {items.length > 20 && (
            <li className="italic">…and {items.length - 20} more</li>
          )}
        </ul>
      )}
    </div>
  )
}

function Diagnostics({ diagnostics }) {
  const entries = []
  if (diagnostics.missing_seed_fingerprint_ids.length > 0) {
    entries.push(
      `${diagnostics.missing_seed_fingerprint_ids.length} missing seed fingerprint(s): ${diagnostics.missing_seed_fingerprint_ids.join(', ')}`,
    )
  }
  if (diagnostics.excluded_edge_count > 0) {
    entries.push(`${diagnostics.excluded_edge_count} edge(s) excluded by persona rules`)
  }
  if (diagnostics.forbidden_edge_count > 0) {
    entries.push(`${diagnostics.forbidden_edge_count} UserForbidden edge(s) skipped`)
  }
  if (diagnostics.below_threshold_edge_count > 0) {
    entries.push(
      `${diagnostics.below_threshold_edge_count} edge(s) below the current threshold`,
    )
  }
  if (diagnostics.excluded_mention_count > 0) {
    entries.push(
      `${diagnostics.excluded_mention_count} mention(s) excluded by persona rules`,
    )
  }
  if (diagnostics.dangling_edge_ids.length > 0) {
    entries.push(
      `${diagnostics.dangling_edge_ids.length} dangling edge reference(s) — data inconsistency`,
    )
  }

  if (entries.length === 0) return null

  return (
    <div
      className="text-xs text-gruvbox-yellow bg-gruvbox-yellow/5 border border-gruvbox-yellow/30 rounded p-2 space-y-1"
      data-testid="persona-detail-diagnostics"
    >
      <div className="font-semibold">Resolve diagnostics</div>
      <ul className="list-disc pl-4 space-y-0.5">
        {entries.map(e => (
          <li key={e}>{e}</li>
        ))}
      </ul>
    </div>
  )
}
