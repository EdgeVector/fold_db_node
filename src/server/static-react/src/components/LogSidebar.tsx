import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { systemClient, type LogEntry } from '../api/clients/systemClient'

type LogEntryWithId = LogEntry & { id?: string }
type LogItem = LogEntryWithId | string

// Content-based dedup key. The RING and WEB observability layers each
// generate their own UUIDs for the same tracing event, so `id` differs
// across the SSE stream (WEB) and the poll fallback (RING). Hash the
// fields that DO survive both paths instead.
const dedupKey = (e: LogEntryWithId): string =>
  `${e.timestamp}|${e.level}|${e.event_type}|${e.message}`

// Coalesce SSE arrivals into one React commit per FLUSH_MS window. During
// the post-completion ingestion burst the backend produced ~80
// observability events/sec, and the prior path called setLogs() once per
// event — 80 React commits/sec, each running an O(n) `prev.some(dedupKey)`
// scan over an unbounded buffer. Quadratic main-thread work plus 80 commits
// /sec was the source of the renderer freeze.
const FLUSH_MS = 100
// Cap the buffer so the dedup-by-Set ref + render reconciliation costs
// don't grow unbounded over a long-lived session. 1000 lines covers ≥ 10s
// at peak burst rate, which is plenty for "what just happened".
const MAX_LOGS = 1000

