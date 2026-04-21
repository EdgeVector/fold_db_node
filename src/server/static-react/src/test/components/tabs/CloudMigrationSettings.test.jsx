/**
 * @fileoverview Tests for CloudMigrationSettings Stripe-checkout-return polling.
 *
 * Covers the UI wiring for the Stripe success redirect: after Stripe bounces
 * the user back to `{origin}?subscription=success`, we poll
 * getSubscriptionStatus() every 2s for up to 20s to catch the webhook-driven
 * plan flip. See PR #411's restore-status polling for the pattern template.
 */

import React from 'react'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, act, waitFor } from '@testing-library/react'
import CloudMigrationSettings from '../../../components/tabs/CloudMigrationSettings.jsx'

vi.mock('../../../api/clients/subscriptionClient', () => {
  class CloudApiError extends Error {
    constructor(status, body = '') {
      super(`Cloud API error (${status}): ${body}`)
      this.name = 'CloudApiError'
      this.status = status
      this.body = body
    }
  }
  return {
    getSubscriptionStatus: vi.fn(),
    createCheckoutSession: vi.fn(),
    createPortalSession: vi.fn(),
    formatBytes: (n) => `${n} B`,
    usagePercent: (used, quota) => (quota <= 0 ? 0 : (used / quota) * 100),
    CloudApiError,
  }
})

vi.mock('../../../api/clients/activateExemem', () => ({
  activateExemem: vi.fn(),
}))

import { getSubscriptionStatus, CloudApiError } from '../../../api/clients/subscriptionClient'

function setUrlParam(param) {
  const search = param ? `?subscription=${param}` : ''
  window.history.replaceState({}, '', `/${search}`)
}

function freeStatus() {
  return {
    ok: true,
    plan: 'free',
    storage: { used_bytes: 0, quota_bytes: 1073741824 },
    has_subscription: false,
  }
}

function paidStatus() {
  return {
    ok: true,
    plan: 'paid',
    storage: { used_bytes: 0, quota_bytes: 53687091200 },
    has_subscription: true,
  }
}

beforeEach(() => {
  localStorage.clear()
  // Not in cloud mode by default so the "idle" path renders the simple form.
  global.fetch = vi.fn(async (url) => {
    if (String(url).includes('/api/auth/credentials')) {
      return { ok: true, json: async () => ({ ok: true, has_credentials: false }) }
    }
    return { ok: false, status: 404, json: async () => ({}) }
  })
})

afterEach(() => {
  vi.useRealTimers()
  vi.clearAllMocks()
  window.history.replaceState({}, '', '/')
})

describe('CloudMigrationSettings Stripe checkout return', () => {
  it('idle: renders normally with no banner when no URL param', async () => {
    setUrlParam(null)
    getSubscriptionStatus.mockResolvedValue(freeStatus())

    render(<CloudMigrationSettings />)

    expect(screen.queryByTestId('upgrade-banner-pending')).toBeNull()
    expect(screen.queryByTestId('upgrade-banner-complete')).toBeNull()
    expect(screen.queryByTestId('upgrade-banner-timeout')).toBeNull()
    expect(screen.queryByTestId('upgrade-banner-cancelled')).toBeNull()
    // The "not cloud mode" form renders the Enable Cloud Backup headline
    // synchronously on the initial render (isCloudMode starts false).
    expect(screen.getAllByText(/Enable Cloud Backup/i).length).toBeGreaterThan(0)
  })

  it('success: polls until plan=paid and shows complete banner', async () => {
    vi.useFakeTimers()
    setUrlParam('success')

    getSubscriptionStatus
      .mockResolvedValueOnce(freeStatus())
      .mockResolvedValueOnce(freeStatus())
      .mockResolvedValue(paidStatus())

    render(<CloudMigrationSettings />)

    // Pending banner appears synchronously on mount.
    expect(screen.getByTestId('upgrade-banner-pending')).toBeInTheDocument()

    // URL param cleaned.
    expect(window.location.search).toBe('')

    // Advance ~6 seconds (3 poll ticks at 2s each).
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })

    await waitFor(() =>
      expect(screen.getByTestId('upgrade-banner-complete')).toBeInTheDocument()
    )
    expect(getSubscriptionStatus).toHaveBeenCalled()
    expect(getSubscriptionStatus.mock.calls.length).toBeGreaterThanOrEqual(3)
  })

  it('timeout: shows timeout banner after 20s when plan never flips', async () => {
    vi.useFakeTimers()
    setUrlParam('success')

    getSubscriptionStatus.mockResolvedValue(freeStatus())

    render(<CloudMigrationSettings />)

    expect(screen.getByTestId('upgrade-banner-pending')).toBeInTheDocument()

    // Advance well past 20s, pumping microtasks between ticks.
    for (let i = 0; i < 12; i++) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000)
      })
    }

    await waitFor(() =>
      expect(screen.getByTestId('upgrade-banner-timeout')).toBeInTheDocument()
    )
  })

  it('cancelled: shows cancelled banner and cleans the URL param', async () => {
    setUrlParam('cancelled')

    render(<CloudMigrationSettings />)

    expect(screen.getByTestId('upgrade-banner-cancelled')).toBeInTheDocument()
    expect(window.location.search).toBe('')
  })
})

