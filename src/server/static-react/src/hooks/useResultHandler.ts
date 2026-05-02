import { useEffect, useRef, useState } from 'react'

/**
 * Owns the "operation result" lifecycle shared by all tabs:
 *   - results state + setter
 *   - resultsRef (scroll target)
 *   - handleOperationResult: sets results and scrolls into view
 *   - Auto-clears results whenever activeTab changes (covers all switch paths)
 */
export function useResultHandler<TResult = unknown>(activeTab: string | null) {
  const [results, setResults] = useState<TResult | null>(null)
  const resultsRef = useRef<HTMLElement | null>(null)

  // Clear results whenever the active tab changes (covers all switch paths)
  useEffect(() => {
    setResults(null)
  }, [activeTab])

  const handleOperationResult = (result: TResult) => {
    setResults(result)
    // Scroll results into view after rendering
    setTimeout(() => {
      resultsRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' })
    }, 100)
  }

  return {
    results,
    setResults,
    resultsRef,
    handleOperationResult,
  }
}
