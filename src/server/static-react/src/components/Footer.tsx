import { useState, useEffect } from 'react'
import { systemClient } from '../api/clients/systemClient'

function Footer() {
  const [storageMode, setStorageMode] = useState<'Local' | 'Cloud'>('Local')
  // Sourced from /api/health so the footer matches the running binary
  // (FOLDDB_BUILD_VERSION). Empty until the probe lands so we never paint a
  // stale literal — the React app's own package.json drifts and the Tauri
  // bundle ships whatever Rust crate version was tagged.
  const [version, setVersion] = useState<string>('')

  useEffect(() => {
    systemClient.getDatabaseConfig().then(res => {
      if (res.data) {
        const isCloud = res.data.type === 'cloud' || res.data.type === 'exemem'
        setStorageMode(isCloud ? 'Cloud' : 'Local')
      }
    }).catch(() => {})

    const hasLocalCreds = localStorage.getItem('exemem_api_url') && localStorage.getItem('exemem_api_key')
    if (hasLocalCreds) setStorageMode('Cloud')

    systemClient.getHealth().then(res => {
      if (res.data?.version) setVersion(res.data.version)
    }).catch(() => {})
  }, [])

  return (
    <footer className="bg-surface border-t border-border px-8 py-2.5 flex-shrink-0 text-tertiary text-sm">
      <div className="flex items-center justify-between">
        <span>{version ? `FoldDB v${version}` : 'FoldDB'}</span>
        <span>{storageMode} Mode</span>
      </div>
    </footer>
  )
}

export default Footer
