import { useState, useEffect, useRef, useCallback } from 'react'
import {
  ArrowPathIcon,
  DocumentTextIcon,
  CameraIcon,
  CalendarDaysIcon,
  CheckCircleIcon,
  UserIcon,
} from '@heroicons/react/24/outline'
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

  useEffect(() => {
    if (!config?.enabled) return
    const timer = setInterval(async () => {
      const resp = await ingestionClient.getAppleNextSync()
      if (resp.success && resp.data) {
        setConfig((prev) =>
          prev
            ? {
                ...prev,
                next_sync: resp.data.next_sync,
                last_sync: resp.data.last_sync,
                last_error: resp.data.last_error,
                last_error_at: resp.data.last_error_at,
              }
            : prev,
        )
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

  const handleToggle = () => updateConfig({ enabled: !config?.enabled })

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
      <div className="bg-surface border border-border rounded-lg p-4 mb-4">
        <p className="text-xs text-secondary">Loading sync settings...</p>
      </div>
    )
  }

  if (!config) return null

  return (
    <div className="bg-surface border border-border rounded-lg p-4 mb-4">
      <div className="flex items-center justify-between mb-3">
        <div className="flex items-center gap-2">
          <ArrowPathIcon aria-hidden="true" className="w-4 h-4 text-secondary" />
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

          <div className="flex items-center gap-4">
            <label className="text-xs text-secondary w-16">Sources:</label>
            {['notes', 'reminders', 'photos', 'calendar', 'contacts'].map((source) => (
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
            {config.last_error && (
              <p
                className="text-xs text-gruvbox-red break-words"
                data-testid="auto-sync-last-error"
              >
                <span className="font-medium">Last error:</span>{' '}
                {formatTime(config.last_error_at)} &mdash; {config.last_error}
              </p>
            )}
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

const SOURCES = [
  {
    key: 'notes',
    label: 'Notes',
    Icon: DocumentTextIcon,
    description: 'Import all notes from Apple Notes. Notes shorter than 20 characters are skipped.',
  },
  {
    key: 'photos',
    label: 'Photos',
    Icon: CameraIcon,
    description: 'Export and import photos from Apple Photos. HEIC files are converted to JPEG.',
    hasLimit: true,
  },
  {
    key: 'calendar',
    label: 'Calendar',
    Icon: CalendarDaysIcon,
    description: 'Import events from Apple Calendar.',
  },
  {
    key: 'reminders',
    label: 'Reminders',
    Icon: CheckCircleIcon,
    description: 'Import all reminders from Apple Reminders, including completed items.',
  },
  {
    key: 'contacts',
    label: 'Contacts',
    Icon: UserIcon,
    description: 'Import contacts from Apple Contacts. Contacts without a display name are skipped.',
  },
]

function SourceToggle({ checked, onChange, disabled }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`relative inline-flex h-5 w-9 shrink-0 rounded-full border-2 border-transparent transition-colors duration-200 focus:outline-none ${
        disabled ? 'opacity-40 cursor-not-allowed' : 'cursor-pointer'
      } ${checked ? 'bg-accent' : 'bg-surface-secondary'}`}
    >
      <span
        className={`pointer-events-none inline-block h-4 w-4 rounded-full bg-white shadow transform transition-transform duration-200 ${
          checked ? 'translate-x-4' : 'translate-x-0'
        }`}
      />
    </button>
  )
}

function ProgressBar({ progress }) {
  return (
    <div className="w-full bg-surface-secondary rounded-full h-1.5 mt-2">
      <div
        className="bg-accent h-1.5 rounded-full transition-all duration-300"
        style={{ width: `${progress}%` }}
      />
    </div>
  )
}

function SourceCard({ source, enabled, onToggle, status, progress, message, result, photosLimit, onPhotosLimitChange }) {
  const isRunning = status === 'running'
  const isDone = status === 'done'
  const isError = status === 'error'

  return (
    <div className={`bg-surface border rounded-lg p-4 ${
      isError ? 'border-red-500/40' : 'border-border'
    }`}>
      <div className="flex items-center justify-between mb-1">
        <div className="flex items-center gap-2">
          <source.Icon aria-hidden="true" className="w-4 h-4 text-secondary" />
          <h3 className="text-sm font-medium text-primary">{source.label}</h3>
          {source.comingSoon && (
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-surface-secondary text-secondary">
              coming soon
            </span>
          )}
        </div>
        <SourceToggle
          checked={enabled}
          onChange={onToggle}
          disabled={source.comingSoon || isRunning}
        />
      </div>

      <p className="text-xs text-secondary mb-2">{source.description}</p>

      {source.hasLimit && enabled && !isRunning && !isDone && (
        <div className="flex items-center gap-2 mb-2">
          <label className="text-xs text-secondary">Limit:</label>
          <input
            type="number"
            min={1}
            max={500}
            value={photosLimit}
            onChange={(e) => onPhotosLimitChange(parseInt(e.target.value) || 50)}
            className="w-20 px-2 py-1 text-xs bg-surface-secondary border border-border rounded text-primary"
          />
          <span className="text-xs text-secondary">photos</span>
        </div>
      )}

      {isRunning && (
        <div>
          <ProgressBar progress={progress} />
          <p className="text-xs text-secondary mt-1">{message}</p>
        </div>
      )}

      {isDone && (
        <div className="flex items-center gap-2 mt-1">
          <span className="text-gruvbox-green text-xs">&#10003;</span>
          <span className="text-xs text-primary">{message}</span>
          {result && (
            <span className="text-xs text-secondary">
              ({result.total} total, {result.ingested} ingested)
            </span>
          )}
        </div>
      )}

      {isError && (
        <div className="flex items-center gap-2 mt-1">
          <span className="text-gruvbox-red text-xs">&#10007;</span>
          <span className="text-xs text-gruvbox-red">{message}</span>
        </div>
      )}
    </div>
  )
}

function useSourceImport(sourceKey, importFn) {
  const [progressId, setProgressId] = useState(null)
  const [status, setStatus] = useState('idle') // idle | running | done | error
  const [progress, setProgress] = useState(0)
  const [message, setMessage] = useState('')
  const [result, setResult] = useState(null)
  const pollRef = useRef(null)

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
    poll()
    return () => clearInterval(pollRef.current)
  }, [progressId, status])

  const start = useCallback(async () => {
    setStatus('running')
    setProgress(5)
    setMessage('Starting...')
    setResult(null)
    try {
      const resp = await importFn()
      if (resp.success && resp.data?.progress_id) {
        setProgressId(resp.data.progress_id)
      } else {
        throw new Error(resp.error?.message || `Failed to start ${sourceKey} import`)
      }
    } catch (e) {
      setStatus('error')
      setMessage(e.message || `Failed to start ${sourceKey} import`)
    }
  }, [importFn, sourceKey])

  const reset = useCallback(() => {
    setStatus('idle')
    setProgress(0)
    setMessage('')
    setResult(null)
    setProgressId(null)
  }, [])

  return { status, progress, message, result, start, reset }
}

export default function AppleImportTab({ onResult: _onResult }) {
  const [available, setAvailable] = useState(null) // null = loading, true/false
  const [enabled, setEnabled] = useState({ notes: true, photos: true, calendar: true, reminders: true, contacts: true })
  const [photosLimit, setPhotosLimit] = useState(50)

  useEffect(() => {
    ingestionClient.getAppleImportStatus().then((resp) => {
      setAvailable(resp.success && resp.data?.available)
    }).catch(() => {
      setAvailable(false)
    })
  }, [])

  const notes = useSourceImport('notes', useCallback(
    () => ingestionClient.appleImportNotes(), []
  ))
  const photos = useSourceImport('photos', useCallback(
    () => ingestionClient.appleImportPhotos(null, photosLimit), [photosLimit]
  ))
  const calendar = useSourceImport('calendar', useCallback(
    () => ingestionClient.appleImportCalendar(), []
  ))
  const reminders = useSourceImport('reminders', useCallback(
    () => ingestionClient.appleImportReminders(), []
  ))
  const contacts = useSourceImport('contacts', useCallback(
    () => ingestionClient.appleImportContacts(), []
  ))

  const imports = { notes, photos, calendar, reminders, contacts }

  const toggleSource = (key) => (val) => {
    setEnabled((prev) => ({ ...prev, [key]: val }))
  }

  const anyRunning = SOURCES.some((s) => imports[s.key].status === 'running')
  const enabledSources = SOURCES.filter((s) => enabled[s.key] && !s.comingSoon)
  const canImportAll = enabledSources.length > 0 && !anyRunning

  const handleImportAll = () => {
    for (const source of enabledSources) {
      const imp = imports[source.key]
      if (imp.status === 'idle' || imp.status === 'done' || imp.status === 'error') {
        imp.reset()
        // Small delay to ensure reset state propagates before start
        setTimeout(() => imp.start(), 0)
      }
    }
  }

  const anyDone = SOURCES.some((s) => imports[s.key].status === 'done')
  const anyError = SOURCES.some((s) => imports[s.key].status === 'error')

  const handleResetAll = () => {
    for (const source of SOURCES) {
      imports[source.key].reset()
    }
  }

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
        <div className="bg-surface border border-border rounded-lg p-6 text-center">
          <p className="text-secondary text-sm">
            Apple Import is only available on macOS.
          </p>
        </div>
      </div>
    )
  }

  return (
    <div className="p-4">
      <div className="flex items-center justify-between mb-4">
        <div>
          <h2 className="text-sm font-medium text-primary">Import My Data</h2>
          <p className="text-xs text-secondary mt-0.5">
            Toggle sources on/off, then import all at once. You may be prompted to grant access on first use.
          </p>
        </div>
      </div>

      <AutoSyncSettings />

      <div className="grid gap-3 mb-4">
        {SOURCES.map((source) => (
          <SourceCard
            key={source.key}
            source={source}
            enabled={enabled[source.key]}
            onToggle={toggleSource(source.key)}
            status={imports[source.key].status}
            progress={imports[source.key].progress}
            message={imports[source.key].message}
            result={imports[source.key].result}
            photosLimit={photosLimit}
            onPhotosLimitChange={setPhotosLimit}
          />
        ))}
      </div>

      <div className="flex items-center gap-3">
        <button
          onClick={handleImportAll}
          disabled={!canImportAll}
          className={`px-5 py-2 text-sm rounded font-medium transition-opacity ${
            canImportAll
              ? 'bg-accent text-white hover:opacity-90'
              : 'bg-surface-secondary text-secondary cursor-not-allowed'
          }`}
        >
          {anyRunning ? 'Importing...' : `Import All (${enabledSources.length})`}
        </button>

        {(anyDone || anyError) && !anyRunning && (
          <button
            onClick={handleResetAll}
            className="px-3 py-2 text-xs border border-border rounded hover:bg-surface-secondary transition-colors"
          >
            Reset All
          </button>
        )}
      </div>
    </div>
  )
}
