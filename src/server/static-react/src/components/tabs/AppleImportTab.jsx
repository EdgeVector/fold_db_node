import { useState, useEffect, useRef, useCallback } from 'react'
import ingestionClient from '../../api/clients/ingestionClient'

function AutoSyncSettings() {
  const [config, setConfig] = useState(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [customHours, setCustomHours] = useState(12)

  useEffect(() => {
    ingestionClient.getAppleSyncConfig().then((resp) => {
      if (resp.success && resp.data) {
        setConfig(resp.data)
        if (typeof resp.data.schedule === 'object' && resp.data.schedule?.custom) {
          setCustomHours(resp.data.schedule.custom.hours)
        }
      }
      setLoading(false)
    }).catch(() => setLoading(false))
  }, [])

  // Refresh next sync time every 30 seconds
  useEffect(() => {
    if (!config?.enabled) return
    const timer = setInterval(async () => {
      const resp = await ingestionClient.getAppleNextSync()
      if (resp.success && resp.data) {
        setConfig((prev) => prev ? { ...prev, next_sync: resp.data.next_sync, last_sync: resp.data.last_sync } : prev)
      }
    }, 30000)
    return () => clearInterval(timer)
  }, [config?.enabled])

  const updateConfig = async (update) => {
    setSaving(true)
    const resp = await ingestionClient.updateAppleSyncConfig(update)
    if (resp.success && resp.data) {
      setConfig(resp.data)
    }
    setSaving(false)
  }

  const handleToggle = () => {
    updateConfig({ enabled: !config?.enabled })
  }

  const handleScheduleChange = (e) => {
    const val = e.target.value
    if (val === 'custom') {
      updateConfig({ schedule: { custom: { hours: customHours } } })
    } else {
      updateConfig({ schedule: val })
    }
  }

  const handleCustomHoursChange = (e) => {
    const hours = parseInt(e.target.value) || 1
    setCustomHours(hours)
    updateConfig({ schedule: { custom: { hours } } })
  }

  const handleSourceToggle = (source) => {
    if (!config) return
    updateConfig({
      sources: { ...config.sources, [source]: !config.sources[source] },
    })
  }

  const handlePhotosLimitChange = (e) => {
    const limit = parseInt(e.target.value) || 50
    updateConfig({ photos_limit: limit })
  }

  const getScheduleValue = () => {
    if (!config) return 'daily'
    if (typeof config.schedule === 'string') return config.schedule
    if (config.schedule?.custom) return 'custom'
    return 'daily'
  }

  const formatTime = (isoStr) => {
    if (!isoStr) return 'Never'
    const d = new Date(isoStr)
    return d.toLocaleString()
  }

  if (loading) {
    return (
      <div className="bg-surface-primary border border-border rounded-lg p-4 mb-4">
        <p className="text-xs text-secondary">Loading sync settings...</p>
      </div>
    )
  }

  if (!config) return null

  return (
    <div className="bg-surface-primary border border-border rounded-lg p-4 mb-4">
      <div className="flex items-center justify-between mb-3">
        <div className="flex items-center gap-2">
          <span className="text-lg">{"\u{1F504}"}</span>
          <h3 className="text-sm font-medium text-primary">Auto-Sync</h3>
        </div>
        <button
          onClick={handleToggle}
          disabled={saving}
          className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors ${
            config.enabled ? 'bg-accent' : 'bg-surface-secondary border border-border'
          }`}
        >
          <span
            className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${
              config.enabled ? 'translate-x-6' : 'translate-x-1'
            }`}
          />
        </button>
      </div>

      {config.enabled && (
        <div className="space-y-3">
          {/* Schedule selector */}
          <div className="flex items-center gap-2">
            <label className="text-xs text-secondary w-16">Schedule:</label>
            <select
              value={getScheduleValue()}
              onChange={handleScheduleChange}
              className="px-2 py-1 text-xs bg-surface-secondary border border-border rounded text-primary"
            >
              <option value="daily">Daily</option>
              <option value="weekly">Weekly</option>
              <option value="custom">Custom</option>
            </select>
            {getScheduleValue() === 'custom' && (
              <div className="flex items-center gap-1">
                <span className="text-xs text-secondary">every</span>
                <input
                  type="number"
                  min={1}
                  max={720}
                  value={customHours}
                  onChange={handleCustomHoursChange}
                  className="w-16 px-2 py-1 text-xs bg-surface-secondary border border-border rounded text-primary"
                />
                <span className="text-xs text-secondary">hours</span>
              </div>
            )}
          </div>

          {/* Source toggles */}
          <div className="flex items-center gap-4">
            <label className="text-xs text-secondary w-16">Sources:</label>
            {['notes', 'reminders', 'photos'].map((source) => (
              <label key={source} className="flex items-center gap-1 text-xs text-primary cursor-pointer">
                <input
                  type="checkbox"
                  checked={config.sources[source]}
                  onChange={() => handleSourceToggle(source)}
                  className="rounded border-border"
                />
                {source.charAt(0).toUpperCase() + source.slice(1)}
              </label>
            ))}
          </div>

          {/* Photos limit (shown when photos enabled) */}
          {config.sources.photos && (
            <div className="flex items-center gap-2">
              <label className="text-xs text-secondary w-16">Photo limit:</label>
              <input
                type="number"
                min={1}
                max={500}
                value={config.photos_limit}
                onChange={handlePhotosLimitChange}
                className="w-20 px-2 py-1 text-xs bg-surface-secondary border border-border rounded text-primary"
              />
            </div>
          )}

          {/* Sync times */}
          <div className="border-t border-border pt-2 space-y-1">
            <p className="text-xs text-secondary">
              <span className="font-medium">Last sync:</span> {formatTime(config.last_sync)}
            </p>
            <p className="text-xs text-secondary">
              <span className="font-medium">Next sync:</span>{' '}
              <span className={config.next_sync ? 'text-accent' : ''}>
                {formatTime(config.next_sync)}
              </span>
            </p>
          </div>
        </div>
      )}

      {!config.enabled && (
        <p className="text-xs text-secondary">
          Enable to automatically re-import Apple data on a schedule. Content-hash dedup ensures unchanged items are skipped.
        </p>
      )}
    </div>
  )
}

