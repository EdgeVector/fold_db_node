import { useState, useEffect, useRef, useCallback } from 'react'
import ingestionClient from '../../api/clients/ingestionClient'

interface AppleSource {
  id: string
  label: string
  icon: string
  description: string
}

const SOURCES: AppleSource[] = [
  { id: 'notes', label: 'Apple Notes', icon: '📝', description: 'Import all notes from Apple Notes' },
  { id: 'reminders', label: 'Apple Reminders', icon: '✅', description: 'Import all reminders including completed items' },
  { id: 'photos', label: 'Apple Photos', icon: '📷', description: 'Export and import photos (HEIC converted to JPEG)' },
  { id: 'calendar', label: 'Apple Calendar', icon: '📅', description: 'Import events from Apple Calendar' },
  { id: 'contacts', label: 'Apple Contacts', icon: '👤', description: 'Import contacts from Apple Contacts' },
]

// Deep-link to the macOS Privacy & Security → Automation pane. Tauri's
// shell.open is permitted for `x-apple.systempreferences:` schemes, and a
// regular browser will prompt the user to launch System Settings on macOS.
// Anywhere else it's a harmless no-op (browser shrugs).
const AUTOMATION_SETTINGS_URL =
  'x-apple.systempreferences:com.apple.preference.security?Privacy_Automation'

const openAutomationSettings = () => {
  // Per-source extract paths emit "Grant access in System Settings →
  // Privacy & Security → Automation" inside their error messages when the
  // TCC probe fails. Surfacing this button next to the failure list saves
  // the user from copy-pasting the path out of the error text.
  window.open(AUTOMATION_SETTINGS_URL, '_blank')
}

interface SourceToggleProps {
  source: AppleSource
  enabled: boolean
  onToggle: (id: string, checked: boolean) => void
}

function SourceToggle({ source, enabled, onToggle }: SourceToggleProps) {
  return (
    <label className="flex items-center gap-3 py-3 px-4 bg-surface-secondary border border-border cursor-pointer hover:border-gruvbox-yellow transition-colors">
      <input
        type="checkbox"
        checked={enabled}
        onChange={(e) => onToggle(source.id, e.target.checked)}
        className="w-4 h-4 accent-gruvbox-green"
      />
      <span className="text-lg">{source.icon}</span>
      <div className="flex-1">
        <div className="text-sm text-primary font-medium">{source.label}</div>
        <div className="text-xs text-secondary">{source.description}</div>
      </div>
    </label>
  )
}

interface ImportProgressProps {
  sourceId: string
  progressId: string
  onFailedChange: (sourceId: string, failed: boolean) => void
}

interface AppleImportPayload {
  source?: string
  total?: number
  ingested?: number
}

function ImportProgress({ sourceId, progressId, onFailedChange }: ImportProgressProps) {
  const [progress, setProgress] = useState(0)
  const [message, setMessage] = useState('Starting...')
  const [done, setDone] = useState(false)
  const [failed, setFailed] = useState(false)
  const [results, setResults] = useState<AppleImportPayload | null>(null)
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)

  useEffect(() => {
    if (!progressId) return

    const poll = async () => {
      try {
        const resp = await ingestionClient.getJobProgress(progressId)
        if (resp.success && resp.data) {
          const job = resp.data as typeof resp.data & { message?: string }
          setProgress(job.progress_percentage || 0)
          setMessage(job.status_message || job.message || '')
          if (job.is_complete) {
            setDone(true)
            // Backend publishes `{source, total, ingested}` on successful
            // apple-import completions (see src/ingestion/progress.rs). Falling
            // back to status_message would mean parsing "Imported N notes" by
            // string, which is exactly the brittle UI path this field exists
            // to retire.
            if (job.results && typeof job.results === 'object') {
              setResults(job.results as AppleImportPayload)
            }
            if (pollRef.current) clearInterval(pollRef.current)
          } else if (job.is_failed) {
            setFailed(true)
            onFailedChange(sourceId, true)
            setMessage(job.error_message || job.message || 'Import failed')
            if (pollRef.current) clearInterval(pollRef.current)
          }
        }
      } catch {
        // keep polling
      }
    }

    pollRef.current = setInterval(poll, 2000)
    poll()
    return () => { if (pollRef.current) clearInterval(pollRef.current) }
  }, [progressId, sourceId, onFailedChange])

  const source = SOURCES.find(s => s.id === sourceId)
  const importedCount = results?.ingested

  return (
    <div className="flex items-center gap-3 py-2">
      <span>{source?.icon}</span>
      <div className="flex-1">
        <div className="flex items-center gap-2 mb-1">
          <span className="text-xs text-primary">{source?.label}</span>
          {done && <span className="text-gruvbox-green text-xs">Done</span>}
          {done && typeof importedCount === 'number' && (
            <span className="text-xs text-secondary" data-testid="apple-import-count">
              ({importedCount} imported{typeof results?.total === 'number' ? ` of ${results.total}` : ''})
            </span>
          )}
          {failed && <span className="text-gruvbox-red text-xs">Failed</span>}
        </div>
        {!done && !failed && (
          <div className="w-full bg-surface-secondary rounded-full h-1.5">
            <div
              className="bg-gruvbox-green h-1.5 rounded-full transition-all duration-300"
              style={{ width: `${progress}%` }}
            />
          </div>
        )}
        <p className="text-xs text-secondary mt-0.5">{message}</p>
      </div>
    </div>
  )
}

