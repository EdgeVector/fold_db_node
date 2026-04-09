import React, { useState, useEffect, useRef } from 'react'
import { defaultApiClient } from '../../api/core/client'
import { systemClient } from '../../api/clients/systemClient'
import { orgClient } from '../../api/clients/orgClient'

function formatRelativeTime(epochMs) {
  if (!epochMs) return null
  const diff = Date.now() - epochMs
  if (diff < 60000) return 'just now'
  if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`
  if (diff < 86400000) return `${Math.floor(diff / 3600000)}h ago`
  return `${Math.floor(diff / 86400000)}d ago`
}

export default function OrgSettingsPanel() {
  const [orgs, setOrgs] = useState([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)
  const [successMsg, setSuccessMsg] = useState(null)
  const [nodePublicKey, setNodePublicKey] = useState(null)
  const [keyCopied, setKeyCopied] = useState(false)
  const [orgSyncStatuses, setOrgSyncStatuses] = useState({})

  const [newOrgName, setNewOrgName] = useState('')
  const [newMemberKey, setNewMemberKey] = useState('')
  const [newMemberName, setNewMemberName] = useState('')
  const [cloudMembers, setCloudMembers] = useState({})
  const [syncNotification, setSyncNotification] = useState(null)

  const syncPollRef = useRef(null)
  const retryRef = useRef(false)
  const lastDownloadCursorsRef = useRef({})

  useEffect(() => {
    const doFetch = () => {
      const hash = localStorage.getItem('fold_user_hash')
      if (!hash) {
        // Auth not ready yet, retry in 1 second
        const timer = setTimeout(doFetch, 1000)
        return () => clearTimeout(timer)
      }
      fetchOrgs()
    }
    doFetch()
    systemClient.getNodePublicKey().then(res => {
      if (res.data?.public_key) setNodePublicKey(res.data.public_key)
    }).catch(() => {})
  }, [])

  // Poll org sync statuses every 10 seconds and fetch cloud members
  useEffect(() => {
    if (orgs.length === 0) return
    const fetchAllSyncStatuses = () => {
      orgs.forEach(org => {
        defaultApiClient.get(`/sync/org/${org.org_hash}/status`)
          .then(res => {
            const data = res.data || res
            setOrgSyncStatuses(prev => ({ ...prev, [org.org_hash]: data }))
            // Detect new data synced by tracking download cursor changes
            const cursor = data.download_cursor || data.last_download_seq || 0
            const prevCursor = lastDownloadCursorsRef.current[org.org_hash]
            if (prevCursor !== undefined && cursor > prevCursor) {
              setSyncNotification(`New org data synced for "${org.org_name}"`)
              setTimeout(() => setSyncNotification(null), 5000)
            }
            lastDownloadCursorsRef.current[org.org_hash] = cursor
          })
          .catch(() => {})
      })
    }
    const fetchAllCloudMembers = () => {
      orgs.forEach(org => {
        orgClient.getCloudMembers(org.org_hash)
          .then(res => {
            const data = res.data || res
            const members = data.members || []
            setCloudMembers(prev => ({ ...prev, [org.org_hash]: members }))
          })
          .catch(() => {})
      })
    }
    fetchAllSyncStatuses()
    fetchAllCloudMembers()
    syncPollRef.current = setInterval(() => {
      fetchAllSyncStatuses()
      fetchAllCloudMembers()
    }, 10000)
    return () => { if (syncPollRef.current) clearInterval(syncPollRef.current) }
  }, [orgs])

  const handleCopyKey = async () => {
    if (nodePublicKey) {
      await navigator.clipboard.writeText(nodePublicKey)
      setKeyCopied(true)
      setTimeout(() => setKeyCopied(false), 2000)
    }
  }

  const showSuccess = (msg) => {
    setSuccessMsg(msg)
    setTimeout(() => setSuccessMsg(null), 3000)
  }

  const fetchOrgs = async () => {
    try {
      setLoading(true)
      const res = await defaultApiClient.get('/org', { cacheable: false })
      const data = res.data || res
      const orgList = data.orgs || []
      setOrgs(orgList)
      setError(null)
      // If we got 0 orgs and no error, auth might not be ready — retry once
      if (orgList.length === 0 && !retryRef.current) {
        retryRef.current = true
        setTimeout(fetchOrgs, 2000)
      }
    } catch (err) {
      setError(err.message || 'Failed to fetch organizations')
    } finally {
      setLoading(false)
    }
  }

  const handleDeleteOrg = async (orgHash, orgName) => {
    if (!window.confirm(`Delete organization "${orgName}"? This will remove all org data from your node.`)) return
    try {
      await orgClient.deleteOrg(orgHash)
      showSuccess(`Organization "${orgName}" deleted`)
      fetchOrgs()
    } catch (err) {
      setError(err.message || 'Failed to delete organization')
    }
  }

  const handleLeaveOrg = async (orgHash, orgName) => {
    if (!window.confirm(`Leave organization "${orgName}"? All org data will be removed from your node.`)) return
    try {
      await orgClient.leaveOrg(orgHash)
      showSuccess(`Left organization "${orgName}"`)
      fetchOrgs()
    } catch (err) {
      setError(err.message || 'Failed to leave organization')
    }
  }

  const handleCreateOrg = async (e) => {
    e.preventDefault()
    if (!newOrgName.trim()) return
    if (!window.confirm(`Create organization "${newOrgName.trim()}"?`)) return
    try {
      setError(null)
      const orgName = newOrgName
      const res = await defaultApiClient.post('/org', { name: orgName })
      if (res.success === false || res.error) {
        setError(res.error || 'Failed to create organization')
        return
      }
      setNewOrgName('')
      showSuccess(`Organization "${orgName}" created!`)
      fetchOrgs()
    } catch (err) {
      setError(err.message || 'Failed to create org')
    }
  }

  const handleAddMember = async (e, orgHash) => {
    e.preventDefault()
    if (!newMemberKey.trim() || !newMemberName.trim()) return
    try {
      const memberName = newMemberName
      await defaultApiClient.post(`/org/${orgHash}/members`, {
        node_public_key: newMemberKey,
        display_name: memberName
      })
      setNewMemberKey('')
      setNewMemberName('')
      showSuccess(`Invite sent to ${memberName}!`)
      fetchOrgs()
    } catch (err) {
      setError(err.message || 'Failed to add member')
    }
  }

  const handleRemoveMember = async (orgHash, nodePublicKey) => {
    try {
      await defaultApiClient.delete(`/org/${orgHash}/members/${encodeURIComponent(nodePublicKey)}`)
      fetchOrgs()
    } catch (err) {
      setError(err.message || 'Failed to remove member')
    }
  }

  if (loading) return <div className="p-4 text-text-muted">Loading organizations...</div>

  return (
    <div className="p-4 flex flex-col gap-6">
      {/* Your Node Public Key */}
      <div className="flex flex-col gap-2 p-4 border border-primary/30 rounded-md bg-primary/5">
        <h3 className="text-sm font-semibold text-text-primary">Your Node Public Key</h3>
        <p className="text-xs text-text-muted">Share this key with an org admin to get invited to their organization.</p>
        <div className="flex items-center gap-2">
          <code className="flex-1 text-xs font-mono text-text-primary bg-bg-surface border border-border rounded px-3 py-2 break-all select-all">
            {nodePublicKey || 'Loading...'}
          </code>
          <button
            onClick={handleCopyKey}
            disabled={!nodePublicKey}
            className="btn-primary whitespace-nowrap text-xs px-4"
          >
            {keyCopied ? 'Copied!' : 'Copy Key'}
          </button>
          {typeof navigator !== 'undefined' && navigator.share && (
            <button
              onClick={() => navigator.share({
                title: 'My FoldDB Public Key',
                text: nodePublicKey
              }).catch(() => {})}
              disabled={!nodePublicKey}
              className="btn-secondary whitespace-nowrap text-xs"
            >
              Share
            </button>
          )}
        </div>
      </div>

      <div className="flex flex-col gap-2">
        <h3 className="text-lg font-medium text-text-primary">Organizations</h3>
        <p className="text-sm text-text-muted">
          Manage your data-sharing organizations and memberships.
        </p>
      </div>

      {successMsg && (
        <div className="p-3 bg-green-900/30 border border-green-500/50 text-green-400 rounded-md text-sm">
          {successMsg}
        </div>
      )}

      {error && (
        <div className="p-3 bg-red-900/30 border border-red-500/50 text-red-400 rounded-md text-sm">
          {error}
        </div>
      )}

      {syncNotification && (
        <div className="p-3 bg-blue-900/30 border border-blue-500/50 text-blue-400 rounded-md text-sm animate-pulse">
          {syncNotification}
        </div>
      )}

      {orgs.length === 0 ? (
        <div className="p-6 border border-border border-dashed rounded-md flex flex-col items-center justify-center gap-4 text-center">
          <p className="text-text-muted">You are not a member of any organization.</p>
        </div>
      ) : (
        <div className="flex flex-col gap-4">
          {orgs.map(org => (
            <div key={org.org_hash} className="border border-border rounded-md overflow-hidden bg-bg-surface">
              <div className="p-3 border-b border-border bg-bg-surface-elevated flex justify-between items-center">
                <div>
                  <h4 className="font-medium text-text-primary">{org.org_name}</h4>
                  <p className="text-xs text-text-muted font-mono">{org.org_hash.substring(0, 16)}...</p>
                </div>
                <div className="flex items-center gap-2">
                  {(() => {
                    const syncStatus = orgSyncStatuses[org.org_hash]
                    if (!syncStatus) return null
                    const lastSyncRelative = formatRelativeTime(syncStatus.last_sync_at)
                    if (syncStatus.last_error) {
                      return (
                        <div className="flex flex-col items-end">
                          <span className="px-2 py-0.5 text-xs rounded-full bg-red-900/30 text-red-400 border border-red-500/50">
                            Error
                          </span>
                          <span className="text-xs text-red-400 truncate max-w-[200px]" title={syncStatus.last_error}>
                            {syncStatus.last_error}
                          </span>
                        </div>
                      )
                    }
                    if (syncStatus.pending_count > 0) {
                      return (
                        <div className="flex flex-col items-end">
                          <span className="px-2 py-0.5 text-xs rounded-full bg-yellow-900/30 text-yellow-400 border border-yellow-500/50">
                            Syncing ({syncStatus.pending_count} pending)
                          </span>
                          {lastSyncRelative && <span className="text-xs text-text-muted">Last synced {lastSyncRelative}</span>}
                        </div>
                      )
                    }
                    return (
                      <div className="flex flex-col items-end">
                        <span className="px-2 py-0.5 text-xs rounded-full bg-green-900/30 text-green-400 border border-green-500/50">
                          Synced
                        </span>
                        {lastSyncRelative && <span className="text-xs text-text-muted">Last synced {lastSyncRelative}</span>}
                      </div>
                    )
                  })()}
                  <div className="px-2 py-1 bg-primary/20 text-primary text-xs rounded uppercase font-semibold">
                    {org.role}
                  </div>
                  {org.role === 'Admin' ? (
                    <button
                      onClick={() => handleDeleteOrg(org.org_hash, org.org_name)}
                      className="text-red-400 hover:text-red-300 text-xs px-2 py-1 bg-red-400/10 hover:bg-red-400/20 rounded transition-colors"
                    >
                      Delete
                    </button>
                  ) : (
                    <button
                      onClick={() => handleLeaveOrg(org.org_hash, org.org_name)}
                      className="text-red-400 hover:text-red-300 text-xs px-2 py-1 bg-red-400/10 hover:bg-red-400/20 rounded transition-colors"
                    >
                      Leave
                    </button>
                  )}
                </div>
              </div>
              
              <div className="p-3">
                <h5 className="text-sm font-medium mb-2 text-text-primary">
                  Members ({org.members?.length || 0})
                  {cloudMembers[org.org_hash] && cloudMembers[org.org_hash].length > (org.members?.length || 0) && (
                    <span className="ml-2 text-xs text-text-muted font-normal">
                      ({cloudMembers[org.org_hash].length} in cloud)
                    </span>
                  )}
                </h5>
                <ul className="flex flex-col gap-2 mb-4">
                  {org.members?.map(m => (
                    <li key={m.node_public_key} className="flex justify-between items-center text-sm p-2 bg-bg-surface border border-border/50 rounded">
                      <div>
                        <span className="font-medium">{m.display_name}</span>
                        <span className="text-xs text-text-muted font-mono ml-2">{m.node_public_key.substring(0, 10)}...</span>
                      </div>
                      {org.role === 'Admin' && (
                        <button
                          onClick={() => handleRemoveMember(org.org_hash, m.node_public_key)}
                          className="text-red-400 hover:text-red-300 text-xs px-2 py-1 bg-red-400/10 hover:bg-red-400/20 rounded transition-colors"
                        >
                          Remove
                        </button>
                      )}
                    </li>
                  ))}
                </ul>

                {/* Cloud members not in local list */}
                {cloudMembers[org.org_hash] && (() => {
                  const localKeys = new Set((org.members || []).map(m => m.node_public_key))
                  // Cloud members have user_hash, not node_public_key — show those not matched locally
                  const cloudOnly = cloudMembers[org.org_hash].filter(cm =>
                    !Array.from(localKeys).some(k => k === cm.user_hash) && cm.status === 'active'
                  )
                  if (cloudOnly.length === 0) return null
                  return (
                    <div className="mb-4">
                      <h6 className="text-xs font-medium text-text-muted mb-1">Cloud-only members</h6>
                      <ul className="flex flex-col gap-1">
                        {cloudOnly.map(cm => (
                          <li key={cm.user_hash} className="flex justify-between items-center text-sm p-2 bg-bg-surface border border-border/30 rounded opacity-75">
                            <div className="flex items-center">
                              <span className="font-medium text-text-primary">Cloud Member</span>
                              <span className="text-xs text-text-muted font-mono ml-2">{cm.user_hash.substring(0, 12)}...</span>
                              <span className="ml-2 text-xs px-1.5 py-0.5 rounded bg-primary/10 text-primary">{cm.role}</span>
                            </div>
                          </li>
                        ))}
                      </ul>
                    </div>
                  )
                })()}

                {org.role === 'Admin' && (
                  <form onSubmit={(e) => handleAddMember(e, org.org_hash)} className="flex gap-2 items-end mt-4 pt-4 border-t border-border/50">
                    <div className="flex-1 flex flex-col gap-1">
                      <label className="text-xs text-text-muted">Member Name</label>
                      <input 
                        type="text" 
                        value={newMemberName}
                        onChange={e => setNewMemberName(e.target.value)}
                        placeholder="Alice"
                        className="input-field text-sm"
                        required
                      />
                    </div>
                    <div className="flex-[2] flex flex-col gap-1">
                      <label className="text-xs text-text-muted">Public Key (User Hash)</label>
                      <input 
                        type="text" 
                        value={newMemberKey}
                        onChange={e => setNewMemberKey(e.target.value)}
                        placeholder="Base64 Public Key"
                        className="input-field text-sm font-mono"
                        required
                      />
                    </div>
                    <button type="submit" className="btn-secondary whitespace-nowrap mb-[1px]">
                      Add Member
                    </button>
                  </form>
                )}
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="mt-4 pt-4 border-t border-border">
        <h4 className="text-sm font-medium mb-3 text-text-primary">Create New Organization</h4>
        <form onSubmit={handleCreateOrg} className="flex gap-2">
          <input 
            type="text" 
            value={newOrgName}
            onChange={e => setNewOrgName(e.target.value)}
            placeholder="Organization Name"
            className="input-field flex-1"
            required
          />
          <button type="submit" className="btn-primary">
            Create
          </button>
        </form>
      </div>
    </div>
  )
}
