import { useState, useEffect, useRef, useCallback } from 'react'
import type { OperationResultPayload } from '../../types/api'
import { toErrorMessage } from '../../utils/schemaUtils'
import { ingestionClient } from '../../api/clients'
import type { SmartFolderScanResponse } from '../../api/clients/ingestionClient'
import { defaultApiClient } from '../../api/core/client'
import { useFolderAutocomplete } from '../../hooks/useFolderAutocomplete'
import { useScanPolling } from '../../hooks/useScanPolling'
import { useBatchMonitor } from '../../hooks/useBatchMonitor'
import { useFilesProgress } from '../../hooks/useFilesProgress'
import FolderInput from './smart-folder/FolderInput'
import ScanResultsView from './smart-folder/ScanResultsView'
import BatchProgressView from './smart-folder/BatchProgressView'

const STORAGE_KEY = 'smartFolderTabState'

interface PersistedState {
  folderPath?: string
  scanProgressId?: string | null
  scanResult?: SmartFolderScanResponse | null
  batchId?: string | null
  spendLimit?: string
  fileProgressIds?: Array<{ file_name: string; progress_id: string }>
  includeAlreadyIngested?: boolean
}

interface Org {
  org_hash: string
  org_name: string
}

type OnResult = (result: OperationResultPayload | null) => void

/** Load persisted SmartFolderTab state from localStorage.
 *  Only restores state when there's an active batch or scan to reconnect to.
 *  Completed/stale sessions start fresh. */
function loadPersistedState(): PersistedState | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return null
    const state = JSON.parse(raw) as PersistedState
    const hasActiveBatch = state.batchId && localStorage.getItem('activeBatchId')
    const hasActiveScan = !!state.scanProgressId
    if (!hasActiveBatch && !hasActiveScan) {
      localStorage.removeItem(STORAGE_KEY)
      return null
    }
    return state
  } catch { return null }
}

function persistState(state: PersistedState) {
  try { localStorage.setItem(STORAGE_KEY, JSON.stringify(state)) } catch { /* best-effort */ }
}

function clearPersistedState() {
  localStorage.removeItem(STORAGE_KEY)
}

interface SmartFolderTabProps {
  onResult: OnResult
}

