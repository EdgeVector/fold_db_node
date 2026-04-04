import React, { useState, useEffect } from 'react'
import { systemClient } from '../../api/clients/systemClient'
import { getSubscriptionStatus, createCheckoutSession, createPortalSession, formatBytes, usagePercent } from '../../api/clients/subscriptionClient'

export default function CloudMigrationSettings({ onClose }) {
  const backupSkipped = localStorage.getItem('folddb_cloud_backup_skipped') === '1'
  const [registering, setRegistering] = useState(false)
  const [error, setError] = useState(null)
  const [inviteCode, setInviteCode] = useState('')

  // Storage tier state (cloud mode only)
  const [isCloudMode, setIsCloudMode] = useState(false)
  const [storageInfo, setStorageInfo] = useState(null)
  const [upgrading, setUpgrading] = useState(false)
  const [hasCredentials, setHasCredentials] = useState(null)
  const [recoveryWords, setRecoveryWords] = useState(null)
  const [showRecovery, setShowRecovery] = useState(false)
  const [inviteCodes, setInviteCodes] = useState(null)
  const [creatingCode, setCreatingCode] = useState(false)

  useEffect(() => {
    // Detect Stripe checkout return
    const params = new URLSearchParams(window.location.search)
    if (params.get('subscription') === 'success') {
      window.history.replaceState({}, '', window.location.pathname)
    }

    // Check if already in cloud mode via localStorage OR keychain
    const hasCloudConfig = localStorage.getItem('exemem_api_url') && localStorage.getItem('exemem_api_key')

    const detectCloudMode = async () => {
      // Check keychain credentials as fallback
      let keychainCreds = false
      try {
        const resp = await fetch('/api/auth/credentials')
        const data = await resp.json()
        keychainCreds = data.ok && data.has_credentials
        setHasCredentials(keychainCreds)
      } catch {
        setHasCredentials(false)
      }

      if (hasCloudConfig || keychainCreds) {
        setIsCloudMode(true)
        try {
          const status = await getSubscriptionStatus()
          setStorageInfo({
            plan: status.plan,
            used_bytes: status.storage.used_bytes,
            quota_bytes: status.storage.quota_bytes,
            has_subscription: status.has_subscription,
          })
        } catch {
          // Cloud API not reachable — still show connected state with defaults
          setStorageInfo({
            plan: 'free',
            used_bytes: 0,
            quota_bytes: 1073741824, // 1 GB
            has_subscription: false,
            offline: true,
          })
        }
      }
    }
    detectCloudMode()
  }, [])

  const handleEnableCloud = async () => {
    if (!inviteCode.trim()) {
      setError('Invite code is required')
      return
    }
    setRegistering(true)
    setError(null)
    try {
      // Register this node's public key with Exemem
      const resp = await fetch('/api/auth/register', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ invite_code: inviteCode.trim() }),
      })
      const data = await resp.json()

      if (!data.ok) {
        throw new Error(data.error || 'Registration failed')
      }

      // Store cloud credentials for subscription client
      localStorage.setItem('exemem_api_url', data.api_url)
      localStorage.setItem('exemem_api_key', data.api_key)

      // Switch database to Exemem mode
      await systemClient.applySetup({
        storage: {
          type: 'exemem',
          api_url: data.api_url,
          api_key: data.api_key,
        }
      })

      // Reload to pick up new config
      window.location.reload()
    } catch (err) {
      setError(err.message || 'Failed to enable cloud backup')
      setRegistering(false)
    }
  }

  const handleUpgrade = async () => {
    setUpgrading(true)
    setError(null)
    try {
      const url = await createCheckoutSession()
      window.location.href = url
    } catch (err) {
      setError(err.message || 'Failed to start checkout')
      setUpgrading(false)
    }
  }

  const handleManageSubscription = async () => {
    setError(null)
    try {
      const url = await createPortalSession()
      window.location.href = url
    } catch (err) {
      setError(err.message || 'Failed to open billing portal')
    }
  }

  // Cloud mode: show storage tier info
  if (isCloudMode && storageInfo) {
    const pct = usagePercent(storageInfo.used_bytes, storageInfo.quota_bytes)
    const barColor = pct > 90 ? 'bg-gruvbox-red' : pct > 80 ? 'bg-gruvbox-orange' : 'bg-gruvbox-green'
    const isFree = storageInfo.plan === 'free'

    return (
      <div className="flex flex-col gap-6 w-full max-w-2xl text-gruvbox-bright p-4 border border-border rounded-md bg-surface shadow-md">
        <h3 className="text-sm font-bold uppercase tracking-widest text-gruvbox-light">Cloud Storage</h3>

        {storageInfo.offline && (
          <div className="flex items-start gap-3 p-3 border border-gruvbox-yellow bg-gruvbox-yellow/5 rounded-md">
            <span className="text-gruvbox-yellow text-sm flex-shrink-0">!</span>
            <p className="text-xs text-gruvbox-yellow leading-relaxed">
              Connected to Exemem but couldn't reach the cloud API. Storage info may be outdated.
            </p>
          </div>
        )}

        {/* Usage bar */}
        <div className="flex flex-col gap-2">
          <div className="flex justify-between text-xs text-gruvbox-dim">
            <span>{formatBytes(storageInfo.used_bytes)} used</span>
            <span>{formatBytes(storageInfo.quota_bytes)} total</span>
          </div>
          <div className="w-full h-2 bg-surface-elevated rounded-full overflow-hidden">
            <div className={`h-full ${barColor} rounded-full transition-all`} style={{ width: `${Math.max(1, pct)}%` }} />
          </div>
          <div className="text-xs text-gruvbox-dim text-right">{pct.toFixed(1)}% used</div>
        </div>

        {/* Plan info */}
        <div className="flex items-center justify-between p-4 border border-border rounded-md bg-surface-elevated">
          <div>
            <div className="text-sm font-bold text-gruvbox-bright">
              {isFree ? 'Free Plan' : 'Paid Plan'}
            </div>
            <div className="text-xs text-gruvbox-dim mt-1">
              {isFree
                ? `1 GB backup storage included`
                : `50 GB backup storage ($5/mo)`}
            </div>
          </div>
          {isFree ? (
            <button
              onClick={handleUpgrade}
              disabled={upgrading}
              className="px-4 py-2 text-xs font-bold border border-gruvbox-green text-surface bg-gruvbox-green hover:bg-gruvbox-green/90 rounded-md transition-colors cursor-pointer disabled:opacity-50"
            >
              {upgrading ? 'Redirecting...' : 'Upgrade to 50 GB - $5/mo'}
            </button>
          ) : (
            <button
              onClick={handleManageSubscription}
              className="px-4 py-2 text-xs font-bold border border-border text-gruvbox-dim hover:text-gruvbox-bright rounded-md transition-colors cursor-pointer"
            >
              Manage Subscription
            </button>
          )}
        </div>

        {pct > 80 && isFree && (
          <div className="flex items-start gap-3 p-3 border border-gruvbox-orange bg-gruvbox-orange/5 rounded-md">
            <span className="text-gruvbox-orange text-sm flex-shrink-0">!</span>
            <p className="text-xs text-gruvbox-orange leading-relaxed">
              You're running low on storage. Upgrade to the paid plan for 50 GB of backup storage.
            </p>
          </div>
        )}

        {/* Passkey recovery */}
        <div className="flex items-center justify-between p-4 border border-border rounded-md bg-surface-elevated">
          <div>
            <div className="text-sm font-bold text-gruvbox-bright">Web Recovery</div>
            <div className="text-xs text-gruvbox-dim mt-1">
              Add a passkey so you can recover your account from any browser
            </div>
          </div>
          <button
            onClick={async () => {
              try {
                const resp = await fetch('/api/auth/credentials')
                const data = await resp.json()
                if (!data.ok || !data.session_token) {
                  setError('No active session. Try re-enabling cloud backup.')
                  return
                }
                const exememUrl = localStorage.getItem('exemem_api_url') || 'https://exemem.com'
                const baseUrl = exememUrl.replace(/\/api\/?$/, '').replace(/execute-api\.[^/]+/, 'exemem.com')
                window.open(`${baseUrl}/link-passkey?session=${encodeURIComponent(data.session_token)}`, '_blank')
              } catch {
                setError('Failed to get session token')
              }
            }}
            className="px-4 py-2 text-xs font-bold border border-gruvbox-blue text-gruvbox-blue hover:bg-gruvbox-blue/10 rounded-md transition-colors cursor-pointer"
          >
            Add Passkey
          </button>
        </div>

        {/* Recovery phrase */}
        <div className="p-4 border border-border rounded-md bg-surface-elevated">
          <div className="flex items-center justify-between">
            <div>
              <div className="text-sm font-bold text-gruvbox-bright">Recovery Phrase</div>
              <div className="text-xs text-gruvbox-dim mt-1">
                24-word phrase to restore your account on a new device
              </div>
            </div>
            <button
              onClick={async () => {
                if (showRecovery) { setShowRecovery(false); setRecoveryWords(null); return }
                try {
                  const resp = await fetch('/api/auth/recovery-phrase')
                  const data = await resp.json()
                  if (data.ok) { setRecoveryWords(data.words); setShowRecovery(true) }
                  else setError(data.error || 'Failed to get recovery phrase')
                } catch { setError('Failed to get recovery phrase') }
              }}
              className="px-4 py-2 text-xs font-bold border border-gruvbox-purple text-gruvbox-purple hover:bg-gruvbox-purple/10 rounded-md transition-colors cursor-pointer"
            >
              {showRecovery ? 'Hide' : 'Show Phrase'}
            </button>
          </div>
          {showRecovery && recoveryWords && (
            <div className="grid grid-cols-4 gap-1.5 mt-3 p-3 border border-border rounded bg-surface font-mono text-xs">
              {recoveryWords.map((word, i) => (
                <div key={i} className="flex items-center gap-1">
                  <span className="text-gruvbox-dim w-5 text-right">{i + 1}.</span>
                  <span className="text-gruvbox-bright">{word}</span>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Invite codes */}
        <div className="p-4 border border-border rounded-md bg-surface-elevated">
          <div className="flex items-center justify-between mb-3">
            <div>
              <div className="text-sm font-bold text-gruvbox-bright">Invite Codes</div>
              <div className="text-xs text-gruvbox-dim mt-1">
                Share invite codes so others can join Exemem
              </div>
            </div>
            <button
              onClick={async () => {
                setCreatingCode(true)
                try {
                  const resp = await fetch('/api/auth/invite-codes', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: '{}',
                  })
                  // Proxy through node — need to forward to Exemem API
                  const creds = await fetch('/api/auth/credentials').then(r => r.json())
                  if (!creds.ok) { setError('No session'); return }
                  const apiUrl = localStorage.getItem('exemem_api_url') || 'https://ygyu7ritx8.execute-api.us-west-2.amazonaws.com'
                  const createResp = await fetch(`${apiUrl}/api/auth/invite-codes`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'Authorization': `Bearer ${creds.session_token}` },
                    body: '{}',
                  })
                  const createData = await createResp.json()
                  if (createData.ok) {
                    // Refresh the list
                    const listResp = await fetch(`${apiUrl}/api/auth/invite-codes`, {
                      headers: { 'Authorization': `Bearer ${creds.session_token}` },
                    })
                    const listData = await listResp.json()
                    if (listData.ok) setInviteCodes(listData.codes)
                  } else {
                    setError(createData.error || 'Failed to create invite code')
                  }
                } catch (e) { setError(e?.message || 'Failed to create invite code') }
                finally { setCreatingCode(false) }
              }}
              disabled={creatingCode}
              className="px-4 py-2 text-xs font-bold border border-gruvbox-green text-gruvbox-green hover:bg-gruvbox-green/10 rounded-md transition-colors cursor-pointer disabled:opacity-50"
            >
              {creatingCode ? 'Creating...' : 'Create Code'}
            </button>
          </div>
          {inviteCodes === null && (
            <button
              onClick={async () => {
                try {
                  const creds = await fetch('/api/auth/credentials').then(r => r.json())
                  if (!creds.ok) return
                  const apiUrl = localStorage.getItem('exemem_api_url') || 'https://ygyu7ritx8.execute-api.us-west-2.amazonaws.com'
                  const resp = await fetch(`${apiUrl}/api/auth/invite-codes`, {
                    headers: { 'Authorization': `Bearer ${creds.session_token}` },
                  })
                  const data = await resp.json()
                  if (data.ok) setInviteCodes(data.codes)
                } catch { /* ignore */ }
              }}
              className="text-xs text-gruvbox-blue underline cursor-pointer bg-transparent border-none"
            >
              Load invite codes
            </button>
          )}
          {inviteCodes && inviteCodes.length === 0 && (
            <p className="text-xs text-gruvbox-dim">No invite codes yet. Create one to share.</p>
          )}
          {inviteCodes && inviteCodes.length > 0 && (
            <div className="space-y-2">
              {inviteCodes.map((c, i) => (
                <div key={i} className="flex items-center justify-between p-2 border border-border rounded bg-surface text-xs">
                  <span className="font-mono text-gruvbox-bright tracking-wider">{c.code}</span>
                  <span className={c.redeemed_by ? 'text-gruvbox-dim' : 'text-gruvbox-green'}>
                    {c.redeemed_by ? 'Used' : 'Active'}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>

        {error && (
          <div className="flex items-start gap-3 p-3 border border-gruvbox-red bg-gruvbox-red/5 rounded-md">
            <span className="text-gruvbox-red text-sm flex-shrink-0">!</span>
            <p className="text-xs text-gruvbox-red leading-relaxed">{error}</p>
          </div>
        )}
      </div>
    )
  }

  // Not in cloud mode: show one-click setup
  return (
    <div className="flex flex-col gap-6 w-full max-w-2xl text-gruvbox-bright p-4 border border-border rounded-md bg-surface shadow-md">

      {/* Skipped backup reminder */}
      {backupSkipped && (
        <div className="flex items-start gap-3 p-4 border border-gruvbox-yellow bg-gruvbox-yellow/5 rounded-md">
          <div className="text-gruvbox-yellow mt-0.5 flex-shrink-0">
            <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-2.5L13.732 4c-.77-.833-1.964-.833-2.732 0L4.082 16.5c-.77.833.192 2.5 1.732 2.5z" />
            </svg>
          </div>
          <div>
            <p className="text-sm font-bold text-gruvbox-yellow">Cloud backup not enabled</p>
            <p className="text-xs text-gruvbox-light mt-1">
              You skipped cloud backup during setup. Your data is only stored locally.
              Enable encrypted cloud backup below to protect against data loss.
            </p>
          </div>
        </div>
      )}

      {/* Explanation Banner */}
      <div className="flex items-start gap-4 p-4 border border-gruvbox-blue bg-gruvbox-blue/5 rounded-md">
        <div className="text-gruvbox-blue mt-1 flex-shrink-0">
          <svg className="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 15a4 4 0 004 4h9a5 5 0 10-.1-9.999 5.002 5.002 0 10-9.78 2.096A4.001 4.001 0 003 15z" />
          </svg>
        </div>
        <div>
          <h4 className="text-sm font-bold text-gruvbox-blue mb-1">Enable Cloud Backup</h4>
          <p className="text-xs text-gruvbox-light leading-relaxed">
            Sync your data securely to Exemem cloud storage. All data is encrypted on your device before uploading.
            Your local database remains your primary store.
          </p>
          <ul className="text-xs text-gruvbox-dim mt-2 space-y-1 list-disc list-inside">
            <li>1 GB free storage included</li>
            <li>End-to-end encrypted — the server never sees your data</li>
            <li>Sync across multiple devices</li>
            <li>Upgrade to 50 GB anytime ($5/mo)</li>
          </ul>
        </div>
      </div>

      {/* Invite code input */}
      <div>
        <label className="text-xs text-gruvbox-dim block mb-1">Invite Code</label>
        <input
          type="text"
          value={inviteCode}
          onChange={(e) => setInviteCode(e.target.value.toUpperCase())}
          placeholder="EXM-XXXX-XXXX"
          className="w-full px-3 py-2 text-sm font-mono tracking-wider border border-border rounded-md bg-surface text-gruvbox-bright placeholder-gruvbox-dim focus:outline-none focus:border-gruvbox-blue"
          maxLength={13}
          disabled={registering}
        />
        <p className="text-xs text-gruvbox-dim mt-1">Get an invite code from an existing Exemem user.</p>
      </div>

      {error && (
        <div className="flex items-start gap-3 p-3 border border-gruvbox-red bg-gruvbox-red/5 rounded-md">
          <span className="text-gruvbox-red text-sm flex-shrink-0">!</span>
          <p className="text-xs text-gruvbox-red leading-relaxed">{error}</p>
        </div>
      )}

      {/* Enable button */}
      <div className="flex justify-end gap-3 pt-4 border-t border-border">
        {onClose && (
          <button
            onClick={onClose}
            disabled={registering}
            className="px-4 py-2 text-xs border border-border text-gruvbox-dim hover:text-gruvbox-bright rounded-md transition-colors cursor-pointer"
          >
            Cancel
          </button>
        )}
        <button
          onClick={handleEnableCloud}
          disabled={registering || !inviteCode.trim()}
          className="px-6 py-2 text-xs font-bold border border-gruvbox-green text-surface bg-gruvbox-green hover:bg-gruvbox-green/90 rounded-md transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
        >
          {registering ? (
            <svg className="animate-spin h-3 w-3 text-surface" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
              <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4"></circle>
              <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
            </svg>
          ) : null}
          {registering ? 'Enabling...' : 'Enable Cloud Backup'}
        </button>
      </div>

    </div>
  )
}
