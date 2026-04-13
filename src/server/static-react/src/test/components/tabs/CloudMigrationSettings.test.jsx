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

vi.mock('../../../api/clients/subscriptionClient', () => ({
  getSubscriptionStatus: vi.fn(),
  createCheckoutSession: vi.fn(),
  createPortalSession: vi.fn(),
  formatBytes: (n) => `${n} B`,
  usagePercent: (used, quota) => (quota <= 0 ? 0 : (used / quota) * 100),
}))

vi.mock('../../../api/clients/activateExemem', () => ({
  activateExemem: vi.fn(),
}))

import { getSubscriptionStatus } from '../../../api/clients/subscriptionClient'

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
