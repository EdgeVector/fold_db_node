import { render, screen, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

import HeaderProgress from '../../components/HeaderProgress'
import type { BatchStatusResponse } from '../../api/clients/ingestionClient'
import type { IndexingStatus } from '../../api/clients/indexingClient'
import { apiOk } from '../utils/testUtilities'

vi.mock('../../api/clients', () => ({
  ingestionClient: {
    getAllProgress: vi.fn(),
    getBatchStatus: vi.fn(),
  },
}))

vi.mock('../../api/clients/indexingClient', () => ({
  getIndexingStatus: vi.fn(),
}))

import { ingestionClient } from '../../api/clients'
import { getIndexingStatus } from '../../api/clients/indexingClient'

const idleIndexingStatus: IndexingStatus = {
  state: 'Idle',
  operations_in_progress: 0,
  total_operations_processed: 0,
  operations_queued: 0,
  last_operation_time: null,
  avg_processing_time_ms: 0,
  operations_per_second: 0,
  current_batch_size: null,
  current_batch_start_time: null,
}

const runningBatch: BatchStatusResponse = {
  batch_id: 'batch-abc',
  status: 'Running',
  spend_limit: null,
  accumulated_cost: 0,
  files_total: 50,
  files_completed: 17,
  files_failed: 0,
  files_remaining: 33,
  estimated_remaining_cost: 0,
  in_flight_count: 1,
  current_file_name: 'foo.txt',
  current_file_step: 'analyzing',
  current_file_progress: 35,
}

const completedBatch: BatchStatusResponse = {
  ...runningBatch,
  status: 'Completed',
  files_completed: 50,
  files_remaining: 0,
  current_file_name: null,
  current_file_step: null,
  current_file_progress: null,
}

const inFlightJob = {
  id: 'job-1',
  current_step: 'analyzing',
  progress_percentage: 35,
  status_message: 'analyzing foo.txt',
  is_complete: false,
  is_failed: false,
  started_at: '2026-05-05T00:00:00.000Z',
  job_type: 'ingestion',
}

describe('HeaderProgress — clears chip after batch completion', () => {
  beforeEach(() => {
    localStorage.setItem('activeBatchId', 'batch-abc')
    vi.mocked(getIndexingStatus).mockResolvedValue(idleIndexingStatus)
  })

  afterEach(() => {
    localStorage.clear()
    vi.clearAllMocks()
  })

  it('hides the chip when the batch transitions Running → Completed, even if per-file jobs linger as is_complete=false', async () => {
    // First poll: batch is Running; per-file IngestionProgress at 35%.
    // Subsequent polls: batch is Completed but the per-file row is STILL
    // is_complete=false (the regression — server-side stale row).
    const getAllProgressMock = vi.mocked(ingestionClient.getAllProgress)
    getAllProgressMock.mockResolvedValue(
      apiOk([inFlightJob]) as unknown as Awaited<
        ReturnType<typeof ingestionClient.getAllProgress>
      >,
    )
    const getBatchStatusMock = vi.mocked(ingestionClient.getBatchStatus)
    getBatchStatusMock
      .mockResolvedValueOnce(
        apiOk(runningBatch) as unknown as Awaited<
          ReturnType<typeof ingestionClient.getBatchStatus>
        >,
      )
      .mockResolvedValue(
        apiOk(completedBatch) as unknown as Awaited<
          ReturnType<typeof ingestionClient.getBatchStatus>
        >,
      )

    vi.useFakeTimers({ shouldAdvanceTime: true })
    const { container } = render(<HeaderProgress />)

    // Wait for the initial mount poll (Running) to render the chip.
    // Percentage = files_completed/files_total = 17/50 = 34% (rounded).
    await waitFor(() => {
      expect(screen.getByText(/ingesting foo\.txt 34%/i)).toBeInTheDocument()
    })

    // Advance the polling interval (2s) so the next poll observes
    // status=Completed.
    await vi.advanceTimersByTimeAsync(2100)

    await waitFor(() => {
      expect(container).toBeEmptyDOMElement()
    })

    // The lingering server-side job (still is_complete=false) must NOT
    // re-surface on subsequent polls — once dismissed at the batch
    // boundary, it stays out of the chip.
    await vi.advanceTimersByTimeAsync(2100)
    expect(container).toBeEmptyDOMElement()

    // And the defensive cleanup: localStorage must be clear so a future
    // navigation doesn't pick the dead batch back up.
    expect(localStorage.getItem('activeBatchId')).toBeNull()
    vi.useRealTimers()
  })
})
