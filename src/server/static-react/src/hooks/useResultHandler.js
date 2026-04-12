import { useEffect, useRef, useState } from 'react'

/**
 * Owns the "operation result" lifecycle shared by all tabs:
 *   - results state + setter
 *   - resultsRef (scroll target)
 *   - handleOperationResult: sets results and scrolls into view
 *   - Auto-clears results whenever activeTab changes (covers all switch paths)
 */
export function useResultHandler(activeTab) {
  const [results, setResults] = useState(null)
  const resultsRef = useRef(null)

  // Clear results whenever the active tab changes (covers all switch paths)
  useEffect(() => {
    setResults(null)
  }, [activeTab])

  const handleOperationResult = (result) => {
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
