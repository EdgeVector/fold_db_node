import { useCallback, useEffect, useMemo, useState, Fragment } from 'react'
import { useAppDispatch, useAppSelector } from '../store/hooks'
import { fetchSchemas, selectAllSchemas } from '../store/schemaSlice'
import { mutationClient } from '../api/clients'
import { schemaClient } from '../api/clients/schemaClient'
import { FieldsTable } from './StructuredResults'
import SchemaName from './shared/SchemaName'
import { getFieldNames as getFieldNamesUtil, getSchemaDisplayName, toggleSetItem } from '../utils/schemaUtils'
import {
  keyId,
  keyLabel,
  StateBadge,
  getMaxVersion,
  getFirstMoleculeUuid,
  VersionBadge,
  VersionHistory,
  RecordMetadata,
  buildFilter,
  type KeyValue,
  type RecordMetadataMap,
} from './data-browser/shared'

interface IngestionResultData {
  schemas_written?: Array<{ schema_name: string; keys_written?: KeyValue[] }>
  mutations_generated?: number
  mutations_executed?: number
  new_schema_created?: boolean
}

interface IngestionResultLike {
  data?: IngestionResultData
  schemas_written?: IngestionResultData['schemas_written']
  mutations_generated?: number
  mutations_executed?: number
  new_schema_created?: boolean
}

interface IngestionReportProps {
  ingestionResult: IngestionResultLike | null | undefined
  onDismiss?: () => void
}

interface KeyRecord {
  fields: Record<string, unknown>
  metadata: RecordMetadataMap
  notFound: boolean
}

interface SchemaShape {
  name?: string
  state?: string
  fields?: unknown
  [k: string]: unknown
}

