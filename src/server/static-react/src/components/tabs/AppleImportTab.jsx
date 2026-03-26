import { useState, useEffect, useRef, useCallback } from 'react'
import ingestionClient from '../../api/clients/ingestionClient'

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
