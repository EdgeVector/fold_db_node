import { useState, useEffect, useRef, useCallback } from 'react'
import { ingestionClient } from '../../api/clients'
import { useAppSelector, useAppDispatch } from '../../store/hooks'
import { selectIngestionConfig, saveIngestionConfig } from '../../store/ingestionSlice'

// Smart model recommendations based on environment
const isLocalOllama = (url) => {
  if (!url) return true
  try {
    const host = new URL(url).hostname
    return host === 'localhost' || host === '127.0.0.1' || host === '::1'
  } catch { return true }
}

const getRecommendedModels = (ollamaUrl) => {
  if (isLocalOllama(ollamaUrl)) {
    // navigator.deviceMemory is capped at 8 in some browsers; assume 16 if unavailable
    const ram = navigator.deviceMemory || 16
    return {
      text: ram >= 64 ? 'llama3.3' : ram >= 32 ? 'llama3.1:8b' : 'llama3.2:3b',
      vision: 'qwen3-vl:2b',
      ocr: 'glm-ocr:latest',
    }
  }
  // Remote/LAN instance — assume a capable server
  return { text: 'llama3.3', vision: 'qwen3-vl:4b-instruct', ocr: 'glm-ocr:latest' }
}

/**
 * Build dropdown options with recommended model always at the top.
 * If recommended is not installed, it appears with "(not installed)" suffix.
 */
const buildModelOptions = (installedModels, recommended) => {
  const isInstalled = installedModels.some(m => m.name === recommended)
  const options = []
  if (!isInstalled) {
    options.push({ name: recommended, label: `${recommended} — Recommended (not installed)` })
  }
  for (const m of installedModels) {
    const sizeStr = m.size ? ` (${(m.size / 1e9).toFixed(1)} GB)` : ''
    const tag = m.name === recommended ? ' — Recommended' : ''
    options.push({ name: m.name, label: `${m.name}${sizeStr}${tag}` })
  }
  return options
}

// Ollama generation parameter defaults and bounds (single source of truth)
const OLLAMA_PARAMS = {
  num_ctx:           { default: 16384, min: 2048, max: 250000, step: 1024 },
  num_predict:       { default: 16384, min: 2048, max: 32000,  step: 1024 },
  temperature:       { default: 0.8,   min: 0,    max: 2,      step: 0.01 },
  top_p:             { default: 0.95,  min: 0,    max: 1,      step: 0.01 },
  top_k:             { default: 0,     min: 0,    max: 200,    step: 1 },
  min_p:             { default: 0.0,   min: 0,    max: 1,      step: 0.01 },
  repeat_penalty:    { default: 1.0,   min: 0,    max: 2,      step: 0.01 },
  presence_penalty:  { default: 0.0,   min: 0,    max: 2,      step: 0.01 },
}

/** Parse a number input safely, returning `fallback` if the value is NaN. */
const safeNumber = (value, fallback) => {
  const n = Number(value)
  return Number.isNaN(n) ? fallback : n
}

/** Clamp a number input value within a param's min/max, guarding against NaN. */
const clampParam = (value, param) => {
  const n = safeNumber(value, param.default)
  return Math.max(param.min, Math.min(param.max, n))
}

