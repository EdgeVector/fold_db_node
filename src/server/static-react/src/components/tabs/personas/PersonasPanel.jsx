import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  listPersonas,
  getPersona,
  updatePersona,
  deletePersona,
  mergePersonas,
  acceptSuggestedPersona,
  RELATIONSHIP_OPTIONS,
} from '../../../api/clients/fingerprintsClient'

// ── Pure filter + sort helpers ─────────────────────────────────────
//
// Extracted so unit tests can exercise them without mounting the
// component. The Personas list can grow into the dozens on a
// dogfood node, so this is the Tier-3 navigability win: filter by
// name/alias substring, sort by recency / name / mentions / trust.

/**
 * Allowed sort modes. Exported so tests and the <select> options
 * stay in sync without magic strings drifting between files.
 */
export const PERSONA_SORT_OPTIONS = [
  { value: 'recent', label: 'Most recent' },
  { value: 'name_asc', label: 'Name (A–Z)' },
  { value: 'mentions_desc', label: 'Most mentions' },
  { value: 'trust_tier_desc', label: 'Trust tier (high→low)' },
]

const PERSONA_SORT_VALUES = new Set(PERSONA_SORT_OPTIONS.map(o => o.value))

/**
 * Case-insensitive substring filter on `name` + `aliases`. Trimmed
 * input; an empty string returns the list unchanged. Stable: order
 * of the input is preserved when entries match.
 */
export function filterPersonas(personas, rawQuery) {
  const q = (rawQuery ?? '').trim().toLowerCase()
  if (q === '') return personas
  return personas.filter(p => {
    const name = (p.name ?? '').toLowerCase()
    if (name.includes(q)) return true
    const aliases = Array.isArray(p.aliases) ? p.aliases : []
    return aliases.some(a => (a ?? '').toLowerCase().includes(q))
  })
}

/**
 * Sort a persona list by one of the allowed modes. Returns a new
 * array — never mutates the input. Unknown modes throw loudly so a
 * typo doesn't silently fall back to a different ordering.
 *
 * - `recent`: by `created_at` desc, nulls last (older records that
 *   pre-date the field sink to the bottom). Tie-breaks on `id` for
 *   determinism.
 * - `name_asc`: case-insensitive locale compare on `name`.
 * - `mentions_desc`: by `mention_count` desc, tie-break on name.
 * - `trust_tier_desc`: by `trust_tier` desc, tie-break on name.
 */
