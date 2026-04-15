import { useCallback, useEffect, useState } from 'react'
import {
  listIngestionErrors,
  resolveIngestionError,
} from '../../../api/clients/fingerprintsClient'

/**
 * Failed records panel. Renders unresolved IngestionError rows so the
 * user can see exactly which ingestion jobs failed and on which source
 * records. Dismiss/Retry both PATCH the row to `resolved: true` — the
 * retry path is a stub for Phase 1 (real re-run lives in a later iter).
 *
 * Data shape comes from GET /api/fingerprints/ingestion-errors; see
 * fingerprintsClient.ts and src/handlers/fingerprints/ingestion_errors.rs.
 */
export default function IngestionErrorsPanel() {
  const [errors, setErrors] = useState([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)
  const [includeResolved, setIncludeResolved] = useState(false)
  const [expandedId, setExpandedId] = useState(null)
  const [busyId, setBusyId] = useState(null)

  const fetchList = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const res = await listIngestionErrors(includeResolved)
      if (res.success) {
        setErrors(res.data?.errors ?? [])
      } else {
        setError(res.error ?? 'Failed to load ingestion errors')
      }
    } catch (e) {
      setError(e?.message ?? 'Network error')
    } finally {
      setLoading(false)
    }
  }, [includeResolved])

  useEffect(() => {
    fetchList()
  }, [fetchList])

  const handleResolve = useCallback(
    async id => {
      setBusyId(id)
      try {
        const res = await resolveIngestionError(id)
        if (res.success) {
          // Drop the row from the open list (or flip its resolved
          // badge if we're showing resolved rows too).
          setErrors(prev => {
            if (includeResolved) {
              return prev.map(e =>
                e.id === id ? { ...e, resolved: true } : e,
              )
            }
            return prev.filter(e => e.id !== id)
          })
        } else {
          setError(res.error ?? 'Failed to dismiss ingestion error')
        }
      } catch (e) {
        setError(e?.message ?? 'Network error while dismissing')
      } finally {
        setBusyId(null)
      }
    },
    [includeResolved],
  )

  return (
    <div className="card p-3" data-testid="ingestion-errors-panel">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-base font-semibold">Failed records</h3>
        <div className="flex items-center gap-3">
          <label className="text-xs text-secondary flex items-center gap-1.5">
            <input
              type="checkbox"
              checked={includeResolved}
              onChange={e => setIncludeResolved(e.target.checked)}
              data-testid="ingestion-errors-include-resolved"
            />
            Show dismissed
          </label>
          <button
            type="button"
            className="btn-secondary text-xs"
            onClick={fetchList}
            disabled={loading}
            data-testid="ingestion-errors-refresh"
          >
            {loading ? 'Loading…' : 'Refresh'}
          </button>
        </div>
      </div>

      {error && (
        <div
          className="text-sm text-gruvbox-red mb-2"
          data-testid="ingestion-errors-error"
        >
          {error}
        </div>
      )}

      {!loading && !error && errors.length === 0 && (
        <div
          className="text-sm text-secondary"
          data-testid="ingestion-errors-empty"
        >
          No failed records — every extractor has been running cleanly.
        </div>
      )}

      <ul className="space-y-2">
        {errors.map(row => (
          <IngestionErrorRow
            key={row.id}
            row={row}
            expanded={expandedId === row.id}
            onToggle={() =>
              setExpandedId(prev => (prev === row.id ? null : row.id))
            }
            onResolve={() => handleResolve(row.id)}
            busy={busyId === row.id}
          />
        ))}
      </ul>
    </div>
  )
}

function IngestionErrorRow({ row, expanded, onToggle, onResolve, busy }) {
  const created = row.created_at ? row.created_at.slice(0, 10) : ''
  return (
    <li
      className={`border rounded p-2 ${
        row.resolved
          ? 'border-border bg-surface/50 opacity-60'
          : 'border-gruvbox-red/40 bg-gruvbox-red/5'
      }`}
      data-testid={`ingestion-error-row-${row.id}`}
    >
      <div className="flex items-center gap-2">
        <span className="text-[9px] uppercase tracking-wider text-gruvbox-aqua bg-gruvbox-aqua/10 border border-gruvbox-aqua/30 rounded px-1.5 py-0.5 font-mono shrink-0">
          {row.extractor}
        </span>
        <span className="font-mono text-sm truncate">
          <span className="text-secondary">{row.source_schema}</span>
          <span className="text-tertiary">:</span>
          {row.source_key}
        </span>
        {row.resolved && (
          <span className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-gray/10 text-tertiary border border-gruvbox-gray/30">
            dismissed
          </span>
        )}
        <span className="ml-auto font-mono text-[11px] text-tertiary shrink-0">
          {created}
        </span>
      </div>

      <div className="text-xs text-gruvbox-red mt-1 font-mono">
        {row.error_class || 'UnknownError'}
      </div>

      <div className="flex items-center gap-2 mt-1">
        <button
          type="button"
          className="text-[11px] text-tertiary underline underline-offset-2"
          onClick={onToggle}
          data-testid={`ingestion-error-toggle-${row.id}`}
        >
          {expanded ? 'Hide details' : 'Show details'}
        </button>
        <span className="text-tertiary text-[11px]">
          · retries: {row.retry_count}
        </span>
        {!row.resolved && (
          <button
            type="button"
            className="ml-auto btn-secondary text-xs"
            onClick={onResolve}
            disabled={busy}
            data-testid={`ingestion-error-dismiss-${row.id}`}
          >
            {busy ? 'Dismissing…' : 'Dismiss'}
          </button>
        )}
      </div>

      {expanded && row.error_msg && (
        <pre
          className="mt-2 text-[11px] text-tertiary font-mono whitespace-pre-wrap break-words bg-surface rounded border border-border p-2 max-h-40 overflow-y-auto"
          data-testid={`ingestion-error-msg-${row.id}`}
        >
          {row.error_msg}
        </pre>
      )}
    </li>
  )
}