function useAiConfig({ configSaveStatus, setConfigSaveStatus, onClose }) {
  const dispatch = useAppDispatch()
  const savedConfig = useAppSelector(selectIngestionConfig)
  const [aiProvider, setAiProvider] = useState('Anthropic')
  const [ollamaModel, setOllamaModel] = useState('')
  const [ollamaVisionModel, setOllamaVisionModel] = useState('qwen3-vl:2b')
  const [ollamaOcrModel, setOllamaOcrModel] = useState('glm-ocr:latest')
  const [ollamaBaseUrl, setOllamaBaseUrl] = useState('http://localhost:11434')
  const [recommended, setRecommended] = useState(() => getRecommendedModels('http://localhost:11434'))
  const [anthropicApiKey, setAnthropicApiKey] = useState('')
  const [hasAnthropicEnvKey, setHasAnthropicEnvKey] = useState(false)
  const [anthropicModel, setAnthropicModel] = useState('claude-sonnet-4-20250514')
  const [anthropicBaseUrl, setAnthropicBaseUrl] = useState('https://api.anthropic.com')
  const [ollamaModels, setOllamaModels] = useState([])
  const [ollamaModelsLoading, setOllamaModelsLoading] = useState(false)
  const [ollamaModelsError, setOllamaModelsError] = useState(null)
  const [showAdvanced, setShowAdvanced] = useState(false)
  // Ollama generation parameters
  const [ollamaNumCtx, setOllamaNumCtx] = useState(OLLAMA_PARAMS.num_ctx.default)
  const [ollamaTemperature, setOllamaTemperature] = useState(OLLAMA_PARAMS.temperature.default)
  const [ollamaTopP, setOllamaTopP] = useState(OLLAMA_PARAMS.top_p.default)
  const [ollamaTopK, setOllamaTopK] = useState(OLLAMA_PARAMS.top_k.default)
  const [ollamaNumPredict, setOllamaNumPredict] = useState(OLLAMA_PARAMS.num_predict.default)
  const [ollamaRepeatPenalty, setOllamaRepeatPenalty] = useState(OLLAMA_PARAMS.repeat_penalty.default)
  const [ollamaPresencePenalty, setOllamaPresencePenalty] = useState(OLLAMA_PARAMS.presence_penalty.default)
  const [ollamaMinP, setOllamaMinP] = useState(OLLAMA_PARAMS.min_p.default)
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

  // Update recommendations when URL changes (local vs remote)
  useEffect(() => { setRecommended(getRecommendedModels(ollamaBaseUrl)) }, [ollamaBaseUrl])

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
      const url = savedConfig.ollama?.base_url || 'http://localhost:11434'
      const rec = getRecommendedModels(url)
      setOllamaModel(savedConfig.ollama?.model || rec.text)
      setOllamaVisionModel(savedConfig.ollama?.vision_model || rec.vision)
      setOllamaOcrModel(savedConfig.ollama?.ocr_model || rec.ocr)
      setOllamaBaseUrl(url)
      setRecommended(rec)
      const gp = savedConfig.ollama?.generation_params
      if (gp) {
        setOllamaNumCtx(gp.num_ctx ?? OLLAMA_PARAMS.num_ctx.default)
        setOllamaTemperature(gp.temperature ?? OLLAMA_PARAMS.temperature.default)
        setOllamaTopP(gp.top_p ?? OLLAMA_PARAMS.top_p.default)
        setOllamaTopK(gp.top_k ?? OLLAMA_PARAMS.top_k.default)
        setOllamaNumPredict(gp.num_predict ?? OLLAMA_PARAMS.num_predict.default)
        setOllamaRepeatPenalty(gp.repeat_penalty ?? OLLAMA_PARAMS.repeat_penalty.default)
        setOllamaPresencePenalty(gp.presence_penalty ?? OLLAMA_PARAMS.presence_penalty.default)
        setOllamaMinP(gp.min_p ?? OLLAMA_PARAMS.min_p.default)
      }
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
        ollama: {
          model: ollamaModel,
          vision_model: ollamaVisionModel,
          ocr_model: ollamaOcrModel,
          base_url: ollamaBaseUrl,
          generation_params: {
            num_ctx: ollamaNumCtx,
            temperature: ollamaTemperature,
            top_p: ollamaTopP,
            top_k: ollamaTopK,
            num_predict: ollamaNumPredict,
            repeat_penalty: ollamaRepeatPenalty,
            presence_penalty: ollamaPresencePenalty,
            min_p: ollamaMinP,
          },
        },
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
                ) : ollamaModels.length > 0 || !ollamaModelsError ? (
                  <select value={ollamaModel} onChange={(e) => setOllamaModel(e.target.value)} className="select">
                    {buildModelOptions(ollamaModels, recommended.text).map(o => (
                      <option key={o.name} value={o.name}>{o.label}</option>
                    ))}
                  </select>
                ) : (
                  <input
                    type="text"
                    value={ollamaModel}
                    onChange={(e) => setOllamaModel(e.target.value)}
                    placeholder={`e.g. ${recommended.text}`}
                    className="input"
                  />
                )}
                {ollamaModelsError && (
                  <p className="text-xs text-gruvbox-red mt-1">{ollamaModelsError}</p>
                )}
                {ollamaModel && !ollamaModelsError && !ollamaModels.some(m => m.name === ollamaModel) && (
                  <p className="text-xs text-gruvbox-yellow mt-1">Not installed. Run: <code className="text-gruvbox-blue">ollama pull {ollamaModel}</code></p>
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
          <>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div>
                <label className="label">Vision Model <span className="text-xs text-secondary">(images)</span></label>
                {ollamaModels.length > 0 || !ollamaModelsError ? (
                  <select value={ollamaVisionModel} onChange={(e) => setOllamaVisionModel(e.target.value)} className="select">
                    {buildModelOptions(ollamaModels, recommended.vision).map(o => (
                      <option key={o.name} value={o.name}>{o.label}</option>
                    ))}
                  </select>
                ) : (
                  <input type="text" value={ollamaVisionModel} onChange={(e) => setOllamaVisionModel(e.target.value)} placeholder={`e.g. ${recommended.vision}`} className="input" />
                )}
                <p className="text-xs text-secondary mt-1">Used for image captioning and classification</p>
                {ollamaVisionModel && !ollamaModelsError && !ollamaModels.some(m => m.name === ollamaVisionModel) && (
                  <p className="text-xs text-gruvbox-yellow mt-1">Not installed. Run: <code className="text-gruvbox-blue">ollama pull {ollamaVisionModel}</code></p>
                )}
              </div>
              <div>
                <label className="label">OCR Model <span className="text-xs text-secondary">(documents)</span></label>
                {ollamaModels.length > 0 || !ollamaModelsError ? (
                  <select value={ollamaOcrModel} onChange={(e) => setOllamaOcrModel(e.target.value)} className="select">
                    {buildModelOptions(ollamaModels, recommended.ocr).map(o => (
                      <option key={o.name} value={o.name}>{o.label}</option>
                    ))}
                  </select>
                ) : (
                  <input type="text" value={ollamaOcrModel} onChange={(e) => setOllamaOcrModel(e.target.value)} placeholder={`e.g. ${recommended.ocr}`} className="input" />
                )}
                <p className="text-xs text-secondary mt-1">Used for text extraction from scanned docs and PDFs</p>
                {ollamaOcrModel && !ollamaModelsError && !ollamaModels.some(m => m.name === ollamaOcrModel) && (
                  <p className="text-xs text-gruvbox-yellow mt-1">Not installed. Run: <code className="text-gruvbox-blue">ollama pull {ollamaOcrModel}</code></p>
                )}
              </div>
            </div>
            <div>
              <label className="label">Ollama URL</label>
              <input
                type="text"
                value={ollamaBaseUrl}
                onChange={(e) => setOllamaBaseUrl(e.target.value)}
                placeholder="http://localhost:11434"
                className="input"
              />
              <p className="text-xs text-secondary mt-1">
                {isLocalOllama(ollamaBaseUrl)
                  ? 'Local instance. Use a LAN address for a remote server (e.g. http://192.168.1.100:11434)'
                  : 'Remote instance — recommending larger models'
                }
              </p>
            </div>
          </>
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
              {aiProvider === 'Ollama' && (
                <>
                  <p className="text-xs text-secondary">Generation parameters sent to Ollama with every request.</p>
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                    <div>
                      <label className="label">Context Window (num_ctx)</label>
                      <input type="number" min={OLLAMA_PARAMS.num_ctx.min} max={OLLAMA_PARAMS.num_ctx.max} step={OLLAMA_PARAMS.num_ctx.step} value={ollamaNumCtx} onChange={(e) => setOllamaNumCtx(clampParam(e.target.value, OLLAMA_PARAMS.num_ctx))} className="input" />
                      <p className="text-xs text-secondary mt-1">{OLLAMA_PARAMS.num_ctx.min.toLocaleString()} - {OLLAMA_PARAMS.num_ctx.max.toLocaleString()} tokens</p>
                    </div>
                    <div>
                      <label className="label">Max Output (num_predict)</label>
                      <input type="number" min={OLLAMA_PARAMS.num_predict.min} max={OLLAMA_PARAMS.num_predict.max} step={OLLAMA_PARAMS.num_predict.step} value={ollamaNumPredict} onChange={(e) => setOllamaNumPredict(clampParam(e.target.value, OLLAMA_PARAMS.num_predict))} className="input" />
                      <p className="text-xs text-secondary mt-1">{OLLAMA_PARAMS.num_predict.min.toLocaleString()} - {OLLAMA_PARAMS.num_predict.max.toLocaleString()} tokens</p>
                    </div>
                    <div>
                      <label className="label">Temperature <span className="text-secondary">({ollamaTemperature.toFixed(2)})</span></label>
                      <input type="range" min={OLLAMA_PARAMS.temperature.min} max={OLLAMA_PARAMS.temperature.max} step={OLLAMA_PARAMS.temperature.step} value={ollamaTemperature} onChange={(e) => setOllamaTemperature(safeNumber(e.target.value, OLLAMA_PARAMS.temperature.default))} className="w-full" />
                      <p className="text-xs text-secondary mt-1">0.0 (deterministic) - 2.0 (creative)</p>
                    </div>
                    <div>
                      <label className="label">Top P <span className="text-secondary">({ollamaTopP.toFixed(2)})</span></label>
                      <input type="range" min={OLLAMA_PARAMS.top_p.min} max={OLLAMA_PARAMS.top_p.max} step={OLLAMA_PARAMS.top_p.step} value={ollamaTopP} onChange={(e) => setOllamaTopP(safeNumber(e.target.value, OLLAMA_PARAMS.top_p.default))} className="w-full" />
                      <p className="text-xs text-secondary mt-1">Nucleus sampling threshold</p>
                    </div>
                    <div>
                      <label className="label">Top K <span className="text-secondary">({ollamaTopK})</span></label>
                      <input type="range" min={OLLAMA_PARAMS.top_k.min} max={OLLAMA_PARAMS.top_k.max} step={OLLAMA_PARAMS.top_k.step} value={ollamaTopK} onChange={(e) => setOllamaTopK(safeNumber(e.target.value, OLLAMA_PARAMS.top_k.default))} className="w-full" />
                      <p className="text-xs text-secondary mt-1">0 = disabled</p>
                    </div>
                    <div>
                      <label className="label">Min P <span className="text-secondary">({ollamaMinP.toFixed(2)})</span></label>
                      <input type="range" min={OLLAMA_PARAMS.min_p.min} max={OLLAMA_PARAMS.min_p.max} step={OLLAMA_PARAMS.min_p.step} value={ollamaMinP} onChange={(e) => setOllamaMinP(safeNumber(e.target.value, OLLAMA_PARAMS.min_p.default))} className="w-full" />
                      <p className="text-xs text-secondary mt-1">Minimum probability threshold</p>
                    </div>
                    <div>
                      <label className="label">Repeat Penalty <span className="text-secondary">({ollamaRepeatPenalty.toFixed(2)})</span></label>
                      <input type="range" min={OLLAMA_PARAMS.repeat_penalty.min} max={OLLAMA_PARAMS.repeat_penalty.max} step={OLLAMA_PARAMS.repeat_penalty.step} value={ollamaRepeatPenalty} onChange={(e) => setOllamaRepeatPenalty(safeNumber(e.target.value, OLLAMA_PARAMS.repeat_penalty.default))} className="w-full" />
                      <p className="text-xs text-secondary mt-1">1.0 = no penalty</p>
                    </div>
                    <div>
                      <label className="label">Presence Penalty <span className="text-secondary">({ollamaPresencePenalty.toFixed(2)})</span></label>
                      <input type="range" min={OLLAMA_PARAMS.presence_penalty.min} max={OLLAMA_PARAMS.presence_penalty.max} step={OLLAMA_PARAMS.presence_penalty.step} value={ollamaPresencePenalty} onChange={(e) => setOllamaPresencePenalty(safeNumber(e.target.value, OLLAMA_PARAMS.presence_penalty.default))} className="w-full" />
                      <p className="text-xs text-secondary mt-1">0.0 = no penalty</p>
                    </div>
                  </div>
                </>
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
