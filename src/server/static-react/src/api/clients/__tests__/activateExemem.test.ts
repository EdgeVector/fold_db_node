import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

import { activateExemem } from '../activateExemem';
import { systemClient } from '../systemClient';

describe('activateExemem', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.spyOn(systemClient, 'applySetup').mockResolvedValue({
      success: true,
      data: { success: true, message: 'ok' },
    } as unknown as Awaited<ReturnType<typeof systemClient.applySetup>>);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  function mockFetchOnce(body: unknown, init: { ok?: boolean; status?: number; text?: string } = {}) {
    const { ok = true, status = 200, text } = init;
    const fetchMock = vi.fn(async () => ({
      ok,
      status,
      json: async () => body,
      text: async () => text ?? '',
    }));
    // @ts-expect-error — jsdom fetch override
    global.fetch = fetchMock;
    return fetchMock;
  }

  it('happy path: writes localStorage, calls applySetup, returns creds', async () => {
    const fetchMock = mockFetchOnce({
      ok: true,
      api_url: 'https://cloud.example',
      api_key: 'key-xyz',
    });

    const result = await activateExemem('  EXM-ABCD-1234  ');

    expect(result).toEqual({
      api_url: 'https://cloud.example',
      api_key: 'key-xyz',
    });
    expect(localStorage.getItem('exemem_api_url')).toBe('https://cloud.example');
    expect(localStorage.getItem('exemem_api_key')).toBe('key-xyz');

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/api/auth/register');
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      invite_code: 'EXM-ABCD-1234',
    });

    expect(systemClient.applySetup).toHaveBeenCalledWith({
      storage: {
        type: 'exemem',
        api_url: 'https://cloud.example',
        api_key: 'key-xyz',
      },
    });
  });

  it('throws immediately on empty invite code without fetching', async () => {
    const fetchMock = vi.fn();
    // @ts-expect-error — jsdom fetch override
    global.fetch = fetchMock;

    await expect(activateExemem('   ')).rejects.toThrow(/invite code/i);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(systemClient.applySetup).not.toHaveBeenCalled();
  });

  it('throws with HTTP prefix on non-2xx response', async () => {
    mockFetchOnce(null, { ok: false, status: 500, text: 'boom' });

    await expect(activateExemem('CODE')).rejects.toThrow(/HTTP 500: boom/);
    expect(systemClient.applySetup).not.toHaveBeenCalled();
  });

  it('throws server error message when data.ok is false', async () => {
    mockFetchOnce({ ok: false, error: 'invite expired' });

    await expect(activateExemem('CODE')).rejects.toThrow('invite expired');
    expect(systemClient.applySetup).not.toHaveBeenCalled();
  });

  it('throws when applySetup reports failure', async () => {
    mockFetchOnce({ ok: true, api_url: 'u', api_key: 'k' });
    (systemClient.applySetup as unknown as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      success: false,
      data: { message: 'storage unreachable' },
    });

    await expect(activateExemem('CODE')).rejects.toThrow('storage unreachable');
  });
});
