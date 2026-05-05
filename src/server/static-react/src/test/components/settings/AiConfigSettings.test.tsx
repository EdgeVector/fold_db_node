/**
 * @fileoverview Tests for the Vision Backend picker in AiConfigSettings.
 *
 * PR #526 added `vision_backend` (Ollama | Anthropic) to `SavedConfig` so
 * users can route image → markdown conversion through Claude on machines
 * without a local Ollama daemon. This test covers the UI counterpart: a
 * dropdown in the AI Config settings that reads + writes the field.
 *
 * Assertions:
 *   1. The picker mounts with the current `vision_backend` from Redux.
 *   2. Flipping the picker + saving calls `ingestionClient.saveConfig`
 *      with the new `vision_backend` value in the payload — even when the
 *      text provider is unchanged, and independent of `provider`.
 */

import React from 'react'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { screen, fireEvent, waitFor } from '@testing-library/react'

import { renderWithRedux, apiOk } from '../../utils/testUtilities'
import ingestionReducer from '../../../store/ingestionSlice'
import useAiConfig from '../../../components/settings/AiConfigSettings'
import type { SaveStatus } from '../../../components/settings/types'
import type { IngestionConfig } from '../../../api/clients/ingestionClient'
import { ingestionClient, webSearchClient } from '../../../api/clients'

// Spy on the singleton so the same object the slice uses is intercepted.
const saveConfigSpy = vi.spyOn(ingestionClient, 'saveConfig')
const getConfigSpy = vi.spyOn(ingestionClient, 'getConfig')
const listModelsSpy = vi.spyOn(ingestionClient, 'listOllamaModels')
const getWebSearchKeyStatusSpy = vi.spyOn(webSearchClient, 'getKeyStatus')
const saveWebSearchKeySpy = vi.spyOn(webSearchClient, 'saveKey')

/**
 * Tiny harness that wires the `useAiConfig` hook the same way SettingsTab
 * does, and exposes a Save button the test can click.
 */
function Harness() {
  const [status, setStatus] = React.useState<SaveStatus | null>(null)
  const { content, saveAiConfig } = useAiConfig({
    configSaveStatus: status,
    setConfigSaveStatus: setStatus,
    onClose: () => {},
  })
  return (
    <div>
      {content}
      <button type="button" onClick={() => saveAiConfig()}>Save Configuration</button>
      {status && <span data-testid="status">{status.message}</span>}
    </div>
  )
}

const baseConfig: IngestionConfig = {
  provider: 'Anthropic',
  anthropic: {
    api_key: '***configured***',
    model: 'claude-sonnet-4-20250514',
    base_url: 'https://api.anthropic.com',
  },
  ollama: {
    model: 'llama3.3',
    vision_model: 'qwen3-vl:2b',
    ocr_model: 'glm-ocr:latest',
    base_url: 'http://localhost:11434',
  },
  vision_backend: 'Ollama',
}