export function sortPersonas(personas, mode) {
  if (!PERSONA_SORT_VALUES.has(mode)) {
    throw new Error(`sortPersonas: unknown mode "${mode}"`)
  }
  const out = personas.slice()
  if (mode === 'recent') {
    out.sort((a, b) => {
      const aHas = typeof a.created_at === 'string' && a.created_at !== ''
      const bHas = typeof b.created_at === 'string' && b.created_at !== ''
      if (aHas && !bHas) return -1
      if (!aHas && bHas) return 1
      if (aHas && bHas && a.created_at !== b.created_at) {
        return a.created_at < b.created_at ? 1 : -1
      }
      return (a.id ?? '').localeCompare(b.id ?? '')
    })
    return out
  }
  if (mode === 'name_asc') {
    out.sort((a, b) =>
      (a.name ?? '').localeCompare(b.name ?? '', undefined, {
        sensitivity: 'base',
      }),
    )
    return out
  }
  if (mode === 'mentions_desc') {
    out.sort((a, b) => {
      const diff = (b.mention_count ?? 0) - (a.mention_count ?? 0)
      if (diff !== 0) return diff
      return (a.name ?? '').localeCompare(b.name ?? '')
    })
    return out
  }
  // trust_tier_desc
  out.sort((a, b) => {
    const diff = (b.trust_tier ?? 0) - (a.trust_tier ?? 0)
    if (diff !== 0) return diff
    return (a.name ?? '').localeCompare(b.name ?? '')
  })
  return out
}

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

  const applyPatch = useCallback(
    async (patch, errorLabel) => {
      if (!selectedId) return
      try {
        const res = await updatePersona(selectedId, patch)
        if (res.success && res.data) {
          // Replace the detail in place so the user sees the freshly
          // resolved cluster counts + diagnostics without a second GET.
          setDetail(res.data)
          // And refetch the list so the summary row for this persona
          // picks up the new counts too.
          fetchList()
        } else {
          setDetailError(res.error ?? errorLabel)
        }
      } catch (e) {
        setDetailError(e?.message ?? `Network error while ${errorLabel.toLowerCase()}`)
      }
    },
    [selectedId, fetchList],
  )

  const handleThresholdCommit = useCallback(
    nextThreshold =>
      applyPatch({ threshold: nextThreshold }, 'Failed to update threshold'),
    [applyPatch],
  )

  // Undo snackbar state for ✂ exclude actions. Shows "Excluded —
  // Undo" for 5 seconds after each exclude, giving the user a fast
  // recovery path without scrolling to the exclusions panel.
  const [undoSnack, setUndoSnack] = useState(null)
  useEffect(() => {
    if (!undoSnack) return
    const timer = setTimeout(() => setUndoSnack(null), 5000)
    return () => clearTimeout(timer)
  }, [undoSnack])

  const handleExcludeMention = useCallback(
    id => {
      applyPatch({ add_excluded_mention_id: id }, 'Failed to exclude mention')
      setUndoSnack({ type: 'mention', id })
    },
    [applyPatch],
  )

  const handleUnexcludeMention = useCallback(
    id => {
      applyPatch({ remove_excluded_mention_id: id }, 'Failed to un-exclude mention')
      setUndoSnack(null)
    },
    [applyPatch],
  )

  const handleExcludeEdge = useCallback(
    id => {
      applyPatch({ add_excluded_edge_id: id }, 'Failed to exclude edge')
      setUndoSnack({ type: 'edge', id })
    },
    [applyPatch],
  )

  const handleUnexcludeEdge = useCallback(
    id => {
      applyPatch({ remove_excluded_edge_id: id }, 'Failed to un-exclude edge')
      setUndoSnack(null)
    },
    [applyPatch],
  )

  const handleRenamePersona = useCallback(
    name => applyPatch({ name }, 'Failed to rename persona'),
    [applyPatch],
  )

  const handleRelationshipChange = useCallback(
    relationship =>
      applyPatch({ relationship }, 'Failed to update relationship'),
    [applyPatch],
  )

  const handleConfirm = useCallback(
    () => applyPatch({ user_confirmed: true }, 'Failed to confirm persona'),
    [applyPatch],
  )

  const handleUnlinkIdentity = useCallback(() => {
    // Confirm because unlinking drops the verified badge — the user
    // loses the cryptographic anchor even though the underlying
    // Identity record survives on the node and can be re-linked.
    const ok = window.confirm(
      'Unlink this persona from its verified identity? The Identity record stays on the node and can be re-linked later.',
    )
    if (!ok) return
    applyPatch({ clear_identity_id: true }, 'Failed to unlink identity')
  }, [applyPatch])

  const handleMerge = useCallback(
    async absorbedId => {
      if (!selectedId || !absorbedId || absorbedId === selectedId) return
      const absorbed = personas.find(p => p.id === absorbedId)
      const survivor = personas.find(p => p.id === selectedId)
      const ok = window.confirm(
        `Merge "${absorbed?.name ?? absorbedId}" INTO "${survivor?.name ?? selectedId}"?\n\n` +
          'The absorbed persona is deleted. Its seed fingerprints, aliases, ' +
          'and exclusions are folded into the survivor. The survivor keeps ' +
          'its own name, threshold, and relationship.',
      )
      if (!ok) return
      try {
        const res = await mergePersonas(selectedId, absorbedId)
        if (res.success) {
          setDetail(res.data ?? null)
          // Drop the absorbed row from the list; refetch to pick up
          // the survivor's new cluster counts.
          setPersonas(prev => prev.filter(p => p.id !== absorbedId))
          fetchList()
        } else {
          setDetailError(res.error ?? 'Failed to merge personas')
        }
      } catch (e) {
        setDetailError(e?.message ?? 'Network error while merging personas')
      }
    },
    [selectedId, personas, fetchList],
  )

  // Undo snack for delete — separate state from the exclude undoSnack
  // so a delete-undo can sit alongside an exclude-undo without one
  // clobbering the other. Auto-dismissed after 5s, same as the
  // exclude snack. Rendered in the same floating container, but
  // discriminated by a `type` field on each snack.
  const [deleteUndoSnack, setDeleteUndoSnack] = useState(null)
  useEffect(() => {
    if (!deleteUndoSnack) return
    // If a restore attempt failed, leave the error visible — the
    // user needs to read it. Otherwise auto-dismiss after 5s, same
    // as the exclude undo snack.
    if (deleteUndoSnack.error) return
    const timer = setTimeout(() => setDeleteUndoSnack(null), 5000)
    return () => clearTimeout(timer)
  }, [deleteUndoSnack])

  const handleDeletePersona = useCallback(async () => {
    if (!selectedId) return
    // Underlying Fingerprint / Mention / Edge records survive — the
    // confirm copy is deliberately about the cluster name, not the
    // graph contents, so the user understands what they're keeping.
    const ok = window.confirm(
      'Delete this persona? The underlying fingerprints, mentions, and edges will remain in the graph — only the saved cluster name and seeds are removed.'
    )
    if (!ok) return
    // Capture the snapshot needed to reconstruct the persona BEFORE
    // we issue the delete — once detail is cleared we can't recover
    // the seed set. acceptSuggestedPersona only needs seeds + name +
    // relationship to materialize a fresh Persona record. Note that
    // the restored persona gets a new id (uuid-generated server-side)
    // and identity_id is NOT re-linked — the user can re-link it
    // manually if needed.
    const snapshot = detail
      ? {
          name: detail.name,
          relationship: detail.relationship,
          seed_fingerprint_ids: [...(detail.seed_fingerprint_ids || [])],
        }
      : null
    try {
      const res = await deletePersona(selectedId)
      if (res.success) {
        // Drop the row from the list and clear the detail pane.
        setPersonas(prev => prev.filter(p => p.id !== selectedId))
        setDetail(null)
        setSelectedId(null)
        if (snapshot && snapshot.seed_fingerprint_ids.length > 0) {
          setDeleteUndoSnack({ type: 'delete', snapshot, error: null })
        }
      } else {
        setDetailError(res.error ?? 'Failed to delete persona')
      }
    } catch (e) {
      setDetailError(e?.message ?? 'Network error while deleting persona')
    }
  }, [selectedId, detail])

  const handleUndoDelete = useCallback(async () => {
    if (!deleteUndoSnack || deleteUndoSnack.type !== 'delete') return
    const { snapshot } = deleteUndoSnack
    try {
      const res = await acceptSuggestedPersona({
        fingerprint_ids: snapshot.seed_fingerprint_ids,
        name: snapshot.name,
        relationship: snapshot.relationship,
      })
      if (res.success && res.data) {
        // Refresh the list and jump into the restored persona so the
        // user can confirm it came back. The restored persona has a
        // new id since acceptSuggestedPersona allocates a fresh UUID.
        await fetchList()
        setDetail(res.data)
        setSelectedId(res.data.id)
        setDeleteUndoSnack(null)
      } else {
        // Surface the error inside the snack rather than silently
        // dropping it — never want a "click Undo, nothing happens"
        // experience.
        setDeleteUndoSnack(s =>
          s ? { ...s, error: res.error ?? 'Failed to restore persona' } : s,
        )
      }
    } catch (e) {
      setDeleteUndoSnack(s =>
        s
          ? {
              ...s,
              error: e?.message ?? 'Network error while restoring persona',
            }
          : s,
      )
    }
  }, [deleteUndoSnack, fetchList])

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
        onExcludeMention={handleExcludeMention}
        onUnexcludeMention={handleUnexcludeMention}
        onExcludeEdge={handleExcludeEdge}
        onUnexcludeEdge={handleUnexcludeEdge}
        onRenamePersona={handleRenamePersona}
        onRelationshipChange={handleRelationshipChange}
        onConfirm={handleConfirm}
        onDelete={handleDeletePersona}
        onUnlinkIdentity={handleUnlinkIdentity}
        onMerge={handleMerge}
        mergeCandidates={personas}
      />
      {deleteUndoSnack && deleteUndoSnack.type === 'delete' && (
        <div
          className="fixed bottom-16 left-1/2 -translate-x-1/2 z-50 px-4 py-2 rounded-lg bg-gruvbox-yellow/20 border border-gruvbox-yellow/40 flex flex-col gap-1 text-xs shadow-lg backdrop-blur max-w-sm"
          data-testid="persona-delete-undo-snack"
        >
          <div className="flex items-center gap-3">
            <span>
              Deleted{' '}
              <span className="font-semibold">
                {deleteUndoSnack.snapshot.name || '(unnamed)'}
              </span>
              . Undo restores it with a new id (identity link is not
              re-created).
            </span>
            <button
              type="button"
              className="text-gruvbox-yellow underline underline-offset-2 font-semibold ml-auto shrink-0"
              onClick={handleUndoDelete}
              data-testid="persona-delete-undo-button"
            >
              Undo
            </button>
          </div>
          {deleteUndoSnack.error && (
            <div
              className="text-gruvbox-red"
              data-testid="persona-delete-undo-error"
            >
              {deleteUndoSnack.error}
            </div>
          )}
        </div>
      )}
      {undoSnack && (
        <div
          className="fixed bottom-4 left-1/2 -translate-x-1/2 z-50 px-4 py-2 rounded-lg bg-gruvbox-blue/20 border border-gruvbox-blue/40 flex items-center gap-3 text-xs shadow-lg backdrop-blur"
          data-testid="persona-undo-snack"
        >
          <span>
            Excluded {undoSnack.type === 'mention' ? 'mention' : 'edge'}{' '}
            <span className="font-mono text-tertiary">{undoSnack.id.slice(0, 12)}…</span>
          </span>
          <button
            type="button"
            className="text-gruvbox-blue underline underline-offset-2 font-semibold"
            onClick={() => {
              if (undoSnack.type === 'mention') handleUnexcludeMention(undoSnack.id)
              else handleUnexcludeEdge(undoSnack.id)
            }}
            data-testid="persona-undo-snack-undo"
          >
            Undo
          </button>
        </div>
      )}
    </div>
  )
}

