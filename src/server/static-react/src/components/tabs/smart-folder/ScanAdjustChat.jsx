import { useState, useRef, useEffect, useCallback } from 'react'
import { ingestionClient } from '../../../api/clients'
import { toErrorMessage } from '../../../utils/schemaUtils'

/**
 * AI chat panel for adjusting scan results via natural language.
 *
 * @param {Object} props
 * @param {Object} props.scanResult - Current scan result with recommended_files and skipped_files
 * @param {Function} props.onScanResultUpdate - Called with the updated scanResult when the AI adjusts files
 */
export default function ScanAdjustChat({ scanResult, onScanResultUpdate }) {
  const [messages, setMessages] = useState([])
  const [input, setInput] = useState('')
  const [isLoading, setIsLoading] = useState(false)
  const messagesEndRef = useRef(null)
  const inputRef = useRef(null)
  const hasGeneratedSummary = useRef(false)

  // Auto-scroll to bottom on new messages
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages])

  // Generate initial summary when scan result first arrives
  useEffect(() => {
    if (!scanResult || hasGeneratedSummary.current) return
    hasGeneratedSummary.current = true

    const rec = scanResult.recommended_files
    const skip = scanResult.skipped_files.filter(f => !f.already_ingested)
    const already = scanResult.skipped_files.filter(f => f.already_ingested)

    // Build category breakdown
    const categories = {}
    for (const f of rec) {
      categories[f.category] = (categories[f.category] || 0) + 1
    }

    let summary = `Found **${rec.length} files** to ingest`
    if (skip.length > 0) summary += `, **${skip.length} skipped**`
    if (already.length > 0) summary += `, **${already.length} already ingested**`
    summary += '.\n\n'

    if (Object.keys(categories).length > 0) {
      summary += '**Breakdown:**\n'
      const sorted = Object.entries(categories).sort((a, b) => b[1] - a[1])
      for (const [cat, count] of sorted) {
        const label = cat.replace(/_/g, ' ')
        summary += `- ${label}: ${count}\n`
      }
    }

    // Note skipped categories
    if (skip.length > 0) {
      const skipCats = {}
      for (const f of skip) {
        skipCats[f.category] = (skipCats[f.category] || 0) + 1
      }
      summary += '\n**Skipped:**\n'
      for (const [cat, count] of Object.entries(skipCats).sort((a, b) => b[1] - a[1])) {
        summary += `- ${cat.replace(/_/g, ' ')}: ${count}\n`
      }
    }

    summary += '\nTell me how to adjust — e.g. *"include all work files"* or *"skip the images"*.'

    setMessages([{ role: 'assistant', content: summary }])
  }, [scanResult])

  const handleSubmit = useCallback(async (e) => {
    e.preventDefault()
    const instruction = input.trim()
    if (!instruction || isLoading || !scanResult) return

    setInput('')
    setMessages(prev => [...prev, { role: 'user', content: instruction }])
    setIsLoading(true)

    try {
      const response = await ingestionClient.adjustScanResults(
        instruction,
        scanResult.recommended_files,
        scanResult.skipped_files,
      )

      if (response.success && response.data?.success) {
        const { recommended_files, skipped_files, summary, total_estimated_cost, message } = response.data

        // Build a change description
        const oldRec = scanResult.recommended_files.length
        const newRec = recommended_files.length
        const diff = newRec - oldRec
        let changeMsg = message
        if (diff > 0) {
          changeMsg += ` (+${diff} added)`
        } else if (diff < 0) {
          changeMsg += ` (${diff} removed)`
        }

        setMessages(prev => [...prev, { role: 'assistant', content: changeMsg }])

        // Update the parent's scan result
        onScanResultUpdate({
          ...scanResult,
          recommended_files,
          skipped_files,
          summary,
          total_estimated_cost,
        })
      } else {
        const errMsg = response.data?.message || 'Failed to adjust files'
        setMessages(prev => [...prev, { role: 'assistant', content: `Error: ${errMsg}` }])
      }
    } catch (error) {
      const errMsg = toErrorMessage(error) || 'Failed to communicate with AI'
      setMessages(prev => [...prev, { role: 'assistant', content: `Error: ${errMsg}` }])
    } finally {
      setIsLoading(false)
      inputRef.current?.focus()
    }
  }, [input, isLoading, scanResult, onScanResultUpdate])

  return (
    <div className="flex flex-col h-full border border-border rounded-lg overflow-hidden">
      {/* Messages area */}
      <div className="flex-1 overflow-y-auto p-3 space-y-3 min-h-0">
        {messages.map((msg, i) => (
          <div key={i} className={`text-sm ${msg.role === 'user' ? 'text-right' : ''}`}>
            <div
              className={`inline-block max-w-full text-left rounded-lg px-3 py-2 ${
                msg.role === 'user'
                  ? 'bg-gruvbox-blue/20 text-primary'
                  : 'bg-surface-secondary text-secondary'
              }`}
            >
              <MessageContent content={msg.content} />
            </div>
          </div>
        ))}
        {isLoading && (
          <div className="text-sm">
            <div className="inline-block bg-surface-secondary text-secondary rounded-lg px-3 py-2">
              <span className="spinner inline-block mr-1" /> Thinking...
            </div>
          </div>
        )}
        <div ref={messagesEndRef} />
      </div>

      {/* Input area */}
      <form onSubmit={handleSubmit} className="border-t border-border p-2 flex gap-2">
        <input
          ref={inputRef}
          type="text"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder='e.g. "include all work files"'
          className="input flex-1 text-sm"
          disabled={isLoading}
        />
        <button
          type="submit"
          disabled={isLoading || !input.trim()}
          className="btn-primary text-sm px-3"
        >
          Send
        </button>
      </form>
    </div>
  )
}

/** Render markdown-lite content (bold, italic, line breaks) */
function MessageContent({ content }) {
  // Split into lines and render with basic formatting
  const lines = content.split('\n')
  return (
    <div className="whitespace-pre-wrap">
      {lines.map((line, i) => (
        <span key={i}>
          {i > 0 && <br />}
          {renderInlineFormatting(line)}
        </span>
      ))}
    </div>
  )
}

function renderInlineFormatting(text) {
  // Bold: **text**
  const parts = text.split(/(\*\*[^*]+\*\*|\*[^*]+\*)/g)
  return parts.map((part, i) => {
    if (part.startsWith('**') && part.endsWith('**')) {
      return <strong key={i}>{part.slice(2, -2)}</strong>
    }
    if (part.startsWith('*') && part.endsWith('*')) {
      return <em key={i}>{part.slice(1, -1)}</em>
    }
    return part
  })
}
