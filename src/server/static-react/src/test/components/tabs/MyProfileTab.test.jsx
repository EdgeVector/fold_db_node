import React from 'react'
import { screen, fireEvent, waitFor } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import MyProfileTab from '../../../components/tabs/MyProfileTab'
import { renderWithRedux } from '../../utils/testStore.jsx'

vi.mock('../../../api/clients/discoveryClient', () => {
  const getInterests = vi.fn()
  const detectInterests = vi.fn()
  const toggleInterest = vi.fn()
  return {
    discoveryClient: { getInterests, detectInterests, toggleInterest },
    DiscoveryClient: vi.fn(),
    default: { getInterests, detectInterests, toggleInterest },
  }
})

import { discoveryClient } from '../../../api/clients/discoveryClient'

const MOCK_PROFILE = {
  categories: [
    { name: 'Software Engineering', count: 42, avg_similarity: 0.65, enabled: true },
    { name: 'Music', count: 28, avg_similarity: 0.58, enabled: true },
    { name: 'Travel', count: 15, avg_similarity: 0.51, enabled: true },
    { name: 'Cooking', count: 10, avg_similarity: 0.45, enabled: false },
    { name: 'Reading', count: 7, avg_similarity: 0.40, enabled: true },
  ],
  total_embeddings_scanned: 200,
  unmatched_count: 98,
  detected_at: '2026-03-29T12:00:00Z',
  seed_version: 1,
}

const EMPTY_PROFILE = {
  categories: [],
  total_embeddings_scanned: 0,
  unmatched_count: 0,
  detected_at: '2026-03-29T12:00:00Z',
  seed_version: 0,
}

function render(onResult = vi.fn()) {
  return renderWithRedux(<MyProfileTab onResult={onResult} />)
}

