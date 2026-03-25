import { useState, useEffect, useRef } from 'react'
import { getEmbeddingConfig, updateEmbeddingConfig } from '../../api/clients/systemClient'

const OLLAMA_MODELS = [
  { value: 'qwen3-embedding:0.6b', label: 'Qwen3 0.6B (1024 dims, fastest)' },
  { value: 'qwen3-embedding:4b',   label: 'Qwen3 4B (2560 dims, balanced)' },
  { value: 'qwen3-embedding:8b',   label: 'Qwen3 8B (4096 dims, best quality)' },
]

function useEmbeddingConfig({ configSaveStatus, setConfigSaveStatus, onClose }) {
  const [provider, setProvider] = useState('Ollama')
  const [ollamaBaseUrl, setOllamaBaseUrl] = useState('http://localhost:11434')
  const [ollamaModel, setOllamaModel] = useState('qwen3-embedding:4b')
  const [customModel, setCustomModel] = useState('')
  const [useCustomModel, setUseCustomModel] = useState(false)
  const [dimensions, setDimensions] = useState('')
  const statusTimeoutRef = useRef(null)

  useEffect(() => {
    return () => { if (statusTimeoutRef.current) clearTimeout(statusTimeoutRef.current) }
  }, [])

  useEffect(() => { loadConfig() }, [])

  const loadConfig = async () => {
    try {
      const response = await getEmbeddingConfig()
      if (!response.success || !response.data) return
      const cfg = response.data
      setProvider(cfg.type)
      if (cfg.type === 'Ollama') {
        setOllamaBaseUrl(cfg.base_url || 'http://localhost:11434')
        const isPreset = OLLAMA_MODELS.some(m => m.value === cfg.model)
        if (isPreset) {
          setOllamaModel(cfg.model)
          setUseCustomModel(false)
        } else {
          setUseCustomModel(true)
          setCustomModel(cfg.model || '')
        }
        setDimensions(cfg.dimensions != null ? String(cfg.dimensions) : '')
      }
    } catch (error) {
      console.error('Failed to load embedding config:', error)
    }
  }

  const saveEmbeddingConfig = async () => {
    try {
      let config
      if (provider === 'FastEmbed') {
        config = { type: 'FastEmbed' }
      } else {
        const model = useCustomModel ? customModel.trim() : ollamaModel
        if (!model) {
          setConfigSaveStatus({ success: false, message: 'Model name is required' })
          statusTimeoutRef.current = setTimeout(() => setConfigSaveStatus(null), 3000)
          return
        }
        const dims = dimensions.trim() ? parseInt(dimensions.trim(), 10) : null
        if (dimensions.trim() && isNaN(dims)) {
          setConfigSaveStatus({ success: false, message: 'Dimensions must be a number' })
          statusTimeoutRef.current = setTimeout(() => setConfigSaveStatus(null), 3000)
          return
        }
        config = { type: 'Ollama', base_url: ollamaBaseUrl.trim() || 'http://localhost:11434', model, dimensions: dims }
      }
      const response = await updateEmbeddingConfig(config)
      if (response.success) {
        setConfigSaveStatus({ success: true, message: response.data?.message || 'Saved. Restart the server to apply.' })
        statusTimeoutRef.current = setTimeout(() => { setConfigSaveStatus(null); onClose() }, 3000)
      } else {
        setConfigSaveStatus({ success: false, message: response.error || 'Failed to save' })
        statusTimeoutRef.current = setTimeout(() => setConfigSaveStatus(null), 4000)
      }
    } catch (error) {
      setConfigSaveStatus({ success: false, message: error instanceof Error ? error.message : String(error) })
      statusTimeoutRef.current = setTimeout(() => setConfigSaveStatus(null), 4000)
    }
  }

  return {
    saveEmbeddingConfig,
    content: (
      <div className="space-y-4">
        <p className="text-sm text-secondary mb-4">
          Choose the embedding model for semantic search. Changes require a server restart and a database reset.
        </p>

        <div>
          <label className="label">Provider</label>
          <select value={provider} onChange={(e) => setProvider(e.target.value)} className="select">
            <option value="FastEmbed">FastEmbed (local ONNX, all-MiniLM-L6-v2, 384 dims)</option>
            <option value="Ollama">Ollama (Qwen3-Embedding or custom)</option>
          </select>
        </div>

        {provider === 'Ollama' && (
          <div className="space-y-3">
            <div>
              <label className="label">Ollama Server URL</label>
              <input
                type="text"
                value={ollamaBaseUrl}
                onChange={(e) => setOllamaBaseUrl(e.target.value)}
                placeholder="http://localhost:11434"
                className="input"
              />
            </div>

            <div>
              <label className="label">Model</label>
              {!useCustomModel ? (
                <select value={ollamaModel} onChange={(e) => setOllamaModel(e.target.value)} className="select">
                  {OLLAMA_MODELS.map(m => (
                    <option key={m.value} value={m.value}>{m.label}</option>
                  ))}
                </select>
              ) : (
                <input
                  type="text"
                  value={customModel}
                  onChange={(e) => setCustomModel(e.target.value)}
                  placeholder="e.g. nomic-embed-text"
                  className="input"
                />
              )}
              <button
                type="button"
                onClick={() => setUseCustomModel(!useCustomModel)}
                className="text-xs text-secondary mt-1 hover:text-primary underline"
              >
                {useCustomModel ? 'Use preset model' : 'Enter custom model name'}
              </button>
            </div>

            <div>
              <label className="label">Dimensions (optional MRL truncation)</label>
              <input
                type="number"
                value={dimensions}
                onChange={(e) => setDimensions(e.target.value)}
                placeholder="Leave blank to use model default"
                className="input"
                min="32"
              />
              <p className="text-xs text-secondary mt-1">
                Qwen3 supports MRL: 0.6B max 1024, 4B max 2560, 8B max 4096. Switching sizes requires resetting the database.
              </p>
            </div>

            <div className="card card-warning p-3">
              <p className="text-xs text-gruvbox-yellow">
                <strong>Note:</strong> Ollama must be running with the selected model pulled (<code>ollama pull {useCustomModel ? customModel || '…' : ollamaModel}</code>).
                After saving, reset the database and restart the server.
              </p>
            </div>
          </div>
        )}

        {configSaveStatus && (
          <div className={`p-3 card ${configSaveStatus.success ? 'card-success text-gruvbox-green' : 'card-error text-gruvbox-red'}`}>
            <span className="text-sm font-medium">{configSaveStatus.success ? '✓' : '✗'} {configSaveStatus.message}</span>
          </div>
        )}
      </div>
    )
  }
}

export default useEmbeddingConfig
