import { useState, useEffect, useCallback } from 'react'
import {
  getIdentityCard,
  setIdentityCard,
  listContacts,
  revokeContact,
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
import { useTrustHandlers } from './trust/useTrustHandlers'

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

  // ===== Invite handlers (extracted to custom hook) =====

  const invite = useTrustHandlers({
    identityCard,
    onResult,
    setError,
    fetchContacts,
    fetchAuditLog,
    setActiveSection,
    setSelectedContact,
    handleAudit,
  })

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
          inviteRole={invite.inviteRole}
          setInviteRole={invite.setInviteRole}
          availableRoles={availableRoles}
          creatingInvite={invite.creatingInvite}
          identityCard={identityCard}
          handleCreateInvite={invite.handleCreateInvite}
          inviteToken={invite.inviteToken}
          handleCopyToken={invite.handleCopyToken}
          copied={invite.copied}
          handleShareViaLink={invite.handleShareViaLink}
          sharing={invite.sharing}
          showQr={invite.showQr}
          setShowQr={invite.setShowQr}
          sharedInviteId={invite.sharedInviteId}
          recipientEmail={invite.recipientEmail}
          setRecipientEmail={invite.setRecipientEmail}
          handleSendViaEmail={invite.handleSendViaEmail}
          sendingEmail={invite.sendingEmail}
          emailSentId={invite.emailSentId}
          reciprocalToken={invite.reciprocalToken}
          setReciprocalToken={invite.setReciprocalToken}
          verifyId={invite.verifyId}
          setVerifyId={invite.setVerifyId}
          verifyCode={invite.verifyCode}
          setVerifyCode={invite.setVerifyCode}
          handleVerifyCode={invite.handleVerifyCode}
          verifying={invite.verifying}
          fetchId={invite.fetchId}
          setFetchId={invite.setFetchId}
          handleFetchById={invite.handleFetchById}
          fetching={invite.fetching}
          acceptToken={invite.acceptToken}
          setAcceptToken={invite.setAcceptToken}
          preview={invite.preview}
          setPreview={invite.setPreview}
          handlePreviewInvite={invite.handlePreviewInvite}
          previewing={invite.previewing}
          acceptRole={invite.acceptRole}
          setAcceptRole={invite.setAcceptRole}
          trustBack={invite.trustBack}
          setTrustBack={invite.setTrustBack}
          handleAcceptInvite={invite.handleAcceptInvite}
          accepting={invite.accepting}
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
