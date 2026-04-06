import { useState, useEffect, useCallback } from 'react'
import { getConflicts, resolveConflict } from '../../api/clients/conflictClient'

function ConflictsTab() {
  const [conflicts, setConflicts] = useState([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(null)
  const [resolving, setResolving] = useState(null)
  const [filterMolecule, setFilterMolecule] = useState('')

  const fetchConflicts = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const response = await getConflicts(filterMolecule || undefined)
      setConflicts(response.data?.conflicts || [])
    } catch (err) {
      setError(err.message || 'Failed to load conflicts')
    } finally {
      setLoading(false)
    }
  }, [filterMolecule])

  useEffect(() => {
    fetchConflicts()
  }, [fetchConflicts])

  const handleResolve = async (conflictId) => {
    setResolving(conflictId)
    try {
      await resolveConflict(conflictId)
      setConflicts(prev => prev.filter(c => c.id !== conflictId))
    } catch (err) {
      setError(err.message || 'Failed to resolve conflict')
    } finally {
      setResolving(null)
    }
  }

  const handleResolveAll = async () => {
    for (const conflict of conflicts) {
      await handleResolve(conflict.id)
    }
  }

  const formatTimestamp = (isoString) => {
    try {
      const date = new Date(isoString)
      return date.toLocaleString()
    } catch {
      return isoString
    }
  }

  const truncateUuid = (uuid) => {
    if (!uuid) return ''
    if (uuid.length <= 16) return uuid
    return `${uuid.slice(0, 8)}...${uuid.slice(-8)}`
  }

  return (
    <div className="p-6 max-w-4xl mx-auto">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h2 className="text-lg font-medium text-primary">Sync Conflicts</h2>
          <p className="text-sm text-secondary mt-1">
            Conflicts detected when merging data from other devices or org members.
            The winning value is already applied — review and acknowledge.
          </p>
        </div>
        <button
          className="btn btn-sm"
          onClick={fetchConflicts}
          disabled={loading}
        >
          Refresh
        </button>
      </div>

      {/* Filter */}
      <div className="mb-4">
        <input
          className="input w-full"
          type="text"
          placeholder="Filter by molecule UUID..."
          value={filterMolecule}
          onChange={(e) => setFilterMolecule(e.target.value)}
        />
      </div>

      {/* Loading */}
      {loading && (
        <div className="text-center py-12">
          <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin mx-auto mb-3" />
          <p className="text-secondary text-sm">Loading conflicts...</p>
        </div>
      )}

      {/* Error */}
      {error && (
        <div className="card card-error mb-4">
          <p className="text-sm">{error}</p>
        </div>
      )}

      {/* Empty state */}
      {!loading && !error && conflicts.length === 0 && (
        <div className="text-center py-12 border border-border rounded-lg">
          <p className="text-gruvbox-green text-lg mb-2">No conflicts</p>
          <p className="text-secondary text-sm">
            All sync merges resolved cleanly. No action needed.
          </p>
        </div>
      )}

      {/* Conflict list */}
      {!loading && conflicts.length > 0 && (
        <>
          <div className="flex items-center justify-between mb-3">
            <p className="text-sm text-secondary">
              {conflicts.length} unresolved conflict{conflicts.length !== 1 ? 's' : ''}
            </p>
            <button
              className="btn btn-sm"
              onClick={handleResolveAll}
              disabled={resolving}
            >
              Acknowledge All
            </button>
          </div>

          <div className="space-y-3">
            {conflicts.map((conflict) => (
              <div
                key={conflict.id}
                className="border border-border rounded-lg p-4 bg-surface"
              >
                <div className="flex items-start justify-between gap-4">
                  <div className="flex-1 min-w-0">
                    {/* Molecule + key */}
                    <div className="flex items-center gap-2 mb-2">
                      <span className="badge badge-warning text-xs">conflict</span>
                      <code className="text-xs text-secondary" title={conflict.molecule_uuid}>
                        {truncateUuid(conflict.molecule_uuid)}
                      </code>
                      {conflict.conflict_key !== 'single' && (
                        <span className="text-xs text-tertiary">
                          key: <code>{conflict.conflict_key}</code>
                        </span>
                      )}
                    </div>

                    {/* Winner vs Loser */}
                    <div className="grid grid-cols-2 gap-3 text-xs">
                      <div className="p-2 rounded bg-surface-secondary border border-gruvbox-green/30">
                        <p className="text-gruvbox-green font-medium mb-1">Winner (applied)</p>
                        <code className="text-primary break-all" title={conflict.winner_atom}>
                          {truncateUuid(conflict.winner_atom)}
                        </code>
                      </div>
                      <div className="p-2 rounded bg-surface-secondary border border-gruvbox-red/30">
                        <p className="text-gruvbox-red font-medium mb-1">Loser (overridden)</p>
                        <code className="text-primary break-all" title={conflict.loser_atom}>
                          {truncateUuid(conflict.loser_atom)}
                        </code>
                      </div>
                    </div>

                    {/* Timestamp */}
                    <p className="text-xs text-tertiary mt-2">
                      Detected: {formatTimestamp(conflict.detected_at)}
                    </p>
                  </div>

                  {/* Resolve button */}
                  <button
                    className="btn btn-sm flex-shrink-0"
                    onClick={() => handleResolve(conflict.id)}
                    disabled={resolving === conflict.id}
                  >
                    {resolving === conflict.id ? 'Resolving...' : 'Acknowledge'}
                  </button>
                </div>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  )
}

export default ConflictsTab
