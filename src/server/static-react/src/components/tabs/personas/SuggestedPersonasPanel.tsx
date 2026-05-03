import { useCallback, useEffect, useState } from 'react'
import {
  listSuggestedPersonas,
  acceptSuggestedPersona,
  RELATIONSHIP_OPTIONS,
  type SuggestedPersonaView,
} from '../../../api/clients/fingerprintsClient'

/**
 * localStorage key used to persist the set of dismissed suggestion
 * ids across page reloads. Versioned so a future data-format change
 * can invalidate the old set without stranding stale entries.
 */
const DISMISSED_STORAGE_KEY = 'folddb.dismissed_suggested_personas.v1'

/**
 * Load the persisted dismissed-id set from localStorage. Returns an
 * empty Set when the key is missing, the value isn't a JSON array,
 * or storage is unavailable (e.g. Safari private mode).
 *
 * Exported so the vitest suite can poke at it directly.
 */
export function loadDismissedFromStorage(): Set<string> {
  try {
    const raw = window.localStorage.getItem(DISMISSED_STORAGE_KEY)
    if (!raw) return new Set<string>()
    const parsed = JSON.parse(raw)
    if (!Array.isArray(parsed)) return new Set<string>()
    return new Set<string>(parsed.filter((x: unknown) => typeof x === 'string') as string[])
  } catch {
    return new Set<string>()
  }
}

/**
 * Persist a dismissed-id Set back to localStorage. Swallows any
 * write failure (quota exceeded, storage unavailable) — dismissals
 * are a UX nicety, not a correctness requirement.
 */
export function saveDismissedToStorage(set: Set<string>) {
  try {
    window.localStorage.setItem(
      DISMISSED_STORAGE_KEY,
      JSON.stringify(Array.from(set)),
    )
  } catch {
    // ignore
  }
}

/**
 * Suggested Personas panel — the design doc's dense-subgraph sweep
 * surface. Renders every candidate cluster the backend proposed,
 * with [Name it] to promote into a real Persona and [Dismiss] to
 * hide the candidate for the rest of this session (frontend-only
 * soft state — no backend write).
 *
 * Data shape: GET /api/fingerprints/suggestions. Accept goes through
 * POST /api/fingerprints/suggestions/accept and returns the
 * freshly-resolved PersonaDetailResponse.
 */
export default function SuggestedPersonasPanel() {
  const [suggestions, setSuggestions] = useState<SuggestedPersonaView[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [dismissed, setDismissed] = useState<Set<string>>(() => loadDismissedFromStorage())
  const [namingId, setNamingId] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)

  const fetchList = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const res = await listSuggestedPersonas()
      if (res.success) {
        setSuggestions(res.data?.suggestions ?? [])
      } else {
        setError(res.error ?? 'Failed to load suggestions')
      }
    } catch (e) {
      setError((e as Error)?.message ?? 'Network error')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    fetchList()
  }, [fetchList])

  const handleDismiss = useCallback(
    (id: string) => {
      setDismissed(prev => {
        const next = new Set(prev)
        next.add(id)
        saveDismissedToStorage(next)
        return next
      })
      if (namingId === id) setNamingId(null)
    },
    [namingId],
  )

  const handleClearDismissals = useCallback(() => {
    setDismissed(new Set<string>())
    saveDismissedToStorage(new Set<string>())
  }, [])

  const handleAccept = useCallback(
    async (suggestion: SuggestedPersonaView, name: string, relationship: string) => {
      const trimmed = name.trim()
      if (!trimmed) return
      setBusyId(suggestion.suggested_id)
      try {
        const res = await acceptSuggestedPersona({
          fingerprint_ids: suggestion.fingerprint_ids,
          name: trimmed,
          relationship,
        })
        if (res.success) {
          // Remove the accepted suggestion from the list — it's a
          // real Persona now and the Personas tab will pick it up
          // on next refresh.
          setSuggestions(prev =>
            prev.filter(s => s.suggested_id !== suggestion.suggested_id),
          )
          setNamingId(null)
        } else {
          setError(res.error ?? 'Failed to accept suggestion')
        }
      } catch (e) {
        setError((e as Error)?.message ?? 'Network error while accepting')
      } finally {
        setBusyId(null)
      }
    },
    [],
  )

  const visible = suggestions.filter(s => !dismissed.has(s.suggested_id))

  return (
    <div className="card p-3" data-testid="suggested-personas-panel">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-base font-semibold">Suggested clusters</h3>
        <div className="flex items-center gap-2">
          {dismissed.size > 0 && (
            <button
              type="button"
              className="btn-secondary text-xs"
              onClick={handleClearDismissals}
              data-testid="suggested-personas-clear-dismissed"
              title="Show every suggestion you previously dismissed"
            >
              Clear {dismissed.size} dismissed
            </button>
          )}
          <button
            type="button"
            className="btn-secondary text-xs"
            onClick={fetchList}
            disabled={loading}
            data-testid="suggested-personas-refresh"
          >
            {loading ? 'Loading…' : 'Refresh'}
          </button>
        </div>
      </div>

      <p className="text-[11px] text-tertiary mb-2">
        Dense subgraphs the resolver spotted in your fingerprint graph.
        Name one to turn it into a Persona. Dismissals are remembered
        across reloads — use &quot;Clear N dismissed&quot; to bring them
        back.
      </p>

      {error && (
        <div
          className="text-sm text-gruvbox-red mb-2"
          data-testid="suggested-personas-error"
        >
          {error}
        </div>
      )}

      {!loading && !error && visible.length === 0 && (
        <div
          className="text-sm text-secondary"
          data-testid="suggested-personas-empty"
        >
          No suggestions right now — either every dense cluster is
          already a Persona, or you haven&apos;t ingested enough signal
          yet.
        </div>
      )}

      <ul className="space-y-2">
        {visible.map(suggestion => (
          <SuggestedRow
            key={suggestion.suggested_id}
            suggestion={suggestion}
            naming={namingId === suggestion.suggested_id}
            busy={busyId === suggestion.suggested_id}
            onBeginNaming={() => setNamingId(suggestion.suggested_id)}
            onCancelNaming={() => setNamingId(null)}
            onDismiss={() => handleDismiss(suggestion.suggested_id)}
            onAccept={(name, relationship) =>
              handleAccept(suggestion, name, relationship)
            }
          />
        ))}
      </ul>
    </div>
  )
}