function PersonaList({ personas, loading, error, selectedId, onSelect, onRefresh }) {
  // Filter + sort live entirely in the list pane — no parent state,
  // no persistence. Reset on reload by design (alpha scope).
  const [filterText, setFilterText] = useState('')
  const [sortMode, setSortMode] = useState('recent')

  const visible = useMemo(() => {
    const filtered = filterPersonas(personas, filterText)
    return sortPersonas(filtered, sortMode)
  }, [personas, filterText, sortMode])

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

      <div className="flex items-center gap-2 mb-3">
        <input
          type="text"
          value={filterText}
          onChange={e => setFilterText(e.target.value)}
          placeholder="Filter by name or alias…"
          className="input text-xs flex-1"
          data-testid="persona-list-filter"
          aria-label="Filter personas by name or alias"
        />
        <select
          value={sortMode}
          onChange={e => setSortMode(e.target.value)}
          className="input text-xs py-1"
          data-testid="persona-list-sort"
          aria-label="Sort personas"
        >
          {PERSONA_SORT_OPTIONS.map(opt => (
            <option key={opt.value} value={opt.value}>
              {opt.label}
            </option>
          ))}
        </select>
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

      {!loading && !error && personas.length > 0 && visible.length === 0 && (
        <div
          className="text-sm text-secondary"
          data-testid="persona-list-empty-filtered"
        >
          No personas match “{filterText.trim()}”. Try a different filter
          or clear the search.
        </div>
      )}

      <ul className="space-y-1">
        {visible.map(p => (
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
                  {!p.user_confirmed && !p.built_in && (
                    <span
                      className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-yellow/10 text-gruvbox-yellow border border-gruvbox-yellow/30"
                      data-testid="badge-tentative"
                      title="Auto-created by the sweep. Confirm to keep, or delete to reject."
                    >
                      tentative
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

function PersonaDetail({
  selectedId,
  detail,
  loading,
  error,
  onThresholdCommit,
  onExcludeMention,
  onUnexcludeMention,
  onExcludeEdge,
  onUnexcludeEdge,
  onRenamePersona,
  onRelationshipChange,
  onConfirm,
  onDelete,
  onUnlinkIdentity,
  onMerge,
  mergeCandidates,
}) {
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
    <div className="lg:w-1/2 card p-3 space-y-3 overflow-y-auto max-h-[calc(100vh-12rem)]" data-testid="persona-detail">
      {loading && <div className="text-sm text-secondary">Loading…</div>}
      {error && (
        <div className="text-sm text-gruvbox-red" data-testid="persona-detail-error">
          {error}
        </div>
      )}
      {!loading && !error && detail && (
        <PersonaDetailBody
          detail={detail}
          onThresholdCommit={onThresholdCommit}
          onExcludeMention={onExcludeMention}
          onUnexcludeMention={onUnexcludeMention}
          onExcludeEdge={onExcludeEdge}
          onUnexcludeEdge={onUnexcludeEdge}
          onRenamePersona={onRenamePersona}
          onRelationshipChange={onRelationshipChange}
          onConfirm={onConfirm}
          onDelete={onDelete}
          onUnlinkIdentity={onUnlinkIdentity}
          onMerge={onMerge}
          mergeCandidates={mergeCandidates}
        />
      )}
    </div>
  )
}

function PersonaDetailBody({
  detail,
  onThresholdCommit,
  onExcludeMention,
  onUnexcludeMention,
  onExcludeEdge,
  onUnexcludeEdge,
  onRenamePersona,
  onRelationshipChange,
  onConfirm,
  onDelete,
  onUnlinkIdentity,
  onMerge,
  mergeCandidates,
}) {
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

  // Local name-edit state. Tracks an "editing" flag so the header
  // can swap between static text and an inline input without a
  // dialog. Sync when the parent replaces `detail` so stale drafts
  // don't leak across persona switches.
  const [editingName, setEditingName] = useState(false)
  const [nameDraft, setNameDraft] = useState(detail.name || '')
  useEffect(() => {
    setNameDraft(detail.name || '')
    setEditingName(false)
  }, [detail.id, detail.name])

  const commitName = () => {
    const trimmed = nameDraft.trim()
    if (!trimmed || trimmed === detail.name) {
      setEditingName(false)
      return
    }
    if (typeof onRenamePersona === 'function') {
      onRenamePersona(trimmed)
    }
    setEditingName(false)
  }

  // Build an id→label index from enriched fingerprints so EdgeRows
  // can render readable labels for each endpoint instead of raw
  // fp_abc123 hashes. Cheap — fingerprints array is small and this
  // runs once per detail render.
  const fpIndex = (detail.fingerprints || []).reduce((acc, fp) => {
    const label = fp.kind === 'face_embedding'
      ? fp.sample_source
        ? `face · ${fp.sample_source}`
        : `face · ${fp.short_id || fp.id.slice(3, 11)}`
      : fp.display_value || fp.short_id || fp.id.slice(3, 11)
    acc[fp.id] = label
    return acc
  }, {})

  return (
    <>
      <header>
        <div className="flex items-center gap-2">
          {editingName && !detail.built_in ? (
            <input
              type="text"
              value={nameDraft}
              onChange={e => setNameDraft(e.target.value)}
              onBlur={commitName}
              onKeyDown={e => {
                if (e.key === 'Enter') commitName()
                if (e.key === 'Escape') {
                  setNameDraft(detail.name || '')
                  setEditingName(false)
                }
              }}
              className="input text-sm font-semibold"
              data-testid="persona-name-input"
              autoFocus
            />
          ) : (
            <button
              type="button"
              className={`text-base font-semibold text-left decoration-dotted ${
                detail.built_in
                  ? 'cursor-not-allowed opacity-80'
                  : 'hover:underline cursor-text'
              }`}
              title={detail.built_in ? "Built-in personas can't be renamed — update the IdentityCard" : 'Click to rename'}
              onClick={() => {
                if (!detail.built_in) setEditingName(true)
              }}
              data-testid="persona-name-button"
            >
              {detail.name || '(unnamed)'}
            </button>
          )}
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
          {!detail.user_confirmed && !detail.built_in && (
            <span
              className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-yellow/10 text-gruvbox-yellow border border-gruvbox-yellow/30"
              data-testid="persona-detail-badge-tentative"
              title="Auto-created by the sweep. Review and confirm, or delete."
            >
              tentative
            </span>
          )}
          {!detail.built_in && typeof onMerge === 'function' && (
            <MergeIntoControl
              detail={detail}
              mergeCandidates={mergeCandidates}
              onMerge={onMerge}
            />
          )}
          {!detail.built_in && typeof onDelete === 'function' && (
            <button
              type="button"
              className="text-[11px] text-tertiary hover:text-gruvbox-red underline underline-offset-2 shrink-0"
              onClick={onDelete}
              data-testid="persona-delete-button"
              title="Delete this persona. Underlying fingerprints, mentions, and edges remain in the graph."
            >
              Delete persona
            </button>
          )}
        </div>
        {!detail.user_confirmed && !detail.built_in && typeof onConfirm === 'function' && (
          <div
            className="mt-2 p-2 rounded border border-gruvbox-yellow/40 bg-gruvbox-yellow/5 flex items-center justify-between gap-2"
            data-testid="persona-confirm-banner"
          >
            <span className="text-xs text-secondary">
              This persona was auto-created from a dense cluster.
              Review the fingerprints and mentions below, then confirm
              or rename to keep it.
            </span>
            <button
              type="button"
              className="btn-primary text-xs shrink-0"
              onClick={onConfirm}
              data-testid="persona-confirm-button"
              title="Mark this persona as confirmed. You can still rename, adjust threshold, or exclude edges/mentions later."
            >
              Confirm
            </button>
          </div>
        )}
        <div className="text-xs text-tertiary mt-1 font-mono break-all">{detail.id}</div>
        <div className="text-xs text-secondary mt-1 flex flex-wrap items-center gap-x-2 gap-y-1">
          <div className="flex items-center gap-1.5 shrink-0">
            <label htmlFor={`persona-relationship-${detail.id}`} className="text-tertiary">
              Relationship:
            </label>
            <select
              id={`persona-relationship-${detail.id}`}
              value={detail.relationship || 'unknown'}
              onChange={e => {
                if (typeof onRelationshipChange === 'function') {
                  onRelationshipChange(e.target.value)
                }
              }}
              className="input text-xs py-0.5"
              data-testid="persona-relationship-select"
            >
              {RELATIONSHIP_OPTIONS.map(opt => (
                <option key={opt} value={opt}>
                  {opt}
                </option>
              ))}
            </select>
          </div>
          <span className="text-tertiary shrink-0">tier {detail.trust_tier}</span>
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
      <EdgeRows
        edges={detail.edges || []}
        fallbackIds={detail.edge_ids}
        onExclude={onExcludeEdge}
        fingerprintIndex={fpIndex}
      />
      <MentionRows
        mentions={detail.mentions || []}
        fallbackIds={detail.mention_ids}
        onExclude={onExcludeMention}
      />

      <ExclusionsPanel
        detail={detail}
        onUnexcludeMention={onUnexcludeMention}
        onUnexcludeEdge={onUnexcludeEdge}
      />

      {detail.identity_id && (
        <div className="text-xs text-secondary flex items-center justify-between gap-2">
          <div>
            <span className="text-tertiary">Identity: </span>
            <span className="font-mono break-all">{detail.identity_id}</span>
          </div>
          {!detail.built_in && typeof onUnlinkIdentity === 'function' && (
            <button
              type="button"
              className="text-[11px] text-tertiary hover:text-gruvbox-yellow underline underline-offset-2 shrink-0"
              onClick={onUnlinkIdentity}
              data-testid="persona-unlink-identity-button"
              title="Unlink this persona from its verified identity. The Identity record stays on the node — only the link is cleared."
            >
              Unlink identity
            </button>
          )}
        </div>
      )}
    </>
  )
}

// Inline dropdown used in the persona detail header. Shown only on
// non-built-in personas; excludes the current persona and any
// built-in personas from the "merge into" options.
function MergeIntoControl({ detail, mergeCandidates = [], onMerge }) {
  const [value, setValue] = useState('')
  const candidates = mergeCandidates.filter(
    p => p.id !== detail.id && !p.built_in,
  )
  if (candidates.length === 0) return null
  return (
    <div
      className="ml-auto flex items-center gap-1 shrink-0"
      data-testid="persona-merge-control"
    >
      <select
        value={value}
        onChange={e => setValue(e.target.value)}
        className="input text-[11px] py-0.5"
        data-testid="persona-merge-select"
        aria-label="Merge another persona into this one"
      >
        <option value="">— merge another into this —</option>
        {candidates.map(p => (
          <option key={p.id} value={p.id}>
            {p.name || '(unnamed)'} · {p.relationship}
          </option>
        ))}
      </select>
      <button
        type="button"
        className="text-[11px] text-tertiary hover:text-gruvbox-yellow underline underline-offset-2"
        disabled={!value}
        onClick={() => {
          if (!value) return
          onMerge(value)
          setValue('')
        }}
        data-testid="persona-merge-button"
        title="Fold the selected persona's seeds/aliases into this one and delete the absorbed record."
      >
        Merge
      </button>
    </div>
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
          {fp.sample_source && (
            <a
              href="#data-browser"
              className="font-mono text-tertiary text-[10px] truncate shrink-0 hover:underline decoration-dotted"
              title={
                fp.sample_mention_at
                  ? `Open Browse tab — ${fp.sample_source} · ${fp.sample_mention_at}`
                  : `Open Browse tab — ${fp.sample_source}`
              }
            >
              · {fp.sample_source}
              {fp.sample_source_field ? ` · ${fp.sample_source_field}` : ''}
            </a>
          )}
          {fp.short_id && (
            <span
              className="font-mono text-[10px] text-tertiary shrink-0 ml-auto"
              title={fp.id}
            >
              {fp.short_id}
            </span>
          )}
        </li>
      ))}
      {fingerprints.length > 40 && (
        <li className="italic">…and {fingerprints.length - 40} more</li>
      )}
    </SectionShell>
  )
}

function EdgeRows({ edges, fallbackIds, onExclude, fingerprintIndex = {} }) {
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
          <span className="truncate" title={e.a}>
            {fingerprintIndex[e.a] || e.a.slice(0, 16) + '…'}
          </span>
          <span className="text-tertiary shrink-0">—</span>
          <span className="truncate" title={e.b}>
            {fingerprintIndex[e.b] || e.b.slice(0, 16) + '…'}
          </span>
          <span className="ml-auto font-mono text-gruvbox-green shrink-0">
            {e.weight.toFixed(2)}
          </span>
          {typeof onExclude === 'function' && (
            <button
              type="button"
              title="Exclude this edge from the persona"
              aria-label="Exclude edge"
              className="text-tertiary hover:text-gruvbox-red shrink-0"
              onClick={() => onExclude(e.id)}
              data-testid={`persona-edge-exclude-${e.id}`}
            >
              ✂
            </button>
          )}
        </li>
      ))}
      {edges.length > 40 && (
        <li className="italic">…and {edges.length - 40} more</li>
      )}
    </SectionShell>
  )
}

