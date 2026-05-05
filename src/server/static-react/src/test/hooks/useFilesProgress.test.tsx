import { describe, it, expect, beforeEach, vi } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'

vi.mock('../../api/clients', () => ({
  ingestionClient: {
    getAllProgress: vi.fn(),
  },
}))

import { ingestionClient } from '../../api/clients'
import type { IngestionProgress } from '../../api/clients/ingestionClient'
import { useFilesProgress } from '../../hooks/useFilesProgress'
import { apiOk } from '../utils/testUtilities'

type GetAllProgressResult = Awaited<ReturnType<typeof ingestionClient.getAllProgress>>
const okProgress = (jobs: IngestionProgress[]): GetAllProgressResult =>
  apiOk(jobs) as unknown as GetAllProgressResult

// Regression: smart folder ingest UI rendered "Ingestion complete" right after
// Proceed because the batch-level Completed flag could fire before per-file
// progress had persisted. useFilesProgress drives the all-files-done gate so
// the UI waits for real per-file completion.

interface ProgressJobOverrides {
  is_complete?: boolean
  is_failed?: boolean
  progress_percentage?: number
  current_step?: string
  error_message?: string
  results?: {
    schema_name: string
    new_schema_created: boolean
    mutations_generated?: number
    mutations_executed?: number
  }
}

function progressJob(id: string, overrides: ProgressJobOverrides = {}): IngestionProgress {
  return {
    id,
    current_step: overrides.current_step ?? 'extracting',
    progress_percentage: overrides.progress_percentage ?? 50,
    status_message: 'Processing...',
    is_complete: overrides.is_complete ?? false,
    is_failed: overrides.is_failed ?? false,
    error_message: overrides.error_message,
    results: overrides.results
      ? {
          schema_name: overrides.results.schema_name,
          new_schema_created: overrides.results.new_schema_created,
          mutations_generated: overrides.results.mutations_generated ?? 0,
          mutations_executed: overrides.results.mutations_executed ?? 0,
        }
      : undefined,
    started_at: new Date().toISOString(),
  }
}

describe('useFilesProgress', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders queued placeholders before the first poll completes', () => {
    vi.mocked(ingestionClient.getAllProgress).mockImplementation(
      () => new Promise(() => {}), // never resolves
    )

    const { result } = renderHook(() =>
      useFilesProgress({
        enabled: 'batch-1',
        fileProgressIds: [
          { file_name: 'a.txt', progress_id: 'pid-a' },
          { file_name: 'b.txt', progress_id: 'pid-b' },
        ],
      }),
    )

    expect(result.current.filesProgress).toHaveLength(2)
    expect(result.current.filesProgress[0]).toMatchObject({
      progress_id: 'pid-a',
      file_name: 'a.txt',
      current_step: 'Queued',
      progress_percentage: 0,
      is_complete: false,
      is_failed: false,
    })
    expect(result.current.allFilesDone).toBe(false)
    expect(result.current.hasFirstPoll).toBe(false)
  })

  it('reports allFilesDone=false while any file is still running', async () => {
    vi.mocked(ingestionClient.getAllProgress).mockResolvedValue(
      okProgress([
        progressJob('pid-a', { is_complete: true, progress_percentage: 100 }),
        progressJob('pid-b', { is_complete: false, progress_percentage: 30 }),
      ]),
    )

    const { result } = renderHook(() =>
      useFilesProgress({
        enabled: 'batch-1',
        fileProgressIds: [
          { file_name: 'a.txt', progress_id: 'pid-a' },
          { file_name: 'b.txt', progress_id: 'pid-b' },
        ],
      }),
    )

    await waitFor(() => expect(result.current.hasFirstPoll).toBe(true))
    expect(result.current.allFilesDone).toBe(false)
    expect(result.current.filesProgress[0].is_complete).toBe(true)
    expect(result.current.filesProgress[1].is_complete).toBe(false)
  })

  it('reports allFilesDone=true only when every file is_complete or is_failed', async () => {
    vi.mocked(ingestionClient.getAllProgress).mockResolvedValue(
      okProgress([
        progressJob('pid-a', { is_complete: true, progress_percentage: 100 }),
        progressJob('pid-b', {
          is_failed: true,
          error_message: 'parse error',
          progress_percentage: 0,
        }),
      ]),
    )

    const { result } = renderHook(() =>
      useFilesProgress({
        enabled: 'batch-1',
        fileProgressIds: [
          { file_name: 'a.txt', progress_id: 'pid-a' },
          { file_name: 'b.txt', progress_id: 'pid-b' },
        ],
      }),
    )

    await waitFor(() => expect(result.current.allFilesDone).toBe(true))
    expect(result.current.filesProgress[1].is_failed).toBe(true)
    expect(result.current.filesProgress[1].error_message).toBe('parse error')
  })

  it('handles the wrapped { progress: [...] } response shape', async () => {
    vi.mocked(ingestionClient.getAllProgress).mockResolvedValue({
      success: true,
      status: 200,
      data: {
        progress: [
          progressJob('pid-a', { is_complete: true, progress_percentage: 100 }),
        ],
      },
    } as unknown as Awaited<ReturnType<typeof ingestionClient.getAllProgress>>)

    const { result } = renderHook(() =>
      useFilesProgress({
        enabled: 'batch-1',
        fileProgressIds: [{ file_name: 'a.txt', progress_id: 'pid-a' }],
      }),
    )

    await waitFor(() => expect(result.current.allFilesDone).toBe(true))
  })

  it('does not poll when disabled (enabled is null)', async () => {
    vi.mocked(ingestionClient.getAllProgress).mockResolvedValue(okProgress([]))

    renderHook(() =>
      useFilesProgress({
        enabled: null,
        fileProgressIds: [{ file_name: 'a.txt', progress_id: 'pid-a' }],
      }),
    )

    // Give the effect a tick to settle.
    await new Promise((r) => setTimeout(r, 30))
    expect(ingestionClient.getAllProgress).not.toHaveBeenCalled()
  })

  it('does not poll when fileProgressIds is empty', async () => {
    vi.mocked(ingestionClient.getAllProgress).mockResolvedValue(okProgress([]))

    renderHook(() =>
      useFilesProgress({
        enabled: 'batch-1',
        fileProgressIds: [],
      }),
    )

    await new Promise((r) => setTimeout(r, 30))
    expect(ingestionClient.getAllProgress).not.toHaveBeenCalled()
  })

  it('ignores progress entries whose id is not in fileProgressIds', async () => {
    vi.mocked(ingestionClient.getAllProgress).mockResolvedValue(
      okProgress([
        progressJob('pid-a', { is_complete: true, progress_percentage: 100 }),
        progressJob('pid-other-batch', { is_complete: true, progress_percentage: 100 }),
      ]),
    )

    const { result } = renderHook(() =>
      useFilesProgress({
        enabled: 'batch-1',
        fileProgressIds: [{ file_name: 'a.txt', progress_id: 'pid-a' }],
      }),
    )

    await waitFor(() => expect(result.current.allFilesDone).toBe(true))
    expect(result.current.filesProgress).toHaveLength(1)
    expect(result.current.filesProgress[0].progress_id).toBe('pid-a')
  })
})
