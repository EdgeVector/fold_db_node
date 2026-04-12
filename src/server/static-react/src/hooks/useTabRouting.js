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
  'word-graph': 'word-graph',
  discovery: 'discovery',
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

  // Sync activeTab with URL hash changes
  useEffect(() => {
    const handleHashChange = () => {
      const tab = resolveTabFromHash()
      if (tab && tab !== activeTab) {
        setActiveTab(tab)
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
