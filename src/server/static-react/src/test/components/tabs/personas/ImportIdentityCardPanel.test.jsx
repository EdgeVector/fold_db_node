import React from 'react'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import ImportIdentityCardPanel from '../../../../components/tabs/personas/ImportIdentityCardPanel'

vi.mock('../../../../api/clients/fingerprintsClient', () => ({
  importIdentityCard: vi.fn(),
  listPersonas: vi.fn(),
}))

import {
  importIdentityCard,
  listPersonas,
} from '../../../../api/clients/fingerprintsClient'

function ok(data) {
  return { success: true, data }
}

function fail(msg) {
  return { success: false, error: msg }
}

function samplePersona(overrides = {}) {
  return {
    id: 'ps_default',
    name: 'Alice',
    identity_linked: false,
    threshold: 0.85,
    relationship: 'friend',
    trust_tier: 2,
    built_in: false,
    user_confirmed: true,
    fingerprint_count: 0,
    edge_count: 0,
    mention_count: 0,
    ...overrides,
  }
}

const cardJson = JSON.stringify({
  pub_key: 'pk_base64',
  display_name: 'Alice',
  birthday: null,
  face_embedding: null,
  node_id: 'pk_base64',
  card_signature: 'sig_base64',
  issued_at: '2026-04-17T12:00:00Z',
})

describe('ImportIdentityCardPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    listPersonas.mockResolvedValue(ok({ personas: [] }))
  })

  it('renders the form with textarea, persona select, and submit button', async () => {
    render(<ImportIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('import-identity-card-panel')).toBeInTheDocument()
    })
    expect(screen.getByTestId('import-identity-card-textarea')).toBeInTheDocument()
    expect(
      screen.getByTestId('import-identity-card-persona-select'),
    ).toBeInTheDocument()
    expect(screen.getByTestId('import-identity-card-submit')).toBeInTheDocument()
  })

  it('disables the submit button while the textarea is empty', async () => {
    render(<ImportIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('import-identity-card-submit')).toBeInTheDocument()
    })
    expect(screen.getByTestId('import-identity-card-submit')).toBeDisabled()
  })

  it('populates the persona dropdown from listPersonas', async () => {
    listPersonas.mockResolvedValue(
      ok({
        personas: [
          samplePersona({ id: 'ps_alice', name: 'Alice' }),
          samplePersona({ id: 'ps_bob', name: 'Bob', relationship: 'colleague' }),
        ],
      }),
    )
    render(<ImportIdentityCardPanel />)
    await waitFor(() => {
      expect(
        screen.getByTestId('import-identity-card-persona-select'),
      ).toHaveTextContent('Alice')
    })
    expect(
      screen.getByTestId('import-identity-card-persona-select'),
    ).toHaveTextContent('Bob')
  })

  it('surfaces a local JSON parse error before hitting the backend', async () => {
    render(<ImportIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('import-identity-card-submit')).toBeInTheDocument()
    })
    const textarea = screen.getByTestId('import-identity-card-textarea')
    fireEvent.change(textarea, { target: { value: 'not json' } })
    fireEvent.click(screen.getByTestId('import-identity-card-submit'))
    await waitFor(() => {
      expect(screen.getByTestId('import-identity-card-error')).toHaveTextContent(
        /JSON/i,
      )
    })
    expect(importIdentityCard).not.toHaveBeenCalled()
  })

  it('submits valid JSON, shows the success result, and clears the textarea', async () => {
    importIdentityCard.mockResolvedValue(
      ok({
        identity_id: 'id_pk_base64',
        verified: true,
        was_already_present: false,
        linked_persona: null,
      }),
    )
    render(<ImportIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('import-identity-card-submit')).toBeInTheDocument()
    })
    const textarea = screen.getByTestId('import-identity-card-textarea')
    fireEvent.change(textarea, { target: { value: cardJson } })
    fireEvent.click(screen.getByTestId('import-identity-card-submit'))

    await waitFor(() => {
      expect(importIdentityCard).toHaveBeenCalledTimes(1)
    })
    const arg = importIdentityCard.mock.calls[0][0]
    expect(arg.card.pub_key).toBe('pk_base64')
    expect(arg.link_persona_id).toBeUndefined()

    await waitFor(() => {
      expect(screen.getByTestId('import-identity-card-result')).toBeInTheDocument()
    })
    expect(screen.getByTestId('import-identity-card-result')).toHaveTextContent(
      'id_pk_base64',
    )
    // Textarea is cleared after success.
    expect(screen.getByTestId('import-identity-card-textarea')).toHaveValue('')
  })

  it('passes link_persona_id when the user picks a persona', async () => {
    listPersonas.mockResolvedValue(
      ok({
        personas: [samplePersona({ id: 'ps_alice', name: 'Alice' })],
      }),
    )
    importIdentityCard.mockResolvedValue(
      ok({
        identity_id: 'id_pk_base64',
        verified: true,
        was_already_present: false,
        linked_persona: {
          id: 'ps_alice',
          name: 'Alice',
          threshold: 0.85,
          relationship: 'friend',
          trust_tier: 2,
          built_in: false,
          user_confirmed: true,
          identity_id: 'id_pk_base64',
          seed_fingerprint_ids: [],
          aliases: [],
          excluded_edge_ids: [],
          excluded_mention_ids: [],
          fingerprint_ids: [],
          edge_ids: [],
          mention_ids: [],
          fingerprints: [],
          edges: [],
          mentions: [],
          diagnostics: null,
        },
      }),
    )
    render(<ImportIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('import-identity-card-persona-select')).toHaveTextContent(
        'Alice',
      )
    })
    fireEvent.change(screen.getByTestId('import-identity-card-persona-select'), {
      target: { value: 'ps_alice' },
    })
    fireEvent.change(screen.getByTestId('import-identity-card-textarea'), {
      target: { value: cardJson },
    })
    fireEvent.click(screen.getByTestId('import-identity-card-submit'))

    await waitFor(() => {
      expect(importIdentityCard).toHaveBeenCalledWith(
        expect.objectContaining({ link_persona_id: 'ps_alice' }),
      )
    })
    await waitFor(() => {
      expect(screen.getByTestId('import-identity-card-result')).toHaveTextContent(
        /Linked persona/i,
      )
    })
  })

  it('surfaces backend error messages verbatim', async () => {
    importIdentityCard.mockResolvedValue(
      fail('card_signature does not verify against pub_key + canonical bytes'),
    )
    render(<ImportIdentityCardPanel />)
    await waitFor(() => {
      expect(screen.getByTestId('import-identity-card-submit')).toBeInTheDocument()
    })
    fireEvent.change(screen.getByTestId('import-identity-card-textarea'), {
      target: { value: cardJson },
    })
    fireEvent.click(screen.getByTestId('import-identity-card-submit'))
    await waitFor(() => {
      expect(screen.getByTestId('import-identity-card-error')).toHaveTextContent(
        /card_signature does not verify/,
      )
    })
  })
})
