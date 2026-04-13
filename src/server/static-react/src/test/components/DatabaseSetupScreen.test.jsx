/**
 * @fileoverview Tests for DatabaseSetupScreen restore-flow polling.
 *
 * Covers the UI wiring for backend PR #406 — after POST /api/auth/restore
 * succeeds, the UI polls GET /api/auth/restore/status until the background
 * cloud bootstrap is `complete` or `failed` and reflects that in the UI.
 */

import React from 'react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import DatabaseSetupScreen, { pollRestoreStatus } from '../../components/DatabaseSetupScreen.jsx';

vi.mock('../../api/clients/systemClient', () => ({
  applySetup: vi.fn().mockResolvedValue({ success: true }),
}));

const VALID_PHRASE = Array(24).fill('abandon').join(' ');

// Build a fetch mock that returns a queue of responses per URL substring.
function makeFetchMock(queues) {
  return vi.fn(async (url) => {
    for (const key of Object.keys(queues)) {
      if (url.includes(key)) {
        const q = queues[key];
        if (q.length === 0) {
          throw new Error(`No more mock responses for ${url}`);
        }
        const next = q.shift();
        return {
          ok: next.ok !== false,
          status: next.status || 200,
          json: async () => next.body,
        };
      }
    }
    throw new Error(`Unexpected fetch: ${url}`);
  });
}

beforeEach(() => {
  localStorage.clear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('pollRestoreStatus', () => {
  it('resolves when the endpoint reports complete', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce({ ok: true, json: async () => ({ status: 'in_progress' }) })
      .mockResolvedValueOnce({ ok: true, json: async () => ({ status: 'complete' }) });

    await expect(
      pollRestoreStatus({ intervalMs: 1, maxMs: 1000, fetchImpl })
    ).resolves.toBeUndefined();
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it('throws with the server-supplied error when status is failed', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValue({ ok: true, json: async () => ({ status: 'failed', error: 'boom' }) });

    await expect(
      pollRestoreStatus({ intervalMs: 1, maxMs: 1000, fetchImpl })
    ).rejects.toThrow('boom');
  });

  it('throws on network-level failure without swallowing it', async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValue({ ok: false, status: 503, json: async () => ({}) });

    await expect(
      pollRestoreStatus({ intervalMs: 1, maxMs: 1000, fetchImpl })
    ).rejects.toThrow('HTTP 503');
  });
});

describe('DatabaseSetupScreen restore flow', () => {
  it('polls restore/status after POST /api/auth/restore and calls onComplete', async () => {
    global.fetch = makeFetchMock({
      '/api/auth/restore/status': [
        { body: { status: 'complete' } },
      ],
      '/api/auth/restore': [
        { body: { ok: true, api_url: 'https://x', api_key: 'k' } },
      ],
    });

    const onComplete = vi.fn();
    render(<DatabaseSetupScreen onComplete={onComplete} />);

    fireEvent.click(screen.getByText('Restore from recovery phrase'));
    fireEvent.change(screen.getByPlaceholderText(/24-word recovery phrase/i), {
      target: { value: VALID_PHRASE },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Restore' }));

    await waitFor(() => expect(onComplete).toHaveBeenCalled(), { timeout: 5000 });

    // Verify we actually polled the status endpoint, not just the restore POST.
    const statusCalls = global.fetch.mock.calls.filter((c) =>
      String(c[0]).includes('/api/auth/restore/status')
    );
    expect(statusCalls.length).toBeGreaterThanOrEqual(1);
  });

  it('shows the failed-state error and a Try again button when status=failed', async () => {
    global.fetch = makeFetchMock({
      '/api/auth/restore/status': [
        { body: { status: 'failed', error: 'cloud download broke' } },
      ],
      '/api/auth/restore': [
        { body: { ok: true, api_url: 'https://x', api_key: 'k' } },
      ],
    });

    const onComplete = vi.fn();
    render(<DatabaseSetupScreen onComplete={onComplete} />);

    fireEvent.click(screen.getByText('Restore from recovery phrase'));
    fireEvent.change(screen.getByPlaceholderText(/24-word recovery phrase/i), {
      target: { value: VALID_PHRASE },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Restore' }));

    await waitFor(() =>
      expect(screen.getByText(/cloud download broke/i)).toBeInTheDocument()
    );
    expect(screen.getByRole('button', { name: /try again/i })).toBeInTheDocument();
    expect(onComplete).not.toHaveBeenCalled();
  });

  it('does not poll status when the initial restore POST fails', async () => {
    global.fetch = makeFetchMock({
      '/api/auth/restore/status': [],
      '/api/auth/restore': [
        { body: { ok: false, error: 'bad recovery phrase' } },
      ],
    });

    const onComplete = vi.fn();
    render(<DatabaseSetupScreen onComplete={onComplete} />);

    fireEvent.click(screen.getByText('Restore from recovery phrase'));
    fireEvent.change(screen.getByPlaceholderText(/24-word recovery phrase/i), {
      target: { value: VALID_PHRASE },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Restore' }));

    await waitFor(() =>
      expect(screen.getByText(/bad recovery phrase/i)).toBeInTheDocument()
    );
    const statusCalls = global.fetch.mock.calls.filter((c) =>
      String(c[0]).includes('/api/auth/restore/status')
    );
    expect(statusCalls.length).toBe(0);
    expect(onComplete).not.toHaveBeenCalled();
  });
});
