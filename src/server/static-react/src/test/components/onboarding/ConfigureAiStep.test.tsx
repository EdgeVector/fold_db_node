import React, { useState, useCallback } from 'react';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import ConfigureAiStep, { type AiStepFields } from '../../../components/onboarding/ConfigureAiStep';

vi.mock('../../../api/clients', () => ({
  ingestionClient: {
    listOllamaModels: vi.fn(),
  },
}));

import { ingestionClient } from '../../../api/clients';

const mockedClient = vi.mocked(ingestionClient as unknown as {
  listOllamaModels: (url: string) => Promise<unknown>;
});

const apiOk = <T,>(data: T) => ({ success: true as const, data, status: 200 });

function Harness({ initial }: { initial?: Partial<AiStepFields> }) {
  const [fields, setFields] = useState<AiStepFields>({
    provider: 'Anthropic',
    anthropicApiKey: '',
    anthropicModel: 'claude-haiku-4-5-20251001',
    ollamaUrl: 'http://localhost:11434',
    ollamaModel: '',
    ...initial,
  });
  const onChange = useCallback(
    (next: Partial<AiStepFields>) => setFields(prev => ({ ...prev, ...next })),
    [],
  );
  return (
    <ConfigureAiStep fields={fields} onChange={onChange} onNext={vi.fn()} onSkip={vi.fn()} />
  );
}

const renderStep = (initial?: Partial<AiStepFields>) =>
  render(<Harness initial={initial} />);

describe('ConfigureAiStep — Ollama Setup hint', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  const switchToOllama = () => {
    const select = screen.getByTestId('provider-select') as HTMLSelectElement;
    fireEvent.change(select, { target: { value: 'Ollama' } });
  };

  it('shows the friendly confirmation when local Ollama returns models (hides $ ollama pull)', async () => {
    mockedClient.listOllamaModels.mockResolvedValue(
      apiOk({ models: [{ name: 'gemma3:27b', size: 27e9 }, { name: 'llama3.1:8b', size: 8e9 }] }),
    );

    renderStep();
    switchToOllama();

    const detected = await screen.findByTestId('ollama-setup-detected', {}, { timeout: 2000 });
    expect(detected.textContent).toMatch(/Model detected on local Ollama/i);
    expect(detected.textContent).toMatch(/gemma3:27b/);
    expect(screen.queryByTestId('ollama-setup-pull')).toBeNull();
    expect(screen.queryByText(/\$ ollama pull/)).toBeNull();
  });

  it('shows the $ ollama pull command when Ollama is unreachable', async () => {
    mockedClient.listOllamaModels.mockRejectedValue(new Error('fetch failed'));

    renderStep();
    switchToOllama();

    const pull = await screen.findByTestId('ollama-setup-pull', {}, { timeout: 2000 });
    expect(pull.textContent).toMatch(/\$ ollama pull/);
    expect(pull.textContent).toMatch(/llama3\.1:8b/);
    expect(screen.queryByTestId('ollama-setup-detected')).toBeNull();
  });

  it('shows the $ ollama pull command when Ollama responds but has no models installed', async () => {
    mockedClient.listOllamaModels.mockResolvedValue(apiOk({ models: [] }));

    renderStep();
    switchToOllama();

    const pull = await screen.findByTestId('ollama-setup-pull', {}, { timeout: 2000 });
    expect(pull.textContent).toMatch(/\$ ollama pull/);
    expect(screen.queryByTestId('ollama-setup-detected')).toBeNull();
  });

  it('uses the typed model in the pull hint when Ollama is unreachable', async () => {
    mockedClient.listOllamaModels.mockResolvedValue(apiOk({ models: [] }));

    renderStep();
    switchToOllama();

    const modelInput = await waitFor(() => {
      const el = screen.getByTestId('model-select') as HTMLInputElement;
      if (el.tagName !== 'INPUT') throw new Error('still showing dropdown');
      return el;
    }, { timeout: 2000 });

    fireEvent.change(modelInput, { target: { value: 'mistral:7b' } });

    const pull = await screen.findByTestId('ollama-setup-pull');
    expect(pull.textContent).toMatch(/\$ ollama pull mistral:7b/);
  });
});
