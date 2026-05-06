import { useEffect, useRef } from 'react'

interface PollResult {
  stop?: boolean
  /**
   * When true, the next interval uses `idleIntervalMs` instead of `intervalMs`.
   * Lets a hook back off when there's no in-flight work without giving up its
   * mount entirely.
   */
  idle?: boolean
}

interface UsePollingOpts {
  /** Truthy value to enable polling (null/undefined/false = idle). */
  key: unknown
  /**
   * Async function called each interval. Return `{ stop: true }` to halt
   * polling; return `{ idle: true }` to use `idleIntervalMs` for the next
   * delay; any other return value continues at `intervalMs`.
   */
  pollFn: () => Promise<PollResult | undefined | void>
  /** Polling interval in milliseconds when there is in-flight work. */
  intervalMs: number
  /**
   * Polling interval when the most recent poll returned `{ idle: true }`.
   * Defaults to `intervalMs` (no backoff). Set higher to throttle the
   * always-on background pollers (header chip, status panel) when nothing
   * is going on.
   */
  idleIntervalMs?: number
  /** Stop polling after this many consecutive failures. */
  maxFailures: number
  /** Called when maxFailures is reached. */
  onMaxFailures?: () => void
  /**
   * Pause polling while the document is hidden, resume (and tick immediately)
   * when it becomes visible again. Default true. Set false for polling that
   * must continue across tab switches (none today).
   */
  pauseWhenHidden?: boolean
}

/**
 * Generic interval-based polling hook with consecutive failure tracking,
 * idle backoff, and tab-visibility gating.
 */
export function usePolling({
  key,
  pollFn,
  intervalMs,
  idleIntervalMs,
  maxFailures,
  onMaxFailures,
  pauseWhenHidden = true,
}: UsePollingOpts): void {
  const pollFnRef = useRef(pollFn)
  const onMaxFailuresRef = useRef(onMaxFailures)
  useEffect(() => {
    pollFnRef.current = pollFn
  })
  useEffect(() => {
    onMaxFailuresRef.current = onMaxFailures
  })

  useEffect(() => {
    if (!key) return
    let cancelled = false
    let stopped = false
    let failCount = 0
    let timer: ReturnType<typeof setTimeout> | null = null
    let lastIdle = false

    const hasDoc = typeof document !== 'undefined'
    const isHidden = () => pauseWhenHidden && hasDoc && document.hidden

    const clearTimer = () => {
      if (timer) {
        clearTimeout(timer)
        timer = null
      }
    }

    const tick = async () => {
      timer = null
      if (cancelled || stopped) return

      // Skip pollFn while hidden but keep the loop alive: re-arm the
      // timer so polling resumes within `intervalMs` of the tab going
      // visible again, even if a `visibilitychange` event is missed
      // (Chrome can drop one when the tab was hidden before the
      // listener attached, or when an extension-driven session never
      // surfaces the tab). The visibility listener still snaps to an
      // immediate tick on hidden→visible for snappiness.
      if (isHidden()) {
        timer = setTimeout(tick, intervalMs)
        return
      }

      try {
        const result = await pollFnRef.current()
        if (cancelled) return
        failCount = 0
        if (result?.stop) {
          stopped = true
          return
        }
        lastIdle = result?.idle === true
      } catch {
        if (cancelled) return
        failCount++
        if (failCount >= maxFailures) {
          onMaxFailuresRef.current?.()
          stopped = true
          return
        }
      }
      if (cancelled || stopped) return
      const delay = lastIdle ? (idleIntervalMs ?? intervalMs) : intervalMs
      timer = setTimeout(tick, delay)
    }

    const onVisibility = () => {
      if (cancelled || stopped || document.hidden) return
      // hidden→visible: cancel any pending hidden-mode reschedule and
      // tick immediately instead of waiting up to intervalMs.
      clearTimer()
      tick()
    }

    tick()

    if (pauseWhenHidden && hasDoc) {
      document.addEventListener('visibilitychange', onVisibility)
    }

    return () => {
      cancelled = true
      clearTimer()
      if (pauseWhenHidden && hasDoc) {
        document.removeEventListener('visibilitychange', onVisibility)
      }
    }
  }, [key, intervalMs, idleIntervalMs, maxFailures, pauseWhenHidden])
}
