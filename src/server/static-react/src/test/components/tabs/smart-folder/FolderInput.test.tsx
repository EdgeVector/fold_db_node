/**
 * @fileoverview Tests for FolderInput's "Try sample data" shortcut.
 *
 * Locks the contract that the button gates on a backend probe of the
 * `sample_data/` directory rather than the build-time `import.meta.env.DEV`
 * flag — that flag was hiding the shortcut for brew-installed users whose
 * release build runs from CWD=~ instead of the repo root.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor, fireEvent, act } from '@testing-library/react'
import FolderInput from '../../../../components/tabs/smart-folder/FolderInput'

const { getSampleDataAvailability } = vi.hoisted(() => ({
  getSampleDataAvailability: vi.fn(),
}))

vi.mock('../../../../api/clients/systemClient', () => ({
  systemClient: {
    getSampleDataAvailability,
  },
}))

// `useFolderAutocomplete` reaches out to the ingestion client, which would
// hit fetch in jsdom. The component only consumes the returned shape, so
// stub it directly rather than running the real hook.
function makeAutocompleteStub() {
  return {
    suggestions: [] as string[],
    selectedIndex: -1,
    showSuggestions: false,
    setShowSuggestions: vi.fn(),
    setSelectedIndex: vi.fn(),
    acceptSuggestion: vi.fn(),
    handleInputKeyDown: vi.fn(),
    inputRef: { current: null },
    suggestionsRef: { current: null },
  } as unknown as Parameters<typeof FolderInput>[0]['autocomplete']
}

function renderInput(onFolderPathChange = vi.fn()) {
  return {
    onFolderPathChange,
    ...render(
      <FolderInput
        folderPath=""
        onFolderPathChange={onFolderPathChange}
        onScan={vi.fn()}
        onCancelScan={vi.fn()}
        isScanning={false}
        scanProgress={null}
        autocomplete={makeAutocompleteStub()}
      />,
    ),
  }
}

beforeEach(() => {
  getSampleDataAvailability.mockReset()
})

describe('FolderInput "Try sample data" gate', () => {
  it('shows the button when the backend reports sample data is available, and uses the resolved path on click', async () => {
    const resolvedPath = '/opt/homebrew/Cellar/folddb/0.4.0/share/folddb/sample_data'
    getSampleDataAvailability.mockResolvedValue({
      data: { available: true, path: resolvedPath },
    })

    const { onFolderPathChange } = renderInput()

    const button = await screen.findByRole('button', { name: /try sample data/i })
    expect(button).toBeInTheDocument()

    fireEvent.click(button)
    // Click hands the absolute path back to the parent — NOT the literal
    // string `sample_data`, which only worked when CWD == repo root.
    expect(onFolderPathChange).toHaveBeenCalledWith(resolvedPath)
  })

  it('hides the button when the backend reports sample data is unavailable', async () => {
    getSampleDataAvailability.mockResolvedValue({ data: { available: false } })

    renderInput()

    // Wait for the probe to settle so we know the absence is decisive,
    // not just the pre-resolution null state.
    await waitFor(() => {
      expect(getSampleDataAvailability).toHaveBeenCalledTimes(1)
    })
    expect(screen.queryByRole('button', { name: /try sample data/i })).toBeNull()
  })

  it('hides the button when the probe rejects (e.g. transient backend error)', async () => {
    getSampleDataAvailability.mockRejectedValue(new Error('boom'))

    await act(async () => {
      renderInput()
    })

    await waitFor(() => {
      expect(getSampleDataAvailability).toHaveBeenCalledTimes(1)
    })
    expect(screen.queryByRole('button', { name: /try sample data/i })).toBeNull()
  })
})
