/**
 * Shared helper for activating Exemem cloud mode.
 *
 * Registers this node with Exemem using an invite code, persists the
 * returned credentials to localStorage, and switches the database over to
 * Exemem cloud storage via `systemClient.applySetup`.
 *
 * Callers are responsible for post-activation behavior (redirect, fetch
 * recovery phrase, reload, etc.) — this helper only covers the shared
 * activation contract so the three UI entry points can't drift.
 *
 * Throws on: empty/whitespace invite code, network failure, non-2xx from
 * /api/auth/register, server-reported registration failure, or an
 * applySetup response whose `success` flag is false. No silent failures.
 */

import { systemClient } from './systemClient';

export interface ActivateExememResult {
  api_url: string;
  api_key: string;
}

export async function activateExemem(
  inviteCode: string,
): Promise<ActivateExememResult> {
  const trimmed = (inviteCode || '').trim();
  if (!trimmed) {
    throw new Error('Invite code is required');
  }

  const resp = await fetch('/api/auth/register', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ invite_code: trimmed }),
  });

  if (!resp.ok) {
    const body = await resp.text().catch(() => '');
    throw new Error(`HTTP ${resp.status}: ${body}`);
  }

  const data = await resp.json();
  if (!data.ok) {
    throw new Error(data.error || 'Registration failed');
  }

  const api_url: string = data.api_url;
  const api_key: string = data.api_key;

  localStorage.setItem('exemem_api_url', api_url);
  localStorage.setItem('exemem_api_key', api_key);

  const response = await systemClient.applySetup({
    storage: {
      type: 'exemem',
      api_url,
      api_key,
    },
  });

  if (response && response.success === false) {
    const message =
      (response as { data?: { message?: string } }).data?.message ||
      'Failed to apply Exemem storage configuration';
    throw new Error(message);
  }

  return { api_url, api_key };
}
