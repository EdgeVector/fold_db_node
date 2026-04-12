import React, { useState, useEffect, useRef } from 'react'
import { FoldDbProvider } from './components/FoldDbProvider'
import Header from './components/Header'
import Footer from './components/Footer'
import UpdateBanner from './components/UpdateBanner'
import ResultsSection from './components/ResultsSection'
import IngestionReport from './components/IngestionReport'
import Sidebar from './components/Sidebar'
import PeopleTab from './components/tabs/PeopleTab'
import SchemaTab from './components/tabs/SchemaTab'
import QueryTab from './components/tabs/QueryTab'
import LlmQueryTab from './components/tabs/LlmQueryTab'
import MutationTab from './components/tabs/MutationTab'
import IngestionTab from './components/tabs/IngestionTab'
import FileUploadTab from './components/tabs/FileUploadTab'
import NativeIndexTab from './components/tabs/NativeIndexTab'
import SmartFolderTab from './components/tabs/SmartFolderTab'
import DataBrowserTab from './components/tabs/DataBrowserTab'
import WordGraphTab from './components/tabs/WordGraphTab'
import AgentTab from './components/tabs/AgentTab'
import DiscoveryTab from './components/tabs/DiscoveryTab'
import DiscoveryBrowseTab from './components/tabs/DiscoveryBrowseTab'
import ViewsTab from './components/tabs/ViewsTab'
import SharingTab from './components/tabs/SharingTab'
import FeedTab from './components/tabs/FeedTab'
import SharedMomentsTab from './components/tabs/SharedMomentsTab'
import AppleImportTab from './components/tabs/AppleImportTab'
import MyProfileTab from './components/tabs/MyProfileTab'
import ConflictsTab from './components/tabs/ConflictsTab'
import TrustTab from './components/tabs/TrustTab'
import RemoteQueryTab from './components/tabs/RemoteQueryTab'
import SettingsTab from './components/tabs/SettingsTab'
import OnboardingWizard, { ONBOARDING_STORAGE_KEY } from './components/onboarding/OnboardingWizard'

import LogSidebar from './components/LogSidebar'
import ErrorBoundary from './components/ErrorBoundary'
import { useApprovedSchemas } from './hooks/useApprovedSchemas.js'
import { useAppSelector, useAppDispatch } from './store/hooks'
import { restoreSession, autoLogin, loadSystemPublicKey } from './store/authSlice'
import { fetchIngestionConfig, selectIngestionConfig, selectIsAiConfigured, selectAiProvider } from './store/ingestionSlice'
import { DEFAULT_TAB } from './constants'
import { BROWSER_CONFIG } from './constants/config'
import { getDatabaseStatus } from './api/clients/systemClient'
import { getIdentityCard } from './api/clients/trustClient'
import DatabaseSetupScreen from './components/DatabaseSetupScreen'

function isIngestionResult(results) {
  if (!results?.success) return false
  const d = results?.data ?? results
  return typeof d === 'object' && Array.isArray(d?.schemas_written) && d.schemas_written.length > 0
}

