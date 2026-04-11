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
  { id: 'upgrade-cloud', label: 'Exemem' },
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
            <CloudMigrationSettings onClose={NOOP} />
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
    <div>
      <div className="flex border-b border-border mb-4 overflow-x-auto">
        {SUB_TABS.map(tab => (
          <button
            key={tab.id}
            onClick={() => setActiveSubTab(tab.id)}
            className={`tab whitespace-nowrap ${activeSubTab === tab.id ? 'tab-active' : ''}`}
          >
            {tab.label}
          </button>
        ))}
      </div>
      {renderContent()}
      {onRelaunchOnboarding && (
        <div className="mt-8 pt-4 border-t border-border">
          <button
            onClick={() => {
              localStorage.removeItem('folddb_onboarding_complete')
              onRelaunchOnboarding()
            }}
            className="btn-secondary"
          >
            Relaunch Setup Wizard
          </button>
        </div>
      )}
    </div>
  )
}
