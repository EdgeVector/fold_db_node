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

import { renderWithRedux } from '../../utils/testUtilities.jsx'
import ingestionReducer from '../../../store/ingestionSlice'
import useAiConfig from '../../../components/settings/AiConfigSettings.jsx'
import { ingestionClient } from '../../../api/clients'

// Spy on the singleton so the same object the slice uses is intercepted.
const saveConfigSpy = vi.spyOn(ingestionClient, 'saveConfig')
const getConfigSpy = vi.spyOn(ingestionClient, 'getConfig')
const listModelsSpy = vi.spyOn(ingestionClient, 'listOllamaModels')

/**
 * Tiny harness that wires the `useAiConfig` hook the same way SettingsTab
 * does, and exposes a Save button the test can click.
 */
function Harness() {
  const [status, setStatus] = React.useState(null)
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

const baseConfig = {
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

function renderHarness(overrides = {}) {
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
  saveConfigSpy.mockResolvedValue({ success: true, data: { success: true, message: 'ok' } })
  getConfigSpy.mockResolvedValue({ success: true, data: baseConfig })
  // Stub the Ollama model list so the debounced effect doesn't complain.
  listModelsSpy.mockResolvedValue({ success: true, data: { models: [] } })
})

describe('AiConfigSettings — Vision Backend picker', () => {
  it('initializes the dropdown from savedConfig.vision_backend', async () => {
    renderHarness({ vision_backend: 'Anthropic' })

    const select = await screen.findByLabelText(/Vision Backend/i)
    expect(select.value).toBe('Anthropic')
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
    expect(select.value).toBe('Ollama')
  })

  it('writes the new value into the saveConfig payload when flipped', async () => {
    renderHarness({ vision_backend: 'Ollama' })

    const select = await screen.findByLabelText(/Vision Backend/i)
    fireEvent.change(select, { target: { value: 'Anthropic' } })

    fireEvent.click(screen.getByRole('button', { name: /Save Configuration/i }))

    await waitFor(() => expect(saveConfigSpy).toHaveBeenCalledTimes(1))
    const payload = saveConfigSpy.mock.calls[0][0]
    expect(payload.vision_backend).toBe('Anthropic')
    // Picker is independent of provider — text provider unchanged.
    expect(payload.provider).toBe('Anthropic')
  })

  it('keeps vision_backend independent of provider (Anthropic text + Ollama vision)', async () => {
    renderHarness({ vision_backend: 'Anthropic' })

    const select = await screen.findByLabelText(/Vision Backend/i)
    fireEvent.change(select, { target: { value: 'Ollama' } })

    fireEvent.click(screen.getByRole('button', { name: /Save Configuration/i }))

    await waitFor(() => expect(saveConfigSpy).toHaveBeenCalledTimes(1))
    const payload = saveConfigSpy.mock.calls[0][0]
    expect(payload.provider).toBe('Anthropic')
    expect(payload.vision_backend).toBe('Ollama')
  })
})
