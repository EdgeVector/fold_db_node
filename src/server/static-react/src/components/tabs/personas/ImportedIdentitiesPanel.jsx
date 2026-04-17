import { useCallback, useEffect, useState } from 'react'
import { listIdentities } from '../../../api/clients/fingerprintsClient'

/**
 * "Identities" audit panel — surfaces every Identity record on this
 * node, joined with its IdentityReceipt (when + how received). The
 * node's own self-Identity appears here too, marked with a "you"
 * badge.
 *
 * Rows sort newest-first by `received_at` so a fresh import appears
 * at the top of the list right after the user pastes a card.
 *
 * This panel is read-only today. "Remove this imported identity"
 * is a follow-up.
 */
export default function ImportedIdentitiesPanel() {
  const [identities, setIdentities] = useState([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)

  const fetchRows = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const res = await listIdentities()
      if (res.success) {
        setIdentities(res.data?.identities ?? [])
      } else {
        setError(res.error ?? 'Failed to load identities')
      }
    } catch (e) {
      setError(e?.message ?? 'Network error')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    fetchRows()
  }, [fetchRows])

  return (
    <div className="card p-4 space-y-3" data-testid="imported-identities-panel">
      <div className="flex items-center justify-between">
        <h3 className="text-base font-semibold">Identities</h3>
        <button
          type="button"
          className="btn-secondary text-xs"
          onClick={fetchRows}
          disabled={loading}
          data-testid="imported-identities-refresh"
        >
          {loading ? 'Loading…' : 'Refresh'}
        </button>
      </div>

      {error && (
        <div className="text-sm text-gruvbox-red" data-testid="imported-identities-error">
          {error}
        </div>
      )}

      {!loading && !error && identities.length === 0 && (
        <div className="text-sm text-secondary" data-testid="imported-identities-empty">
          No Identity cards on this node yet. Your own card is issued
          when you complete the setup wizard; peer cards are imported
          via the Import Card tab.
        </div>
      )}

      {identities.length > 0 && (
        <ul className="divide-y divide-border text-xs" data-testid="imported-identities-list">
          {identities.map(row => (
            <li
              key={row.identity_id}
              className="py-2 flex items-start gap-3"
              data-testid={`imported-identity-row-${row.identity_id}`}
            >
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="font-medium text-primary">
                    {row.display_name || '(unnamed)'}
                  </span>
                  {row.is_self && (
                    <span
                      className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-blue/10 text-gruvbox-blue border border-gruvbox-blue/30"
                      data-testid="badge-self"
                    >
                      you
                    </span>
                  )}
                  {row.trust_level && (
                    <span className="text-[10px] px-2 py-0.5 rounded-full bg-gruvbox-green/10 text-gruvbox-green border border-gruvbox-green/30">
                      {row.trust_level}
                    </span>
                  )}
                </div>
                <div className="text-[11px] text-tertiary font-mono break-all mt-0.5">
                  {row.identity_id}
                </div>
                <div className="text-[11px] text-secondary mt-1">
                  {row.received_via && (
                    <>
                      via <span className="text-tertiary">{row.received_via}</span>
                      {row.received_at && (
                        <>
                          {' · '}
                          <span className="text-tertiary">{row.received_at}</span>
                        </>
                      )}
                    </>
                  )}
                  {!row.received_via && (
                    <span className="text-tertiary italic">no receipt</span>
                  )}
                </div>
              </div>
              <div className="text-[10px] text-tertiary whitespace-nowrap">
                issued {row.issued_at}
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