function MentionRows({ mentions, fallbackIds, onExclude }) {
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
          <a
            href="#data-browser"
            className="truncate hover:underline decoration-dotted"
            title={`Open Browse tab — ${m.source_schema}:${m.source_key}`}
            data-testid={`persona-mention-link-${m.id}`}
          >
            <span className="font-mono text-secondary">{m.source_schema}</span>
            <span className="text-tertiary">:</span>
            <span className="font-mono">{m.source_key}</span>
            {m.source_field && (
              <span className="text-tertiary"> · {m.source_field}</span>
            )}
          </a>
          {m.created_at && (
            <span className="font-mono text-tertiary shrink-0 ml-auto">
              {m.created_at.slice(0, 10)}
            </span>
          )}
          {typeof onExclude === 'function' && (
            <button
              type="button"
              title="Exclude this mention from the persona"
              aria-label="Exclude mention"
              className={`text-tertiary hover:text-gruvbox-red shrink-0 ${m.created_at ? '' : 'ml-auto'}`}
              onClick={() => onExclude(m.id)}
              data-testid={`persona-mention-exclude-${m.id}`}
            >
              ✂
            </button>
          )}
        </li>
      ))}
      {mentions.length > 40 && (
        <li className="italic">…and {mentions.length - 40} more</li>
      )}
    </SectionShell>
  )
}

