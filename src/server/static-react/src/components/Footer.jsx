import { useState, useEffect } from 'react'
import { systemClient } from '../api/clients/systemClient'
import packageJson from '../../package.json'

function Footer() {
  const [storageMode, setStorageMode] = useState('Local')

  useEffect(() => {
    // Check database config from server
    systemClient.getDatabaseConfig().then(res => {
      if (res.data) {
        const isCloud = res.data.type === 'cloud' || res.data.type === 'dynamodb' || res.data.type === 'exemem'
        setStorageMode(isCloud ? 'Cloud' : 'Local')
      }
    }).catch(() => {})

    // Also check localStorage for credentials set during onboarding (before server restart)
    const hasLocalCreds = localStorage.getItem('exemem_api_url') && localStorage.getItem('exemem_api_key')
    if (hasLocalCreds) setStorageMode('Cloud')
  }, [])

  return (
    <footer className="bg-surface border-t border-border px-8 py-2.5 flex-shrink-0 text-tertiary text-sm">
      <div className="flex items-center justify-between">
        <span>FoldDB v{packageJson.version}</span>
        <span>{storageMode} Mode</span>
      </div>
    </footer>
  )
}

export default Footer
