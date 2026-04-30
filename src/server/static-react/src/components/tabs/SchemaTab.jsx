import { useState, useEffect, useRef, useCallback } from 'react'
import { ChevronDownIcon, ChevronRightIcon } from '@heroicons/react/24/solid'
import { getRangeSchemaInfo, getHashRangeSchemaInfo } from '../../utils/rangeSchemaHelpers'
import { useAppSelector, useAppDispatch } from '../../store/hooks'
import {
  selectAllSchemas,
  approveSchema as approveSchemaAction,
  blockSchema as blockSchemaAction,
  fetchSchemas
} from '../../store/schemaSlice'
import SchemaName from '../shared/SchemaName'
import { MagnifyingGlassIcon, EllipsisHorizontalIcon } from '@heroicons/react/24/outline'
import { toErrorMessage, isSystemSchema } from '../../utils/schemaUtils'
import { getAllFieldPolicies, setFieldPolicy as setFieldPolicyApi } from '../../api/clients/sharingClient'
import schemaClient from '../../api/clients/schemaClient'
import { useOrgNames } from '../../hooks/useOrgNames'

const TRUST_TIERS = ['Public', 'Outer', 'Trusted', 'Inner', 'Owner']

// ===== Access Policy Badge =====

function AccessBadge({ policy }) {
  if (!policy) {
    return <span className="px-1.5 py-0.5 text-xs bg-surface-secondary text-secondary rounded font-mono">no policy</span>
  }

  const readTier = policy.min_read_tier
  const writeTier = policy.min_write_tier

  const tierColor = (tier) => {
    if (tier === 'Owner') return 'text-gruvbox-red'
    if (tier === 'Public') return 'text-gruvbox-green'
    return 'text-gruvbox-yellow'
  }

  return (
    <span className="inline-flex items-center gap-1 text-xs font-mono">
      <span className={`px-1.5 py-0.5 rounded bg-surface-secondary ${tierColor(readTier)}`} title={`Read: ${readTier} tier`}>
        R:{readTier}
      </span>
      <span className={`px-1.5 py-0.5 rounded bg-surface-secondary ${tierColor(writeTier)}`} title={`Write: ${writeTier} tier`}>
        W:{writeTier}
      </span>
    </span>
  )
}

// ===== Field Policy Detail Panel =====

