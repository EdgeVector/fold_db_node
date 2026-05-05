import React from 'react'
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react'
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { Provider } from 'react-redux'
import { combineReducers, configureStore } from '@reduxjs/toolkit'
import AppleImportTab from '../../../components/tabs/AppleImportTab'
import ingestionReducer, {
  appleJobProgressed,
  appleJobStarted,
} from '../../../store/ingestionSlice'
import { appleJobsListener } from '../../../store/appleJobsMiddleware'

const mockGetAppleImportStatus = vi.fn()
const mockAppleImportNotes = vi.fn()
const mockAppleImportReminders = vi.fn()
const mockAppleImportPhotos = vi.fn()
const mockAppleImportCalendar = vi.fn()
const mockAppleImportContacts = vi.fn()
const mockGetJobProgress = vi.fn()
const mockGetAppleSyncConfig = vi.fn()

vi.mock('../../../api/clients/ingestionClient', () => ({
  default: {
    getAppleImportStatus: (...args: unknown[]) => mockGetAppleImportStatus(...args),
    appleImportNotes: (...args: unknown[]) => mockAppleImportNotes(...args),
    appleImportReminders: (...args: unknown[]) => mockAppleImportReminders(...args),
    appleImportPhotos: (...args: unknown[]) => mockAppleImportPhotos(...args),
    appleImportCalendar: (...args: unknown[]) => mockAppleImportCalendar(...args),
    appleImportContacts: (...args: unknown[]) => mockAppleImportContacts(...args),
    getJobProgress: (...args: unknown[]) => mockGetJobProgress(...args),
    getAppleSyncConfig: (...args: unknown[]) => mockGetAppleSyncConfig(...args),
  },
}))

// The middleware imports from `../../api/clients`, so mock that path too —
// otherwise it would fall back to the real network client during tests.
vi.mock('../../../api/clients', () => ({
  ingestionClient: {
    getJobProgress: (...args: unknown[]) => mockGetJobProgress(...args),
  },
}))

function buildStore() {
  return configureStore({
    reducer: combineReducers({ ingestion: ingestionReducer }),
    middleware: (getDefault) =>
      getDefault({ serializableCheck: false }).prepend(
        appleJobsListener.middleware,
      ),
  })
}

function renderWithStore(ui: React.ReactElement) {
  const store = buildStore()
  return {
    store,
    ...render(<Provider store={store}>{ui}</Provider>),
  }
}

