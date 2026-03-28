import React from 'react'
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react'
import { describe, it, expect, beforeEach, vi } from 'vitest'
import AppleImportTab from '../../../components/tabs/AppleImportTab'

const mockGetAppleImportStatus = vi.fn()
const mockAppleImportNotes = vi.fn()
const mockAppleImportReminders = vi.fn()
const mockAppleImportPhotos = vi.fn()
const mockAppleImportCalendar = vi.fn()
const mockGetJobProgress = vi.fn()

vi.mock('../../../api/clients/ingestionClient', () => ({
  default: {
    getAppleImportStatus: (...args) => mockGetAppleImportStatus(...args),
    appleImportNotes: (...args) => mockAppleImportNotes(...args),
    appleImportReminders: (...args) => mockAppleImportReminders(...args),
    appleImportPhotos: (...args) => mockAppleImportPhotos(...args),
    appleImportCalendar: (...args) => mockAppleImportCalendar(...args),
    getJobProgress: (...args) => mockGetJobProgress(...args),
  },
}))

describe('AppleImportTab', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('shows loading state initially', () => {
    mockGetAppleImportStatus.mockReturnValue(new Promise(() => {})) // never resolves
    render(<AppleImportTab onResult={vi.fn()} />)
    expect(screen.getByText('Checking Apple import availability...')).toBeTruthy()
  })

  it('shows unavailable message when not on macOS', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: false } })
    render(<AppleImportTab onResult={vi.fn()} />)
    await waitFor(() => {
      expect(screen.getByText('Apple Import is only available on macOS.')).toBeTruthy()
    })
  })

  it('renders all four source cards when available', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    render(<AppleImportTab onResult={vi.fn()} />)
    await waitFor(() => {
      expect(screen.getByText('Notes')).toBeTruthy()
      expect(screen.getByText('Photos')).toBeTruthy()
      expect(screen.getByText('Calendar')).toBeTruthy()
      expect(screen.getByText('Reminders')).toBeTruthy()
    })
  })

  it('shows "coming soon" badge on Calendar', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    render(<AppleImportTab onResult={vi.fn()} />)
    await waitFor(() => {
      expect(screen.getByText('coming soon')).toBeTruthy()
    })
  })

  it('shows Import All button with count of enabled sources', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    render(<AppleImportTab onResult={vi.fn()} />)
    await waitFor(() => {
      // Notes, Photos, Reminders enabled by default (Calendar is comingSoon so excluded)
      expect(screen.getByText('Import All (3)')).toBeTruthy()
    })
  })

  it('updates Import All count when toggling a source off', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    render(<AppleImportTab onResult={vi.fn()} />)

    await waitFor(() => {
      expect(screen.getByText('Import All (3)')).toBeTruthy()
    })

    // Toggle off one source (find the toggle switches - they are role="switch")
    const toggles = screen.getAllByRole('switch')
    // toggles[0] = Notes, toggles[1] = Photos, toggles[2] = Calendar (disabled), toggles[3] = Reminders
    fireEvent.click(toggles[0]) // Toggle off Notes

    expect(screen.getByText('Import All (2)')).toBeTruthy()
  })

  it('triggers parallel imports when Import All is clicked', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    mockAppleImportNotes.mockResolvedValue({ success: true, data: { progress_id: 'notes-1' } })
    mockAppleImportReminders.mockResolvedValue({ success: true, data: { progress_id: 'rem-1' } })
    mockAppleImportPhotos.mockResolvedValue({ success: true, data: { progress_id: 'photos-1' } })
    mockGetJobProgress.mockResolvedValue({ success: true, data: { progress_percentage: 50, status_message: 'Processing...' } })

    render(<AppleImportTab onResult={vi.fn()} />)

    await waitFor(() => {
      expect(screen.getByText('Import All (3)')).toBeTruthy()
    })

    await act(async () => {
      fireEvent.click(screen.getByText('Import All (3)'))
      // Let setTimeout(0) callbacks fire
      vi.advanceTimersByTime(0)
    })

    await waitFor(() => {
      expect(mockAppleImportNotes).toHaveBeenCalled()
      expect(mockAppleImportReminders).toHaveBeenCalled()
      expect(mockAppleImportPhotos).toHaveBeenCalled()
      // Calendar should not be called (coming soon)
      expect(mockAppleImportCalendar).not.toHaveBeenCalled()
    })
  })

  it('shows error state per source', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    mockAppleImportNotes.mockResolvedValue({ success: false, error: { message: 'Notes access denied' } })
    mockAppleImportReminders.mockResolvedValue({ success: true, data: { progress_id: 'rem-1' } })
    mockAppleImportPhotos.mockResolvedValue({ success: true, data: { progress_id: 'photos-1' } })
    mockGetJobProgress.mockResolvedValue({ success: true, data: { progress_percentage: 50, status_message: 'Working...' } })

    render(<AppleImportTab onResult={vi.fn()} />)

    await waitFor(() => {
      expect(screen.getByText('Import All (3)')).toBeTruthy()
    })

    await act(async () => {
      fireEvent.click(screen.getByText('Import All (3)'))
      vi.advanceTimersByTime(0)
    })

    await waitFor(() => {
      expect(screen.getByText('Notes access denied')).toBeTruthy()
    })
  })

  it('shows photos limit input only for photos source', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    render(<AppleImportTab onResult={vi.fn()} />)

    await waitFor(() => {
      expect(screen.getByText('Limit:')).toBeTruthy()
      const limitInput = screen.getByDisplayValue('50')
      expect(limitInput).toBeTruthy()
    })
  })

  it('passes custom photos limit to import call', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    mockAppleImportNotes.mockResolvedValue({ success: true, data: { progress_id: 'n-1' } })
    mockAppleImportReminders.mockResolvedValue({ success: true, data: { progress_id: 'r-1' } })
    mockAppleImportPhotos.mockResolvedValue({ success: true, data: { progress_id: 'p-1' } })
    mockGetJobProgress.mockResolvedValue({ success: true, data: { progress_percentage: 10, status_message: 'Working...' } })

    render(<AppleImportTab onResult={vi.fn()} />)

    await waitFor(() => {
      expect(screen.getByDisplayValue('50')).toBeTruthy()
    })

    // Change limit
    fireEvent.change(screen.getByDisplayValue('50'), { target: { value: '100' } })

    await act(async () => {
      fireEvent.click(screen.getByText('Import All (3)'))
      vi.advanceTimersByTime(0)
    })

    await waitFor(() => {
      expect(mockAppleImportPhotos).toHaveBeenCalledWith(null, 100)
    })
  })
})
