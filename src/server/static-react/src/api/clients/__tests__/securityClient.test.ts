import { describe, it, expect, vi } from 'vitest';

import { UnifiedSecurityClient } from '../securityClient';
import type { KeyInfo, SystemKeyResponse } from '../securityClient';
import { ApiClient } from '../../core/client';

describe('UnifiedSecurityClient.getSystemPublicKey', () => {
  function makeClient(getImpl: (..._args: unknown[]) => Promise<unknown>) {
    const mockApi = { get: vi.fn(getImpl) } as unknown as Pick<ApiClient, 'get'>;
    // @ts-expect-error - partial mock
    return { client: new UnifiedSecurityClient(mockApi), mockApi };
  }

  it('parses the success wire shape into SystemKeyResponse', async () => {
    // Wire body produced by GET /api/security/system-key on success.
    // Mirrors fold_db::security::PublicKeyInfo with default serde.
    const wire: SystemKeyResponse = {
      success: true,
      key: {
        id: 'system',
        public_key: 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',
        owner_id: 'node-owner',
        created_at: 1_700_000_000,
        expires_at: null,
        is_active: true,
        permissions: ['read', 'write'],
        metadata: { version: '1' },
      },
    };

    const { client } = makeClient(async () => ({
      success: true,
      data: wire,
      status: 200,
    }));

    const res = await client.getSystemPublicKey();
    expect(res.success).toBe(true);
    expect(res.data?.success).toBe(true);
    const key = res.data?.key as KeyInfo;
    expect(key.id).toBe('system');
    expect(key.public_key.length).toBe(44);
    expect(key.owner_id).toBe('node-owner');
    expect(key.created_at).toBe(1_700_000_000);
    expect(key.expires_at).toBeNull();
    expect(key.is_active).toBe(true);
    expect(key.permissions).toEqual(['read', 'write']);
    expect(key.metadata).toEqual({ version: '1' });
  });

  it('parses the error wire shape (404/500) into SystemKeyResponse', async () => {
    const wire: SystemKeyResponse = {
      success: false,
      error: 'System key not found',
    };

    const { client } = makeClient(async () => ({
      success: true,
      data: wire,
      status: 404,
    }));

    const res = await client.getSystemPublicKey();
    expect(res.data?.success).toBe(false);
    expect(res.data?.error).toBe('System key not found');
    expect(res.data?.key).toBeUndefined();
  });

  it('accepts a numeric expires_at when the key has an expiration', async () => {
    const wire: SystemKeyResponse = {
      success: true,
      key: {
        id: 'system',
        public_key: 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',
        owner_id: 'node-owner',
        created_at: 1_700_000_000,
        expires_at: 1_900_000_000,
        is_active: true,
        permissions: [],
        metadata: {},
      },
    };

    const { client } = makeClient(async () => ({
      success: true,
      data: wire,
      status: 200,
    }));

    const res = await client.getSystemPublicKey();
    expect(res.data?.key?.expires_at).toBe(1_900_000_000);
  });
});
