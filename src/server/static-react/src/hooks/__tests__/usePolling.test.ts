import { renderHook, act } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { usePolling } from '../usePolling'

// Helper: mutate document.hidden + fire visibilitychange. jsdom exposes
// `document.hidden` as a non-configurable getter, so override via
// Object.defineProperty before each test.
function setHidden(hidden: boolean): void {
  Object.defineProperty(document, 'hidden', {
    configurable: true,
    get: () => hidden,
  })
  document.dispatchEvent(new Event('visibilitychange'))
}

describe('usePolling', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    setHidden(false)
  })

  afterEach(() => {
    vi.useRealTimers()
    setHidden(false)
  })

  it('ticks on mount and at intervalMs', async () => {
    const pollFn = vi.fn().mockResolvedValue(undefined)
    renderHook(() =>
      usePolling({ key: 'k', pollFn, intervalMs: 1000, maxFailures: 3 }),
    )

    // Mount-time tick fires synchronously inside the effect.
    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    expect(pollFn).toHaveBeenCalledTimes(1)

    await act(async () => { await vi.advanceTimersByTimeAsync(1000) })
    expect(pollFn).toHaveBeenCalledTimes(2)

    await act(async () => { await vi.advanceTimersByTimeAsync(1000) })
    expect(pollFn).toHaveBeenCalledTimes(3)
  })

  it('uses idleIntervalMs after pollFn returns { idle: true }', async () => {
    const pollFn = vi.fn()
      .mockResolvedValueOnce({ idle: true })
      .mockResolvedValue(undefined)

    renderHook(() =>
      usePolling({
        key: 'k',
        pollFn,
        intervalMs: 1000,
        idleIntervalMs: 5000,
        maxFailures: 3,
      }),
    )

    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    expect(pollFn).toHaveBeenCalledTimes(1)

    // Active interval has elapsed but idle backoff is 5000, so no tick yet.
    await act(async () => { await vi.advanceTimersByTimeAsync(1000) })
    expect(pollFn).toHaveBeenCalledTimes(1)

    // Round out to 5000 — idle interval fires.
    await act(async () => { await vi.advanceTimersByTimeAsync(4000) })
    expect(pollFn).toHaveBeenCalledTimes(2)
  })

  it('returns to active interval once pollFn no longer reports idle', async () => {
    const pollFn = vi.fn()
      .mockResolvedValueOnce({ idle: true })
      .mockResolvedValueOnce(undefined)
      .mockResolvedValue(undefined)

    renderHook(() =>
      usePolling({
        key: 'k',
        pollFn,
        intervalMs: 1000,
        idleIntervalMs: 5000,
        maxFailures: 3,
      }),
    )

    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    expect(pollFn).toHaveBeenCalledTimes(1) // idle returned

    await act(async () => { await vi.advanceTimersByTimeAsync(5000) })
    expect(pollFn).toHaveBeenCalledTimes(2) // idle interval; this poll returns non-idle

    await act(async () => { await vi.advanceTimersByTimeAsync(1000) })
    expect(pollFn).toHaveBeenCalledTimes(3) // back to active interval
  })

  it('bootstraps once at mount when hidden, then pauses subsequent hidden ticks', async () => {
    setHidden(true)
    const pollFn = vi.fn().mockResolvedValue(undefined)

    renderHook(() =>
      usePolling({ key: 'k', pollFn, intervalMs: 1000, maxFailures: 3 }),
    )

    // Bootstrap tick fires at mount even when hidden.
    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    expect(pollFn).toHaveBeenCalledTimes(1)

    // Subsequent hidden ticks skip pollFn.
    await act(async () => { await vi.advanceTimersByTimeAsync(5000) })
    expect(pollFn).toHaveBeenCalledTimes(1)

    // Becoming visible triggers an immediate tick.
    await act(async () => { setHidden(false) })
    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    expect(pollFn).toHaveBeenCalledTimes(2)

    // Subsequent ticks at intervalMs.
    await act(async () => { await vi.advanceTimersByTimeAsync(1000) })
    expect(pollFn).toHaveBeenCalledTimes(3)

    // Hiding stops further pollFn calls (timer keeps cycling but skips pollFn).
    await act(async () => { setHidden(true) })
    await act(async () => { await vi.advanceTimersByTimeAsync(5000) })
    expect(pollFn).toHaveBeenCalledTimes(3)
  })

  it('does not pause when pauseWhenHidden is false', async () => {
    setHidden(true)
    const pollFn = vi.fn().mockResolvedValue(undefined)

    renderHook(() =>
      usePolling({
        key: 'k',
        pollFn,
        intervalMs: 1000,
        maxFailures: 3,
        pauseWhenHidden: false,
      }),
    )

    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    expect(pollFn).toHaveBeenCalledTimes(1)

    await act(async () => { await vi.advanceTimersByTimeAsync(2000) })
    expect(pollFn).toHaveBeenCalledTimes(3)
  })

  it('stops polling when pollFn returns { stop: true }', async () => {
    const pollFn = vi.fn()
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce({ stop: true })

    renderHook(() =>
      usePolling({ key: 'k', pollFn, intervalMs: 1000, maxFailures: 3 }),
    )

    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    await act(async () => { await vi.advanceTimersByTimeAsync(1000) })
    expect(pollFn).toHaveBeenCalledTimes(2)

    // After stop, no further calls — even after long delays.
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000) })
    expect(pollFn).toHaveBeenCalledTimes(2)
  })

  it('stops after maxFailures consecutive failures', async () => {
    const onMaxFailures = vi.fn()
    const pollFn = vi.fn().mockRejectedValue(new Error('boom'))

    renderHook(() =>
      usePolling({
        key: 'k',
        pollFn,
        intervalMs: 1000,
        maxFailures: 2,
        onMaxFailures,
      }),
    )

    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    await act(async () => { await vi.advanceTimersByTimeAsync(1000) })
    expect(onMaxFailures).toHaveBeenCalledTimes(1)
    expect(pollFn).toHaveBeenCalledTimes(2)

    await act(async () => { await vi.advanceTimersByTimeAsync(10_000) })
    expect(pollFn).toHaveBeenCalledTimes(2)
  })

  it('polls once unconditionally on mount even when document is hidden', async () => {
    // Bootstrap-tick contract: the first poll after a key change runs
    // regardless of visibility, so a job already completed before the
    // user surfaces the tab is captured. Without this, a tab that stays
    // hidden continuously since mount (e.g. claude-in-chrome sessions
    // where document.hidden never flips) would never see results.
    setHidden(true)
    const pollFn = vi.fn().mockResolvedValue(undefined)

    renderHook(() =>
      usePolling({ key: 'job-42', pollFn, intervalMs: 1000, maxFailures: 3 }),
    )

    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    expect(pollFn).toHaveBeenCalledTimes(1)
  })

  it('subsequent polls honor visibility after the bootstrap tick', async () => {
    setHidden(true)
    const pollFn = vi.fn().mockResolvedValue(undefined)

    renderHook(() =>
      usePolling({ key: 'job-42', pollFn, intervalMs: 1000, maxFailures: 3 }),
    )

    // Bootstrap fires.
    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    expect(pollFn).toHaveBeenCalledTimes(1)

    // Subsequent hidden ticks skip pollFn (PR #902 perf benefit preserved).
    await act(async () => { await vi.advanceTimersByTimeAsync(5000) })
    expect(pollFn).toHaveBeenCalledTimes(1)

    // Visibility returns → immediate tick.
    await act(async () => { setHidden(false) })
    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    expect(pollFn).toHaveBeenCalledTimes(2)
  })

  it('captures a completed backend job at mount even when hidden', async () => {
    // Bootstrap tick picks up {stop:true} on the first poll without
    // ever requiring a visibility flip — fixes the dogfood repro where
    // a Smart Folder scan completed while the tab was hidden in a
    // claude-in-chrome session and the UI stayed on "Starting scan...".
    setHidden(true)
    const pollFn = vi.fn().mockResolvedValueOnce({ stop: true })

    renderHook(() =>
      usePolling({ key: 'scan-1', pollFn, intervalMs: 1000, maxFailures: 3 }),
    )

    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    expect(pollFn).toHaveBeenCalledTimes(1)

    // Stopped cleanly — no further polls even after long wait or visibility flip.
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000) })
    expect(pollFn).toHaveBeenCalledTimes(1)
    await act(async () => { setHidden(false) })
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000) })
    expect(pollFn).toHaveBeenCalledTimes(1)
  })

  it('catches up via interval reschedule when no visibilitychange fires', async () => {
    // Defense in depth: even if the visibilitychange event is missed,
    // the next scheduled tick (≤ intervalMs after the bootstrap-skipped
    // hidden tick) discovers visibility and runs pollFn.
    setHidden(true)
    const pollFn = vi.fn().mockResolvedValue(undefined)

    renderHook(() =>
      usePolling({ key: 'k', pollFn, intervalMs: 1000, maxFailures: 3 }),
    )

    // Bootstrap tick fires at mount.
    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    expect(pollFn).toHaveBeenCalledTimes(1)

    // Subsequent hidden ticks skip pollFn.
    await act(async () => { await vi.advanceTimersByTimeAsync(5000) })
    expect(pollFn).toHaveBeenCalledTimes(1)

    // Flip document.hidden=false WITHOUT dispatching visibilitychange.
    Object.defineProperty(document, 'hidden', {
      configurable: true,
      get: () => false,
    })

    await act(async () => { await vi.advanceTimersByTimeAsync(1000) })
    expect(pollFn).toHaveBeenCalledTimes(2)
  })

  it('does not poll when key is null', async () => {
    const pollFn = vi.fn().mockResolvedValue(undefined)
    renderHook(() =>
      usePolling({ key: null, pollFn, intervalMs: 1000, maxFailures: 3 }),
    )

    await act(async () => { await vi.advanceTimersByTimeAsync(5000) })
    expect(pollFn).toHaveBeenCalledTimes(0)
  })
})
