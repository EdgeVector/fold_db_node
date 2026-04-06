import { useState, useEffect } from 'react'
import { useAppDispatch, useAppSelector } from '../store/hooks'
import { logoutUser, autoLogin } from '../store/authSlice'
import { selectIngestionConfig, selectAiProvider, selectActiveModel, selectIsAiConfigured } from '../store/ingestionSlice'
import { BROWSER_CONFIG } from '../constants/config'
import { systemClient } from '../api/clients/systemClient'
import { getSubscriptionStatus, formatBytes } from '../api/clients/subscriptionClient'
import { orgClient } from '../api/clients/orgClient'
import HeaderProgress from './HeaderProgress'
import AnimatedLogo from './AnimatedLogo'
import SyncStatusIndicator from './SyncStatusIndicator'
import PendingInvitesModal from './PendingInvitesModal'
import { EnvelopeIcon } from '@heroicons/react/24/outline'

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
  const [nodePublicKey, setNodePublicKey] = useState(null)

  // Pending Invites State
  const [pendingInvites, setPendingInvites] = useState([])
  const [isInvitesModalOpen, setIsInvitesModalOpen] = useState(false)

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
    systemClient.getNodePublicKey().then(res => {
      if (res.data?.public_key) setNodePublicKey(res.data.public_key)
    }).catch(() => {})
    // Fetch cloud storage quota if connected
    const hasCloud = localStorage.getItem('exemem_api_url') && localStorage.getItem('exemem_api_key')
    if (hasCloud) {
      getSubscriptionStatus().then(status => {
        setStorageSize(status.storage.used_bytes)
        setStorageQuota(status.storage.quota_bytes)
      }).catch(() => { /* cloud API not reachable */ })
    }
  }, [])

  // Poll for invites after layout mount
  useEffect(() => {
    if (!isAuthenticated) return;
    const fetchInvites = async () => {
      try {
        const res = await orgClient.getPendingInvites();
        const invites = res.data?.invites || res.data || [];
        if (Array.isArray(invites)) setPendingInvites(invites);
      } catch {
        // fail silently for telemetry
      }
    };
    fetchInvites();
    // Poll every 60 seconds
    const interval = setInterval(fetchInvites, 60000);
    return () => clearInterval(interval);
  }, [isAuthenticated]);

  const handleLogout = () => {
    dispatch(logoutUser())
    localStorage.removeItem(BROWSER_CONFIG.STORAGE_KEYS.USER_ID)
    localStorage.removeItem(BROWSER_CONFIG.STORAGE_KEYS.USER_HASH)
    // Re-trigger auto-login with node identity (no login screen)
    dispatch(autoLogin())
  }

  const formattedSize = formatStorageSize(storageSize)
  const formattedQuota = storageQuota ? formatBytes(storageQuota) : null
  const quotaWarning = storageQuota && storageSize ? (storageSize / storageQuota) > 0.8 : false

  const displayKey = nodePublicKey || user?.id
  const truncatedDisplay = displayKey ? `${displayKey.slice(0, 8)}...` : null
  const [idCopied, setIdCopied] = useState(false)

  const handleCopyId = async () => {
    const valueToCopy = nodePublicKey || user?.id
    if (valueToCopy) {
      await navigator.clipboard.writeText(valueToCopy)
      setIdCopied(true)
      setTimeout(() => setIdCopied(false), 2000)
    }
  }

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
        <div className="flex items-center gap-3">
          {/* Status badges — compact pills instead of verbose text */}
          <div className="flex items-center gap-1.5">
            <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-gruvbox-elevated border border-border text-secondary">
              {storageMode || '...'}
              {formattedSize && formattedQuota
                ? <span className={quotaWarning ? 'text-gruvbox-orange' : ''}>{formattedSize}/{formattedQuota}</span>
                : formattedSize && <span>{formattedSize}</span>}
            </span>
            {ingestionConfig && (
              <button
                onClick={onAiSettingsClick}
                className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium border cursor-pointer transition-colors
                  ${aiReady
                    ? 'bg-gruvbox-elevated border-gruvbox-green/30 text-gruvbox-green hover:border-gruvbox-green'
                    : 'bg-gruvbox-elevated border-gruvbox-red/30 text-gruvbox-red hover:border-gruvbox-red'}`}
                title={aiReady ? `${aiProvider} · ${activeModel}` : 'AI not configured — click to set up'}
              >
                {aiReady ? `AI: ${aiProvider}` : 'AI: off'}
              </button>
            )}
            <SyncStatusIndicator onCloudSettingsClick={onCloudSettingsClick} />
          </div>

          {/* Node Public Key — truncated with copy */}
          {isAuthenticated && truncatedDisplay && (
            <button
              onClick={handleCopyId}
              className="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-mono text-tertiary bg-transparent border border-border cursor-pointer hover:text-secondary hover:border-secondary transition-colors"
              title={`Public Key: ${displayKey}\nClick to copy`}
            >
              {idCopied ? 'Copied!' : truncatedDisplay}
            </button>
          )}

          {/* Action buttons */}
          <div className="relative">
            <button
              onClick={() => setIsInvitesModalOpen(true)}
              className="p-2 text-tertiary hover:text-primary transition-colors bg-transparent border-none cursor-pointer flex items-center justify-center"
              title="Inbox"
            >
              <EnvelopeIcon className="w-5 h-5" />
              {pendingInvites.length > 0 && (
                <span className="absolute top-0 right-0 w-2.5 h-2.5 bg-gruvbox-red rounded-full border-2 border-surface"></span>
              )}
            </button>
          </div>
          <button onClick={onSettingsClick} className="btn-secondary" title="Settings">
            Settings
          </button>
        </div>
      </div>
      
      <PendingInvitesModal 
        isOpen={isInvitesModalOpen} 
        onClose={() => setIsInvitesModalOpen(false)}
        pendingInvites={pendingInvites}
        setPendingInvites={setPendingInvites}
      />
    </header>
  )
}

export default Header
