import { useState, useEffect } from 'react'
import IngestionReport from '../../IngestionReport'
import { fmtCost } from '../../../utils/formatCost'
import type { BatchStatusResponse } from '../../../api/clients/ingestionClient'
import type { FileProgress } from '../../../hooks/useFilesProgress'

interface FailedFile {
  name: string
  error: string
}

interface BatchStatusLike extends BatchStatusResponse {
  failed_files?: FailedFile[]
  is_local_provider?: boolean
}

interface FailedFilesBadgeProps {
  count: number
  files?: FailedFile[]
}

/** Clickable "(N failed)" badge that expands to show failed file details */
function FailedFilesBadge({ count, files }: FailedFilesBadgeProps) {
  const [expanded, setExpanded] = useState(false)
  if (count === 0) return null
  return (
    <>
      <button
        onClick={() => setExpanded(prev => !prev)}
        className="text-gruvbox-red underline cursor-pointer hover:opacity-80"
      >
        ({count} failed)
      </button>
      {expanded && files && files.length > 0 && (
        <div className="mt-2 bg-surface-secondary border border-gruvbox-red/30 rounded p-2 text-xs space-y-1 max-h-40 overflow-y-auto">
          {files.map((f: FailedFile, i: number) => (
            <div key={i} className="flex flex-col">
              <span className="font-mono text-primary">{f.name}</span>
              <span className="text-gruvbox-red ml-2">{f.error}</span>
            </div>
          ))}
        </div>
      )}
    </>
  )
}

interface FileRowProps {
  file: FileProgress
}

/** Per-file row showing status pill + progress bar + current step. */
function FileRow({ file }: FileRowProps) {
  const status = file.is_failed
    ? 'failed'
    : file.is_complete
      ? 'completed'
      : file.progress_percentage > 0
        ? 'running'
        : 'queued'
  const pillClass =
    status === 'failed'
      ? 'bg-gruvbox-red/20 text-gruvbox-red'
      : status === 'completed'
        ? 'bg-gruvbox-green/20 text-gruvbox-green'
        : status === 'running'
          ? 'bg-gruvbox-blue/20 text-gruvbox-blue'
          : 'bg-border text-secondary'
  const barClass =
    status === 'failed'
      ? 'bg-gruvbox-red'
      : status === 'completed'
        ? 'bg-gruvbox-green'
        : 'bg-gruvbox-blue'
  const pct = file.is_complete ? 100 : file.progress_percentage
  return (
    <div className="bg-surface-secondary border border-border rounded p-2 space-y-1">
      <div className="flex items-center justify-between gap-2">
        <span
          className="text-xs font-mono text-primary truncate flex-1"
          title={file.file_name}
        >
          {file.file_name}
        </span>
        <span className={`text-[10px] uppercase font-semibold rounded px-1.5 py-0.5 ${pillClass}`}>
          {status}
        </span>
        <span className="text-xs text-secondary tabular-nums">{pct}%</span>
      </div>
      <div className="w-full bg-border rounded-full h-1 overflow-hidden">
        <div
          className={`h-full transition-all duration-300 ${barClass}`}
          style={{ width: `${pct}%` }}
        />
      </div>
      {file.is_failed && file.error_message ? (
        <p className="text-xs text-gruvbox-red truncate" title={file.error_message}>
          {file.error_message}
        </p>
      ) : (
        file.current_step && (
          <p className="text-xs text-secondary truncate">{file.current_step}</p>
        )
      )}
    </div>
  )
}

interface FilesProgressListProps {
  files: FileProgress[]
}

function FilesProgressList({ files }: FilesProgressListProps) {
  if (files.length === 0) return null
  return (
    <div className="space-y-1.5 max-h-64 overflow-y-auto">
      {files.map((f) => (
        <FileRow key={f.progress_id} file={f} />
      ))}
    </div>
  )
}

interface BatchProgressViewProps {
  batchStatus: BatchStatusLike | null | undefined
  batchReport: unknown
  filesProgress: FileProgress[]
  allFilesDone: boolean
  hasFirstPoll: boolean
  onResume: (limit: number) => void
  onCancel: () => void
  onBack: () => void
  onDismissReport: () => void
  isIngesting: boolean
}

/**
 * Displays batch ingestion progress: running, paused, completed, cancelled, failed states.
 *
 * The "Ingestion complete" terminal display is gated on `allFilesDone` — every
 * per-file progress job must report `is_complete` or `is_failed`. This avoids
 * the prior bug where the batch-level `Completed` flag could fire before the
 * per-file progress jobs had persisted, producing a fake-success UI moments
 * after Proceed was clicked.
 */
