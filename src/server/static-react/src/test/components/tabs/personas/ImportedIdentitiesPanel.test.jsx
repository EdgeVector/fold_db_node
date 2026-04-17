import React from 'react'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import ImportedIdentitiesPanel from '../../../../components/tabs/personas/ImportedIdentitiesPanel'

vi.mock('../../../../api/clients/fingerprintsClient', () => ({
  listIdentities: vi.fn(),
}))

import { listIdentities } from '../../../../api/clients/fingerprintsClient'

function row(overrides = {}) {
  return {
    identity_id: 'id_abc',
    pub_key: 'pk_abc',
    display_name: 'Alice',
    issued_at: '2026-04-10T12:00:00Z',
    received_via: 'Paste',
    received_at: '2026-04-15T12:00:00Z',
    trust_level: 'Attested',
    is_self: false,
    ...overrides,
  }
}

function ok(data) {
  return { success: true, data }
}

describe('ImportedIdentitiesPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('shows the empty-state message when the node has no identities', async () => {
    listIdentities.mockResolvedValue(ok({ identities: [] }))
    render(<ImportedIdentitiesPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('imported-identities-empty')).toBeInTheDocument()
    })
  })

  it('renders one row per identity with display_name, trust_level, and receipt info', async () => {
    listIdentities.mockResolvedValue(
      ok({
        identities: [
          row({
            identity_id: 'id_alice',
            display_name: 'Alice',
            received_via: 'Paste',
            trust_level: 'Attested',
            is_self: false,
          }),
        ],
      }),
    )
    render(<ImportedIdentitiesPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('imported-identity-row-id_alice')).toBeInTheDocument()
    })
    const r = screen.getByTestId('imported-identity-row-id_alice')
    expect(r).toHaveTextContent('Alice')
    expect(r).toHaveTextContent('Attested')
    expect(r).toHaveTextContent('Paste')
    expect(r).toHaveTextContent('id_alice')
  })

  it("marks the node's own self-Identity with a 'you' badge", async () => {
    listIdentities.mockResolvedValue(
      ok({
        identities: [
          row({
            identity_id: 'id_me',
            display_name: 'Me',
            trust_level: 'Self',
            received_via: 'Self',
            is_self: true,
          }),
          row({
            identity_id: 'id_bob',
            display_name: 'Bob',
            is_self: false,
          }),
        ],
      }),
    )
    render(<ImportedIdentitiesPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('imported-identity-row-id_me')).toBeInTheDocument()
    })
    const meRow = screen.getByTestId('imported-identity-row-id_me')
    expect(meRow.querySelector('[data-testid="badge-self"]')).not.toBeNull()
    const bobRow = screen.getByTestId('imported-identity-row-id_bob')
    expect(bobRow.querySelector('[data-testid="badge-self"]')).toBeNull()
  })

  it('handles identities with no receipt gracefully', async () => {
    listIdentities.mockResolvedValue(
      ok({
        identities: [
          row({
            identity_id: 'id_orphan',
            received_via: null,
            received_at: null,
            trust_level: null,
          }),
        ],
      }),
    )
    render(<ImportedIdentitiesPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('imported-identity-row-id_orphan')).toBeInTheDocument()
    })
    expect(screen.getByTestId('imported-identity-row-id_orphan')).toHaveTextContent(
      'no receipt',
    )
  })

  it('surfaces a list-level error when the API fails', async () => {
    listIdentities.mockResolvedValue({ success: false, error: 'boom' })
    render(<ImportedIdentitiesPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('imported-identities-error')).toHaveTextContent('boom')
    })
  })

  it('refetches the list when the refresh button is clicked', async () => {
    listIdentities.mockResolvedValue(ok({ identities: [] }))
    render(<ImportedIdentitiesPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('imported-identities-empty')).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId('imported-identities-refresh'))
    await waitFor(() => {
      expect(listIdentities).toHaveBeenCalledTimes(2)
    })
  })
})