describe('AppleImportTab', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.useFakeTimers()
    // AutoSyncSettings calls this on mount — return null config so it renders nothing
    mockGetAppleSyncConfig.mockResolvedValue({ success: true, data: null })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('shows loading state initially', () => {
    mockGetAppleImportStatus.mockReturnValue(new Promise(() => {})) // never resolves
    renderWithStore(<AppleImportTab onResult={vi.fn()} />)
    expect(screen.getByText('Checking Apple import availability...')).toBeTruthy()
  })

  it('shows unavailable message when not on macOS', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: false } })
    renderWithStore(<AppleImportTab onResult={vi.fn()} />)
    await waitFor(() => {
      expect(screen.getByText('Apple Import is only available on macOS.')).toBeTruthy()
    })
  })

  it('renders all five source cards when available', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    renderWithStore(<AppleImportTab onResult={vi.fn()} />)
    await waitFor(() => {
      expect(screen.getByText('Notes')).toBeTruthy()
      expect(screen.getByText('Photos')).toBeTruthy()
      expect(screen.getByText('Calendar')).toBeTruthy()
      expect(screen.getByText('Reminders')).toBeTruthy()
      expect(screen.getByText('Contacts')).toBeTruthy()
    })
  })

  it('shows Import All button with count of enabled sources', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    renderWithStore(<AppleImportTab onResult={vi.fn()} />)
    await waitFor(() => {
      // All five sources enabled by default
      expect(screen.getByText('Import All (5)')).toBeTruthy()
    })
  })

  it('updates Import All count when toggling a source off', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    renderWithStore(<AppleImportTab onResult={vi.fn()} />)

    await waitFor(() => {
      expect(screen.getByText('Import All (5)')).toBeTruthy()
    })

    // Toggle off one source (find the toggle switches - they are role="switch")
    const toggles = screen.getAllByRole('switch')
    // toggles[0] = Notes, toggles[1] = Photos, toggles[2] = Calendar,
    // toggles[3] = Reminders, toggles[4] = Contacts (SOURCES order)
    fireEvent.click(toggles[0]) // Toggle off Notes

    expect(screen.getByText('Import All (4)')).toBeTruthy()
  })

  it('triggers parallel imports when Import All is clicked', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    mockAppleImportNotes.mockResolvedValue({ success: true, data: { progress_id: 'notes-1' } })
    mockAppleImportReminders.mockResolvedValue({ success: true, data: { progress_id: 'rem-1' } })
    mockAppleImportPhotos.mockResolvedValue({ success: true, data: { progress_id: 'photos-1' } })
    mockAppleImportCalendar.mockResolvedValue({ success: true, data: { progress_id: 'cal-1' } })
    mockAppleImportContacts.mockResolvedValue({ success: true, data: { progress_id: 'con-1' } })
    mockGetJobProgress.mockResolvedValue({ success: true, data: { progress_percentage: 50, status_message: 'Processing...' } })

    renderWithStore(<AppleImportTab onResult={vi.fn()} />)

    await waitFor(() => {
      expect(screen.getByText('Import All (5)')).toBeTruthy()
    })

    await act(async () => {
      fireEvent.click(screen.getByText('Import All (5)'))
      // Let setTimeout(0) callbacks fire
      vi.advanceTimersByTime(0)
    })

    await waitFor(() => {
      expect(mockAppleImportNotes).toHaveBeenCalled()
      expect(mockAppleImportReminders).toHaveBeenCalled()
      expect(mockAppleImportPhotos).toHaveBeenCalled()
      expect(mockAppleImportCalendar).toHaveBeenCalled()
      expect(mockAppleImportContacts).toHaveBeenCalled()
    })
  })

  it('shows error state per source', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    mockAppleImportNotes.mockResolvedValue({ success: false, error: { message: 'Notes access denied' } })
    mockAppleImportReminders.mockResolvedValue({ success: true, data: { progress_id: 'rem-1' } })
    mockAppleImportPhotos.mockResolvedValue({ success: true, data: { progress_id: 'photos-1' } })
    mockAppleImportCalendar.mockResolvedValue({ success: true, data: { progress_id: 'cal-1' } })
    mockAppleImportContacts.mockResolvedValue({ success: true, data: { progress_id: 'con-1' } })
    mockGetJobProgress.mockResolvedValue({ success: true, data: { progress_percentage: 50, status_message: 'Working...' } })

    renderWithStore(<AppleImportTab onResult={vi.fn()} />)

    await waitFor(() => {
      expect(screen.getByText('Import All (5)')).toBeTruthy()
    })

    await act(async () => {
      fireEvent.click(screen.getByText('Import All (5)'))
      vi.advanceTimersByTime(0)
    })

    await waitFor(() => {
      expect(screen.getByText('Notes access denied')).toBeTruthy()
    })
  })

  it('shows photos limit input only for photos source', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    renderWithStore(<AppleImportTab onResult={vi.fn()} />)

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
    mockAppleImportCalendar.mockResolvedValue({ success: true, data: { progress_id: 'c-1' } })
    mockAppleImportContacts.mockResolvedValue({ success: true, data: { progress_id: 'con-1' } })
    mockGetJobProgress.mockResolvedValue({ success: true, data: { progress_percentage: 10, status_message: 'Working...' } })

    renderWithStore(<AppleImportTab onResult={vi.fn()} />)

    await waitFor(() => {
      expect(screen.getByDisplayValue('50')).toBeTruthy()
    })

    // Change limit
    fireEvent.change(screen.getByDisplayValue('50'), { target: { value: '100' } })

    await act(async () => {
      fireEvent.click(screen.getByText('Import All (5)'))
      vi.advanceTimersByTime(0)
    })

    await waitFor(() => {
      expect(mockAppleImportPhotos).toHaveBeenCalledWith(null, 100)
    })
  })

  it('reflects external store updates and survives unmount/remount with state intact', async () => {
    mockGetAppleImportStatus.mockResolvedValue({ success: true, data: { available: true } })
    // Stub progress polling so the middleware never updates state from the network.
    mockGetJobProgress.mockReturnValue(new Promise(() => {}))

    const store = buildStore()
    const first = render(
      <Provider store={store}>
        <AppleImportTab onResult={vi.fn()} />
      </Provider>,
    )

    await waitFor(() => {
      expect(screen.getByText('Import All (5)')).toBeTruthy()
    })

    // Simulate a job that started in another session of the same store.
    act(() => {
      store.dispatch(appleJobStarted({ key: 'notes', progressId: 'job-9' }))
      store.dispatch(
        appleJobProgressed({ key: 'notes', progress: 47, message: 'Halfway through Notes' }),
      )
    })

    await waitFor(() => {
      expect(screen.getByText('Halfway through Notes')).toBeTruthy()
    })

    // Unmount (e.g. user navigated to another tab) — Redux state must persist.
    first.unmount()

    // Remount (user navigates back) into a fresh container. Live progress
    // should still be visible because state lives in the store, not the tree.
    expect(store.getState().ingestion.appleJobs.notes.message).toBe(
      'Halfway through Notes',
    )

    render(
      <Provider store={store}>
        <AppleImportTab onResult={vi.fn()} />
      </Provider>,
    )

    await waitFor(() => {
      expect(screen.getByText('Halfway through Notes')).toBeTruthy()
    })
  })
})
