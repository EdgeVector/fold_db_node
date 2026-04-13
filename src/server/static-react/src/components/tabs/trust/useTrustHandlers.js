import { useState } from 'react'
import {
  createTrustInvite,
  previewTrustInvite,
  acceptTrustInvite,
  shareTrustInvite,
  fetchSharedInvite,
  sendVerifiedInvite,
  verifyInviteCode,
} from '../../../api/clients/trustClient'

/**
 * Custom hook encapsulating trust invite state and handlers:
 * create, copy, share (link/QR/email), fetch by id, verify code,
 * preview, and accept.
 *
 * Pure refactor — behavior matches the previous inline closures in TrustTab.
 */
export function useTrustHandlers({
  identityCard,
  onResult,
  setError,
  fetchContacts,
  fetchAuditLog,
  setActiveSection,
  setSelectedContact,
  handleAudit,
}) {
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

  return {
    // state
    inviteRole, setInviteRole,
    creatingInvite,
    inviteToken,
    copied,
    showQr, setShowQr,
    sharing,
    sharedInviteId,
    fetchId, setFetchId,
    fetching,
    recipientEmail, setRecipientEmail,
    sendingEmail,
    emailSentId,
    verifyId, setVerifyId,
    verifyCode, setVerifyCode,
    verifying,
    acceptToken, setAcceptToken,
    preview, setPreview,
    previewing,
    accepting,
    reciprocalToken, setReciprocalToken,
    acceptRole, setAcceptRole,
    trustBack, setTrustBack,
    // handlers
    handleCreateInvite,
    handleCopyToken,
    handleShareViaLink,
    handleFetchById,
    handleSendViaEmail,
    handleVerifyCode,
    handlePreviewInvite,
    handleAcceptInvite,
  }
}
