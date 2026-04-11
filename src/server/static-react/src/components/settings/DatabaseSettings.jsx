import { useState, useEffect, useRef } from 'react'
import { getDatabaseConfig, updateDatabaseConfig, resetDatabase } from '../../api/clients/systemClient'
import { ingestionClient } from '../../api/clients'
import { TrashIcon } from '@heroicons/react/24/solid'

function useDatabaseConfig({ configSaveStatus, setConfigSaveStatus, onClose }) {
  const [dbType, setDbType] = useState('local')
  const [dbPath, setDbPath] = useState('data')
  const [isResetting, setIsResetting] = useState(false)
  const [resetResult, setResetResult] = useState(null)
  const pollIntervalRef = useRef(null)
  const fallbackTimeoutRef = useRef(null)
  const reloadTimeoutRef = useRef(null)
  const statusTimeoutRef = useRef(null)

  useEffect(() => {
    return () => {
      if (pollIntervalRef.current) clearInterval(pollIntervalRef.current)
      if (fallbackTimeoutRef.current) clearTimeout(fallbackTimeoutRef.current)
      if (reloadTimeoutRef.current) clearTimeout(reloadTimeoutRef.current)
      if (statusTimeoutRef.current) clearTimeout(statusTimeoutRef.current)
    }
  }, [])

  useEffect(() => { loadDatabaseConfig() }, [])

  const loadDatabaseConfig = async () => {
    try {
      const response = await getDatabaseConfig()
      if (response.success && response.data) {
        const c = response.data
        setDbType(c.type)
        if (c.type === 'local') setDbPath(c.path || 'data')
      }
    } catch (error) { console.error('Failed to load database config:', error) }
  }

  const saveDatabaseConfig = async () => {
    try {
      let config
      if (dbType === 'local') config = { type: 'local', path: dbPath }
      else {
        setConfigSaveStatus({ success: false, message: 'Only local storage is supported' })
        statusTimeoutRef.current = setTimeout(() => setConfigSaveStatus(null), 3000)
        return
      }
      const response = await updateDatabaseConfig(config)
      if (response.success) {
        setConfigSaveStatus({ success: true, message: response.data.requires_restart ? 'Saved. Please restart server.' : response.data.message || 'Saved and restarted' })
        statusTimeoutRef.current = setTimeout(() => { setConfigSaveStatus(null); if (!response.data.requires_restart) onClose() }, 3000)
      } else setConfigSaveStatus({ success: false, message: response.error || 'Failed to save' })
    } catch (error) { setConfigSaveStatus({ success: false, message: (error instanceof Error ? error.message : String(error)) || 'Failed to save' }) }
    statusTimeoutRef.current = setTimeout(() => setConfigSaveStatus(null), 5000)
  }

  const handleResetDatabase = async () => {
    setIsResetting(true); setResetResult(null)
    try {
      const response = await resetDatabase(true)
      if (response.success && response.data) {
        if (response.data.job_id) {
          pollIntervalRef.current = setInterval(async () => {
            try {
              const pr = await ingestionClient.getJobProgress(response.data.job_id)
              if (pr.success && pr.data) {
                if (pr.data.is_complete) { clearInterval(pollIntervalRef.current); pollIntervalRef.current = null; setResetResult({ type: 'success', message: 'Reset complete. Reloading...' }); reloadTimeoutRef.current = setTimeout(() => window.location.reload(), 1000) }
                else if (pr.data.is_failed) { clearInterval(pollIntervalRef.current); pollIntervalRef.current = null; setResetResult({ type: 'error', message: pr.data.error_message || 'Reset failed' }); setIsResetting(false) }
              }
            } catch { /* Polling error - will retry on next interval */ }
          }, 1000)
          fallbackTimeoutRef.current = setTimeout(() => { clearInterval(pollIntervalRef.current); pollIntervalRef.current = null; fallbackTimeoutRef.current = null; if (isResetting) { setResetResult({ type: 'success', message: 'Reset likely complete. Reloading...' }); reloadTimeoutRef.current = setTimeout(() => window.location.reload(), 1000) } }, 60000)
        } else { setResetResult({ type: 'success', message: response.data.message || 'Reset successfully' }); reloadTimeoutRef.current = setTimeout(() => window.location.reload(), 2000) }
      } else { setResetResult({ type: 'error', message: response.error || 'Reset failed' }); setIsResetting(false) }
    } catch (error) { setResetResult({ type: 'error', message: `Network error: ${error instanceof Error ? error.message : String(error)}` }); setIsResetting(false) }
  }

  return {
    saveDatabaseConfig,
    content: (
      <div className="space-y-4">
        <p className="text-sm text-secondary mb-4">Configure your database storage location. Changes require a restart.</p>

        <div>
          <label className="label">Storage Type</label>
          <div className="text-sm text-primary font-medium py-2">Local Storage</div>
          <p className="text-xs text-secondary">FoldDB stores your data locally on this device. To enable cloud backup, go to the Exemem tab.</p>
        </div>

        <div>
          <label className="label">Path</label>
          <input type="text" value={dbPath} onChange={(e) => setDbPath(e.target.value)} placeholder="data" className="input" />
          <p className="text-xs text-secondary mt-1">Local filesystem path for the database</p>
        </div>

        <div className="mt-8 pt-6 border-t border-gruvbox-red">
          <div className="flex items-center gap-2 mb-3">
            <TrashIcon className="w-5 h-5 text-gruvbox-red" />
            <h4 className="text-md font-semibold text-gruvbox-red">Danger Zone</h4>
          </div>
          <p className="text-sm text-secondary mb-4">Permanently delete all data and restart. Cannot be undone.</p>
          {!isResetting ? (
            <button onClick={handleResetDatabase} className="btn-danger flex items-center gap-2"><TrashIcon className="w-4 h-4" /> Reset Database</button>
          ) : (
            <div className="card card-info p-3 flex items-center gap-2 text-sm text-gruvbox-blue"><span className="spinner" /> Resetting Database...</div>
          )}
          {resetResult && (
            <div className={`mt-4 p-3 text-sm card ${resetResult.type === 'success' ? 'card-success text-gruvbox-green' : 'card-error text-gruvbox-red'}`}>
              {resetResult.message}
            </div>
          )}
        </div>

        {configSaveStatus && (
          <div className={`p-3 card ${configSaveStatus.success ? 'card-success text-gruvbox-green' : 'card-error text-gruvbox-red'}`}>
            <span className="text-sm font-medium">{configSaveStatus.success ? '✓' : '✗'} {configSaveStatus.message}</span>
          </div>
        )}
      </div>
    )
  }
}

export default useDatabaseConfig