// Single lookup for URL hash → tab ID (prevents duplication)
const HASH_TO_TAB = {
  agent: 'agent',
  schemas: 'schemas', schema: 'schemas',
  query: 'query', mutation: 'mutation',
  ingestion: 'ingestion', 'json-ingestion': 'ingestion',
  'file-upload': 'file-upload',
  'native-index': 'native-index', search: 'native-index',
  'llm-query': 'llm-query', 'ai-query': 'llm-query',
  'smart-folder': 'smart-folder', import: 'smart-folder',
  'data-browser': 'data-browser', browser: 'data-browser',
  'word-graph': 'word-graph',
  discovery: 'discovery',
  'discovery-browse': 'discovery-browse',
  views: 'views',
  people: 'people',
  sharing: 'people',
  trust: 'people', 'trust-graph': 'people',
  feed: 'people',
  'shared-moments': 'people',
  'my-profile': 'people', profile: 'people',
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

export function AppContent() {
  const [activeTab, setActiveTab] = useState(() => resolveTabFromHash() || DEFAULT_TAB)
  const [settingsSubTab, setSettingsSubTab] = useState(null)
  const [results, setResults] = useState(null)
  const [setupDismissed, setSetupDismissed] = useState(
    () => localStorage.getItem('folddb_setup_dismissed') === '1'
  )
  const [dbStatus, setDbStatus] = useState(null) // { initialized, has_saved_config }
  const [dbStatusLoading, setDbStatusLoading] = useState(true)
  const [showOnboarding, setShowOnboarding] = useState(false)

  // Clear results whenever the active tab changes (covers all switch paths)
  useEffect(() => {
    setResults(null)
  }, [activeTab])

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

  // Redux state and dispatch
  const dispatch = useAppDispatch()
  const { isAuthenticated, isLoading: isAuthLoading } = useAppSelector(state => state.auth)

  // Restore session on mount FIRST - this must run before other effects.
  // Always auto-login with node identity (public key is the sole identity source).
  useEffect(() => {
    const userId = localStorage.getItem(BROWSER_CONFIG.STORAGE_KEYS.USER_ID)
    const userHash = localStorage.getItem(BROWSER_CONFIG.STORAGE_KEYS.USER_HASH)
    if (userId && userHash) {
      dispatch(restoreSession({ id: userId, hash: userHash }))
      return
    }

    // No stored credentials — auto-login with node's public key identity
    dispatch(autoLogin())
  }, [dispatch])

  // Load the system public key for display in Key Management tab
  useEffect(() => {
    dispatch(loadSystemPublicKey())
  }, [dispatch])

  // Check database status after authenticated
  useEffect(() => {
    if (!isAuthenticated) return
    setDbStatusLoading(true)
    getDatabaseStatus()
      .then(response => {
        if (response.success && response.data) {
          setDbStatus(response.data)
          // Show onboarding if neither backend nor localStorage says it's complete.
          // Backend marker file is authoritative (--empty-db wipes it to reset),
          // but localStorage is the fallback for cases where the backend marker
          // file wasn't written (API call failed, data dir cleaned without reset).
          if (!response.data.onboarding_complete
              && localStorage.getItem('folddb_onboarding_complete') !== '1') {
            // Final short-circuit: if the node already has an identity card
            // with a display_name, treat setup as complete. This prevents the
            // wizard from re-appearing on reload when the backend marker file
            // is missing but the user has already configured their identity.
            getIdentityCard()
              .then(res => {
                const hasIdentity = !!res?.data?.identity_card?.display_name
                if (hasIdentity) {
                  localStorage.setItem('folddb_onboarding_complete', '1')
                } else {
                  setShowOnboarding(true)
                }
              })
              .catch(() => {
                setShowOnboarding(true)
              })
          }
        } else {
          // If endpoint is unavailable (older backend), assume initialized
          setDbStatus({ initialized: true, has_saved_config: true, onboarding_complete: true })
        }
      })
      .catch(() => {
        // If endpoint doesn't exist, assume initialized (backwards compat)
        setDbStatus({ initialized: true, has_saved_config: true, onboarding_complete: true })
      })
      .finally(() => setDbStatusLoading(false))
  }, [isAuthenticated])



  // Only fetch schemas when authenticated
  const {
    error: schemasError,
    refetch: refetchSchemas
  } = useApprovedSchemas({ enabled: isAuthenticated })

  // Fetch AI configuration on mount (after auth)
  useEffect(() => {
    if (isAuthenticated) {
      dispatch(fetchIngestionConfig())
    }
  }, [dispatch, isAuthenticated])

  // Check AI configuration status for setup banner
  const ingestionConfig = useAppSelector(selectIngestionConfig)
  const aiConfigured = useAppSelector(selectIsAiConfigured)
  const aiProvider = useAppSelector(selectAiProvider)
  const showSetupBanner = isAuthenticated && ingestionConfig !== null && !aiConfigured && !setupDismissed

  const handleTabChange = (tab) => {
    setActiveTab(tab)
    // Clear settings sub-tab when navigating away from settings
    if (tab !== 'settings') setSettingsSubTab(null)
    // Update URL hash to match active tab
    if (typeof window !== 'undefined') {
      window.location.hash = tab;
    }
  }

  const navigateToSettings = (subTab) => {
    setSettingsSubTab(subTab || null)
    handleTabChange('settings')
  }

  const resultsRef = useRef(null)

  const handleOperationResult = (result) => {
    setResults(result)
    // Scroll results into view after rendering
    setTimeout(() => {
      resultsRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' })
    }, 100)
  }

  const handleSchemaUpdated = () => {
    refetchSchemas()
  }

  const renderActiveTab = () => {
    switch (activeTab) {
      case 'agent':
        return <AgentTab />
      case 'schemas':
        return (
          <SchemaTab
            onResult={handleOperationResult}
            onSchemaUpdated={handleSchemaUpdated}
          />
        )
      case 'query':
        return <QueryTab onResult={handleOperationResult} />
      case 'llm-query':
        return <LlmQueryTab onResult={handleOperationResult} />
      case 'mutation':
        return <MutationTab onResult={handleOperationResult} />
      case 'smart-folder':
        return <SmartFolderTab onResult={handleOperationResult} />
      case 'ingestion':
        return <IngestionTab onResult={handleOperationResult} />
      case 'file-upload':
        return <FileUploadTab onResult={handleOperationResult} />
      case 'native-index':
        return <NativeIndexTab onResult={handleOperationResult} />
      case 'data-browser':
        return <DataBrowserTab />
      case 'word-graph':
        return <WordGraphTab />
      case 'discovery':
        return <DiscoveryTab onResult={handleOperationResult} />
      case 'discovery-browse':
        return <DiscoveryBrowseTab onResult={handleOperationResult} />
      case 'views':
        return <ViewsTab onResult={handleOperationResult} />
      case 'sharing':
        return <SharingTab onResult={handleOperationResult} />
      case 'apple-import':
        return <AppleImportTab onResult={handleOperationResult} />
      case 'feed':
        return <FeedTab />
      case 'shared-moments':
        return <SharedMomentsTab onResult={handleOperationResult} />
      case 'my-profile':
        return <MyProfileTab onResult={handleOperationResult} />
      case 'conflicts':
        return <ConflictsTab />
      case 'people':
        return <PeopleTab onResult={handleOperationResult} />
      case 'trust':
        return <TrustTab onResult={handleOperationResult} />
      case 'remote-query':
        return <RemoteQueryTab onResult={handleOperationResult} />
      case 'settings':
        return (
          <SettingsTab
            onResult={handleOperationResult}
            initialSubTab={settingsSubTab}
            onRelaunchOnboarding={() => { setShowOnboarding(true) }}
          />
        )
      default:
        return null
    }
  }

  // Show loading spinner while auto-login is in progress or checking db status
  if (!isAuthenticated || isAuthLoading || dbStatusLoading) {
    return (
      <div className="h-screen flex items-center justify-center bg-surface-secondary">
        <div className="text-center">
          <div className="w-6 h-6 border-2 border-border border-t-primary rounded-full animate-spin mx-auto mb-4" />
          <p className="text-secondary text-sm">Loading...</p>
        </div>
      </div>
    );
  }

  // Show database setup screen for fresh installs (no saved config, not initialized)
  if (dbStatus && !dbStatus.initialized && !dbStatus.has_saved_config) {
    return (
      <DatabaseSetupScreen
        onComplete={() => {
          // Re-check database status after setup
          setDbStatusLoading(true)
          getDatabaseStatus()
            .then(response => {
              if (response.success && response.data) {
                setDbStatus(response.data)
              }
            })
            .catch(() => {
              // Assume initialized after successful setup call
              setDbStatus({ initialized: true, has_saved_config: true })
            })
            .finally(() => setDbStatusLoading(false))
        }}
      />
    );
  }

  // Show onboarding wizard on first run (after DB is initialized)
  if (showOnboarding && dbStatus?.initialized) {
    return (
      <OnboardingWizard
        onComplete={() => setShowOnboarding(false)}
      />
    )
  }

  return (
    <div className="h-screen flex flex-col bg-surface overflow-hidden">
      <Header
        onSettingsClick={() => navigateToSettings(null)}
        onAiSettingsClick={() => navigateToSettings('ai')}
        onCloudSettingsClick={() => navigateToSettings('upgrade-cloud')}
      />
      <UpdateBanner />

      {showSetupBanner && (
        <div className="bg-gruvbox-elevated border-b border-border px-8 py-3 flex items-center justify-between">
          <span className="text-gruvbox-blue text-sm">
            Configure AI to get started — FoldDB needs Anthropic or local Ollama for ingestion and search.
          </span>
          <div className="flex items-center gap-3">
            <button
              onClick={() => navigateToSettings('ai')}
              className="bg-gruvbox-blue text-surface text-sm px-4 py-1.5 border-none cursor-pointer hover:bg-gruvbox-green transition-colors"
            >
              Configure AI
            </button>
            <button
              onClick={() => {
                setSetupDismissed(true)
                localStorage.setItem('folddb_setup_dismissed', '1')
              }}
              className="text-gruvbox-blue text-sm bg-transparent border-none cursor-pointer hover:text-gruvbox-bright transition-colors"
            >
              Dismiss
            </button>
          </div>
        </div>
      )}

      {aiConfigured && aiProvider !== 'Ollama' && (
        <div className="bg-gruvbox-yellow/15 border-b-2 border-gruvbox-yellow px-4 sm:px-8 py-2 sm:py-3 flex flex-col sm:flex-row items-start sm:items-center justify-between gap-2 sm:gap-4">
          <span className="text-gruvbox-yellow text-xs sm:text-sm font-medium">
            Warning: AI is using {aiProvider} — personal data may be sent to external servers. Switch to a local LLM (Ollama) to keep data on your device.
          </span>
          <button
            onClick={() => navigateToSettings('ai')}
            className="bg-gruvbox-yellow text-surface text-xs sm:text-sm px-3 sm:px-4 py-1 sm:py-1.5 border-none cursor-pointer hover:bg-gruvbox-orange transition-colors whitespace-nowrap flex-shrink-0"
          >
            Switch to Local LLM
          </button>
        </div>
      )}

      <div className="flex flex-1 overflow-hidden">
        <Sidebar
          activeTab={activeTab}
          onTabChange={handleTabChange}
        />

        <main className="flex-1 overflow-y-auto bg-surface-secondary">
          <div className="max-w-5xl mx-auto p-6 bg-surface min-h-full">
            {/* Schema Loading/Error States */}
            {schemasError && (
              <div className="mb-4 p-3 bg-surface border border-border border-l-4 border-l-gruvbox-red">
                <p className="text-gruvbox-red text-sm">{schemasError}</p>
              </div>
            )}

            {/* Section Title */}
            <div className="text-xs uppercase tracking-widest text-tertiary mb-3">
              {activeTab.replaceAll('-', ' ')}
            </div>

            {/* Tab Content */}
            {renderActiveTab()}

            {/* Results */}
            {results && isIngestionResult(results) && (
              <IngestionReport
                ingestionResult={results}
                onDismiss={() => setResults(null)}
              />
            )}
            {results && !isIngestionResult(results) && (
              <div className="mt-6" ref={resultsRef}>
                <div className="text-xs uppercase tracking-widest text-tertiary mb-3">
                  Results
                </div>
                <ResultsSection results={results} />
              </div>
            )}
          </div>
        </main>

        <LogSidebar />
      </div>

      <Footer />
    </div>
  )
}

function App() {
  return (
    <ErrorBoundary>
      <FoldDbProvider>
        <AppContent />
      </FoldDbProvider>
    </ErrorBoundary>
  )
}

export default App