describe('MyProfileTab', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  describe('loading state', () => {
    it('shows loading spinner while fetching profile', () => {
      discoveryClient.getInterests.mockReturnValue(new Promise(() => {})) // never resolves
      render()
      expect(screen.getByText('Loading your interest fingerprint...')).toBeInTheDocument()
    })
  })

  describe('empty state', () => {
    it('shows empty state when no profile exists', async () => {
      discoveryClient.getInterests.mockResolvedValue({
        success: true,
        data: EMPTY_PROFILE,
      })

      render()

      await waitFor(() => {
        expect(screen.getByText('No Interest Fingerprint Yet')).toBeInTheDocument()
      })
      expect(screen.getByText('Generate Fingerprint')).toBeInTheDocument()
    })

    it('triggers detection and re-fetches profile when Generate Fingerprint is clicked', async () => {
      // First call returns empty, second call (after detect) returns full profile
      discoveryClient.getInterests
        .mockResolvedValueOnce({ success: true, data: EMPTY_PROFILE })
        .mockResolvedValueOnce({ success: true, data: MOCK_PROFILE })
      discoveryClient.detectInterests.mockResolvedValue({
        success: true,
        data: MOCK_PROFILE,
      })

      const onResult = vi.fn()
      render(onResult)

      await waitFor(() => {
        expect(screen.getByText('Generate Fingerprint')).toBeInTheDocument()
      })

      fireEvent.click(screen.getByText('Generate Fingerprint'))

      await waitFor(() => {
        expect(discoveryClient.detectInterests).toHaveBeenCalled()
      })

      // After detection, loadProfile is called to re-fetch canonical data
      await waitFor(() => {
        expect(discoveryClient.getInterests).toHaveBeenCalledTimes(2)
      })

      await waitFor(() => {
        expect(onResult).toHaveBeenCalledWith(
          expect.objectContaining({ success: true })
        )
      })

      // Profile should now be displayed (radar chart, stats, etc.)
      await waitFor(() => {
        expect(screen.getByText('Interest Fingerprint')).toBeInTheDocument()
        expect(screen.getByText('5')).toBeInTheDocument() // interests detected
      })
    })
  })

  describe('profile display', () => {
    beforeEach(() => {
      discoveryClient.getInterests.mockResolvedValue({
        success: true,
        data: MOCK_PROFILE,
      })
    })

    it('shows stats cards with correct values', async () => {
      render()

      await waitFor(() => {
        expect(screen.getByText('5')).toBeInTheDocument() // interests detected
        expect(screen.getByText('200')).toBeInTheDocument() // data points scanned
        expect(screen.getByText('51%')).toBeInTheDocument() // coverage (102/200)
      })
    })

    it('renders radar chart when 3+ enabled categories', async () => {
      render()

      await waitFor(() => {
        expect(screen.getByText('Interest Fingerprint')).toBeInTheDocument()
      })

      // SVG radar chart should be present (rendered as inline SVG)
      const svg = document.querySelector('svg')
      expect(svg).toBeInTheDocument()
    })

    it('shows all category names in the list', async () => {
      render()

      await waitFor(() => {
        expect(screen.getByText('Software Engineering')).toBeInTheDocument()
        expect(screen.getByText('Music')).toBeInTheDocument()
        expect(screen.getByText('Travel')).toBeInTheDocument()
        expect(screen.getByText('Cooking')).toBeInTheDocument()
        expect(screen.getByText('Reading')).toBeInTheDocument()
      })
    })

    it('shows category count and similarity', async () => {
      render()

      await waitFor(() => {
        expect(screen.getByText(/42 items/)).toBeInTheDocument()
        expect(screen.getByText(/65\.0% avg match/)).toBeInTheDocument()
      })
    })

    it('shows privacy note', async () => {
      render()

      await waitFor(() => {
        expect(screen.getByText('Your fingerprint is private by default')).toBeInTheDocument()
      })
    })
  })

  describe('category toggle', () => {
    it('calls toggleInterest when a category switch is clicked', async () => {
      discoveryClient.getInterests.mockResolvedValue({
        success: true,
        data: MOCK_PROFILE,
      })

      const updatedProfile = {
        ...MOCK_PROFILE,
        categories: MOCK_PROFILE.categories.map(c =>
          c.name === 'Software Engineering' ? { ...c, enabled: false } : c
        ),
      }
      discoveryClient.toggleInterest.mockResolvedValue({
        success: true,
        data: updatedProfile,
      })

      render()

      await waitFor(() => {
        expect(screen.getByText('Software Engineering')).toBeInTheDocument()
      })

      // Find the toggle switches - they're role="switch"
      const switches = screen.getAllByRole('switch')
      // Click the first one (Software Engineering)
      fireEvent.click(switches[0])

      await waitFor(() => {
        expect(discoveryClient.toggleInterest).toHaveBeenCalledWith(
          'Software Engineering',
          false // toggling from enabled to disabled
        )
      })
    })
  })

  describe('re-scan', () => {
    it('re-detects interests when Re-scan button is clicked', async () => {
      discoveryClient.getInterests.mockResolvedValue({
        success: true,
        data: MOCK_PROFILE,
      })
      discoveryClient.detectInterests.mockResolvedValue({
        success: true,
        data: MOCK_PROFILE,
      })

      const onResult = vi.fn()
      render(onResult)

      await waitFor(() => {
        expect(screen.getByText('Re-scan')).toBeInTheDocument()
      })

      fireEvent.click(screen.getByText('Re-scan'))

      await waitFor(() => {
        expect(discoveryClient.detectInterests).toHaveBeenCalled()
      })

      // After detection, loadProfile is called to re-fetch
      await waitFor(() => {
        expect(discoveryClient.getInterests).toHaveBeenCalledTimes(2)
      })
    })
  })

  describe('tag cloud fallback', () => {
    it('shows tag cloud when fewer than 3 enabled categories', async () => {
      const twoEnabledProfile = {
        ...MOCK_PROFILE,
        categories: [
          { name: 'Music', count: 28, avg_similarity: 0.58, enabled: true },
          { name: 'Travel', count: 15, avg_similarity: 0.51, enabled: true },
          { name: 'Cooking', count: 10, avg_similarity: 0.45, enabled: false },
        ],
      }

      discoveryClient.getInterests.mockResolvedValue({
        success: true,
        data: twoEnabledProfile,
      })

      render()

      await waitFor(() => {
        expect(screen.getByText('Interest Fingerprint')).toBeInTheDocument()
      })

      // With <3 enabled categories, no SVG radar chart - tag cloud instead
      // All categories should still appear as tags (in both tag cloud and category list)
      expect(screen.getAllByText(/Music/).length).toBeGreaterThanOrEqual(1)
      expect(screen.getAllByText(/Travel/).length).toBeGreaterThanOrEqual(1)
    })
  })
})