function FieldPolicyPanel({ schemaName, fieldName, policy, onClose, onUpdate }) {
  const [readTier, setReadTier] = useState(policy?.min_read_tier ?? 'Owner')
  const [writeTier, setWriteTier] = useState(policy?.min_write_tier ?? 'Owner')
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState(null)
  const [preset, setPreset] = useState('')

  const applyPreset = (name) => {
    setPreset(name)
    switch (name) {
      case 'owner-only':
        setReadTier('Owner'); setWriteTier('Owner'); break
      case 'public-read':
        setReadTier('Public'); setWriteTier('Owner'); break
      case 'trusted-read':
        setReadTier('Trusted'); setWriteTier('Owner'); break
      case 'trusted-rw':
        setReadTier('Trusted'); setWriteTier('Trusted'); break
    }
  }

  const handleSave = async () => {
    setSaving(true)
    setSaveError(null)
    try {
      await setFieldPolicyApi(schemaName, fieldName, {
        trust_domain: policy?.trust_domain ?? 'personal',
        min_read_tier: readTier,
        min_write_tier: writeTier,
        capabilities: [],
      })
      onUpdate()
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setSaveError(msg || 'Failed to save policy')
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="mt-2 p-3 bg-surface border border-border rounded-lg space-y-3">
      <div className="flex items-center justify-between">
        <h4 className="text-sm font-semibold text-primary">
          Access Policy: {schemaName}.{fieldName}
        </h4>
        <button onClick={onClose} className="text-xs text-secondary hover:text-primary">Close</button>
      </div>

      {/* Presets */}
      <div className="flex gap-2">
        {[
          ['owner-only', 'Owner Only'],
          ['public-read', 'Public Read'],
          ['trusted-read', 'Trusted Read'],
          ['trusted-rw', 'Trusted R+W'],
        ].map(([key, label]) => (
          <button
            key={key}
            onClick={() => applyPreset(key)}
            className={`px-2 py-1 text-xs rounded border ${
              preset === key ? 'border-accent text-accent' : 'border-border text-secondary hover:text-primary'
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {/* Manual controls */}
      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className="text-xs text-secondary block mb-1">Min Read Tier</label>
          <select
            value={readTier}
            onChange={(e) => setReadTier(e.target.value)}
            className="w-full bg-gruvbox-elevated border border-border rounded px-2 py-1 text-sm text-primary"
          >
            {TRUST_TIERS.map(t => <option key={t} value={t}>{t}</option>)}
          </select>
          <p className="text-xs text-secondary mt-0.5">Owner = only you</p>
        </div>
        <div>
          <label className="text-xs text-secondary block mb-1">Min Write Tier</label>
          <select
            value={writeTier}
            onChange={(e) => setWriteTier(e.target.value)}
            className="w-full bg-gruvbox-elevated border border-border rounded px-2 py-1 text-sm text-primary"
          >
            {TRUST_TIERS.map(t => <option key={t} value={t}>{t}</option>)}
          </select>
          <p className="text-xs text-secondary mt-0.5">Owner = only you</p>
        </div>
      </div>

      {/* Capability info */}
      {policy?.capabilities?.length > 0 && (
        <p className="text-xs text-secondary">
          {policy.capabilities.length} capability token(s) attached
        </p>
      )}

      {saveError && (
        <p className="text-xs text-gruvbox-red">{saveError}</p>
      )}

      <button
        onClick={handleSave}
        disabled={saving}
        className="px-4 py-1.5 bg-accent text-surface rounded text-sm font-medium disabled:opacity-50 hover:bg-accent/80"
      >
        {saving ? 'Saving...' : 'Save Policy'}
      </button>
    </div>
  )
}

// ===== Main SchemaTab =====

function SchemaTab({ onResult, onSchemaUpdated }) {
  const highlightTimerRef = useRef(null)
  const dispatch = useAppDispatch()
  const schemas = useAppSelector(selectAllSchemas)
  const [expandedSchemas, setExpandedSchemas] = useState({})
  const [highlightedSchema, setHighlightedSchema] = useState(null)
  // Per-schema field policies: { schemaName: { fieldName: policy | null } }
  const [fieldPolicies, setFieldPolicies] = useState({})
  // Which field's detail panel is open: "schemaName.fieldName" or null
  const [activePolicyField, setActivePolicyField] = useState(null)
  // Which row's "⋯" action menu is open (one at a time across the page).
  const [openMenuFor, setOpenMenuFor] = useState(null)
  const orgNames = useOrgNames()

  // Close the row-action menu on outside click / Escape. Single global
  // listener while a menu is open — avoids per-row listener churn.
  useEffect(() => {
    if (!openMenuFor) return
    const onDocClick = (e) => {
      if (!e.target.closest?.('[data-row-menu-root]')) setOpenMenuFor(null)
    }
    const onKey = (e) => { if (e.key === 'Escape') setOpenMenuFor(null) }
    document.addEventListener('mousedown', onDocClick)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDocClick)
      document.removeEventListener('keydown', onKey)
    }
  }, [openMenuFor])

  useEffect(() => {
    dispatch(fetchSchemas({ forceRefresh: true }))
    return () => { if (highlightTimerRef.current) clearTimeout(highlightTimerRef.current) }
  }, [dispatch])

  const loadPolicies = useCallback(async (schemaName) => {
    try {
      const policies = await getAllFieldPolicies(schemaName)
      setFieldPolicies(prev => ({ ...prev, [schemaName]: policies }))
    } catch {
      // Non-critical — just means badges won't show
    }
  }, [])

  const toggleSchema = async (schemaName) => {
    const isCurrentlyExpanded = expandedSchemas[schemaName]

    setExpandedSchemas(prev => ({
      ...prev,
      [schemaName]: !prev[schemaName]
    }))

    if (!isCurrentlyExpanded) {
      const schema = schemas.find(s => s.name === schemaName)
      if (schema && (!schema.fields || Object.keys(schema.fields).length === 0)) {
        dispatch(fetchSchemas({ forceRefresh: true }))
        if (onSchemaUpdated) onSchemaUpdated()
      }
      // Load field policies when expanding
      loadPolicies(schemaName)
    }
  }

  const getStateColor = (state) => {
    const key = state?.toLowerCase()
    const colors = {
      approved: 'badge badge-success',
      pending: 'badge badge-warning',
      blocked: 'badge badge-error'
    }
    return colors[key] || 'bg-surface-secondary text-secondary border border-border'
  }

  const approveSchema = async (schemaName) => {
    try {
      const result = await dispatch(approveSchemaAction({ schemaName }))
      if (approveSchemaAction.fulfilled.match(result)) {
        const backfillHash = result.payload?.backfillHash
        await dispatch(fetchSchemas({ forceRefresh: true }))
        if (onResult) {
          const message = backfillHash
            ? `Schema ${schemaName} approved successfully. Backfill started with hash: ${backfillHash}`
            : `Schema ${schemaName} approved successfully`
          onResult({ success: true, message, backfillHash })
        }
        if (onSchemaUpdated) onSchemaUpdated()
      } else {
        const errorMessage = typeof result.payload === 'string'
          ? result.payload
          : result.payload?.error || `Failed to approve schema: ${schemaName}`
        throw new Error(errorMessage)
      }
    } catch (err) {
      console.error('Failed to approve schema:', err)
      if (onResult) onResult({ error: `Failed to approve schema: ${toErrorMessage(err)}` })
    }
  }

  const blockSchema = async (schemaName) => {
    try {
      const result = await dispatch(blockSchemaAction({ schemaName }))
      if (blockSchemaAction.fulfilled.match(result)) {
        await dispatch(fetchSchemas({ forceRefresh: true }))
        if (onResult) onResult({ success: true, message: `Schema ${schemaName} blocked successfully` })
        if (onSchemaUpdated) onSchemaUpdated()
      } else {
        const errorMessage = typeof result.payload === 'string'
          ? result.payload
          : result.payload?.error || `Failed to block schema: ${schemaName}`
        throw new Error(errorMessage)
      }
    } catch (err) {
      console.error('Failed to block schema:', err)
      if (onResult) onResult({ error: `Failed to block schema: ${toErrorMessage(err)}` })
    }
  }

  const scrollToSchema = (schemaName) => {
    setExpandedSchemas(prev => ({ ...prev, [schemaName]: true }))
    setHighlightedSchema(schemaName)
    window.requestAnimationFrame(() => {
      const el = document.getElementById(`schema-${schemaName}`)
      if (el) el.scrollIntoView({ behavior: 'smooth', block: 'start' })
    })
    if (highlightTimerRef.current) clearTimeout(highlightTimerRef.current)
    highlightTimerRef.current = setTimeout(() => {
      setHighlightedSchema(null)
      highlightTimerRef.current = null
    }, 2000)
  }

  const renderSchema = (schema) => {
    const isExpanded = expandedSchemas[schema.name]
    const state = schema.state || 'Unknown'
    const rangeSchemaInfo = schema.fields ? getRangeSchemaInfo(schema) : null
    const hashRangeSchemaInfo = getHashRangeSchemaInfo(schema)
    const schemaPolicies = fieldPolicies[schema.name] || {}

    const isApproved = state.toLowerCase() === 'approved'
    const isBlocked = state.toLowerCase() === 'blocked'
    const menuOpen = openMenuFor === schema.name

    return (
      <div
        key={schema.name}
        id={`schema-${schema.name}`}
        className={`border-b border-border last:border-b-0 transition-shadow duration-500${highlightedSchema === schema.name ? ' ring-2 ring-gruvbox-purple' : ''}`}
      >
        <div
          role="button"
          tabIndex={0}
          className="w-full px-4 py-2 hover:bg-surface-secondary cursor-pointer select-none text-left"
          onClick={() => toggleSchema(schema.name)}
          onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggleSchema(schema.name) } }}
          aria-expanded={isExpanded}
          aria-label={`${isExpanded ? 'Collapse' : 'Expand'} schema ${schema.descriptive_name || schema.name}`}
        >
          <div className="flex items-center justify-between gap-2">
            <div className="flex items-center gap-2 min-w-0">
              {isExpanded ? (
                <ChevronDownIcon className="w-4 h-4 text-primary shrink-0" />
              ) : (
                <ChevronRightIcon className="w-4 h-4 text-primary shrink-0" />
              )}
              <h3 className="font-medium text-primary truncate">
                <SchemaName schema={schema} className="font-medium text-primary" />
              </h3>
              <span className={`px-2 py-0.5 text-xs font-medium rounded-full shrink-0 ${getStateColor(state)}`}>
                {state}
              </span>
              {schema.org_hash && (
                <span className="badge badge-info shrink-0" title={schema.org_hash}>
                  {orgNames[schema.org_hash] || 'Org'}
                </span>
              )}
              {rangeSchemaInfo && (
                <span className="badge badge-info shrink-0">Range Schema</span>
              )}
              {hashRangeSchemaInfo && (
                <span className="badge badge-info shrink-0">HashRange Schema</span>
              )}
            </div>
            <div className="flex items-center gap-1 shrink-0">
              {isApproved && (
                <button
                  className="btn-secondary btn-sm flex items-center gap-1"
                  onClick={(e) => {
                    e.stopPropagation();
                    window.location.hash = `query?schema=${encodeURIComponent(schema.name)}`;
                  }}
                  title="Query this schema"
                >
                  <MagnifyingGlassIcon className="w-3.5 h-3.5" />
                  Query
                </button>
              )}
              {(isApproved || isBlocked) && (
                <div className="relative" data-row-menu-root>
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation()
                      setOpenMenuFor(menuOpen ? null : schema.name)
                    }}
                    aria-label={`More actions for ${schema.descriptive_name || schema.name}`}
                    aria-haspopup="menu"
                    aria-expanded={menuOpen}
                    className="btn-secondary btn-sm px-2 flex items-center justify-center"
                  >
                    <EllipsisHorizontalIcon className="w-4 h-4" />
                  </button>
                  {menuOpen && (
                    <div
                      role="menu"
                      className="absolute right-0 top-full mt-1 z-30 min-w-[140px] bg-gruvbox-elevated border border-border rounded shadow-lg py-1"
                    >
                      {isApproved && (
                        <button
                          role="menuitem"
                          onClick={(e) => { e.stopPropagation(); setOpenMenuFor(null); blockSchema(schema.name) }}
                          className="w-full text-left px-3 py-1.5 text-sm text-gruvbox-red hover:bg-surface-secondary bg-transparent border-none cursor-pointer"
                        >
                          Block schema
                        </button>
                      )}
                      {isBlocked && (
                        <button
                          role="menuitem"
                          onClick={(e) => { e.stopPropagation(); setOpenMenuFor(null); approveSchema(schema.name) }}
                          className="w-full text-left px-3 py-1.5 text-sm text-primary hover:bg-surface-secondary bg-transparent border-none cursor-pointer"
                        >
                          Re-approve
                        </button>
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>
          </div>
        </div>

        {isExpanded && schema.fields && (
          <div className="p-4 border-t border-border">
            {/* Range Schema Information */}
            {rangeSchemaInfo && (
              <div className="card card-info mb-4 p-3">
                <h4 className="text-sm font-medium text-gruvbox-blue mb-2">Range Schema Information</h4>
                <div className="space-y-1 text-xs text-gruvbox-blue">
                  <p><strong>Range Key:</strong> {rangeSchemaInfo.rangeKey}</p>
                  <p><strong>Total Fields:</strong> {rangeSchemaInfo.totalFields}</p>
                  <p><strong>Range Fields:</strong> {rangeSchemaInfo.rangeFields.length}</p>
                  <p className="opacity-70">This schema uses range-based storage for efficient querying and mutations.</p>
                </div>
              </div>
            )}

            {/* HashRange Schema Information */}
            {hashRangeSchemaInfo && (
              <div className="card card-info mb-4 p-3">
                <h4 className="text-sm font-medium text-gruvbox-blue mb-2">HashRange Schema Information</h4>
                <div className="space-y-1 text-xs text-gruvbox-blue">
                  <p><strong>Hash Field:</strong> {hashRangeSchemaInfo.hashField}</p>
                  <p><strong>Range Field:</strong> {hashRangeSchemaInfo.rangeField}</p>
                  <p><strong>Total Fields:</strong> {hashRangeSchemaInfo.totalFields}</p>
                  <p className="opacity-70">This schema uses hash-range-based storage for efficient querying and mutations with both hash and range keys.</p>
                </div>
              </div>
            )}

            <div className="space-y-3">
              {Array.isArray(schema.fields) ? (
                schema.fields.map(fieldName => {
                  const classifications = schema.field_classifications?.[fieldName]
                  const refSchema = schema.ref_fields?.[fieldName]
                  const policy = schemaPolicies[fieldName]
                  const policyKey = `${schema.name}.${fieldName}`
                  const isPolicyOpen = activePolicyField === policyKey

                  return (
                    <div key={fieldName} className="card p-3">
                      <div className="flex items-center justify-between">
                        <div className="flex-1">
                          <div className="flex items-center space-x-2 flex-wrap gap-y-1">
                            <span className="font-medium text-primary">{fieldName}</span>
                            {rangeSchemaInfo?.rangeKey === fieldName && (
                              <span className="badge badge-info">Range Key</span>
                            )}
                            {hashRangeSchemaInfo?.hashField === fieldName && (
                              <span className="badge badge-info">Hash Key</span>
                            )}
                            {hashRangeSchemaInfo?.rangeField === fieldName && (
                              <span className="badge badge-info">Range Key</span>
                            )}
                            {classifications && classifications.length > 0 && (
                              <span className="flex space-x-1">
                                {classifications.map(cls => (
                                  <span key={cls} className="px-1.5 py-0.5 text-xs bg-surface-secondary text-primary rounded-full font-mono">
                                    {cls}
                                  </span>
                                ))}
                              </span>
                            )}
                            {refSchema && (
                              <button
                                className="font-mono text-xs text-gruvbox-purple hover:text-gruvbox-yellow underline decoration-dotted cursor-pointer bg-transparent border-none p-0"
                                title={refSchema}
                                onClick={() => scrollToSchema(refSchema)}
                              >
                                Ref&lt;{refSchema.length > 16 ? refSchema.slice(0, 12) + '...' : refSchema}&gt;
                              </button>
                            )}
                          </div>
                        </div>
                        {/* Access policy badge + drill-in button */}
                        <button
                          onClick={() => setActivePolicyField(isPolicyOpen ? null : policyKey)}
                          className="flex items-center gap-1 hover:opacity-80 transition-opacity"
                          title="Click to edit access policy"
                        >
                          <AccessBadge policy={policy} />
                        </button>
                      </div>

                      {/* Drill-in policy editor */}
                      {isPolicyOpen && (
                        <FieldPolicyPanel
                          schemaName={schema.name}
                          fieldName={fieldName}
                          policy={policy}
                          onClose={() => setActivePolicyField(null)}
                          onUpdate={() => {
                            loadPolicies(schema.name)
                            setActivePolicyField(null)
                          }}
                        />
                      )}
                    </div>
                  )
                })
              ) : (
                <p className="text-sm text-secondary italic">No fields defined</p>
              )}
            </div>
          </div>
        )}
      </div>
    )
  }

  const [schemaFilter, setSchemaFilter] = useState('all') // 'all' | 'personal' | 'org'
  const [stateFilter, setStateFilter] = useState('all') // 'all' | 'approved' | 'blocked'
  // null = use default (expanded if any system schema holds data); boolean = user override.
  const [systemSectionOverride, setSystemSectionOverride] = useState(null)
  // Per-system-schema key counts, used to guarantee we never hide a data-bearing schema.
  const [systemKeyCounts, setSystemKeyCounts] = useState({})

  // Only show local schemas (approved or blocked) — never show "available" (global catalog)
  const localSchemas = schemas.filter(s => {
    const state = (s.state || '').toLowerCase()
    return state === 'approved' || state === 'blocked'
  })

  const filteredSchemas = localSchemas.filter(s => {
    // Owner filter
    if (schemaFilter === 'personal' && s.org_hash) return false
    if (schemaFilter === 'org' && !s.org_hash) return false
    // State filter
    if (stateFilter !== 'all') {
      const normalizedState = (s.state || '').toLowerCase()
      if (normalizedState !== stateFilter) return false
    }
    return true
  })

  // Partition into user vs system schemas. Backend `system: bool` wins when
  // present; otherwise isSystemSchema falls back to a known-name allow-list.
  const userSchemas = []
  const systemSchemas = []
  for (const s of filteredSchemas) (isSystemSchema(s) ? systemSchemas : userSchemas).push(s)

  // Stable key for the useEffect dependency so it doesn't re-run every render.
  const systemSchemaNamesKey = systemSchemas.map(s => s.name).sort().join('|')

  // Fetch a 1-row sample per system schema to decide whether it's data-bearing.
  // Cheap: each call returns total_count without streaming actual rows.
  useEffect(() => {
    let cancelled = false
    for (const s of systemSchemas) {
      if (systemKeyCounts[s.name] !== undefined) continue
      schemaClient.listSchemaKeys(s.name, 0, 1).then(res => {
        if (cancelled || !res?.success) return
        const count = res.data?.total_count ?? 0
        setSystemKeyCounts(prev => ({ ...prev, [s.name]: count }))
      }).catch(() => {
        // Treat as unknown; don't block UI if the keys endpoint errors.
      })
    }
    return () => { cancelled = true }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [systemSchemaNamesKey])

  const anySystemHasData = systemSchemas.some(s => (systemKeyCounts[s.name] ?? 0) > 0)
  // Default: expanded iff any system schema has data (so we never silently hide
  // data). Explicit user toggle overrides.
  const systemSectionExpanded = systemSectionOverride ?? anySystemHasData

  // Count schemas by state for the filter badges
  const stateCounts = localSchemas.reduce((acc, s) => {
    const state = (s.state || '').toLowerCase()
    acc[state] = (acc[state] || 0) + 1
    return acc
  }, {})

  const renderSectionHeader = (label, count, extra = null) => (
    <div className="flex items-center gap-2 pt-2">
      <h3 className="text-sm font-semibold text-secondary uppercase tracking-wide">{label}</h3>
      <span className="text-xs text-tertiary">({count})</span>
      {extra}
    </div>
  )

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        {/* Owner filter */}
        {['all', 'personal', 'org'].map(f => (
          <button
            key={f}
            onClick={() => setSchemaFilter(f)}
            className={`px-3 py-1 text-xs rounded-full border ${
              schemaFilter === f
                ? 'border-primary text-primary bg-primary/10'
                : 'border-border text-secondary hover:text-primary'
            }`}
          >
            {f.charAt(0).toUpperCase() + f.slice(1)}
          </button>
        ))}
        <span className="text-border mx-1">|</span>
        {/* State filter */}
        {['all', 'approved', 'blocked'].map(f => {
          const count = f === 'all' ? localSchemas.length : (stateCounts[f] || 0)
          return (
            <button
              key={f}
              onClick={() => setStateFilter(f)}
              className={`px-3 py-1 text-xs rounded-full border ${
                stateFilter === f
                  ? 'border-primary text-primary bg-primary/10'
                  : 'border-border text-secondary hover:text-primary'
              }`}
            >
              {f.charAt(0).toUpperCase() + f.slice(1)} ({count})
            </button>
          )
        })}
      </div>

      {filteredSchemas.length === 0 && (
        <p className="text-secondary">No schemas match the current filters.</p>
      )}

      {userSchemas.length > 0 && (
        <div>
          {renderSectionHeader('User schemas', userSchemas.length)}
          <div className="border-t border-border">
            {userSchemas.map(renderSchema)}
          </div>
        </div>
      )}

      {systemSchemas.length > 0 && (
        <div>
          {renderSectionHeader(
            'System schemas',
            systemSchemas.length,
            <button
              onClick={() => setSystemSectionOverride(!systemSectionExpanded)}
              className="ml-auto text-xs text-secondary hover:text-primary flex items-center gap-1"
              aria-expanded={systemSectionExpanded}
              aria-controls="system-schemas-section"
            >
              {systemSectionExpanded ? (
                <>
                  <ChevronDownIcon className="w-3.5 h-3.5" />
                  Hide
                </>
              ) : (
                <>
                  <ChevronRightIcon className="w-3.5 h-3.5" />
                  Show
                  {anySystemHasData && (
                    <span className="ml-1 px-1.5 py-0.5 text-[10px] rounded bg-gruvbox-yellow/20 text-gruvbox-yellow font-medium">
                      has data
                    </span>
                  )}
                </>
              )}
            </button>,
          )}
          <p className="text-xs text-tertiary -mt-2">
            Built-in infrastructure schemas seeded by the schema service.
          </p>
          {systemSectionExpanded && (
            <div id="system-schemas-section" className="border-t border-border">
              {systemSchemas.map(renderSchema)}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

export default SchemaTab
