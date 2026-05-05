import React from 'react'
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react'
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { Provider } from 'react-redux'
import { combineReducers, configureStore } from '@reduxjs/toolkit'
import AppleImportTab from '../../../components/tabs/AppleImportTab'
import ingestionReducer, {
  appleJobCompleted,
  appleJobProgressed,
  appleJobStarted,
  type AppleSourceKey,
} from '../../../store/ingestionSlice'
import { appleJobsListener } from '../../../store/appleJobsMiddleware'

const ALL_KEYS: readonly AppleSourceKey[] = [
  'notes',
  'photos',
  'calendar',
  'reminders',
  'contacts',
]

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

  // Repro for the dogfood-2026-05-05 idle-reset bug:
  // After "Import All" is clicked while the previous run is `done`, the
  // POSTs to /apple-import/<source> can stay in flight for a long time
  // under backend load (HEIC conversion, 100+ notes ingest, contacts
  // permission timeout). The old `reset() → setTimeout(() → start())`
  // pattern moved every job to `idle` synchronously and only flipped to
  // `running` once the POST resolved. If the user observed the store
  // during that window — or if the POST never resolved — state stayed
  // visibly idle even though the user just kicked off five imports.
  //
  // Contract: between the click and the POST resolving, every job the
  // user just asked for must already be in `running`/`Starting...`.
  // Never idle, never with a stale "Done" left over.
  it('"Import All" never leaves jobs in idle while the start POSTs are in flight', async () => {
    mockGetAppleImportStatus.mockResolvedValue({
      success: true,
      data: { available: true },
    })
    // POSTs hang forever — simulates the under-load case where the
    // backend doesn't respond promptly. State must NOT regress to idle.
    mockAppleImportNotes.mockReturnValue(new Promise(() => {}))
    mockAppleImportReminders.mockReturnValue(new Promise(() => {}))
    mockAppleImportPhotos.mockReturnValue(new Promise(() => {}))
    mockAppleImportCalendar.mockReturnValue(new Promise(() => {}))
    mockAppleImportContacts.mockReturnValue(new Promise(() => {}))
    mockGetJobProgress.mockReturnValue(new Promise(() => {}))

    const store = buildStore()
    // Pre-load: every source already finished a prior run (so the
    // for-loop in handleImportAll matches `imp.status === 'done'`).
    for (const key of ALL_KEYS) {
      store.dispatch(appleJobStarted({ key, progressId: `${key}-prev` }))
      store.dispatch(
        appleJobCompleted({
          key,
          result: { total: 1, ingested: 1 },
          message: 'Done',
        }),
      )
    }

    render(
      <Provider store={store}>
        <AppleImportTab onResult={vi.fn()} />
      </Provider>,
    )

    await waitFor(() => {
      expect(screen.getByText('Import All (5)')).toBeTruthy()
    })

    await act(async () => {
      fireEvent.click(screen.getByText('Import All (5)'))
      // Drain any setTimeout(0) callbacks the click might have scheduled.
      vi.advanceTimersByTime(0)
      await Promise.resolve()
      vi.advanceTimersByTime(0)
    })

    const { appleJobs } = store.getState().ingestion
    for (const key of ALL_KEYS) {
      expect(appleJobs[key].status).toBe('running')
      expect(appleJobs[key].message).toBe('Starting...')
      expect(appleJobs[key].progress).toBe(5)
      expect(appleJobs[key].result).toBeNull()
    }
  })

  // Follow-up to the dogfood-2026-05-05 observation: after the imports
  // had actually completed in the backend, the Redux store was observed
  // with all five `appleJobs` entries reset to fresh `idle` state. PR
  // #892 closed the click-time idle window (the most likely cause), but
  // the post-completion contract — "once a job is `done`, no React
  // mount/unmount cycle and no listener-middleware activity drops it
  // back to `idle`" — needs its own pin. Without this assertion any
  // future `useEffect(() => () => imports[k].reset())` cleanup or
  // listener side-effect that quietly fires `appleJobReset` would
  // silently regress the persistence promise from PR #884.
  it('completed jobs survive AppleImportTab unmount/remount without regressing to idle', async () => {
    mockGetAppleImportStatus.mockResolvedValue({
      success: true,
      data: { available: true },
    })
    // No new polls should fire during this test — the store is pre-loaded
    // at `done`, and the listener middleware only arms on `appleJobStarted`.
    mockGetJobProgress.mockReturnValue(new Promise(() => {}))

    const store = buildStore()
    for (const key of ALL_KEYS) {
      store.dispatch(appleJobStarted({ key, progressId: `${key}-final` }))
      store.dispatch(
        appleJobCompleted({
          key,
          result: { total: 50, ingested: 47 },
          message: '✓ Imported 47 of 50',
        }),
      )
    }

    const first = render(
      <Provider store={store}>
        <AppleImportTab onResult={vi.fn()} />
      </Provider>,
    )
    await waitFor(() => {
      expect(screen.getByText('Import All (5)')).toBeTruthy()
    })

    // Simulate navigating to another tab — AppleImportTab unmounts.
    first.unmount()

    // Bump fake-timer time forward to simulate "minutes later" idle.
    // No action should have been dispatched on its own; the only mutation
    // surface is `appleJobReset` (button click) or middleware reactions
    // to `appleJobStarted`, neither of which happens here.
    vi.advanceTimersByTime(5 * 60_000)
    await Promise.resolve()

    // Sanity: state still reads `done` for every source while unmounted.
    {
      const { appleJobs } = store.getState().ingestion
      for (const key of ALL_KEYS) {
        expect(appleJobs[key].status).toBe('done')
        expect(appleJobs[key].progressId).toBe(`${key}-final`)
        expect(appleJobs[key].result).toEqual({ total: 50, ingested: 47 })
      }
    }

    // Navigate back — AppleImportTab remounts, reads from the same store.
    render(
      <Provider store={store}>
        <AppleImportTab onResult={vi.fn()} />
      </Provider>,
    )
    await waitFor(() => {
      expect(screen.getByText('Import All (5)')).toBeTruthy()
    })

    // Final assertion: the post-completion contract holds across the
    // unmount/remount cycle. No path resets a `done` job to `idle` on
    // its own.
    const { appleJobs } = store.getState().ingestion
    for (const key of ALL_KEYS) {
      expect(appleJobs[key].status).toBe('done')
      expect(appleJobs[key].progressId).toBe(`${key}-final`)
      expect(appleJobs[key].message).toBe('✓ Imported 47 of 50')
      expect(appleJobs[key].result).toEqual({ total: 50, ingested: 47 })
    }
  })

  // Variant of the above where completion arrives WHILE the component
  // is unmounted — the close-as-possible deterministic mirror of the
  // dogfood scenario (user navigates to Browse, imports finish in the
  // background via the listener middleware's poll loop, user navigates
  // back). The middleware lives at the store level and keeps polling
  // regardless of mount state, so the completion dispatch must land in
  // the store and persist into the remount.
  it('completion arriving while unmounted lands as `done` and persists into remount', async () => {
    mockGetAppleImportStatus.mockResolvedValue({
      success: true,
      data: { available: true },
    })
    // First poll tick returns is_complete=true so the listener fork
    // resolves cleanly into `appleJobCompleted` for every key.
    mockGetJobProgress.mockResolvedValue({
      success: true,
      data: {
        progress_percentage: 100,
        status_message: '✓ Imported 12 reminders',
        is_complete: true,
        results: { total: 12, ingested: 12 },
      },
    })

    const store = buildStore()
    const first = render(
      <Provider store={store}>
        <AppleImportTab onResult={vi.fn()} />
      </Provider>,
    )
    // Wait for initial mount/load before dispatching state.
    await waitFor(() => {
      expect(screen.getByText('Import All (5)')).toBeTruthy()
    })

    // Pre-load: every source is `running` with a real progressId, as it
    // would be a few seconds after Import All while the user navigates
    // away. The listener middleware is armed because we dispatched
    // appleJobStarted directly.
    act(() => {
      for (const key of ALL_KEYS) {
        store.dispatch(appleJobStarted({ key, progressId: `${key}-live` }))
      }
    })
    await waitFor(() => {
      expect(screen.getByText('Importing...')).toBeTruthy()
    })

    // Navigate away mid-flight.
    first.unmount()

    // Advance past one poll interval so the listener fork ticks while
    // unmounted. With is_complete=true on the first tick, every key
    // should land at `done` without anyone in the React tree mediating.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000)
    })

    {
      const { appleJobs } = store.getState().ingestion
      for (const key of ALL_KEYS) {
        expect(appleJobs[key].status).toBe('done')
        expect(appleJobs[key].result).toEqual({ total: 12, ingested: 12 })
      }
    }

    // Navigate back. The remount must observe `done`, never `idle`.
    render(
      <Provider store={store}>
        <AppleImportTab onResult={vi.fn()} />
      </Provider>,
    )
    await waitFor(() => {
      expect(screen.getByText('Import All (5)')).toBeTruthy()
    })

    const { appleJobs } = store.getState().ingestion
    for (const key of ALL_KEYS) {
      expect(appleJobs[key].status).toBe('done')
      expect(appleJobs[key].result).toEqual({ total: 12, ingested: 12 })
      expect(appleJobs[key].message).toBe('✓ Imported 12 reminders')
    }
  })
})
