import { memo, useState, useEffect, useCallback, useMemo } from 'react'
import type { ChangeEvent, ComponentType, SVGProps } from 'react'
import {
  ArrowPathIcon,
  DocumentTextIcon,
  CameraIcon,
  CalendarDaysIcon,
  CheckCircleIcon,
  UserIcon,
} from '@heroicons/react/24/outline'
import type { OperationResultPayload } from '../../types/api'
import ingestionClient from '../../api/clients/ingestionClient'
import type { EnabledSources } from '../../api/clients/ingestionClient'
import { useAppDispatch, useAppSelector } from '../../store/hooks'
import {
  appleJobFailed,
  appleJobReset,
  appleJobStarted,
  appleJobStarting,
  appleSyncConfigOptimisticPatch,
  debouncedPatchAppleSyncConfig,
  fetchAppleSyncConfig,
  flushAppleSyncDebouncedPatch,
  selectAppleEnabledSources,
  selectAppleJob,
  selectApplePhotosLimit,
  selectAppleSyncConfig,
  selectAppleSyncConfigLoaded,
  type AppleSourceKey,
  type ImportResult,
} from '../../store/ingestionSlice'

function AutoSyncSettings() {
  const dispatch = useAppDispatch()
  const config = useAppSelector(selectAppleSyncConfig)
  const loaded = useAppSelector(selectAppleSyncConfigLoaded)
  const [customHours, setCustomHours] = useState(12)

  // Seed customHours from the loaded config the first time it lands so the
  // input shows whatever the backend persisted. Tracking this in component
  // state (instead of deriving from config) keeps the input editable while
  // the user is mid-typing without forcing a slice round-trip per keystroke.
  useEffect(() => {
    if (config && typeof config.schedule === 'object' && config.schedule?.custom) {
      setCustomHours(config.schedule.custom.hours)
    }
  }, [config])

  useEffect(() => {
    if (!config?.enabled) return
    const timer = setInterval(async () => {
      const resp = await ingestionClient.getAppleNextSync()
      if (resp.success && resp.data) {
        const { next_sync, last_sync, last_error, last_error_at } = resp.data
        // Status-only patch: do NOT go through the API-debouncer; this is
        // an inbound update that just refreshes the sidecar fields.
        dispatch(
          appleSyncConfigOptimisticPatch({
            next_sync,
            last_sync,
            last_error,
            last_error_at,
          }),
        )
      }
    }, 30000)
    return () => clearInterval(timer)
  }, [config?.enabled, dispatch])

  const handleToggle = () => {
    if (!config) return
    dispatch(debouncedPatchAppleSyncConfig({ enabled: !config.enabled }))
  }

  const handleScheduleChange = (e: ChangeEvent<HTMLSelectElement>) => {
    const val = e.target.value
    if (val === 'custom') {
      dispatch(
        debouncedPatchAppleSyncConfig({ schedule: { custom: { hours: customHours } } }),
      )
    } else {
      dispatch(
        debouncedPatchAppleSyncConfig({ schedule: val as 'daily' | 'weekly' }),
      )
    }
  }

  const handleCustomHoursChange = (e: ChangeEvent<HTMLInputElement>) => {
    const hours = parseInt(e.target.value) || 1
    setCustomHours(hours)
    dispatch(debouncedPatchAppleSyncConfig({ schedule: { custom: { hours } } }))
  }

  const handleSourceToggle = (source: keyof EnabledSources) => {
    if (!config) return
    dispatch(
      debouncedPatchAppleSyncConfig({
        sources: { ...config.sources, [source]: !config.sources[source] },
      }),
    )
  }

  const handlePhotosLimitChange = (e: ChangeEvent<HTMLInputElement>) => {
    const limit = parseInt(e.target.value) || 50
    dispatch(debouncedPatchAppleSyncConfig({ photos_limit: limit }))
  }

  const getScheduleValue = () => {
    if (!config) return 'daily'
    if (typeof config.schedule === 'string') return config.schedule
    if (config.schedule?.custom) return 'custom'
    return 'daily'
  }

  const formatTime = (isoStr: string | null) => {
    if (!isoStr) return 'Never'
    const d = new Date(isoStr)
    return d.toLocaleString()
  }

  if (!loaded) {
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
            {(['notes', 'reminders', 'photos', 'calendar', 'contacts'] as const).map((source) => (
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

interface SourceDef {
  key: keyof EnabledSources
  label: string
  Icon: ComponentType<SVGProps<SVGSVGElement> & { 'aria-hidden'?: boolean | string }>
  description: string
  hasLimit?: boolean
  comingSoon?: boolean
}

const SOURCES: SourceDef[] = [
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

interface SourceToggleProps {
  checked: boolean
  onChange: (val: boolean) => void
  disabled?: boolean
}

function SourceToggle({ checked, onChange, disabled }: SourceToggleProps) {
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

function ProgressBar({ progress }: { progress: number }) {
  return (
    <div className="w-full bg-surface-secondary rounded-full h-1.5 mt-2">
      <div
        className="bg-accent h-1.5 rounded-full transition-all duration-300"
        style={{ width: `${progress}%` }}
      />
    </div>
  )
}

type ImportStatus = 'idle' | 'running' | 'done' | 'error'

interface SourceCardProps {
  source: SourceDef
  enabled: boolean
  onToggle: (val: boolean) => void
  toggleDisabled: boolean
  status: ImportStatus
  progress: number
  message: string
  result: ImportResult | null
  photosLimit: number
  onPhotosLimitChange: (limit: number) => void
}

const SourceCard = memo(function SourceCard({ source, enabled, onToggle, toggleDisabled, status, progress, message, result, photosLimit, onPhotosLimitChange }: SourceCardProps) {
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
          disabled={source.comingSoon || isRunning || toggleDisabled}
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
        <div className="flex items-start gap-2 mt-1">
          <span className="text-gruvbox-red text-xs leading-snug">&#10007;</span>
          <span className="text-xs text-gruvbox-red break-words leading-snug">{message}</span>
        </div>
      )}
    </div>
  )
})

interface ImportFnResp {
  success: boolean
  data?: { progress_id?: string }
  error?: { message?: string } | string
}

function useSourceImport(sourceKey: AppleSourceKey, importFn: () => Promise<ImportFnResp>) {
  const dispatch = useAppDispatch()
  const job = useAppSelector(selectAppleJob(sourceKey))

  const start = useCallback(async () => {
    // Optimistic running marker BEFORE the POST resolves. Without
    // this the UI sat at `idle` (or whatever the previous run left
    // behind) for as long as the start POST was in flight, which
    // under backend load can be tens of seconds. See appleJobStarting
    // in ingestionSlice.ts for the dogfood-2026-05-05 repro.
    dispatch(appleJobStarting({ key: sourceKey }))
    try {
      const resp = await importFn()
      if (resp.success && resp.data?.progress_id) {
        dispatch(appleJobStarted({ key: sourceKey, progressId: resp.data.progress_id }))
        return
      }
      const errMsg = typeof resp.error === 'string' ? resp.error : resp.error?.message
      dispatch(appleJobFailed({
        key: sourceKey,
        message: errMsg || `Failed to start ${sourceKey} import`,
      }))
    } catch (e) {
      dispatch(appleJobFailed({
        key: sourceKey,
        message: (e instanceof Error ? e.message : null) || `Failed to start ${sourceKey} import`,
      }))
    }
  }, [dispatch, importFn, sourceKey])

  const reset = useCallback(() => {
    dispatch(appleJobReset({ key: sourceKey }))
  }, [dispatch, sourceKey])

  return {
    status: job.status,
    progress: job.progress,
    message: job.message,
    result: job.result,
    start,
    reset,
  }
}

interface AppleImportTabProps {
  onResult?: (result: OperationResultPayload) => void
}

export default function AppleImportTab({ onResult: _onResult }: AppleImportTabProps) {
  const dispatch = useAppDispatch()
  const [available, setAvailable] = useState<boolean | null>(null) // null = loading, true/false
  const enabled = useAppSelector(selectAppleEnabledSources)
  const photosLimit = useAppSelector(selectApplePhotosLimit)
  const syncConfigLoaded = useAppSelector(selectAppleSyncConfigLoaded)

  useEffect(() => {
    ingestionClient.getAppleImportStatus().then((resp) => {
      setAvailable(Boolean(resp.success && resp.data?.available))
    }).catch(() => {
      setAvailable(false)
    })
  }, [])

  // Hydrate the shared sync config the first time the tab mounts. Subsequent
  // mounts reuse the slice copy — this is what makes the toggles persist
  // across navigation away and back. AutoSyncSettings reads the same slice
  // so the two surfaces never disagree.
  useEffect(() => {
    if (!syncConfigLoaded) {
      dispatch(fetchAppleSyncConfig())
    }
  }, [dispatch, syncConfigLoaded])

  const notes = useSourceImport('notes', useCallback(
    () => ingestionClient.appleImportNotes() as unknown as Promise<ImportFnResp>, []
  ))
  const photos = useSourceImport('photos', useCallback(
    () => ingestionClient.appleImportPhotos(null as unknown as string | undefined, photosLimit) as unknown as Promise<ImportFnResp>, [photosLimit]
  ))
  const calendar = useSourceImport('calendar', useCallback(
    () => ingestionClient.appleImportCalendar() as unknown as Promise<ImportFnResp>, []
  ))
  const reminders = useSourceImport('reminders', useCallback(
    () => ingestionClient.appleImportReminders() as unknown as Promise<ImportFnResp>, []
  ))
  const contacts = useSourceImport('contacts', useCallback(
    () => ingestionClient.appleImportContacts() as unknown as Promise<ImportFnResp>, []
  ))

  const imports: Record<keyof EnabledSources, ReturnType<typeof useSourceImport>> = { notes, photos, calendar, reminders, contacts }

  // Stable per-source toggle callbacks. A fresh callback every render
  // would defeat React.memo on SourceCard — every card would re-render
  // on every progress tick because `onToggle` always changed identity.
  // Each handler dispatches a debounced patch to the slice; the optimistic
  // reducer flips the toggle synchronously so the UI is snappy, the
  // backend PATCH coalesces with any neighboring toggle within 500ms.
  const toggleHandlers = useMemo<Record<keyof EnabledSources, (val: boolean) => void>>(() => ({
    notes: (val) => dispatch(debouncedPatchAppleSyncConfig({ sources: { notes: val } })),
    photos: (val) => dispatch(debouncedPatchAppleSyncConfig({ sources: { photos: val } })),
    calendar: (val) => dispatch(debouncedPatchAppleSyncConfig({ sources: { calendar: val } })),
    reminders: (val) => dispatch(debouncedPatchAppleSyncConfig({ sources: { reminders: val } })),
    contacts: (val) => dispatch(debouncedPatchAppleSyncConfig({ sources: { contacts: val } })),
  }), [dispatch])

  const handlePhotosLimitChange = useCallback((limit: number) => {
    dispatch(debouncedPatchAppleSyncConfig({ photos_limit: limit }))
  }, [dispatch])

  // If the user kicks off "Import All" while a debounced patch is still
  // queued, flush it now — otherwise the latest toggle/limit edit could
  // miss the backend by ~500ms and the import would run with stale config.
  const flushPendingPatch = useCallback(() => {
    dispatch(flushAppleSyncDebouncedPatch())
  }, [dispatch])

  const anyRunning = SOURCES.some((s) => imports[s.key].status === 'running')
  const enabledSources = SOURCES.filter((s) => enabled[s.key] && !s.comingSoon)
  const canImportAll = enabledSources.length > 0 && !anyRunning && syncConfigLoaded

  const handleImportAll = () => {
    // If a debounced toggle/limit edit is still queued, flush it BEFORE
    // we start the imports — without this the user could click "Import
    // All" within 500ms of toggling Contacts off, the import would run
    // with the pre-toggle state, and Contacts would still get imported.
    flushPendingPatch()
    // Call start() directly. start() flips the job to `running` /
    // `Starting...` synchronously via appleJobStarting BEFORE the
    // POST is awaited, so there is never a window where the job is
    // visibly idle between user click and POST response. The earlier
    // `reset() → setTimeout(() → start())` hop produced exactly that
    // window and was the root of the dogfood-2026-05-05 idle-reset
    // observation.
    for (const source of enabledSources) {
      const imp = imports[source.key]
      if (imp.status !== 'running') {
        imp.start()
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
            onToggle={toggleHandlers[source.key]}
            toggleDisabled={!syncConfigLoaded}
            status={imports[source.key].status}
            progress={imports[source.key].progress}
            message={imports[source.key].message}
            result={imports[source.key].result}
            photosLimit={photosLimit}
            onPhotosLimitChange={handlePhotosLimitChange}
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
