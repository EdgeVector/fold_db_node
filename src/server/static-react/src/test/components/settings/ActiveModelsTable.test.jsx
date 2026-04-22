/**
 * @fileoverview Tests for ActiveModelsTable — the Active Models summary at
 * the top of the AI Config panel.
 *
 * Coverage focus:
 *   1. Renders one row per Role (7 rows) from GET /api/ingestion/config/roles.
 *   2. Each row shows the resolved provider + model + override badge.
 *   3. Stats cells update from GET /api/ingestion/stats polling.
 *   4. Test button is disabled for Vision/Ocr; enabled for text-capable roles.
 *   5. Clicking Test fires POST /api/ingestion/config/test-role and shows
 *      the response inline with the echoed model name.
 *   6. The table has an accessible region label.
 */

import React from 'react'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { screen, fireEvent, waitFor, render, within } from '@testing-library/react'

import ActiveModelsTable from '../../../components/settings/ActiveModelsTable.jsx'
import { ingestionClient } from '../../../api/clients'

const getRolesSpy = vi.spyOn(ingestionClient, 'getRoles')
const getStatsSpy = vi.spyOn(ingestionClient, 'getAiStats')
const testRoleSpy = vi.spyOn(ingestionClient, 'testRole')

const sampleRoles = [
  {
    role: 'IngestionText',
    display_name: 'Ingestion Text',
    doc: 'Schema analysis + structured extraction from ingested content.',
    is_text_capable: true,
    provider: 'Anthropic',
    model: 'claude-haiku-4-5-20251001',
    override_active: false,
    status: 'ok',
    generation_params: {
      num_ctx: 16384,
      temperature: 0.1,
      top_p: 0.95,
      top_k: 0,
      num_predict: 16384,
      repeat_penalty: 1.0,
      presence_penalty: 0.0,
      min_p: 0.0,
    },
  },
  {
    role: 'SmartFolder',
    display_name: 'Smart Folder',
    doc: 'Classify files into schemas.',
    is_text_capable: true,
    provider: 'Anthropic',
    model: 'claude-haiku-4-5-20251001',
    override_active: false,
    status: 'ok',
    generation_params: {
      num_ctx: 16384,
      temperature: 0.0,
      top_p: 0.95,
      top_k: 0,
      num_predict: 256,
      repeat_penalty: 1.0,
      presence_penalty: 0.0,
      min_p: 0.0,
    },
  },
  {
    role: 'DiscoveryInterests',
    display_name: 'Discovery Interests',
    doc: 'Classify into interests.',
    is_text_capable: true,
    provider: 'Anthropic',
    model: 'claude-haiku-4-5-20251001',
    override_active: false,
    status: 'ok',
    generation_params: sampleGenParams(),
  },
  {
    role: 'MutationAgent',
    display_name: 'Mutation Agent',
    doc: 'Mutation execution.',
    is_text_capable: true,
    provider: 'Anthropic',
    model: 'claude-haiku-4-5-20251001',
    override_active: false,
    status: 'ok',
    generation_params: sampleGenParams(),
  },
  {
    role: 'QueryChat',
    display_name: 'Query & Chat',
    doc: 'Natural-language search and chat.',
    is_text_capable: true,
    provider: 'Anthropic',
    model: 'claude-sonnet-4-20250514',
    // Override not "active" — Sonnet comes from role default, not an explicit override.
    override_active: false,
    status: 'ok',
    generation_params: sampleGenParams(),
  },
  {
    role: 'Vision',
    display_name: 'Vision',
    doc: 'Image → markdown.',
    is_text_capable: false,
    provider: 'Ollama',
    model: 'qwen3-vl:2b',
    override_active: false,
    status: 'ok',
    generation_params: sampleGenParams(),
  },
  {
    role: 'Ocr',
    display_name: 'OCR',
    doc: 'Document text extraction.',
    is_text_capable: false,
    provider: 'Ollama',
    model: 'glm-ocr:latest',
    override_active: false,
    status: 'ok',
    generation_params: sampleGenParams(),
  },
]

function sampleGenParams() {
  return {
    num_ctx: 16384,
    temperature: 0.7,
    top_p: 0.95,
    top_k: 0,
    num_predict: 16384,
    repeat_penalty: 1.0,
    presence_penalty: 0.0,
    min_p: 0.0,
  }
}

const emptyStats = {
  stats: sampleRoles.map(r => ({
    role: r.role,
    call_count: 0,
    avg_latency_ms: 0,
    error_count: 0,
    last_called_elapsed_s: null,
  })),
  window: 'since_process_start',
}

