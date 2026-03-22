import { useState, useEffect, useCallback, useRef } from 'react'
import { ChevronDownIcon, ChevronRightIcon } from '@heroicons/react/24/solid'
import { listViews, approveView, blockView, deleteView } from '../../api/clients/viewsClient'
import { llmQueryClient } from '../../api/clients/llmQueryClient'

function ViewsTab({ onResult }) {
  const [views, setViews] = useState([])
  const [loading, setLoading] = useState(true)
  const [expandedViews, setExpandedViews] = useState({})
  const [showChat, setShowChat] = useState(false)

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
            onClick={() => setShowChat(!showChat)}
          >
            {showChat ? 'Close' : 'Create View'}
          </button>
        </div>
      </div>

      {showChat && (
        <ViewCreatorChat
          onViewCreated={() => fetchViews()}
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

function ViewCreatorChat({ onViewCreated, onResult }) {
  const [messages, setMessages] = useState([
    {
      id: 'welcome',
      role: 'assistant',
      content: "Describe the transform view you'd like to create. For example:\n\n" +
        '- "Create a view that counts words in BlogPost content"\n' +
        '- "Make a view that combines Author names with their BlogPost titles"\n' +
        '- "Create a view that extracts hashtags from Tweet content"\n\n' +
        "I'll inspect the schemas, generate the Rust WASM transform, compile it, and register the view.",
    },
  ])
  const [inputText, setInputText] = useState('')
  const [isProcessing, setIsProcessing] = useState(false)
  const [sessionId, setSessionId] = useState(null)
  const [thinkingSeconds, setThinkingSeconds] = useState(0)
  const thinkingTimerRef = useRef(null)
  const messagesEndRef = useRef(null)

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages])

  useEffect(() => {
    if (isProcessing) {
      setThinkingSeconds(0)
      thinkingTimerRef.current = setInterval(() => setThinkingSeconds(s => s + 1), 1000)
    } else {
      if (thinkingTimerRef.current) {
        clearInterval(thinkingTimerRef.current)
        thinkingTimerRef.current = null
      }
    }
    return () => {
      if (thinkingTimerRef.current) clearInterval(thinkingTimerRef.current)
    }
  }, [isProcessing])

  const handleSubmit = useCallback(async (e) => {
    e?.preventDefault()
    const text = inputText.trim()
    if (!text || isProcessing) return
    setInputText('')
    setIsProcessing(true)

    setMessages(prev => [...prev, { id: `user-${Date.now()}`, role: 'user', content: text }])

    try {
      const contextHint = 'The user wants to create a transform view. ' +
        'Use get_schema to inspect source schemas before generating the view. ' +
        'Use the create_view tool to compile and register the view. ' +
        'Every view MUST have a rust_transform — identity views are not allowed.'

      const response = await llmQueryClient.agentQuery({
        query: `[View Creator Context: ${contextHint}]\n\nUser request: ${text}`,
        session_id: sessionId,
        max_iterations: 15,
      })

      if (!response.success) {
        setMessages(prev => [...prev, {
          id: `err-${Date.now()}`,
          role: 'assistant',
          content: `Error: ${response.error || 'Agent query failed'}`,
        }])
        return
      }

      const result = response.data
      if (result.session_id) setSessionId(result.session_id)

      // Check if a view was created via tool calls
      let viewCreated = false
      if (result.tool_calls?.length > 0) {
        for (const tc of result.tool_calls) {
          if (tc.tool === 'create_view' && tc.result?.success) {
            viewCreated = true
            setMessages(prev => [...prev, {
              id: `view-${Date.now()}`,
              role: 'view_created',
              content: tc.result.view_name || 'View created',
              data: tc.result,
            }])
          }
        }
      }

      setMessages(prev => [...prev, {
        id: `assistant-${Date.now()}`,
        role: 'assistant',
        content: result.answer,
      }])

      if (viewCreated) {
        onViewCreated()
        if (onResult) onResult({ success: true, message: 'View created via AI' })
      }
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error)
      setMessages(prev => [...prev, {
        id: `err-${Date.now()}`,
        role: 'assistant',
        content: `Error: ${msg}`,
      }])
    } finally {
      setIsProcessing(false)
    }
  }, [inputText, isProcessing, sessionId, onViewCreated, onResult])

  return (
    <div className="card overflow-hidden flex flex-col" style={{ height: '400px' }}>
      <div className="px-4 py-2 bg-surface-secondary border-b border-border flex items-center justify-between">
        <span className="text-xs font-medium text-primary">View Creator (AI)</span>
        <button
          onClick={() => {
            setMessages([messages[0]])
            setSessionId(null)
          }}
          className="text-xs text-gruvbox-blue hover:underline bg-transparent border-none cursor-pointer"
        >
          New Conversation
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {messages.map((msg) => (
          <div key={msg.id}>
            {msg.role === 'user' && (
              <div className="flex justify-end">
                <div className="max-w-[80%] px-3 py-2 bg-gruvbox-elevated border border-gruvbox-orange rounded-lg">
                  <p className="text-xs opacity-70 mb-1">You</p>
                  <p className="text-sm text-primary whitespace-pre-wrap">{msg.content}</p>
                </div>
              </div>
            )}
            {msg.role === 'assistant' && (
              <div className="flex justify-start">
                <div className="max-w-[80%] px-3 py-2 bg-surface-secondary border border-border rounded-lg">
                  <p className="text-xs text-tertiary mb-1">Assistant</p>
                  <p className="text-sm text-primary whitespace-pre-wrap">{msg.content}</p>
                </div>
              </div>
            )}
            {msg.role === 'view_created' && (
              <div className="flex justify-start">
                <div className="max-w-[80%] px-3 py-2 bg-surface-secondary border border-gruvbox-green rounded-lg">
                  <p className="text-xs text-gruvbox-green mb-1">View Created</p>
                  <p className="text-sm text-primary font-mono">{msg.content}</p>
                </div>
              </div>
            )}
          </div>
        ))}

        {isProcessing && (
          <div className="flex justify-start">
            <div className="px-3 py-2 bg-surface-secondary border border-border rounded-lg">
              <p className="text-sm text-tertiary">
                {thinkingSeconds < 10
                  ? 'Thinking...'
                  : thinkingSeconds < 30
                    ? 'Generating transform...'
                    : `Compiling WASM (${thinkingSeconds}s)...`}
              </p>
            </div>
          </div>
        )}
        <div ref={messagesEndRef} />
      </div>

      <form onSubmit={handleSubmit} className="px-4 py-3 border-t border-border bg-surface">
        <div className="flex gap-2">
          <input
            type="text"
            value={inputText}
            onChange={(e) => setInputText(e.target.value)}
            placeholder="Describe the view you want to create..."
            disabled={isProcessing}
            className="input flex-1"
            autoFocus
          />
          <button
            type="submit"
            disabled={!inputText.trim() || isProcessing}
            className="btn-primary"
          >
            {isProcessing ? 'Working...' : 'Send'}
          </button>
        </div>
      </form>
    </div>
  )
}

export default ViewsTab
