import { useState, useCallback } from 'react'
import { browseRemoteNode, proxyQueryRemote } from '../../api/clients/trustClient'

function RemoteQueryTab({ onResult: _onResult }) {
  const [remoteUrl, setRemoteUrl] = useState('')
  const [nodeInfo, setNodeInfo] = useState(null)
  const [browsing, setBrowsing] = useState(false)
  const [selectedSchema, setSelectedSchema] = useState('')
  const [results, setResults] = useState(null)
  const [querying, setQuerying] = useState(false)
  const [error, setError] = useState(null)

  const handleBrowse = useCallback(async () => {
    if (!remoteUrl.trim()) return
    setBrowsing(true)
    setError(null)
    setNodeInfo(null)
    setSelectedSchema('')
    setResults(null)
    try {
      const resp = await browseRemoteNode(remoteUrl.trim())
      if (resp.success && resp.data) {
        setNodeInfo(resp.data)
      } else {
        setError(resp.error || 'Failed to connect to remote node')
      }
    } catch (err) {
      setError(err.message || 'Failed to connect')
    } finally {
      setBrowsing(false)
    }
  }, [remoteUrl])

  const handleQuery = useCallback(async () => {
    if (!remoteUrl.trim() || !selectedSchema) return
    setQuerying(true)
    setError(null)
    setResults(null)
    try {
      const resp = await proxyQueryRemote(remoteUrl.trim(), selectedSchema, [])
      if (resp.success && resp.data) {
        // The response might be nested
        const data = resp.data.data || resp.data
        const queryResults = data.results || data
        setResults(Array.isArray(queryResults) ? queryResults : [])
      } else {
        setError(resp.error || 'Query failed')
      }
    } catch (err) {
      setError(err.message || 'Query failed')
    } finally {
      setQuerying(false)
    }
  }, [remoteUrl, selectedSchema])

  const truncateKey = (key) => {
    if (!key) return ''
    if (key.length <= 20) return key
    return `${key.slice(0, 10)}...${key.slice(-10)}`
  }

  return (
    <div className="p-6">
      <div className="mb-6">
        <h2 className="text-lg font-medium text-primary">Remote Query</h2>
        <p className="text-sm text-secondary mt-1">
          Query data on another FoldDB node. Your request is signed with your node's key
          and the remote node controls what you can see based on your trust level.
        </p>
      </div>

      {/* Error */}
      {error && (
        <div className="card card-error mb-4 p-4">
          <p className="text-sm">{error}</p>
          <button className="text-xs underline mt-1" onClick={() => setError(null)}>Dismiss</button>
        </div>
      )}

      {/* Connect to remote node */}
      <div className="border border-border rounded-lg p-4 mb-4 bg-surface">
        <h3 className="text-sm font-medium text-primary mb-2">Connect to Node</h3>
        <div className="flex gap-2">
          <input
            className="input flex-1"
            type="text"
            placeholder="http://192.168.1.10:9001"
            value={remoteUrl}
            onChange={(e) => setRemoteUrl(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleBrowse()}
          />
          <button
            className="btn"
            onClick={handleBrowse}
            disabled={browsing || !remoteUrl.trim()}
          >
            {browsing ? 'Connecting...' : 'Connect'}
          </button>
        </div>
      </div>

      {/* Node info + schema selection */}
      {nodeInfo && (
        <div className="border border-border rounded-lg p-4 mb-4 bg-surface">
          <div className="flex items-center gap-2 mb-3">
            <span className="badge badge-success text-xs">connected</span>
            <code className="text-xs text-secondary" title={nodeInfo.public_key}>
              {truncateKey(nodeInfo.public_key)}
            </code>
          </div>

          <h3 className="text-sm font-medium text-primary mb-2">
            Available Schemas ({nodeInfo.schemas?.length || nodeInfo.shared_schemas?.length || 0})
          </h3>

          {nodeInfo.shared_schemas?.length === 0 && (
            <p className="text-xs text-tertiary">No schemas available on this node.</p>
          )}

          {(nodeInfo.schemas?.length || nodeInfo.shared_schemas?.length) > 0 && (
            <>
              <div className="space-y-1 max-h-48 overflow-y-auto mb-3">
                {(nodeInfo.schemas || (nodeInfo.shared_schemas ?? []).map(s => ({name: s}))).map((schema) => {
                  const name = typeof schema === 'string' ? schema : schema.name
                  const desc = typeof schema === 'object' ? schema.descriptive_name : null
                  const display = desc || (name.length > 40 ? name.slice(0, 40) + '...' : name)
                  return (
                    <button
                      key={name}
                      className={`w-full text-left px-3 py-2 text-xs rounded transition-colors ${
                        selectedSchema === name
                          ? 'bg-gruvbox-blue/20 text-gruvbox-blue border border-gruvbox-blue/30'
                          : 'bg-surface-secondary text-primary hover:bg-gruvbox-elevated'
                      }`}
                      onClick={() => setSelectedSchema(name)}
                    >
                      {display}
                      {desc && <span className="text-tertiary ml-1 text-[10px]">({name.slice(0, 8)}...)</span>}
                    </button>
                  )
                })}
              </div>

              <button
                className="btn"
                onClick={handleQuery}
                disabled={querying || !selectedSchema}
              >
                {querying ? 'Querying...' : 'Query Selected Schema'}
              </button>
            </>
          )}
        </div>
      )}

      {/* Results */}
      {results !== null && (
        <div className="border border-border rounded-lg p-4 bg-surface">
          <h3 className="text-sm font-medium text-primary mb-2">
            Results ({results.length} records)
          </h3>

          {results.length === 0 && (
            <p className="text-xs text-tertiary">
              No records returned. You may not have access to this schema's data,
              or the schema may be empty.
            </p>
          )}

          {results.length > 0 && (
            <div className="space-y-2 max-h-96 overflow-y-auto">
              {results.map((record, idx) => {
                const fields = record.fields || record
                const key = record.key
                return (
                  <div key={idx} className="p-3 bg-surface-secondary rounded text-xs">
                    {key && (
                      <div className="text-tertiary mb-1">
                        Key: {JSON.stringify(key)}
                      </div>
                    )}
                    {Object.entries(fields).map(([fname, fval]) => (
                      <div key={fname} className="flex gap-2">
                        <span className="text-secondary font-medium min-w-[100px]">{fname}:</span>
                        <span className="text-primary break-all">
                          {typeof fval === 'object' ? JSON.stringify(fval) : String(fval)}
                        </span>
                      </div>
                    ))}
                  </div>
                )
              })}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

export default RemoteQueryTab
