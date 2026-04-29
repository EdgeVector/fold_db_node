import { useCallback, useEffect, useMemo, useState, Fragment } from 'react'
import {
  InboxArrowDownIcon,
  MagnifyingGlassIcon,
  XMarkIcon,
} from '@heroicons/react/24/outline'
import { useAppSelector } from '../../store/hooks'
import { selectAllSchemas } from '../../store/schemaSlice'
import { useOrgNames } from '../../hooks/useOrgNames'
import { schemaClient } from '../../api/clients/schemaClient'
import { mutationClient, systemClient } from '../../api/clients'
import * as trustClient from '../../api/clients/trustClient'
import { FieldsTable } from '../StructuredResults'
import SchemaName from '../shared/SchemaName'
import ShareRecordModal from '../ShareRecordModal'
import { ArrowUpTrayIcon } from '@heroicons/react/24/outline'
import { getFieldNames, toggleSetItem } from '../../utils/schemaUtils'
import {
  keyId,
  keyLabel,
  StateBadge,
  SchemaTypeBadge,
  getMaxVersion,
  getFirstMoleculeUuid,
  VersionBadge,
  VersionHistory,
  RecordMetadata,
  buildFilter,
} from '../data-browser/shared'

const PAGE_SIZE = 50

export default function DataBrowserTab() {
  const schemas = useAppSelector(selectAllSchemas)

  // Schema-level expand state + cached keys
  const [expandedSchemas, setExpandedSchemas] = useState(() => new Set())
  const [schemaKeys, setSchemaKeys] = useState({})       // { name: { keys, total_count } }
  const [schemaLoading, setSchemaLoading] = useState({})  // { name: bool }
  const [schemaErrors, setSchemaErrors] = useState({})    // { name: string }

  const orgNames = useOrgNames()

  // Key-level expand state + cached records
  const [expandedKeys, setExpandedKeys] = useState(() => new Set())
  const [keyRecords, setKeyRecords] = useState({})        // { compositeId: { fields, metadata } }
  const [keyLoading, setKeyLoading] = useState({})        // { compositeId: bool }
  const [shareTarget, setShareTarget] = useState(null)

  // Contact book (pub_key -> display_name) + own node public key, used to render
  // the "Shared by X" badge on records whose author differs from this node.
  const [contactsByKey, setContactsByKey] = useState(() => new Map())
  const [ownPublicKey, setOwnPublicKey] = useState(null)

  useEffect(() => {
    let cancelled = false
    async function load() {
      try {
        const res = await trustClient.listContacts()
        if (!cancelled && res?.success && res.data?.contacts) {
          const map = new Map()
          for (const c of res.data.contacts) {
            if (c?.public_key) map.set(c.public_key, c.display_name || 'Unknown')
          }
          setContactsByKey(map)
        }
      } catch { /* ignore */ }
      try {
        const res = await systemClient.getAutoIdentity()
        if (!cancelled && res?.success) {
          const pk = res.data?.public_key || res.data?.data?.public_key || null
          setOwnPublicKey(pk)
        }
      } catch { /* ignore */ }
    }
    load()
    return () => { cancelled = true }
  }, [])

  // Resolve author_pub_key -> badge label. Returns null when the record is owned
  // by this node (no badge) or when author is unknown (omitted by the caller).
  const resolveSharedBy = useCallback((authorPubKey) => {
    if (!authorPubKey) return null
    if (ownPublicKey && authorPubKey === ownPublicKey) return null
    const name = contactsByKey.get(authorPubKey)
    return name ? `Shared by ${name}` : 'Shared by External'
  }, [contactsByKey, ownPublicKey])

  const schemaList = useMemo(() => {
    if (!Array.isArray(schemas)) return []
    return [...schemas]
      .filter((s) => s.state !== 'blocked' && s.state !== 'available')
      .sort((a, b) =>
        (a.descriptive_name || a.name || '').localeCompare(b.descriptive_name || b.name || '')
      )
  }, [schemas])

  // Type-to-filter for the schema list. With a real ingest the list can
  // grow into the dozens (one schema per inferred shape), and scrolling
  // to find one is the bottleneck. Substring match against name +
  // descriptive_name; case-insensitive; trimmed. Empty query passes
  // everything through unchanged.
  const [filterQuery, setFilterQuery] = useState('')
  const visibleSchemas = useMemo(() => {
    const q = filterQuery.trim().toLowerCase()
    if (q === '') return schemaList
    return schemaList.filter((s) => {
      const name = (s.name || '').toLowerCase()
      const descriptive = (s.descriptive_name || '').toLowerCase()
      return name.includes(q) || descriptive.includes(q)
    })
  }, [schemaList, filterQuery])

  const fieldCount = useCallback((schema) => getFieldNames(schema).length, [])

  // -- Schema expansion: fetch keys --
  const toggleSchema = useCallback(async (name) => {
    setExpandedSchemas((prev) => toggleSetItem(prev, name))

    // Fetch keys on first expand (or if not already loaded)
    if (!schemaKeys[name] && !schemaLoading[name]) {
      setSchemaLoading((p) => ({ ...p, [name]: true }))
      setSchemaErrors((p) => ({ ...p, [name]: null }))
      try {
        const res = await schemaClient.listSchemaKeys(name, 0, PAGE_SIZE)
        if (res.success && res.data) {
          setSchemaKeys((p) => ({ ...p, [name]: { keys: res.data.keys || [], total_count: res.data.total_count || 0 } }))
        } else {
          setSchemaErrors((p) => ({ ...p, [name]: res.error || 'Failed to fetch keys' }))
        }
      } catch (e) {
        setSchemaErrors((p) => ({ ...p, [name]: (e instanceof Error ? e.message : String(e)) || 'Network error' }))
      } finally {
        setSchemaLoading((p) => ({ ...p, [name]: false }))
      }
    }
  }, [schemaKeys, schemaLoading])

  // -- Load more keys --
  const loadMoreKeys = useCallback(async (name) => {
    const current = schemaKeys[name]
    if (!current) return
    const offset = current.keys.length
    setSchemaLoading((p) => ({ ...p, [name]: true }))
    try {
      const res = await schemaClient.listSchemaKeys(name, offset, PAGE_SIZE)
      if (res.success && res.data) {
        setSchemaKeys((p) => ({
          ...p,
          [name]: {
            keys: [...(p[name]?.keys || []), ...(res.data.keys || [])],
            total_count: res.data.total_count || p[name]?.total_count || 0,
          },
        }))
      }
    } catch {
      // silent - user can retry
    } finally {
      setSchemaLoading((p) => ({ ...p, [name]: false }))
    }
  }, [schemaKeys])

  // -- Key expansion: fetch field values --
  const toggleKey = useCallback(async (schemaName, kv, schema) => {
    const id = keyId(schemaName, kv)
    setExpandedKeys((prev) => toggleSetItem(prev, id))

    if (!keyRecords[id] && !keyLoading[id]) {
      setKeyLoading((p) => ({ ...p, [id]: true }))
      try {
        const fields = getFieldNames(schema)
        const filter = buildFilter(kv)
        const query = { schema_name: schemaName, fields }
        if (filter) query.filter = filter
        const res = await mutationClient.executeQuery(query)
        if (res.success) {
          const arr = Array.isArray(res.data?.results) ? res.data.results : []
          const match = arr.find((x) => {
            return String(x?.key?.hash || '') === String(kv?.hash || '') &&
                   String(x?.key?.range || '') === String(kv?.range || '')
          }) || arr[0]
          setKeyRecords((p) => ({ ...p, [id]: { fields: match?.fields || {}, metadata: match?.metadata || {}, author_pub_key: match?.author_pub_key || null } }))
        } else {
          setKeyRecords((p) => ({ ...p, [id]: { fields: {}, metadata: {}, author_pub_key: null } }))
        }
      } catch { /* show empty fields on error - user can re-expand */
        setKeyRecords((p) => ({ ...p, [id]: { fields: {}, metadata: {}, author_pub_key: null } }))
      } finally {
        setKeyLoading((p) => ({ ...p, [id]: false }))
      }
    }
  }, [keyRecords, keyLoading])

  if (schemaList.length === 0) {
    return (
      <div className="text-secondary text-sm py-6 text-center">
        No schemas loaded. Ingest some data first.
      </div>
    )
  }

  // Filter bar header. Renders whenever there's at least one schema —
  // even at N=1 the count is useful, and the input being there from the
  // start means the "type to filter" interaction is discoverable
  // without anything having to change as the list grows.
  const renderFilterBar = () => (
    <div className="flex items-center gap-3 mb-3">
      <div className="relative flex-1">
        <MagnifyingGlassIcon
          aria-hidden="true"
          className="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-tertiary pointer-events-none"
        />
        <input
          type="text"
          value={filterQuery}
          onChange={(e) => setFilterQuery(e.target.value)}
          placeholder="Filter schemas — type a name"
          className="input pl-9 pr-9"
          aria-label="Filter schemas"
        />
        {filterQuery && (
          <button
            type="button"
            onClick={() => setFilterQuery('')}
            aria-label="Clear filter"
            className="absolute right-2 top-1/2 -translate-y-1/2 text-tertiary hover:text-primary p-1"
          >
            <XMarkIcon aria-hidden="true" className="w-4 h-4" />
          </button>
        )}
      </div>
      <span className="text-xs text-tertiary whitespace-nowrap">
        {filterQuery
          ? `${visibleSchemas.length} of ${schemaList.length}`
          : `${schemaList.length} ${schemaList.length === 1 ? 'schema' : 'schemas'}`}
      </span>
    </div>
  )

  return (
    <div>
      {renderFilterBar()}
      {visibleSchemas.length === 0 && (
        <div className="text-secondary text-sm py-8 text-center border border-dashed border-border rounded">
          No schemas match <span className="text-primary font-mono">"{filterQuery}"</span>.
        </div>
      )}
      <div className="space-y-1">
      {visibleSchemas.map((schema) => {
        const name = schema.name
        const isOpen = expandedSchemas.has(name)
        const data = schemaKeys[name]
        const loading = schemaLoading[name]
        const error = schemaErrors[name]

        return (
          <div key={name} className="border border-border">
            {/* Schema row */}
            <button
              type="button"
              className="w-full flex items-center gap-2 px-3 py-2 text-left bg-surface hover:bg-surface-secondary transition-colors"
              onClick={() => toggleSchema(name)}
            >
              <span className="text-xs text-secondary">{isOpen ? '▾' : '▸'}</span>
              <SchemaName schema={schema} name={name} />
              <SchemaTypeBadge schemaType={schema.schema_type} />
              {schema.org_hash && (
                <span className="px-1.5 py-0.5 text-xs rounded bg-gruvbox-blue/15 text-gruvbox-blue" title={schema.org_hash}>
                  {orgNames[schema.org_hash] || 'Org'}
                </span>
              )}
              <span className="text-xs text-tertiary">({fieldCount(schema)} fields)</span>
              {data && <span className="text-xs text-tertiary">({data.total_count} {data.total_count === 1 ? 'record' : 'records'})</span>}
              <StateBadge state={schema.state || 'approved'} />
            </button>

            {/* Keys list */}
            {isOpen && (
              <div className="pl-6 pr-3 pb-2 bg-surface-secondary">
                {loading && !data && (
                  <div className="text-xs text-secondary py-2">Loading keys...</div>
                )}
                {error && (
                  <div className="text-xs text-gruvbox-red py-1">{error}</div>
                )}
                {data && data.keys.length === 0 && (
                  <div className="text-xs text-secondary py-2 italic">No keys found</div>
                )}
                {data && data.keys.map((kv) => {
                  const id = keyId(name, kv)
                  const isKeyOpen = expandedKeys.has(id)
                  const record = keyRecords[id]
                  const kLoading = keyLoading[id]

                  const maxVersion = record ? getMaxVersion(record.metadata) : 0
                  const molUuid = record ? getFirstMoleculeUuid(record.metadata) : null
                  const sharedBy = record ? resolveSharedBy(record.author_pub_key) : null

                  return (
                    <div key={id} className="border-b border-border last:border-b-0">
                      <div className="flex items-center">
                        <button
                          type="button"
                          className="flex-1 flex items-center gap-2 px-2 py-1.5 text-left hover:bg-surface transition-colors"
                          onClick={() => toggleKey(name, kv, schema)}
                        >
                          <span className="text-xs text-secondary">{isKeyOpen ? '▾' : '▸'}</span>
                          <span className="text-xs font-mono text-primary">{keyLabel(kv)}</span>
                          <VersionBadge version={maxVersion} />
                          {sharedBy && (
                            <span
                              className="inline-flex items-center gap-1 px-1.5 py-0.5 text-[10px] font-semibold rounded bg-gruvbox-green/15 text-gruvbox-green"
                              title={record?.author_pub_key || ''}
                            >
                              <InboxArrowDownIcon aria-hidden="true" className="w-3 h-3" />
                              <span>{sharedBy}</span>
                            </span>
                          )}
                          {kLoading && <span className="text-xs text-secondary">(loading...)</span>}
                        </button>
                        <button
                          type="button"
                          className="p-1 text-tertiary hover:text-primary transition-colors bg-transparent border-none cursor-pointer"
                          title="Share with contact"
                          onClick={(e) => { e.stopPropagation(); setShareTarget({ schema: name, key: kv }); }}
                        >
                          <ArrowUpTrayIcon className="w-4 h-4" />
                        </button>
                      </div>

                      {isKeyOpen && (
                        <div className="pl-6 pb-2">
                          {record ? (
                            <Fragment>
                              <RecordMetadata metadata={record.metadata} schemaName={name} recordKey={keyLabel(kv)} sharedBy={sharedBy} />
                              {maxVersion > 1 && <VersionHistory moleculeUuid={molUuid} />}
                              <FieldsTable fields={record.fields} />
                            </Fragment>
                          ) : (
                            <div className="text-xs text-secondary italic">Loading...</div>
                          )}
                        </div>
                      )}
                    </div>
                  )
                })}

                {/* Show more */}
                {data && data.keys.length < data.total_count && (
                  <div className="pt-2">
                    <button
                      type="button"
                      className="btn-secondary btn-sm"
                      onClick={() => loadMoreKeys(name)}
                      disabled={loading}
                    >
                      {loading ? 'Loading...' : `Show more keys (${data.keys.length}/${data.total_count})`}
                    </button>
                  </div>
                )}
              </div>
            )}
          </div>
        )
      })}
      </div>
      {shareTarget && (
        <ShareRecordModal
          schemaName={shareTarget.schema}
          recordKey={shareTarget.key}
          isOpen={!!shareTarget}
          onClose={() => setShareTarget(null)}
        />
      )}
    </div>
  )
}
