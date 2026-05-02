import { useEffect, useState } from 'react'
import { DEFAULT_TAB } from '../constants'

// Single lookup for URL hash → tab ID (prevents duplication)
const HASH_TO_TAB: Record<string, string> = {
  agent: 'agent',
  schemas: 'schemas',
  schema: 'schemas',
  query: 'query',
  mutation: 'mutation',
  ingestion: 'ingestion',
  'json-ingestion': 'ingestion',
  'file-upload': 'file-upload',
  'native-index': 'native-index',
  search: 'native-index',
  'llm-query': 'llm-query',
  'ai-query': 'llm-query',
  'smart-folder': 'smart-folder',
  import: 'smart-folder',
  'data-browser': 'data-browser',
  browser: 'data-browser',
  browse: 'data-browser',
  'word-graph': 'word-graph',
  discovery: 'discovery',
  discover: 'discovery',
  'discovery-browse': 'discovery-browse',
  views: 'views',
  people: 'people',
  sharing: 'people',
  trust: 'people',
  'trust-graph': 'people',
  feed: 'people',
  'shared-moments': 'people',
  'my-profile': 'people',
  profile: 'people',
  'apple-import': 'apple-import',
  conflicts: 'conflicts',
  'remote-query': 'remote-query',
  settings: 'settings',
}

// Strip an optional "?params" suffix from a hash so deep links like
// "#query?schema=Foo" resolve to the "query" tab. The full raw hash
// is exposed separately via location.hash for tabs that want to read
// their own params (e.g. QueryTab pre-selecting a schema).
function parseHashTab(hashWithoutPrefix: string): string | null {
  if (!hashWithoutPrefix) return null
  const sep = hashWithoutPrefix.indexOf('?')
  const id = sep === -1 ? hashWithoutPrefix : hashWithoutPrefix.slice(0, sep)
  return HASH_TO_TAB[id] || null
}

function resolveTabFromHash(): string | null {
  if (typeof window !== 'undefined' && window.location.hash) {
    return parseHashTab(window.location.hash.slice(1))
  }
  return null
}

interface TabRoutingResult {
  activeTab: string
  settingsSubTab: string | null
  handleTabChange: (tab: string) => void
  navigateToSettings: (subTab?: string | null) => void
}

/**
 * Tab routing hook:
 *   - Owns activeTab + settingsSubTab state
 *   - Two-way syncs activeTab with window.location.hash (deep linking)
 *   - Exposes handleTabChange (also clears settingsSubTab when leaving settings)
 *   - Exposes navigateToSettings(subTab) convenience
 */
export function useTabRouting(): TabRoutingResult {
  const [activeTab, setActiveTab] = useState<string>(
    () => resolveTabFromHash() || DEFAULT_TAB
  )
  const [settingsSubTab, setSettingsSubTab] = useState<string | null>(null)

  // Sync activeTab with URL hash changes. Unknown hashes get rewritten to
  // match the current activeTab so the address bar can't linger out of sync
  // with rendered content (e.g. someone bookmarks `/#browse` before it became
  // a real alias, or types a typo — URL normalizes instead of silently lying).
  useEffect(() => {
    const handleHashChange = () => {
      if (typeof window === 'undefined') return
      const raw = window.location.hash.slice(1)
      const tab = parseHashTab(raw)
      if (tab && tab !== activeTab) {
        setActiveTab(tab)
      } else if (raw && !tab && window.location.hash !== `#${activeTab}`) {
        // unknown hash — normalize URL to match current state
        window.history.replaceState(null, '', `#${activeTab}`)
      }
    }

    window.addEventListener('hashchange', handleHashChange)
    handleHashChange()
    return () => window.removeEventListener('hashchange', handleHashChange)
  }, [activeTab])

  const handleTabChange = (tab: string): void => {
    setActiveTab(tab)
    // Clear settings sub-tab when navigating away from settings
    if (tab !== 'settings') setSettingsSubTab(null)
    // Update URL hash to match active tab
    if (typeof window !== 'undefined') {
      window.location.hash = tab
    }
  }

  const navigateToSettings = (subTab?: string | null): void => {
    setSettingsSubTab(subTab || null)
    handleTabChange('settings')
  }

  return {
    activeTab,
    settingsSubTab,
    handleTabChange,
    navigateToSettings,
  }
}
