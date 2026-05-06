/**
 * Global handler for `503 { error: "node_not_provisioned" }`.
 *
 * Any backend route can return this once the rename-to-provision follow-up
 * lands; the canonical recovery is the onboarding wizard. We attach a single
 * error interceptor to the shared API client that fires a window event the
 * App listens for and flips its onboarding flag.
 *
 * No silent failures — the interceptor re-throws every error after dispatch
 * so existing call-site handling still runs.
 */

import { isApiError } from '../core/errors'
import { getSharedClient } from '../core/client'

export const NODE_NOT_PROVISIONED_EVENT = 'folddb:node_not_provisioned'

/**
 * Detect the canonical "this node hasn't been provisioned yet" response.
 * Matches both the explicit error code and a 503 whose JSON body carries it
 * under common keys (`error`, `code`, or nested `details.error`).
 */
function isNodeNotProvisioned(error: unknown): boolean {
  if (!isApiError(error) || error.status !== 503) return false
  if (error.code === 'node_not_provisioned') return true
  const body = error.response as { error?: unknown; code?: unknown } | undefined
  if (body && typeof body === 'object') {
    if (body.error === 'node_not_provisioned') return true
    if (body.code === 'node_not_provisioned') return true
  }
  const details = error.details as { error?: unknown } | undefined
  if (details && details.error === 'node_not_provisioned') return true
  return false
}

let installed = false

/**
 * Install the interceptor exactly once. Idempotent — safe to call from
 * multiple bootstrapping paths (App mount, tests).
 */
export function installNodeProvisioningHandler(): void {
  if (installed) return
  installed = true

  getSharedClient().addErrorInterceptor((error) => {
    if (isNodeNotProvisioned(error) && typeof window !== 'undefined') {
      window.dispatchEvent(new CustomEvent(NODE_NOT_PROVISIONED_EVENT))
    }
    return error
  })
}

// Test-only escape hatch so suites can re-run installation against a fresh
// shared client in isolation.
export function __resetForTests(): void {
  installed = false
}