function LogSidebar() {
  const [logs, setLogs] = useState<LogItem[]>([])
  const [isCollapsed, setIsCollapsed] = useState(true)
  const logContainerRef = useRef<HTMLDivElement | null>(null)

  // SSE arrivals queue here and drain into `logs` state on a FLUSH_MS
  // timer. ingestEntry() does the dedup synchronously against seenKeysRef
  // so duplicate events never reach state in the first place — replaces
  // the per-setLogs `prev.some(...)` scan that turned quadratic.
  const pendingRef = useRef<LogEntryWithId[]>([])
  const flushTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const seenKeysRef = useRef<Set<string>>(new Set())

  const formatLog = (entry: LogItem): string => {
    if (typeof entry === 'string') return entry
    const meta = entry.metadata ? JSON.stringify(entry.metadata) : ''
    return `[${entry.level}] [${entry.event_type}] - ${entry.message} ${meta}`
  }

  const getLevelColor = (level: string | undefined): string => {
    switch (level?.toUpperCase()) {
      case 'ERROR': return 'text-gruvbox-red'
      case 'WARN': case 'WARNING': return 'text-gruvbox-yellow'
      case 'INFO': return 'text-secondary'
      default: return 'text-tertiary'
    }
  }

  const formatLogEntry = (entry: LogItem): ReactNode => {
    if (typeof entry === 'string') return <span className="text-tertiary">{entry}</span>
    const time = entry.timestamp ? new Date(entry.timestamp).toLocaleTimeString('en-US', { hour12: false }) : ''
    return (
      <>
        <span className="text-tertiary">{time}</span>
        <span className={`${getLevelColor(entry.level)} ml-2`}>[{entry.level}]</span>
        <span className="text-secondary ml-1">{entry.message}</span>
      </>
    )
  }

  const handleCopy = () => navigator.clipboard.writeText(logs.map(formatLog).join('\n')).catch(() => { /* clipboard not available */ })
  const handleClear = useCallback(() => {
    pendingRef.current = []
    seenKeysRef.current = new Set()
    setLogs([])
  }, [])

  // O(1) admission: dedup against seenKeysRef, then queue for the next
  // flush. Returns true if the entry was new.
  const ingestEntry = useCallback((entry: LogEntryWithId): boolean => {
    const key = dedupKey(entry)
    if (seenKeysRef.current.has(key)) return false
    seenKeysRef.current.add(key)
    pendingRef.current.push(entry)
    return true
  }, [])

  const scheduleFlush = useCallback(() => {
    if (flushTimerRef.current !== null) return
    flushTimerRef.current = setTimeout(() => {
      flushTimerRef.current = null
      if (pendingRef.current.length === 0) return
      const incoming = pendingRef.current
      pendingRef.current = []
      setLogs(prev => {
        const merged = prev.length === 0 ? incoming.slice() : [...prev, ...incoming]
        if (merged.length <= MAX_LOGS) return merged
        // Drop the oldest entries and forget their dedup keys so the
        // index doesn't grow unboundedly over a long-lived session.
        const overflow = merged.length - MAX_LOGS
        for (let i = 0; i < overflow; i++) {
          const e = merged[i]
          if (typeof e !== 'string') seenKeysRef.current.delete(dedupKey(e))
        }
        return merged.slice(overflow)
      })
    }, FLUSH_MS)
  }, [])

  useEffect(() => {
    let cancelled = false

    // Initial buffer fetch. Stays a separate setLogs so older entries
    // land at the front of the array even if SSE has already populated
    // pendingRef with newer events that arrived between mount and this
    // resolving.
    systemClient.getLogs().then(r => {
      if (cancelled || !r.success || !r.data) return
      const fetched = Array.isArray(r.data.logs) ? (r.data.logs as LogEntryWithId[]) : []
      const fresh: LogEntryWithId[] = []
      for (const e of fetched) {
        const key = dedupKey(e)
        if (seenKeysRef.current.has(key)) continue
        seenKeysRef.current.add(key)
        fresh.push(e)
      }
      if (fresh.length === 0) return
      setLogs(prev => [...fresh, ...prev])
    }).catch(() => { /* leave logs empty; SSE/poll will populate */ })

    const eventSource = systemClient.createLogStream(
      (message) => {
        if (cancelled) return
        let entry: LogEntryWithId
        try {
          entry = JSON.parse(message) as LogEntryWithId
        } catch {
          entry = {
            id: `stream-${Date.now()}`,
            timestamp: Date.now(),
            level: 'INFO',
            event_type: 'stream',
            message,
          }
        }
        if (ingestEntry(entry)) scheduleFlush()
      },
      () => {}
    )

    // Fallback poll — only runs when the SSE stream is not OPEN. EventSource
    // auto-reconnects, so this fills the gap during reconnect windows. Skip
    // when the tab is hidden so a hidden Logs sidebar doesn't keep the main
    // thread busy during ingestion.
    const pollFallback = async () => {
      if (cancelled) return
      if (eventSource.readyState === EventSource.OPEN) return
      if (typeof document !== 'undefined' && document.hidden) return
      // Find the latest timestamp seen across both committed logs and the
      // pending buffer so the `since` cursor doesn't double-fetch entries
      // that haven't flushed yet.
      const cur = await new Promise<LogItem[]>(resolve => {
        setLogs(c => { resolve(c); return c })
      })
      let since: number | undefined
      const lastCommitted = cur[cur.length - 1]
      if (lastCommitted && typeof lastCommitted !== 'string') since = lastCommitted.timestamp
      const pending = pendingRef.current
      const lastPending = pending[pending.length - 1]
      if (lastPending && (since === undefined || lastPending.timestamp > since)) {
        since = lastPending.timestamp
      }
      try {
        const r = await systemClient.getLogs(since)
        if (cancelled || !r.success || !r.data?.logs?.length) return
        let added = false
        for (const entry of r.data.logs as LogEntryWithId[]) {
          if (ingestEntry(entry)) added = true
        }
        if (added) scheduleFlush()
      } catch {
        /* next interval will retry */
      }
    }
    const pollInterval = setInterval(() => { void pollFallback() }, 5000)

    return () => {
      cancelled = true
      eventSource.close()
      clearInterval(pollInterval)
      if (flushTimerRef.current) {
        clearTimeout(flushTimerRef.current)
        flushTimerRef.current = null
      }
    }
  }, [ingestEntry, scheduleFlush])

  useEffect(() => {
    if (logContainerRef.current) logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight
  }, [logs])

  if (isCollapsed) {
    return (
      <aside className="w-10 sidebar items-center py-4">
        <button onClick={() => setIsCollapsed(false)} className="btn-secondary btn-sm p-2" title="Expand logs">
          <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
          </svg>
        </button>
      </aside>
    )
  }

  return (
    <aside className="w-80 sidebar">
      <div className="flex items-center justify-between px-4 py-3 border-b border-border">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium">Logs</span>
          <span className="badge badge-neutral">{logs.length}</span>
        </div>
        <div className="flex items-center gap-3">
          <button onClick={handleClear} className="btn-secondary btn-sm">clear</button>
          <button onClick={handleCopy} className="btn-secondary btn-sm">copy</button>
          <button onClick={() => setIsCollapsed(true)} className="btn-secondary btn-sm p-1" title="Collapse">
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
            </svg>
          </button>
        </div>
      </div>

      <div ref={logContainerRef} className="flex-1 overflow-y-auto p-3 font-mono text-xs">
        {logs.length === 0 ? (
          <div className="text-tertiary text-center py-8">No logs yet</div>
        ) : (
          logs.map((entry, idx) => {
            const key = (typeof entry !== 'string' && entry.id) || idx
            return <div key={key} className="list-item">{formatLogEntry(entry)}</div>
          })
        )}
      </div>

      <div className="flex items-center justify-between px-4 py-2 border-t border-border bg-surface-secondary text-xs text-tertiary">
        <div className="flex items-center gap-1.5">
          <span className="status-dot status-dot-success animate-pulse" />
          <span>streaming</span>
        </div>
        <span>{logs.length} entries</span>
      </div>
    </aside>
  )
}

export default LogSidebar