export default function BatchProgressView({
  batchStatus,
  batchReport,
  filesProgress,
  allFilesDone,
  hasFirstPoll,
  onResume,
  onCancel,
  onBack,
  onDismissReport,
  isIngesting,
}: BatchProgressViewProps) {
  const [newLimit, setNewLimit] = useState('')

  // Pre-fill new limit when paused
  useEffect(() => {
    if (batchStatus?.status === 'Paused') {
      const suggested = batchStatus.accumulated_cost + batchStatus.estimated_remaining_cost
      setNewLimit(suggested.toFixed(2))
    }
  }, [batchStatus?.status, batchStatus?.accumulated_cost, batchStatus?.estimated_remaining_cost])

  const isRunning = batchStatus?.status === 'Running'
  const isPaused = batchStatus?.status === 'Paused'
  const isCompleted = batchStatus?.status === 'Completed'
  const isCancelled = batchStatus?.status === 'Cancelled'
  const isFailed = batchStatus?.status === 'Failed'
  const isBatchTerminal = isCompleted || isCancelled || isFailed
  // Only show the "Ingestion complete" final state once every per-file
  // progress job has actually finished (or failed). For very fast providers
  // the batch-level flag can flip before per-file results have persisted.
  const showFinalState = isBatchTerminal && allFilesDone

  const handleResume = () => {
    const limit = parseFloat(newLimit)
    if (isNaN(limit) || limit <= 0) return
    onResume(limit)
  }

  // Running OR (terminal but still finalizing per-file progress)
  if ((isRunning || (isBatchTerminal && !allFilesDone)) && batchStatus) {
    const finalizing = isBatchTerminal && !allFilesDone
    return (
      <div className="space-y-3">
        <p className="text-sm font-medium">
          {finalizing ? 'Finalizing per-file progress...' : 'Ingesting files...'}
        </p>
        <div className="w-full bg-border rounded-full h-2 overflow-hidden">
          <div
            className="h-full bg-primary transition-all duration-300"
            style={{ width: `${batchStatus.files_total > 0 ? Math.round((batchStatus.files_completed / batchStatus.files_total) * 100) : 0}%` }}
          />
        </div>
        <div className="flex items-center justify-between text-sm text-secondary">
          <span>{batchStatus.files_completed}/{batchStatus.files_total} files {batchStatus.files_failed > 0 && <FailedFilesBadge count={batchStatus.files_failed} files={batchStatus.failed_files} />}</span>
          {batchStatus.is_local_provider ? (
            <span>Free (local)</span>
          ) : (
            <span>≈ {fmtCost(batchStatus.accumulated_cost)} estimated{batchStatus.spend_limit != null ? ` / ${fmtCost(batchStatus.spend_limit ?? 0)} limit` : ''}</span>
          )}
        </div>
        <FilesProgressList files={filesProgress} />
        <div className="flex justify-end gap-2">
          {!finalizing && <button onClick={onCancel} className="btn-secondary">Cancel</button>}
          <button onClick={onBack} className="btn-secondary">Scan Another</button>
        </div>
      </div>
    )
  }

  // Paused
  if (isPaused && batchStatus) {
    return (
      <div className="space-y-3">
        <p className="text-sm font-medium text-gruvbox-yellow">Paused -- spend limit reached</p>
        <div className="w-full bg-border rounded-full h-2 overflow-hidden">
          <div
            className="h-full bg-gruvbox-yellow transition-all duration-300"
            style={{ width: `${batchStatus.files_total > 0 ? Math.round((batchStatus.files_completed / batchStatus.files_total) * 100) : 0}%` }}
          />
        </div>
        <div className="flex items-center justify-between text-sm text-secondary">
          <span>{batchStatus.files_completed}/{batchStatus.files_total} files</span>
          <span>≈ {fmtCost(batchStatus.accumulated_cost)} estimated / {fmtCost(batchStatus.spend_limit ?? 0)} limit</span>
        </div>
        <p className="text-sm text-secondary">
          {batchStatus.files_remaining} files remaining (~{fmtCost(batchStatus.estimated_remaining_cost)} to finish)
        </p>
        <FilesProgressList files={filesProgress} />
        <div className="flex items-center gap-3">
          <label className="flex items-center gap-2 text-sm text-secondary">
            New limit:
            <input
              type="text"
              value={newLimit}
              onChange={(e) => setNewLimit(e.target.value)}
              className="input w-24 text-sm"
            />
          </label>
          <button onClick={handleResume} className="btn-primary">Resume</button>
          <button onClick={onCancel} className="btn-secondary">Stop</button>
        </div>
      </div>
    )
  }

  // Completed / Cancelled / Failed (gated on all per-file jobs done)
  if (showFinalState && batchStatus) {
    // Real counts come from per-file results, not the batch-level estimate.
    const realCompleted = filesProgress.filter((f) => f.is_complete).length
    const realFailed = filesProgress.filter((f) => f.is_failed).length
    return (
      <div className="space-y-3">
        <p className="text-sm font-medium">
          {isCompleted && 'Ingestion complete'}
          {isCancelled && 'Ingestion cancelled'}
          {isFailed && 'Ingestion failed'}
        </p>
        <div className="text-sm text-secondary">
          <span>
            {realCompleted} files ingested
            {' '}{realFailed > 0 && <FailedFilesBadge count={realFailed} files={batchStatus.failed_files} />}
            {batchStatus.is_local_provider ? ' · Free (local)' : ` · ≈ ${fmtCost(batchStatus.accumulated_cost)} estimated`}
          </span>
        </div>
        <FilesProgressList files={filesProgress} />
        {batchReport != null && (
          <IngestionReport
            ingestionResult={batchReport as Parameters<typeof IngestionReport>[0]['ingestionResult']}
            onDismiss={onDismissReport}
          />
        )}
        <div className="flex justify-end">
          <button onClick={onBack} className="btn-secondary">Scan Another</button>
        </div>
      </div>
    )
  }

  // Waiting for first batch status poll. Stay in this state until at least
  // one progress poll has come back so we don't briefly render `null`.
  if (isIngesting || !hasFirstPoll || !batchStatus) {
    return (
      <div className="space-y-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 text-sm text-secondary">
            <span className="spinner" /> Starting batch...
          </div>
          <button onClick={onBack} className="btn-secondary text-sm">Cancel</button>
        </div>
        <FilesProgressList files={filesProgress} />
      </div>
    )
  }

  return null
}
