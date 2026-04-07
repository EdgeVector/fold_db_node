import { useState, useEffect } from 'react'
import { FoldDbProvider } from './components/FoldDbProvider'
import Header from './components/Header'
import Footer from './components/Footer'
import UpdateBanner from './components/UpdateBanner'
import ResultsSection from './components/ResultsSection'
import IngestionReport from './components/IngestionReport'
import TabNavigation from './components/TabNavigation'
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
import SettingsModal from './components/SettingsModal'
import OnboardingWizard, { ONBOARDING_STORAGE_KEY } from './components/onboarding/OnboardingWizard'

import LogSidebar from './components/LogSidebar'
import ErrorBoundary from './components/ErrorBoundary'
import { useApprovedSchemas } from './hooks/useApprovedSchemas.js'
import { useAppSelector, useAppDispatch } from './store/hooks'
import { initializeSystemKey, restoreSession, autoLogin } from './store/authSlice'
import { fetchIngestionConfig, selectIngestionConfig, selectIsAiConfigured, selectAiProvider } from './store/ingestionSlice'
import { DEFAULT_TAB } from './constants'
import { BROWSER_CONFIG } from './constants/config'
import { getDatabaseStatus, markOnboardingComplete } from './api/clients/systemClient'
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
  ingestion: 'ingestion', 'file-upload': 'file-upload',
  'native-index': 'native-index',
  'llm-query': 'llm-query', 'ai-query': 'llm-query',
  'smart-folder': 'smart-folder', import: 'smart-folder',
  'data-browser': 'data-browser',
  'word-graph': 'word-graph',
  discovery: 'discovery',
  'discovery-browse': 'discovery-browse',
  views: 'views',
  sharing: 'sharing',
  'apple-import': 'apple-import',
  feed: 'feed',
  'shared-moments': 'shared-moments',
  'my-profile': 'my-profile',
  conflicts: 'conflicts',
  trust: 'trust',
}

function resolveTabFromHash() {
  if (typeof window !== 'undefined' && window.location.hash) {
    return HASH_TO_TAB[window.location.hash.slice(1)] || null
  }
  return null
}

export function AppContent() {
  const [activeTab, setActiveTab] = useState(() => resolveTabFromHash() || DEFAULT_TAB)
  const [isSettingsOpen, setIsSettingsOpen] = useState(false)
  const [settingsInitialTab, setSettingsInitialTab] = useState(null)
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

  // Check database status after authenticated
  useEffect(() => {
    if (!isAuthenticated) return
    setDbStatusLoading(true)
    getDatabaseStatus()
      .then(response => {
        if (response.success && response.data) {
          setDbStatus(response.data)
          // Show onboarding if backend says it hasn't been completed.
          // This is authoritative — it's based on a marker file in the data dir,
          // so --empty-db (which wipes the data dir) resets onboarding state.
          if (!response.data.onboarding_complete) {
            setShowOnboarding(true)
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

  // Initialize system key ONLY after authenticated and DB initialized
  useEffect(() => {
    if (isAuthenticated && dbStatus?.initialized) {
      dispatch(initializeSystemKey())
    }
  }, [dispatch, isAuthenticated, dbStatus?.initialized])



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
    // Update URL hash to match active tab
    if (typeof window !== 'undefined') {
      window.location.hash = tab;
    }
  }

  const handleOperationResult = (result) => {
    setResults(result)
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
      case 'trust':
        return <TrustTab onResult={handleOperationResult} />
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
        onSettingsClick={() => { setSettingsInitialTab(null); setIsSettingsOpen(true) }}
        onAiSettingsClick={() => { setSettingsInitialTab('ai'); setIsSettingsOpen(true) }}
        onCloudSettingsClick={() => { setSettingsInitialTab('upgrade-cloud'); setIsSettingsOpen(true) }}
      />
      <UpdateBanner />
      <SettingsModal
        isOpen={isSettingsOpen}
        onClose={() => setIsSettingsOpen(false)}
        initialTab={settingsInitialTab}
        onRelaunchOnboarding={() => { setIsSettingsOpen(false); setShowOnboarding(true) }}
      />

      {showSetupBanner && (
        <div className="bg-gruvbox-elevated border-b border-border px-8 py-3 flex items-center justify-between">
          <span className="text-gruvbox-blue text-sm">
            Configure AI to get started — FoldDB needs Anthropic or local Ollama for ingestion and search.
          </span>
          <div className="flex items-center gap-3">
            <button
              onClick={() => setIsSettingsOpen(true)}
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
        <div className="bg-gruvbox-yellow/15 border-b-2 border-gruvbox-yellow px-8 py-3 flex items-center justify-between">
          <span className="text-gruvbox-yellow text-sm font-medium">
            Warning: AI is using {aiProvider} — personal data may be sent to external servers. Switch to a local LLM (Ollama) to keep data on your device.
          </span>
          <button
            onClick={() => { setSettingsInitialTab('ai'); setIsSettingsOpen(true) }}
            className="bg-gruvbox-yellow text-surface text-sm px-4 py-1.5 border-none cursor-pointer hover:bg-gruvbox-orange transition-colors whitespace-nowrap ml-4"
          >
            Switch to Local LLM
          </button>
        </div>
      )}

      <div className="flex flex-1 overflow-hidden">
        <div className="flex-1 flex flex-col overflow-hidden">
          <TabNavigation
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
                {activeTab.replace('-', ' ')}
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
                <div className="mt-6">
                  <div className="text-xs uppercase tracking-widest text-tertiary mb-3">
                    Results
                  </div>
                  <ResultsSection results={results} />
                </div>
              )}
            </div>
          </main>
        </div>

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