function ImportSection({ label, icon, description, fields, onImport, progressId, onReset }) {
  const [status, setStatus] = useState('idle') // idle | running | done | error
  const [progress, setProgress] = useState(0)
  const [message, setMessage] = useState('')
  const [result, setResult] = useState(null)
  const pollRef = useRef(null)

  // Poll progress when we have a progressId
  useEffect(() => {
    if (!progressId || status !== 'running') return

    const poll = async () => {
      try {
        const resp = await ingestionClient.getJobProgress(progressId)
        if (resp.success && resp.data) {
          const job = resp.data
          setProgress(job.progress_percentage || 0)
          setMessage(job.status_message || job.message || '')

          if (job.is_complete) {
            setStatus('done')
            setResult(job.results || job.result || null)
            clearInterval(pollRef.current)
          } else if (job.is_failed) {
            setStatus('error')
            setMessage(job.error_message || job.message || 'Import failed')
            clearInterval(pollRef.current)
          }
        }
      } catch {
        // Ignore poll errors, keep polling
      }
    }

    pollRef.current = setInterval(poll, 2000)
    poll() // immediate first poll
    return () => clearInterval(pollRef.current)
  }, [progressId, status])

  const handleImport = async () => {
    setStatus('running')
    setProgress(5)
    setMessage('Starting...')
    setResult(null)
    try {
      await onImport()
    } catch (e) {
      setStatus('error')
      setMessage(e.message || 'Failed to start import')
    }
  }

  const handleReset = () => {
    setStatus('idle')
    setProgress(0)
    setMessage('')
    setResult(null)
    if (onReset) onReset()
  }

  return (
    <div className="bg-surface-primary border border-border rounded-lg p-4 mb-4">
      <div className="flex items-center gap-2 mb-2">
        <span className="text-lg">{icon}</span>
        <h3 className="text-sm font-medium text-primary">{label}</h3>
      </div>
      <p className="text-xs text-secondary mb-3">{description}</p>

      {fields}

      {status === 'idle' && (
        <button
          onClick={handleImport}
          className="px-4 py-2 bg-accent text-white text-sm rounded hover:opacity-90 transition-opacity"
        >
          Import {label}
        </button>
      )}

      {status === 'running' && (
        <div className="space-y-2">
          <div className="w-full bg-surface-secondary rounded-full h-2">
            <div
              className="bg-accent h-2 rounded-full transition-all duration-300"
              style={{ width: `${progress}%` }}
            />
          </div>
          <p className="text-xs text-secondary">{message}</p>
        </div>
      )}

      {status === 'done' && (
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <span className="text-green-500">&#10003;</span>
            <span className="text-sm text-primary">{message}</span>
          </div>
          {result && (
            <p className="text-xs text-secondary">
              Total: {result.total}, Ingested: {result.ingested}
            </p>
          )}
          <button
            onClick={handleReset}
            className="px-3 py-1 text-xs border border-border rounded hover:bg-surface-secondary transition-colors"
          >
            Import Again
          </button>
        </div>
      )}

      {status === 'error' && (
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <span className="text-red-500">&#10007;</span>
            <span className="text-sm text-red-400">{message}</span>
          </div>
          <button
            onClick={handleReset}
            className="px-3 py-1 text-xs border border-border rounded hover:bg-surface-secondary transition-colors"
          >
            Try Again
          </button>
        </div>
      )}
    </div>
  )
}

