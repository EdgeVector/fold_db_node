import { useState, useEffect, useCallback, useRef } from 'react'
import { ingestionClient } from '../../api/clients'
import { useAppDispatch } from '../../store/hooks'
import { fetchIngestionConfig } from '../../store/ingestionSlice'

const ANTHROPIC_MODELS = [
  { value: 'claude-sonnet-4-20250514', label: 'Claude Sonnet 4' },
  { value: 'claude-haiku-4-5-20251001', label: 'Claude Haiku 4.5' },
]

export default function ConfigureAiStep({ onNext, onSkip }) {
  const dispatch = useAppDispatch()
  const [provider, setProvider] = useState('Anthropic')
  const [ollamaModel, setOllamaModel] = useState('')
  const [ollamaUrl, setOllamaUrl] = useState('http://localhost:11434')
  const [anthropicApiKey, setAnthropicApiKey] = useState('')
  const [anthropicModel, setAnthropicModel] = useState('claude-sonnet-4-20250514')
  const [ollamaModels, setOllamaModels] = useState([])
  const [ollamaModelsLoading, setOllamaModelsLoading] = useState(false)
  const [ollamaModelsError, setOllamaModelsError] = useState(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [saveResult, setSaveResult] = useState(null)
  const [alreadyConfigured, setAlreadyConfigured] = useState(false)
  const ollamaFetchTimeoutRef = useRef(null)

  useEffect(() => {
    return () => {
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

  useEffect(() => {
    if (provider !== 'Ollama') return
    if (ollamaFetchTimeoutRef.current) clearTimeout(ollamaFetchTimeoutRef.current)
    ollamaFetchTimeoutRef.current = setTimeout(() => fetchOllamaModels(ollamaUrl), 500)
    return () => { if (ollamaFetchTimeoutRef.current) clearTimeout(ollamaFetchTimeoutRef.current) }
  }, [provider, ollamaUrl, fetchOllamaModels])

  useEffect(() => {
    let cancelled = false
    ingestionClient.getConfig().then(response => {
      if (cancelled) return
      if (response.success && response.data) {
        const cfg = response.data
        setProvider(cfg.provider || 'Anthropic')
        if (cfg.ollama?.model) setOllamaModel(cfg.ollama.model)
        if (cfg.ollama?.base_url) setOllamaUrl(cfg.ollama.base_url)
        if (cfg.anthropic?.model) setAnthropicModel(cfg.anthropic.model)
        if (cfg.anthropic?.api_key && cfg.anthropic.api_key.includes('configured')) {
          setAlreadyConfigured(true)
        }
      }
      setLoading(false)
    }).catch(() => {
      if (!cancelled) setLoading(false)
    })
    return () => { cancelled = true }
  }, [])

  const handleSave = async () => {
    setSaving(true)
    setSaveResult(null)
    const config = {
      provider,
      ollama: {
        model: provider === 'Ollama' ? (ollamaModel || (ollamaModels[0]?.name ?? '')) : '',
        base_url: ollamaUrl,
      },
      anthropic: {
        api_key: provider === 'Anthropic' ? anthropicApiKey : '',
        model: provider === 'Anthropic' ? anthropicModel : '',
        base_url: 'https://api.anthropic.com',
      },
    }
    try {
      const response = await ingestionClient.saveConfig(config)
      if (response.success) {
        dispatch(fetchIngestionConfig())
        onNext()
        return
      } else {
        setSaveResult('error')
      }
    } catch {
      setSaveResult('error')
    } finally {
      setSaving(false)
    }
  }

  if (loading) {
    return <p className="text-secondary text-center py-6">Loading configuration...</p>
  }

  const currentModel = provider === 'Anthropic'
    ? anthropicModel
    : ollamaModel
  const canSave = saving
    || (provider === 'Anthropic' && !anthropicApiKey && !alreadyConfigured)

  return (
    <div>
      <h2 className="text-sm font-bold mb-1">
        <span className="text-gruvbox-green">AI SETUP</span>{' '}
        <span className="text-secondary">Provider configuration</span>
      </h2>
      <p className="text-primary">FoldDB uses AI for data ingestion and search.</p>

      {alreadyConfigured && (
        <div className="card-success p-3 mt-3">
          <p><span className="badge badge-success">CONFIGURED</span></p>
          <p className="text-primary mt-1">AI provider is already set up. Update below or skip.</p>
        </div>
      )}

      <div className="mt-4">
        <p className="label">Provider</p>
        <select
          value={provider}
          onChange={e => setProvider(e.target.value)}
          className="select"
          data-testid="provider-select"
        >
          <option value="Anthropic">Anthropic (Cloud)</option>
          <option value="Ollama">Ollama (Local)</option>
        </select>
      </div>

      <div className="mt-3">
        <p className="label">Model</p>
        {provider === 'Anthropic' ? (
          <select
            value={anthropicModel}
            onChange={e => setAnthropicModel(e.target.value)}
            className="select"
            data-testid="model-select"
          >
            {ANTHROPIC_MODELS.map(m => <option key={m.value} value={m.value}>{m.label}</option>)}
          </select>
        ) : ollamaModelsLoading ? (
          <div className="input flex items-center text-secondary">Loading models...</div>
        ) : ollamaModels.length > 0 ? (
          <select
            value={ollamaModel}
            onChange={e => setOllamaModel(e.target.value)}
            className="select"
            data-testid="model-select"
          >
            {ollamaModels.map(m => (
              <option key={m.name} value={m.name}>{m.name} ({(m.size / 1e9).toFixed(1)} GB)</option>
            ))}
          </select>
        ) : (
          <input
            type="text"
            value={ollamaModel}
            onChange={e => setOllamaModel(e.target.value)}
            placeholder="e.g. llama3"
            className="input"
            data-testid="model-select"
          />
        )}
        {provider === 'Ollama' && ollamaModelsError && (
          <p className="text-gruvbox-red text-xs mt-1">{ollamaModelsError}</p>
        )}
      </div>

      {provider === 'Anthropic' && (
        <div className="mt-3">
          <p className="label">API Key</p>
          <input
            type="password"
            value={anthropicApiKey}
            onChange={e => setAnthropicApiKey(e.target.value)}
            placeholder={alreadyConfigured ? '***configured***' : 'sk-ant-...'}
            className="input"
            data-testid="api-key-input"
          />
          <p className="mt-1">
            <a
              href="https://console.anthropic.com/settings/keys"
              target="_blank"
              rel="noopener noreferrer"
              className="text-gruvbox-link text-xs hover:underline"
            >
              Get API key from Anthropic
            </a>
          </p>
        </div>
      )}

      {provider === 'Ollama' && (
        <>
          <div className="mt-3">
            <p className="label">Ollama URL</p>
            <input
              type="text"
              value={ollamaUrl}
              onChange={e => setOllamaUrl(e.target.value)}
              placeholder="http://localhost:11434"
              className="input"
            />
            <p className="text-secondary text-xs mt-1">
              Use a LAN address (e.g. http://192.168.1.100:11434) for a remote instance
            </p>
          </div>
          <div className="card p-3 mt-3">
            <p className="font-bold text-primary">Setup</p>
            <p className="text-secondary">Make sure Ollama is running:</p>
            <p className="text-gruvbox-yellow mt-1">$ ollama pull {currentModel}</p>
          </div>
        </>
      )}

      {saveResult === 'error' && (
        <p className="text-gruvbox-red mt-2">Failed to save. Please try again.</p>
      )}

      <div className="flex gap-2 mt-4">
        <button
          onClick={handleSave}
          disabled={canSave}
          className="btn-primary flex-1 text-center"
        >
          {saving ? 'Saving...' : 'Save & Continue'}
        </button>
        <button onClick={onSkip} className="btn-secondary flex-1 text-center">
          Skip
        </button>
      </div>
    </div>
  )
}