function ExclusionsPanel({ detail, onUnexcludeMention, onUnexcludeEdge }) {
  const excludedMentions = detail.excluded_mention_ids || []
  const excludedEdges = detail.excluded_edge_ids || []
  const [open, setOpen] = useState(false)
  const total = excludedMentions.length + excludedEdges.length

  if (total === 0) return null

  return (
    <div
      className="text-xs border-t border-border pt-2"
      data-testid="persona-exclusions-panel"
    >
      <button
        type="button"
        className="text-tertiary underline underline-offset-2"
        onClick={() => setOpen(o => !o)}
        data-testid="persona-exclusions-toggle"
      >
        {open ? 'Hide' : 'Show'} {total} excluded item{total === 1 ? '' : 's'}
      </button>
      {open && (
        <div className="mt-2 space-y-2">
          {excludedMentions.length > 0 && (
            <div>
              <div className="text-[11px] text-secondary mb-1">
                Excluded mentions ({excludedMentions.length})
              </div>
              <ul className="space-y-0.5">
                {excludedMentions.map(id => (
                  <li
                    key={id}
                    className="flex items-baseline gap-2 text-[11px] font-mono text-tertiary"
                  >
                    <span className="truncate">{id}</span>
                    {typeof onUnexcludeMention === 'function' && (
                      <button
                        type="button"
                        className="ml-auto text-tertiary underline underline-offset-2 hover:text-gruvbox-green shrink-0"
                        onClick={() => onUnexcludeMention(id)}
                        data-testid={`persona-mention-unexclude-${id}`}
                      >
                        Undo
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {excludedEdges.length > 0 && (
            <div>
              <div className="text-[11px] text-secondary mb-1">
                Excluded edges ({excludedEdges.length})
              </div>
              <ul className="space-y-0.5">
                {excludedEdges.map(id => (
                  <li
                    key={id}
                    className="flex items-baseline gap-2 text-[11px] font-mono text-tertiary"
                  >
                    <span className="truncate">{id}</span>
                    {typeof onUnexcludeEdge === 'function' && (
                      <button
                        type="button"
                        className="ml-auto text-tertiary underline underline-offset-2 hover:text-gruvbox-green shrink-0"
                        onClick={() => onUnexcludeEdge(id)}
                        data-testid={`persona-edge-unexclude-${id}`}
                      >
                        Undo
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </div>
  )
}

function pl(n, singular, plural) {
  return n === 1 ? `${n} ${singular}` : `${n} ${plural}`
}

function Diagnostics({ diagnostics }) {
  const entries = []
  if (diagnostics.missing_seed_fingerprint_ids.length > 0) {
    const n = diagnostics.missing_seed_fingerprint_ids.length
    entries.push(
      `${pl(n, 'missing seed fingerprint', 'missing seed fingerprints')}: ${diagnostics.missing_seed_fingerprint_ids.join(', ')}`,
    )
  }
  if (diagnostics.excluded_edge_count > 0) {
    entries.push(`${pl(diagnostics.excluded_edge_count, 'edge', 'edges')} excluded by persona rules`)
  }
  if (diagnostics.forbidden_edge_count > 0) {
    entries.push(`${pl(diagnostics.forbidden_edge_count, 'UserForbidden edge', 'UserForbidden edges')} skipped`)
  }
  if (diagnostics.below_threshold_edge_count > 0) {
    entries.push(
      `${pl(diagnostics.below_threshold_edge_count, 'edge', 'edges')} below the current threshold`,
    )
  }
  if (diagnostics.excluded_mention_count > 0) {
    entries.push(
      `${pl(diagnostics.excluded_mention_count, 'mention', 'mentions')} excluded by persona rules`,
    )
  }
  if (diagnostics.dangling_edge_ids.length > 0) {
    entries.push(
      `${pl(diagnostics.dangling_edge_ids.length, 'dangling edge reference', 'dangling edge references')} — data inconsistency`,
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
