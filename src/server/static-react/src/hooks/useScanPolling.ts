import { useState, useCallback, useRef, useEffect } from 'react'
import { ingestionClient } from '../api/clients'
import type { SmartFolderScanResponse } from '../api/clients/ingestionClient'
import { usePolling } from './usePolling'

interface ScanProgress {
  id: string
  job_type: string
  current_step: string
  progress_percentage: number
  status_message: string
  is_complete: boolean
  is_failed: boolean
  error_message?: string
  results?: Record<string, unknown>
  started_at: number
  completed_at?: number
}

interface UseScanPollingOpts {
  scanProgressId: string | null
  onComplete: (result: SmartFolderScanResponse) => void
  onFail: (message: string) => void
}

/**
 * Polls scan progress by ID and fetches the final scan result on completion.
 */
export function useScanPolling({
  scanProgressId,
  onComplete,
  onFail,
}: UseScanPollingOpts): { scanProgress: ScanProgress | null } {
  const [scanProgress, setScanProgress] = useState<ScanProgress | null>(null)
  const onCompleteRef = useRef(onComplete)
  const onFailRef = useRef(onFail)
  useEffect(() => {
    onCompleteRef.current = onComplete
  })
  useEffect(() => {
    onFailRef.current = onFail
  })

  const pollFn = useCallback(async () => {
    if (!scanProgressId) return { stop: true }
    const resp = await ingestionClient.getJobProgress(scanProgressId)
    if (!resp.success || !resp.data) throw new Error('poll failed')
    setScanProgress(resp.data)
    if (resp.data.is_complete && !resp.data.is_failed) {
      const result = await ingestionClient.getScanResult(scanProgressId)
      if (result.success && result.data) onCompleteRef.current(result.data)
      setScanProgress(null)
      return { stop: true }
    } else if (resp.data.is_failed) {
      onFailRef.current(resp.data.error_message || 'Scan failed')
      setScanProgress(null)
      return { stop: true }
    }
    return undefined
  }, [scanProgressId])

  const onMaxFailures = useCallback(() => {
    onFailRef.current('Lost connection to scan job')
    setScanProgress(null)
  }, [])

  usePolling({
    key: scanProgressId,
    pollFn,
    intervalMs: 1000,
    maxFailures: 5,
    onMaxFailures,
  })

  // Reset progress when scanProgressId is cleared
  useEffect(() => {
    if (!scanProgressId) setScanProgress(null)
  }, [scanProgressId])

  return { scanProgress }
}
