import React from 'react'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import MyIdentityCardPanel from '../../../../components/tabs/personas/MyIdentityCardPanel'

vi.mock('../../../../api/clients/fingerprintsClient', () => ({
  getMyIdentityCard: vi.fn(),
}))

import { getMyIdentityCard } from '../../../../api/clients/fingerprintsClient'

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
})
