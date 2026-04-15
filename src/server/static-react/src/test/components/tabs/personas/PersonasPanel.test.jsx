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
  updatePersona: vi.fn(),
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
  listPersonas,
  getPersona,
  updatePersona,
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

      // Threshold slider is present, editable, and reflects current value.
      const slider = screen.getByTestId('persona-detail-threshold-input')
      expect(slider).not.toBeDisabled()
      expect(slider).not.toHaveAttribute('readonly')
      expect(slider).toHaveValue('0.9')
    })

    it('commits threshold via PATCH when the slider is released', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_me', name: 'Me', built_in: true })]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_me',
            name: 'Me',
            threshold: 0.9,
            built_in: true,
            fingerprint_ids: ['fp_a', 'fp_b'],
          }),
        ),
      )
      updatePersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_me',
            name: 'Me',
            threshold: 0.75,
            built_in: true,
            // After lowering the threshold the cluster grows.
            fingerprint_ids: ['fp_a', 'fp_b', 'fp_c'],
          }),
        ),
      )

      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_me')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_me'))

      await waitFor(() => {
        expect(screen.getByTestId('persona-fingerprints')).toHaveTextContent(
          'Fingerprints: 2',
        )
      })

      const slider = screen.getByTestId('persona-detail-threshold-input')

      // Drag the slider — local state updates, no PATCH yet.
      fireEvent.change(slider, { target: { value: '0.75' } })
      expect(updatePersona).not.toHaveBeenCalled()

      // Release the slider — fires the commit + PATCH.
      fireEvent.mouseUp(slider)
      await waitFor(() => {
        expect(updatePersona).toHaveBeenCalledWith('ps_me', { threshold: 0.75 })
      })

      // Response is swapped into the detail view.
      await waitFor(() => {
        expect(screen.getByTestId('persona-fingerprints')).toHaveTextContent(
          'Fingerprints: 3',
        )
      })

      // List is refetched so the list row counts stay in sync.
      expect(listPersonas).toHaveBeenCalledTimes(2)
    })

    it('does not fire PATCH if the new threshold matches the current one', async () => {
      listPersonas.mockResolvedValue(okList([makeSummary({ id: 'ps_a', name: 'A' })]))
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_a', name: 'A', threshold: 0.85 })),
      )

      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_a')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_a'))

      await waitFor(() => {
        expect(screen.getByTestId('persona-fingerprints')).toBeInTheDocument()
      })

      const slider = screen.getByTestId('persona-detail-threshold-input')
      // Release without changing — commit should be a no-op.
      fireEvent.mouseUp(slider)
      expect(updatePersona).not.toHaveBeenCalled()
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

  describe('exclusions', () => {
    it('sends add_excluded_mention_id when the ✂ mention button is clicked', async () => {
      listPersonas.mockResolvedValue(okList([makeSummary({ id: 'ps_a', name: 'A' })]))
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_a',
            name: 'A',
            mention_ids: ['mn_x'],
            mentions: [
              {
                id: 'mn_x',
                source_schema: 'Photos',
                source_key: 'IMG_1',
                source_field: '',
                extractor: 'face_detect',
                confidence: 0.95,
                created_at: '2026-04-15T10:00:00Z',
              },
            ],
            excluded_mention_ids: [],
            excluded_edge_ids: [],
          }),
        ),
      )
      updatePersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_a',
            name: 'A',
            mention_ids: [],
            mentions: [],
            excluded_mention_ids: ['mn_x'],
            excluded_edge_ids: [],
          }),
        ),
      )

      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_a')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_a'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-mention-exclude-mn_x')).toBeInTheDocument()
      })

      fireEvent.click(screen.getByTestId('persona-mention-exclude-mn_x'))
      await waitFor(() => {
        expect(updatePersona).toHaveBeenCalledWith('ps_a', {
          add_excluded_mention_id: 'mn_x',
        })
      })

      // Exclusions panel now offers an Undo button for that mention.
      await waitFor(() => {
        expect(
          screen.getByTestId('persona-exclusions-toggle'),
        ).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-exclusions-toggle'))
      expect(
        screen.getByTestId('persona-mention-unexclude-mn_x'),
      ).toBeInTheDocument()
    })

    it('sends add_excluded_edge_id when the ✂ edge button is clicked', async () => {
      listPersonas.mockResolvedValue(okList([makeSummary({ id: 'ps_b', name: 'B' })]))
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_b',
            name: 'B',
            edge_ids: ['eg_z'],
            edges: [
              {
                id: 'eg_z',
                a: 'fp_1',
                b: 'fp_2',
                kind: 'StrongMatch',
                weight: 0.9,
                created_at: '2026-04-15T10:00:00Z',
              },
            ],
            excluded_mention_ids: [],
            excluded_edge_ids: [],
          }),
        ),
      )
      updatePersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_b',
            name: 'B',
            edge_ids: [],
            edges: [],
            excluded_mention_ids: [],
            excluded_edge_ids: ['eg_z'],
          }),
        ),
      )

      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_b')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_b'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-edge-exclude-eg_z')).toBeInTheDocument()
      })

      fireEvent.click(screen.getByTestId('persona-edge-exclude-eg_z'))
      await waitFor(() => {
        expect(updatePersona).toHaveBeenCalledWith('ps_b', {
          add_excluded_edge_id: 'eg_z',
        })
      })
    })

    it('sends a PATCH with { name } when the user renames a non-built-in persona', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_rename', name: 'Old' })]),
      )
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_rename', name: 'Old', built_in: false })),
      )
      updatePersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_rename', name: 'New Name', built_in: false })),
      )

      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_rename')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_rename'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-name-button')).toBeInTheDocument()
      })

      fireEvent.click(screen.getByTestId('persona-name-button'))
      const input = screen.getByTestId('persona-name-input')
      fireEvent.change(input, { target: { value: 'New Name' } })
      fireEvent.keyDown(input, { key: 'Enter' })

      await waitFor(() => {
        expect(updatePersona).toHaveBeenCalledWith('ps_rename', {
          name: 'New Name',
        })
      })
    })

    it('hides the name edit affordance for built-in personas', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_me', name: 'Me', built_in: true })]),
      )
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_me', name: 'Me', built_in: true })),
      )

      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_me')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_me'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-name-button')).toBeInTheDocument()
      })

      // Clicking the name button is a no-op for built-in personas —
      // the input never renders.
      fireEvent.click(screen.getByTestId('persona-name-button'))
      expect(screen.queryByTestId('persona-name-input')).toBeNull()
    })

    it('sends a PATCH with { relationship } when the dropdown changes', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_rel', name: 'Rel' })]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_rel',
            name: 'Rel',
            relationship: 'unknown',
          }),
        ),
      )
      updatePersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_rel',
            name: 'Rel',
            relationship: 'friend',
          }),
        ),
      )

      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_rel')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_rel'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-relationship-select')).toBeInTheDocument()
      })
      fireEvent.change(screen.getByTestId('persona-relationship-select'), {
        target: { value: 'friend' },
      })
      await waitFor(() => {
        expect(updatePersona).toHaveBeenCalledWith('ps_rel', {
          relationship: 'friend',
        })
      })
    })

    it('un-excludes a mention through the exclusions panel Undo button', async () => {
      listPersonas.mockResolvedValue(okList([makeSummary({ id: 'ps_c', name: 'C' })]))
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_c',
            name: 'C',
            mentions: [],
            mention_ids: [],
            excluded_mention_ids: ['mn_old'],
            excluded_edge_ids: [],
          }),
        ),
      )
      updatePersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_c',
            name: 'C',
            mentions: [],
            mention_ids: [],
            excluded_mention_ids: [],
            excluded_edge_ids: [],
          }),
        ),
      )

      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_c')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_c'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-exclusions-toggle')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-exclusions-toggle'))
      fireEvent.click(screen.getByTestId('persona-mention-unexclude-mn_old'))

      await waitFor(() => {
        expect(updatePersona).toHaveBeenCalledWith('ps_c', {
          remove_excluded_mention_id: 'mn_old',
        })
      })
    })
  })
})
