import { useEffect, useState } from 'react'
import { useAppDispatch, useAppSelector } from '../store/hooks'
import { restoreSession, autoLogin, loadSystemPublicKey } from '../store/authSlice'
import {
  fetchIngestionConfig,
  selectIngestionConfig,
  selectIsAiConfigured,
  selectAiProvider,
} from '../store/ingestionSlice'
import { BROWSER_CONFIG } from '../constants/config'

/**
 * Orchestrates authentication bootstrap:
 *   1. Restore session from localStorage, or auto-login with node identity
 *   2. Load system public key for display
 *   3. Fetch AI/ingestion config once authenticated
 *
 * Also exposes the setup-banner state (AI-not-configured + not-yet-dismissed).
 */
export function useAuthInitialization() {
  const dispatch = useAppDispatch()
  const { isAuthenticated, isLoading: isAuthLoading } = useAppSelector(
    (state) => state.auth
  )

  // Restore session FIRST - this must run before other effects.
  // Always auto-login with node identity (public key is the sole identity source).
  useEffect(() => {
    const userId = localStorage.getItem(BROWSER_CONFIG.STORAGE_KEYS.USER_ID)
    const userHash = localStorage.getItem(BROWSER_CONFIG.STORAGE_KEYS.USER_HASH)
    if (userId && userHash) {
      dispatch(restoreSession({ id: userId, hash: userHash }))
      return
    }
    dispatch(autoLogin())
  }, [dispatch])

  // Load the system public key for display in Key Management tab
  useEffect(() => {
    dispatch(loadSystemPublicKey())
  }, [dispatch])

  // Fetch AI configuration on mount (after auth)
  useEffect(() => {
    if (isAuthenticated) {
      dispatch(fetchIngestionConfig())
    }
  }, [dispatch, isAuthenticated])

  const ingestionConfig = useAppSelector(selectIngestionConfig)
  const aiConfigured = useAppSelector(selectIsAiConfigured)
  const aiProvider = useAppSelector(selectAiProvider)

  // Setup banner state (persisted dismissal)
  const [setupDismissed, setSetupDismissed] = useState(
    () => localStorage.getItem('folddb_setup_dismissed') === '1'
  )
  const dismissSetup = () => {
    setSetupDismissed(true)
    localStorage.setItem('folddb_setup_dismissed', '1')
  }

  const showSetupBanner =
    isAuthenticated && ingestionConfig !== null && !aiConfigured && !setupDismissed

  return {
    isAuthenticated,
    isAuthLoading,
    ingestionConfig,
    aiConfigured,
    aiProvider,
    showSetupBanner,
    dismissSetup,
  }
}
