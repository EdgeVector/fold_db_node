import { useCallback, useEffect, useState } from 'react'
import {
  importIdentityCard,
  listPersonas,
} from '../../../api/clients/fingerprintsClient'

/**
 * "Import Identity Card" panel — Phase 3b of the Identity Card
 * exchange flow (Phase 3a is MyIdentityCardPanel on the issuer
 * side).
 *
 * Flow:
 *   1. Peer hands the user their Identity Card JSON over some
 *      out-of-band channel (email, AirDrop, QR copy-paste).
 *   2. User pastes the JSON here.
 *   3. User optionally picks an existing Persona to link the
 *      imported Identity to (e.g. "I've had an Alice Persona for
 *      months, now mark her as verified").
 *   4. Submit → backend verifies the Ed25519 signature → writes
 *      Identity + IdentityReceipt → optionally patches the chosen
 *      Persona's identity_id.
 *
 * The backend rejects malformed base64, wrong-length keys, and
 * signatures that don't verify, all with 400s and human-readable
 * error messages. We surface those verbatim.
 */
export default function ImportIdentityCardPanel() {
  const [pasted, setPasted] = useState('')
  const [linkPersonaId, setLinkPersonaId] = useState('')
  const [personas, setPersonas] = useState([])
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState(null)
  const [result, setResult] = useState(null)

  // Pull the persona list once so the user can pick one to link.
  // The dropdown is optional; the empty-string value means "just
  // verify and save, don't link".
  useEffect(() => {
    let cancelled = false
    listPersonas()
      .then(res => {
        if (cancelled) return
        if (res.success) setPersonas(res.data?.personas ?? [])
      })
      .catch(() => {
        // Non-fatal — the user can still submit without linking.
        if (!cancelled) setPersonas([])
      })
    return () => {
      cancelled = true
    }
  }, [])

  const handleSubmit = useCallback(
    async e => {
      e.preventDefault()
      setError(null)
      setResult(null)

      // Parse the pasted JSON locally so a typo shows a clear error
      // before it hits the backend.
      let card
      try {
        card = JSON.parse(pasted)
      } catch (err) {
        setError(`Pasted text is not valid JSON: ${err?.message ?? err}`)
        return
      }

      setSubmitting(true)
      try {
        const res = await importIdentityCard({
          card,
          link_persona_id: linkPersonaId || undefined,
        })
        if (res.success) {
          setResult(res.data)
          // Clear the textarea so an accidental re-submit doesn't
          // paste the same card twice by mistake. The result panel
          // still shows what happened.
          setPasted('')
          setLinkPersonaId('')
        } else {
          setError(res.error ?? 'Failed to import identity card')
        }
      } catch (err) {
        setError(err?.message ?? 'Network error')
      } finally {
        setSubmitting(false)
      }
    },
    [pasted, linkPersonaId],
  )

  return (
    <div className="card p-4 space-y-4" data-testid="import-identity-card-panel">
      <div>
        <h3 className="text-base font-semibold">Import identity card</h3>
        <p className="text-xs text-secondary mt-1">
          Paste a peer&apos;s Identity Card JSON below. The node will
          verify the Ed25519 signature before anything is saved. Cards
          that don&apos;t verify are rejected.
        </p>
      </div>

      <form className="space-y-3" onSubmit={handleSubmit}>
        <label className="block">
          <span className="text-xs text-tertiary">Identity Card JSON</span>
          <textarea
            className="input w-full font-mono text-xs h-48 mt-1"
            placeholder='{\n  "pub_key": "…",\n  "display_name": "Alice",\n  …\n}'
            value={pasted}
            onChange={e => setPasted(e.target.value)}
            data-testid="import-identity-card-textarea"
            spellCheck={false}
          />
        </label>

        <label className="block">
          <span className="text-xs text-tertiary">
            Link to existing persona (optional)
          </span>
          <select
            className="input w-full mt-1"
            value={linkPersonaId}
            onChange={e => setLinkPersonaId(e.target.value)}
            data-testid="import-identity-card-persona-select"
          >
            <option value="">— don&apos;t link —</option>
            {personas.map(p => (
              <option key={p.id} value={p.id}>
                {p.name || '(unnamed)'} · {p.relationship}
              </option>
            ))}
          </select>
        </label>

        <div className="flex items-center gap-3">
          <button
            type="submit"
            className="btn-primary text-xs"
            disabled={submitting || !pasted.trim()}
            data-testid="import-identity-card-submit"
          >
            {submitting ? 'Verifying…' : 'Verify and import'}
          </button>
          {error && (
            <span
              className="text-xs text-gruvbox-red"
              data-testid="import-identity-card-error"
            >
              {error}
            </span>
          )}
        </div>
      </form>

      {result && (
        <div
          className="border border-gruvbox-green/40 bg-gruvbox-green/5 rounded p-3 text-xs space-y-1"
          data-testid="import-identity-card-result"
        >
          <div>
            <span className="text-secondary">Identity id: </span>
            <span className="font-mono">{result.identity_id}</span>
          </div>
          <div>
            <span className="text-secondary">Verified: </span>
            <span>{result.verified ? 'yes' : 'no'}</span>
          </div>
          <div>
            <span className="text-secondary">Already present: </span>
            <span>{result.was_already_present ? 'yes' : 'no'}</span>
          </div>
          {result.linked_persona && (
            <div>
              <span className="text-secondary">Linked persona: </span>
              <span className="font-mono">
                {result.linked_persona.id} ({result.linked_persona.name})
              </span>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
