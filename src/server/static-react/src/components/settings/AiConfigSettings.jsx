import { useState, useEffect, useRef, useCallback } from 'react'
import { ingestionClient } from '../../api/clients'
import { useAppSelector, useAppDispatch } from '../../store/hooks'
import { selectIngestionConfig, saveIngestionConfig } from '../../store/ingestionSlice'

function useAiConfig({ configSaveStatus, setConfigSaveStatus, onClose }) {
  const dispatch = useAppDispatch()
  const savedConfig = useAppSelector(selectIngestionConfig)
  const [aiProvider, setAiProvider] = useState('Anthropic')
  const [ollamaModel, setOllamaModel] = useState('')
  const [ollamaBaseUrl, setOllamaBaseUrl] = useState('http://localhost:11434')
  const [anthropicApiKey, setAnthropicApiKey] = useState('')
  const [hasAnthropicEnvKey, setHasAnthropicEnvKey] = useState(false)
  const [anthropicModel, setAnthropicModel] = useState('claude-sonnet-4-20250514')
  const [anthropicBaseUrl, setAnthropicBaseUrl] = useState('https://api.anthropic.com')
  const [ollamaModels, setOllamaModels] = useState([])
  const [ollamaModelsLoading, setOllamaModelsLoading] = useState(false)
  const [ollamaModelsError, setOllamaModelsError] = useState(null)
  const [showAdvanced, setShowAdvanced] = useState(false)
  const statusTimeoutRef = useRef(null)
  const ollamaFetchTimeoutRef = useRef(null)

  useEffect(() => {
    return () => {
      if (statusTimeoutRef.current) clearTimeout(statusTimeoutRef.current)
      if (ollamaFetchTimeoutRef.current) clearTimeout(ollamaFetchTimeoutRef.current)
    }
  }, [])

  const fetchOllamaModels = useCallback(async (url) => {
    if (!url) return
    setOllamaModelsLoading(true)
    setOllamaModelsError(null)
    setOllamaModels([])
    try {
      const response = await ingestionClient.listOllamaModels(url)
      const data = response?.data ?? response
      const models = data?.models ?? []
      const error = data?.error
      setOllamaModels(models)
      if (error) {
        setOllamaModelsError(error)
      } else if (models.length === 0) {
        setOllamaModelsError('No models found. Run: ollama pull <model>')
      } else {
        setOllamaModelsError(null)
        // Auto-select first model if none currently selected
        setOllamaModel(prev => {
          if (!prev || !models.some(m => m.name === prev)) return models[0].name
          return prev
        })
      }
    } catch (err) {
      setOllamaModels([])
      setOllamaModelsError(`Could not connect to Ollama: ${err?.message || err}`)
    } finally {
      setOllamaModelsLoading(false)
    }
  }, [])

  // Fetch Ollama models when provider is Ollama and URL changes (debounced)
  useEffect(() => {
    if (aiProvider !== 'Ollama') return
    if (ollamaFetchTimeoutRef.current) clearTimeout(ollamaFetchTimeoutRef.current)
    ollamaFetchTimeoutRef.current = setTimeout(() => fetchOllamaModels(ollamaBaseUrl), 500)
    return () => { if (ollamaFetchTimeoutRef.current) clearTimeout(ollamaFetchTimeoutRef.current) }
  }, [aiProvider, ollamaBaseUrl, fetchOllamaModels])

  // Initialize form state from Redux store
  useEffect(() => {
    if (savedConfig) {
      setOllamaModel(savedConfig.ollama?.model || 'llama3.3')
      setOllamaBaseUrl(savedConfig.ollama?.base_url || 'http://localhost:11434')
      const anthropicKey = savedConfig.anthropic?.api_key || ''
      if (anthropicKey === '***configured***') {
        setHasAnthropicEnvKey(true)
        setAnthropicApiKey('')
      } else {
        setHasAnthropicEnvKey(false)
        setAnthropicApiKey(anthropicKey)
      }
      setAnthropicModel(savedConfig.anthropic?.model || 'claude-sonnet-4-20250514')
      setAnthropicBaseUrl(savedConfig.anthropic?.base_url || 'https://api.anthropic.com')
      setAiProvider(savedConfig.provider || 'Anthropic')
    }
  }, [savedConfig])

  const saveAiConfig = async () => {
    try {
      const config = {
        provider: aiProvider,
        ollama: { model: ollamaModel, base_url: ollamaBaseUrl },
        anthropic: { api_key: anthropicApiKey, model: anthropicModel, base_url: anthropicBaseUrl },
      }
      await dispatch(saveIngestionConfig(config)).unwrap()
      setConfigSaveStatus({ success: true, message: 'Configuration saved successfully' })
      if (statusTimeoutRef.current) clearTimeout(statusTimeoutRef.current)
      statusTimeoutRef.current = setTimeout(() => { setConfigSaveStatus(null); onClose() }, 1500)
    } catch (error) {
      setConfigSaveStatus({ success: false, message: (error instanceof Error ? error.message : String(error)) || 'Failed to save configuration' })
      if (statusTimeoutRef.current) clearTimeout(statusTimeoutRef.current)
      statusTimeoutRef.current = setTimeout(() => setConfigSaveStatus(null), 3000)
    }
  }

  return {
    aiProvider,
    saveAiConfig,
    content: (
      <div className="space-y-4">
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <div>
            <label className="label">Provider</label>
            <select value={aiProvider} onChange={(e) => setAiProvider(e.target.value)} className="select">
              <option value="Anthropic">Anthropic</option>
              <option value="Ollama">Ollama</option>
            </select>
          </div>
          <div>
            <label className="label">Model</label>
            {aiProvider === 'Anthropic' ? (
              <select value={anthropicModel} onChange={(e) => setAnthropicModel(e.target.value)} className="select">
                <option value="claude-sonnet-4-20250514">Claude Sonnet 4</option>
                <option value="claude-haiku-4-5-20251001">Claude Haiku 4.5</option>
              </select>
            ) : (
              <>
                {ollamaModelsLoading ? (
                  <div className="input flex items-center text-sm text-secondary">Loading models...</div>
                ) : ollamaModels.length > 0 ? (
                  <select value={ollamaModel} onChange={(e) => setOllamaModel(e.target.value)} className="select">
                    {ollamaModels.map(m => (
                      <option key={m.name} value={m.name}>{m.name} ({(m.size / 1e9).toFixed(1)} GB)</option>
                    ))}
                  </select>
                ) : (
                  <input
                    type="text"
                    value={ollamaModel}
                    onChange={(e) => setOllamaModel(e.target.value)}
                    placeholder="e.g. llama3.3"
                    className="input"
                  />
                )}
                {ollamaModelsError && (
                  <p className="text-xs text-gruvbox-red mt-1">{ollamaModelsError}</p>
                )}
                {ollamaModel && !ollamaModelsError && (
                  <p className="text-xs text-secondary mt-1">Pull model: <code className="text-gruvbox-blue">ollama pull {ollamaModel}</code></p>
                )}
              </>
            )}
          </div>
        </div>

        {aiProvider === 'Anthropic' && (
          <div>
            <label className="label">API Key <span className="text-xs text-secondary">(<a href="https://console.anthropic.com/settings/keys" target="_blank" rel="noopener noreferrer" className="text-gruvbox-blue hover:underline">get key</a>)</span></label>
            {hasAnthropicEnvKey && !anthropicApiKey && (
              <p className="text-xs text-gruvbox-green mb-1">API key configured</p>
            )}
            <input type="password" value={anthropicApiKey} onChange={(e) => setAnthropicApiKey(e.target.value)} placeholder={hasAnthropicEnvKey ? 'Enter new key to replace...' : 'sk-ant-...'} className="input" />
          </div>
        )}

        {aiProvider === 'Ollama' && (
          <div>
            <label className="label">Ollama URL</label>
            <input
              type="text"
              value={ollamaBaseUrl}
              onChange={(e) => setOllamaBaseUrl(e.target.value)}
              placeholder="http://localhost:11434"
              className="input"
            />
            <p className="text-xs text-secondary mt-1">Use a LAN address for a remote instance (e.g. http://192.168.1.100:11434)</p>
          </div>
        )}

        <div>
          <button onClick={() => setShowAdvanced(!showAdvanced)} className="text-sm text-secondary hover:text-primary flex items-center gap-1">
            <span>{showAdvanced ? '▼' : '▶'}</span> Advanced
          </button>
          {showAdvanced && (
            <div className="mt-3 space-y-3 pl-4 border-l-2 border-border">
              {aiProvider === 'Anthropic' && (
                <div>
                  <label className="label">Base URL</label>
                  <input
                    type="text"
                    value={anthropicBaseUrl}
                    onChange={(e) => setAnthropicBaseUrl(e.target.value)}
                    className="input"
                  />
                </div>
              )}
            </div>
          )}
        </div>

        {configSaveStatus && (
          <div className={`p-3 card ${configSaveStatus.success ? 'card-success text-gruvbox-green' : 'card-error text-gruvbox-red'}`}>
            <span className="text-sm font-medium">{configSaveStatus.success ? '✓' : '✗'} {configSaveStatus.message}</span>
          </div>
        )}
      </div>
    )
  }
}

export default useAiConfig