export default function IngestionReport({ ingestionResult, onDismiss }: IngestionReportProps) {
  const dispatch = useAppDispatch()
  const allSchemas = useAppSelector(selectAllSchemas) as SchemaShape[]

  const data: IngestionResultData | undefined = ingestionResult?.data || (ingestionResult as IngestionResultData | undefined)
  const schemasWrittenRaw = data?.schemas_written || []
  const mutationsGenerated = data?.mutations_generated ?? 0
  const mutationsExecuted = data?.mutations_executed ?? 0
  const newSchemaCreated = data?.new_schema_created ?? false

  const [schemasLoaded, setSchemasLoaded] = useState(false)

  useEffect(() => {
    dispatch(fetchSchemas({ forceRefresh: true })).finally(() => setSchemasLoaded(true))
  }, [dispatch])

  const schemaLookup = useMemo(() => {
    const map: Record<string, SchemaShape> = {}
    if (Array.isArray(allSchemas)) {
      for (const s of allSchemas) {
        if (s?.name) map[s.name] = s
      }
    }
    return map
  }, [allSchemas])

  const schemasWritten = useMemo(() => {
    const visible = schemasLoaded
      ? schemasWrittenRaw.filter((sw) => !!schemaLookup[sw.schema_name])
      : schemasWrittenRaw
    return [...visible].sort((a, b) => {
      const nameA = getSchemaDisplayName(schemaLookup[a.schema_name]) || a.schema_name || ''
      const nameB = getSchemaDisplayName(schemaLookup[b.schema_name]) || b.schema_name || ''
      return nameA.localeCompare(nameB)
    })
  }, [schemasWrittenRaw, schemaLookup, schemasLoaded])

  const [expandedSchemas, setExpandedSchemas] = useState<Set<string>>(() => new Set())
  const [expandedKeys, setExpandedKeys] = useState<Set<string>>(() => new Set())
  const [keyRecords, setKeyRecords] = useState<Record<string, KeyRecord>>({})
  const [keyLoading, setKeyLoading] = useState<Record<string, boolean>>({})
  const [fieldNamesCache, setFieldNamesCache] = useState<Record<string, string[]>>({})

  const getFieldNames = useCallback((schemaName: string): string[] => {
    const schema = schemaLookup[schemaName]
    if (schema) return getFieldNamesUtil(schema)
    return fieldNamesCache[schemaName] || []
  }, [schemaLookup, fieldNamesCache])

  const fieldCount = useCallback((schemaName: string) => {
    return getFieldNames(schemaName).length
  }, [getFieldNames])

  const toggleSchema = useCallback(async (schemaName: string) => {
    setExpandedSchemas((prev) => toggleSetItem(prev, schemaName))

    if (!schemaLookup[schemaName] && !fieldNamesCache[schemaName]) {
      try {
        const res = await schemaClient.getSchema(schemaName)
        const data = res.data as { schema?: SchemaShape } | SchemaShape | undefined
        const schema = (data && typeof data === 'object' && 'schema' in data ? (data as { schema?: SchemaShape }).schema : data) as SchemaShape | undefined
        if (schema) {
          const f = schema.fields
          const names = Array.isArray(f) ? f.filter((x): x is string => typeof x === 'string')
            : (f && typeof f === 'object') ? Object.keys(f as object) : []
          setFieldNamesCache((p) => ({ ...p, [schemaName]: names }))
        }
      } catch { /* will show empty fields */ }
    }
  }, [schemaLookup, fieldNamesCache])

  const toggleKey = useCallback(async (schemaName: string, kv: KeyValue) => {
    const id = keyId(schemaName, kv)
    setExpandedKeys((prev) => toggleSetItem(prev, id))

    if (!keyRecords[id] && !keyLoading[id]) {
      setKeyLoading((p) => ({ ...p, [id]: true }))
      try {
        let fields = getFieldNames(schemaName)
        if (fields.length === 0) {
          try {
            const res = await schemaClient.getSchema(schemaName)
            const data = res.data as { schema?: SchemaShape } | SchemaShape | undefined
            const schema = (data && typeof data === 'object' && 'schema' in data ? (data as { schema?: SchemaShape }).schema : data) as SchemaShape | undefined
            if (schema) {
              const f = schema.fields
              const names = Array.isArray(f) ? f.filter((x): x is string => typeof x === 'string')
                : (f && typeof f === 'object') ? Object.keys(f as object) : []
              setFieldNamesCache((p) => ({ ...p, [schemaName]: names }))
              fields = names
            }
          } catch { /* proceed with empty */ }
        }
        const filter = buildFilter(kv)
        const query: Record<string, unknown> = { schema_name: schemaName, fields }
        if (filter) query.filter = filter
        const res = await mutationClient.executeQuery(query)
        if (res.success) {
          const responseData = res.data as { results?: unknown }
          const arr = Array.isArray(responseData?.results) ? responseData.results : []
          const match = arr.find((x: unknown) => {
            const item = x as { key?: KeyValue } | null
            return String(item?.key?.hash || '') === String(kv?.hash || '') &&
                   String(item?.key?.range || '') === String(kv?.range || '')
          }) || arr[0]
          const matchObj = match as { fields?: Record<string, unknown>; metadata?: RecordMetadataMap } | undefined
          const notFound = !match
          setKeyRecords((p) => ({ ...p, [id]: { fields: matchObj?.fields || {}, metadata: matchObj?.metadata || {}, notFound } }))
        } else {
          setKeyRecords((p) => ({ ...p, [id]: { fields: {}, metadata: {}, notFound: true } }))
        }
      } catch {
        setKeyRecords((p) => ({ ...p, [id]: { fields: {}, metadata: {}, notFound: true } }))
      } finally {
        setKeyLoading((p) => ({ ...p, [id]: false }))
      }
    }
  }, [keyRecords, keyLoading, getFieldNames])

  if (schemasWritten.length === 0) {
    return null
  }

  return (
    <div className="mt-6 card">
      <div className="flex items-center justify-between px-4 py-3 border-b border-border bg-surface-secondary">
        <div className="flex items-center gap-3">
          <span className="text-gruvbox-green">✔</span>
          <span className="font-medium">INGESTION REPORT</span>
          <span className="text-xs text-secondary">
            {mutationsGenerated} generated, {mutationsExecuted} executed
          </span>
          {newSchemaCreated && (
            <span className="inline-flex items-center px-1.5 py-0.5 text-[10px] font-semibold rounded bg-gruvbox-green/20 text-gruvbox-green">
              new
            </span>
          )}
        </div>
        {onDismiss && (
          <button
            type="button"
            className="btn-secondary btn-sm text-xs"
            onClick={onDismiss}
          >
            Dismiss
          </button>
        )}
      </div>

      <div className="p-4 space-y-1">
        {schemasWritten.map((sw) => {
          const name = sw.schema_name
          const keys: KeyValue[] = sw.keys_written || []
          const isOpen = expandedSchemas.has(name)
          const schema = schemaLookup[name]

          return (
            <div key={name} className="border border-border">
              <button
                type="button"
                className="w-full flex items-center gap-2 px-3 py-2 text-left bg-surface hover:bg-surface-secondary transition-colors"
                onClick={() => toggleSchema(name)}
              >
                <span className="text-xs text-secondary">{isOpen ? '▾' : '▸'}</span>
                <SchemaName schema={schema} name={name} />
                <span className="text-xs text-tertiary">
                  ({keys.length} record{keys.length !== 1 ? 's' : ''})
                </span>
                {fieldCount(name) > 0 && (
                  <span className="text-xs text-tertiary">({fieldCount(name)} fields)</span>
                )}
                {schema && <StateBadge state={schema.state || 'approved'} />}
              </button>

              {isOpen && (
                <div className="pl-6 pr-3 pb-2 bg-surface-secondary">
                  {keys.length === 0 && (
                    <div className="text-xs text-secondary py-2 italic">No keys</div>
                  )}
                  {keys.map((kv) => {
                    const id = keyId(name, kv)
                    const isKeyOpen = expandedKeys.has(id)
                    const record = keyRecords[id]
                    const kLoading = keyLoading[id]

                    const maxVersion = record ? getMaxVersion(record.metadata) : 0
                    const molUuid = record ? getFirstMoleculeUuid(record.metadata) : null

                    return (
                      <div key={id} className="border-b border-border last:border-b-0">
                        <button
                          type="button"
                          className="w-full flex items-center gap-2 px-2 py-1.5 text-left hover:bg-surface transition-colors"
                          onClick={() => toggleKey(name, kv)}
                        >
                          <span className="text-xs text-secondary">{isKeyOpen ? '▾' : '▸'}</span>
                          <span className="text-xs font-mono text-primary">{keyLabel(kv)}</span>
                          <VersionBadge version={maxVersion} />
                          {kLoading && <span className="text-xs text-secondary">(loading...)</span>}
                        </button>

                        {isKeyOpen && (
                          <div className="pl-6 pb-2">
                            {record ? (
                              record.notFound ? (
                                <div className="text-xs text-gruvbox-red italic px-3 py-2">Record not found in database</div>
                              ) : (
                                <Fragment>
                                  <RecordMetadata metadata={record.metadata} schemaName={name} recordKey={keyLabel(kv)} />
                                  {maxVersion > 1 && <VersionHistory moleculeUuid={molUuid} />}
                                  <FieldsTable fields={record.fields} />
                                </Fragment>
                              )
                            ) : (
                              <div className="text-xs text-secondary italic">Loading...</div>
                            )}
                          </div>
                        )}
                      </div>
                    )
                  })}
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}
