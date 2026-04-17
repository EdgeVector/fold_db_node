import { useCallback, useEffect, useState } from 'react'
import { getMyIdentityCard } from '../../../api/clients/fingerprintsClient'

/**
 * "My Identity Card" panel — shows the node owner's signed Identity
 * Card and lets them copy the canonical payload for sharing.
 *
 * This is Phase 3a of the design doc's Identity Card exchange flow.
 * The full flow (QR rendering, scan-to-receive, verify signature,
 * link to Persona) is deferred; this view just surfaces what's
 * already signed and on disk so the user can see and copy it.
 *
 * Trust boundary: the payload contains only public card material
 * (pubkey, display_name, birthday, signature, issued_at). No
 * private keys. The signature is Ed25519 over the other fields
 * and is verifiable standalone.
 */
export default function MyIdentityCardPanel() {
  const [card, setCard] = useState(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)
  const [copied, setCopied] = useState(false)

  const fetchCard = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const res = await getMyIdentityCard()
      if (res.success) {
        setCard(res.data ?? null)
      } else {
        // 404 = no card yet (user hasn't completed setup). Surface
        // the backend message so the user knows what to do next.
        setError(res.error ?? 'Failed to load identity card')
      }
    } catch (e) {
      setError(e?.message ?? 'Network error')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    fetchCard()
  }, [fetchCard])

  const handleCopy = useCallback(async () => {
    if (!card) return
    const payload = JSON.stringify(card, null, 2)
    try {
      await navigator.clipboard.writeText(payload)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // Silent — some browsers block clipboard in insecure contexts.
      // The payload is visible in the pre block below; the user can
      // still select-and-copy manually.
    }
  }, [card])

  return (
    <div className="card p-3 space-y-3" data-testid="my-identity-card-panel">
      <div className="flex items-center justify-between">
        <h3 className="text-base font-semibold">My Identity Card</h3>
        <button
          type="button"
          className="btn-secondary text-xs"
          onClick={fetchCard}
          disabled={loading}
          data-testid="my-identity-card-refresh"
        >
          {loading ? 'Loading…' : 'Refresh'}
        </button>
      </div>

      <p className="text-[11px] text-tertiary">
        Your signed Identity Card. Share this payload with trusted
        peers (in person, via QR, or over an end-to-end encrypted
        message) so they can add you as a verified Identity on their
        node. The signature is valid as long as the payload is
        unchanged — if you edit your display name or birthday later,
        issue a new card rather than re-signing.
      </p>

      {error && (
        <div
          className="text-sm text-gruvbox-red"
          data-testid="my-identity-card-error"
        >
          {error}
        </div>
      )}

      {!loading && !error && card && (
        <>
          <dl
            className="text-xs grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1"
            data-testid="my-identity-card-fields"
          >
            <dt className="text-tertiary">Display name</dt>
            <dd className="font-medium">{card.display_name}</dd>
            <dt className="text-tertiary">Birthday</dt>
            <dd>{card.birthday || <span className="text-tertiary italic">not set</span>}</dd>
            <dt className="text-tertiary">Public key</dt>
            <dd className="font-mono break-all text-[11px]" title={card.pub_key}>
              {card.pub_key}
            </dd>
            <dt className="text-tertiary">Issued at</dt>
            <dd className="font-mono text-[11px]">{card.issued_at}</dd>
            <dt className="text-tertiary">Signature</dt>
            <dd className="font-mono break-all text-[11px]" title={card.card_signature}>
              {card.card_signature.slice(0, 32)}…
            </dd>
          </dl>

          <div className="flex items-center gap-2">
            <button
              type="button"
              className="btn-primary text-xs"
              onClick={handleCopy}
              data-testid="my-identity-card-copy"
            >
              {copied ? 'Copied!' : 'Copy card JSON'}
            </button>
            <span className="text-[11px] text-tertiary">
              Paste into a trusted peer's Receive Card flow (Phase 3,
              not yet shipped).
            </span>
          </div>

          <pre
            className="text-[11px] text-tertiary font-mono whitespace-pre-wrap break-words bg-surface rounded border border-border p-2 max-h-64 overflow-y-auto"
            data-testid="my-identity-card-json"
          >
            {JSON.stringify(card, null, 2)}
          </pre>
        </>
      )}
    </div>
  )
}