interface AppleDataStepProps {
  onNext: () => void
  onSkip: () => void
}

type SourceEnabledMap = Record<string, boolean>

export default function AppleDataStep({ onNext, onSkip }: AppleDataStepProps) {
  const [available, setAvailable] = useState<boolean | null>(null)
  const [enabled, setEnabled] = useState<SourceEnabledMap>({ notes: true, reminders: true, photos: true, calendar: true, contacts: true })
  const [importing, setImporting] = useState(false)
  const [progressIds, setProgressIds] = useState<Record<string, string>>({})
  const [failedSources, setFailedSources] = useState<Record<string, string>>({})
  const [jobFailedSources, setJobFailedSources] = useState<Record<string, boolean>>({})
  const [allDone, setAllDone] = useState(false)
  const [photosLimit] = useState(50)

  useEffect(() => {
    let cancelled = false
    ingestionClient.getAppleImportStatus()
      .then(resp => {
        if (cancelled) return
        setAvailable(!!(resp.success && resp.data?.available))
      })
      .catch(() => {
        if (!cancelled) setAvailable(false)
      })
    return () => { cancelled = true }
  }, [])

  const handleToggle = useCallback((id: string, checked: boolean) => {
    setEnabled(prev => ({ ...prev, [id]: checked }))
  }, [])

  const handleJobFailedChange = useCallback((sourceId: string, failed: boolean) => {
    setJobFailedSources(prev => (prev[sourceId] === failed ? prev : { ...prev, [sourceId]: failed }))
  }, [])

  const handleImportAll = async () => {
    setImporting(true)
    setFailedSources({})
    setJobFailedSources({})
    const ids: Record<string, string> = {}
    const failures: Record<string, string> = {}
    const selected = Object.entries(enabled).filter(([, v]) => v).map(([k]) => k)

    for (const source of selected) {
      try {
        let resp: Awaited<ReturnType<typeof ingestionClient.appleImportNotes>> | undefined
        if (source === 'notes') resp = await ingestionClient.appleImportNotes()
        else if (source === 'reminders') resp = await ingestionClient.appleImportReminders()
        else if (source === 'photos') resp = await ingestionClient.appleImportPhotos(undefined, photosLimit)
        else if (source === 'calendar') resp = await ingestionClient.appleImportCalendar()
        else if (source === 'contacts') resp = await ingestionClient.appleImportContacts()

        if (resp?.success && resp.data?.progress_id) {
          ids[source] = resp.data.progress_id
        } else {
          failures[source] = resp?.error || resp?.message || 'Import failed to start'
        }
      } catch (err) {
        failures[source] = err instanceof Error && err.message ? err.message : 'Import failed to start'
      }
    }

    setProgressIds(ids)
    setFailedSources(failures)

    // Only auto-advance when there's truly nothing to do (no started imports
    // AND no failures — e.g. user deselected everything and somehow got here).
    // If everything failed, leave allDone=false so the wizard surfaces the
    // failure list and offers Retry. "No silent failures."
    if (Object.keys(ids).length === 0 && Object.keys(failures).length === 0) {
      setAllDone(true)
    }
  }

  const handleRetry = () => {
    setFailedSources({})
    setJobFailedSources({})
    setProgressIds({})
    setAllDone(false)
    handleImportAll()
  }

  useEffect(() => {
    if (!importing || Object.keys(progressIds).length === 0) return

    const checkAll = async () => {
      let allComplete = true
      for (const pid of Object.values(progressIds)) {
        try {
          const resp = await ingestionClient.getJobProgress(pid)
          if (resp.success && resp.data) {
            if (!resp.data.is_complete && !resp.data.is_failed) {
              allComplete = false
            }
          }
        } catch {
          allComplete = false
        }
      }
      if (allComplete) setAllDone(true)
    }

    const interval = setInterval(checkAll, 3000)
    return () => clearInterval(interval)
  }, [importing, progressIds])

  const anyEnabled = Object.values(enabled).some(v => v)
  const anyJobFailed = Object.values(jobFailedSources).some(v => v)

  if (available === null) {
    return <p className="text-secondary text-center py-6">Checking Apple data availability...</p>
  }

  if (!available) {
    return (
      <div>
        <h2 className="text-sm font-bold mb-1">
          <span className="text-gruvbox-blue">APPLE DATA</span>{' '}
          <span className="text-secondary">Connect your data</span>
        </h2>
        <div className="card p-6 text-center mt-4">
          <p className="text-secondary text-sm">
            Apple Import is only available on macOS. You can import data later from the Apple Import tab.
          </p>
        </div>
        <div className="flex gap-2 mt-4">
          <button onClick={onSkip} className="btn-primary flex-1 text-center">Continue</button>
        </div>
      </div>
    )
  }

  return (
    <div>
      <h2 className="text-sm font-bold mb-1">
        <span className="text-gruvbox-blue">APPLE DATA</span>{' '}
        <span className="text-secondary">Connect your data sources</span>
      </h2>
      <p className="text-primary mb-1">Import data from your macOS apps into FoldDB.</p>
      <p className="text-xs text-secondary mb-4">
        All data stays on your device. You may be prompted for permission on first use — if an
        import reports "Grant access in System Settings → Privacy & Security → Automation",
        use the button below to jump straight there.
      </p>

      {!importing ? (
        <>
          <div className="space-y-2">
            {SOURCES.map(source => (
              <SourceToggle
                key={source.id}
                source={source}
                enabled={enabled[source.id]}
                onToggle={handleToggle}
              />
            ))}
          </div>

          <div className="flex gap-2 mt-4">
            <button
              onClick={handleImportAll}
              disabled={!anyEnabled}
              className="btn-primary flex-1 text-center"
            >
              Import Selected
            </button>
            <button onClick={onSkip} className="btn-secondary flex-1 text-center">
              Skip
            </button>
          </div>
          <div className="mt-3 text-center">
            <button
              type="button"
              onClick={openAutomationSettings}
              data-testid="apple-permissions-deeplink"
              className="text-xs text-secondary bg-transparent border-none cursor-pointer hover:underline"
            >
              Open System Settings → Privacy &amp; Security → Automation
            </button>
          </div>
        </>
      ) : (
        <>
          {Object.keys(progressIds).length > 0 && (
            <div className="card p-4 space-y-1">
              {Object.entries(progressIds).map(([sourceId, pid]) => (
                <ImportProgress
                  key={sourceId}
                  sourceId={sourceId}
                  progressId={pid}
                  onFailedChange={handleJobFailedChange}
                />
              ))}
            </div>
          )}

          {Object.keys(failedSources).length > 0 && (
            <div
              role="alert"
              data-testid="apple-import-failures"
              className="card p-4 mt-3 border border-gruvbox-red"
            >
              <div className="text-sm text-gruvbox-red font-medium mb-2">
                {Object.keys(progressIds).length === 0
                  ? 'Import failed to start.'
                  : 'Some imports failed to start.'}
              </div>
              <ul className="space-y-1">
                {Object.entries(failedSources).map(([sourceId, message]) => {
                  const source = SOURCES.find(s => s.id === sourceId)
                  return (
                    <li key={sourceId} className="text-xs text-primary flex items-start gap-2">
                      <span>{source?.icon}</span>
                      <span className="flex-1">
                        <span className="font-medium">{source?.label || sourceId}</span>
                        <span className="text-secondary"> — {message}</span>
                      </span>
                    </li>
                  )
                })}
              </ul>
            </div>
          )}

          {(Object.keys(failedSources).length > 0 || anyJobFailed) && (
            <div className="mt-3 text-center">
              <button
                type="button"
                onClick={openAutomationSettings}
                data-testid="apple-permissions-deeplink"
                className="btn-secondary text-xs px-3 py-1"
              >
                Open System Settings → Privacy &amp; Security → Automation
              </button>
            </div>
          )}

          {Object.keys(progressIds).length === 0 && Object.keys(failedSources).length > 0 ? (
            <div className="flex gap-2 mt-4">
              <button onClick={handleRetry} className="btn-primary flex-1 text-center">
                Retry
              </button>
              <button onClick={onSkip} className="btn-secondary flex-1 text-center">
                Skip
              </button>
            </div>
          ) : allDone ? (
            <div className="flex gap-2 mt-4">
              <button onClick={onNext} className="btn-primary flex-1 text-center">
                Continue
              </button>
            </div>
          ) : (
            <p className="text-xs text-secondary mt-3 text-center">
              Importing... you can continue and imports will finish in the background.
              <button onClick={onNext} className="text-gruvbox-blue ml-2 bg-transparent border-none cursor-pointer text-xs hover:underline">
                Skip ahead
              </button>
            </p>
          )}
        </>
      )}
    </div>
  )
}
