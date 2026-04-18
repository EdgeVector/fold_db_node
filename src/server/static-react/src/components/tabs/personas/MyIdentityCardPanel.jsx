import { useCallback, useEffect, useRef, useState } from 'react'
import { QRCodeSVG } from 'qrcode.react'
import {
  detectFaces,
  getMyIdentityCard,
  reissueMyIdentityCard,
  sendIdentityCard,
} from '../../../api/clients/fingerprintsClient'
import { listContacts } from '../../../api/clients/trustClient'

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
  // the form; draftName mirrors the current card so a Cancel
  // restores the stored value without a refetch. Birthday is NOT
  // surfaced in the edit form on purpose — a real person's birthday
  // doesn't change, and exposing it as a normal editable field
  // implies otherwise. The backend still accepts birthday patches
  // (for a future first-time-set wizard), just not from this panel.
  const [editing, setEditing] = useState(false)
  const [draftName, setDraftName] = useState('')
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState(null)

  // QR code is off by default — takes up 200+px of vertical space
  // and most of the time the user just wants to copy JSON.
  const [showQr, setShowQr] = useState(false)

  // Send-to-contact state. Contacts are fetched lazily on first
  // open of the picker so we don't pay the trust-client cost for
  // users who never send their card through messaging.
  const [sendPickerOpen, setSendPickerOpen] = useState(false)
  const [contacts, setContacts] = useState([])
  const [contactsLoading, setContactsLoading] = useState(false)
  const [contactsError, setContactsError] = useState(null)
  const [sendingTo, setSendingTo] = useState(null) // contact pub_key currently in-flight
  const [sendResult, setSendResult] = useState(null) // { display_name } on success
  const [sendError, setSendError] = useState(null)

  // Attach-face flow state. `stage` walks through
  // idle → camera (getUserMedia running) → detecting (POST to
  // /detect-faces) → saving (POST to /reissue). Errors surface
  // inline; the modal stays open until the user cancels or succeeds.
  const [attachOpen, setAttachOpen] = useState(false)
  const [attachStage, setAttachStage] = useState('idle')
  const [attachError, setAttachError] = useState(null)
  const [removingFace, setRemovingFace] = useState(false)
  const videoRef = useRef(null)
  const streamRef = useRef(null)

  const stopCamera = useCallback(() => {
    const stream = streamRef.current
    if (stream) {
      for (const track of stream.getTracks()) {
        track.stop()
      }
    }
    streamRef.current = null
    if (videoRef.current) {
      videoRef.current.srcObject = null
    }
  }, [])

  useEffect(() => stopCamera, [stopCamera])

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
    setSaveError(null)
    setEditing(true)
  }, [card])

  const cancelEdit = useCallback(() => {
    setEditing(false)
    setSaveError(null)
  }, [])

  const handleSave = useCallback(async () => {
    if (!card) return
    const trimmedName = draftName.trim()
    if (!trimmedName || trimmedName === card.display_name) {
      // No-op — close the form without a round trip.
      setEditing(false)
      return
    }
    setSaving(true)
    setSaveError(null)
    try {
      const res = await reissueMyIdentityCard({ display_name: trimmedName })
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
  }, [card, draftName])

  const openSendPicker = useCallback(async () => {
    setSendPickerOpen(true)
    setSendError(null)
    setSendResult(null)
    if (contacts.length > 0 || contactsLoading) return
    setContactsLoading(true)
    setContactsError(null)
    try {
      const res = await listContacts()
      if (res.success) {
        setContacts(res.data?.contacts ?? [])
      } else {
        setContactsError(res.error ?? 'Failed to load contacts')
      }
    } catch (e) {
      setContactsError(e?.message ?? 'Network error while loading contacts')
    } finally {
      setContactsLoading(false)
    }
  }, [contacts.length, contactsLoading])

  const handleSendTo = useCallback(async contact => {
    if (!contact?.public_key) return
    setSendingTo(contact.public_key)
    setSendError(null)
    setSendResult(null)
    try {
      const res = await sendIdentityCard(contact.public_key)
      if (res.success) {
        // Close the picker on success so the confirmation message
        // is visually decoupled from the list of contacts. Keep the
        // recipient's display name around for the success toast.
        setSendResult({
          display_name:
            res.data?.recipient_display_name ?? contact.display_name,
        })
        setSendPickerOpen(false)
      } else {
        setSendError(res.error ?? 'Failed to send identity card')
      }
    } catch (e) {
      setSendError(e?.message ?? 'Network error while sending')
    } finally {
      setSendingTo(null)
    }
  }, [])

  const closeAttach = useCallback(() => {
    stopCamera()
    setAttachOpen(false)
    setAttachStage('idle')
    setAttachError(null)
  }, [stopCamera])

  const openAttach = useCallback(async () => {
    setAttachOpen(true)
    setAttachError(null)
    setAttachStage('camera')
    if (
      !navigator.mediaDevices ||
      typeof navigator.mediaDevices.getUserMedia !== 'function'
    ) {
      setAttachError(
        'Camera access is not available in this browser. Attach face from a device with a camera and a modern browser.',
      )
      setAttachStage('idle')
      return
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ video: true })
      streamRef.current = stream
      if (videoRef.current) {
        videoRef.current.srcObject = stream
        // Autoplay attr should be enough, but call play() explicitly
        // for browsers that require a user-gesture-tied trigger.
        videoRef.current.play?.().catch(() => {})
      }
    } catch (e) {
      setAttachError(
        e?.message ??
          'Could not open the camera. Grant camera permission and try again.',
      )
      setAttachStage('idle')
    }
  }, [])

  const snapAndDetect = useCallback(async () => {
    const video = videoRef.current
    if (!video || !streamRef.current) {
      setAttachError('Camera is not ready yet.')
      return
    }
    setAttachError(null)
    setAttachStage('detecting')
    try {
      // Draw the current video frame to a canvas and read the PNG
      // base64 payload. toDataURL returns `data:image/png;base64,<payload>`
      // — strip the prefix so the backend gets raw base64.
      const canvas = document.createElement('canvas')
      canvas.width = video.videoWidth || 640
      canvas.height = video.videoHeight || 480
      const ctx = canvas.getContext('2d')
      ctx.drawImage(video, 0, 0, canvas.width, canvas.height)
      const dataUrl = canvas.toDataURL('image/png')
      const commaIdx = dataUrl.indexOf(',')
      const imageBase64 =
        commaIdx >= 0 ? dataUrl.slice(commaIdx + 1) : dataUrl

      const detectRes = await detectFaces(imageBase64)
      if (!detectRes.success) {
        setAttachError(detectRes.error ?? 'Face detection failed.')
        setAttachStage('camera')
        return
      }
      const faces = detectRes.data?.faces ?? []
      if (faces.length === 0) {
        setAttachError('No face detected. Try better lighting.')
        setAttachStage('camera')
        return
      }
      if (faces.length > 1) {
        setAttachError(
          'Multiple faces detected. Make sure only you are in frame.',
        )
        setAttachStage('camera')
        return
      }

      setAttachStage('saving')
      const embedding = faces[0].embedding
      const saveRes = await reissueMyIdentityCard({ face_embedding: embedding })
      if (!saveRes.success) {
        setAttachError(saveRes.error ?? 'Failed to reissue identity card.')
        setAttachStage('camera')
        return
      }
      setCard(saveRes.data ?? null)
      closeAttach()
    } catch (e) {
      setAttachError(e?.message ?? 'Network error while attaching face.')
      setAttachStage('camera')
    }
  }, [closeAttach])

  const handleRemoveFace = useCallback(async () => {
    if (!card) return
    setRemovingFace(true)
    setAttachError(null)
    try {
      const res = await reissueMyIdentityCard({ face_embedding: null })
      if (res.success) {
        setCard(res.data ?? null)
      } else {
        setAttachError(res.error ?? 'Failed to remove face.')
      }
    } catch (e) {
      setAttachError(e?.message ?? 'Network error while removing face.')
    } finally {
      setRemovingFace(false)
    }
  }, [card])

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
            <dt className="text-tertiary">Face</dt>
            <dd data-testid="my-identity-card-face-status">
              {card.face_embedding ? (
                <span className="text-gruvbox-green">
                  attached ({card.face_embedding.length}-d)
                </span>
              ) : (
                <span className="text-tertiary italic">not attached</span>
              )}
            </dd>
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
            <button
              type="button"
              className="btn-secondary text-xs"
              onClick={() => setShowQr(v => !v)}
              data-testid="my-identity-card-qr-toggle"
              aria-pressed={showQr}
            >
              {showQr ? 'Hide QR' : 'Show QR'}
            </button>
            <button
              type="button"
              className="btn-secondary text-xs"
              onClick={openSendPicker}
              data-testid="my-identity-card-send"
              disabled={sendPickerOpen}
            >
              Send to contact
            </button>
            {!card.face_embedding ? (
              <button
                type="button"
                className="btn-secondary text-xs"
                onClick={openAttach}
                disabled={attachOpen}
                data-testid="my-identity-card-attach-face"
              >
                Attach face
              </button>
            ) : (
              <button
                type="button"
                className="btn-secondary text-xs"
                onClick={handleRemoveFace}
                disabled={removingFace}
                data-testid="my-identity-card-remove-face"
              >
                {removingFace ? 'Removing…' : 'Remove face'}
              </button>
            )}
            <span className="text-[11px] text-tertiary">
              Editing reissues the card — a new Ed25519 signature is
              computed over the updated payload. Peers will see the
              stale card until you re-send.
            </span>
          </div>

          <p
            className="text-[11px] text-tertiary"
            data-testid="my-identity-card-attach-help"
          >
            Attaches your face to your signed Identity Card. To also
            add this photo to your photo library, upload it through
            Photos separately.
          </p>

          {attachOpen && (
            <div
              className="rounded border border-border p-3 space-y-2"
              data-testid="my-identity-card-attach-modal"
            >
              <div className="flex items-center justify-between">
                <span className="text-xs font-semibold">
                  Take a selfie
                </span>
                <button
                  type="button"
                  className="text-[11px] text-tertiary underline"
                  onClick={closeAttach}
                  data-testid="my-identity-card-attach-close"
                >
                  Cancel
                </button>
              </div>
              <video
                ref={videoRef}
                autoPlay
                muted
                playsInline
                className="w-full max-w-sm rounded bg-black"
                data-testid="my-identity-card-attach-video"
              />
              {attachError && (
                <div
                  className="text-xs text-gruvbox-red"
                  data-testid="my-identity-card-attach-error"
                >
                  {attachError}
                </div>
              )}
              <button
                type="button"
                className="btn-primary text-xs"
                onClick={snapAndDetect}
                disabled={
                  attachStage === 'detecting' || attachStage === 'saving'
                }
                data-testid="my-identity-card-attach-snap"
              >
                {attachStage === 'detecting'
                  ? 'Detecting…'
                  : attachStage === 'saving'
                  ? 'Signing…'
                  : 'Snap and attach'}
              </button>
            </div>
          )}

          {sendResult && (
            <div
              className="text-[11px] text-gruvbox-green"
              data-testid="my-identity-card-send-result"
            >
              Sent to {sendResult.display_name}.
            </div>
          )}

          {sendPickerOpen && (
            <div
              className="rounded border border-border p-3 space-y-2"
              data-testid="my-identity-card-send-picker"
            >
              <div className="flex items-center justify-between">
                <span className="text-xs font-semibold">
                  Send to which contact?
                </span>
                <button
                  type="button"
                  className="text-[11px] text-tertiary underline"
                  onClick={() => setSendPickerOpen(false)}
                  data-testid="my-identity-card-send-close"
                >
                  Close
                </button>
              </div>
              {contactsLoading && (
                <div className="text-xs text-secondary">Loading contacts…</div>
              )}
              {contactsError && (
                <div
                  className="text-xs text-gruvbox-red"
                  data-testid="my-identity-card-send-contacts-error"
                >
                  {contactsError}
                </div>
              )}
              {!contactsLoading && !contactsError && contacts.length === 0 && (
                <div
                  className="text-xs text-secondary"
                  data-testid="my-identity-card-send-empty"
                >
                  No contacts yet. Connect via discovery first before
                  sending your card over messaging.
                </div>
              )}
              {contacts.length > 0 && (
                <ul className="space-y-1">
                  {contacts
                    .filter(c => !c.revoked)
                    .map(contact => (
                      <li
                        key={contact.public_key}
                        className="flex items-center justify-between gap-2"
                      >
                        <div className="min-w-0 flex-1">
                          <div className="text-xs font-medium truncate">
                            {contact.display_name || '(unnamed)'}
                          </div>
                          <div className="text-[10px] text-tertiary font-mono truncate">
                            {contact.public_key}
                          </div>
                        </div>
                        <button
                          type="button"
                          className="btn-primary text-[11px] py-0.5 shrink-0"
                          onClick={() => handleSendTo(contact)}
                          disabled={sendingTo === contact.public_key}
                          data-testid={`my-identity-card-send-to-${contact.public_key}`}
                        >
                          {sendingTo === contact.public_key
                            ? 'Sending…'
                            : 'Send'}
                        </button>
                      </li>
                    ))}
                </ul>
              )}
              {sendError && (
                <div
                  className="text-xs text-gruvbox-red"
                  data-testid="my-identity-card-send-error"
                >
                  {sendError}
                </div>
              )}
            </div>
          )}

          {showQr && (
            <div
              className="flex flex-col items-center gap-2 bg-white rounded p-4 w-fit"
              data-testid="my-identity-card-qr"
            >
              {/*
                QR payload is COMPACT JSON (no indentation). QR
                capacity scales roughly with byte count, and the
                pretty-printed form would blow past the practical
                scanning limit for phone cameras at common sizes.
                Level-M error correction is the QR default and
                tolerates some smudging; level H would give better
                tolerance but bump the QR density, which hurts
                low-light phone scans.
              */}
              <QRCodeSVG
                value={JSON.stringify(card)}
                size={256}
                level="M"
                marginSize={2}
                data-testid="my-identity-card-qr-svg"
              />
              <p className="text-[10px] text-gray-600 max-w-[256px] text-center">
                Scan with your phone's QR reader or another node's
                Import Card flow. The payload is the signed card
                JSON — verifiable without a network call.
              </p>
            </div>
          )}

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
