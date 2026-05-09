import * as React from 'react'
import { useEffect, useState } from 'react'
import DirectoryBrowserModal from './DirectoryBrowserModal'
import type { useFolderAutocomplete } from '../../../hooks/useFolderAutocomplete'

const STALE_SCAN_MS = 15000

/**
 * Tauri capabilities required in src-tauri/capabilities/default.json:
 *   - dialog:allow-open   (Browse button: open({ directory: true }))
 */

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown
  }
}

const isTauri = typeof window !== 'undefined' && Boolean(window.__TAURI_INTERNALS__)

interface ScanProgress {
  progress_percentage?: number
  status_message?: string
}

interface FolderInputProps {
  folderPath: string
  onFolderPathChange: (path: string) => void
  onScan: () => void
  onCancelScan: () => void
  isScanning: boolean
  scanProgress: ScanProgress | null
  autocomplete: ReturnType<typeof useFolderAutocomplete>
}

/**
 * Folder path input with autocomplete, scan/cancel button, and progress display.
 */
export default function FolderInput({
  folderPath,
  onFolderPathChange,
  onScan,
  onCancelScan,
  isScanning,
  scanProgress,
  autocomplete,
}: FolderInputProps) {
  const {
    suggestions,
    selectedIndex,
    showSuggestions,
    setShowSuggestions,
    setSelectedIndex,
    acceptSuggestion,
    handleInputKeyDown,
    inputRef,
    suggestionsRef,
  } = autocomplete

  const [, setPickerError] = useState<unknown>(null)
  const [showBrowser, setShowBrowser] = useState(false)
  const [staleScan, setStaleScan] = useState(false)

  // Defense-in-depth: if the scan stays in the same state for too long
  // (no progress update OR no progress object at all), surface a recovery
  // affordance so the user isn't left staring at a frozen spinner. Resets
  // whenever the progress payload changes identity or scanning ends.
  const progressKey = scanProgress
    ? `${scanProgress.progress_percentage ?? ''}|${scanProgress.status_message ?? ''}`
    : ''
  useEffect(() => {
    if (!isScanning) {
      setStaleScan(false)
      return
    }
    setStaleScan(false)
    const timer = setTimeout(() => setStaleScan(true), STALE_SCAN_MS)
    return () => clearTimeout(timer)
  }, [isScanning, progressKey])

  const openFolderPicker = async () => {
    if (isTauri) {
      try {
        const { open } = await import('@tauri-apps/plugin-dialog')
        const selected = await open({ directory: true, multiple: false, title: 'Select folder to scan' })
        if (selected && typeof selected === 'string') onFolderPathChange(selected)
      } catch (error) {
        setPickerError(error)
        console.error('Failed to open folder picker:', error)
      }
    } else {
      setShowBrowser(true)
    }
  }

  const handleBrowserSelect = (path: string) => {
    onFolderPathChange(path)
    setShowBrowser(false)
  }

  return (
    <>
      <div className="flex gap-3">
        <div className="relative flex-1">
          <input
            ref={inputRef}
            type="text"
            value={folderPath}
            onChange={(e) => onFolderPathChange(e.target.value)}
            onKeyDown={handleInputKeyDown}
            onFocus={() => { if (suggestions.length > 0) setShowSuggestions(true) }}
            placeholder="/path/to/your/folder"
            className="input w-full"
            disabled={isScanning}
            autoComplete="off"
          />
          {!isScanning && showSuggestions && suggestions.length > 0 && (
            <ul
              ref={suggestionsRef as unknown as React.RefObject<HTMLUListElement>}
              className="absolute z-50 left-0 right-0 top-full mt-1 border border-border rounded-lg bg-surface shadow-lg max-h-48 overflow-y-auto"
            >
              {suggestions.map((path: string, i: number) => (
                <li
                  key={path}
                  className={`px-3 py-1.5 cursor-pointer text-sm font-mono truncate ${
                    i === selectedIndex ? 'bg-accent text-surface' : 'hover:bg-gruvbox-hover'
                  }`}
                  onMouseDown={(e) => { e.preventDefault(); acceptSuggestion(path) }}
                  onMouseEnter={() => setSelectedIndex(i)}
                >
                  {path}
                </li>
              ))}
            </ul>
          )}
        </div>
        <button onClick={openFolderPicker} disabled={isScanning} className="btn-secondary" title="Browse">Browse</button>
        {isScanning
          ? <button onClick={onCancelScan} className="btn-secondary">Cancel</button>
          : <button onClick={() => onScan()} disabled={!folderPath.trim()} className="btn-primary">Scan</button>
        }
      </div>
      {isScanning && scanProgress && (
        <div className="space-y-1.5">
          <div className="w-full bg-border rounded-full h-1.5 overflow-hidden">
            <div
              className="h-full bg-primary rounded-full transition-all duration-300"
              style={{ width: `${scanProgress.progress_percentage || 0}%` }}
            />
          </div>
          <p className="text-xs text-secondary">{scanProgress.status_message || 'Starting scan...'}</p>
        </div>
      )}
      {isScanning && !scanProgress && (
        <p className="text-xs text-secondary">Starting scan...</p>
      )}
      {isScanning && staleScan && (
        <div className="rounded-md border border-warning/40 bg-warning/10 px-3 py-2 text-xs text-secondary flex items-center justify-between gap-3">
          <span>
            Lost track of scan{scanProgress?.status_message ? ` (${scanProgress.status_message})` : ''}. The job may have stalled.
          </span>
          <button
            onClick={onCancelScan}
            className="btn-secondary text-xs px-2 py-0.5"
          >
            Cancel and retry
          </button>
        </div>
      )}
      {import.meta.env.DEV && (
        <button
          onClick={() => onFolderPathChange('sample_data')}
          className="text-xs text-secondary hover:text-primary underline"
          disabled={isScanning}
        >
          Try sample data
        </button>
      )}
      {showBrowser && (
        <DirectoryBrowserModal
          initialPath={folderPath.includes('/') ? folderPath : '/'}
          onSelect={handleBrowserSelect}
          onClose={() => setShowBrowser(false)}
        />
      )}
    </>
  )
}
