import { useCallback, useEffect, useState } from 'react'
import {
  listPersonas,
  getPersona,
  updatePersonaThreshold,
} from '../../../api/clients/fingerprintsClient'

/**
 * Personas sub-tab content. Shows a list of every Persona on the node,
 * with click-through to a detail view that runs the resolver and
 * surfaces any ResolveDiagnostics the backend returned.
 *
 * Data shape comes from GET /api/fingerprints/personas and
 * /api/fingerprints/personas/:id (see fingerprintsClient.ts and
 * src/handlers/fingerprints/personas.rs).
 *
 * Threshold slider is editable — dragging updates local state only;
 * releasing the slider fires a PATCH /api/fingerprints/personas/:id
 * with the new threshold, then the detail view is replaced with the
 * freshly-resolved response and the list is refetched so the cluster
 * counts match.
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

  const handleThresholdCommit = useCallback(
    async nextThreshold => {
      if (!selectedId) return
      try {
        const res = await updatePersonaThreshold(selectedId, nextThreshold)
        if (res.success && res.data) {
          // Replace the detail in place so the user sees the freshly
          // resolved cluster counts + diagnostics without a second GET.
          setDetail(res.data)
          // And refetch the list so the summary row for this persona
          // picks up the new counts too.
          fetchList()
        } else {
          setDetailError(res.error ?? 'Failed to update threshold')
        }
      } catch (e) {
        setDetailError(e?.message ?? 'Network error while updating threshold')
      }
    },
    [selectedId, fetchList],
  )

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
        onThresholdCommit={handleThresholdCommit}
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

function PersonaDetail({ selectedId, detail, loading, error, onThresholdCommit }) {
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
      {!loading && !error && detail && (
        <PersonaDetailBody detail={detail} onThresholdCommit={onThresholdCommit} />
      )}
    </div>
  )
}

function PersonaDetailBody({ detail, onThresholdCommit }) {
  // Local mirror of the slider value so the knob moves smoothly
  // while the user drags. We only call the parent (and fire the
  // PATCH) when the user releases the slider, which keeps us from
  // spamming the backend on every pixel of drag.
  const [sliderValue, setSliderValue] = useState(detail.threshold)

  // If the parent replaces `detail` (e.g. after a PATCH response or
  // when the selected persona changes), resync the local state.
  useEffect(() => {
    setSliderValue(detail.threshold)
  }, [detail.threshold, detail.id])

  const commit = () => {
    // Guard against no-ops and missing callback (e.g. tests that
    // don't pass it). Pass a number, not a string.
    const next = Number(sliderValue)
    if (!Number.isFinite(next)) return
    if (Math.abs(next - detail.threshold) < 1e-6) return
    if (typeof onThresholdCommit === 'function') {
      onThresholdCommit(next)
    }
  }

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

      <div data-testid="persona-detail-threshold">
        <label className="text-xs text-secondary block mb-1">
          Threshold: <span className="font-mono">{Number(sliderValue).toFixed(2)}</span>
        </label>
        <input
          type="range"
          min="0"
          max="1"
          step="0.01"
          value={sliderValue}
          onChange={e => setSliderValue(e.target.value)}
          onMouseUp={commit}
          onTouchEnd={commit}
          onKeyUp={commit}
          className="w-full"
          data-testid="persona-detail-threshold-input"
        />
      </div>

      {detail.diagnostics && <Diagnostics diagnostics={detail.diagnostics} />}

      <FingerprintRows fingerprints={detail.fingerprints || []} fallbackIds={detail.fingerprint_ids} />
      <EdgeRows edges={detail.edges || []} fallbackIds={detail.edge_ids} />
      <MentionRows mentions={detail.mentions || []} fallbackIds={detail.mention_ids} />

      {detail.identity_id && (
        <div className="text-xs text-secondary">
          <span className="text-tertiary">Identity: </span>
          <span className="font-mono break-all">{detail.identity_id}</span>
        </div>
      )}
    </>
  )
}

function SectionShell({ label, count, testId, children }) {
  return (
    <div data-testid={testId}>
      <div className="text-xs text-secondary mb-1">
        {label}: <span className="font-mono text-tertiary">{count}</span>
      </div>
      {count > 0 && (
        <ul className="max-h-40 overflow-y-auto text-[11px] text-tertiary space-y-0.5">
          {children}
        </ul>
      )}
    </div>
  )
}

function FingerprintRows({ fingerprints, fallbackIds }) {
  // Fall back to opaque ids if the backend hasn't returned enriched
  // content yet (older release, or dangling fingerprints). This keeps
  // the panel from silently showing nothing when enrichment is
  // missing.
  if (fingerprints.length === 0) {
    return (
      <SectionShell
        label="Fingerprints"
        count={fallbackIds.length}
        testId="persona-fingerprints"
      >
        {fallbackIds.slice(0, 20).map(id => (
          <li key={id} className="truncate font-mono">
            {id}
          </li>
        ))}
        {fallbackIds.length > 20 && (
          <li className="italic">…and {fallbackIds.length - 20} more</li>
        )}
      </SectionShell>
    )
  }
  return (
    <SectionShell
      label="Fingerprints"
      count={fingerprints.length}
      testId="persona-fingerprints"
    >
      {fingerprints.slice(0, 40).map(fp => (
        <li key={fp.id} className="flex items-baseline gap-2">
          <span className="text-[9px] uppercase tracking-wider text-gruvbox-yellow bg-gruvbox-yellow/10 border border-gruvbox-yellow/30 rounded px-1.5 py-0.5 font-mono shrink-0">
            {fp.kind || 'unknown'}
          </span>
          <span className="truncate text-primary">{fp.display_value || '(empty)'}</span>
        </li>
      ))}
      {fingerprints.length > 40 && (
        <li className="italic">…and {fingerprints.length - 40} more</li>
      )}
    </SectionShell>
  )
}

function EdgeRows({ edges, fallbackIds }) {
  if (edges.length === 0) {
    return (
      <SectionShell label="Edges" count={fallbackIds.length} testId="persona-edges">
        {fallbackIds.slice(0, 20).map(id => (
          <li key={id} className="truncate font-mono">
            {id}
          </li>
        ))}
        {fallbackIds.length > 20 && (
          <li className="italic">…and {fallbackIds.length - 20} more</li>
        )}
      </SectionShell>
    )
  }
  return (
    <SectionShell label="Edges" count={edges.length} testId="persona-edges">
      {edges.slice(0, 40).map(e => (
        <li key={e.id} className="flex items-baseline gap-2">
          <span className="text-[9px] uppercase tracking-wider text-gruvbox-blue bg-gruvbox-blue/10 border border-gruvbox-blue/30 rounded px-1.5 py-0.5 font-mono shrink-0">
            {e.kind}
          </span>
          <span className="font-mono truncate">{e.a}</span>
          <span className="text-tertiary">·</span>
          <span className="font-mono truncate">{e.b}</span>
          <span className="ml-auto font-mono text-gruvbox-green shrink-0">
            {e.weight.toFixed(2)}
          </span>
        </li>
      ))}
      {edges.length > 40 && (
        <li className="italic">…and {edges.length - 40} more</li>
      )}
    </SectionShell>
  )
}

function MentionRows({ mentions, fallbackIds }) {
  if (mentions.length === 0) {
    return (
      <SectionShell label="Mentions" count={fallbackIds.length} testId="persona-mentions">
        {fallbackIds.slice(0, 20).map(id => (
          <li key={id} className="truncate font-mono">
            {id}
          </li>
        ))}
        {fallbackIds.length > 20 && (
          <li className="italic">…and {fallbackIds.length - 20} more</li>
        )}
      </SectionShell>
    )
  }
  return (
    <SectionShell label="Mentions" count={mentions.length} testId="persona-mentions">
      {mentions.slice(0, 40).map(m => (
        <li key={m.id} className="flex items-baseline gap-2">
          <span className="text-[9px] uppercase tracking-wider text-gruvbox-aqua bg-gruvbox-aqua/10 border border-gruvbox-aqua/30 rounded px-1.5 py-0.5 font-mono shrink-0">
            {m.extractor}
          </span>
          <span className="truncate">
            <span className="font-mono text-secondary">{m.source_schema}</span>
            <span className="text-tertiary">:</span>
            <span className="font-mono">{m.source_key}</span>
            {m.source_field && (
              <span className="text-tertiary"> · {m.source_field}</span>
            )}
          </span>
          {m.created_at && (
            <span className="ml-auto font-mono text-tertiary shrink-0">
              {m.created_at.slice(0, 10)}
            </span>
          )}
        </li>
      ))}
      {mentions.length > 40 && (
        <li className="italic">…and {mentions.length - 40} more</li>
      )}
    </SectionShell>
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
