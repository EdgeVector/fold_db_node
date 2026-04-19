import { useState, useEffect, useRef, useCallback } from 'react'
import ingestionClient from '../../api/clients/ingestionClient'

const SOURCES = [
  { id: 'notes', label: 'Apple Notes', icon: '\uD83D\uDCDD', description: 'Import all notes from Apple Notes' },
  { id: 'reminders', label: 'Apple Reminders', icon: '\u2705', description: 'Import all reminders including completed items' },
  { id: 'photos', label: 'Apple Photos', icon: '\uD83D\uDCF7', description: 'Export and import photos (HEIC converted to JPEG)' },
  { id: 'calendar', label: 'Apple Calendar', icon: '\uD83D\uDCC5', description: 'Import events from Apple Calendar' },
  { id: 'contacts', label: 'Apple Contacts', icon: '\uD83D\uDC64', description: 'Import contacts from Apple Contacts' },
]

function SourceToggle({ source, enabled, onToggle }) {
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

function ImportProgress({ sourceId, progressId }) {
  const [progress, setProgress] = useState(0)
  const [message, setMessage] = useState('Starting...')
  const [done, setDone] = useState(false)
  const [failed, setFailed] = useState(false)
  const pollRef = useRef(null)

  useEffect(() => {
    if (!progressId) return

    const poll = async () => {
      try {
        const resp = await ingestionClient.getJobProgress(progressId)
        if (resp.success && resp.data) {
          const job = resp.data
          setProgress(job.progress_percentage || 0)
          setMessage(job.status_message || job.message || '')
          if (job.is_complete) {
            setDone(true)
            clearInterval(pollRef.current)
          } else if (job.is_failed) {
            setFailed(true)
            setMessage(job.error_message || job.message || 'Import failed')
            clearInterval(pollRef.current)
          }
        }
      } catch {
        // keep polling
      }
    }

    pollRef.current = setInterval(poll, 2000)
    poll()
    return () => clearInterval(pollRef.current)
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

export default function AppleDataStep({ onNext, onSkip }) {
  const [available, setAvailable] = useState(null)
  const [enabled, setEnabled] = useState({ notes: true, reminders: true, photos: true, calendar: true, contacts: true })
  const [importing, setImporting] = useState(false)
  const [progressIds, setProgressIds] = useState({})
  const [allDone, setAllDone] = useState(false)
  const [photosLimit] = useState(50)

  useEffect(() => {
    ingestionClient.getAppleImportStatus()
      .then(resp => setAvailable(resp.success && resp.data?.available))
      .catch(() => setAvailable(false))
  }, [])

  const handleToggle = useCallback((id, checked) => {
    setEnabled(prev => ({ ...prev, [id]: checked }))
  }, [])

  const handleImportAll = async () => {
    setImporting(true)
    const ids = {}
    const selected = Object.entries(enabled).filter(([, v]) => v).map(([k]) => k)

    for (const source of selected) {
      try {
        let resp
        if (source === 'notes') resp = await ingestionClient.appleImportNotes()
        else if (source === 'reminders') resp = await ingestionClient.appleImportReminders()
        else if (source === 'photos') resp = await ingestionClient.appleImportPhotos(null, photosLimit)
        else if (source === 'calendar') resp = await ingestionClient.appleImportCalendar()
        else if (source === 'contacts') resp = await ingestionClient.appleImportContacts()

        if (resp?.success && resp.data?.progress_id) {
          ids[source] = resp.data.progress_id
        }
      } catch {
        // Skip failed starts
      }
    }

    setProgressIds(ids)

    // Poll until all are done (simplified — real completion tracked per-source)
    if (Object.keys(ids).length === 0) {
      setAllDone(true)
    }
  }

  // Check if all imports are complete by polling
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
          <div className="card p-4 space-y-1">
            {Object.entries(progressIds).map(([sourceId, pid]) => (
              <ImportProgress key={sourceId} sourceId={sourceId} progressId={pid} />
            ))}
          </div>

          {allDone ? (
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
