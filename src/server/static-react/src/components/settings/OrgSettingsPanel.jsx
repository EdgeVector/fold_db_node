import React, { useState, useEffect } from 'react'
import { defaultApiClient } from '../../api/core/client'

export default function OrgSettingsPanel() {
  const [orgs, setOrgs] = useState([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)
  
  const [newOrgName, setNewOrgName] = useState('')
  const [newMemberKey, setNewMemberKey] = useState('')
  const [newMemberName, setNewMemberName] = useState('')

  useEffect(() => {
    fetchOrgs()
  }, [])

  const fetchOrgs = async () => {
    try {
      setLoading(true)
      const res = await defaultApiClient.get('/org')
      const data = res.data || res
      setOrgs(data.orgs || [])
      setError(null)
    } catch (err) {
      setError(err.message || 'Failed to fetch organizations')
    } finally {
      setLoading(false)
    }
  }

  const handleCreateOrg = async (e) => {
    e.preventDefault()
    if (!newOrgName.trim()) return
    try {
      await defaultApiClient.post('/org', { name: newOrgName })
      setNewOrgName('')
      fetchOrgs()
    } catch (err) {
      setError(err.message || 'Failed to create org')
    }
  }

  const handleAddMember = async (e, orgHash) => {
    e.preventDefault()
    if (!newMemberKey.trim() || !newMemberName.trim()) return
    try {
      await defaultApiClient.post(`/org/${orgHash}/members`, {
        node_public_key: newMemberKey,
        display_name: newMemberName
      })
      setNewMemberKey('')
      setNewMemberName('')
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
      <div className="flex flex-col gap-2">
        <h3 className="text-lg font-medium text-text-primary">Organizations</h3>
        <p className="text-sm text-text-muted">
          Manage your data-sharing organizations and memberships.
        </p>
      </div>

      {error && (
        <div className="p-3 bg-red-900/30 border border-red-500/50 text-red-400 rounded-md text-sm">
          {error}
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
                <div className="px-2 py-1 bg-primary/20 text-primary text-xs rounded uppercase font-semibold">
                  {org.role}
                </div>
              </div>
              
              <div className="p-3">
                <h5 className="text-sm font-medium mb-2 text-text-primary">Members ({org.members?.length || 0})</h5>
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