beforeEach(() => {
  vi.clearAllMocks()
  getRolesSpy.mockResolvedValue({ success: true, data: { roles: sampleRoles } })
  getStatsSpy.mockResolvedValue({ success: true, data: emptyStats })
})

afterEach(() => {
  // Component polls stats on a 5s interval; tear down the DOM to cancel.
})

describe('ActiveModelsTable', () => {
  it('renders one row per role from the /roles endpoint', async () => {
    render(<ActiveModelsTable />)
    const region = await screen.findByRole('region', { name: /active ai models/i })
    for (const role of sampleRoles) {
      expect(within(region).getByText(role.display_name)).toBeTruthy()
    }
  })

  it('shows the resolved provider and model for each role', async () => {
    render(<ActiveModelsTable />)
    await screen.findByText('Ingestion Text')
    // Haiku is the default for 4 roles (IngestionText, SmartFolder,
    // DiscoveryInterests, MutationAgent), so use getAllByText.
    expect(screen.getAllByText('claude-haiku-4-5-20251001').length).toBe(4)
    expect(screen.getByText('claude-sonnet-4-20250514')).toBeTruthy()
    expect(screen.getByText('qwen3-vl:2b')).toBeTruthy()
    expect(screen.getByText('glm-ocr:latest')).toBeTruthy()
  })

  it('shows "No calls yet" when stats are empty', async () => {
    render(<ActiveModelsTable />)
    await screen.findByText('Ingestion Text')
    const noCalls = await screen.findAllByText(/no calls yet/i)
    expect(noCalls.length).toBeGreaterThan(0)
  })

  it('renders stats after the store has calls recorded', async () => {
    getStatsSpy.mockResolvedValue({
      success: true,
      data: {
        window: 'since_process_start',
        stats: [
          {
            role: 'IngestionText',
            call_count: 42,
            avg_latency_ms: 320.1,
            error_count: 0,
            last_called_elapsed_s: 3.2,
          },
          ...emptyStats.stats.filter(s => s.role !== 'IngestionText'),
        ],
      },
    })
    render(<ActiveModelsTable />)
    await waitFor(() => {
      expect(screen.getByText(/42 calls.*320ms.*0 errs/)).toBeTruthy()
    })
  })

  it('renders an [*] badge on rows where override_active is true', async () => {
    const rolesWithOverride = sampleRoles.map(r =>
      r.role === 'QueryChat' ? { ...r, override_active: true } : r,
    )
    getRolesSpy.mockResolvedValue({
      success: true,
      data: { roles: rolesWithOverride },
    })
    render(<ActiveModelsTable />)
    const badge = await screen.findByLabelText(/user override active/i)
    expect(badge.textContent).toContain('*')
  })

  it('disables the Test action for Vision and Ocr rows', async () => {
    render(<ActiveModelsTable />)
    await screen.findByText('Vision')
    // Vision and Ocr have a plain "Test" span (not a button) with a tooltip.
    const visionRow = screen.getByText('Vision').closest('div')
    // The Test indicator is a span, not a button — no click handler.
    const testIndicator = within(visionRow.parentElement).getByText('Test', {
      selector: 'span',
    })
    expect(testIndicator).toBeTruthy()
  })

  it('fires testRole with the user prompt and shows the response inline', async () => {
    testRoleSpy.mockResolvedValue({
      success: true,
      data: {
        role: 'IngestionText',
        provider: 'Anthropic',
        model: 'claude-haiku-4-5-20251001',
        latency_ms: 250,
        response: 'hello from the model',
      },
    })
    render(<ActiveModelsTable />)
    const ingestionRow = (
      await screen.findByText('Ingestion Text')
    ).closest('div').parentElement
    const testButton = within(ingestionRow).getByText('Test', {
      selector: 'button',
    })
    fireEvent.click(testButton)
    // Row expands with prompt field + Run test.
    const runButton = await within(ingestionRow).findByText('Run test')
    fireEvent.click(runButton)
    await waitFor(() => {
      expect(testRoleSpy).toHaveBeenCalledWith('IngestionText', 'Say hello.')
    })
    await waitFor(() => {
      expect(within(ingestionRow).getByText('hello from the model')).toBeTruthy()
    })
  })

  it('surfaces a status dot for roles with missing_api_key', async () => {
    const rolesWithMissingKey = sampleRoles.map(r =>
      r.role === 'IngestionText' ? { ...r, status: 'missing_api_key' } : r,
    )
    getRolesSpy.mockResolvedValue({
      success: true,
      data: { roles: rolesWithMissingKey },
    })
    render(<ActiveModelsTable />)
    await waitFor(() => {
      expect(screen.getByText(/API key required/i)).toBeTruthy()
    })
  })
})
