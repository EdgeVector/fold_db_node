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
  deletePersona: vi.fn(),
  mergePersonas: vi.fn(),
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
  listPersonas,
  getPersona,
  updatePersona,
  deletePersona,
  mergePersonas,
  acceptSuggestedPersona,
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
      expect(diag).toHaveTextContent('2 edges excluded')
      expect(diag).toHaveTextContent('1 UserForbidden edge skipped')
      expect(diag).toHaveTextContent('4 edges below the current threshold')
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

  describe('tentative personas + confirm flow', () => {
    it('renders a tentative badge on unconfirmed, non-built-in personas', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({
            id: 'ps_auto_t1',
            name: 'Tom Tang',
            user_confirmed: false,
            built_in: false,
          }),
        ]),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_auto_t1')).toBeInTheDocument()
      })
      expect(screen.getByTestId('badge-tentative')).toBeInTheDocument()
    })

    it('does NOT render a tentative badge on confirmed personas', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({
            id: 'ps_a',
            name: 'Alice',
            user_confirmed: true,
            built_in: false,
          }),
        ]),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_a')).toBeInTheDocument()
      })
      expect(screen.queryByTestId('badge-tentative')).toBeNull()
    })

    it('shows a Confirm banner + button on tentative persona detail', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({
            id: 'ps_auto_t2',
            name: 'Cluster',
            user_confirmed: false,
            built_in: false,
          }),
        ]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_auto_t2',
            name: 'Cluster',
            user_confirmed: false,
            built_in: false,
          }),
        ),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_auto_t2')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_auto_t2'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-confirm-banner')).toBeInTheDocument()
      })
      expect(screen.getByTestId('persona-confirm-button')).toBeInTheDocument()
    })

    it('fires updatePersona({ user_confirmed: true }) on Confirm click', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({
            id: 'ps_auto_t3',
            name: 'Tentative One',
            user_confirmed: false,
            built_in: false,
          }),
        ]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_auto_t3',
            name: 'Tentative One',
            user_confirmed: false,
            built_in: false,
          }),
        ),
      )
      updatePersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_auto_t3',
            name: 'Tentative One',
            user_confirmed: true,
            built_in: false,
          }),
        ),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_auto_t3')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_auto_t3'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-confirm-button')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-confirm-button'))
      await waitFor(() => {
        expect(updatePersona).toHaveBeenCalledWith('ps_auto_t3', {
          user_confirmed: true,
        })
      })
      // After a successful confirm, the banner should disappear (detail
      // was replaced with user_confirmed=true).
      await waitFor(() => {
        expect(screen.queryByTestId('persona-confirm-banner')).toBeNull()
      })
    })

    it('does NOT show the Confirm banner on built-in (Me) persona', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({
            id: 'ps_me',
            name: 'Me',
            user_confirmed: false, // even if flag is somehow false
            built_in: true,
          }),
        ]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_me',
            name: 'Me',
            user_confirmed: false,
            built_in: true,
          }),
        ),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_me')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_me'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-detail')).toBeInTheDocument()
      })
      expect(screen.queryByTestId('persona-confirm-banner')).toBeNull()
      expect(screen.queryByTestId('badge-tentative')).toBeNull()
    })
  })

  describe('delete flow', () => {
    it('renders the Delete persona button on non-built-in personas', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_d', name: 'Delete Me', built_in: false })]),
      )
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_d', name: 'Delete Me', built_in: false })),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_d')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_d'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-delete-button')).toBeInTheDocument()
      })
    })

    it('hides the Delete persona button on built-in (Me) persona', async () => {
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
        expect(screen.getByTestId('persona-detail')).toBeInTheDocument()
      })
      expect(screen.queryByTestId('persona-delete-button')).toBeNull()
    })

    it('confirms then calls deletePersona, removes row from list', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({ id: 'ps_keep', name: 'Keep', built_in: false }),
          makeSummary({ id: 'ps_del', name: 'Del', built_in: false }),
        ]),
      )
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_del', name: 'Del', built_in: false })),
      )
      deletePersona.mockResolvedValue({
        success: true,
        data: { deleted_persona_id: 'ps_del' },
      })
      // Stub window.confirm to accept.
      const origConfirm = window.confirm
      window.confirm = () => true
      try {
        render(<PersonasPanel />)
        await waitFor(() => {
          expect(screen.getByTestId('persona-row-ps_del')).toBeInTheDocument()
        })
        fireEvent.click(screen.getByTestId('persona-row-ps_del'))
        await waitFor(() => {
          expect(screen.getByTestId('persona-delete-button')).toBeInTheDocument()
        })
        fireEvent.click(screen.getByTestId('persona-delete-button'))
        await waitFor(() => {
          expect(deletePersona).toHaveBeenCalledWith('ps_del')
        })
        // Row gone from list.
        await waitFor(() => {
          expect(screen.queryByTestId('persona-row-ps_del')).toBeNull()
        })
        // Other row still there.
        expect(screen.getByTestId('persona-row-ps_keep')).toBeInTheDocument()
      } finally {
        window.confirm = origConfirm
      }
    })

    it('skips the delete when the user cancels the confirm dialog', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_c', name: 'Keep', built_in: false })]),
      )
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_c', name: 'Keep', built_in: false })),
      )
      const origConfirm = window.confirm
      window.confirm = () => false
      try {
        render(<PersonasPanel />)
        await waitFor(() => {
          expect(screen.getByTestId('persona-row-ps_c')).toBeInTheDocument()
        })
        fireEvent.click(screen.getByTestId('persona-row-ps_c'))
        await waitFor(() => {
          expect(screen.getByTestId('persona-delete-button')).toBeInTheDocument()
        })
        fireEvent.click(screen.getByTestId('persona-delete-button'))
        expect(deletePersona).not.toHaveBeenCalled()
        // Row still there.
        expect(screen.getByTestId('persona-row-ps_c')).toBeInTheDocument()
      } finally {
        window.confirm = origConfirm
      }
    })
  })

  describe('delete undo', () => {
    // Shared setup helper — render the panel, select the persona, and
    // run the delete flow with `window.confirm` accepted. Returns the
    // saved confirm so each test can restore it in finally.
    async function deleteAndAwaitSnack({
      personaId = 'ps_undo',
      name = 'UndoMe',
      seeds = ['fp_a', 'fp_b', 'fp_c'],
      relationship = 'friend',
    } = {}) {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: personaId, name, built_in: false, relationship })]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: personaId,
            name,
            built_in: false,
            relationship,
            seed_fingerprint_ids: seeds,
          }),
        ),
      )
      deletePersona.mockResolvedValue({
        success: true,
        data: { deleted_persona_id: personaId },
      })
      const origConfirm = window.confirm
      window.confirm = () => true
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId(`persona-row-${personaId}`)).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId(`persona-row-${personaId}`))
      await waitFor(() => {
        expect(screen.getByTestId('persona-delete-button')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-delete-button'))
      await waitFor(() => {
        expect(deletePersona).toHaveBeenCalledWith(personaId)
      })
      return { origConfirm, seeds, name, relationship }
    }

    it('shows the delete-undo snack after a successful delete', async () => {
      const { origConfirm } = await deleteAndAwaitSnack()
      try {
        await waitFor(() => {
          expect(
            screen.getByTestId('persona-delete-undo-snack'),
          ).toBeInTheDocument()
        })
        // Both the snack and the Undo button must be findable by
        // their distinct test ids — separate from the exclude snack.
        expect(
          screen.getByTestId('persona-delete-undo-button'),
        ).toBeInTheDocument()
        expect(screen.queryByTestId('persona-undo-snack')).toBeNull()
      } finally {
        window.confirm = origConfirm
      }
    })

    it('auto-dismisses the delete-undo snack after 5 seconds', async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true })
      try {
        const { origConfirm } = await deleteAndAwaitSnack()
        try {
          await waitFor(() => {
            expect(
              screen.getByTestId('persona-delete-undo-snack'),
            ).toBeInTheDocument()
          })
          vi.advanceTimersByTime(5100)
          await waitFor(() => {
            expect(
              screen.queryByTestId('persona-delete-undo-snack'),
            ).toBeNull()
          })
        } finally {
          window.confirm = origConfirm
        }
      } finally {
        vi.useRealTimers()
      }
    })

    it('Undo click invokes acceptSuggestedPersona with captured seeds + name + relationship', async () => {
      const { origConfirm, seeds, name, relationship } = await deleteAndAwaitSnack()
      try {
        await waitFor(() => {
          expect(
            screen.getByTestId('persona-delete-undo-button'),
          ).toBeInTheDocument()
        })
        // Restored persona comes back with a fresh server-allocated id.
        const restored = makeDetail({
          id: 'ps_undo_restored',
          name,
          relationship,
          seed_fingerprint_ids: seeds,
        })
        acceptSuggestedPersona.mockResolvedValue({
          success: true,
          data: restored,
        })
        // The fetchList that runs after restore needs a non-rejecting
        // mock so the post-undo refresh doesn't blow up.
        listPersonas.mockResolvedValue(
          okList([
            makeSummary({
              id: 'ps_undo_restored',
              name,
              relationship,
              built_in: false,
            }),
          ]),
        )
        fireEvent.click(screen.getByTestId('persona-delete-undo-button'))
        await waitFor(() => {
          expect(acceptSuggestedPersona).toHaveBeenCalledTimes(1)
        })
        expect(acceptSuggestedPersona).toHaveBeenCalledWith({
          fingerprint_ids: seeds,
          name,
          relationship,
        })
        // Snack disappears once restore succeeds.
        await waitFor(() => {
          expect(
            screen.queryByTestId('persona-delete-undo-snack'),
          ).toBeNull()
        })
      } finally {
        window.confirm = origConfirm
      }
    })

    it('surfaces an error message in the snack when restore fails', async () => {
      const { origConfirm } = await deleteAndAwaitSnack()
      try {
        await waitFor(() => {
          expect(
            screen.getByTestId('persona-delete-undo-button'),
          ).toBeInTheDocument()
        })
        acceptSuggestedPersona.mockResolvedValue({
          success: false,
          error: 'Backend rejected restore',
        })
        fireEvent.click(screen.getByTestId('persona-delete-undo-button'))
        await waitFor(() => {
          expect(
            screen.getByTestId('persona-delete-undo-error'),
          ).toHaveTextContent('Backend rejected restore')
        })
        // Snack itself stays visible so the user can read the error.
        expect(
          screen.getByTestId('persona-delete-undo-snack'),
        ).toBeInTheDocument()
      } finally {
        window.confirm = origConfirm
      }
    })
  })

  describe('unlink identity flow', () => {
    it('shows the Unlink identity button on verified non-built-in personas', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_v', name: 'Verified', built_in: false })]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_v',
            name: 'Verified',
            built_in: false,
            identity_id: 'id_abc',
          }),
        ),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_v')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_v'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-unlink-identity-button')).toBeInTheDocument()
      })
    })

    it('hides the Unlink button when identity_id is null', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_u', name: 'Unverified' })]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({ id: 'ps_u', name: 'Unverified', identity_id: null }),
        ),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_u')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_u'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-detail')).toBeInTheDocument()
      })
      expect(screen.queryByTestId('persona-unlink-identity-button')).toBeNull()
    })

    it('hides the Unlink button on built-in (Me) persona even when linked', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_me', name: 'Me', built_in: true })]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_me',
            name: 'Me',
            built_in: true,
            identity_id: 'id_self',
          }),
        ),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_me')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_me'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-detail')).toBeInTheDocument()
      })
      expect(screen.queryByTestId('persona-unlink-identity-button')).toBeNull()
    })

    it('confirms then calls updatePersona with clear_identity_id=true', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_v', name: 'Verified', built_in: false })]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_v',
            name: 'Verified',
            built_in: false,
            identity_id: 'id_abc',
          }),
        ),
      )
      updatePersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_v',
            name: 'Verified',
            built_in: false,
            identity_id: null,
          }),
        ),
      )
      const origConfirm = window.confirm
      window.confirm = () => true
      try {
        render(<PersonasPanel />)
        await waitFor(() => {
          expect(screen.getByTestId('persona-row-ps_v')).toBeInTheDocument()
        })
        fireEvent.click(screen.getByTestId('persona-row-ps_v'))
        await waitFor(() => {
          expect(screen.getByTestId('persona-unlink-identity-button')).toBeInTheDocument()
        })
        fireEvent.click(screen.getByTestId('persona-unlink-identity-button'))
        await waitFor(() => {
          expect(updatePersona).toHaveBeenCalledWith('ps_v', {
            clear_identity_id: true,
          })
        })
      } finally {
        window.confirm = origConfirm
      }
    })

    it('does nothing when the user cancels the Unlink confirm dialog', async () => {
      listPersonas.mockResolvedValue(
        okList([makeSummary({ id: 'ps_v', name: 'Verified', built_in: false })]),
      )
      getPersona.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_v',
            name: 'Verified',
            built_in: false,
            identity_id: 'id_abc',
          }),
        ),
      )
      const origConfirm = window.confirm
      window.confirm = () => false
      try {
        render(<PersonasPanel />)
        await waitFor(() => {
          expect(screen.getByTestId('persona-row-ps_v')).toBeInTheDocument()
        })
        fireEvent.click(screen.getByTestId('persona-row-ps_v'))
        await waitFor(() => {
          expect(screen.getByTestId('persona-unlink-identity-button')).toBeInTheDocument()
        })
        fireEvent.click(screen.getByTestId('persona-unlink-identity-button'))
        expect(updatePersona).not.toHaveBeenCalled()
      } finally {
        window.confirm = origConfirm
      }
    })
  })

  describe('merge flow', () => {
    it('shows the merge dropdown with other non-built-in personas as options', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({ id: 'ps_a', name: 'Alice', built_in: false }),
          makeSummary({ id: 'ps_b', name: 'Alice Smith', built_in: false }),
          makeSummary({ id: 'ps_me', name: 'Me', built_in: true }),
        ]),
      )
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_a', name: 'Alice', built_in: false })),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_a')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_a'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-merge-select')).toBeInTheDocument()
      })
      const select = screen.getByTestId('persona-merge-select')
      // The selected persona is excluded, and built-in Me is excluded.
      expect(select).toHaveTextContent('Alice Smith')
      expect(select).not.toHaveTextContent('Me · ')
    })

    it('hides the merge control on built-in personas', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({ id: 'ps_me', name: 'Me', built_in: true }),
          makeSummary({ id: 'ps_a', name: 'Alice', built_in: false }),
        ]),
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
        expect(screen.getByTestId('persona-detail')).toBeInTheDocument()
      })
      expect(screen.queryByTestId('persona-merge-control')).toBeNull()
    })

    it('hides the merge control when no other non-built-in personas exist', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({ id: 'ps_me', name: 'Me', built_in: true }),
          makeSummary({ id: 'ps_lonely', name: 'Lonely', built_in: false }),
        ]),
      )
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_lonely', name: 'Lonely', built_in: false })),
      )
      render(<PersonasPanel />)
      await waitFor(() => {
        expect(screen.getByTestId('persona-row-ps_lonely')).toBeInTheDocument()
      })
      fireEvent.click(screen.getByTestId('persona-row-ps_lonely'))
      await waitFor(() => {
        expect(screen.getByTestId('persona-detail')).toBeInTheDocument()
      })
      expect(screen.queryByTestId('persona-merge-control')).toBeNull()
    })

    it('confirms then calls mergePersonas(survivor, absorbed), drops the absorbed row', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({ id: 'ps_survivor', name: 'Alice' }),
          makeSummary({ id: 'ps_absorbed', name: 'Alice Smith' }),
        ]),
      )
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_survivor', name: 'Alice' })),
      )
      mergePersonas.mockResolvedValue(
        okDetail(
          makeDetail({
            id: 'ps_survivor',
            name: 'Alice',
            aliases: ['Alice Smith'],
          }),
        ),
      )
      const origConfirm = window.confirm
      window.confirm = () => true
      try {
        render(<PersonasPanel />)
        await waitFor(() => {
          expect(screen.getByTestId('persona-row-ps_survivor')).toBeInTheDocument()
        })
        fireEvent.click(screen.getByTestId('persona-row-ps_survivor'))
        await waitFor(() => {
          expect(screen.getByTestId('persona-merge-select')).toBeInTheDocument()
        })
        fireEvent.change(screen.getByTestId('persona-merge-select'), {
          target: { value: 'ps_absorbed' },
        })
        fireEvent.click(screen.getByTestId('persona-merge-button'))
        await waitFor(() => {
          expect(mergePersonas).toHaveBeenCalledWith('ps_survivor', 'ps_absorbed')
        })
        // Absorbed row disappears from the list.
        await waitFor(() => {
          expect(screen.queryByTestId('persona-row-ps_absorbed')).toBeNull()
        })
      } finally {
        window.confirm = origConfirm
      }
    })

    it('skips the merge when the user cancels the confirm dialog', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({ id: 'ps_a', name: 'Alice' }),
          makeSummary({ id: 'ps_b', name: 'Alice Smith' }),
        ]),
      )
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_a', name: 'Alice' })),
      )
      const origConfirm = window.confirm
      window.confirm = () => false
      try {
        render(<PersonasPanel />)
        await waitFor(() => {
          expect(screen.getByTestId('persona-row-ps_a')).toBeInTheDocument()
        })
        fireEvent.click(screen.getByTestId('persona-row-ps_a'))
        await waitFor(() => {
          expect(screen.getByTestId('persona-merge-select')).toBeInTheDocument()
        })
        fireEvent.change(screen.getByTestId('persona-merge-select'), {
          target: { value: 'ps_b' },
        })
        fireEvent.click(screen.getByTestId('persona-merge-button'))
        expect(mergePersonas).not.toHaveBeenCalled()
      } finally {
        window.confirm = origConfirm
      }
    })

    it('surfaces backend error messages verbatim on merge failure', async () => {
      listPersonas.mockResolvedValue(
        okList([
          makeSummary({ id: 'ps_a', name: 'Alice' }),
          makeSummary({ id: 'ps_b', name: 'Bob' }),
        ]),
      )
      getPersona.mockResolvedValue(
        okDetail(makeDetail({ id: 'ps_a', name: 'Alice' })),
      )
      mergePersonas.mockResolvedValue({
        success: false,
        error:
          'both personas are linked to different verified identities (id_x vs id_y); unlink one before merging',
      })
      const origConfirm = window.confirm
      window.confirm = () => true
      try {
        render(<PersonasPanel />)
        await waitFor(() => {
          expect(screen.getByTestId('persona-row-ps_a')).toBeInTheDocument()
        })
        fireEvent.click(screen.getByTestId('persona-row-ps_a'))
        await waitFor(() => {
          expect(screen.getByTestId('persona-merge-select')).toBeInTheDocument()
        })
        fireEvent.change(screen.getByTestId('persona-merge-select'), {
          target: { value: 'ps_b' },
        })
        fireEvent.click(screen.getByTestId('persona-merge-button'))
        await waitFor(() => {
          expect(screen.getByTestId('persona-detail-error')).toHaveTextContent(
            /different verified identities/,
          )
        })
      } finally {
        window.confirm = origConfirm
      }
    })
  })
})
