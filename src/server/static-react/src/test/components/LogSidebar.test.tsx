/**
 * @fileoverview Tests for LogSidebar component, focused on the dedup
 * regression after the LoggingSystem retirement (Phase 3 / T7).
 *
 * RING and WEB observability layers each generate their own UUIDs for
 * the same tracing event, so the prior `id`-based dedup in LogSidebar
 * leaked one duplicate per event whenever the SSE stream and the poll
 * fallback both delivered. Pin the content-based dedup so it doesn't
 * regress.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, act, waitFor } from '@testing-library/react'
import LogSidebar from '../../components/LogSidebar'

type LogEntry = {
  id: string
  timestamp: number
  level: string
  event_type: string
  message: string
  metadata?: Record<string, string>
}

const mkEntry = (overrides: Partial<LogEntry> = {}): LogEntry => ({
  id: `id-${Math.random().toString(36).slice(2)}`,
  timestamp: 1_700_000_000_000,
  level: 'INFO',
  event_type: 'test',
  message: 'hello',
  ...overrides,
})

let getLogsMock: ReturnType<typeof vi.fn>
let createLogStreamMock: ReturnType<typeof vi.fn>
let onMessageHandler: ((msg: string) => void) | null

vi.mock('../../api/clients/systemClient', () => ({
  systemClient: {
    get getLogs() { return getLogsMock },
    get createLogStream() { return createLogStreamMock },
  },
}))

beforeEach(() => {
  vi.useFakeTimers()
  onMessageHandler = null
  getLogsMock = vi.fn().mockResolvedValue({ success: true, data: { logs: [] } })
  createLogStreamMock = vi.fn((onMessage: (msg: string) => void) => {
    onMessageHandler = onMessage
    return { close: vi.fn() }
  })
})

afterEach(() => {
  vi.useRealTimers()
})

const expand = async () => {
  const expandBtn = screen.getByTitle('Expand logs')
  await act(async () => { expandBtn.click() })
}

describe('LogSidebar — RING/WEB id-mismatch dedup', () => {
  it('deduplicates the same event arriving from SSE then poll with a different id', async () => {
    // Initial fetch: empty buffer.
    render(<LogSidebar />)
    await expand()
    await act(async () => { await Promise.resolve() })

    // SSE delivers an event (WEB layer id).
    const webEvent = mkEntry({ id: 'web-uuid', timestamp: 1, message: 'hi', event_type: 'svc' })
    await act(async () => { onMessageHandler!(JSON.stringify(webEvent)) })

    // Poll fires after 2s — RING returns the SAME logical event with a
    // different id (RING layer uuid).
    const ringEvent = { ...webEvent, id: 'ring-uuid' }
    getLogsMock.mockResolvedValueOnce({ success: true, data: { logs: [ringEvent] } })
    await act(async () => { vi.advanceTimersByTime(2000); await Promise.resolve(); await Promise.resolve() })

    expect(screen.getByText('1 entries')).toBeInTheDocument()
  })

  it('preserves SSE events that arrive before initial getLogs() resolves', async () => {
    // Defer the initial fetch resolution.
    let resolveInitial!: (v: { success: true; data: { logs: LogEntry[] } }) => void
    getLogsMock.mockImplementationOnce(() => new Promise(r => { resolveInitial = r as typeof resolveInitial }))

    render(<LogSidebar />)
    await expand()
    await act(async () => { await Promise.resolve() })

    // Event arrives via SSE before initial fetch resolves.
    const webEvent = mkEntry({ id: 'web-1', timestamp: 5, message: 'mid-flight', event_type: 'svc' })
    await act(async () => { onMessageHandler!(JSON.stringify(webEvent)) })
    expect(screen.getByText('1 entries')).toBeInTheDocument()

    // Initial fetch now resolves with two RING entries — one for the
    // event already received via SSE (different id), one for an older
    // entry that SSE missed.
    const ringSame = { ...webEvent, id: 'ring-1' }
    const olderRing = mkEntry({ id: 'ring-0', timestamp: 1, message: 'older', event_type: 'svc' })
    await act(async () => {
      resolveInitial({ success: true, data: { logs: [olderRing, ringSame] } })
      await Promise.resolve()
    })

    // Final state: olderRing + the SSE event (deduped against ringSame).
    await waitFor(() => expect(screen.getByText('2 entries')).toBeInTheDocument())
    expect(screen.getByText('older')).toBeInTheDocument()
    expect(screen.getByText('mid-flight')).toBeInTheDocument()
  })

  it('admits events that share a timestamp but differ in message', async () => {
    render(<LogSidebar />)
    await expand()
    await act(async () => { await Promise.resolve() })

    const a = mkEntry({ id: 'a', timestamp: 100, message: 'one', event_type: 'svc' })
    const b = mkEntry({ id: 'b', timestamp: 100, message: 'two', event_type: 'svc' })
    await act(async () => { onMessageHandler!(JSON.stringify(a)) })
    await act(async () => { onMessageHandler!(JSON.stringify(b)) })

    expect(screen.getByText('2 entries')).toBeInTheDocument()
  })
})
