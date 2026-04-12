import { QRCodeSVG } from 'qrcode.react'
import { declineTrustInvite } from '../../../api/clients/trustClient'

export default function InvitePanel({
  // Invite creation
  inviteRole,
  setInviteRole,
  availableRoles,
  creatingInvite,
  identityCard,
  handleCreateInvite,
  inviteToken,
  handleCopyToken,
  copied,
  handleShareViaLink,
  sharing,
  showQr,
  setShowQr,
  sharedInviteId,
  // Email
  recipientEmail,
  setRecipientEmail,
  handleSendViaEmail,
  sendingEmail,
  emailSentId,
  // Reciprocal
  reciprocalToken,
  setReciprocalToken,
  // Accept
  verifyId,
  setVerifyId,
  verifyCode,
  setVerifyCode,
  handleVerifyCode,
  verifying,
  fetchId,
  setFetchId,
  handleFetchById,
  fetching,
  acceptToken,
  setAcceptToken,
  preview,
  setPreview,
  handlePreviewInvite,
  previewing,
  acceptRole,
  setAcceptRole,
  trustBack,
  setTrustBack,
  handleAcceptInvite,
  accepting,
  setError,
  onResult,
}) {
  return (
    <>
      {/* Create invite */}
      <div className="border border-border rounded-lg p-4 mb-6 bg-surface">
        <h3 className="text-sm font-medium text-primary mb-1">Share a Trust Invite</h3>
        <p className="text-xs text-secondary mb-3">
          Generate a trust invite token to share with someone. They can paste it into their FoldDB to connect with you.
        </p>
        <form onSubmit={handleCreateInvite} className="flex gap-3 items-end mb-3">
          <div className="w-40">
            <label className="block text-xs text-secondary mb-1">Proposed Role</label>
            <select
              className="input w-full"
              value={inviteRole}
              onChange={(e) => setInviteRole(e.target.value)}
            >
              {Object.values(availableRoles).length > 0
                ? Object.values(availableRoles).map((role) => (
                    <option key={role.name} value={role.name}>{role.name.replace(/_/g, ' ')}</option>
                  ))
                : ['friend', 'family', 'doctor', 'trainer', 'accountant', 'collaborator'].map(r => (
                    <option key={r} value={r}>{r}</option>
                  ))
              }
            </select>
          </div>
          <button
            type="submit"
            className="btn"
            disabled={creatingInvite || !identityCard}
          >
            {creatingInvite ? 'Creating...' : 'Generate Invite'}
          </button>
        </form>

        {inviteToken && (
          <div className="mt-3">
            <label className="block text-xs text-secondary mb-1">Your Trust Invite Token</label>
            <div className="flex gap-2">
              <input
                className="input flex-1 font-mono text-xs"
                type="text"
                value={inviteToken}
                readOnly
                onClick={(e) => e.target.select()}
              />
              <button className="btn btn-sm" onClick={handleCopyToken}>
                {copied ? 'Copied!' : 'Copy'}
              </button>
            </div>
            <p className="text-xs text-tertiary mt-1">
              Share this token directly, use "Share via Link" to upload to Exemem, or show a QR code for in-person sharing.
            </p>
            <div className="flex gap-2 mt-2">
              <button
                className="btn btn-sm"
                onClick={handleShareViaLink}
                disabled={sharing}
              >
                {sharing ? 'Uploading...' : 'Share via Link'}
              </button>
              <button
                className="btn btn-sm"
                onClick={() => setShowQr(!showQr)}
              >
                {showQr ? 'Hide QR' : 'Show QR Code'}
              </button>
            </div>
            {showQr && (
              <div className="mt-3 p-4 bg-white rounded-lg inline-block">
                <QRCodeSVG value={inviteToken} size={200} level="M" />
                <p className="text-xs text-gray-500 mt-2 text-center max-w-[200px]">
                  Only show this in person — anyone who scans it can claim the invite.
                </p>
              </div>
            )}
            {sharedInviteId && (
              <div className="mt-2 p-2 bg-surface-secondary border border-gruvbox-green/30 rounded">
                <p className="text-xs text-gruvbox-green font-medium mb-1">Shared via Exemem</p>
                <div className="flex gap-2 items-center">
                  <span className="text-xs text-secondary">Invite ID:</span>
                  <code className="text-xs text-primary font-mono">{sharedInviteId}</code>
                  <button
                    className="btn btn-sm text-xs"
                    onClick={() => {
                      navigator.clipboard.writeText(sharedInviteId)
                    }}
                  >
                    Copy ID
                  </button>
                </div>
                <p className="text-xs text-tertiary mt-1">
                  Share this short ID instead of the full token. One-time use, expires in 7 days.
                </p>
              </div>
            )}
          {/* Send via email */}
          <div className="mt-3 pt-3 border-t border-border">
            <h4 className="text-xs font-medium text-secondary mb-2">Send via Email</h4>
            <p className="text-xs text-tertiary mb-2">
              Enter the recipient's email. They'll receive a verification code to claim the invite.
            </p>
            <div className="flex gap-2">
              <input
                className="input flex-1 text-xs"
                type="email"
                placeholder="recipient@example.com"
                value={recipientEmail}
                onChange={(e) => setRecipientEmail(e.target.value)}
              />
              <button
                className="btn btn-sm"
                onClick={handleSendViaEmail}
                disabled={sendingEmail || !recipientEmail.trim()}
              >
                {sendingEmail ? 'Sending...' : 'Send'}
              </button>
            </div>
            {emailSentId && (
              <div className="mt-2 p-2 bg-surface-secondary border border-gruvbox-green/30 rounded">
                <p className="text-xs text-gruvbox-green">
                  Verification code sent to {recipientEmail}. They need to enter the code in their FoldDB.
                </p>
              </div>
            )}
          </div>
          </div>
        )}
      </div>

      {/* Reciprocal token (after accepting with trust-back) */}
      {reciprocalToken && (
        <div className="border border-gruvbox-green/30 rounded-lg p-4 mb-6 bg-surface">
          <h3 className="text-sm font-medium text-gruvbox-green mb-1">Send this back to your contact</h3>
          <p className="text-xs text-secondary mb-2">
            You accepted with trust-back. Share this token with the sender so they can add you to their contacts too.
          </p>
          <div className="flex gap-2">
            <input
              className="input flex-1 font-mono text-xs"
              type="text"
              value={reciprocalToken}
              readOnly
              onClick={(e) => e.target.select()}
            />
            <button className="btn btn-sm" onClick={() => {
              navigator.clipboard.writeText(reciprocalToken)
            }}>
              Copy
            </button>
          </div>
          <button
            className="text-xs text-tertiary mt-2 underline cursor-pointer bg-transparent border-none"
            onClick={() => setReciprocalToken(null)}
          >
            Dismiss
          </button>
        </div>
      )}

      {/* Accept invite */}
      <div className="border border-border rounded-lg p-4 bg-surface">
        <h3 className="text-sm font-medium text-primary mb-1">Accept a Trust Invite</h3>
        <p className="text-xs text-secondary mb-3">
          Received a verification code via email? Enter the invite ID and code below.
          Or enter a short invite ID, or paste a full token.
        </p>

        {/* Verify with code (email flow) */}
        <div className="mb-4 p-3 border border-border rounded bg-surface-secondary">
          <h4 className="text-xs font-medium text-secondary mb-2">Verify with Email Code</h4>
          <div className="flex gap-2 mb-2">
            <input
              className="input flex-1 font-mono text-xs"
              type="text"
              placeholder="Invite ID"
              value={verifyId}
              onChange={(e) => setVerifyId(e.target.value)}
            />
            <input
              className="input w-28 font-mono text-xs text-center"
              type="text"
              placeholder="6-digit code"
              maxLength={6}
              value={verifyCode}
              onChange={(e) => setVerifyCode(e.target.value.replace(/\D/g, ''))}
            />
            <button
              className="btn btn-sm"
              onClick={handleVerifyCode}
              disabled={verifying || !verifyId.trim() || verifyCode.length !== 6}
            >
              {verifying ? 'Verifying...' : 'Verify'}
            </button>
          </div>
        </div>

        {/* Fetch by invite ID (no code) */}
        <div className="flex gap-2 mb-3">
          <input
            className="input flex-1 font-mono text-xs"
            type="text"
            placeholder="Invite ID (e.g. abc123def456)"
            value={fetchId}
            onChange={(e) => setFetchId(e.target.value)}
          />
          <button
            className="btn btn-sm"
            onClick={handleFetchById}
            disabled={fetching || !fetchId.trim()}
          >
            {fetching ? 'Fetching...' : 'Fetch'}
          </button>
        </div>

        <div className="text-xs text-tertiary text-center mb-3">— or paste a full token —</div>
        <div className="flex gap-2 mb-3">
          <input
            className="input flex-1 font-mono text-xs"
            type="text"
            placeholder="Paste trust invite token..."
            value={acceptToken}
            onChange={(e) => { setAcceptToken(e.target.value); setPreview(null) }}
          />
          <button
            className="btn btn-sm"
            onClick={handlePreviewInvite}
            disabled={previewing || !acceptToken.trim()}
          >
            {previewing ? 'Loading...' : 'Preview'}
          </button>
        </div>

        {preview && (
          <div className="border border-border rounded-lg p-4 bg-surface-secondary">
            <div className="flex items-center gap-2 mb-3">
              {preview.valid ? (
                <span className="badge badge-success text-xs">verified</span>
              ) : (
                <span className="badge badge-error text-xs">invalid signature</span>
              )}
              <span className="text-xs text-tertiary">
                Fingerprint: <code>{preview.sender.fingerprint}</code>
              </span>
            </div>

            <p className="text-sm text-primary font-medium mb-1">
              {preview.sender.display_name}
            </p>
            {preview.sender.contact_hint && (
              <p className="text-xs text-secondary mb-2">{preview.sender.contact_hint}</p>
            )}
            <p className="text-xs text-tertiary mb-3">
              Wants to trust you as role: {preview.proposed_role}
            </p>

            {preview.valid && (
              <div className="flex items-end gap-3">
                <div className="w-40">
                  <label className="block text-xs text-secondary mb-1">Your Role (override)</label>
                  <select
                    className="input w-full"
                    value={acceptRole}
                    onChange={(e) => setAcceptRole(e.target.value)}
                  >
                    <option value="">Use proposed role</option>
                    {Object.values(availableRoles).length > 0
                      ? Object.values(availableRoles).map((role) => (
                          <option key={role.name} value={role.name}>{role.name.replace(/_/g, ' ')}</option>
                        ))
                      : ['friend', 'family', 'doctor', 'trainer', 'accountant', 'collaborator'].map(r => (
                          <option key={r} value={r}>{r}</option>
                        ))
                    }
                  </select>
                </div>
                <label className="flex items-center gap-2 text-sm text-secondary cursor-pointer">
                  <input
                    type="checkbox"
                    checked={trustBack}
                    onChange={(e) => setTrustBack(e.target.checked)}
                    className="rounded"
                  />
                  Trust back
                </label>
                <button
                  className="btn"
                  onClick={handleAcceptInvite}
                  disabled={accepting}
                >
                  {accepting ? 'Accepting...' : (trustBack ? 'Accept & Trust Back' : 'Accept Only')}
                </button>
                <button
                  className="btn btn-sm text-gruvbox-red border-gruvbox-red/30 hover:bg-gruvbox-red/10"
                  onClick={async () => {
                    try {
                      const resp = await declineTrustInvite(acceptToken)
                      if (resp.success) {
                        setAcceptToken('')
                        setPreview(null)
                        if (onResult) onResult({ success: true, data: { message: `Declined invite from ${resp.data?.sender || 'unknown'}` } })
                      }
                    } catch (err) {
                      setError(err?.message || 'Failed to decline invite')
                    }
                  }}
                >
                  Decline
                </button>
              </div>
            )}
          </div>
        )}
      </div>
    </>
  )
}
