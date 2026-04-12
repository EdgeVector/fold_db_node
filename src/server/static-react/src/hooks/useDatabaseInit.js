import { useCallback, useEffect, useState } from 'react'
import { getDatabaseStatus } from '../api/clients/systemClient'
import { getIdentityCard } from '../api/clients/trustClient'

/**
 * Orchestrates database initialization check and onboarding wizard decision.
 *
 * Once authenticated:
 *   1. Fetch /system/database_status
 *   2. If onboarding isn't complete (backend marker + localStorage fallback),
 *      consult the identity card — if a display_name exists, mark complete;
 *      otherwise show the onboarding wizard.
 *   3. If the status endpoint is unavailable (older backend), assume initialized.
 *
 * Returns `recheckDbStatus` so callers (e.g. DatabaseSetupScreen.onComplete)
 * can re-run the status check after first-time setup.
 */
export function useDatabaseInit(isAuthenticated) {
  const [dbStatus, setDbStatus] = useState(null) // { initialized, has_saved_config }
  const [dbStatusLoading, setDbStatusLoading] = useState(true)
  const [showOnboarding, setShowOnboarding] = useState(false)

  const runStatusCheck = useCallback((evaluateOnboarding) => {
    setDbStatusLoading(true)
    getDatabaseStatus()
      .then((response) => {
        if (response.success && response.data) {
          setDbStatus(response.data)
          if (evaluateOnboarding) {
            // Show onboarding if neither backend nor localStorage says it's complete.
            // Backend marker file is authoritative (--empty-db wipes it to reset),
            // but localStorage is the fallback for cases where the backend marker
            // file wasn't written (API call failed, data dir cleaned without reset).
            if (
              !response.data.onboarding_complete &&
              localStorage.getItem('folddb_onboarding_complete') !== '1'
            ) {
              // Final short-circuit: if the node already has an identity card
              // with a display_name, treat setup as complete. This prevents the
              // wizard from re-appearing on reload when the backend marker file
              // is missing but the user has already configured their identity.
              getIdentityCard()
                .then((res) => {
                  const hasIdentity = !!res?.data?.identity_card?.display_name
                  if (hasIdentity) {
                    localStorage.setItem('folddb_onboarding_complete', '1')
                  } else {
                    setShowOnboarding(true)
                  }
                })
                .catch(() => {
                  setShowOnboarding(true)
                })
            }
          }
        } else {
          // If endpoint is unavailable (older backend), assume initialized
          setDbStatus({
            initialized: true,
            has_saved_config: true,
            onboarding_complete: true,
          })
        }
      })
      .catch(() => {
        // If endpoint doesn't exist, assume initialized (backwards compat)
        setDbStatus({
          initialized: true,
          has_saved_config: true,
          onboarding_complete: true,
        })
      })
      .finally(() => setDbStatusLoading(false))
  }, [])

  // Check database status after authenticated
  useEffect(() => {
    if (!isAuthenticated) return
    runStatusCheck(true)
  }, [isAuthenticated, runStatusCheck])

  // Re-check after DatabaseSetupScreen completes (no onboarding evaluation —
  // preserves the exact behavior of the inline handler which only set dbStatus).
  const recheckDbStatus = useCallback(() => {
    setDbStatusLoading(true)
    getDatabaseStatus()
      .then((response) => {
        if (response.success && response.data) {
          setDbStatus(response.data)
        }
      })
      .catch(() => {
        // Assume initialized after successful setup call
        setDbStatus({ initialized: true, has_saved_config: true })
      })
      .finally(() => setDbStatusLoading(false))
  }, [])

  return {
    dbStatus,
    dbStatusLoading,
    showOnboarding,
    setShowOnboarding,
    recheckDbStatus,
  }
}
