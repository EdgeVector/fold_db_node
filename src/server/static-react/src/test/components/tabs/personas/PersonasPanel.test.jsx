import React from 'react'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import PersonasPanel from '../../../../components/tabs/personas/PersonasPanel'

// Mock the fingerprints client — tests exercise the component logic
// without hitting the real backend. Each test sets up its own
// expected responses via the mocked functions.
vi.mock('../../../../api/clients/fingerprintsClient', () => ({
  listPersonas: vi.fn(),
  getPersona: vi.fn(),
}))

import {
  listPersonas,
  getPersona,
} from '../../../../api/clients/fingerprintsClient'

// ── Test data builders ─────────────────────────────────────────────

function makeSummary(overrides = {}) {
  return {
    id: 'ps_default',
    name: 'Default',
    identity_linked: false,
    threshold: 0.85,
    relationship: 'unknown',
    trust_tier: 0,
    built_in: false,
    user_confirmed: false,
    fingerprint_count: 0,
    edge_count: 0,
    mention_count: 0,
    ...overrides,
  }
}

function makeDetail(overrides = {}) {
  return {
    id: 'ps_default',
    name: 'Default',
    threshold: 0.85,
    relationship: 'unknown',
    trust_tier: 0,
    built_in: false,
    user_confirmed: false,
    identity_id: null,
    seed_fingerprint_ids: [],
    aliases: [],
    fingerprint_ids: [],
    edge_ids: [],
    mention_ids: [],
    diagnostics: null,
    ...overrides,
  }
}

function okList(personas) {
  return { success: true, data: { personas } }
}

function okDetail(detail) {
  return { success: true, data: detail }
}

// ── Tests ──────────────────────────────────────────────────────────

describe('PersonasPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  describe('list view', () => {
    it('shows an empty-state message when the node has no personas', async () => {
      listPersonas.mockResolvedValue(okList([]))
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-list-empty')).toBeInTheDocument()
      })
    })

    it('renders each persona with built-in and verified badges when applicable', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({
            id: 'ps_me',
            name: 'Me',
            built_in: true,
            identity_linked: true,
            relationship: 'self',
            trust_tier: 4,
            threshold: 0.9,
            fingerprint_count: 3,
            edge_count: 2,
            mention_count: 5,
          }),
          makeSummary({
            id: 'ps_alice',
            name: 'Alice',
            relationship: 'friend',
            fingerprint_count: 12,
            edge_count: 8,
            mention_count: 20,
          }),
        ]),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_me')).toBeInTheDocument()
      })

      const meRow = screen.getByTestId('persona-row-ps_me')
      expect(meRow).toHaveTextContent('Me')
      expect(meRow).toHaveTextContent('self · 3 fps · 2 edges · 5 mentions')
      // Me has both built-in and verified badges
      expect(meRow.querySelector('[data-testid="badge-built-in"]')).not.toBeNull()
      expect(meRow.querySelector('[data-testid="badge-verified"]')).not.toBeNull()

      const aliceRow = screen.getByTestId('persona-row-ps_alice')
      // Alice has neither
      expect(aliceRow.querySelector('[data-testid="badge-built-in"]')).toBeNull()
      expect(aliceRow.querySelector('[data-testid="badge-verified"]')).toBeNull()
      expect(aliceRow).toHaveTextContent('friend · 12 fps · 8 edges · 20 mentions')
    })

    it('surfaces a list-level error message when the API fails', async () => {
      listPersonas.mockResolvedValue({ success: false, error: 'boom' })
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-list-error')).toHaveTextContent('boom')
      })
    })

    it('refresh button refetches the list', async () => {
      listPersonas.mockResolvedValue(okList([]))
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-list-empty')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-list-refresh'))
      await waitFor(() => {
        expect(listPersonas).toHaveBeenCalledTimes(2)
      })
    })
  })

  describe('detail view', () => {
    it('shows a placeholder until a persona is selected', async () => {
      listPersonas.mockResolvedValue(okList([makeSummary({ id: 'ps_a', name: 'A' })]))
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-detail-placeholder')).toBeInTheDocument()
      })
    })

    it('fetches detail when a persona is clicked and renders counts', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_me', name: 'Me', built_in: true })]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_me',
            name: 'Me',
            threshold: 0.9,
            relationship: 'self',
            trust_tier: 4,
            built_in: true,
            user_confirmed: true,
            identity_id: 'id_pubkey',
            seed_fingerprint_ids: ['fp_seed'],
            fingerprint_ids: ['fp_a', 'fp_b', 'fp_c'],
            edge_ids: ['eg_1', 'eg_2'],
            mention_ids: ['mn_1'],
            diagnostics: null,
          }),
        ),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_me')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_me'))

      await waitFor(() => {
        expect(getPersona).toHaveBeenCalledWith('ps_me')
        expect(screen.getByTestId('persona-fingerprints')).toHaveTextContent(
          'Fingerprints: 3',
        )
      })
      expect(screen.getByTestId('persona-detail')).toBeInTheDocument()
      expect(screen.getByTestId('persona-edges')).toHaveTextContent('Edges: 2')
      expect(screen.getByTestId('persona-mentions')).toHaveTextContent('Mentions: 1')

      // Threshold slider is present and read-only.
      const slider = screen.getByTestId('persona-detail-threshold-input')
      expect(slider).toBeDisabled()
      expect(slider).toHaveAttribute('readonly')
      expect(slider).toHaveValue('0.9')
    })

    it('renders diagnostics block when the resolver surfaced any filter hits', async () => {
      listPersonas.mockResolvedValue(okList([makeSummary({ id: 'ps_a', name: 'A' })]))
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_a',
            name: 'A',
            diagnostics: {
              missing_seed_fingerprint_ids: [],
              excluded_edge_count: 2,
              forbidden_edge_count: 1,
              below_threshold_edge_count: 4,
              excluded_mention_count: 0,
              dangling_edge_ids: [],
            },
          }),
        ),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_a')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_a'))

      await waitFor(() => {
        expect(screen.getByTestId('persona-detail-diagnostics')).toBeInTheDocument()
      })
      const diag = screen.getByTestId('persona-detail-diagnostics')
      expect(diag).toHaveTextContent('2 edge(s) excluded')
      expect(diag).toHaveTextContent('1 UserForbidden edge(s) skipped')
      expect(diag).toHaveTextContent('4 edge(s) below the current threshold')
    })

    it('does NOT render diagnostics block when resolver was clean', async () => {
      listPersonas.mockResolvedValue(okList([makeSummary({ id: 'ps_a', name: 'A' })]))
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_a', name: 'A', diagnostics: null })),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_a')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_a'))

      await waitFor(() => {
        expect(screen.getByTestId('persona-detail')).toBeInTheDocument()
      })
      expect(screen.queryByTestId('persona-detail-diagnostics')).toBeNull()
    })

    it('shows detail-level error when the get call fails', async () => {
      listPersonas.mockResolvedValue(okList([makeSummary({ id: 'ps_a', name: 'A' })]))
      getPersona.mockResolvedValue({ success: false, error: 'nope' })
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_a')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_a'))

      await waitFor(() => {
        expect(screen.getByTestId('persona-detail-error')).toHaveTextContent('nope')
      })
    })
  })
})
