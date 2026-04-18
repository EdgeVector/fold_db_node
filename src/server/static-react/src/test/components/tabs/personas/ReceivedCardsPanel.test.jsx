import React from 'react'
import { act, render, screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import ReceivedCardsPanel from '../../../../components/tabs/personas/ReceivedCardsPanel'

vi.mock('../../../../api/clients/fingerprintsClient', () => ({
  listReceivedCards: vi.fn(),
  acceptReceivedCard: vi.fn(),
  dismissReceivedCard: vi.fn(),
}))

import {
  listReceivedCards,
  acceptReceivedCard,
  dismissReceivedCard,
} from '../../../../api/clients/fingerprintsClient'

function row(overrides = {}) {
  return {
    message_id: 'msg_1',
    sender_public_key: 'pk_alice',
    sender_pseudonym: 'ps_alice',
    status: 'pending',
    received_at: '2026-04-18T10:00:00Z',
    resolved_at: null,
    accepted_identity_id: null,
    error: null,
    display_name: 'Alice',
    issued_at: '2026-04-17T12:00:00Z',
    card: { pub_key: 'pk_alice', display_name: 'Alice' },
    ...overrides,
  }
}

function ok(data) {
  return { success: true, data }
}

describe('ReceivedCardsPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('shows the empty-state message when inbox is empty', async () => {
    listReceivedCards.mockResolvedValue(ok({ received_cards: [] }))
    render(<ReceivedCardsPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('received-cards-empty')).toBeInTheDocument()
    })
  })

  it('renders one row per received card with accept + dismiss for pending', async () => {
    listReceivedCards.mockResolvedValue(
      ok({
        received_cards: [row({ message_id: 'msg_a' }), row({ message_id: 'msg_b' })],
      }),
    )
    render(<ReceivedCardsPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('received-card-row-msg_a')).toBeInTheDocument()
    })
    expect(screen.getByTestId('received-card-accept-msg_a')).toBeInTheDocument()
    expect(screen.getByTestId('received-card-dismiss-msg_a')).toBeInTheDocument()
    expect(screen.getByTestId('received-card-accept-msg_b')).toBeInTheDocument()
  })

  it('hides accept + dismiss buttons on already-resolved rows', async () => {
    listReceivedCards.mockResolvedValue(
      ok({
        received_cards: [
          row({
            message_id: 'msg_ok',
            status: 'accepted',
            accepted_identity_id: 'id_alice',
          }),
          row({ message_id: 'msg_no', status: 'dismissed' }),
        ],
      }),
    )
    render(<ReceivedCardsPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('received-card-row-msg_ok')).toBeInTheDocument()
    })
    expect(screen.queryByTestId('received-card-accept-msg_ok')).toBeNull()
    expect(screen.queryByTestId('received-card-dismiss-msg_ok')).toBeNull()
    expect(screen.queryByTestId('received-card-accept-msg_no')).toBeNull()
  })

  it('calls acceptReceivedCard and swaps the row in place on success', async () => {
    listReceivedCards.mockResolvedValue(
      ok({ received_cards: [row({ message_id: 'msg_1' })] }),
    )
    acceptReceivedCard.mockResolvedValue(
      ok({
        received_card: row({
          message_id: 'msg_1',
          status: 'accepted',
          accepted_identity_id: 'id_alice',
          resolved_at: '2026-04-18T10:05:00Z',
        }),
        identity_id: 'id_alice',
      }),
    )
    render(<ReceivedCardsPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('received-card-accept-msg_1')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('received-card-accept-msg_1'))
    await waitFor(() => {
      expect(acceptReceivedCard).toHaveBeenCalledWith('msg_1')
    })
    await waitFor(() => {
      // Row transitioned to accepted, buttons gone, identity link shown.
      expect(screen.queryByTestId('received-card-accept-msg_1')).toBeNull()
    })
    expect(
      screen.getByTestId('received-card-row-msg_1'),
    ).toHaveTextContent('id_alice')
  })

  it('refetches the list when accept fails so the server-side error stamp appears', async () => {
    listReceivedCards
      .mockResolvedValueOnce(ok({ received_cards: [row({ message_id: 'msg_x' })] }))
      .mockResolvedValueOnce(
        ok({
          received_cards: [
            row({
              message_id: 'msg_x',
              status: 'pending',
              error:
                'card_signature does not verify against pub_key + canonical bytes.',
            }),
          ],
        }),
      )
    acceptReceivedCard.mockResolvedValue({
      success: false,
      error: 'card_signature does not verify',
    })
    render(<ReceivedCardsPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('received-card-accept-msg_x')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('received-card-accept-msg_x'))
    await waitFor(() => {
      expect(
        screen.getByTestId('received-card-error-msg_x'),
      ).toHaveTextContent(/does not verify/)
    })
    expect(listReceivedCards).toHaveBeenCalledTimes(2)
  })

  it('auto-refetches every 30s while mounted', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      listReceivedCards.mockResolvedValue(ok({ received_cards: [] }))
      render(<ReceivedCardsPanel />)
      await waitFor(() => {
        expect(listReceivedCards).toHaveBeenCalledTimes(1)
      })

      // No clicks — purely the timer drives the second call.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000)
      })
      expect(listReceivedCards).toHaveBeenCalledTimes(2)

      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000)
      })
      expect(listReceivedCards).toHaveBeenCalledTimes(3)
    } finally {
      vi.useRealTimers()
    }
  })

  it('clears the auto-poll interval on unmount', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      listReceivedCards.mockResolvedValue(ok({ received_cards: [] }))
      const { unmount } = render(<ReceivedCardsPanel />)
      await waitFor(() => {
        expect(listReceivedCards).toHaveBeenCalledTimes(1)
      })
      unmount()
      await act(async () => {
        await vi.advanceTimersByTimeAsync(120_000)
      })
      // Still only the initial mount call — interval was cleared.
      expect(listReceivedCards).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('suppresses the auto-poll while a row is mid-action', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      listReceivedCards.mockResolvedValue(
        ok({ received_cards: [row({ message_id: 'msg_p' })] }),
      )
      // Accept never resolves during the test → pendingAction stays set.
      let releaseAccept
      acceptReceivedCard.mockImplementation(
        () => new Promise(resolve => {
          releaseAccept = resolve
        }),
      )

      render(<ReceivedCardsPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('received-card-accept-msg_p')).toBeInTheDocument()
      })
      expect(listReceivedCards).toHaveBeenCalledTimes(1)

      fireEvent.click(screen.getByTestId('received-card-accept-msg_p'))
      await waitFor(() => {
        expect(acceptReceivedCard).toHaveBeenCalledWith('msg_p')
      })

      // Tick past the poll interval — should NOT refetch while mid-action.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000)
      })
      expect(listReceivedCards).toHaveBeenCalledTimes(1)

      // Release the accept; now the row is no longer mid-action.
      await act(async () => {
        releaseAccept(ok({ received_card: row({ message_id: 'msg_p', status: 'accepted' }), identity_id: 'id_p' }))
      })

      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000)
      })
      expect(listReceivedCards).toHaveBeenCalledTimes(2)
    } finally {
      vi.useRealTimers()
    }
  })

  it('renders a "last refreshed at" line that updates after fetch', async () => {
    listReceivedCards.mockResolvedValue(ok({ received_cards: [] }))
    render(<ReceivedCardsPanel />)
    await waitFor(() => {
      expect(
        screen.getByTestId('received-cards-last-refreshed'),
      ).toHaveTextContent(/last refreshed at \d{2}:\d{2}:\d{2}/)
    })
  })

  it('calls dismissReceivedCard and flips the status on success', async () => {
    listReceivedCards.mockResolvedValue(
      ok({ received_cards: [row({ message_id: 'msg_z' })] }),
    )
    dismissReceivedCard.mockResolvedValue(
      ok(row({ message_id: 'msg_z', status: 'dismissed' })),
    )
    render(<ReceivedCardsPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('received-card-dismiss-msg_z')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('received-card-dismiss-msg_z'))
    await waitFor(() => {
      expect(dismissReceivedCard).toHaveBeenCalledWith('msg_z')
    })
    await waitFor(() => {
      expect(screen.queryByTestId('received-card-dismiss-msg_z')).toBeNull()
    })
  })
})
