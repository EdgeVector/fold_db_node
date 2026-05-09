import { renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest'

vi.mock('../../api/clients', () => ({
  ingestionClient: {
    getJobProgress: vi.fn(),
    getScanResult: vi.fn(),
  },
}))

import { ingestionClient } from '../../api/clients'
import type { SmartFolderScanResponse } from '../../api/clients/ingestionClient'
import { useScanPolling } from '../useScanPolling'

type ProgressPayload = {
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

const buildProgress = (overrides: Partial<ProgressPayload> = {}): ProgressPayload => ({
  id: 'job-1',
  job_type: 'smart_folder_scan',
  current_step: 'finalizing',
  progress_percentage: 98,
  status_message: 'Finalizing... 5 to ingest, 0 skipped',
  is_complete: false,
  is_failed: false,
  results: undefined,
  started_at: Date.now(),
  ...overrides,
})

const buildScanResult = (overrides: Partial<SmartFolderScanResponse> = {}): SmartFolderScanResponse => ({
  success: true,
  total_files: 5,
  recommended_files: [],
  skipped_files: [],
  summary: {},
  total_estimated_cost: 0,
  scan_truncated: false,
  max_depth_used: 10,
  max_files_used: 100,
  ...overrides,
})

const okProgress = (data: ProgressPayload) => ({ success: true as const, data, status: 200 })
const okResult = (data: SmartFolderScanResponse) => ({ success: true as const, data, status: 200 })

describe('useScanPolling', () => {
  let onComplete: Mock
  let onFail: Mock

  beforeEach(() => {
    vi.clearAllMocks()
    onComplete = vi.fn()
    onFail = vi.fn()
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  it('does not poll when scanProgressId is null', async () => {
    renderHook(() =>
      useScanPolling({ scanProgressId: null, onComplete, onFail }),
    )
    await new Promise((r) => setTimeout(r, 30))
    expect(ingestionClient.getJobProgress).not.toHaveBeenCalled()
  })

  it('fetches scan result and calls onComplete when is_complete=true on first poll', async () => {
    const scanResult = buildScanResult({ recommended_files: [
      { path: '/a.txt', should_ingest: true, category: 'doc', reason: 'ok', file_size_bytes: 100, estimated_cost: 0.01 },
    ] })
    vi.mocked(ingestionClient.getJobProgress).mockResolvedValue(
      okProgress(buildProgress({ is_complete: true, progress_percentage: 100 })) as never,
    )
    vi.mocked(ingestionClient.getScanResult).mockResolvedValue(okResult(scanResult) as never)

    const { result } = renderHook(() =>
      useScanPolling({ scanProgressId: 'job-1', onComplete, onFail }),
    )

    await waitFor(() => expect(onComplete).toHaveBeenCalledWith(scanResult))
    expect(onFail).not.toHaveBeenCalled()
    expect(result.current.scanProgress).toBeNull()
  })

  it('calls onFail (not onComplete) when is_complete=true but getScanResult throws', async () => {
    vi.mocked(ingestionClient.getJobProgress).mockResolvedValue(
      okProgress(buildProgress({ is_complete: true, progress_percentage: 100 })) as never,
    )
    vi.mocked(ingestionClient.getScanResult).mockRejectedValue(new Error('network down'))

    renderHook(() =>
      useScanPolling({ scanProgressId: 'job-1', onComplete, onFail }),
    )

    await waitFor(() => expect(onFail).toHaveBeenCalledTimes(1))
    expect(onFail.mock.calls[0][0]).toMatch(/result fetch failed/i)
    expect(onComplete).not.toHaveBeenCalled()
  })

  it('calls onFail (not onComplete) when is_complete=true but getScanResult returns success:false', async () => {
    vi.mocked(ingestionClient.getJobProgress).mockResolvedValue(
      okProgress(buildProgress({ is_complete: true, progress_percentage: 100 })) as never,
    )
    vi.mocked(ingestionClient.getScanResult).mockResolvedValue({
      success: false,
      error: 'scan vanished',
      status: 404,
    } as never)

    renderHook(() =>
      useScanPolling({ scanProgressId: 'job-1', onComplete, onFail }),
    )

    await waitFor(() => expect(onFail).toHaveBeenCalledTimes(1))
    expect(onFail.mock.calls[0][0]).toMatch(/scan vanished|result fetch failed/i)
    expect(onComplete).not.toHaveBeenCalled()
  })

  it('calls onFail with error_message when is_failed=true', async () => {
    vi.mocked(ingestionClient.getJobProgress).mockResolvedValue(
      okProgress(
        buildProgress({ is_complete: true, is_failed: true, error_message: 'permission denied' }),
      ) as never,
    )

    renderHook(() =>
      useScanPolling({ scanProgressId: 'job-1', onComplete, onFail }),
    )

    await waitFor(() => expect(onFail).toHaveBeenCalledWith('permission denied'))
    expect(onComplete).not.toHaveBeenCalled()
    expect(ingestionClient.getScanResult).not.toHaveBeenCalled()
  })

  it('reconciles a restored scanProgressId by fetching once on mount and advancing past Finalizing', async () => {
    // Simulates the "reload after backend completed" path: persisted
    // scanProgressId is fed in, the very first progress poll already shows
    // is_complete=true with results=null (results live at scan-result endpoint).
    // The hook must fetch the scan result and fire onComplete — never get stuck.
    const scanResult = buildScanResult()
    vi.mocked(ingestionClient.getJobProgress).mockResolvedValue(
      okProgress(
        buildProgress({
          is_complete: true,
          progress_percentage: 98,
          status_message: 'Finalizing... 5 to ingest, 0 skipped',
        }),
      ) as never,
    )
    vi.mocked(ingestionClient.getScanResult).mockResolvedValue(okResult(scanResult) as never)

    renderHook(() =>
      useScanPolling({ scanProgressId: 'persisted-job', onComplete, onFail }),
    )

    await waitFor(() => expect(onComplete).toHaveBeenCalledWith(scanResult))
    expect(ingestionClient.getJobProgress).toHaveBeenCalledWith('persisted-job')
    expect(ingestionClient.getScanResult).toHaveBeenCalledWith('persisted-job')
  })
})
