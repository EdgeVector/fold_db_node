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
  sendVerifiedInvite,
  verifyInviteCode,
  listSharingRoles,
  assignRoleToContact,
  removeRoleFromContact,
  auditContactAccess,
  getSharingPosture,
  getAuditLog,
} from '../../api/clients/trustClient'
import ContactsPanel from './trust/ContactsPanel'
import InvitePanel from './trust/InvitePanel'
import SharingOverviewPanel from './trust/SharingOverviewPanel'
import IdentityCardPanel from './trust/IdentityCardPanel'
import AuditLogPanel from './trust/AuditLogPanel'

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
    } catch (err) {
      console.error('Failed to load identity card:', err)
    } finally {
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
    } catch (err) {
      console.error('Failed to load contacts:', err)
    } finally {
      setContactsLoading(false)
    }
  }, [])

  const fetchAuditLog = useCallback(async () => {
    try {
      const response = await getAuditLog(50)
      if (response.success && response.data) {
        setAuditEvents(response.data.events || [])
      }
    } catch (err) {
      console.error('Failed to load audit log:', err)
    }
  }, [])

  const fetchRoles = useCallback(async () => {
    try {
      const response = await listSharingRoles()
      if (response.success && response.data) {
        setAvailableRoles(response.data.roles || {})
      }
    } catch (err) {
      console.error('Failed to load sharing roles:', err)
    }
  }, [])

  useEffect(() => {
    fetchIdentity()
    fetchContacts()
    fetchAuditLog()
    fetchRoles()
    getSharingPosture().then(r => { if (r.success && r.data) setPosture(r.data) }).catch(() => {})
  }, [fetchIdentity, fetchContacts, fetchAuditLog, fetchRoles])

  // ===== Role & audit handlers =====

  const handleAudit = async (publicKey) => {
    setSelectedContact(publicKey)
    setAuditLoading(true)
    setAuditResult(null)
    try {
      const response = await auditContactAccess(publicKey)
      if (response.success && response.data) {
        setAuditResult(response.data)
      }
    } catch (err) {
      setError(err?.message || 'Failed to audit contact access')
    } finally {
      setAuditLoading(false)
    }
  }

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
        <div className="card card-error mb-4 p-4">
          <p className="text-sm">{error}</p>
          <button className="text-xs underline mt-1" onClick={() => setError(null)}>Dismiss</button>
        </div>
      )}

      {activeSection === 'contacts' && (
        <ContactsPanel
          identityLoading={identityLoading}
          identityCard={identityCard}
          setActiveSection={setActiveSection}
          contactsLoading={contactsLoading}
          contacts={contacts}
          selectedContact={selectedContact}
          setSelectedContact={setSelectedContact}
          setAuditResult={setAuditResult}
          handleAudit={handleAudit}
          handleRevoke={handleRevoke}
          revoking={revoking}
          availableRoles={availableRoles}
          assigningRole={assigningRole}
          handleAssignRole={handleAssignRole}
          handleRemoveRole={handleRemoveRole}
          auditLoading={auditLoading}
          auditResult={auditResult}
        />
      )}

      {activeSection === 'invite' && (
        <InvitePanel
          inviteRole={inviteRole}
          setInviteRole={setInviteRole}
          availableRoles={availableRoles}
          creatingInvite={creatingInvite}
          identityCard={identityCard}
          handleCreateInvite={handleCreateInvite}
          inviteToken={inviteToken}
          handleCopyToken={handleCopyToken}
          copied={copied}
          handleShareViaLink={handleShareViaLink}
          sharing={sharing}
          showQr={showQr}
          setShowQr={setShowQr}
          sharedInviteId={sharedInviteId}
          recipientEmail={recipientEmail}
          setRecipientEmail={setRecipientEmail}
          handleSendViaEmail={handleSendViaEmail}
          sendingEmail={sendingEmail}
          emailSentId={emailSentId}
          reciprocalToken={reciprocalToken}
          setReciprocalToken={setReciprocalToken}
          verifyId={verifyId}
          setVerifyId={setVerifyId}
          verifyCode={verifyCode}
          setVerifyCode={setVerifyCode}
          handleVerifyCode={handleVerifyCode}
          verifying={verifying}
          fetchId={fetchId}
          setFetchId={setFetchId}
          handleFetchById={handleFetchById}
          fetching={fetching}
          acceptToken={acceptToken}
          setAcceptToken={setAcceptToken}
          preview={preview}
          setPreview={setPreview}
          handlePreviewInvite={handlePreviewInvite}
          previewing={previewing}
          acceptRole={acceptRole}
          setAcceptRole={setAcceptRole}
          trustBack={trustBack}
          setTrustBack={setTrustBack}
          handleAcceptInvite={handleAcceptInvite}
          accepting={accepting}
          setError={setError}
          onResult={onResult}
        />
      )}

      {activeSection === 'posture' && (
        <SharingOverviewPanel
          posture={posture}
          setPosture={setPosture}
          setError={setError}
          onResult={onResult}
        />
      )}

      {activeSection === 'identity' && (
        <IdentityCardPanel
          identityLoading={identityLoading}
          identityCard={identityCard}
          editName={editName}
          setEditName={setEditName}
          editHint={editHint}
          setEditHint={setEditHint}
          handleSaveIdentity={handleSaveIdentity}
          savingIdentity={savingIdentity}
        />
      )}

      {activeSection === 'audit' && (
        <AuditLogPanel auditEvents={auditEvents} />
      )}
    </div>
  )
}

export default TrustTab
