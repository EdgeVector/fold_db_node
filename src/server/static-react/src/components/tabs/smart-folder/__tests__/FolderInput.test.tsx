import { render, screen, act } from '@testing-library/react'
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import FolderInput from '../FolderInput'

// Stub the directory browser modal — irrelevant here and pulls in fs APIs.
vi.mock('../DirectoryBrowserModal', () => ({
  default: () => null,
}))

const noopAutocomplete = {
  suggestions: [] as string[],
  selectedIndex: -1,
  showSuggestions: false,
  setShowSuggestions: vi.fn(),
  setSelectedIndex: vi.fn(),
  acceptSuggestion: vi.fn(),
  handleInputKeyDown: vi.fn(),
  inputRef: { current: null } as unknown,
  suggestionsRef: { current: null } as unknown,
} as unknown as Parameters<typeof FolderInput>[0]['autocomplete']

const baseProps = {
  folderPath: '~/Documents',
  onFolderPathChange: vi.fn(),
  onScan: vi.fn(),
  onCancelScan: vi.fn(),
  autocomplete: noopAutocomplete,
}

describe('FolderInput stale-scan affordance', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
    vi.clearAllMocks()
  })

  it('does not surface the stale banner before the timeout', () => {
    render(
      <FolderInput
        {...baseProps}
        isScanning={true}
        scanProgress={{ progress_percentage: 98, status_message: 'Finalizing... 5 to ingest, 0 skipped' }}
      />,
    )

    act(() => {
      vi.advanceTimersByTime(5000)
    })

    expect(screen.queryByText(/Lost track of scan/i)).toBeNull()
  })

  it('surfaces a "Lost track of scan" banner with cancel-and-retry when progress stays frozen for 15s', () => {
    render(
      <FolderInput
        {...baseProps}
        isScanning={true}
        scanProgress={{ progress_percentage: 98, status_message: 'Finalizing... 5 to ingest, 0 skipped' }}
      />,
    )

    act(() => {
      vi.advanceTimersByTime(16000)
    })

    expect(screen.getByText(/Lost track of scan/i)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Cancel and retry/i })).toBeInTheDocument()
  })

  it('clears the stale banner once isScanning flips back to false', () => {
    const { rerender } = render(
      <FolderInput
        {...baseProps}
        isScanning={true}
        scanProgress={{ progress_percentage: 98, status_message: 'Finalizing...' }}
      />,
    )
    act(() => {
      vi.advanceTimersByTime(16000)
    })
    expect(screen.getByText(/Lost track of scan/i)).toBeInTheDocument()

    rerender(
      <FolderInput
        {...baseProps}
        isScanning={false}
        scanProgress={null}
      />,
    )

    expect(screen.queryByText(/Lost track of scan/i)).toBeNull()
  })

  it('resets the stale timer when scan progress advances', () => {
    const { rerender } = render(
      <FolderInput
        {...baseProps}
        isScanning={true}
        scanProgress={{ progress_percentage: 50, status_message: 'Classifying...' }}
      />,
    )
    act(() => {
      vi.advanceTimersByTime(10000)
    })

    rerender(
      <FolderInput
        {...baseProps}
        isScanning={true}
        scanProgress={{ progress_percentage: 80, status_message: 'Classifying...' }}
      />,
    )
    act(() => {
      vi.advanceTimersByTime(10000)
    })

    // Cumulative elapsed time is 20s, but the progress change at 10s reset
    // the timer — only 10s have elapsed since the last update, so no banner.
    expect(screen.queryByText(/Lost track of scan/i)).toBeNull()
  })
})
