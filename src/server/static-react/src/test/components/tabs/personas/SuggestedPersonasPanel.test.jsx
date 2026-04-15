import React from 'react'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import SuggestedPersonasPanel from '../../../../components/tabs/personas/SuggestedPersonasPanel'

vi.mock('../../../../api/clients/fingerprintsClient', () => ({
  listSuggestedPersonas: vi.fn(),
  acceptSuggestedPersona: vi.fn(),
  RELATIONSHIP_OPTIONS: [
    'self',
    'family',
    'colleague',
    'friend',
    'acquaintance',
    'unknown',
  ],
}))

import {
  listSuggestedPersonas,
  acceptSuggestedPersona,
} from '../../../../api/clients/fingerprintsClient'

function makeSample(kind, value, idSuffix) {
  return {
    id: `fp_${idSuffix}`,
    kind,
    display_value: value,
    first_seen: null,
    last_seen: null,
  }
}

function makeSuggestion(overrides = {}) {
  return {
    suggested_id: 'sg_default',
    suggested_name: 'Unnamed cluster',
    fingerprint_ids: ['fp_a', 'fp_b', 'fp_c'],
    fingerprint_count: 3,
    edge_count: 2,
    mention_count: 5,
    sample_fingerprints: [
      makeSample('email', 'tom@acme.com', 'a'),
      makeSample('full_name', 'Tom Tang', 'b'),
    ],
    ...overrides,
  }
}

function ok(data) {
  return { success: true, data }
}

const DISMISSED_STORAGE_KEY = 'folddb.dismissed_suggested_personas.v1'

