import { useMemo, useState, useCallback, useEffect } from 'react'
import {
  extractData,
  summarizeCounts,
  getSortedHashKeys,
  getSortedRangeKeys,
  getFieldsAt,
  sliceKeys,
  type FieldMap
} from '../utils/hashRangeResults'
import { executeQuery } from '../api/clients/mutationClient'
import { getSchema } from '../api/clients/schemaClient'
import { toggleSetItem, getFieldNames } from '../utils/schemaUtils'

const DEFAULT_PAGE_SIZE = 50

interface ReferenceKey {
  hash?: string
  range?: string
}

interface ReferenceValueShape {
  schema: string
  key: ReferenceKey
}

interface RangeWindowState {
  start: number
  count: number
}

function isReference(value: unknown): value is ReferenceValueShape {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false
  const v = value as { schema?: unknown; key?: unknown }
  return typeof v.schema === 'string' && !!v.key && typeof v.key === 'object'
}

function isReferenceArray(value: unknown): value is ReferenceValueShape[] {
  return Array.isArray(value) && value.length > 0 && value.some(isReference)
}

function buildFilterFromKey(key: ReferenceKey): Record<string, unknown> | undefined {
  if (key.hash && key.range) {
    return { HashRangeKey: { hash: key.hash, range: key.range } }
  }
  if (key.hash) {
    return { HashKey: key.hash }
  }
  if (key.range) {
    return { RangeKey: key.range }
  }
  return undefined
}

function truncateHash(s: unknown): string {
  if (typeof s === 'string' && s.length > 16) return s.slice(0, 8) + '...' + s.slice(-8)
  return String(s ?? '')
}

function referenceLabel(ref: ReferenceValueShape): string {
  const parts: string[] = []
  if (ref.key.hash) parts.push(`hash:${truncateHash(ref.key.hash)}`)
  if (ref.key.range) parts.push(`range:${ref.key.range}`)
  const name = truncateHash(ref.schema)
  return `${name} (${parts.join(', ')})`
}

interface ReferenceValueProps {
  reference: ReferenceValueShape
}

