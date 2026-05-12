import { useCallback, useEffect, useState } from 'react'
import { InboxArrowDownIcon } from '@heroicons/react/24/outline'
import { mutationClient } from '../../api/clients'
import { SCHEMA_BADGE_COLORS } from '../../constants/ui'
import { BROWSER_CONFIG } from '../../constants/config'
import {
  createHashRangeKeyFilter,
  createHashKeyFilter,
  createRangeKeyFilter,
} from '../../utils/filterUtils'

type HashRangeFilter = ReturnType<typeof createHashKeyFilter>
import FaceOverlay from '../FaceOverlay'

export interface KeyValue {
  hash?: string | null
  range?: string | null
}

export interface RecordMetadataValue {
  molecule_version?: number
  molecule_uuid?: string
  source_file_name?: string
  metadata?: { file_hash?: string }
  [k: string]: unknown
}

export type RecordMetadataMap = Record<string, RecordMetadataValue>

interface SchemaLike {
  state?: string
  [k: string]: unknown
}

export function keyId(schemaName: string, kv: KeyValue | null | undefined): string {
  return `${schemaName}|${kv?.hash ?? ''}|${kv?.range ?? ''}`
}

export function keyLabel(kv: KeyValue | null | undefined): string {
  const parts: string[] = []
  if (kv?.hash) parts.push(kv.hash)
  if (kv?.range) parts.push(kv.range)
  return parts.join(' / ') || '(default)'
}

export function StateBadge({ state }: { state: string }) {
  const cls = SCHEMA_BADGE_COLORS[state] || 'badge badge-warning'
  return <span className={cls}>{state}</span>
}

export function SchemaTypeBadge({ schemaType }: { schemaType: string | Record<string, unknown> | null | undefined }) {
  if (!schemaType) return null
  const type = typeof schemaType === 'string' ? schemaType : Object.keys(schemaType)[0]
  if (!type) return null
  const colors: Record<string, string> = {
    Single: 'bg-gruvbox-blue/15 text-gruvbox-blue',
    Hash: 'bg-gruvbox-link/15 text-gruvbox-link',
    Range: 'bg-gruvbox-purple/15 text-gruvbox-purple',
    HashRange: 'bg-gruvbox-orange/15 text-gruvbox-orange',
  }
  return (
    <span className={`inline-flex items-center px-1.5 py-0.5 text-[10px] font-mono font-semibold rounded ${colors[type] || 'bg-surface-secondary text-secondary'}`}>
      {type}
    </span>
  )
}

export function getMaxVersion(metadata: RecordMetadataMap | null | undefined): number {
  if (!metadata || typeof metadata !== 'object') return 0
  let max = 0
  for (const v of Object.values(metadata)) {
    const ver = v?.molecule_version
    if (typeof ver === 'number' && ver > max) max = ver
  }
  return max
}

export function getFirstMoleculeUuid(metadata: RecordMetadataMap | null | undefined): string | null {
  if (!metadata || typeof metadata !== 'object') return null
  for (const v of Object.values(metadata)) {
    if (v?.molecule_uuid) return v.molecule_uuid
  }
  return null
}

export function VersionBadge({ version }: { version: number }) {
  if (!version || version <= 1) return null
  return (
    <span className="inline-flex items-center px-1.5 py-0.5 text-[10px] font-mono font-semibold rounded bg-gruvbox-blue/20 text-gruvbox-blue">
      v{version}
    </span>
  )
}

interface HistoryEvent {
  timestamp?: string
  version?: number
  old_atom_uuid?: string | null
  new_atom_uuid?: string | null
}

interface AtomContent {
  content?: unknown
}