describe('SuggestedPersonasPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    window.localStorage.clear()
  })

  it('shows an empty-state message when there are no suggestions', async () => {
    listSuggestedPersonas.mockResolvedValue(ok({ suggestions: [] }))
    render(<SuggestedPersonasPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('suggested-personas-empty')).toBeInTheDocument()
    })
  })

  it('renders one row per suggestion with name, counts, and samples', async () => {
    listSuggestedPersonas.mockResolvedValue(
      ok({
        suggestions: [
          makeSuggestion({
            suggested_id: 'sg_1',
            suggested_name: 'Tom Tang',
            fingerprint_count: 4,
            edge_count: 3,
            mention_count: 9,
          }),
        ],
      }),
    )
    render(<SuggestedPersonasPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('suggested-row-sg_1')).toBeInTheDocument()
    })
    const row = screen.getByTestId('suggested-row-sg_1')
    expect(row).toHaveTextContent('Tom Tang')
    expect(row).toHaveTextContent('4 fps')
    expect(row).toHaveTextContent('9 mentions')
    expect(row).toHaveTextContent('tom@acme.com')
  })

  it('dismisses a suggestion locally without calling the backend', async () => {
    listSuggestedPersonas.mockResolvedValue(
      ok({ suggestions: [makeSuggestion({ suggested_id: 'sg_d' })] }),
    )
    render(<SuggestedPersonasPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('suggested-row-sg_d')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('suggested-dismiss-sg_d'))
    await waitFor(() => {
      expect(screen.queryByTestId('suggested-row-sg_d')).toBeNull()
    })
    expect(acceptSuggestedPersona).not.toHaveBeenCalled()
  })

  it('accepts a suggestion with a chosen name and submits the right payload', async () => {
    listSuggestedPersonas.mockResolvedValue(
      ok({
        suggestions: [
          makeSuggestion({
            suggested_id: 'sg_accept',
            suggested_name: 'Tom Tang',
            fingerprint_ids: ['fp_a', 'fp_b', 'fp_c'],
          }),
        ],
      }),
    )
    acceptSuggestedPersona.mockResolvedValue(
      ok({
        id: 'ps_new',
        name: 'Tom (colleague)',
      }),
    )

    render(<SuggestedPersonasPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('suggested-row-sg_accept')).toBeInTheDocument()
    })

    // Click Name it → input + relationship dropdown render.
    fireEvent.click(screen.getByTestId('suggested-name-sg_accept'))
    const input = screen.getByTestId('suggested-name-input-sg_accept')
    fireEvent.change(input, { target: { value: 'Tom (colleague)' } })
    const relSelect = screen.getByTestId('suggested-relationship-select-sg_accept')
    fireEvent.change(relSelect, { target: { value: 'colleague' } })
    fireEvent.click(screen.getByTestId('suggested-confirm-sg_accept'))

    await waitFor(() => {
      expect(acceptSuggestedPersona).toHaveBeenCalledWith({
        fingerprint_ids: ['fp_a', 'fp_b', 'fp_c'],
        name: 'Tom (colleague)',
        relationship: 'colleague',
      })
    })

    // Row is removed from the list after a successful accept.
    await waitFor(() => {
      expect(screen.queryByTestId('suggested-row-sg_accept')).toBeNull()
    })
  })

  it('surfaces a backend error message', async () => {
    listSuggestedPersonas.mockResolvedValue({ success: false, error: 'boom' })
    render(<SuggestedPersonasPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('suggested-personas-error')).toHaveTextContent('boom')
    })
  })

  it('persists dismissals to localStorage so they survive a reload', async () => {
    listSuggestedPersonas.mockResolvedValue(
      ok({ suggestions: [makeSuggestion({ suggested_id: 'sg_persist' })] }),
    )
    const { unmount } = render(<SuggestedPersonasPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('suggested-row-sg_persist')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('suggested-dismiss-sg_persist'))
    await waitFor(() => {
      expect(screen.queryByTestId('suggested-row-sg_persist')).toBeNull()
    })

    // Confirm the dismissal landed in localStorage with the versioned key.
    const stored = JSON.parse(
      window.localStorage.getItem(DISMISSED_STORAGE_KEY) || '[]',
    )
    expect(stored).toContain('sg_persist')

    // Remount (simulates a reload) and the row should still be hidden.
    unmount()
    listSuggestedPersonas.mockResolvedValue(
      ok({ suggestions: [makeSuggestion({ suggested_id: 'sg_persist' })] }),
    )
    render(<SuggestedPersonasPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('suggested-personas-empty')).toBeInTheDocument()
    })
    expect(screen.queryByTestId('suggested-row-sg_persist')).toBeNull()
  })

  it('clears every dismissal when Clear N dismissed is clicked', async () => {
    window.localStorage.setItem(
      DISMISSED_STORAGE_KEY,
      JSON.stringify(['sg_a', 'sg_b']),
    )
    listSuggestedPersonas.mockResolvedValue(
      ok({
        suggestions: [
          makeSuggestion({ suggested_id: 'sg_a' }),
          makeSuggestion({ suggested_id: 'sg_b' }),
        ],
      }),
    )

    render(<SuggestedPersonasPanel />)
    // Both rows hidden on initial render because both ids are persisted
    // in the dismissed set.
    await waitFor(() => {
      expect(screen.getByTestId('suggested-personas-empty')).toBeInTheDocument()
    })

    fireEvent.click(screen.getByTestId('suggested-personas-clear-dismissed'))
    await waitFor(() => {
      expect(screen.getByTestId('suggested-row-sg_a')).toBeInTheDocument()
      expect(screen.getByTestId('suggested-row-sg_b')).toBeInTheDocument()
    })

    // Storage is wiped.
    expect(
      JSON.parse(window.localStorage.getItem(DISMISSED_STORAGE_KEY) || '[]'),
    ).toEqual([])
  })

  it('loadDismissedFromStorage tolerates malformed JSON', async () => {
    const { loadDismissedFromStorage } = await import(
      '../../../../components/tabs/personas/SuggestedPersonasPanel'
    )
    window.localStorage.setItem(DISMISSED_STORAGE_KEY, 'not-json')
    expect(loadDismissedFromStorage().size).toBe(0)
    window.localStorage.setItem(DISMISSED_STORAGE_KEY, '42')
    expect(loadDismissedFromStorage().size).toBe(0)
    window.localStorage.setItem(
      DISMISSED_STORAGE_KEY,
      JSON.stringify(['sg_a', 1, null, 'sg_b']),
    )
    const s = loadDismissedFromStorage()
    expect(s.has('sg_a')).toBe(true)
    expect(s.has('sg_b')).toBe(true)
    expect(s.size).toBe(2)
  })
})
