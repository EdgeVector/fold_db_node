import { useState, useEffect, useCallback } from 'react'
import {
  getIdentityCard,
  setIdentityCard,
  listContacts,
  revokeContact,
  createTrustInvite,
  previewTrustInvite,
  acceptTrustInvite,
  shareTrustInvite,
  fetchSharedInvite,
  getAuditLog,
} from '../../api/clients/trustClient'

function TrustTab({ onResult }) {
  const [activeSection, setActiveSection] = useState('contacts')
  const [error, setError] = useState(null)

  // Identity card
  const [identityCard, setIdentityCardState] = useState(null)
  const [identityLoading, setIdentityLoading] = useState(true)
  const [editName, setEditName] = useState('')
  const [editHint, setEditHint] = useState('')
  const [savingIdentity, setSavingIdentity] = useState(false)

  // Contacts
  const [contacts, setContacts] = useState([])
  const [contactsLoading, setContactsLoading] = useState(true)
  const [revoking, setRevoking] = useState(null)

  // Invite creation
  const [inviteDistance, setInviteDistance] = useState('1')
  const [creatingInvite, setCreatingInvite] = useState(false)
  const [inviteToken, setInviteToken] = useState(null)
  const [copied, setCopied] = useState(false)
  const [sharing, setSharing] = useState(false)
  const [sharedInviteId, setSharedInviteId] = useState(null)
  const [fetchId, setFetchId] = useState('')
  const [fetching, setFetching] = useState(false)

  // Invite acceptance
  const [acceptToken, setAcceptToken] = useState('')
  const [preview, setPreview] = useState(null)
  const [previewing, setPreviewing] = useState(false)
  const [accepting, setAccepting] = useState(false)
  const [acceptDistance, setAcceptDistance] = useState('')
  const [trustBack, setTrustBack] = useState(true)

  // Audit log
  const [auditEvents, setAuditEvents] = useState([])

  // ===== Fetch data =====

  const fetchIdentity = useCallback(async () => {
    setIdentityLoading(true)
    try {
      const response = await getIdentityCard()
      if (response.success && response.data) {
        const card = response.data.identity_card
        setIdentityCardState(card)
        if (card) {
          setEditName(card.display_name)
          setEditHint(card.contact_hint || '')
        }
      }
    } catch { /* ignore */ } finally {
      setIdentityLoading(false)
    }
  }, [])

  const fetchContacts = useCallback(async () => {
    setContactsLoading(true)
    try {
      const response = await listContacts()
      if (response.success && response.data) {
        setContacts(response.data.contacts || [])
      }
    } catch { /* ignore */ } finally {
      setContactsLoading(false)
    }
  }, [])

  const fetchAuditLog = useCallback(async () => {
    try {
      const response = await getAuditLog(50)
      if (response.success && response.data) {
        setAuditEvents(response.data.events || [])
      }
    } catch { /* ignore */ }
  }, [])

  useEffect(() => {
    fetchIdentity()
    fetchContacts()
    fetchAuditLog()
  }, [fetchIdentity, fetchContacts, fetchAuditLog])

  // ===== Identity card handlers =====

  const handleSaveIdentity = async (e) => {
    e.preventDefault()
    if (!editName.trim()) return
    setSavingIdentity(true)
    setError(null)
    try {
      const response = await setIdentityCard(editName.trim(), editHint.trim() || null)
      if (response.success) {
        setIdentityCardState({ display_name: editName.trim(), contact_hint: editHint.trim() || null })
        if (onResult) onResult({ success: true, data: { message: 'Identity card saved' } })
      } else {
        setError(response.error || 'Failed to save identity card')
      }
    } catch (err) {
      setError(err.message || 'Failed to save identity card')
    } finally {
      setSavingIdentity(false)
    }
  }

  // ===== Contact handlers =====

  const handleRevoke = async (publicKey) => {
    setRevoking(publicKey)
    setError(null)
    try {
      const response = await revokeContact(publicKey)
      if (response.success) {
        await fetchContacts()
        await fetchAuditLog()
        if (onResult) onResult({ success: true, data: { message: 'Contact revoked' } })
      } else {
        setError(response.error || 'Failed to revoke contact')
      }
    } catch (err) {
      setError(err.message || 'Failed to revoke contact')
    } finally {
      setRevoking(null)
    }
  }

  // ===== Invite handlers =====

  const handleCreateInvite = async (e) => {
    e.preventDefault()
    const dist = parseInt(inviteDistance, 10)
    if (isNaN(dist) || dist < 1) {
      setError('Distance must be a positive integer')
      return
    }
    if (!identityCard) {
      setError('Please set your identity card first (go to Identity tab)')
      return
    }
    setCreatingInvite(true)
    setError(null)
    setInviteToken(null)
    try {
      const response = await createTrustInvite(dist)
      if (response.success && response.data) {
        setInviteToken(response.data.token)
      } else {
        setError(response.error || 'Failed to create invite')
      }
    } catch (err) {
      setError(err.message || 'Failed to create invite')
    } finally {
      setCreatingInvite(false)
    }
  }

  const handleCopyToken = () => {
    if (inviteToken) {
      navigator.clipboard.writeText(inviteToken)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    }
  }

  const handleShareViaLink = async () => {
    if (!inviteToken) return
    setSharing(true)
    setError(null)
    try {
      const response = await shareTrustInvite(inviteToken)
      if (response.success && response.data) {
        setSharedInviteId(response.data.invite_id)
      } else {
        setError(response.error || 'Failed to share invite via Exemem')
      }
    } catch (err) {
      setError(err.message || 'Failed to share invite (is cloud backup enabled?)')
    } finally {
      setSharing(false)
    }
  }

  const handleFetchById = async () => {
    if (!fetchId.trim()) return
    setFetching(true)
    setError(null)
    try {
      const response = await fetchSharedInvite(fetchId.trim())
      if (response.success && response.data?.token) {
        setAcceptToken(response.data.token)
        setFetchId('')
        // Auto-preview
        const previewResp = await previewTrustInvite(response.data.token)
        if (previewResp.success && previewResp.data) {
          setPreview(previewResp.data)
          setAcceptDistance(String(previewResp.data.proposed_distance))
        }
      } else {
        setError(response.error || 'Invite not found or already claimed')
      }
    } catch (err) {
      setError(err.message || 'Failed to fetch invite')
    } finally {
      setFetching(false)
    }
  }

  const handlePreviewInvite = async () => {
    if (!acceptToken.trim()) return
    setPreviewing(true)
    setError(null)
    setPreview(null)
    try {
      const response = await previewTrustInvite(acceptToken.trim())
      if (response.success && response.data) {
        setPreview(response.data)
        setAcceptDistance(String(response.data.proposed_distance))
      } else {
        setError(response.error || 'Invalid invite token')
      }
    } catch (err) {
      setError(err.message || 'Failed to preview invite')
    } finally {
      setPreviewing(false)
    }
  }

  const handleAcceptInvite = async () => {
    if (!acceptToken.trim()) return
    setAccepting(true)
    setError(null)
    try {
      const dist = acceptDistance ? parseInt(acceptDistance, 10) : undefined
      const response = await acceptTrustInvite(acceptToken.trim(), dist, trustBack)
      if (response.success && response.data) {
        setAcceptToken('')
        setPreview(null)
        await fetchContacts()
        await fetchAuditLog()
        const msg = trustBack
          ? `Accepted and trusted back ${response.data.sender.display_name}`
          : `Accepted invite from ${response.data.sender.display_name}`
        if (onResult) onResult({ success: true, data: { message: msg } })
      } else {
        setError(response.error || 'Failed to accept invite')
      }
    } catch (err) {
      setError(err.message || 'Failed to accept invite')
    } finally {
      setAccepting(false)
    }
  }

  // ===== Helpers =====

  const truncateKey = (key) => {
    if (!key) return ''
    if (key.length <= 20) return key
    return `${key.slice(0, 10)}...${key.slice(-10)}`
  }

  const formatTimestamp = (isoString) => {
    try { return new Date(isoString).toLocaleString() }
    catch { return isoString }
  }

  const directionBadge = (direction) => {
    switch (direction) {
      case 'mutual': return <span className="badge badge-success text-xs">mutual</span>
      case 'outgoing': return <span className="badge badge-info text-xs">you trust them</span>
      case 'incoming': return <span className="badge badge-warning text-xs">they trust you</span>
      default: return null
    }
  }

  const formatAuditAction = (action) => {
    if (!action) return 'Unknown'
    if (action.TrustGrant) return `Grant trust to ${truncateKey(action.TrustGrant.user_id)} at distance ${action.TrustGrant.distance}`
    if (action.TrustRevoke) return `Revoke trust for ${truncateKey(action.TrustRevoke.user_id)}`
    if (action.Read) return `Read ${action.Read.schema_name}`
    if (action.Write) return `Write ${action.Write.schema_name}`
    if (action.AccessDenied) return `Access denied: ${action.AccessDenied.schema_name}`
    return JSON.stringify(action)
  }

  // ===== Sections =====

  const sections = [
    { id: 'contacts', label: 'Contacts' },
    { id: 'invite', label: 'Add Contact' },
    { id: 'identity', label: 'My Identity' },
    { id: 'audit', label: 'Audit Log' },
  ]

  return (
    <div className="p-6 max-w-4xl mx-auto">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h2 className="text-lg font-medium text-primary">Trust Graph</h2>
          <p className="text-sm text-secondary mt-1">
            Manage trusted contacts, share trust invites, and control who can access your data.
          </p>
        </div>
        <button
          className="btn btn-sm"
          onClick={() => { fetchContacts(); fetchAuditLog(); fetchIdentity() }}
        >
          Refresh
        </button>
      </div>

      {/* Section tabs */}
      <div className="flex gap-1 mb-6 border-b border-border">
        {sections.map(({ id, label }) => (
          <button
            key={id}
            className={`px-4 py-2 text-sm border-b-2 transition-colors ${
              activeSection === id
                ? 'border-gruvbox-blue text-gruvbox-blue font-medium'
                : 'border-transparent text-secondary hover:text-primary'
            }`}
            onClick={() => setActiveSection(id)}
          >
            {label}
            {id === 'contacts' && contacts.length > 0 && (
              <span className="ml-1.5 text-xs text-tertiary">({contacts.length})</span>
            )}
          </button>
        ))}
      </div>

      {/* Error */}
      {error && (
        <div className="card card-error mb-4">
          <p className="text-sm">{error}</p>
          <button className="text-xs underline mt-1" onClick={() => setError(null)}>Dismiss</button>
        </div>
      )}

      {/* ===== CONTACTS ===== */}
      {activeSection === 'contacts' && (
        <>
          {/* Identity card warning */}
          {!identityLoading && !identityCard && (
            <div className="bg-gruvbox-yellow/15 border border-gruvbox-yellow/30 rounded-lg p-4 mb-4">
              <p className="text-sm text-gruvbox-yellow">
                Set your display name in the <button className="underline font-medium" onClick={() => setActiveSection('identity')}>My Identity</button> tab
                before sharing trust invites.
              </p>
            </div>
          )}

          {contactsLoading && (
            <div className="text-center py-12">
              <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin mx-auto mb-3" />
              <p className="text-secondary text-sm">Loading contacts...</p>
            </div>
          )}

          {!contactsLoading && contacts.length === 0 && (
            <div className="text-center py-12 border border-border rounded-lg">
              <p className="text-secondary text-lg mb-2">No trusted contacts</p>
              <p className="text-tertiary text-sm mb-4">
                Share a trust invite or accept one to add your first contact.
              </p>
              <button className="btn" onClick={() => setActiveSection('invite')}>
                Add Contact
              </button>
            </div>
          )}

          {!contactsLoading && contacts.length > 0 && (
            <div className="space-y-2">
              {contacts.map((contact) => (
                <div
                  key={contact.public_key}
                  className="border border-border rounded-lg p-4 bg-surface"
                >
                  <div className="flex items-start justify-between gap-4">
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2 mb-1">
                        <span className="text-sm font-medium text-primary">
                          {contact.display_name}
                        </span>
                        {directionBadge(contact.direction)}
                        <span className="badge badge-info text-xs">
                          distance: {contact.trust_distance}
                        </span>
                      </div>
                      {contact.contact_hint && (
                        <p className="text-xs text-secondary mb-1">{contact.contact_hint}</p>
                      )}
                      <div className="flex items-center gap-3 text-xs text-tertiary">
                        <code title={contact.public_key}>{truncateKey(contact.public_key)}</code>
                        <span>Connected {formatTimestamp(contact.connected_at)}</span>
                      </div>
                    </div>
                    <button
                      className="btn btn-sm text-gruvbox-red border-gruvbox-red/30 hover:bg-gruvbox-red/10"
                      onClick={() => handleRevoke(contact.public_key)}
                      disabled={revoking === contact.public_key}
                    >
                      {revoking === contact.public_key ? 'Revoking...' : 'Revoke'}
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </>
      )}

      {/* ===== ADD CONTACT (INVITE) ===== */}
      {activeSection === 'invite' && (
        <>
          {/* Create invite */}
          <div className="border border-border rounded-lg p-4 mb-6 bg-surface">
            <h3 className="text-sm font-medium text-primary mb-1">Share a Trust Invite</h3>
            <p className="text-xs text-secondary mb-3">
              Generate a trust invite token to share with someone. They can paste it into their FoldDB to connect with you.
            </p>
            <form onSubmit={handleCreateInvite} className="flex gap-3 items-end mb-3">
              <div className="w-40">
                <label className="block text-xs text-secondary mb-1">Proposed Distance</label>
                <input
                  className="input w-full"
                  type="number"
                  min="1"
                  value={inviteDistance}
                  onChange={(e) => setInviteDistance(e.target.value)}
                />
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
                  Share this token directly, or use "Share via Link" to upload it to Exemem for easy sharing.
                </p>
                <div className="flex gap-2 mt-2">
                  <button
                    className="btn btn-sm"
                    onClick={handleShareViaLink}
                    disabled={sharing}
                  >
                    {sharing ? 'Uploading...' : 'Share via Link'}
                  </button>
                </div>
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
              </div>
            )}
          </div>

          {/* Accept invite */}
          <div className="border border-border rounded-lg p-4 bg-surface">
            <h3 className="text-sm font-medium text-primary mb-1">Accept a Trust Invite</h3>
            <p className="text-xs text-secondary mb-3">
              Enter a short invite ID (shared via link) or paste a full invite token.
            </p>

            {/* Fetch by invite ID */}
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
                  Wants to trust you at distance {preview.proposed_distance}
                </p>

                {preview.valid && (
                  <div className="flex items-end gap-3">
                    <div className="w-32">
                      <label className="block text-xs text-secondary mb-1">Your Distance</label>
                      <input
                        className="input w-full"
                        type="number"
                        min="1"
                        value={acceptDistance}
                        onChange={(e) => setAcceptDistance(e.target.value)}
                      />
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
                  </div>
                )}
              </div>
            )}
          </div>
        </>
      )}

      {/* ===== MY IDENTITY ===== */}
      {activeSection === 'identity' && (
        <div className="border border-border rounded-lg p-4 bg-surface">
          <h3 className="text-sm font-medium text-primary mb-1">Identity Card</h3>
          <p className="text-xs text-secondary mb-4">
            Your display name and contact hint are shared only with people you send trust invites to.
            This information stays on your device and is never synced to Exemem.
          </p>

          {identityLoading ? (
            <div className="text-center py-8">
              <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin mx-auto" />
            </div>
          ) : (
            <form onSubmit={handleSaveIdentity} className="space-y-4">
              <div>
                <label className="block text-xs text-secondary mb-1">Display Name *</label>
                <input
                  className="input w-full"
                  type="text"
                  placeholder="Your name..."
                  value={editName}
                  onChange={(e) => setEditName(e.target.value)}
                />
              </div>
              <div>
                <label className="block text-xs text-secondary mb-1">Contact Hint (optional)</label>
                <input
                  className="input w-full"
                  type="text"
                  placeholder="Email, phone, or handle for verification..."
                  value={editHint}
                  onChange={(e) => setEditHint(e.target.value)}
                />
                <p className="text-xs text-tertiary mt-1">
                  Helps others verify it's really you when they receive your trust invite.
                </p>
              </div>
              <button
                type="submit"
                className="btn"
                disabled={savingIdentity || !editName.trim()}
              >
                {savingIdentity ? 'Saving...' : (identityCard ? 'Update' : 'Save')}
              </button>
            </form>
          )}
        </div>
      )}

      {/* ===== AUDIT LOG ===== */}
      {activeSection === 'audit' && (
        <div>
          {auditEvents.length === 0 && (
            <div className="text-center py-12 border border-border rounded-lg">
              <p className="text-secondary text-lg mb-2">No audit events</p>
              <p className="text-tertiary text-sm">
                Trust operations will appear here as they occur.
              </p>
            </div>
          )}

          {auditEvents.length > 0 && (
            <div className="space-y-2">
              {auditEvents.map((event, idx) => (
                <div
                  key={event.id || idx}
                  className="border border-border rounded-lg p-3 bg-surface"
                >
                  <div className="flex items-start justify-between gap-3">
                    <div className="flex-1 min-w-0">
                      <p className="text-sm text-primary">
                        {formatAuditAction(event.action)}
                      </p>
                      <span className="text-xs text-tertiary">
                        {formatTimestamp(event.timestamp)}
                      </span>
                    </div>
                    <span className={`badge text-xs ${event.decision_granted ? 'badge-success' : 'badge-warning'}`}>
                      {event.decision_granted ? 'granted' : 'denied'}
                    </span>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

export default TrustTab
