import { useCallback, useState } from 'react'
import { discoveryClient } from '../../../api/clients/discoveryClient'
import { toErrorMessage } from '../../../utils/schemaUtils'
import RoleSelect from './RoleSelect'

export default function SearchPanel({ onResult }) {
  const [query, setQuery] = useState('')
  const [results, setResults] = useState([])
  const [searching, setSearching] = useState(false)
  const [error, setError] = useState(null)
  const [connectingTo, setConnectingTo] = useState(null)
  const [connectMessage, setConnectMessage] = useState('')
  const [connectRole, setConnectRole] = useState('acquaintance')

  const handleSearch = useCallback(async () => {
    if (!query.trim()) return
    setSearching(true)
    setError(null)
    try {
      const res = await discoveryClient.search(query)
      if (res.success) {
        setResults(res.data?.results || [])
        onResult({ success: true, data: res.data })
      } else {
        setError(res.error || 'Search failed')
        onResult({ error: res.error || 'Search failed' })
      }
    } catch (e) {
      const msg = toErrorMessage(e)
      setError(msg || 'Network error')
      onResult({ error: msg || 'Network error' })
    } finally {
      setSearching(false)
    }
  }, [query, onResult])

  const handleConnect = async (pseudonym) => {
    if (!connectMessage.trim()) return
    try {
      const res = await discoveryClient.connect(pseudonym, connectMessage, connectRole !== 'acquaintance' ? connectRole : undefined)
      if (res.success) {
        setConnectingTo(null)
        setConnectMessage('')
        setConnectRole('acquaintance')
        onResult({ success: true, data: { message: 'Connection request sent' } })
      } else {
        onResult({ error: res.error || 'Connect failed' })
      }
    } catch (e) {
      onResult({ error: toErrorMessage(e) || 'Network error' })
    }
  }

  return (
    <div className="space-y-3">
      <div className="flex gap-2">
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
          placeholder="Search the discovery network..."
          className="input flex-1"
        />
        <button onClick={handleSearch} disabled={searching || !query.trim()} className="btn-primary">
          {searching ? 'Searching...' : 'Search Network'}
        </button>
      </div>

      {error && <div className="text-sm text-gruvbox-red">{error}</div>}

      {results.length > 0 && (
        <div className="space-y-2">
          <div className="text-xs text-secondary">{results.length} results</div>
          {results.map((r, i) => (
            <div key={i} className="border border-border rounded p-3">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <span className="badge badge-info">{r.category}</span>
                  <span className="text-xs text-secondary">
                    similarity: {(r.similarity * 100).toFixed(1)}%
                  </span>
                </div>
                {connectingTo === r.pseudonym ? (
                  <div className="flex gap-1 items-center">
                    <RoleSelect value={connectRole} onChange={setConnectRole} />
                    <input
                      type="text"
                      value={connectMessage}
                      onChange={(e) => setConnectMessage(e.target.value)}
                      placeholder="Message..."
                      className="input text-xs w-48"
                    />
                    <button
                      onClick={() => handleConnect(r.pseudonym)}
                      disabled={!connectMessage.trim()}
                      className="btn-primary btn-sm"
                    >
                      Send
                    </button>
                    <button
                      onClick={() => { setConnectingTo(null); setConnectMessage(''); setConnectRole('acquaintance') }}
                      className="btn-secondary btn-sm"
                    >
                      Cancel
                    </button>
                  </div>
                ) : (
                  <button
                    onClick={() => setConnectingTo(r.pseudonym)}
                    className="btn-secondary btn-sm"
                  >
                    Connect
                  </button>
                )}
              </div>
              {r.content_preview && (
                <p className="text-xs text-secondary mt-1">{r.content_preview}</p>
              )}
              <div className="text-xs text-tertiary mt-1 font-mono truncate">
                {r.pseudonym}
              </div>
            </div>
          ))}
        </div>
      )}

      {results.length === 0 && !searching && !error && query && (
        <div className="text-sm text-secondary text-center py-4">No results found</div>
      )}
    </div>
  )
}
