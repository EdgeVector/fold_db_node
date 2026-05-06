/**
 * Dev-only PerformanceObserver that logs Long Tasks (main-thread blocks
 * > LONG_TASK_MS) to the console. Catches regressions like the
 * post-completion ingestion-burst freeze (PR fa86f) where SSE/render
 * cascades blocked the renderer for tens of seconds.
 *
 * No-op in production builds and in environments without the
 * `longtask` PerformanceObserver entry type (Safari today). Idempotent —
 * calling install twice returns the same teardown.
 */

const LONG_TASK_MS = 100

let installed: (() => void) | null = null

export function installLongTaskObserver(): () => void {
  if (installed) return installed
  if (typeof PerformanceObserver === "undefined") {
    installed = () => {}
    return installed
  }
  // `supportedEntryTypes` is the cheap feature test; not all browsers
  // expose `longtask` (Safari).
  const supported = (PerformanceObserver as unknown as {
    supportedEntryTypes?: ReadonlyArray<string>
  }).supportedEntryTypes
  if (!supported || !supported.includes("longtask")) {
    installed = () => {}
    return installed
  }

  const observer = new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) {
      if (entry.duration < LONG_TASK_MS) continue
      console.warn(
        `[long-task] ${entry.duration.toFixed(0)}ms blocked at ${entry.startTime.toFixed(0)}ms (name=${entry.name})`,
      )
    }
  })
  observer.observe({ entryTypes: ["longtask"] })

  installed = () => {
    observer.disconnect()
    installed = null
  }
  return installed
}