interface SuggestedRowProps {
  suggestion: SuggestedPersonaView
  naming: boolean
  busy: boolean
  onBeginNaming: () => void
  onCancelNaming: () => void
  onDismiss: () => void
  onAccept: (name: string, relationship: string) => void
}

function SuggestedRow({
  suggestion,
  naming,
  busy,
  onBeginNaming,
  onCancelNaming,
  onDismiss,
  onAccept,
}: SuggestedRowProps) {
  const [nameDraft, setNameDraft] = useState(suggestion.suggested_name)
  const [relationshipDraft, setRelationshipDraft] = useState('unknown')

  return (
    <li
      className="border border-gruvbox-yellow/40 bg-gruvbox-yellow/5 rounded p-2"
      data-testid={`suggested-row-${suggestion.suggested_id}`}
    >
      <div className="flex items-baseline gap-2">
        <span className="text-sm font-semibold">
          {suggestion.suggested_name}
        </span>
        <span className="text-[11px] text-tertiary">
          · {suggestion.fingerprint_count} fps · {suggestion.edge_count} edges
          · {suggestion.mention_count} mentions
        </span>
      </div>

      {suggestion.sample_fingerprints.length > 0 && (
        <ul
          className="text-[11px] text-tertiary mt-1 space-y-0.5"
          data-testid={`suggested-samples-${suggestion.suggested_id}`}
        >
          {suggestion.sample_fingerprints.slice(0, 5).map(fp => (
            <li key={fp.id} className="flex items-baseline gap-2">
              <span className="text-[9px] uppercase tracking-wider text-gruvbox-yellow bg-gruvbox-yellow/10 border border-gruvbox-yellow/30 rounded px-1.5 py-0.5 font-mono shrink-0">
                {fp.kind || 'unknown'}
              </span>
              <span className="truncate">{fp.display_value || '(empty)'}</span>
            </li>
          ))}
        </ul>
      )}

      <div className="flex items-center gap-2 mt-2">
        {naming ? (
          <>
            <input
              type="text"
              value={nameDraft}
              onChange={e => setNameDraft(e.target.value)}
              placeholder="Persona name"
              className="input text-xs flex-1"
              data-testid={`suggested-name-input-${suggestion.suggested_id}`}
              autoFocus
            />
            <select
              value={relationshipDraft}
              onChange={e => setRelationshipDraft(e.target.value)}
              className="input text-xs"
              data-testid={`suggested-relationship-select-${suggestion.suggested_id}`}
            >
              {RELATIONSHIP_OPTIONS.map(opt => (
                <option key={opt} value={opt}>
                  {opt}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="btn-primary text-xs"
              disabled={busy || !nameDraft.trim()}
              onClick={() => onAccept(nameDraft, relationshipDraft)}
              data-testid={`suggested-confirm-${suggestion.suggested_id}`}
            >
              {busy ? 'Creating…' : 'Create'}
            </button>
            <button
              type="button"
              className="btn-secondary text-xs"
              onClick={onCancelNaming}
              data-testid={`suggested-cancel-${suggestion.suggested_id}`}
            >
              Cancel
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className="btn-primary text-xs"
              onClick={onBeginNaming}
              data-testid={`suggested-name-${suggestion.suggested_id}`}
            >
              Name it
            </button>
            <button
              type="button"
              className="btn-secondary text-xs"
              onClick={onDismiss}
              data-testid={`suggested-dismiss-${suggestion.suggested_id}`}
            >
              Dismiss
            </button>
          </>
        )}
      </div>
    </li>
  )
}
