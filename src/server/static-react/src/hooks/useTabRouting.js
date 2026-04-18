import { useEffect, useState } from 'react'
import { DEFAULT_TAB } from '../constants'

// Single lookup for URL hash → tab ID (prevents duplication)
const HASH_TO_TAB = {
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

function resolveTabFromHash() {
  if (typeof window !== 'undefined' && window.location.hash) {
    return HASH_TO_TAB[window.location.hash.slice(1)] || null
  }
  return null
}

/**
 * Tab routing hook:
 *   - Owns activeTab + settingsSubTab state
 *   - Two-way syncs activeTab with window.location.hash (deep linking)
 *   - Exposes handleTabChange (also clears settingsSubTab when leaving settings)
 *   - Exposes navigateToSettings(subTab) convenience
 */
export function useTabRouting() {
  const [activeTab, setActiveTab] = useState(
    () => resolveTabFromHash() || DEFAULT_TAB
  )
  const [settingsSubTab, setSettingsSubTab] = useState(null)

  // Sync activeTab with URL hash changes. Unknown hashes get rewritten to
  // match the current activeTab so the address bar can't linger out of sync
  // with rendered content (e.g. someone bookmarks `/#browse` before it became
  // a real alias, or types a typo — URL normalizes instead of silently lying).
  useEffect(() => {
    const handleHashChange = () => {
      if (typeof window === 'undefined') return
      const raw = window.location.hash.slice(1)
      const tab = raw ? HASH_TO_TAB[raw] || null : null
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

  const handleTabChange = (tab) => {
    setActiveTab(tab)
    // Clear settings sub-tab when navigating away from settings
    if (tab !== 'settings') setSettingsSubTab(null)
    // Update URL hash to match active tab
    if (typeof window !== 'undefined') {
      window.location.hash = tab
    }
  }

  const navigateToSettings = (subTab) => {
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
