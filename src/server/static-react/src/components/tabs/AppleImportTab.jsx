import { useState, useEffect, useRef, useCallback } from 'react'
import ingestionClient from '../../api/clients/ingestionClient'

const SOURCES = [
  {
    key: 'notes',
    label: 'Notes',
    icon: "\uD83D\uDCDD",
    description: 'Import all notes from Apple Notes. Notes shorter than 20 characters are skipped.',
  },
  {
    key: 'photos',
    label: 'Photos',
    icon: "\uD83D\uDCF7",
    description: 'Export and import photos from Apple Photos. HEIC files are converted to JPEG.',
    hasLimit: true,
  },
  {
    key: 'calendar',
    label: 'Calendar',
    icon: "\uD83D\uDCC5",
    description: 'Import events from Apple Calendar.',
    comingSoon: true,
  },
  {
    key: 'reminders',
    label: 'Reminders',
    icon: "\u2705",
    description: 'Import all reminders from Apple Reminders, including completed items.',
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
    <div className={`bg-surface-primary border rounded-lg p-4 ${
      isError ? 'border-red-500/40' : 'border-border'
    }`}>
      <div className="flex items-center justify-between mb-1">
        <div className="flex items-center gap-2">
          <span className="text-base">{source.icon}</span>
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
          <span className="text-green-500 text-xs">&#10003;</span>
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
          <span className="text-red-500 text-xs">&#10007;</span>
          <span className="text-xs text-red-400">{message}</span>
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

export default function AppleImportTab({ onResult }) {
  const [available, setAvailable] = useState(null) // null = loading, true/false
  const [enabled, setEnabled] = useState({ notes: true, photos: true, calendar: false, reminders: true })
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

  const imports = { notes, photos, calendar, reminders }

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
        <div className="bg-surface-primary border border-border rounded-lg p-6 text-center">
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
