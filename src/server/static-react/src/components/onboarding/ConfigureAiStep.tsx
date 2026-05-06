import { useState, useEffect, useCallback, useRef } from 'react'
import { ingestionClient } from '../../api/clients'
import type { OllamaModel } from '../../api/clients/ingestionClient'

const ANTHROPIC_MODELS = [
  { value: 'claude-haiku-4-5-20251001', label: 'Claude Haiku 4.5 (recommended)' },
  { value: 'claude-sonnet-4-20250514', label: 'Claude Sonnet 4' },
]

export type AiProvider = 'Anthropic' | 'Ollama' | 'skip'

export interface AiStepFields {
  provider: AiProvider
  anthropicApiKey: string
  anthropicModel: string
  ollamaUrl: string
  ollamaModel: string
}

interface ConfigureAiStepProps {
  fields: AiStepFields
  onChange: (next: Partial<AiStepFields>) => void
  onNext: () => void
  onSkip: () => void
}

export default function ConfigureAiStep({ fields, onChange, onNext, onSkip }: ConfigureAiStepProps) {
  const [ollamaModels, setOllamaModels] = useState<OllamaModel[]>([])
  const [ollamaModelsLoading, setOllamaModelsLoading] = useState(false)
  const [ollamaModelsError, setOllamaModelsError] = useState<string | null>(null)
  const ollamaFetchTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    return () => {
      if (ollamaFetchTimeoutRef.current) clearTimeout(ollamaFetchTimeoutRef.current)
    }
  }, [])

  const fetchOllamaModels = useCallback(async (url: string) => {
    if (!url) return
    setOllamaModelsLoading(true)
    setOllamaModelsError(null)
    setOllamaModels([])
    try {
      const response = await ingestionClient.listOllamaModels(url)
      const responseUnknown = response as unknown as { data?: { models?: OllamaModel[]; error?: string }; models?: OllamaModel[]; error?: string }
      const inner: { models?: OllamaModel[]; error?: string } = responseUnknown.data ?? responseUnknown ?? {}
      const models: OllamaModel[] = inner.models ?? []
      const error = inner.error
      setOllamaModels(models)
      if (error) {
        setOllamaModelsError(error)
      } else if (models.length === 0) {
        setOllamaModelsError('No models found. Run: ollama pull <model>')
      } else {
        setOllamaModelsError(null)
        if (!fields.ollamaModel || !models.some(m => m.name === fields.ollamaModel)) {
          onChange({ ollamaModel: models[0].name })
        }
      }
    } catch (err) {
      setOllamaModels([])
      const msg = err instanceof Error ? err.message : String(err)
      setOllamaModelsError(`Could not connect to Ollama: ${msg}`)
    } finally {
      setOllamaModelsLoading(false)
    }
    // fields.ollamaModel deliberately excluded — including it re-fires the
    // fetch every keystroke when the user picks a different model.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onChange])

  useEffect(() => {
    if (fields.provider !== 'Ollama') return
    if (ollamaFetchTimeoutRef.current) clearTimeout(ollamaFetchTimeoutRef.current)
    ollamaFetchTimeoutRef.current = setTimeout(() => fetchOllamaModels(fields.ollamaUrl), 500)
    return () => { if (ollamaFetchTimeoutRef.current) clearTimeout(ollamaFetchTimeoutRef.current) }
  }, [fields.provider, fields.ollamaUrl, fetchOllamaModels])

  const currentModel =
    fields.provider === 'Anthropic' ? fields.anthropicModel : fields.ollamaModel
  const canSave =
    fields.provider === 'Anthropic' && !fields.anthropicApiKey.trim()

  return (
    <div>
      <h2 className="text-sm font-bold mb-1">
        <span className="text-gruvbox-green">AI SETUP</span>{' '}
        <span className="text-secondary">Provider configuration</span>
      </h2>
      <p className="text-primary">FoldDB uses AI for data ingestion and search.</p>

      <div className="mt-4">
        <p className="label">Provider</p>
        <select
          value={fields.provider}
          onChange={e => onChange({ provider: e.target.value as AiProvider })}
          className="select"
          data-testid="provider-select"
        >
          <option value="Anthropic">Anthropic (Cloud)</option>
          <option value="Ollama">Ollama (Local)</option>
        </select>
      </div>

      <div className="mt-3">
        <p className="label">Model</p>
        {fields.provider === 'Anthropic' ? (
          <select
            value={fields.anthropicModel}
            onChange={e => onChange({ anthropicModel: e.target.value })}
            className="select"
            data-testid="model-select"
          >
            {ANTHROPIC_MODELS.map(m => <option key={m.value} value={m.value}>{m.label}</option>)}
          </select>
        ) : ollamaModelsLoading ? (
          <div className="input flex items-center text-secondary">Loading models...</div>
        ) : ollamaModels.length > 0 ? (
          <select
            value={fields.ollamaModel}
            onChange={e => onChange({ ollamaModel: e.target.value })}
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
            value={fields.ollamaModel}
            onChange={e => onChange({ ollamaModel: e.target.value })}
            placeholder="e.g. llama3"
            className="input"
            data-testid="model-select"
          />
        )}
        {fields.provider === 'Ollama' && ollamaModelsError && (
          <p className="text-gruvbox-red text-xs mt-1">{ollamaModelsError}</p>
        )}
      </div>

      {fields.provider === 'Anthropic' && (
        <div className="mt-3">
          <p className="label">API Key</p>
          <input
            type="password"
            value={fields.anthropicApiKey}
            onChange={e => onChange({ anthropicApiKey: e.target.value })}
            placeholder="sk-ant-..."
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

      {fields.provider === 'Ollama' && (
        <>
          <div className="mt-3">
            <p className="label">Ollama URL</p>
            <input
              type="text"
              value={fields.ollamaUrl}
              onChange={e => onChange({ ollamaUrl: e.target.value })}
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

      <div className="flex gap-2 mt-4">
        <button
          onClick={onNext}
          disabled={canSave}
          className="btn-primary flex-1 text-center"
        >
          Save & Continue
        </button>
        <button onClick={onSkip} className="btn-secondary flex-1 text-center">
          Skip
        </button>
      </div>
    </div>
  )
}
