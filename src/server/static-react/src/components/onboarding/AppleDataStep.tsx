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
}

function ImportProgress({ sourceId, progressId }: ImportProgressProps) {
  const [progress, setProgress] = useState(0)
  const [message, setMessage] = useState('Starting...')
  const [done, setDone] = useState(false)
  const [failed, setFailed] = useState(false)
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
            if (pollRef.current) clearInterval(pollRef.current)
          } else if (job.is_failed) {
            setFailed(true)
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
  }, [progressId])

  const source = SOURCES.find(s => s.id === sourceId)

  return (
    <div className="flex items-center gap-3 py-2">
      <span>{source?.icon}</span>
      <div className="flex-1">
        <div className="flex items-center gap-2 mb-1">
          <span className="text-xs text-primary">{source?.label}</span>
          {done && <span className="text-gruvbox-green text-xs">Done</span>}
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
type SourcePermissionMap = Record<string, boolean>

// Deep-link to the macOS Privacy & Security → Automation pane. Tauri's
// shell.open is permitted for `x-apple.systempreferences:` schemes, and a
// regular browser will prompt the user to launch System Settings on macOS.
// Anywhere else it's a harmless no-op (browser shrugs).
const AUTOMATION_SETTINGS_URL =
  'x-apple.systempreferences:com.apple.preference.security?Privacy_Automation'

export default function AppleDataStep({ onNext, onSkip }: AppleDataStepProps) {
  const [available, setAvailable] = useState<boolean | null>(null)
  const [enabled, setEnabled] = useState<SourceEnabledMap>({ notes: true, reminders: true, photos: true, calendar: true, contacts: true })
  const [importing, setImporting] = useState(false)
  const [progressIds, setProgressIds] = useState<Record<string, string>>({})
  const [failedSources, setFailedSources] = useState<Record<string, string>>({})
  const [allDone, setAllDone] = useState(false)
  const [photosLimit] = useState(50)
  const [permissions, setPermissions] = useState<SourcePermissionMap | null>(null)
  const [permissionsChecking, setPermissionsChecking] = useState(false)

  // After the wizard knows it's on macOS, run the per-source TCC pre-flight
  // so we can warn before the user clicks Import — otherwise contacts (and
  // any other source missing Automation) would wedge the import for ~30s
  // before surfacing the same "Grant access" message we surface up front.
  // `permissions === null` (probe failed entirely) intentionally falls
  // through to the legacy "click and find out" path; that's safer than
  // gating the user out of imports because a probe call dropped.
  const refreshPermissions = useCallback(async () => {
    setPermissionsChecking(true)
    try {
      const resp = await ingestionClient.getAppleImportPermissions()
      if (resp.success && resp.data) {
        setPermissions(resp.data as SourcePermissionMap)
      } else {
        setPermissions(null)
      }
    } catch {
      setPermissions(null)
    } finally {
      setPermissionsChecking(false)
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    ingestionClient.getAppleImportStatus()
      .then(async resp => {
        if (cancelled) return
        const isAvail = !!(resp.success && resp.data?.available)
        setAvailable(isAvail)
        if (isAvail) {
          await refreshPermissions()
        }
      })
      .catch(() => {
        if (!cancelled) setAvailable(false)
      })
    return () => { cancelled = true }
  }, [refreshPermissions])

  const handleToggle = useCallback((id: string, checked: boolean) => {
    setEnabled(prev => ({ ...prev, [id]: checked }))
  }, [])

  // Selected sources whose probe came back `false`. Used both to render the
  // pre-import banner and to short-circuit a click on Import Selected
  // (re-checking once before deciding — the user may have granted access
  // since mount).
  const missingSelectedPermissions = useCallback(
    (perms: SourcePermissionMap | null): string[] => {
      if (!perms) return []
      return Object.entries(enabled)
        .filter(([id, on]) => on && perms[id] === false)
        .map(([id]) => id)
    },
    [enabled],
  )

  const handleOpenSettings = useCallback(() => {
    // window.open is harmless on non-macOS browsers; on macOS the OS
    // intercepts the x-apple.systempreferences scheme. We deliberately do
    // NOT block on this returning — there's no callback signal the user
    // actually granted access; they have to click Retry / Refresh.
    window.open(AUTOMATION_SETTINGS_URL, '_blank')
  }, [])

  const handleImportAll = async () => {
    // Re-probe just before kicking off; the user may have granted access
    // between mount and clicking Import. If anything's still missing,
    // surface the inline error and bail before spawning any background
    // jobs — that's the whole point of the pre-flight.
    const fresh = await ingestionClient
      .getAppleImportPermissions()
      .then(r => (r.success && r.data ? (r.data as SourcePermissionMap) : null))
      .catch(() => null)
    if (fresh) setPermissions(fresh)

    const blockers = missingSelectedPermissions(fresh ?? permissions)
    if (blockers.length > 0) {
      // Render the banner the user already saw (or now sees) and bail.
      // We do NOT mark `importing=true` because there's nothing in
      // progress to monitor.
      return
    }

    setImporting(true)
    setFailedSources({})
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
        All data stays on your device. You may be prompted for permission on first use.
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

          {(() => {
            const blockers = missingSelectedPermissions(permissions)
            if (blockers.length === 0) return null
            const blockerLabels = blockers
              .map(id => SOURCES.find(s => s.id === id)?.label ?? id)
              .join(', ')
            return (
              <div
                role="alert"
                data-testid="apple-permissions-banner"
                className="card p-4 mt-3 border border-gruvbox-yellow"
              >
                <div className="text-sm text-gruvbox-yellow font-medium mb-1">
                  Grant Apple permissions before importing
                </div>
                <p className="text-xs text-primary mb-2">
                  These selected sources can't be reached yet: {blockerLabels}.
                  Grant access in System Settings → Privacy &amp; Security →
                  Automation (and Full Disk Access for Photos), then refresh.
                </p>
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={handleOpenSettings}
                    className="btn-primary text-xs px-3 py-1"
                  >
                    Open System Settings
                  </button>
                  <button
                    type="button"
                    onClick={refreshPermissions}
                    disabled={permissionsChecking}
                    data-testid="apple-permissions-refresh"
                    className="btn-secondary text-xs px-3 py-1"
                  >
                    {permissionsChecking ? 'Checking...' : 'Refresh'}
                  </button>
                </div>
              </div>
            )
          })()}

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
        </>
      ) : (
        <>
          {Object.keys(progressIds).length > 0 && (
            <div className="card p-4 space-y-1">
              {Object.entries(progressIds).map(([sourceId, pid]) => (
                <ImportProgress key={sourceId} sourceId={sourceId} progressId={pid} />
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