export function VersionHistory({ moleculeUuid }: { moleculeUuid: string | null | undefined }) {
  const [expanded, setExpanded] = useState(false)
  const [events, setEvents] = useState<HistoryEvent[] | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [atomContents, setAtomContents] = useState<Record<string, AtomContent>>({})
  const [atomLoading, setAtomLoading] = useState<Record<string, boolean>>({})

  const fetchHistory = useCallback(async () => {
    if (!moleculeUuid || events) return
    setLoading(true)
    setError(null)
    try {
      const res = await mutationClient.getMoleculeHistory(moleculeUuid)
      const inner = (res.data && typeof res.data === 'object' && 'data' in res.data)
        ? (res.data as { data?: { events?: HistoryEvent[] } }).data
        : undefined
      if (res.success && inner) {
        setEvents(inner.events || [])
      } else {
        setError(res.error || 'Failed to load history')
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [moleculeUuid, events])

  const toggleExpand = useCallback(() => {
    const next = !expanded
    setExpanded(next)
    if (next) fetchHistory()
  }, [expanded, fetchHistory])

  const fetchAtom = useCallback(async (atomUuid: string | null | undefined) => {
    if (!atomUuid) return
    if (atomContents[atomUuid] || atomLoading[atomUuid]) return
    setAtomLoading((p) => ({ ...p, [atomUuid]: true }))
    try {
      const res = await mutationClient.getAtomContent(atomUuid)
      const inner = (res.data && typeof res.data === 'object' && 'data' in res.data)
        ? (res.data as { data?: AtomContent }).data
        : undefined
      if (res.success && inner) {
        setAtomContents((p) => ({ ...p, [atomUuid]: inner }))
      }
    } catch { /* ignore */ }
    finally {
      setAtomLoading((p) => ({ ...p, [atomUuid]: false }))
    }
  }, [atomContents, atomLoading])

  if (!moleculeUuid) return null

  return (
    <div className="mb-1">
      <button
        type="button"
        className="flex items-center gap-1 text-xs text-tertiary hover:text-secondary transition-colors"
        onClick={toggleExpand}
      >
        <span>{expanded ? '▾' : '▸'}</span>
        <span>Version history</span>
      </button>
      {expanded && (
        <div className="pl-4 pt-1">
          {loading && <div className="text-xs text-secondary">Loading history...</div>}
          {error && <div className="text-xs text-gruvbox-red">{error}</div>}
          {events && events.length === 0 && (
            <div className="text-xs text-secondary italic">No history events</div>
          )}
          {events && events.length > 0 && (
            <div className="space-y-1.5">
              {events.map((evt, i) => {
                const ts = evt.timestamp ? new Date(evt.timestamp).toLocaleString() : '?'
                const oldAtom = evt.old_atom_uuid
                const newAtom = evt.new_atom_uuid
                const oldContent = oldAtom ? atomContents[oldAtom] : null
                const newContent = newAtom ? atomContents[newAtom] : null

                return (
                  <div key={i} className="border-l-2 border-gruvbox-blue/30 pl-2 text-xs">
                    <div className="flex items-center gap-2 text-secondary">
                      <span className="font-mono">{ts}</span>
                      <span className="text-tertiary">v{evt.version}</span>
                    </div>
                    <div className="flex items-center gap-2 mt-0.5">
                      {oldAtom && (
                        <button
                          type="button"
                          className="text-gruvbox-red hover:underline font-mono text-[10px]"
                          onClick={() => fetchAtom(oldAtom)}
                          title={`Old: ${oldAtom}`}
                        >
                          {atomLoading[oldAtom] ? 'loading...' : 'old value'}
                        </button>
                      )}
                      {!oldAtom && <span className="text-tertiary text-[10px]">(created)</span>}
                      <span className="text-tertiary">-&gt;</span>
                      <button
                        type="button"
                        className="text-gruvbox-green hover:underline font-mono text-[10px]"
                        onClick={() => fetchAtom(newAtom)}
                        title={`New: ${newAtom}`}
                      >
                        {newAtom && atomLoading[newAtom] ? 'loading...' : 'new value'}
                      </button>
                    </div>
                    {oldContent && (
                      <div className="mt-1 p-1.5 bg-gruvbox-red/5 rounded text-[10px] font-mono break-all">
                        <span className="text-gruvbox-red">- </span>
                        {typeof oldContent.content === 'string'
                          ? oldContent.content
                          : JSON.stringify(oldContent.content)}
                      </div>
                    )}
                    {newContent && (
                      <div className="mt-1 p-1.5 bg-gruvbox-green/5 rounded text-[10px] font-mono break-all">
                        <span className="text-gruvbox-green">+ </span>
                        {typeof newContent.content === 'string'
                          ? newContent.content
                          : JSON.stringify(newContent.content)}
                      </div>
                    )}
                  </div>
                )
              })}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

const IMAGE_EXTENSIONS = /\.(jpe?g|png|gif|webp|svg)$/i

function authHeaders(): Record<string, string> {
  const userHash = localStorage.getItem(BROWSER_CONFIG.STORAGE_KEYS.USER_HASH)
  if (!userHash) return {}
  return { 'x-user-hash': userHash, 'x-user-id': userHash }
}

interface RecordMetadataProps {
  metadata: RecordMetadataMap | null | undefined
  schemaName?: string
  recordKey?: string
  sharedBy?: string
}

export function RecordMetadata({ metadata, schemaName, recordKey, sharedBy }: RecordMetadataProps) {
  const [expanded, setExpanded] = useState(false)
  const [blobUrl, setBlobUrl] = useState<string | null>(null)

  const entries: Array<[string, RecordMetadataValue]> =
    (metadata && typeof metadata === 'object') ? Object.entries(metadata) : []
  const representative: RecordMetadataValue | undefined =
    entries.find(([k]) => k === 'source_file_name')?.[1] ||
    entries.find(([, v]) => v?.source_file_name)?.[1] ||
    entries[0]?.[1]
  const sourceFile = representative?.source_file_name
  const fileHash = representative?.metadata?.file_hash
  const hasData = !!(sourceFile || fileHash)
  const isImage = !!sourceFile && IMAGE_EXTENSIONS.test(sourceFile)
  const fileUrl = fileHash ? `/api/file/${fileHash}?name=${encodeURIComponent(sourceFile || '')}` : null

  useEffect(() => {
    if (!expanded || !isImage || !fileUrl) return
    let revoked = false
    fetch(fileUrl, { headers: authHeaders() })
      .then((res) => {
        if (!res.ok) throw new Error(res.statusText)
        return res.blob()
      })
      .then((blob) => {
        if (revoked) return
        setBlobUrl(URL.createObjectURL(blob))
      })
      .catch(() => setBlobUrl(null))
    return () => {
      revoked = true
      setBlobUrl((prev) => { if (prev) URL.revokeObjectURL(prev); return null })
    }
  }, [expanded, isImage, fileUrl])

  const openFile = useCallback(() => {
    if (!fileUrl) return
    fetch(fileUrl, { headers: authHeaders() })
      .then((res) => {
        if (!res.ok) throw new Error(res.statusText)
        return res.blob()
      })
      .then((blob) => {
        const url = URL.createObjectURL(blob)
        window.open(url, '_blank')
      })
      .catch(() => {})
  }, [fileUrl])

  if (!hasData && !sharedBy) return null
  if (!hasData && sharedBy) {
    return (
      <div className="mb-1 text-xs text-gruvbox-green flex items-center gap-1">
        <InboxArrowDownIcon aria-hidden="true" className="w-3.5 h-3.5" />
        <span>{sharedBy}</span>
      </div>
    )
  }

  return (
    <div className="mb-1">
      {sharedBy && (
        <div className="text-xs text-gruvbox-green mb-1 flex items-center gap-1">
          <InboxArrowDownIcon aria-hidden="true" className="w-3.5 h-3.5" />
          <span>{sharedBy}</span>
        </div>
      )}
      <button
        type="button"
        className="flex items-center gap-1 text-xs text-tertiary hover:text-secondary transition-colors"
        onClick={() => setExpanded((v) => !v)}
      >
        <span>{expanded ? '▾' : '▸'}</span>
        <span>Source info</span>
        {sourceFile && !expanded && (
          <span className="font-mono text-secondary ml-1 truncate max-w-[300px]">{sourceFile}</span>
        )}
      </button>
      {expanded && (
        <div className="pl-4 pt-1 space-y-1 text-xs text-secondary font-mono">
          {sourceFile && fileUrl && !isImage && (
            <div>File: <button type="button" onClick={openFile} className="text-gruvbox-blue hover:underline cursor-pointer">{sourceFile}</button></div>
          )}
          {sourceFile && (!fileUrl || isImage) && (
            <div>File: {sourceFile}</div>
          )}
          {fileHash && <div>Hash: {fileHash.length > 16 ? fileHash.slice(0, 16) + '...' : fileHash}</div>}
          {isImage && blobUrl && (
            <div className="mt-2">
              <img
                src={blobUrl}
                alt={sourceFile}
                className="max-w-xs max-h-64 rounded border border-border object-contain bg-surface-secondary"
              />
              {schemaName && recordKey && (
                <FaceOverlay schemaName={schemaName} recordKey={recordKey} />
              )}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

export function buildFilter(kv: KeyValue | null | undefined): HashRangeFilter | undefined {
  const h = kv?.hash
  const r = kv?.range
  if (h && r) return createHashRangeKeyFilter(h, r)
  if (h) return createHashKeyFilter(h)
  if (r) return createRangeKeyFilter(r)
  return undefined
}

// Re-export SchemaLike for consumers — kept here since IngestionReport uses it indirectly via schema lookups
export type { SchemaLike }
