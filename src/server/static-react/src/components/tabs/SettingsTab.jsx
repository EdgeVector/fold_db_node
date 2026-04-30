import { useState, useEffect } from 'react'
import KeyManagementTab from './KeyManagementTab'
import MyProfileTab from './MyProfileTab'
import useAiConfig from '../settings/AiConfigSettings'
import SchemaServiceSettings from '../settings/SchemaServiceSettings'
import useDatabaseConfig from '../settings/DatabaseSettings'
import CloudMigrationSettings from './CloudMigrationSettings'
import BackupSettingsPanel from '../settings/BackupSettingsPanel'
import OrgSettingsPanel from '../settings/OrgSettingsPanel'

const NOOP = () => {}

const SUB_TABS = [
  { id: 'ai', label: 'AI Configuration' },
  { id: 'keys', label: 'Key Management' },
  { id: 'schema-service', label: 'Schema Service' },
  { id: 'database', label: 'Database' },
  { id: 'org', label: 'Organizations' },
  { id: 'upgrade-cloud', label: 'Cloud Features' },
  { id: 'profile', label: 'Profile' },
]

export default function SettingsTab({ onResult, initialSubTab, onRelaunchOnboarding }) {
  const [activeSubTab, setActiveSubTab] = useState(initialSubTab || 'ai')
  const [configSaveStatus, setConfigSaveStatus] = useState(null)

  // Update sub-tab when initialSubTab prop changes
  useEffect(() => {
    if (initialSubTab) setActiveSubTab(initialSubTab)
  }, [initialSubTab])

  // These custom hooks use React hooks internally and MUST be
  // called unconditionally (not inside conditionals or callbacks).
  const aiConfig = useAiConfig({ configSaveStatus, setConfigSaveStatus, onClose: NOOP })
  const dbConfig = useDatabaseConfig({ configSaveStatus, setConfigSaveStatus, onClose: NOOP })

  const renderContent = () => {
    switch (activeSubTab) {
      case 'ai':
        return (
          <div className="space-y-4">
            {aiConfig.content}
            <div className="flex items-center gap-3 pt-4 border-t border-border">
              <button onClick={() => aiConfig.saveAiConfig()} className="btn-primary">
                Save Configuration
              </button>
              {configSaveStatus && (
                <span className="text-sm text-gruvbox-green">{configSaveStatus}</span>
              )}
            </div>
          </div>
        )
      case 'keys':
        return <KeyManagementTab onResult={onResult || NOOP} />
      case 'schema-service':
        return <SchemaServiceSettings />
      case 'database':
        return (
          <div className="space-y-4">
            {dbConfig.content}
            <div className="flex items-center gap-3 pt-4 border-t border-border">
              <button onClick={() => dbConfig.saveDatabaseConfig()} className="btn-primary">
                Save and Restart DB
              </button>
              {configSaveStatus && (
                <span className="text-sm text-gruvbox-green">{configSaveStatus}</span>
              )}
            </div>
          </div>
        )
      case 'org':
        return <OrgSettingsPanel />
      case 'upgrade-cloud':
        return (
          <div className="space-y-6">
            <CloudMigrationSettings />
            <BackupSettingsPanel />
          </div>
        )
      case 'profile':
        return <MyProfileTab onResult={onResult || NOOP} />
      default:
        return null
    }
  }

  return (
    <div className="flex gap-6 -mt-1">
      {/* Left rail — same pattern as People IA (#762) and the app's
        * main sidebar. 7 destinations stacked vertically, active item
        * gets a yellow border-l-2 stripe + bg + bright text. Predictable
        * grammar across the app's "many sub-views" surfaces. */}
      <nav
        className="shrink-0 w-44 border-r border-border pr-2 -ml-1 flex flex-col"
        aria-label="Settings sub-sections"
      >
        <div>
          {SUB_TABS.map((tab) => {
            const isActive = activeSubTab === tab.id
            return (
              <button
                key={tab.id}
                type="button"
                onClick={() => setActiveSubTab(tab.id)}
                aria-current={isActive ? 'page' : undefined}
                className={`w-full text-left px-3 py-1.5 text-sm transition-colors border-l-2 bg-transparent cursor-pointer ${
                  isActive
                    ? 'bg-surface-secondary border-l-gruvbox-yellow text-primary'
                    : 'text-secondary hover:text-primary hover:bg-surface-secondary border-l-transparent'
                }`}
              >
                {tab.label}
              </button>
            )
          })}
        </div>
        {/* "Relaunch Setup Wizard" lives at the bottom of the rail so
          * it's always visible regardless of which sub-tab is showing.
          * Previously rendered after the active panel — disappeared
          * below the fold on dense panels (Profile, Cloud Features)
          * and floated lonely on sparse ones (Key Management). */}
        {onRelaunchOnboarding && (
          <div className="mt-4 pt-3 border-t border-border px-3">
            <button
              type="button"
              onClick={() => {
                localStorage.removeItem('folddb_onboarding_complete')
                onRelaunchOnboarding()
              }}
              className="text-xs text-tertiary hover:text-primary text-left bg-transparent border-none cursor-pointer p-0 transition-colors"
            >
              Relaunch setup wizard
            </button>
          </div>
        )}
      </nav>
      <div className="flex-1 min-w-0">
        {renderContent()}
      </div>
    </div>
  )
}
