function ResultViewer({ result }) {
  if (!result) return null

  const isError = result.success === false || result.error
  const isSuccess = result.success === true

  return (
    <div className={`p-4 mt-4 rounded ${
      isError ? 'bg-red-900/30 border border-red-700/50' :
      isSuccess ? 'bg-green-900/30 border border-green-700/50' :
      'bg-surface-secondary'
    }`}>
      {isError && (
        <div className="text-red-400 font-medium mb-2">Mutation failed</div>
      )}
      {isSuccess && (
        <div className="text-green-400 font-medium mb-2">Mutation succeeded</div>
      )}
      <pre className="font-mono text-sm whitespace-pre-wrap text-secondary">
        {typeof result === 'string' ? result : JSON.stringify(result, null, 2)}
      </pre>
    </div>
  )
}

export default ResultViewer
