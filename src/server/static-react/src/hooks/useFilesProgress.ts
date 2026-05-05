import { useState, useCallback, useEffect } from 'react'
import { ingestionClient } from '../api/clients'
import { usePolling } from './usePolling'

export interface FileProgress {
  progress_id: string
  file_name: string
  current_step: string
  progress_percentage: number
  is_complete: boolean
  is_failed: boolean
  error_message?: string
  results?: {
    schema_name?: string
    new_schema_created?: boolean
    mutations_generated?: number
    mutations_executed?: number
  }
}

interface FileProgressId {
  file_name: string
  progress_id: string
}

interface UseFilesProgressOpts {
  /** Poll while truthy (e.g. the active batch_id). Polling stops when null. */
  enabled: unknown
  fileProgressIds: FileProgressId[]
}

interface UseFilesProgressResult {
  filesProgress: FileProgress[]
  /** True when every fileProgressId has reported is_complete or is_failed. */
  allFilesDone: boolean
  /** True once at least one poll has returned (so UI can stop showing "Starting..."). */
  hasFirstPoll: boolean
}

/** Read the `getAllProgress()` response shape — either `{ progress: [...] }` or a bare array. */
function extractProgressList(data: unknown): unknown[] {
  if (Array.isArray((data as { progress?: unknown })?.progress)) {
    return (data as { progress: unknown[] }).progress
  }
  if (Array.isArray(data)) return data
  return []
}

/**
 * Polls per-file ingestion progress for a smart-folder batch and reports
 * whether every file has reached a terminal state (complete or failed).
 *
 * Used to gate the "Ingestion complete" UI on real per-file completion
 * rather than the batch-level `Completed` flag, which can flip before all
 * per-file progress jobs have persisted their final state.
 */
export function useFilesProgress({
  enabled,
  fileProgressIds,
}: UseFilesProgressOpts): UseFilesProgressResult {
  const [progressMap, setProgressMap] = useState<Map<string, FileProgress>>(new Map())
  const [hasFirstPoll, setHasFirstPoll] = useState(false)

  const pollFn = useCallback(async () => {
    if (fileProgressIds.length === 0) return { stop: true }
    const resp = await ingestionClient.getAllProgress()
    if (!resp.success || !resp.data) throw new Error('poll failed')
    const list = extractProgressList(resp.data)
    const nameByPid = new Map(fileProgressIds.map((f) => [f.progress_id, f.file_name]))

    const next = new Map<string, FileProgress>()
    for (const job of list) {
      const j = job as {
        id?: string
        current_step?: string
        progress_percentage?: number
        is_complete?: boolean
        is_failed?: boolean
        error_message?: string
        results?: FileProgress['results']
      }
      if (!j.id || !nameByPid.has(j.id)) continue
      next.set(j.id, {
        progress_id: j.id,
        file_name: nameByPid.get(j.id) ?? j.id,
        current_step: j.current_step ?? '',
        progress_percentage: j.progress_percentage ?? 0,
        is_complete: !!j.is_complete,
        is_failed: !!j.is_failed,
        error_message: j.error_message,
        results: j.results,
      })
    }

    setProgressMap(next)
    setHasFirstPoll(true)

    const allTerminal = fileProgressIds.every((f) => {
      const p = next.get(f.progress_id)
      return p && (p.is_complete || p.is_failed)
    })
    return allTerminal ? { stop: true } : undefined
  }, [fileProgressIds])

  // Reset state when the batch identity changes (caller toggles `enabled`).
  useEffect(() => {
    if (!enabled) {
      setProgressMap(new Map())
      setHasFirstPoll(false)
    }
  }, [enabled])

  usePolling({
    key: enabled && fileProgressIds.length > 0 ? enabled : null,
    pollFn,
    intervalMs: 1500,
    maxFailures: 5,
  })

  // Preserve fileProgressIds order in the output array; backfill placeholders
  // for files we haven't seen progress for yet (so the UI can render rows
  // immediately as "queued").
  const filesProgress: FileProgress[] = fileProgressIds.map((f) => {
    const p = progressMap.get(f.progress_id)
    if (p) return p
    return {
      progress_id: f.progress_id,
      file_name: f.file_name,
      current_step: 'Queued',
      progress_percentage: 0,
      is_complete: false,
      is_failed: false,
    }
  })

  const allFilesDone =
    fileProgressIds.length > 0 &&
    fileProgressIds.every((f) => {
      const p = progressMap.get(f.progress_id)
      return !!p && (p.is_complete || p.is_failed)
    })

  return { filesProgress, allFilesDone, hasFirstPoll }
}