export default function AppleImportTab({ onResult }) {
  const [available, setAvailable] = useState(null) // null = loading, true/false
  const [notesProgressId, setNotesProgressId] = useState(null)
  const [remindersProgressId, setRemindersProgressId] = useState(null)
  const [photosProgressId, setPhotosProgressId] = useState(null)
  const [photosLimit, setPhotosLimit] = useState(50)

  useEffect(() => {
    ingestionClient.getAppleImportStatus().then((resp) => {
      console.log('APPLE_STATUS_RESP:', JSON.stringify(resp))
      setAvailable(resp.success && resp.data?.available)
    }).catch((e) => {
      console.log('APPLE_STATUS_ERR:', e.message)
      setAvailable(false)
    })
  }, [])

  const importNotes = useCallback(async () => {
    const resp = await ingestionClient.appleImportNotes()
    if (resp.success && resp.data?.progress_id) {
      setNotesProgressId(resp.data.progress_id)
    } else {
      throw new Error(resp.error?.message || 'Failed to start notes import')
    }
  }, [])

  const importReminders = useCallback(async () => {
    const resp = await ingestionClient.appleImportReminders()
    if (resp.success && resp.data?.progress_id) {
      setRemindersProgressId(resp.data.progress_id)
    } else {
      throw new Error(resp.error?.message || 'Failed to start reminders import')
    }
  }, [])

  const importPhotos = useCallback(async () => {
    const resp = await ingestionClient.appleImportPhotos(null, photosLimit)
    if (resp.success && resp.data?.progress_id) {
      setPhotosProgressId(resp.data.progress_id)
    } else {
      throw new Error(resp.error?.message || 'Failed to start photos import')
    }
  }, [photosLimit])

  if (available === null) {
    return (
      <div className="p-4 text-center text-secondary text-sm">
        Checking Apple import availability...
      </div>
    )
  }

  if (!available) {
    return (
      <div className="p-4">
        <div className="bg-surface-primary border border-border rounded-lg p-6 text-center">
          <p className="text-secondary text-sm">
            Apple Import is only available on macOS.
          </p>
        </div>
      </div>
    )
  }

  return (
    <div className="p-4 space-y-2">
      <p className="text-xs text-secondary mb-4">
        Import data from macOS apps into FoldDB. You may be prompted to grant access permission on first use.
      </p>

      <AutoSyncSettings />

      <ImportSection
        label="Apple Notes"
        icon={"\uD83D\uDCDD"}
        description="Import all notes from Apple Notes. Notes shorter than 20 characters are skipped."
        progressId={notesProgressId}
        onImport={importNotes}
        onReset={() => setNotesProgressId(null)}
      />

      <ImportSection
        label="Apple Reminders"
        icon={"\u2705"}
        description="Import all reminders from Apple Reminders, including completed items."
        progressId={remindersProgressId}
        onImport={importReminders}
        onReset={() => setRemindersProgressId(null)}
      />

      <ImportSection
        label="Apple Photos"
        icon={"\uD83D\uDCF7"}
        description="Export and import photos from Apple Photos. HEIC files are converted to JPEG."
        fields={
          <div className="flex items-center gap-2 mb-3">
            <label className="text-xs text-secondary">Limit:</label>
            <input
              type="number"
              min={1}
              max={500}
              value={photosLimit}
              onChange={(e) => setPhotosLimit(parseInt(e.target.value) || 50)}
              className="w-20 px-2 py-1 text-xs bg-surface-secondary border border-border rounded text-primary"
            />
            <span className="text-xs text-secondary">photos</span>
          </div>
        }
        progressId={photosProgressId}
        onImport={importPhotos}
        onReset={() => setPhotosProgressId(null)}
      />
    </div>
  )
}
