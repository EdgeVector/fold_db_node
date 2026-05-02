import { useEffect, useRef } from 'react'

interface PollResult {
  stop?: boolean
}

interface UsePollingOpts {
  /** Truthy value to enable polling (null/undefined/false = idle). */
  key: unknown
  /**
   * Async function called each interval. Return `{ stop: true }` to halt
   * polling; any other return value continues.
   */
  pollFn: () => Promise<PollResult | undefined | void>
  /** Polling interval in milliseconds. */
  intervalMs: number
  /** Stop polling after this many consecutive failures. */
  maxFailures: number
  /** Called when maxFailures is reached. */
  onMaxFailures?: () => void
}

/**
 * Generic interval-based polling hook with consecutive failure tracking.
 */
export function usePolling({
  key,
  pollFn,
  intervalMs,
  maxFailures,
  onMaxFailures,
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
    let failCount = 0
    let timer: ReturnType<typeof setInterval> | null = null

    const tick = async () => {
      try {
        const result = await pollFnRef.current()
        if (cancelled) return
        failCount = 0
        if (result?.stop) {
          if (timer) {
            clearInterval(timer)
            timer = null
          }
        }
      } catch {
        if (cancelled) return
        failCount++
        if (failCount >= maxFailures) {
          onMaxFailuresRef.current?.()
          if (timer) {
            clearInterval(timer)
            timer = null
          }
        }
      }
    }

    tick()
    timer = setInterval(tick, intervalMs)
    return () => {
      cancelled = true
      if (timer) clearInterval(timer)
    }
  }, [key, intervalMs, maxFailures])
}
