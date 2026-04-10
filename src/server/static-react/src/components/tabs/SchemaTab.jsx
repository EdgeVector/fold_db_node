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
import { MagnifyingGlassIcon } from '@heroicons/react/24/outline'
import { toErrorMessage } from '../../utils/schemaUtils'
import { getAllFieldPolicies, setFieldPolicy as setFieldPolicyApi } from '../../api/clients/sharingClient'

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
    try {
      await setFieldPolicyApi(schemaName, fieldName, {
        trust_domain: policy?.trust_domain ?? 'personal',
        min_read_tier: readTier,
        min_write_tier: writeTier,
        capabilities: [],
      })
      onUpdate()
    } catch (err) {
      console.error('Failed to save field policy:', err)
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="mt-2 p-3 bg-surface-primary border border-border rounded-lg space-y-3">
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

      <button
        onClick={handleSave}
        disabled={saving}
        className="px-4 py-1.5 bg-accent text-surface-primary rounded text-sm font-medium disabled:opacity-50 hover:bg-accent/80"
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

    return (
      <div key={schema.name} id={`schema-${schema.name}`} className={`card overflow-hidden transition-shadow duration-500${highlightedSchema === schema.name ? ' ring-2 ring-gruvbox-purple' : ''}`}>
        <div
          role="button"
          tabIndex={0}
          className="w-full px-4 py-3 bg-surface-secondary cursor-pointer select-none text-left"
          onClick={() => toggleSchema(schema.name)}
          onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggleSchema(schema.name) } }}
          aria-expanded={isExpanded}
          aria-label={`${isExpanded ? 'Collapse' : 'Expand'} schema ${schema.descriptive_name || schema.name}`}
        >
          <div className="flex items-center justify-between">
            <div className="flex items-center space-x-2">
              {isExpanded ? (
                <ChevronDownIcon className="w-4 h-4 text-tertiary transition-transform duration-200" />
              ) : (
                <ChevronRightIcon className="w-4 h-4 text-tertiary transition-transform duration-200" />
              )}
              <h3 className="font-medium text-primary">
                <SchemaName schema={schema} className="font-medium text-primary" />
              </h3>
              <span className={`px-2 py-1 text-xs font-medium rounded-full ${getStateColor(state)}`}>
                {state}
              </span>
              {schema.org_hash && (
                <span className="badge badge-info">Org</span>
              )}
              {rangeSchemaInfo && (
                <span className="badge badge-info">Range Schema</span>
              )}
              {hashRangeSchemaInfo && (
                <span className="badge badge-info">HashRange Schema</span>
              )}
            </div>
            <div className="flex items-center space-x-2">
              {state.toLowerCase() === 'approved' && (
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
              {state.toLowerCase() === 'approved' && (
                <button
                  className="btn-secondary btn-sm hover:border-gruvbox-red hover:text-gruvbox-red"
                  onClick={(e) => { e.stopPropagation(); blockSchema(schema.name) }}
                >
                  Block
                </button>
              )}
              {state.toLowerCase() === 'blocked' && (
                <button
                  className="btn-secondary btn-sm"
                  onClick={(e) => { e.stopPropagation(); approveSchema(schema.name) }}
                >
                  Re-approve
                </button>
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

  // Count schemas by state for the filter badges
  const stateCounts = localSchemas.reduce((acc, s) => {
    const state = (s.state || '').toLowerCase()
    acc[state] = (acc[state] || 0) + 1
    return acc
  }, {})

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
      {filteredSchemas.length > 0 ? (
        filteredSchemas.map(renderSchema)
      ) : (
        <p className="text-secondary">No schemas match the current filters.</p>
      )}
    </div>
  )
}

export default SchemaTab
