import { useState, useEffect, useCallback } from 'react'
import { ChevronDownIcon, ChevronRightIcon } from '@heroicons/react/24/solid'
import { listViews, approveView, blockView, deleteView, createView } from '../../api/clients/viewsClient'

function ViewsTab({ onResult }) {
  const [views, setViews] = useState([])
  const [loading, setLoading] = useState(true)
  const [expandedViews, setExpandedViews] = useState({})
  const [showCreateForm, setShowCreateForm] = useState(false)

  const fetchViews = useCallback(async () => {
    try {
      setLoading(true)
      const data = await listViews()
      setViews(data)
    } catch (err) {
      if (onResult) onResult({ error: `Failed to load views: ${err.message}` })
    } finally {
      setLoading(false)
    }
  }, [onResult])

  useEffect(() => {
    fetchViews()
  }, [fetchViews])

  const toggleView = (name) => {
    setExpandedViews(prev => ({ ...prev, [name]: !prev[name] }))
  }

  const handleApprove = async (name) => {
    try {
      await approveView(name)
      if (onResult) onResult({ success: true, message: `View '${name}' approved` })
      await fetchViews()
    } catch (err) {
      if (onResult) onResult({ error: `Failed to approve view: ${err.message}` })
    }
  }

  const handleBlock = async (name) => {
    try {
      await blockView(name)
      if (onResult) onResult({ success: true, message: `View '${name}' blocked` })
      await fetchViews()
    } catch (err) {
      if (onResult) onResult({ error: `Failed to block view: ${err.message}` })
    }
  }

  const handleDelete = async (name) => {
    try {
      await deleteView(name)
      if (onResult) onResult({ success: true, message: `View '${name}' deleted` })
      setExpandedViews(prev => { const next = { ...prev }; delete next[name]; return next })
      await fetchViews()
    } catch (err) {
      if (onResult) onResult({ error: `Failed to delete view: ${err.message}` })
    }
  }

  const getStateColor = (state) => {
    const key = state?.toLowerCase()
    const colors = {
      approved: 'badge badge-success',
      available: 'badge badge-info',
      blocked: 'badge badge-error',
    }
    return colors[key] || 'badge'
  }

  const renderView = ([view, state]) => {
    const isExpanded = expandedViews[view.name]
    const isIdentity = !view.wasm_transform || view.wasm_transform.length === 0
    const sourceSchemas = [...new Set(view.input_queries.map(q => q.schema_name))]

    return (
      <div key={view.name} className="card overflow-hidden">
        <button
          type="button"
          className="w-full px-4 py-3 bg-surface-secondary cursor-pointer select-none text-left"
          onClick={() => toggleView(view.name)}
        >
          <div className="flex items-center justify-between">
            <div className="flex items-center space-x-2">
              {isExpanded ? (
                <ChevronDownIcon className="w-4 h-4 text-tertiary" />
              ) : (
                <ChevronRightIcon className="w-4 h-4 text-tertiary" />
              )}
              <h3 className="font-medium text-primary">{view.name}</h3>
              <span className={getStateColor(state)}>{state}</span>
              <span className="badge">{isIdentity ? 'Identity' : 'WASM'}</span>
              <span className="text-xs text-tertiary">{view.schema_type}</span>
            </div>
            <div className="flex items-center space-x-2">
              {state?.toLowerCase() === 'available' && (
                <button
                  className="btn-secondary btn-sm"
                  onClick={(e) => { e.stopPropagation(); handleApprove(view.name) }}
                >
                  Approve
                </button>
              )}
              {state?.toLowerCase() === 'approved' && (
                <button
                  className="btn-secondary btn-sm hover:border-gruvbox-red hover:text-gruvbox-red"
                  onClick={(e) => { e.stopPropagation(); handleBlock(view.name) }}
                >
                  Block
                </button>
              )}
              {state?.toLowerCase() === 'blocked' && (
                <button
                  className="btn-secondary btn-sm"
                  onClick={(e) => { e.stopPropagation(); handleApprove(view.name) }}
                >
                  Re-approve
                </button>
              )}
              <button
                className="btn-secondary btn-sm hover:border-gruvbox-red hover:text-gruvbox-red"
                onClick={(e) => { e.stopPropagation(); handleDelete(view.name) }}
              >
                Delete
              </button>
            </div>
          </div>
        </button>

        {isExpanded && (
          <div className="p-4 border-t border-border space-y-3">
            <div className="card card-info p-3">
              <h4 className="text-sm font-medium text-gruvbox-blue mb-2">Source Schemas</h4>
              <div className="flex flex-wrap gap-2">
                {sourceSchemas.map(s => (
                  <span key={s} className="badge badge-info">{s}</span>
                ))}
              </div>
            </div>

            <div>
              <h4 className="text-sm font-medium text-primary mb-2">Input Queries</h4>
              {view.input_queries.map((q, i) => (
                <div key={i} className="card p-3 mb-2">
                  <span className="font-mono text-xs text-primary">{q.schema_name}</span>
                  <span className="text-tertiary text-xs ml-2">
                    [{q.fields.join(', ')}]
                  </span>
                </div>
              ))}
            </div>

            <div>
              <h4 className="text-sm font-medium text-primary mb-2">Output Fields</h4>
              <div className="space-y-1">
                {Object.entries(view.output_fields).map(([name, type]) => (
                  <div key={name} className="card p-2 flex items-center justify-between">
                    <span className="font-mono text-xs text-primary">{name}</span>
                    <span className="text-xs text-tertiary font-mono">
                      {typeof type === 'string' ? type : JSON.stringify(type)}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}
      </div>
    )
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium text-primary">Transform Views</h2>
        <div className="flex space-x-2">
          <button className="btn-secondary btn-sm" onClick={fetchViews}>
            Refresh
          </button>
          <button
            className="btn-primary btn-sm"
            onClick={() => setShowCreateForm(!showCreateForm)}
          >
            {showCreateForm ? 'Cancel' : 'Create View'}
          </button>
        </div>
      </div>

      {showCreateForm && (
        <CreateViewForm
          onCreated={() => { setShowCreateForm(false); fetchViews() }}
          onResult={onResult}
        />
      )}

      {loading ? (
        <p className="text-secondary">Loading views...</p>
      ) : views.length > 0 ? (
        views.map(renderView)
      ) : (
        <p className="text-secondary">No views registered.</p>
      )}
    </div>
  )
}

function CreateViewForm({ onCreated, onResult }) {
  const [name, setName] = useState('')
  const [schemaType, setSchemaType] = useState('Single')
  const [queriesJson, setQueriesJson] = useState('[\n  { "schema_name": "", "fields": [] }\n]')
  const [outputFieldsJson, setOutputFieldsJson] = useState('{\n  "field_name": "Any"\n}')
  const [submitting, setSubmitting] = useState(false)

  const handleSubmit = async (e) => {
    e.preventDefault()
    try {
      setSubmitting(true)
      const input_queries = JSON.parse(queriesJson)
      const output_fields = JSON.parse(outputFieldsJson)

      await createView({
        name,
        schema_type: schemaType,
        input_queries,
        output_fields,
      })

      if (onResult) onResult({ success: true, message: `View '${name}' created` })
      onCreated()
    } catch (err) {
      if (onResult) onResult({ error: `Failed to create view: ${err.message}` })
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <form onSubmit={handleSubmit} className="card p-4 space-y-3">
      <h3 className="font-medium text-primary">Create Identity View</h3>

      <div>
        <label className="label">Name</label>
        <input
          className="input w-full"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="MyView"
          required
        />
      </div>

      <div>
        <label className="label">Schema Type</label>
        <select
          className="select w-full"
          value={schemaType}
          onChange={(e) => setSchemaType(e.target.value)}
        >
          <option value="Single">Single</option>
          <option value="Range">Range</option>
          <option value="Hash">Hash</option>
          <option value="HashRange">HashRange</option>
        </select>
      </div>

      <div>
        <label className="label">Input Queries (JSON)</label>
        <textarea
          className="textarea w-full font-mono text-xs"
          rows={4}
          value={queriesJson}
          onChange={(e) => setQueriesJson(e.target.value)}
        />
      </div>

      <div>
        <label className="label">Output Fields (JSON)</label>
        <textarea
          className="textarea w-full font-mono text-xs"
          rows={3}
          value={outputFieldsJson}
          onChange={(e) => setOutputFieldsJson(e.target.value)}
        />
      </div>

      <button
        type="submit"
        className="btn-primary"
        disabled={submitting || !name}
      >
        {submitting ? 'Creating...' : 'Create View'}
      </button>
    </form>
  )
}

export default ViewsTab