function renderHarness(overrides: Partial<IngestionConfig> = {}) {
  const config = { ...baseConfig, ...overrides }
  return renderWithRedux(<Harness />, {
    preloadedState: {
      ingestion: {
        config,
        loading: false,
        error: null,
        saving: false,
        saveError: null,
      },
    },
    extraReducers: { ingestion: ingestionReducer },
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  // Save succeeds with a no-op re-fetch result so the thunk resolves fully.
  saveConfigSpy.mockResolvedValue(apiOk({ success: true, message: 'ok' }))
  getConfigSpy.mockResolvedValue(apiOk(baseConfig))
  // Stub the Ollama model list so the debounced effect doesn't complain.
  listModelsSpy.mockResolvedValue(apiOk({ models: [] }))
  // Default: no web-search key configured. Individual tests override.
  getWebSearchKeyStatusSpy.mockResolvedValue(apiOk({ has_key: false }))
  saveWebSearchKeySpy.mockResolvedValue(apiOk({ has_key: true }))
})

describe('AiConfigSettings — Vision Backend picker', () => {
  it('initializes the dropdown from savedConfig.vision_backend', async () => {
    renderHarness({ vision_backend: 'Anthropic' })

    const select = await screen.findByLabelText(/Vision Backend/i)
    expect((select as HTMLSelectElement).value).toBe('Anthropic')
  })

  it('defaults to Ollama when savedConfig omits vision_backend', async () => {
    const { vision_backend: _omit, ...without } = baseConfig
    renderWithRedux(<Harness />, {
      preloadedState: {
        ingestion: {
          config: without,
          loading: false,
          error: null,
          saving: false,
          saveError: null,
        },
      },
      extraReducers: { ingestion: ingestionReducer },
    })

    const select = await screen.findByLabelText(/Vision Backend/i)
    expect((select as HTMLSelectElement).value).toBe('Ollama')
  })

  it('writes the new value into the saveConfig payload when flipped', async () => {
    renderHarness({ vision_backend: 'Ollama' })

    const select = await screen.findByLabelText(/Vision Backend/i)
    fireEvent.change(select, { target: { value: 'Anthropic' } })

    fireEvent.click(screen.getByRole('button', { name: /Save Configuration/i }))

    await waitFor(() => expect(saveConfigSpy).toHaveBeenCalledTimes(1))
    const payload = saveConfigSpy.mock.calls[0][0]
    expect((payload as IngestionConfig).vision_backend).toBe('Anthropic')
    // Picker is independent of provider — text provider unchanged.
    expect((payload as IngestionConfig).provider).toBe('Anthropic')
  })

  it('keeps vision_backend independent of provider (Anthropic text + Ollama vision)', async () => {
    renderHarness({ vision_backend: 'Anthropic' })

    const select = await screen.findByLabelText(/Vision Backend/i)
    fireEvent.change(select, { target: { value: 'Ollama' } })

    fireEvent.click(screen.getByRole('button', { name: /Save Configuration/i }))

    await waitFor(() => expect(saveConfigSpy).toHaveBeenCalledTimes(1))
    const payload = saveConfigSpy.mock.calls[0][0]
    expect((payload as IngestionConfig).provider).toBe('Anthropic')
    expect((payload as IngestionConfig).vision_backend).toBe('Ollama')
  })
})

describe('AiConfigSettings — Web Search key', () => {
  it('fetches key presence on mount and shows the unset state', async () => {
    renderHarness()

    await waitFor(() => expect(getWebSearchKeyStatusSpy).toHaveBeenCalledTimes(1))
    expect(await screen.findByTestId('web-search-key-status-unset')).toBeInTheDocument()
    expect(screen.queryByTestId('web-search-key-status-set')).not.toBeInTheDocument()
  })

  it('shows the configured state when the server reports has_key=true', async () => {
    getWebSearchKeyStatusSpy.mockResolvedValue(apiOk({ has_key: true }))
    renderHarness()

    expect(await screen.findByTestId('web-search-key-status-set')).toBeInTheDocument()
  })

  it('saves a new key, clears the input, and re-fetches presence', async () => {
    renderHarness()
    await waitFor(() => expect(getWebSearchKeyStatusSpy).toHaveBeenCalledTimes(1))

    const input = (await screen.findByTestId('web-search-key-input')) as HTMLInputElement
    fireEvent.change(input, { target: { value: 'BRAVE_TEST_KEY' } })

    // Server flips has_key true after the save.
    getWebSearchKeyStatusSpy.mockResolvedValueOnce(apiOk({ has_key: true }))

    fireEvent.click(screen.getByTestId('web-search-key-save'))

    await waitFor(() => expect(saveWebSearchKeySpy).toHaveBeenCalledWith('BRAVE_TEST_KEY'))
    await waitFor(() => expect(input.value).toBe(''))
    await waitFor(() =>
      expect(screen.getByTestId('web-search-key-status-set')).toBeInTheDocument(),
    )
    // Two calls: initial mount + post-save refresh.
    expect(getWebSearchKeyStatusSpy).toHaveBeenCalledTimes(2)
  })

  it('surfaces an error message when saving fails', async () => {
    saveWebSearchKeySpy.mockRejectedValueOnce(new Error('boom'))
    renderHarness()
    await waitFor(() => expect(getWebSearchKeyStatusSpy).toHaveBeenCalledTimes(1))

    const input = await screen.findByTestId('web-search-key-input')
    fireEvent.change(input, { target: { value: 'BRAVE_TEST_KEY' } })
    fireEvent.click(screen.getByTestId('web-search-key-save'))

    const error = await screen.findByTestId('web-search-key-error')
    expect(error.textContent).toContain('boom')
  })
})
