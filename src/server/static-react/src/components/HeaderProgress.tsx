import { useState, useEffect, useCallback, useRef } from 'react'
import { ingestionClient } from '../api/clients'
import { getIndexingStatus } from '../api/clients/indexingClient'
import type { IndexingStatus } from '../api/clients/indexingClient'
import type { BatchStatusResponse, IngestionProgress } from '../api/clients/ingestionClient'
import { fmtCost } from '../utils/formatCost'
import { usePolling } from '../hooks/usePolling'

type ProgressJob = IngestionProgress & { job_type?: string }

function HeaderProgress() {
  const [jobs, setJobs] = useState<ProgressJob[]>([])
  const [indexingStatus, setIndexingStatus] = useState<IndexingStatus | null>(null)
  const [batchInfo, setBatchInfo] = useState<BatchStatusResponse | null>(null)
  const [isLoading, setIsLoading] = useState(true)
  const dismissTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const prevBatchStatusRef = useRef<BatchStatusResponse['status'] | null>(null)
  // IDs of in-flight per-file ingestion jobs that were live when the batch
  // hit a terminal state. The Smart Folder batch can flip to Completed
  // server-side while individual `IngestionProgress` rows still report
  // `is_complete=false` (e.g. spend-limit pause, dropped task, slow cleanup).
  // Without dismissing them, the jobs indicator below would render
  // "ingesting X%" indefinitely after the batch ends.
  const dismissedJobIdsRef = useRef<Set<string>>(new Set())

  useEffect(() => {
    return () => { if (dismissTimeoutRef.current) clearTimeout(dismissTimeoutRef.current) }
  }, [])

  const fetchProgress = useCallback(async () => {
    let nextJobs: ProgressJob[] = []
    let nextIndexing: IndexingStatus | null = null
    let nextBatch: BatchStatusResponse | null = null
    try {
      const promises: Array<Promise<unknown>> = [
        ingestionClient.getAllProgress(),
        getIndexingStatus()
      ]

      const activeBatchId = localStorage.getItem('activeBatchId')
      if (activeBatchId) {
        promises.push(ingestionClient.getBatchStatus(activeBatchId))
      }

      const results = await Promise.allSettled(promises)

      let progressData: ProgressJob[] = []
      if (results[0].status === 'fulfilled') {
        const response = results[0].value as { data?: ProgressJob[] | { progress?: ProgressJob[] }; progress?: ProgressJob[] }
        const data = response.data
        const raw =
          (data && typeof data === 'object' && !Array.isArray(data) ? data.progress : undefined) ??
          (Array.isArray(data) ? data : undefined) ??
          response.progress ??
          []
        if (Array.isArray(raw)) progressData = raw
      }
      nextJobs = progressData
      setJobs(nextJobs)

      if (results[1].status === 'fulfilled') {
        nextIndexing = results[1].value as IndexingStatus
      }
      setIndexingStatus(nextIndexing)

      if (results.length > 2 && results[2].status === 'fulfilled') {
        const batchResp = results[2].value as { success?: boolean; data?: BatchStatusResponse }
        if (batchResp.success && batchResp.data) {
          const s = batchResp.data.status
          const wasActive =
            prevBatchStatusRef.current === 'Running' ||
            prevBatchStatusRef.current === 'Paused'
          const isTerminal = s === 'Completed' || s === 'Cancelled' || s === 'Failed'
          if (s === 'Running' && !wasActive) {
            // New batch — start with a clean dismissal set.
            dismissedJobIdsRef.current = new Set()
          }
          if (wasActive && isTerminal) {
            // Snapshot any still-running per-file jobs and dismiss them so
            // the jobs indicator doesn't render their stale percentages.
            for (const j of progressData) {
              if (!j.is_complete && !j.is_failed) dismissedJobIdsRef.current.add(j.id)
            }
            // useBatchMonitor normally clears these, but it only runs while
            // SmartFolderTab is mounted — defend against a navigated-away user.
            localStorage.removeItem('activeBatchId')
            localStorage.removeItem('activeBatchStatus')
            if (dismissTimeoutRef.current) clearTimeout(dismissTimeoutRef.current)
            setBatchInfo(null)
            // Leave nextBatch as null so usePolling can drop to the idle interval.
          } else {
            nextBatch = batchResp.data
            setBatchInfo(nextBatch)
          }
          prevBatchStatusRef.current = s
        } else {
          setBatchInfo(null)
          prevBatchStatusRef.current = null
        }
      } else if (results.length > 2) {
        // Batch fetch was issued but rejected (404 after server GC, network).
        // Clear so the chip doesn't stick on the last-seen Running snapshot.
        setBatchInfo(null)
        localStorage.removeItem('activeBatchId')
        localStorage.removeItem('activeBatchStatus')
        prevBatchStatusRef.current = null
      } else if (!activeBatchId) {
        setBatchInfo(null)
        prevBatchStatusRef.current = null
      }
    } catch (error) {
      console.error('Failed to fetch progress:', error)
      setJobs([])
      setIndexingStatus(null)
    } finally {
      setIsLoading(false)
    }

    const dismissed = dismissedJobIdsRef.current
    const hasActiveJobs = nextJobs.some(
      j => !j.is_complete && !j.is_failed && !dismissed.has(j.id),
    )
    const isIndexingActive = nextIndexing?.state === 'Indexing'
    const hasActiveBatch = !!nextBatch && (nextBatch.status === 'Running' || nextBatch.status === 'Paused')
    const idle = !hasActiveJobs && !isIndexingActive && !hasActiveBatch
    return idle ? { idle: true } : undefined
  }, [])

  usePolling({
    key: 'header-progress',
    pollFn: fetchProgress,
    intervalMs: 2000,
    idleIntervalMs: 10000,
    maxFailures: Number.POSITIVE_INFINITY,
  })

  const activeJobs = jobs.filter(
    j => !j.is_complete && !j.is_failed && !dismissedJobIdsRef.current.has(j.id),
  )
  const isIndexingActive = indexingStatus?.state === 'Indexing'
  const hasBatch = batchInfo && (batchInfo.status === 'Running' || batchInfo.status === 'Paused')

  if (isLoading || (activeJobs.length === 0 && !isIndexingActive && !hasBatch)) {
    return null
  }

  const indicators: React.ReactNode[] = []

  if (hasBatch && batchInfo) {
    const isPaused = batchInfo.status === 'Paused'
    const pct = batchInfo.files_total > 0 ? Math.round((batchInfo.files_completed / batchInfo.files_total) * 100) : 0

    const dotColor = isPaused ? 'bg-gruvbox-yellow' : 'bg-gruvbox-blue'
    const dotAnim = isPaused ? '' : 'animate-pulse'
    const textClass = isPaused ? 'text-gruvbox-yellow' : 'text-gruvbox-blue'
    const barColor = isPaused ? 'bg-gruvbox-yellow' : 'bg-primary'

    const fileLabel = batchInfo.current_file_name ? ` ${batchInfo.current_file_name}` : ''
    const statusText = isPaused
      ? `paused ${fmtCost(batchInfo.accumulated_cost)}/${fmtCost(batchInfo.spend_limit ?? 0)} limit`
      : `ingesting${fileLabel} ${pct}% ${fmtCost(batchInfo.accumulated_cost)}${batchInfo.spend_limit != null ? `/${fmtCost(batchInfo.spend_limit)}` : ''}`

    indicators.push(
      <div key="batch" className="flex items-center gap-2 px-3 py-1.5 bg-surface-secondary border border-border">
        <div className={`w-2 h-2 rounded-full ${dotColor} ${dotAnim}`} />
        <span className={`text-xs font-mono ${textClass}`}>{statusText}</span>
        <div className="w-16 h-1 bg-border rounded-full overflow-hidden">
          <div
            className={`h-full ${barColor} transition-all duration-300`}
            style={{ width: `${pct}%` }}
          />
        </div>
      </div>
    )
  }

  if (isIndexingActive) {
    const opsQueued = indexingStatus?.operations_queued || 0
    const opsInProgress = indexingStatus?.operations_in_progress || 0
    indicators.push(
      <div key="indexing" className="flex items-center gap-2 px-3 py-1.5 bg-surface-secondary border border-border">
        <div className="w-2 h-2 bg-gruvbox-blue rounded-full animate-pulse" />
        <span className="text-xs font-mono text-secondary">
          indexing {opsInProgress > 0 ? `(${opsInProgress})` : ''}{opsQueued > 0 ? ` +${opsQueued}` : ''}
        </span>
      </div>
    )
  }

  if (activeJobs.length > 0 && !hasBatch) {
    const totalProgress = activeJobs.reduce((sum, job) => sum + (job.progress_percentage || 0), 0)
    const avgProgress = Math.round(totalProgress / activeJobs.length)

    const getJobLabel = (job: ProgressJob) => {
      if (job.job_type === 'database_reset') return 'reset'
      if (job.job_type === 'indexing') return 'indexing'
      return 'ingesting'
    }

    const statusText = activeJobs.length === 1
      ? `${getJobLabel(activeJobs[0])} ${avgProgress}%`
      : `${activeJobs.length} jobs ${avgProgress}%`

    const primaryJob = activeJobs[0]
    const isReset = primaryJob?.job_type === 'database_reset'
    const isJobIndexing = primaryJob?.job_type === 'indexing'

    const dotColor = isReset
      ? 'bg-gruvbox-red'
      : isJobIndexing
        ? 'bg-gruvbox-blue'
        : 'bg-gruvbox-blue'

    const textClass = isReset
      ? 'text-gruvbox-red'
      : isJobIndexing
        ? 'text-secondary'
        : 'text-gruvbox-blue'

    indicators.push(
      <div key="jobs" className="flex items-center gap-2 px-3 py-1.5 bg-surface-secondary border border-border">
        <div className={`w-2 h-2 rounded-full animate-pulse ${dotColor}`} />
        <span className={`text-xs font-mono ${textClass}`}>
          {statusText}
        </span>
        <div className="w-16 h-1 bg-border rounded-full overflow-hidden">
          <div
            className="h-full bg-primary transition-all duration-300"
            style={{ width: `${avgProgress}%` }}
          />
        </div>
      </div>
    )
  }

  return <>{indicators}</>
}

export default HeaderProgress