describe('CloudMigrationSettings partial-creds reset', () => {
  // When localStorage has stale creds from a previous instance and the cloud
  // rejects them (401/403), we must drop back to the invite-code form instead
  // of showing a phantom "Connected but offline" banner — that limbo state
  // causes repeated keychain password prompts every time a handler reads the
  // local credentials file.

  it('auth rejection: clears localStorage, deletes local creds, shows invite form', async () => {
    localStorage.setItem('exemem_api_url', 'https://stale.example')
    localStorage.setItem('exemem_api_key', 'stale-key')

    getSubscriptionStatus.mockRejectedValue(new CloudApiError(401, 'AUTH_FAILED'))

    const fetchMock = vi.fn(async (url, init) => {
      if (String(url).includes('/api/auth/credentials')) {
        if (init && init.method === 'DELETE') {
          return { ok: true, json: async () => ({ ok: true }) }
        }
        return { ok: true, json: async () => ({ ok: true, has_credentials: true }) }
      }
      return { ok: false, status: 404, json: async () => ({}) }
    })
    global.fetch = fetchMock

    render(<CloudMigrationSettings />)

    // Wait for the detect effect's catch branch to actually run.
    await waitFor(() => expect(localStorage.getItem('exemem_api_url')).toBeNull())
    expect(localStorage.getItem('exemem_api_key')).toBeNull()
    expect(
      fetchMock.mock.calls.some(
        ([url, init]) =>
          String(url).includes('/api/auth/credentials') && init && init.method === 'DELETE',
      ),
    ).toBe(true)
    // Invite-code form should be the only state rendered.
    expect(screen.getAllByText(/Enable Cloud Backup/i).length).toBeGreaterThan(0)
  })

  it('transient error: keeps cloud mode and shows offline banner', async () => {
    localStorage.setItem('exemem_api_url', 'https://api.example')
    localStorage.setItem('exemem_api_key', 'good-key')

    getSubscriptionStatus.mockRejectedValue(new Error('network down'))

    global.fetch = vi.fn(async (url) => {
      if (String(url).includes('/api/auth/credentials')) {
        return { ok: true, json: async () => ({ ok: true, has_credentials: true }) }
      }
      return { ok: false, status: 404, json: async () => ({}) }
    })

    render(<CloudMigrationSettings />)

    // Offline banner text is static copy on the cloud-mode render branch.
    await waitFor(() =>
      expect(
        screen.getByText(/couldn't reach the cloud API/i),
      ).toBeInTheDocument(),
    )
    // Stale-creds reset path must NOT fire for transient errors.
    expect(localStorage.getItem('exemem_api_url')).toBe('https://api.example')
    expect(localStorage.getItem('exemem_api_key')).toBe('good-key')
  })

  it('5xx cloud error: keeps cloud mode (not a stale-creds signal)', async () => {
    localStorage.setItem('exemem_api_url', 'https://api.example')
    localStorage.setItem('exemem_api_key', 'good-key')

    getSubscriptionStatus.mockRejectedValue(new CloudApiError(503, 'service unavailable'))

    global.fetch = vi.fn(async (url) => {
      if (String(url).includes('/api/auth/credentials')) {
        return { ok: true, json: async () => ({ ok: true, has_credentials: true }) }
      }
      return { ok: false, status: 404, json: async () => ({}) }
    })

    render(<CloudMigrationSettings />)

    await waitFor(() =>
      expect(
        screen.getByText(/couldn't reach the cloud API/i),
      ).toBeInTheDocument(),
    )
    expect(localStorage.getItem('exemem_api_url')).toBe('https://api.example')
  })
})
