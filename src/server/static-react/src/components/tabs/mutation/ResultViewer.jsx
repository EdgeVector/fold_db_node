function ResultViewer({ result }) {
  if (!result) return null

  const isError = result.success === false || result.error
  const isSuccess = result.success === true

  return (
    <div className={`p-4 mt-4 rounded ${
      isError ? 'bg-gruvbox-red/10 border border-gruvbox-red/30' :
      isSuccess ? 'bg-gruvbox-green/10 border border-gruvbox-green/30' :
      'bg-surface-secondary'
    }`}>
      {isError && (
        <div className="text-gruvbox-red font-medium mb-2">Mutation failed</div>
      )}
      {isSuccess && (
        <div className="text-gruvbox-green font-medium mb-2">Mutation succeeded</div>
      )}
      <pre className="font-mono text-sm whitespace-pre-wrap text-secondary">
        {typeof result === 'string' ? result : JSON.stringify(result, null, 2)}
      </pre>
    </div>
  )
}

export default ResultViewer
