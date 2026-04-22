import { useCallback, useEffect, useRef, useState } from 'react'
import {
  acceptReceivedCard,
  dismissReceivedCard,
  listReceivedCards,
} from '../../../api/clients/fingerprintsClient'

/**
 * Inbound Identity Cards panel — the only UX for importing a peer's
 * card. Replaces the previous manual paste/QR import flow.
 *
 * Pairs with the outbound send half in MyIdentityCardPanel → Send to
 * contact. Rows arrive via the discovery poll loop, which pulls
 * `identity_card_send` payloads off the messaging_service inbox for
 * this node's pseudonyms and stores them as `pending` rows. This
 * panel lists them and lets the user Accept (runs the Ed25519
 * verifier + writes an Identity record) or Reject (records the
 * rejection without importing — row flips to `dismissed`).
 *
 * Status transitions:
 *   pending → accepted     (user clicks Accept, verify succeeds)
 *   pending → pending+error (user clicks Accept, verify failed;
 *                            row retains error so user can reject)
 *   pending → dismissed    (user clicks Reject)
 */
export default function ReceivedCardsPanel() {
  const [rows, setRows] = useState([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)
  // Per-row action state keyed by message_id; lets two rows toggle
  // disabled/in-flight independently.
  const [pendingAction, setPendingAction] = useState({})
  const [lastRefreshedAt, setLastRefreshedAt] = useState(null)
  // Mirror pendingAction into a ref so the 30s poll can read the
  // latest value without re-binding the interval every time a row's
  // accept/dismiss flips its in-flight flag.
  const pendingActionRef = useRef(pendingAction)
  useEffect(() => {
    pendingActionRef.current = pendingAction
  }, [pendingAction])

  const fetchRows = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const res = await listReceivedCards()
      if (res.success) {
        setRows(res.data?.received_cards ?? [])
        setLastRefreshedAt(Date.now())
      } else {
        setError(res.error ?? 'Failed to load received cards')
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

  // Auto-refresh the inbox every 30s while mounted. Suppress the
  // poll when any row is mid-action (accept/dismiss in-flight) so
  // we don't stomp the in-flight UI state with a refetch.
  useEffect(() => {
    const id = setInterval(() => {
      const anyPending = Object.values(pendingActionRef.current).some(
        v => v != null,
      )
      if (anyPending) return
      fetchRows()
    }, 30_000)
    return () => clearInterval(id)
  }, [fetchRows])

  const setRowPending = (id, flag) =>
    setPendingAction(prev => ({ ...prev, [id]: flag }))

  const handleAccept = useCallback(async row => {
    setRowPending(row.message_id, 'accept')
    try {
      const res = await acceptReceivedCard(row.message_id)
      if (res.success) {
        // Replace the row in place so the user sees the accepted
        // state without a full refetch.
        setRows(prev =>
          prev.map(r =>
            r.message_id === row.message_id ? res.data.received_card : r,
          ),
        )
      } else {
        // Refetch so we pick up the server-side error stamp on the
        // row (handler writes that before returning 4xx/5xx).
        await fetchRows()
      }
    } catch {
      await fetchRows()
    } finally {
      setRowPending(row.message_id, null)
    }
  }, [fetchRows])

  const handleDismiss = useCallback(async row => {
    setRowPending(row.message_id, 'dismiss')
    try {
      const res = await dismissReceivedCard(row.message_id)
      if (res.success) {
        setRows(prev =>
          prev.map(r => (r.message_id === row.message_id ? res.data : r)),
        )
      } else {
        await fetchRows()
      }
    } catch {
      await fetchRows()
    } finally {
      setRowPending(row.message_id, null)
    }
  }, [fetchRows])

  return (
    <div className="card p-4 space-y-3" data-testid="received-cards-panel">
      <div className="flex items-center justify-between">
        <h3 className="text-base font-semibold">Inbound identity cards</h3>
        <button
          type="button"
          className="btn-secondary text-xs"
          onClick={fetchRows}
          disabled={loading}
          data-testid="received-cards-refresh"
        >
          {loading ? 'Loading…' : 'Refresh'}
        </button>
      </div>

      <p className="text-[11px] text-tertiary">
        Identity Cards peers have sent to this node via the
        messaging service. Accept runs the Ed25519 verifier and
        installs the card as a local Identity record; Reject
        discards it with an audit row and never writes the Identity.
      </p>

      <div
        className="text-[11px] text-tertiary"
        data-testid="received-cards-last-refreshed"
      >
        {lastRefreshedAt
          ? `last refreshed at ${formatClock(lastRefreshedAt)}`
          : 'last refreshed at —'}
      </div>

      {error && (
        <div className="text-sm text-gruvbox-red" data-testid="received-cards-error">
          {error}
        </div>
      )}

      {!loading && !error && rows.length === 0 && (
        <div className="text-sm text-secondary" data-testid="received-cards-empty">
          Inbox is empty. Cards arrive here when a contact uses
          &ldquo;Send to contact&rdquo; on their My Card panel.
        </div>
      )}

      {rows.length > 0 && (
        <ul className="divide-y divide-border" data-testid="received-cards-list">
          {rows.map(row => (
            <li
              key={row.message_id}
              className="py-2 flex items-start gap-3"
              data-testid={`received-card-row-${row.message_id}`}
            >
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="text-sm font-medium">
                    {row.display_name || '(unnamed sender)'}
                  </span>
                  <StatusBadge status={row.status} />
                </div>
                <div className="text-[11px] text-tertiary font-mono break-all mt-0.5">
                  {row.sender_public_key}
                </div>
                <div className="text-[11px] text-secondary mt-1">
                  received <span className="text-tertiary">{row.received_at}</span>
                  {row.issued_at && (
                    <>
                      {' · '}issued{' '}
                      <span className="text-tertiary">{row.issued_at}</span>
                    </>
                  )}
                </div>
                {row.error && (
                  <div
                    className="text-[11px] text-gruvbox-red mt-1"
                    data-testid={`received-card-error-${row.message_id}`}
                  >
                    {row.error}
                  </div>
                )}
                {row.accepted_identity_id && (
                  <div className="text-[11px] text-secondary mt-1">
                    linked to{' '}
                    <span className="font-mono text-tertiary">
                      {row.accepted_identity_id}
                    </span>
                  </div>
                )}
              </div>
              {row.status === 'pending' && (
                <div className="flex items-center gap-1 shrink-0">
                  <button
                    type="button"
                    className="btn-primary text-[11px] py-0.5"
                    onClick={() => handleAccept(row)}
                    disabled={pendingAction[row.message_id] != null}
                    data-testid={`received-card-accept-${row.message_id}`}
                  >
                    {pendingAction[row.message_id] === 'accept'
                      ? 'Verifying…'
                      : 'Accept'}
                  </button>
                  <button
                    type="button"
                    className="btn-secondary text-[11px] py-0.5"
                    onClick={() => handleDismiss(row)}
                    disabled={pendingAction[row.message_id] != null}
                    data-testid={`received-card-reject-${row.message_id}`}
                  >
                    {pendingAction[row.message_id] === 'dismiss'
                      ? 'Rejecting…'
                      : 'Reject'}
                  </button>
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

function formatClock(ms) {
  const d = new Date(ms)
  const pad = n => String(n).padStart(2, '0')
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

function StatusBadge({ status }) {
  const styles = {
    pending:
      'bg-gruvbox-yellow/10 text-gruvbox-yellow border-gruvbox-yellow/30',
    accepted:
      'bg-gruvbox-green/10 text-gruvbox-green border-gruvbox-green/30',
    dismissed: 'bg-surface text-tertiary border-border',
  }
  const cls = styles[status] ?? styles.dismissed
  return (
    <span
      className={`text-[10px] px-2 py-0.5 rounded-full border ${cls}`}
      data-testid={`received-card-status-${status}`}
    >
      {status}
    </span>
  )
}
