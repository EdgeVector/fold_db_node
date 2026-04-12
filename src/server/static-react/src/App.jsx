import React from 'react'
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
import OnboardingWizard from './components/onboarding/OnboardingWizard'

import LogSidebar from './components/LogSidebar'
import ErrorBoundary from './components/ErrorBoundary'
import DatabaseSetupScreen from './components/DatabaseSetupScreen'
import { useApprovedSchemas } from './hooks/useApprovedSchemas.js'
import { useAuthInitialization } from './hooks/useAuthInitialization.js'
import { useTabRouting } from './hooks/useTabRouting.js'
import { useDatabaseInit } from './hooks/useDatabaseInit.js'
import { useResultHandler } from './hooks/useResultHandler.js'

function isIngestionResult(results) {
  if (!results?.success) return false
  const d = results?.data ?? results
  return typeof d === 'object' && Array.isArray(d?.schemas_written) && d.schemas_written.length > 0
}

export function AppContent() {
  // Auth + AI config orchestration
  const {
    isAuthenticated,
    isAuthLoading,
    aiConfigured,
    aiProvider,
    showSetupBanner,
    dismissSetup,
  } = useAuthInitialization()

  // Tab routing / URL hash sync
  const { activeTab, settingsSubTab, handleTabChange, navigateToSettings } =
    useTabRouting()

  // Database status + onboarding wizard
  const {
    dbStatus,
    dbStatusLoading,
    showOnboarding,
    setShowOnboarding,
    recheckDbStatus,
  } = useDatabaseInit(isAuthenticated)

  // Operation result handling (shared across tabs)
  const { results, setResults, resultsRef, handleOperationResult } =
    useResultHandler(activeTab)

  // Only fetch schemas when authenticated
  const { error: schemasError, refetch: refetchSchemas } = useApprovedSchemas({
    enabled: isAuthenticated,
  })

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
    return <DatabaseSetupScreen onComplete={recheckDbStatus} />
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
              onClick={dismissSetup}
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
