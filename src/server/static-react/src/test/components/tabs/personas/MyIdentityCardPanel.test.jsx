import React from 'react'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import MyIdentityCardPanel from '../../../../components/tabs/personas/MyIdentityCardPanel'

vi.mock('../../../../api/clients/fingerprintsClient', () => ({
  getMyIdentityCard: vi.fn(),
  reissueMyIdentityCard: vi.fn(),
}))

import {
  getMyIdentityCard,
  reissueMyIdentityCard,
} from '../../../../api/clients/fingerprintsClient'

function card(overrides = {}) {
  return {
    pub_key: 'pk_abc',
    display_name: 'Tom Tang',
    birthday: '1990-04-17',
    face_embedding: null,
    node_id: 'pk_abc',
    card_signature:
      'sig_base64_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
    issued_at: '2026-04-14T12:00:00Z',
    ...overrides,
  }
}

function ok(data) {
  return { success: true, data }
}

describe('MyIdentityCardPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders the card fields when present', async () => {
    getMyIdentityCard.mockResolvedValue(ok(card()))
    render(<MyIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('my-identity-card-fields')).toBeInTheDocument()
    })
    const fields = screen.getByTestId('my-identity-card-fields')
    expect(fields).toHaveTextContent('Tom Tang')
    expect(fields).toHaveTextContent('1990-04-17')
    expect(fields).toHaveTextContent('pk_abc')
    expect(fields).toHaveTextContent('2026-04-14T12:00:00Z')
  })

  it('shows "not set" when birthday is null', async () => {
    getMyIdentityCard.mockResolvedValue(ok(card({ birthday: null })))
    render(<MyIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('my-identity-card-fields')).toBeInTheDocument()
    })
    expect(screen.getByTestId('my-identity-card-fields')).toHaveTextContent(
      'not set',
    )
  })

  it('surfaces the backend error (e.g. 404 when no card yet)', async () => {
    getMyIdentityCard.mockResolvedValue({
      success: false,
      error: 'self-Identity not yet issued — complete the setup wizard first',
    })
    render(<MyIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('my-identity-card-error')).toHaveTextContent(
        'not yet issued',
      )
    })
  })

  it('copies the card JSON to the clipboard on Copy click', async () => {
    getMyIdentityCard.mockResolvedValue(ok(card()))
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      configurable: true,
    })

    render(<MyIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('my-identity-card-copy')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('my-identity-card-copy'))
    await waitFor(() => {
      expect(writeText).toHaveBeenCalledTimes(1)
    })
    const payload = writeText.mock.calls[0][0]
    // Payload is canonical JSON including every field.
    const parsed = JSON.parse(payload)
    expect(parsed.pub_key).toBe('pk_abc')
    expect(parsed.display_name).toBe('Tom Tang')
    expect(parsed.card_signature).toMatch(/^sig_/)
  })

  it('renders the pretty-printed JSON below the fields', async () => {
    getMyIdentityCard.mockResolvedValue(ok(card()))
    render(<MyIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('my-identity-card-json')).toBeInTheDocument()
    })
    expect(screen.getByTestId('my-identity-card-json')).toHaveTextContent(
      '"display_name": "Tom Tang"',
    )
  })

  describe('edit + reissue flow', () => {
    it('shows the edit form after clicking Edit card', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-edit')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-edit'))
      expect(screen.getByTestId('my-identity-card-edit-form')).toBeInTheDocument()
      expect(screen.getByTestId('my-identity-card-draft-name')).toHaveValue(
        'Tom Tang',
      )
      // Birthday is intentionally NOT editable here — a real person's
      // birthday doesn't change; surfacing it would imply otherwise.
      expect(
        screen.queryByTestId('my-identity-card-draft-birthday'),
      ).toBeNull()
    })

    it('calls reissueMyIdentityCard with only the changed display_name', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      reissueMyIdentityCard.mockResolvedValue(
        ok(card({ display_name: 'Thomas', issued_at: '2026-04-18T00:00:00Z' })),
      )
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-edit')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-edit'))
      fireEvent.change(screen.getByTestId('my-identity-card-draft-name'), {
        target: { value: 'Thomas' },
      })
      fireEvent.click(screen.getByTestId('my-identity-card-save'))
      await waitFor(() => {
        expect(reissueMyIdentityCard).toHaveBeenCalledWith({
          display_name: 'Thomas',
        })
      })
      // After success we drop back to the display view with the new card.
      await waitFor(() => {
        expect(screen.queryByTestId('my-identity-card-edit-form')).toBeNull()
      })
      expect(screen.getByTestId('my-identity-card-fields')).toHaveTextContent(
        'Thomas',
      )
    })

    it('does not call reissue when nothing changed', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-edit')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-edit'))
      fireEvent.click(screen.getByTestId('my-identity-card-save'))
      expect(reissueMyIdentityCard).not.toHaveBeenCalled()
      // Form closes even though no network call happened.
      expect(screen.queryByTestId('my-identity-card-edit-form')).toBeNull()
    })

    it('surfaces backend error messages in the save-error slot', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      reissueMyIdentityCard.mockResolvedValue({
        success: false,
        error: 'display_name must not be empty',
      })
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-edit')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-edit'))
      fireEvent.change(screen.getByTestId('my-identity-card-draft-name'), {
        target: { value: 'Rename' },
      })
      fireEvent.click(screen.getByTestId('my-identity-card-save'))
      await waitFor(() => {
        expect(
          screen.getByTestId('my-identity-card-save-error'),
        ).toHaveTextContent(/display_name must not be empty/)
      })
      // Form stays open so the user can fix the input.
      expect(screen.getByTestId('my-identity-card-edit-form')).toBeInTheDocument()
    })

    it('Cancel restores the display view without reissuing', async () => {
      getMyIdentityCard.mockResolvedValue(ok(card()))
      render(<MyIdentityCardPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('my-identity-card-edit')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('my-identity-card-edit'))
      fireEvent.change(screen.getByTestId('my-identity-card-draft-name'), {
        target: { value: 'Rename' },
      })
      fireEvent.click(screen.getByTestId('my-identity-card-cancel'))
      expect(reissueMyIdentityCard).not.toHaveBeenCalled()
      expect(screen.queryByTestId('my-identity-card-edit-form')).toBeNull()
    })
  })
})