function ReferenceValue({ reference }: ReferenceValueProps) {
  const [fetched, setFetched] = useState<unknown>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [expanded, setExpanded] = useState(false)
  const [displayName, setDisplayName] = useState<string | null>(null)

  const handleFetch = async () => {
    if (fetched) {
      setExpanded(!expanded)
      return
    }
    setLoading(true)
    setError(null)
    try {
      const schemaRes = await getSchema(reference.schema)
      const schemaData = schemaRes.data as { schema?: unknown } | unknown
      const schema = (schemaData && typeof schemaData === 'object' && 'schema' in schemaData)
        ? (schemaData as { schema?: unknown }).schema
        : schemaData
      if (schema && typeof schema === 'object' && 'descriptive_name' in schema) {
        const name = (schema as { descriptive_name?: unknown }).descriptive_name
        if (typeof name === 'string') setDisplayName(name)
      }
      const fieldNames = getFieldNames(schema as Parameters<typeof getFieldNames>[0])
      if (fieldNames.length === 0) {
        throw new Error(`No fields found for schema "${reference.schema}"`)
      }

      const filter = buildFilterFromKey(reference.key)
      const query: Record<string, unknown> = { schema_name: reference.schema, fields: fieldNames }
      if (filter) query.filter = filter

      let queryRes = await executeQuery(query)
      if (!queryRes.success) {
        throw new Error(queryRes.error || 'Query failed')
      }
      const extractResults = (resp: typeof queryRes): unknown => {
        const data = resp.data as { results?: unknown } | unknown
        if (data && typeof data === 'object' && 'results' in data) {
          return (data as { results?: unknown }).results
        }
        return data
      }
      let results: unknown = extractResults(queryRes)
      if (Array.isArray(results) && results.length === 0 && filter) {
        const retryRes = await executeQuery({ schema_name: reference.schema, fields: fieldNames })
        if (retryRes.success) {
          results = extractResults(retryRes)
        }
      }
      setFetched(results)
      setExpanded(true)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="space-y-1">
      <div className="flex items-center gap-2">
        <span className="font-mono text-xs text-secondary">→ {displayName || referenceLabel(reference)}</span>
        <button
          type="button"
          className="btn-secondary btn-sm text-xs px-2 py-0.5"
          onClick={handleFetch}
          disabled={loading}
        >
          {loading ? 'Loading...' : fetched ? (expanded ? 'Hide' : 'Show') : 'Fetch'}
        </button>
      </div>
      {error && <div className="text-xs text-gruvbox-red">{error}</div>}
      {fetched != null && expanded ? (
        <div className="ml-4 border-l border-border pl-2">
          {Array.isArray(fetched)
            ? fetched.map((item: unknown, i: number) => {
                const fields = (item && typeof item === 'object' && 'fields' in item)
                  ? (item as { fields?: FieldMap }).fields
                  : (item as FieldMap | undefined)
                return <FieldsTable key={i} fields={fields ?? {}} />
              })
            : (() => {
                const fields = (fetched && typeof fetched === 'object' && 'fields' in fetched)
                  ? (fetched as { fields?: FieldMap }).fields
                  : (fetched as FieldMap | undefined)
                return <FieldsTable fields={fields ?? {}} />
              })()}
        </div>
      ) : null}
    </div>
  )
}

function ReferenceArrayValue({ references }: { references: ReferenceValueShape[] }) {
  return (
    <div className="space-y-2">
      <span className="text-xs text-secondary">{references.length} reference{references.length !== 1 ? 's' : ''}</span>
      {references.map((ref, i) => (
        <ReferenceValue key={`${ref.schema}-${ref.key?.hash || ''}-${ref.key?.range || ''}-${i}`} reference={ref} />
      ))}
    </div>
  )
}

function renderFieldValue(value: unknown) {
  if (isReference(value)) {
    return <ReferenceValue reference={value} />
  }
  if (isReferenceArray(value)) {
    return <ReferenceArrayValue references={value.filter(isReference)} />
  }
  return <pre className="font-mono whitespace-pre-wrap break-words">{formatValue(value)}</pre>
}

interface ToggleButtonProps {
  isOpen: boolean
  onClick: () => void
  label: string
}

function ToggleButton({ isOpen, onClick, label }: ToggleButtonProps) {
  return (
    <button
      type="button"
      className="text-left w-full flex items-center justify-between px-3 py-2 hover:bg-surface-secondary rounded"
      onClick={onClick}
      aria-expanded={isOpen}
    >
      <span className="font-mono text-sm text-primary truncate">{label}</span>
      <span className="ml-2 text-secondary text-xs">{isOpen ? '▼' : '▶'}</span>
    </button>
  )
}

export function FieldsTable({ fields }: { fields: FieldMap | null | undefined }) {
  const entries = useMemo(() => Object.entries(fields || {}), [fields])
  if (entries.length === 0) {
    return (
      <div className="text-xs text-secondary italic px-3 py-2">No fields</div>
    )
  }

  return (
    <div className="px-3 py-2 overflow-x-auto">
      <table className="min-w-full border-separate border-spacing-y-1">
        <tbody>
          {entries.map(([k, v]) => (
            <tr key={k} className="bg-surface">
              <td className="align-top text-xs font-medium text-primary pr-4 whitespace-nowrap max-w-[200px] truncate" title={k}>{k}</td>
              <td className="align-top text-xs text-primary max-w-[500px]">
                <div className="max-h-48 overflow-y-auto overflow-x-hidden">
                  {renderFieldValue(v)}
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function formatValue(value: unknown): string {
  if (value === null) return 'null'
  if (typeof value === 'string') return value
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

interface StructuredResultsProps {
  results: unknown
  pageSize?: number
}

export default function StructuredResults({ results, pageSize = DEFAULT_PAGE_SIZE }: StructuredResultsProps) {
  const data = useMemo(() => extractData(results) || {}, [results])
  const counts = useMemo(() => summarizeCounts(results), [results])
  const allHashes = useMemo(() => getSortedHashKeys(results), [results])

  const [hashOpen, setHashOpen] = useState<Set<string>>(() => new Set())
  const [rangeOpen, setRangeOpen] = useState<Set<string>>(() => new Set())

  const [hashWindow, setHashWindow] = useState<RangeWindowState>({ start: 0, count: pageSize })
  const [rangeWindows, setRangeWindows] = useState<Map<string, RangeWindowState>>(() => new Map())

  useEffect(() => {
    setHashOpen(new Set(allHashes))
    const windows = new Map<string, RangeWindowState>()
    allHashes.forEach((h) => {
      const total = getSortedRangeKeys(data, h).length
      windows.set(h, { start: 0, count: Math.min(pageSize, total) })
    })
    setRangeWindows(windows)
  }, [allHashes, data, pageSize])

  const toggleHash = useCallback((h: string) => {
    setHashOpen((prev) => toggleSetItem(prev, h))
    setRangeWindows((prev) => {
      if (!hashOpen.has(h)) {
        const total = getSortedRangeKeys(data, h).length
        const next = new Map(prev)
        next.set(h, { start: 0, count: Math.min(pageSize, total) })
        return next
      }
      return prev
    })
  }, [data, hashOpen, pageSize])

  const toggleRange = useCallback((h: string, r: string) => {
    const key = h + '||' + r
    setRangeOpen((prev) => toggleSetItem(prev, key))
  }, [])

  const showMoreHashes = useCallback(() => {
    const nextCount = Math.min(allHashes.length, hashWindow.count + pageSize)
    setHashWindow(() => ({ start: 0, count: nextCount }))
  }, [allHashes, hashWindow.count, pageSize])

  const visibleHashes = useMemo(() => sliceKeys(allHashes, hashWindow.start, hashWindow.count), [allHashes, hashWindow])

  return (
    <div className="space-y-2">
      <div className="text-xs text-secondary">
        <span className="mr-4">Hashes: <strong>{counts.hashes}</strong></span>
        <span>Ranges: <strong>{counts.ranges}</strong></span>
      </div>

      <div className="border border-border divide-y divide-border bg-surface-secondary">
        {visibleHashes.map((h) => (
          <div key={h} className="p-2">
            <ToggleButton
              isOpen={hashOpen.has(h)}
              onClick={() => toggleHash(h)}
              label={`hash: ${String(h)}`}
            />

            {hashOpen.has(h) && (
              <HashRanges
                data={data}
                hashKey={h}
                rangeOpen={rangeOpen}
                onToggleRange={toggleRange}
                pageSize={pageSize}
                rangeWindow={rangeWindows.get(h)}
                setRangeWindow={(w) => setRangeWindows((prev) => new Map(prev).set(h, w))}
              />
            )}
          </div>
        ))}
      </div>

      {hashWindow.count < allHashes.length && (
        <div className="pt-2">
          <button type="button" onClick={showMoreHashes} className="btn-secondary btn-sm">
            Show more hashes ({hashWindow.count}/{allHashes.length})
          </button>
        </div>
      )}
    </div>
  )
}

interface HashRangesProps {
  data: unknown
  hashKey: string
  rangeOpen: Set<string>
  onToggleRange: (h: string, r: string) => void
  pageSize: number
  rangeWindow: RangeWindowState | undefined
  setRangeWindow: (w: RangeWindowState) => void
}

function HashRanges({ data, hashKey, rangeOpen, onToggleRange, pageSize, rangeWindow, setRangeWindow }: HashRangesProps) {
  const allRanges = useMemo(() => getSortedRangeKeys(data, hashKey), [data, hashKey])
  const effectiveWindow = useMemo(
    () => rangeWindow || { start: 0, count: Math.min(pageSize, allRanges.length) },
    [rangeWindow, pageSize, allRanges.length]
  )
  const visibleRanges = useMemo(
    () => sliceKeys(allRanges, effectiveWindow.start, effectiveWindow.count),
    [allRanges, effectiveWindow]
  )

  const showMoreRanges = useCallback(() => {
    const next = Math.min(allRanges.length, effectiveWindow.count + pageSize)
    setRangeWindow({ start: 0, count: next })
  }, [allRanges.length, effectiveWindow.count, pageSize, setRangeWindow])

  return (
    <div className="ml-4 mt-1 border-l pl-3">
      {visibleRanges.map((r) => (
        <div key={r} className="py-1">
          <ToggleButton
            isOpen={rangeOpen.has(hashKey + '||' + r)}
            onClick={() => onToggleRange(hashKey, r)}
            label={`range: ${String(r)}`}
          />
          {rangeOpen.has(hashKey + '||' + r) && (
            <div className="ml-4 mt-1">
              <FieldsTable fields={getFieldsAt(data, hashKey, r) || {}} />
            </div>
          )}
        </div>
      ))}

      {effectiveWindow.count < allRanges.length && (
        <div className="pt-1">
          <button type="button" onClick={showMoreRanges} className="btn-secondary btn-sm">
            Show more ranges ({effectiveWindow.count}/{allRanges.length})
          </button>
        </div>
      )}
    </div>
  )
}
