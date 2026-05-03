export function looksTechnical(message: string | null | undefined): boolean {
  if (!message || typeof message !== 'string') return false
  const m = message.trim()
  if (m.length === 0) return false
  if (m.length > 140) return true
  if (/^(Internal error|Configuration error|InternalError|panicked at)\b/i.test(m)) return true
  if (/\b[a-z][a-z0-9_]+::[A-Za-z][A-Za-z0-9_]*/.test(m)) return true
  if (/\b[a-z][a-z0-9_]{2,}\([^)]*\)/.test(m)) return true
  const snakeMatches = m.match(/\b[a-z][a-z0-9]*(?:_[a-z0-9]+){1,}\b/g) || []
  if (snakeMatches.length >= 3) return true
  return false
}

interface ErrorMessageProps {
  error: string | null | undefined
  fallback?: string
  onRetry?: () => void
  testId?: string
}

export function ErrorMessage({ error, fallback, onRetry, testId }: ErrorMessageProps) {
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
