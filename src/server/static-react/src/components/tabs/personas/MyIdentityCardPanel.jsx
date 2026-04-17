import { useCallback, useEffect, useState } from 'react'
import {
  getMyIdentityCard,
  reissueMyIdentityCard,
} from '../../../api/clients/fingerprintsClient'

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

  // Inline edit form state. `editing` toggles the display <dl> into
  // the form; draftName/draftBirthday mirror the current card so a
  // Cancel restores the stored values without a refetch.
  const [editing, setEditing] = useState(false)
  const [draftName, setDraftName] = useState('')
  const [draftBirthday, setDraftBirthday] = useState('')
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState(null)

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

  const startEdit = useCallback(() => {
    if (!card) return
    setDraftName(card.display_name || '')
    setDraftBirthday(card.birthday || '')
    setSaveError(null)
    setEditing(true)
  }, [card])

  const cancelEdit = useCallback(() => {
    setEditing(false)
    setSaveError(null)
  }, [])

  const handleSave = useCallback(async () => {
    if (!card) return
    const req = {}
    const trimmedName = draftName.trim()
    if (trimmedName && trimmedName !== card.display_name) {
      req.display_name = trimmedName
    }
    // Birthday three-state: empty string vs. original.
    // - empty and was set → null (clear)
    // - empty and was unset → skip (nothing to do)
    // - non-empty and differs → set
    const trimmedBirthday = draftBirthday.trim()
    if (trimmedBirthday === '' && card.birthday) {
      req.birthday = null
    } else if (trimmedBirthday !== '' && trimmedBirthday !== card.birthday) {
      req.birthday = trimmedBirthday
    }
    if (Object.keys(req).length === 0) {
      setEditing(false)
      return
    }
    setSaving(true)
    setSaveError(null)
    try {
      const res = await reissueMyIdentityCard(req)
      if (res.success) {
        setCard(res.data ?? null)
        setEditing(false)
      } else {
        setSaveError(res.error ?? 'Failed to reissue identity card')
      }
    } catch (e) {
      setSaveError(e?.message ?? 'Network error')
    } finally {
      setSaving(false)
    }
  }, [card, draftName, draftBirthday])

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

      {!loading && !error && card && editing && (
        <form
          className="space-y-3"
          onSubmit={e => {
            e.preventDefault()
            handleSave()
          }}
          data-testid="my-identity-card-edit-form"
        >
          <label className="block">
            <span className="text-xs text-tertiary">Display name</span>
            <input
              type="text"
              className="input w-full mt-1 text-sm"
              value={draftName}
              onChange={e => setDraftName(e.target.value)}
              data-testid="my-identity-card-draft-name"
              autoFocus
            />
          </label>
          <label className="block">
            <span className="text-xs text-tertiary">
              Birthday (YYYY-MM-DD, blank to clear)
            </span>
            <input
              type="text"
              className="input w-full mt-1 text-sm font-mono"
              value={draftBirthday}
              onChange={e => setDraftBirthday(e.target.value)}
              data-testid="my-identity-card-draft-birthday"
              placeholder="1990-04-17"
            />
          </label>
          {saveError && (
            <div
              className="text-xs text-gruvbox-red"
              data-testid="my-identity-card-save-error"
            >
              {saveError}
            </div>
          )}
          <div className="flex items-center gap-2">
            <button
              type="submit"
              className="btn-primary text-xs"
              disabled={saving}
              data-testid="my-identity-card-save"
            >
              {saving ? 'Signing…' : 'Save and reissue'}
            </button>
            <button
              type="button"
              className="btn-secondary text-xs"
              onClick={cancelEdit}
              disabled={saving}
              data-testid="my-identity-card-cancel"
            >
              Cancel
            </button>
          </div>
        </form>
      )}

      {!loading && !error && card && !editing && (
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
            <button
              type="button"
              className="btn-secondary text-xs"
              onClick={startEdit}
              data-testid="my-identity-card-edit"
            >
              Edit card
            </button>
            <span className="text-[11px] text-tertiary">
              Editing reissues the card — a new Ed25519 signature is
              computed over the updated payload. Peers will see the
              stale card until you re-send.
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
