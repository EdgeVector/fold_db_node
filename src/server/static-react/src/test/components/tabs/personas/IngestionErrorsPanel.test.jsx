import React from 'react'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import IngestionErrorsPanel from '../../../../components/tabs/personas/IngestionErrorsPanel'

vi.mock('../../../../api/clients/fingerprintsClient', () => ({
  listIngestionErrors: vi.fn(),
  resolveIngestionError: vi.fn(),
}))

import {
  listIngestionErrors,
  resolveIngestionError,
} from '../../../../api/clients/fingerprintsClient'

function makeError(overrides = {}) {
  return {
    id: 'ie_default',
    source_schema: 'Photos',
    source_key: 'IMG_1234',
    extractor: 'face_detect',
    error_class: 'FaceDetectorTimeout',
    error_msg: 'timed out after 30s\n  at inner\n  at outer',
    retry_count: 0,
    resolved: false,
    created_at: '2026-04-15T10:00:00Z',
    last_retry_at: null,
    ...overrides,
  }
}

function ok(data) {
  return { success: true, data }
}

describe('IngestionErrorsPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('shows an empty-state message when there are no failures', async () => {
    listIngestionErrors.mockResolvedValue(ok({ errors: [] }))
    render(<IngestionErrorsPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('ingestion-errors-empty')).toBeInTheDocument()
    })
  })

  it('renders one row per failure with source, extractor, error class', async () => {
    listIngestionErrors.mockResolvedValue(
      ok({
        errors: [
          makeError({
            id: 'ie_1',
            source_schema: 'Notes',
            source_key: 'n_42',
            extractor: 'ner_llm',
            error_class: 'ModelTimeout',
          }),
          makeError({
            id: 'ie_2',
            source_schema: 'Photos',
            source_key: 'IMG_1',
            extractor: 'face_detect',
            error_class: 'IOError',
          }),
        ],
      }),
    )
    render(<IngestionErrorsPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('ingestion-error-row-ie_1')).toBeInTheDocument()
    })
    expect(screen.getByTestId('ingestion-error-row-ie_1')).toHaveTextContent(
      'Notes',
    )
    expect(screen.getByTestId('ingestion-error-row-ie_1')).toHaveTextContent(
      'ner_llm',
    )
    expect(screen.getByTestId('ingestion-error-row-ie_1')).toHaveTextContent(
      'ModelTimeout',
    )
    expect(screen.getByTestId('ingestion-error-row-ie_2')).toBeInTheDocument()
  })

  it('expands and collapses the error_msg on click', async () => {
    listIngestionErrors.mockResolvedValue(
      ok({ errors: [makeError({ id: 'ie_x' })] }),
    )
    render(<IngestionErrorsPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('ingestion-error-row-ie_x')).toBeInTheDocument()
    })
    expect(screen.queryByTestId('ingestion-error-msg-ie_x')).toBeNull()
    fireEvent.click(screen.getByTestId('ingestion-error-toggle-ie_x'))
    expect(screen.getByTestId('ingestion-error-msg-ie_x')).toHaveTextContent(
      'timed out after 30s',
    )
    fireEvent.click(screen.getByTestId('ingestion-error-toggle-ie_x'))
    expect(screen.queryByTestId('ingestion-error-msg-ie_x')).toBeNull()
  })

  it('removes a row from the list after Dismiss succeeds', async () => {
    listIngestionErrors.mockResolvedValue(
      ok({ errors: [makeError({ id: 'ie_dismiss' })] }),
    )
    resolveIngestionError.mockResolvedValue(
      ok(makeError({ id: 'ie_dismiss', resolved: true })),
    )
    render(<IngestionErrorsPanel />)
    await waitFor(() => {
      expect(
        screen.getByTestId('ingestion-error-row-ie_dismiss'),
      ).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('ingestion-error-dismiss-ie_dismiss'))
    await waitFor(() => {
      expect(screen.queryByTestId('ingestion-error-row-ie_dismiss')).toBeNull()
    })
    expect(resolveIngestionError).toHaveBeenCalledWith('ie_dismiss')
  })

  it('toggles the include_resolved filter and refetches', async () => {
    listIngestionErrors.mockResolvedValue(ok({ errors: [] }))
    render(<IngestionErrorsPanel />)
    await waitFor(() => {
      expect(listIngestionErrors).toHaveBeenCalledWith(false)
    })
    fireEvent.click(screen.getByTestId('ingestion-errors-include-resolved'))
    await waitFor(() => {
      expect(listIngestionErrors).toHaveBeenCalledWith(true)
    })
  })

  it('surfaces a backend error message', async () => {
    listIngestionErrors.mockResolvedValue({ success: false, error: 'boom' })
    render(<IngestionErrorsPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('ingestion-errors-error')).toHaveTextContent(
        'boom',
      )
    })
  })
})
