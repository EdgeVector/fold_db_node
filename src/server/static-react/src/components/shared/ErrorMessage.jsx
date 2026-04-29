import React from 'react'

/**
 * Heuristic: does this look like an internal/technical error string we
 * should hide behind a disclosure rather than show as the headline?
 *
 * Targets the common shapes that leak from the Rust backend, e.g.
 *   - "Internal error: fingerprints: canonical_names not initialized..."
 *   - "Configuration error: registry not initialized"
 *   - messages containing function refs (foo_bar()) or Rust paths (foo::bar)
 *   - long single-line messages that read like stack frames
 *
 * Conservative on purpose: a short friendly message ("Network error",
 * "Failed to load personas") returns false and renders inline.
 */
export function looksTechnical(message) {
  if (!message || typeof message !== 'string') return false
  const m = message.trim()
  if (m.length === 0) return false
  if (m.length > 140) return true
  if (/^(Internal error|Configuration error|InternalError|panicked at)\b/i.test(m)) return true
  if (/\b[a-z][a-z0-9_]+::[A-Za-z][A-Za-z0-9_]*/.test(m)) return true // Rust path
  if (/\b[a-z][a-z0-9_]{2,}\([^)]*\)/.test(m)) return true // foo_bar() call ref
  // Three or more snake_case identifiers in a row → almost certainly internal
  const snakeMatches = m.match(/\b[a-z][a-z0-9]*(?:_[a-z0-9]+){1,}\b/g) || []
  if (snakeMatches.length >= 3) return true
  return false
}

/**
 * Renders an error message with a friendly headline and the raw text
 * tucked behind a "Technical details" disclosure when the message looks
 * internal. Use this anywhere you'd render `{error}` from an API call.
 *
 * Props:
 *   error    string | null     raw error message (already stringified)
 *   fallback string            user-facing summary when the raw message
 *                              is too technical (e.g. "Couldn't load personas")
 *   onRetry  () => void        optional; renders a Retry button
 *   testId   string            optional data-testid override
 */
export function ErrorMessage({ error, fallback, onRetry, testId }) {
  if (!error) return null
  const technical = looksTechnical(error)

  return (
    <div
      className="text-sm text-gruvbox-red"
      data-testid={testId ?? 'error-message'}
      role="alert"
    >
      {technical ? (
        <>
          <div>{fallback ?? 'Something went wrong'}</div>
          <details className="mt-1">
            <summary className="text-xs text-secondary cursor-pointer hover:text-primary">
              Technical details
            </summary>
            <pre className="mt-1 bg-surface-secondary border border-border text-secondary text-xs p-2 overflow-x-auto whitespace-pre-wrap">
              {error}
            </pre>
          </details>
        </>
      ) : (
        <div>{error}</div>
      )}
      {onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="mt-2 btn-secondary text-xs"
        >
          Retry
        </button>
      )}
    </div>
  )
}

export default ErrorMessage
