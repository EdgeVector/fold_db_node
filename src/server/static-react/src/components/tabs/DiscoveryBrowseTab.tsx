import { useCallback, useEffect, useState } from 'react'
import { discoveryClient } from '../../api/clients/discoveryClient'
import { toErrorMessage } from '../../utils/schemaUtils'

const PAGE_SIZE = 20

function MatchQualityBadge({ similarity }) {
  const pct = Math.round(similarity * 100)
  let color = 'text-gruvbox-red'
  if (pct >= 90) color = 'text-gruvbox-green'
  else if (pct >= 75) color = 'text-gruvbox-blue'
  else if (pct >= 60) color = 'text-gruvbox-yellow'

  return (
    <div className="flex items-center gap-1.5">
      <div className={`text-lg font-bold ${color}`}>{pct}%</div>
      <div className="text-xs text-tertiary">match</div>
    </div>
  )
}

function ProfileCard({ result, onConnect }) {
  const [showConnect, setShowConnect] = useState(false)
  const [message, setMessage] = useState('')
  const [sending, setSending] = useState(false)

  const handleSend = async () => {
    if (!message.trim()) return
    setSending(true)
    try {
      const res = await discoveryClient.connect(result.pseudonym, message)
      if (res.success) {
        setShowConnect(false)
        setMessage('')
        onConnect({ success: true })
      } else {
        onConnect({ error: res.error || 'Connect failed' })
      }
    } catch (e) {
      onConnect({ error: toErrorMessage(e) || 'Network error' })
    } finally {
      setSending(false)
    }
  }

  return (
    <div className="card rounded p-4 space-y-3">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-4">
          <MatchQualityBadge similarity={result.similarity} />
          <div>
            <span className="badge badge-info">{result.category}</span>
            {result.content_preview && (
              <p className="text-xs text-secondary mt-1 max-w-md truncate">
                {result.content_preview}
              </p>
            )}
          </div>
        </div>
        {showConnect ? (
          <div className="flex gap-1 items-center">
            <input
              type="text"
              value={message}
              onChange={(e) => setMessage(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleSend()}
              placeholder="Message..."
              className="input text-xs w-48"
              disabled={sending}
            />
            <button
              onClick={handleSend}
              disabled={!message.trim() || sending}
              className="btn-primary btn-sm"
            >
              {sending ? '...' : 'Send'}
            </button>
            <button
              onClick={() => { setShowConnect(false); setMessage('') }}
              className="btn-secondary btn-sm"
            >
              Cancel
            </button>
          </div>
        ) : (
          <button onClick={() => setShowConnect(true)} className="btn-primary btn-sm">
            Connect
          </button>
        )}
      </div>
      <div className="text-xs text-tertiary font-mono truncate">{result.pseudonym}</div>
    </div>
  )
}

function CategoryCard({ category, isSelected, onClick }) {
  return (
    <button
      onClick={onClick}
      className={`p-3 rounded border text-left transition-colors ${
        isSelected
          ? 'bg-surface-secondary border-gruvbox-blue text-primary'
          : 'bg-surface border-border text-secondary hover:border-gruvbox-blue/50'
      }`}
    >
      <div className="font-medium text-sm">{category.category}</div>
      <div className="text-xs text-tertiary mt-1">
        {category.user_count} user{category.user_count !== 1 ? 's' : ''}
      </div>
    </button>
  )
}

function EmptySearchState() {
  return (
    <div className="card p-8 text-center space-y-4 rounded">
      <div className="text-3xl text-gruvbox-yellow">&#128269;</div>
      <h3 className="text-lg text-primary font-semibold">Search the discovery network</h3>
      <p className="text-secondary text-sm max-w-md mx-auto">
        Enter a search query or select a category above to find people with similar
        interests. Results show anonymized profiles with match quality scores.
      </p>
    </div>
  )
}

function NoResultsState({ query }) {
  return (
    <div className="card p-8 text-center space-y-4 rounded">
      <div className="text-3xl text-gruvbox-yellow">&#128528;</div>
      <h3 className="text-lg text-primary font-semibold">No matches found</h3>
      <p className="text-secondary text-sm max-w-md mx-auto">
        No profiles matched &ldquo;{query}&rdquo; yet. As more people join the discovery
        network, matches will appear. Try broadening your search or selecting a different category.
      </p>
    </div>
  )
}

function NetworkUnavailableState() {
  return (
    <div className="card p-6 text-center rounded">
      <h3 className="text-lg text-primary mb-2">Discovery Not Available</h3>
      <p className="text-secondary text-sm">
        Discovery requires an Exemem cloud account. Enable cloud backup in
        Settings to join the discovery network and find users with similar data.
      </p>
    </div>
  )
}

export default function DiscoveryBrowseTab({ onResult }) {
  // Categories
  const [categories, setCategories] = useState([])
  const [categoriesLoading, setCategoriesLoading] = useState(true)
  const [selectedCategory, setSelectedCategory] = useState(null)
  const [serviceAvailable, setServiceAvailable] = useState(true)

  // Search
  const [query, setQuery] = useState('')
  const [results, setResults] = useState([])
  const [searching, setSearching] = useState(false)
  const [searchError, setSearchError] = useState(null)
  const [hasSearched, setHasSearched] = useState(false)

  // Pagination
  const [page, setPage] = useState(0)
  const [hasMore, setHasMore] = useState(false)

  // Load categories on mount
  const loadCategories = useCallback(async () => {
    setCategoriesLoading(true)
    try {
      const res = await discoveryClient.browseCategories()
      if (res.success) {
        setCategories(res.data?.categories || [])
        setServiceAvailable(true)
      } else if (res.status === 503) {
        setServiceAvailable(false)
      } else {
        setServiceAvailable(true)
        setCategories([])
      }
    } catch {
      setServiceAvailable(false)
    } finally {
      setCategoriesLoading(false)
    }
  }, [])

  useEffect(() => { loadCategories() }, [loadCategories])

  // Execute search
  const executeSearch = useCallback(async (searchQuery, categoryFilter, pageOffset) => {
    if (!searchQuery.trim()) return
    setSearching(true)
    setSearchError(null)
    try {
      const res = await discoveryClient.search(
        searchQuery,
        PAGE_SIZE,
        categoryFilter || undefined,
        pageOffset * PAGE_SIZE,
      )
      if (res.success) {
        const newResults = res.data?.results || []
        if (pageOffset === 0) {
          setResults(newResults)
        } else {
          setResults(prev => [...prev, ...newResults])
        }
        setHasMore(newResults.length === PAGE_SIZE)
        setHasSearched(true)
      } else {
        setSearchError(res.error || 'Search failed')
      }
    } catch (e) {
      setSearchError(toErrorMessage(e) || 'Network error')
    } finally {
      setSearching(false)
    }
  }, [])

  const handleSearch = () => {
    if (!query.trim()) return
    setPage(0)
    executeSearch(query, selectedCategory, 0)
  }

  const handleLoadMore = () => {
    const nextPage = page + 1
    setPage(nextPage)
    executeSearch(query, selectedCategory, nextPage)
  }

  const handleCategoryClick = (cat) => {
    const newCat = selectedCategory === cat.category ? null : cat.category
    setSelectedCategory(newCat)
    // If we have a query, re-search with new filter
    if (query.trim()) {
      setPage(0)
      executeSearch(query, newCat, 0)
    }
  }

  const handleConnect = (result) => {
    if (result.success) {
      onResult({ success: true, data: { message: 'Connection request sent' } })
    } else {
      onResult({ error: result.error })
    }
  }

  if (!serviceAvailable) {
    return <NetworkUnavailableState />
  }

  return (
    <div className="space-y-4">
      {/* Category Browser */}
      <div>
        <div className="flex items-center justify-between mb-2">
          <h3 className="text-sm font-semibold text-primary">Interest Categories</h3>
          {selectedCategory && (
            <button
              onClick={() => {
                setSelectedCategory(null)
                if (query.trim()) {
                  setPage(0)
                  executeSearch(query, null, 0)
                }
              }}
              className="text-xs text-gruvbox-blue hover:text-primary transition-colors"
            >
              Clear filter
            </button>
          )}
        </div>
        {categoriesLoading ? (
          <div className="text-sm text-secondary">Loading categories...</div>
        ) : categories.length === 0 ? (
          <div className="card-info p-3 rounded text-xs text-secondary">
            No categories on the network yet. Be the first to publish your interests.
          </div>
        ) : (
          <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-2">
            {categories.map(cat => (
              <CategoryCard
                key={cat.category}
                category={cat}
                isSelected={selectedCategory === cat.category}
                onClick={() => handleCategoryClick(cat)}
              />
            ))}
          </div>
        )}
      </div>

      {/* Search Bar */}
      <div className="flex gap-2">
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
          placeholder={
            selectedCategory
              ? `Search in "${selectedCategory}"...`
              : 'Search all interests...'
          }
          className="input flex-1"
        />
        <button
          onClick={handleSearch}
          disabled={searching || !query.trim()}
          className="btn-primary"
        >
          {searching ? 'Searching...' : 'Search'}
        </button>
      </div>

      {selectedCategory && (
        <div className="text-xs text-secondary">
          Filtering by: <span className="badge badge-info">{selectedCategory}</span>
        </div>
      )}

      {/* Error */}
      {searchError && <div className="text-sm text-gruvbox-red">{searchError}</div>}

      {/* Results */}
      {hasSearched && results.length > 0 && (
        <div className="space-y-3">
          <div className="text-xs text-secondary">
            {results.length} result{results.length !== 1 ? 's' : ''}
            {selectedCategory ? ` in "${selectedCategory}"` : ''}
          </div>

          {results.map((r, i) => (
            <ProfileCard key={`${r.pseudonym}-${i}`} result={r} onConnect={handleConnect} />
          ))}

          {hasMore && (
            <button
              onClick={handleLoadMore}
              disabled={searching}
              className="btn-secondary w-full"
            >
              {searching ? 'Loading...' : 'Load More'}
            </button>
          )}
        </div>
      )}

      {/* Empty States */}
      {hasSearched && results.length === 0 && !searching && !searchError && (
        <NoResultsState query={query} />
      )}

      {!hasSearched && !searching && <EmptySearchState />}
    </div>
  )
}