function SmartFolderTab({ onResult: onResultProp }: SmartFolderTabProps) {
  // Stabilize the onResult callback so polling effects don't restart on every parent render
  const onResultRef = useRef<OnResult>(onResultProp)
  useEffect(() => { onResultRef.current = onResultProp })
  const onResult = useCallback<OnResult>((...args) => onResultRef.current(...args), [])

  // Restore persisted state on mount
  const [restored] = useState(() => loadPersistedState())

  const [folderPath, setFolderPath] = useState<string>(() => restored?.folderPath || '~/Documents')
  const [isScanning, setIsScanning] = useState(() => !!restored?.scanProgressId)
  const [isLoadingMore, setIsLoadingMore] = useState(false)
  const [isIngesting, setIsIngesting] = useState(false)
  const [scanResult, setScanResult] = useState<SmartFolderScanResponse | null>(() => restored?.scanResult || null)
  const [batchId, setBatchId] = useState<string | null>(() => restored?.batchId || null)
  const [spendLimit, setSpendLimit] = useState<string>(() => restored?.spendLimit || '')
  const [fileProgressIds, setFileProgressIds] = useState<Array<{ file_name: string; progress_id: string }>>(() => restored?.fileProgressIds || [])
  const [scanProgressId, setScanProgressId] = useState<string | null>(() => restored?.scanProgressId || null)
  const [includeAlreadyIngested, setIncludeAlreadyIngested] = useState(() => !!restored?.includeAlreadyIngested)

  // Org selector state
  const [orgs, setOrgs] = useState<Org[]>([])
  const [selectedOrg, setSelectedOrg] = useState('')
  useEffect(() => {
    let cancelled = false
    let retries = 0
    const maxRetries = 5
    const fetchOrgs = () => {
      if (cancelled) return
      const hash = localStorage.getItem('fold_user_hash')
      if (!hash) {
        retries++
        if (retries < maxRetries) setTimeout(fetchOrgs, 1000)
        return
      }
      defaultApiClient.get('/org').then(res => {
        if (cancelled) return
        const data = ((res as { data?: unknown }).data ?? res) as { orgs?: Org[] }
        setOrgs(data.orgs || [])
      }).catch(() => {})
    }
    fetchOrgs()
    return () => { cancelled = true }
  }, [])

  // Ref for batchStatus so handleBack can read it without a stale closure
  const batchStatusRef = useRef<{ status?: string } | null>(null)

  // Persist key state whenever it changes
  useEffect(() => {
    if (!scanProgressId && !scanResult && !batchId) {
      clearPersistedState()
      return
    }
    persistState({ folderPath, scanProgressId, scanResult, batchId, spendLimit, includeAlreadyIngested, fileProgressIds })
  }, [folderPath, scanProgressId, scanResult, batchId, spendLimit, includeAlreadyIngested, fileProgressIds])

  // --- Handlers ---

  const startScan = useCallback(async (maxFiles?: number) => {
    if (!folderPath.trim()) return
    setIsScanning(true)
    setScanResult(null)
    onResult(null)
    try {
      const response = await ingestionClient.smartFolderScan(folderPath.trim(), 10, maxFiles)
      const respData = response.data as { progress_id?: string } | undefined
      if (response.success && respData?.progress_id) {
        setScanProgressId(respData.progress_id)
      } else {
        onResult({ success: false, error: 'Failed to start scan' })
        setIsScanning(false)
      }
    } catch (error) {
      onResult({ success: false, error: toErrorMessage(error) || 'Failed to scan folder' })
      setIsScanning(false)
    }
  }, [folderPath, onResult])

  const handleScan = useCallback(async (maxFiles?: number) => {
    setBatchId(null)
    setIncludeAlreadyIngested(false)
    await startScan(maxFiles)
  }, [startScan])

  const handleScanComplete = useCallback((result: SmartFolderScanResponse) => {
    setScanResult(result)
    // Default the spend limit to 5× the estimated cost (with a $1 floor) so
    // small-batch ingests don't immediately trip on rounding/variance. The
    // previous "default to exactly the estimate" behavior meant every single
    // ingest hit "spend limit reached" before any file finished — operators
    // had to manually bump the field on every Proceed click. Users can still
    // tighten it down if they want.
    const estimated = result.total_estimated_cost || 0
    const safeDefault = Math.max(estimated * 5, 1).toFixed(2)
    setSpendLimit(estimated > 0 ? safeDefault : '')
    setScanProgressId(null)
    setIsScanning(false)
  }, [])

  const handleScanFail = useCallback((msg: string) => {
    onResult({ success: false, error: msg })
    setScanProgressId(null)
    setIsScanning(false)
  }, [onResult])

  const handleBatchLost = useCallback(() => {
    setBatchId(null)
    clearPersistedState()
  }, [])

  const handleBatchTerminal = useCallback(() => {
    clearPersistedState()
  }, [])

  const handleCancelScan = useCallback(() => {
    setScanProgressId(null)
    setIsScanning(false)
    clearPersistedState()
  }, [])

  const handleLoadMore = useCallback(async () => {
    if (!scanResult) return
    const nextLimit = (scanResult.max_files_used || 100) + 400
    setIsLoadingMore(true)
    await startScan(nextLimit)
    setIsLoadingMore(false)
  }, [scanResult, startScan])

  const handleIngest = useCallback(async () => {
    if (!scanResult) return
    const files = includeAlreadyIngested
      ? [...scanResult.recommended_files, ...scanResult.skipped_files.filter(f => (f as { already_ingested?: boolean }).already_ingested)]
      : scanResult.recommended_files
    const filePaths = files.map(f => f.path)
    const fileCosts = files.map(f => f.estimated_cost)
    if (filePaths.length === 0) { onResult({ success: false, error: 'No files recommended' }); return }
    setIsIngesting(true)
    onResult(null)
    try {
      const limit = spendLimit ? parseFloat(spendLimit) : undefined
      const response = await ingestionClient.smartFolderIngest(
        folderPath.trim(), filePaths, true, limit, fileCosts, includeAlreadyIngested, selectedOrg || undefined
      )
      if (response.success && response.data) {
        const ingestData = response.data
        setBatchId(ingestData.batch_id)
        setFileProgressIds(ingestData.file_progress_ids || [])
        onResult({ success: true, data: { message: ingestData.message, batch_id: ingestData.batch_id, files_found: ingestData.files_found } })
      } else {
        onResult({ success: false, error: 'Failed to start ingestion' })
      }
    } catch (error) {
      onResult({ success: false, error: toErrorMessage(error) || 'Failed to start ingestion' })
    } finally {
      setIsIngesting(false)
    }
  }, [scanResult, includeAlreadyIngested, spendLimit, folderPath, selectedOrg, onResult])

  const handleResume = useCallback(async (limit?: number) => {
    if (!batchId || limit === undefined) return
    try {
      await ingestionClient.resumeBatch(batchId, limit)
    } catch (error) {
      onResult({ success: false, error: toErrorMessage(error) || 'Failed to resume' })
    }
  }, [batchId, onResult])

  const handleCancel = useCallback(async () => {
    if (!batchId) return
    try {
      await ingestionClient.cancelBatch(batchId)
      localStorage.removeItem('activeBatchId')
      localStorage.removeItem('activeBatchStatus')
    } catch (error) {
      onResult({ success: false, error: toErrorMessage(error) || 'Failed to cancel' })
    }
  }, [batchId, onResult])

  const handleBack = useCallback(async () => {
    const status = batchStatusRef.current
    if (batchId && (!status || status.status === 'Running' || status.status === 'Paused')) {
      try { await ingestionClient.cancelBatch(batchId) } catch { /* best-effort */ }
    }
    setScanResult(null)
    setScanProgressId(null)
    setBatchId(null)
    setSpendLimit('')
    setFileProgressIds([])
    clearPersistedState()
    localStorage.removeItem('activeBatchId')
    localStorage.removeItem('activeBatchStatus')
    onResult(null)
  }, [batchId, onResult])

  // --- Hooks ---

  const autocomplete = useFolderAutocomplete({
    folderPath,
    isDisabled: isScanning,
    onFolderPathChange: setFolderPath,
    onSubmit: () => handleScan(),
  })

  const { scanProgress } = useScanPolling({
    scanProgressId,
    onComplete: handleScanComplete,
    onFail: handleScanFail,
  })

  const { batchStatus, batchReport, setBatchReport } = useBatchMonitor({
    batchId,
    fileProgressIds,
    onBatchLost: handleBatchLost,
    onTerminal: handleBatchTerminal,
  })
  batchStatusRef.current = batchStatus as { status?: string } | null

  const { filesProgress, allFilesDone, hasFirstPoll } = useFilesProgress({
    enabled: batchId,
    fileProgressIds,
  })

  const selectedOrgName = orgs.find(o => o.org_hash === selectedOrg)?.org_name

  // --- Render ---

  return (
    <div className="space-y-4">
      {orgs.length > 0 && !batchId && (
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <select
              value={selectedOrg}
              onChange={(e) => setSelectedOrg(e.target.value)}
              className="input text-sm py-1 px-2"
            >
              <option value="">Personal</option>
              {orgs.map(org => (
                <option key={org.org_hash} value={org.org_hash}>
                  {org.org_name}
                </option>
              ))}
            </select>
            {selectedOrg && (
              <span className="text-xs text-secondary bg-primary/10 text-primary px-2 py-1 rounded">
                Ingesting into: {selectedOrgName}
              </span>
            )}
          </div>
        </div>
      )}

      {!scanResult && !batchId && (
        <FolderInput
          folderPath={folderPath}
          onFolderPathChange={setFolderPath}
          onScan={handleScan}
          onCancelScan={handleCancelScan}
          isScanning={isScanning}
          scanProgress={scanProgress}
          autocomplete={autocomplete}
        />
      )}

      {scanResult && !batchId && (
        <ScanResultsView
          scanResult={scanResult}
          onScanResultUpdate={(updated: SmartFolderScanResponse) => {
            setScanResult(updated)
            setSpendLimit(updated.total_estimated_cost?.toFixed(2) || '')
          }}
          spendLimit={spendLimit}
          onSpendLimitChange={setSpendLimit}
          includeAlreadyIngested={includeAlreadyIngested}
          onIncludeChange={setIncludeAlreadyIngested}
          onIngest={handleIngest}
          onLoadMore={handleLoadMore}
          onBack={handleBack}
          isIngesting={isIngesting}
          isLoadingMore={isLoadingMore}
        />
      )}

      {batchId && (
        <BatchProgressView
          batchStatus={batchStatus}
          batchReport={batchReport}
          filesProgress={filesProgress}
          allFilesDone={allFilesDone}
          hasFirstPoll={hasFirstPoll}
          onResume={handleResume}
          onCancel={handleCancel}
          onBack={handleBack}
          onDismissReport={() => setBatchReport(null)}
          isIngesting={isIngesting}
        />
      )}
    </div>
  )
}

export default SmartFolderTab
