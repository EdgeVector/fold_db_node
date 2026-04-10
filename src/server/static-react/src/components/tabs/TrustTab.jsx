import { useState, useEffect, useCallback } from 'react'
import { QRCodeSVG } from 'qrcode.react'
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
  sendVerifiedInvite,
  verifyInviteCode,
  listSharingRoles,
  assignRoleToContact,
  removeRoleFromContact,
  auditContactAccess,
  getSharingPosture,
  declineTrustInvite,
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

  // Roles & audit
  const [availableRoles, setAvailableRoles] = useState({})
  const [selectedContact, setSelectedContact] = useState(null)
  const [auditResult, setAuditResult] = useState(null)
  const [auditLoading, setAuditLoading] = useState(false)
  const [assigningRole, setAssigningRole] = useState(false)

  // Posture
  const [posture, setPosture] = useState(null)

  // Invite creation
  const [inviteRole, setInviteRole] = useState('friend')
  const [creatingInvite, setCreatingInvite] = useState(false)
  const [inviteToken, setInviteToken] = useState(null)
  const [copied, setCopied] = useState(false)
  const [showQr, setShowQr] = useState(false)
  const [sharing, setSharing] = useState(false)
  const [sharedInviteId, setSharedInviteId] = useState(null)
  const [fetchId, setFetchId] = useState('')
  const [fetching, setFetching] = useState(false)

  // Email verification
  const [recipientEmail, setRecipientEmail] = useState('')
  const [sendingEmail, setSendingEmail] = useState(false)
  const [emailSentId, setEmailSentId] = useState(null)
  const [verifyId, setVerifyId] = useState('')
  const [verifyCode, setVerifyCode] = useState('')
  const [verifying, setVerifying] = useState(false)

  // Invite acceptance
  const [acceptToken, setAcceptToken] = useState('')
  const [preview, setPreview] = useState(null)
  const [previewing, setPreviewing] = useState(false)
  const [accepting, setAccepting] = useState(false)
  const [reciprocalToken, setReciprocalToken] = useState(null)
  const [acceptRole, setAcceptRole] = useState('')
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

  const fetchRoles = useCallback(async () => {
    try {
      const response = await listSharingRoles()
      if (response.success && response.data) {
        setAvailableRoles(response.data.roles || {})
      }
    } catch { /* ignore */ }
  }, [])

  useEffect(() => {
    fetchIdentity()
    fetchContacts()
    fetchAuditLog()
    fetchRoles()
    getSharingPosture().then(r => { if (r.success && r.data) setPosture(r.data) }).catch(() => {})
  }, [fetchIdentity, fetchContacts, fetchAuditLog, fetchRoles])

  // ===== Role & audit handlers =====

  const handleAssignRole = async (publicKey, roleName) => {
    setAssigningRole(true)
    setError(null)
    try {
      const response = await assignRoleToContact(publicKey, roleName)
      if (response.success) {
        await fetchContacts()
        if (selectedContact === publicKey) handleAudit(publicKey)
        if (onResult) onResult({ success: true, data: { message: `Role "${roleName}" assigned` } })
      } else {
        setError(response.error || 'Failed to assign role')
      }
    } catch (err) {
      setError(err.message || 'Failed to assign role')
    } finally {
      setAssigningRole(false)
    }
  }

  const handleRemoveRole = async (publicKey, domain) => {
    setError(null)
    try {
      const response = await removeRoleFromContact(publicKey, domain)
      if (response.success) {
        await fetchContacts()
        if (selectedContact === publicKey) handleAudit(publicKey)
      } else {
        setError(response.error || 'Failed to remove role')
      }
    } catch (err) {
      setError(err.message || 'Failed to remove role')
    }
  }

  const handleAudit = async (publicKey) => {
    setSelectedContact(publicKey)
    setAuditLoading(true)
    setAuditResult(null)
    try {
      const response = await auditContactAccess(publicKey)
      if (response.success && response.data) {
        setAuditResult(response.data)
      }
    } catch { /* ignore */ } finally {
      setAuditLoading(false)
    }
  }

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
    if (!inviteRole.trim()) {
      setError('Please select a role')
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
      const response = await createTrustInvite(inviteRole.trim())
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
          setAcceptRole(previewResp.data.proposed_role)
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

  const handleSendViaEmail = async () => {
    if (!recipientEmail.trim() || !inviteToken || !identityCard) return
    setSendingEmail(true)
    setError(null)
    try {
      const response = await sendVerifiedInvite(
        inviteToken,
        recipientEmail.trim(),
        identityCard.display_name,
      )
      if (response.success && response.data) {
        setEmailSentId(response.data.invite_id)
        if (onResult) onResult({ success: true, data: { message: `Verification code sent to ${recipientEmail}` } })
      } else {
        setError(response.error || 'Failed to send verification email')
      }
    } catch (err) {
      setError(err.message || 'Failed to send email (is cloud backup enabled?)')
    } finally {
      setSendingEmail(false)
    }
  }

  const handleVerifyCode = async () => {
    if (!verifyId.trim() || !verifyCode.trim()) return
    setVerifying(true)
    setError(null)
    try {
      const response = await verifyInviteCode(verifyId.trim(), verifyCode.trim())
      if (response.success && response.data?.token) {
        setAcceptToken(response.data.token)
        setVerifyId('')
        setVerifyCode('')
        // Auto-preview
        const previewResp = await previewTrustInvite(response.data.token)
        if (previewResp.success && previewResp.data) {
          setPreview(previewResp.data)
          setAcceptRole(previewResp.data.proposed_role)
        }
        if (onResult) onResult({ success: true, data: { message: 'Code verified! Review the invite below.' } })
      } else {
        setError(response.error || 'Invalid verification code')
      }
    } catch (err) {
      setError(err.message || 'Verification failed')
    } finally {
      setVerifying(false)
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
        setAcceptRole(response.data.proposed_role)
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
      const roleOverride = acceptRole ? acceptRole : undefined
      const response = await acceptTrustInvite(acceptToken.trim(), roleOverride, trustBack)
      if (response.success && response.data) {
        const senderKey = response.data.sender?.public_key
        if (response.data.reciprocal_token) {
          setReciprocalToken(response.data.reciprocal_token)
        }
        setAcceptToken('')
        setPreview(null)
        await fetchContacts()
        await fetchAuditLog()
        // Switch to contacts and open the new contact for role assignment
        if (senderKey) {
          setActiveSection('contacts')
          setSelectedContact(senderKey)
          handleAudit(senderKey)
        }
        const msg = trustBack
          ? `Accepted ${response.data.sender.display_name} — assign a role below`
          : `Accepted invite from ${response.data.sender.display_name} — assign a role below`
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
    if (action.TrustGrant) return `Grant trust to ${truncateKey(action.TrustGrant.user_id)} at tier ${action.TrustGrant.tier}`
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
    { id: 'posture', label: 'Sharing Overview' },
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
                      </div>
                      {/* Role badges */}
                      {contact.roles && Object.keys(contact.roles).length > 0 && (
                        <div className="flex flex-wrap gap-1 mb-1">
                          {Object.entries(contact.roles).map(([domain, role]) => (
                            <span key={domain} className="badge badge-info text-xs">
                              {role} ({domain})
                            </span>
                          ))}
                        </div>
                      )}
                      {contact.contact_hint && (
                        <p className="text-xs text-secondary mb-1">{contact.contact_hint}</p>
                      )}
                      <div className="flex items-center gap-3 text-xs text-tertiary">
                        <code title={contact.public_key}>{truncateKey(contact.public_key)}</code>
                        <span>Connected {formatTimestamp(contact.connected_at)}</span>
                      </div>
                    </div>
                    <div className="flex gap-1 flex-shrink-0">
                      <button
                        className="btn btn-sm"
                        onClick={() => {
                          if (selectedContact === contact.public_key) {
                            setSelectedContact(null)
                            setAuditResult(null)
                          } else {
                            handleAudit(contact.public_key)
                          }
                        }}
                      >
                        {selectedContact === contact.public_key ? 'Close' : 'Manage'}
                      </button>
                      <button
                        className="btn btn-sm text-gruvbox-red border-gruvbox-red/30 hover:bg-gruvbox-red/10"
                        onClick={() => handleRevoke(contact.public_key)}
                        disabled={revoking === contact.public_key}
                      >
                        {revoking === contact.public_key ? '...' : 'Revoke'}
                      </button>
                    </div>
                  </div>

                  {/* Expanded contact detail: role assignment + audit */}
                  {selectedContact === contact.public_key && (
                    <div className="mt-3 pt-3 border-t border-border">
                      {/* Role assignment */}
                      <h4 className="text-xs font-medium text-secondary mb-2">Assign Roles</h4>
                      <div className="flex flex-wrap gap-2 mb-3">
                        {Object.values(availableRoles).map((role) => {
                          const isActive = contact.roles?.[role.domain] === role.name
                          return (
                            <button
                              key={role.name}
                              className={`text-xs px-2 py-1 rounded border transition-colors ${
                                isActive
                                  ? 'bg-gruvbox-blue/20 border-gruvbox-blue text-gruvbox-blue'
                                  : 'border-border text-secondary hover:border-gruvbox-blue hover:text-primary'
                              }`}
                              onClick={() => {
                                if (isActive) {
                                  handleRemoveRole(contact.public_key, role.domain)
                                } else {
                                  handleAssignRole(contact.public_key, role.name)
                                }
                              }}
                              disabled={assigningRole}
                              title={`${role.description} (${role.domain} domain, tier ${role.tier})`}
                            >
                              {role.name.replace(/_/g, ' ')}
                            </button>
                          )
                        })}
                      </div>

                      {/* Sharing audit */}
                      <h4 className="text-xs font-medium text-secondary mb-2">
                        What {contact.display_name} can see
                      </h4>
                      {auditLoading && (
                        <div className="text-center py-4">
                          <div className="w-4 h-4 border-2 border-border border-t-primary rounded-full animate-spin mx-auto" />
                        </div>
                      )}
                      {auditResult && !auditLoading && (
                        <div>
                          {auditResult.accessible_schemas.length === 0 ? (
                            <p className="text-xs text-tertiary">No data accessible. Assign a role to start sharing.</p>
                          ) : (
                            <>
                              <p className="text-xs text-secondary mb-2">
                                {auditResult.total_readable} readable field{auditResult.total_readable !== 1 ? 's' : ''} across {auditResult.accessible_schemas.length} schema{auditResult.accessible_schemas.length !== 1 ? 's' : ''}
                              </p>
                              <div className="space-y-1 max-h-48 overflow-y-auto">
                                {auditResult.accessible_schemas.map((schema) => (
                                  <div key={schema.schema_name} className="text-xs p-2 bg-surface-secondary rounded">
                                    <div className="flex items-center gap-2">
                                      <span className="font-medium text-primary">{schema.schema_name}</span>
                                      <span className="text-tertiary">({schema.trust_domain})</span>
                                    </div>
                                    <span className="text-gruvbox-green">
                                      {schema.readable_fields.length} readable
                                    </span>
                                    {schema.writable_fields.length > 0 && (
                                      <span className="text-gruvbox-yellow ml-2">
                                        {schema.writable_fields.length} writable
                                      </span>
                                    )}
                                  </div>
                                ))}
                              </div>
                            </>
                          )}
                        </div>
                      )}
                    </div>
                  )}
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
                        } catch { /* ignore */ }
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
      )}

      {/* ===== SHARING OVERVIEW ===== */}
      {activeSection === 'posture' && (
        <div>
          {posture ? (
            <div className="space-y-4">
              <div className="grid grid-cols-2 gap-4">
                <div className="border border-border rounded-lg p-4 bg-surface text-center">
                  <div className="text-2xl font-bold text-primary">{posture.total_policy_fields}</div>
                  <div className="text-xs text-secondary">Protected fields</div>
                </div>
                <div className="border border-border rounded-lg p-4 bg-surface text-center">
                  <div className="text-2xl font-bold text-gruvbox-yellow">{posture.total_unprotected_fields}</div>
                  <div className="text-xs text-secondary">Unprotected fields</div>
                </div>
              </div>
              {posture.total_unprotected_fields > 0 && (
                <div className="bg-gruvbox-yellow/15 border border-gruvbox-yellow/30 rounded-lg p-4">
                  <p className="text-sm text-gruvbox-yellow font-medium mb-1">
                    {posture.total_unprotected_fields} fields have no access policy
                  </p>
                  <p className="text-xs text-secondary mb-3">
                    Anyone you trust in any domain can see these fields. Apply default policies based on data classification to protect them.
                  </p>
                  <button
                    className="btn btn-sm"
                    onClick={async () => {
                      try {
                        const resp = await fetch('/api/sharing/apply-defaults', { method: 'POST' })
                        const data = await resp.json()
                        if (data.ok !== false) {
                          const d = data.data || data
                          if (onResult) onResult({ success: true, data: { message: `Applied policies to ${d.fields_updated} fields across ${d.schemas_updated} schemas` } })
                          getSharingPosture().then(r => { if (r.success && r.data) setPosture(r.data) }).catch(() => {})
                        }
                      } catch { /* ignore */ }
                    }}
                  >
                    Apply Default Policies
                  </button>
                </div>
              )}
              {posture.domains.length > 0 && (
                <div className="border border-border rounded-lg p-4 bg-surface">
                  <h3 className="text-sm font-medium text-primary mb-3">Trust Domains</h3>
                  <div className="space-y-2">
                    {posture.domains.map((domain) => (
                      <div key={domain} className="flex items-center justify-between text-sm">
                        <span className="text-primary">{domain}</span>
                        <div className="flex gap-3 text-xs text-secondary">
                          <span>{posture.schemas_per_domain[domain] || 0} schemas</span>
                          <span>{posture.contacts_per_domain[domain] || 0} contacts</span>
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              )}
              {posture.domains.length === 0 && (
                <div className="text-center py-8 border border-border rounded-lg">
                  <p className="text-secondary">No trust domains configured yet.</p>
                  <p className="text-xs text-tertiary mt-1">Assign roles to contacts to create trust domains.</p>
                </div>
              )}
            </div>
          ) : (
            <div className="text-center py-8">
              <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin mx-auto" />
            </div>
          )}
        </div>
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
