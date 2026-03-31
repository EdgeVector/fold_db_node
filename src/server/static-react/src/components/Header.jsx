import { useState, useEffect } from 'react'
import { useAppDispatch, useAppSelector } from '../store/hooks'
import { logoutUser } from '../store/authSlice'
import { selectIngestionConfig, selectAiProvider, selectActiveModel, selectIsAiConfigured } from '../store/ingestionSlice'
import { BROWSER_CONFIG } from '../constants/config'
import { systemClient } from '../api/clients/systemClient'
import { getSubscriptionStatus, formatBytes } from '../api/clients/subscriptionClient'
import HeaderProgress from './HeaderProgress'
import AnimatedLogo from './AnimatedLogo'
import SyncStatusIndicator from './SyncStatusIndicator'

function classifySchemaEnv(url) {
  if (!url) return { label: 'None', color: 'text-gruvbox-yellow' }
  if (url.includes('127.0.0.1') || url.includes('localhost')) return { label: 'Local', color: 'text-gruvbox-yellow' }
  if (url.includes('us-east-1')) return { label: 'Prod', color: 'text-gruvbox-green' }
  if (url.includes('us-west-2')) return { label: 'Dev', color: 'text-gruvbox-blue' }
  return { label: 'Custom', color: 'text-secondary' }
}

function formatStorageSize(bytes) {
  if (!bytes || bytes <= 0) return null
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`
}

function Header({ onSettingsClick, onAiSettingsClick, onCloudSettingsClick }) {
  const dispatch = useAppDispatch()
  const { isAuthenticated, user } = useAppSelector(state => state.auth)
  const ingestionConfig = useAppSelector(selectIngestionConfig)
  const aiProvider = useAppSelector(selectAiProvider)
  const activeModel = useAppSelector(selectActiveModel)
  const aiReady = useAppSelector(selectIsAiConfigured)
  const [storageMode, setStorageMode] = useState(null)
  const [storageSize, setStorageSize] = useState(null)
  const [storageQuota, setStorageQuota] = useState(null)
  const [schemaEnv, setSchemaEnv] = useState(null)

  useEffect(() => {
    systemClient.getDatabaseConfig().then(res => {
      if (res.data) {
        const isCloud = res.data.type === 'cloud' || res.data.type === 'dynamodb' || res.data.type === 'exemem'
        setStorageMode(isCloud ? 'Cloud' : 'Local')
        if (res.data.storage_size_bytes) setStorageSize(res.data.storage_size_bytes)
      }
    }).catch(() => { /* best-effort - header info is non-critical */ })
    systemClient.getSystemStatus().then(res => {
      if (res.data) setSchemaEnv(classifySchemaEnv(res.data.schema_service_url))
    }).catch(() => { /* best-effort - header info is non-critical */ })
    // Fetch cloud storage quota if connected
    const hasCloud = localStorage.getItem('exemem_api_url') && localStorage.getItem('exemem_api_key')
    if (hasCloud) {
      getSubscriptionStatus().then(status => {
        setStorageSize(status.storage.used_bytes)
        setStorageQuota(status.storage.quota_bytes)
      }).catch(() => { /* cloud API not reachable */ })
    }
  }, [])

  const handleLogout = () => {
    dispatch(logoutUser())
    localStorage.removeItem(BROWSER_CONFIG.STORAGE_KEYS.USER_ID)
    localStorage.removeItem(BROWSER_CONFIG.STORAGE_KEYS.USER_HASH)
  }

  const isLocal = storageMode === 'Local'
  const formattedSize = formatStorageSize(storageSize)
  const formattedQuota = storageQuota ? formatBytes(storageQuota) : null
  const quotaWarning = storageQuota && storageSize ? (storageSize / storageQuota) > 0.8 : false

  return (
    <header className="bg-surface border-b border-border px-8 py-3 flex-shrink-0">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-6">
          <a href="/" className="flex items-center gap-2 text-lg font-medium tracking-tight text-primary no-underline hover:text-primary">
            <AnimatedLogo size={72} />
            FoldDB
          </a>
          <HeaderProgress />
        </div>
        <div className="flex items-center gap-4">
          <div className="flex items-center gap-2 text-sm text-secondary font-mono">
            <span>{storageMode || '...'}</span>
            {formattedSize && formattedQuota
              ? <><span className="text-tertiary">/</span><span className={quotaWarning ? 'text-gruvbox-orange' : 'text-secondary'}>{formattedSize} / {formattedQuota}</span></>
              : formattedSize && <><span className="text-tertiary">/</span><span className="text-secondary">{formattedSize}</span></>}
            {schemaEnv && <><span className="text-tertiary">/</span><span className={schemaEnv.color}>Schema: {schemaEnv.label}</span></>}
            {ingestionConfig && (
              <><span className="text-tertiary">/</span><button
                onClick={onAiSettingsClick}
                className={`bg-transparent border-none cursor-pointer p-0 font-mono text-sm ${aiReady ? 'text-gruvbox-green' : 'text-gruvbox-red'} hover:text-primary`}
                title={aiReady ? `${aiProvider} · ${activeModel}` : 'AI not configured -- click to open Settings'}
              >
                {aiReady ? `AI: ${aiProvider}` : 'AI: off'}
              </button></>
            )}
            <span className="text-tertiary">/</span>
            <SyncStatusIndicator onCloudSettingsClick={onCloudSettingsClick} />
          </div>
          {isAuthenticated && (
            <div className="flex items-center gap-4">
              <span className="text-secondary text-sm">
                {user?.id}
              </span>
              <button
                onClick={handleLogout}
                className="text-tertiary text-sm bg-transparent border-none cursor-pointer hover:text-primary transition-colors"
              >
                logout
              </button>
            </div>
          )}
          <button onClick={onSettingsClick} className="btn-secondary" title="Settings">
            Settings
          </button>
        </div>
      </div>
    </header>
  )
}

export default Header
